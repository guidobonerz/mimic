//! MOS 8360 (PAL) and 7360 (NTSC) Text Editing Device of the C16, C116 and
//! Plus/4: video, sound, three timers, raster interrupt and the CPU clock.
//!
//! Sources: Commodore Plus/4 user's guide and "TED 7360R0 preliminary data
//! sheet" (register map), Plus/4 Programmer's Reference (memory map). The
//! implementation is this project's own.
//!
//! | Reg     | Function                                                     |
//! | ------- | ------------------------------------------------------------ |
//! | $00–$05 | timers 1–3 (low, high); writing low stops, high starts       |
//! | $06     | 6 ECM, 5 BMM, 4 DEN, 3 RSEL (25 rows), 0–2 YSCROLL            |
//! | $07     | 7 256 characters (no reverse), 6 NTSC, 4 MCM, 3 CSEL (40 cols), 0–2 XSCROLL |
//! | $08     | keyboard / joystick latch (the machine handles it)           |
//! | $09     | IRQ flags: 1 raster, 3/4/6 timer 1/2/3, 7 any (write 1 clears) |
//! | $0A     | IRQ mask (same bits), 0 raster compare bit 8                 |
//! | $0B     | raster compare bits 0–7                                      |
//! | $0C/$0D | cursor position (10 bits)                                    |
//! | $0E/$0F/$10/$12 | voice 1 / voice 2 frequency (10 bits each)           |
//! | $11     | 7 D/A mode, 6 voice 2 noise, 5 voice 2, 4 voice 1, 0–3 volume |
//! | $12     | 3–5 bitmap base (A13–A15), 2 characters from ROM, 0–1 voice 1 high |
//! | $13     | 2–7 character base (A10–A15), 1 single clock, 0 ROM active (read) |
//! | $14     | 3–7 video matrix base (A11–A15): attributes, characters at +$400 |
//! | $15–$18 | background colours 0–3                                        |
//! | $19     | border colour                                                |
//! | $1C/$1D | raster line (bit 8 in $1C)                                    |
//! | $1E     | horizontal position / 2                                      |
//! | $1F     | 3–6 flash counter, 0–2 character row line                    |
//! | $3E/$3F | write: ROM / RAM at `$8000–$FFFF` (the machine handles it)    |
//!
//! Colours are 7 bits: 0–3 hue, 4–6 luminance. Attribute bytes add bit 7
//! (flash). Character codes ≥ 128 are shown reversed unless `$07` bit 7.
//!
//! Timing: [`Ted::tick`] is one double clock (dot clock / 4, 1.77 MHz PAL)
//! and puts out 4 pixels; a line has 114 of them. The CPU gets every double
//! clock in the borders and every second one (single clock) where the TED
//! fetches (display lines, inside the fetch window) or when `$13` bit 1
//! forces it; on the first line of each character row the TED takes the
//! fetch window completely. Timers count single clocks.
//!
//! Simplifications: the picture is computed from the registers at each
//! pixel (no fetch pipeline); refresh cycles do not slow the CPU; light pen
//! and the `$1A/$1B` character position reload are not modelled.

pub mod palette;

use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use emu_core::{BeamPos, Decimator, Savestate, StateError, StateReader, StateWriter, SAMPLE_RATE};

/// TED type. A savestate only loads into a TED of the same type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    /// PAL.
    #[default]
    Mos8360,
    /// NTSC.
    Mos7360,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Timing {
    pub lines: u16,
    /// Visible lines (full overscan, not cropped); the window wraps past the
    /// last line to line 0.
    pub first_line: u16,
    pub height: u32,
    /// Dot clock; the double CPU clock is a quarter of it.
    pub pixel_clock_hz: u32,
    pub standard: VideoStandard,
}

/// Double clocks per line.
pub const TICKS_PER_LINE: u16 = 114;
const LINE_PIXELS: u16 = TICKS_PER_LINE * 4;

/// Pixel within the line where the 40-column window starts; the frame
/// buffer starts [`FRAME_LEFT`] pixels earlier.
const DISPLAY_X: u16 = 64;
const FRAME_LEFT: u16 = 32;
/// Frame width: 32 border pixels each side of the 320-pixel window.
pub const WIDTH: u32 = 384;

/// Double clocks in which the TED fetches on display lines.
const FETCH_TICKS: std::ops::Range<u16> = 12..96;

/// First and last line of the 25-row window; 24 rows show 8..=199.
const WINDOW_TOP: u16 = 4;
const WINDOW_BOTTOM: u16 = 203;

pub const PAL: Timing = Timing {
    lines: 312,
    first_line: 274,
    height: 284,
    pixel_clock_hz: 7_093_790,
    standard: VideoStandard::Pal,
};

pub const NTSC: Timing = Timing {
    lines: 262,
    first_line: 244,
    height: 240,
    pixel_clock_hz: 7_159_090,
    standard: VideoStandard::Ntsc,
};

impl Model {
    pub fn timing(self) -> &'static Timing {
        match self {
            Model::Mos8360 => &PAL,
            Model::Mos7360 => &NTSC,
        }
    }
}

impl Timing {
    /// Double clock (CPU clock outside the display).
    pub fn double_hz(&self) -> u32 {
        self.pixel_clock_hz / 4
    }

    pub fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: self.pixel_clock_hz,
            active_pixels: WIDTH,
            total_pixels: u32::from(LINE_PIXELS),
            lines_per_field: u32::from(self.lines),
            standard: self.standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// The TED's view of memory.
pub trait TedMemory {
    /// A byte for video: from ROM if `rom` (character / bitmap data with
    /// `$12` bit 2) and the address is in ROM space, else from RAM.
    fn fetch(&self, addr: u16, rom: bool) -> u8;
}

/// What one double clock did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickOut {
    /// The CPU runs a cycle in this double clock.
    pub cpu: bool,
    /// A new field starts with the next tick.
    pub frame_end: bool,
}

/// One voice: a 10-bit counter from the frequency value to 1023 toggles
/// the output (`f = single clock / 8 / (1024 − value)`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Voice {
    counter: u16,
    out: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ted {
    model: Model,
    regs: [u8; 0x20],
    timers: [u16; 3],
    timer_latch: u16,
    timer_running: [bool; 3],
    irq_flags: u8,
    line: u16,
    tick: u16,
    frame: u64,
    voices: [Voice; 2],
    noise: u8,
    sound_prescale: u8,
    resampler: Decimator,
    /// Audio produced but not yet taken (output, not machine state).
    pub samples: Vec<i16>,
    pixels: Vec<u8>,
}

impl Ted {
    pub fn new(model: Model) -> Self {
        let t = model.timing();
        Self {
            model,
            regs: [0; 0x20],
            timers: [0; 3],
            timer_latch: 0,
            timer_running: [false; 3],
            irq_flags: 0,
            line: 0,
            tick: 0,
            frame: 0,
            voices: [Voice::default(); 2],
            noise: 0xff,
            sound_prescale: 0,
            resampler: Decimator::new(t.double_hz(), SAMPLE_RATE).expect("valid rates"),
            samples: Vec::new(),
            pixels: vec![0; (WIDTH * t.height) as usize],
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn timing(&self) -> &'static Timing {
        self.model.timing()
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn beam(&self) -> BeamPos {
        BeamPos {
            frame: self.frame,
            line: self.line,
            cycle: self.tick,
            hpos: (self.tick * 4 + LINE_PIXELS - FRAME_LEFT) % LINE_PIXELS,
        }
    }

    pub fn irq(&self) -> bool {
        self.irq_flags & self.regs[0x0a] & 0x5a != 0
    }

    /// Characters and bitmaps come from ROM (`$12` bit 2).
    pub fn rom_fetch(&self) -> bool {
        self.regs[0x12] & 0x04 != 0
    }

    /// Video matrix base: attributes here, character codes at `+$400`.
    pub fn matrix_base(&self) -> u16 {
        u16::from(self.regs[0x14] & 0xf8) << 8
    }

    fn compare_line(&self) -> u16 {
        (u16::from(self.regs[0x0a] & 1) << 8) | u16::from(self.regs[0x0b])
    }

    /// Side-effect-free register read (`$FF00–$FF1F`; `$08` is the
    /// machine's).
    pub fn peek(&self, reg: u8) -> u8 {
        let reg = usize::from(reg & 0x1f);
        match reg {
            0..=5 => (self.timers[reg / 2] >> (8 * (reg % 2))) as u8,
            0x09 => {
                let any = if self.irq() { 0x80 } else { 0 };
                self.irq_flags | any | 0x25
            }
            0x0a => self.regs[0x0a] | 0xa0,
            0x1c => 0xfe | (self.line >> 8) as u8,
            0x1d => self.line as u8,
            0x1e => ((self.tick * 4) / 2) as u8,
            0x1f => {
                let flash = ((self.frame >> 1) & 0x0f) as u8;
                0x80 | (flash << 3) | self.char_line()
            }
            _ => self.regs[reg],
        }
    }

    pub fn read(&mut self, reg: u8) -> u8 {
        self.peek(reg)
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        let reg = usize::from(reg & 0x1f);
        match reg {
            0..=5 => {
                let t = reg / 2;
                if reg % 2 == 0 {
                    self.timers[t] = (self.timers[t] & 0xff00) | u16::from(v);
                    if t == 0 {
                        self.timer_latch = (self.timer_latch & 0xff00) | u16::from(v);
                    }
                    self.timer_running[t] = false;
                } else {
                    self.timers[t] = (self.timers[t] & 0x00ff) | (u16::from(v) << 8);
                    if t == 0 {
                        self.timer_latch = (self.timer_latch & 0x00ff) | (u16::from(v) << 8);
                        self.timers[0] = self.timer_latch;
                    }
                    self.timer_running[t] = true;
                }
            }
            0x09 => self.irq_flags &= !v,
            0x1c => self.set_line((self.line & 0xff) | (u16::from(v & 1) << 8)),
            0x1d => self.set_line((self.line & 0x100) | u16::from(v)),
            _ => self.regs[reg] = v,
        }
    }

    fn set_line(&mut self, line: u16) {
        if line < self.timing().lines {
            self.line = line;
        }
    }

    fn rows25(&self) -> bool {
        self.regs[0x06] & 0x08 != 0
    }

    fn den(&self) -> bool {
        self.regs[0x06] & 0x10 != 0
    }

    /// First line of character row 0.
    fn first_row_line(&self) -> u16 {
        1 + u16::from(self.regs[0x06] & 7)
    }

    /// Line within the current character row.
    fn char_line(&self) -> u8 {
        (self.line.wrapping_sub(self.first_row_line()) & 7) as u8
    }

    /// Character row of the current line, if inside the rows.
    fn char_row(&self) -> Option<u16> {
        let first = self.first_row_line();
        let row = self.line.checked_sub(first)? / 8;
        (row < 25 && self.line <= WINDOW_BOTTOM).then_some(row)
    }

    fn in_window_v(&self) -> bool {
        let (top, bottom) = if self.rows25() {
            (WINDOW_TOP, WINDOW_BOTTOM)
        } else {
            (WINDOW_TOP + 4, WINDOW_BOTTOM - 4)
        };
        (top..=bottom).contains(&self.line)
    }

    /// Whether the CPU gets this double clock.
    fn cpu_slot(&self) -> bool {
        let display = self.den() && self.char_row().is_some();
        let fetching = display && FETCH_TICKS.contains(&self.tick);
        let row_start = display && self.char_line() == 0;
        if fetching && row_start {
            false
        } else if fetching || self.regs[0x13] & 0x02 != 0 {
            self.tick % 2 == 1
        } else {
            true
        }
    }

    /// One double clock.
    pub fn tick(&mut self, mem: &dyn TedMemory) -> TickOut {
        if self.tick == 0 && self.line == self.compare_line() {
            self.irq_flags |= 0x02;
        }
        let cpu = self.cpu_slot();
        // Timers and sound run on the single clock.
        if self.tick % 2 == 1 {
            self.clock_timers();
        }
        self.sound();
        self.draw(mem);

        self.tick += 1;
        let mut frame_end = false;
        if self.tick == TICKS_PER_LINE {
            self.tick = 0;
            self.line += 1;
            if self.line == self.timing().lines {
                self.line = 0;
                self.frame += 1;
                frame_end = true;
            }
        }
        TickOut { cpu, frame_end }
    }

    fn clock_timers(&mut self) {
        const FLAGS: [u8; 3] = [0x08, 0x10, 0x40];
        for (t, flag) in FLAGS.into_iter().enumerate() {
            if !self.timer_running[t] {
                continue;
            }
            if self.timers[t] == 0 {
                self.irq_flags |= flag;
                // Timer 1 reloads its latch; timers 2 and 3 wrap.
                self.timers[t] = if t == 0 { self.timer_latch } else { 0xffff };
            } else {
                self.timers[t] -= 1;
            }
        }
    }

    fn sound(&mut self) {
        self.sound_prescale += 1;
        if self.sound_prescale == 8 {
            self.sound_prescale = 0;
            let freqs = [
                u16::from(self.regs[0x0e]) | (u16::from(self.regs[0x12] & 3) << 8),
                u16::from(self.regs[0x0f]) | (u16::from(self.regs[0x10] & 3) << 8),
            ];
            for (i, v) in self.voices.iter_mut().enumerate() {
                v.counter += 1;
                if v.counter >= 1024 {
                    v.counter = freqs[i];
                    v.out = !v.out;
                    if i == 1 {
                        // 8-bit LFSR (taps 8, 6, 5, 4) for the noise.
                        let bit = (self.noise
                            ^ (self.noise >> 2)
                            ^ (self.noise >> 3)
                            ^ (self.noise >> 4))
                            & 1;
                        self.noise = (self.noise >> 1) | (bit << 7);
                    }
                }
            }
        }
        let ctrl = self.regs[0x11];
        let da = ctrl & 0x80 != 0;
        let mut level = 0i32;
        if ctrl & 0x10 != 0 && (da || self.voices[0].out) {
            level += 1;
        }
        // Voice 2 plays its square wave, or noise if only bit 6 is set.
        let voice2 = if ctrl & 0x20 != 0 {
            self.voices[1].out
        } else {
            self.noise & 1 != 0
        };
        if ctrl & 0x60 != 0 && (da || voice2) {
            level += 1;
        }
        let volume = i32::from((ctrl & 0x0f).min(8));
        let s = level * volume * 1500 - 8 * 1500;
        if let Some(out) = self.resampler.push(s) {
            // Bound the backlog if nobody drains it.
            if self.samples.len() < SAMPLE_RATE as usize * 4 {
                self.samples.push(out);
            }
        }
    }

    /// The 4 pixels of this double clock.
    fn draw(&mut self, mem: &dyn TedMemory) {
        let t = self.timing();
        let y = (self.line + t.lines - t.first_line) % t.lines;
        if u32::from(y) >= t.height {
            return;
        }
        let x0 = self.tick * 4;
        for i in 0..4 {
            let x = x0 + i;
            let Some(fx) = x.checked_sub(FRAME_LEFT) else {
                continue;
            };
            if u32::from(fx) >= WIDTH {
                continue;
            }
            let c = self.pixel(mem, x);
            self.pixels[(u32::from(y) * WIDTH + u32::from(fx)) as usize] = c & 0x7f;
        }
    }

    fn pixel(&self, mem: &dyn TedMemory, x: u16) -> u8 {
        let border = self.regs[0x19];
        let (left, right) = if self.regs[0x07] & 0x08 != 0 {
            (DISPLAY_X, DISPLAY_X + 320)
        } else {
            (DISPLAY_X + 8, DISPLAY_X + 312)
        };
        if !self.den() || !self.in_window_v() || x < left || x >= right {
            return border;
        }
        let bg0 = self.regs[0x15];
        let Some(row) = self.char_row() else {
            return bg0;
        };
        let xs = x - DISPLAY_X;
        let Some(px) = xs.checked_sub(u16::from(self.regs[0x07] & 7)) else {
            return bg0;
        };
        let col = px / 8;
        if col >= 40 {
            return bg0;
        }
        let bit = 7 - (px % 8) as u8;
        let cell = row * 40 + col;
        let vm = self.matrix_base();
        let attr = mem.fetch(vm.wrapping_add(cell), false);
        let code = mem.fetch(vm.wrapping_add(0x400 + cell), false);
        let line = u16::from(self.char_line());
        let ecm = self.regs[0x06] & 0x40 != 0;
        let bmm = self.regs[0x06] & 0x20 != 0;
        let mcm = self.regs[0x07] & 0x10 != 0;
        let rom = self.rom_fetch();

        if bmm {
            let base = u16::from(self.regs[0x12] & 0x38) << 10;
            let pattern = mem.fetch(base.wrapping_add(cell * 8 + line), rom);
            let lum_hi = attr & 0x70;
            let lum_lo = (attr & 0x07) << 4;
            if mcm {
                let pair = (pattern >> (bit & 6)) & 3;
                return match pair {
                    0 => bg0,
                    1 => (code >> 4) | lum_hi,
                    2 => (code & 0x0f) | lum_lo,
                    _ => self.regs[0x16],
                };
            }
            return if pattern >> bit & 1 != 0 {
                (code & 0x0f) | lum_lo
            } else {
                (code >> 4) | lum_hi
            };
        }

        let charset = u16::from(self.regs[0x13] & 0xfc) << 8;
        let reverse_allowed = self.regs[0x07] & 0x80 == 0 && !ecm;
        let index = if ecm {
            code & 0x3f
        } else if reverse_allowed {
            code & 0x7f
        } else {
            code
        };
        let mut pattern = mem.fetch(charset.wrapping_add(u16::from(index) * 8 + line), rom);
        let flash_on = (self.frame >> 4) & 1 == 0;
        if attr & 0x80 != 0 && !flash_on {
            pattern = 0;
        }
        let mut inverse = reverse_allowed && code & 0x80 != 0;
        let cursor = (u16::from(self.regs[0x0c] & 3) << 8) | u16::from(self.regs[0x0d]);
        if cell == cursor && flash_on {
            inverse = !inverse;
        }
        if inverse {
            pattern = !pattern;
        }
        let fg = attr & 0x7f;
        if mcm && attr & 0x08 != 0 && !ecm {
            let pair = (pattern >> (bit & 6)) & 3;
            return match pair {
                0 => bg0,
                1 => self.regs[0x16],
                2 => self.regs[0x17],
                _ => fg & 0x77,
            };
        }
        if pattern >> bit & 1 != 0 {
            fg
        } else if ecm {
            self.regs[0x15 + usize::from(code >> 6)]
        } else {
            bg0
        }
    }
}

const TAG: [u8; 4] = *b"TED1";
const VERSION: u16 = 1;

impl Savestate for Ted {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(match self.model {
                Model::Mos8360 => 0,
                Model::Mos7360 => 1,
            });
            w.raw(&self.regs);
            for t in self.timers {
                w.u16(t);
            }
            w.u16(self.timer_latch);
            for r in self.timer_running {
                w.bool(r);
            }
            w.u8(self.irq_flags);
            w.u16(self.line);
            w.u16(self.tick);
            w.u64(self.frame);
            for v in &self.voices {
                w.u16(v.counter);
                w.bool(v.out);
            }
            w.u8(self.noise);
            w.u8(self.sound_prescale);
            self.resampler.save(w);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let model = match c.u8()? {
            0 => Model::Mos8360,
            1 => Model::Mos7360,
            _ => return Err(StateError::Invalid("TED model")),
        };
        if model != self.model {
            return Err(StateError::Invalid("TED model differs"));
        }
        let mut t = Ted::new(model);
        c.raw_into(&mut t.regs)?;
        for timer in &mut t.timers {
            *timer = c.u16()?;
        }
        t.timer_latch = c.u16()?;
        for r in &mut t.timer_running {
            *r = c.bool()?;
        }
        t.irq_flags = c.u8()? & 0x5a;
        t.line = c.u16()?;
        t.tick = c.u16()?;
        if t.line >= model.timing().lines || t.tick >= TICKS_PER_LINE {
            return Err(StateError::Invalid("TED beam position"));
        }
        t.frame = c.u64()?;
        for v in &mut t.voices {
            v.counter = c.u16()?.min(1023);
            v.out = c.bool()?;
        }
        t.noise = c.u8()?;
        t.sound_prescale = c.u8()? % 8;
        t.resampler.load(&mut c)?;
        c.raw_into(&mut t.pixels)?;
        for p in &mut t.pixels {
            *p &= 0x7f;
        }
        t.samples = std::mem::take(&mut self.samples);
        *self = t;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
