//! MOS 6561 (PAL) and 6560 (NTSC) Video Interface Chip of the VIC-20.
//!
//! Sources: VIC-20 Programmer's Reference Guide (register map, colours,
//! sound), MOS 6560/6561 data sheet (timing). The implementation is this
//! project's own.
//!
//! The VIC reads memory in φ1, so it never stops the CPU. It shows a
//! character matrix of up to 31 × 46 cells (8 × 8 or 8 × 16 pixels) inside
//! a border. Every CPU cycle it puts out 4 pixels; a character takes two
//! cycles (code and colour, then the pattern).
//!
//! | Reg | Function                                                          |
//! | --- | ----------------------------------------------------------------- |
//! | $0  | 7 interlace, 0–6 horizontal origin (units of 4 pixels = 1 cycle)  |
//! | $1  | vertical origin (units of 2 lines)                                |
//! | $2  | 7 VM9 (screen and colour RAM address bit 9), 0–6 columns          |
//! | $3  | 7 raster bit 0 (read), 1–6 rows, 0 8 × 16 characters              |
//! | $4  | raster bits 8–1 (read)                                            |
//! | $5  | 4–7 VM13–VM10 screen address, 0–3 CB13–CB10 character address     |
//! | $6/$7 | light pen X / Y                                                 |
//! | $8/$9 | paddle X / Y (POTX / POTY)                                      |
//! | $A–$C | bass, alto, soprano: 7 on, 0–6 frequency                        |
//! | $D  | noise: 7 on, 0–6 frequency                                        |
//! | $E  | 4–7 auxiliary colour, 0–3 volume                                  |
//! | $F  | 4–7 background colour, 3 normal (1) / reverse (0), 0–2 border     |
//!
//! Colour RAM (4 bits per cell): 0–2 character colour, 3 multicolour. In
//! multicolour cells pixel pairs pick background (00), border (01),
//! character (10) or auxiliary colour (11).
//!
//! The VIC sees 16 KB: its `$0000–$1FFF` are the CPU's `$8000–$9FFF`
//! (character ROM), its `$2000–$3FFF` the CPU's `$0000–$1FFF` (RAM). The
//! machine provides that view through [`VicMemory`].
//!
//! Simplifications: interlace is stored but not drawn; a character is
//! fetched in one go at its first cycle; light pen and paddles read fixed
//! values (no device connected).

pub mod palette;

use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use emu_core::{BeamPos, Decimator, Savestate, StateError, StateReader, StateWriter, SAMPLE_RATE};

/// VIC type. A savestate only loads into a VIC of the same type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    /// PAL-B.
    #[default]
    Mos6561,
    /// NTSC-M.
    Mos6560,
}

/// Line and frame geometry of a VIC type.
#[derive(Debug, PartialEq, Eq)]
pub struct Timing {
    pub lines: u16,
    pub cycles: u8,
    /// Visible lines of the frame buffer (full overscan, not cropped).
    pub first_line: u16,
    pub height: u32,
    /// Frame column of the first pixel of cycle 0 (a multiple of 4). The
    /// line is shown rotated by this many pixels so the KERNAL's text
    /// window sits in the middle like on a monitor (the end of the line
    /// wraps to the left border).
    pub x_offset: u32,
    /// Dot clock; the CPU clock is a quarter of it.
    pub pixel_clock_hz: u32,
    pub standard: VideoStandard,
}

/// 6561: 312 lines of 71 cycles, 4.433 618 MHz dot clock.
pub const PAL: Timing = Timing {
    lines: 312,
    cycles: 71,
    first_line: 28,
    height: 284,
    x_offset: 8,
    pixel_clock_hz: 4_433_618,
    standard: VideoStandard::Pal,
};

/// 6560: 261 lines of 65 cycles, 4.090 909 MHz dot clock.
pub const NTSC: Timing = Timing {
    lines: 261,
    cycles: 65,
    first_line: 22,
    height: 239,
    x_offset: 24,
    pixel_clock_hz: 4_090_909,
    standard: VideoStandard::Ntsc,
};

impl Model {
    pub fn timing(self) -> &'static Timing {
        match self {
            Model::Mos6561 => &PAL,
            Model::Mos6560 => &NTSC,
        }
    }
}

impl Timing {
    /// CPU clock (dot clock / 4).
    pub fn cpu_hz(&self) -> u32 {
        self.pixel_clock_hz / 4
    }

    /// Frame width: the whole line, 4 pixels per cycle.
    pub fn width(&self) -> u32 {
        u32::from(self.cycles) * 4
    }

    pub fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: self.pixel_clock_hz,
            active_pixels: self.width(),
            total_pixels: self.width(),
            lines_per_field: u32::from(self.lines),
            standard: self.standard,
            output: OutputType::Composite,
            field: FieldParity::Progressive,
        }
    }
}

/// The VIC's 14-bit view of memory.
pub trait VicMemory {
    /// A byte at a VIC address (`$0000–$3FFF`).
    fn read(&self, addr: u16) -> u8;
    /// The colour RAM nibble at `index` (`$000–$3FF`).
    fn color(&self, index: u16) -> u8;
}

/// Prescaler of each voice in CPU cycles per counter step: bass, alto,
/// soprano, noise (each an octave above the previous).
const VOICE_PRESCALE: [u16; 4] = [128, 64, 32, 16];

/// One sound channel: a 7-bit counter from the frequency value to 127
/// toggles the output when it overflows (square wave of
/// `clock / (2 × prescale × (128 − value))`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Voice {
    prescale: u16,
    counter: u8,
    out: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vic {
    model: Model,
    regs: [u8; 16],
    line: u16,
    cycle: u8,
    frame: u64,
    /// Character rows are being displayed in this frame.
    rows_active: bool,
    row: u8,
    char_line: u8,
    /// Character columns are being displayed in this line.
    cols_active: bool,
    col: u8,
    /// Second half of the current character: pattern and colours.
    pending: Option<[u8; 4]>,
    last_bus: u8,
    pixels: Vec<u8>,
    /// Paddle resistances on POTX/POTY (0 = none, 65535 = open), set by
    /// the machine each cycle.
    pots: [u16; 2],
    voices: [Voice; 4],
    noise: u16,
    resampler: Decimator,
    /// Audio produced but not yet taken (output, not machine state).
    pub samples: Vec<i16>,
}

impl Vic {
    pub fn new(model: Model) -> Self {
        let t = model.timing();
        Self {
            model,
            regs: [0; 16],
            line: 0,
            cycle: 0,
            frame: 0,
            rows_active: false,
            row: 0,
            char_line: 0,
            cols_active: false,
            col: 0,
            pending: None,
            last_bus: 0xff,
            pixels: vec![0; (t.width() * t.height) as usize],
            pots: [0xffff; 2],
            voices: [Voice::default(); 4],
            noise: 0xffff,
            resampler: Decimator::new(t.cpu_hz(), SAMPLE_RATE).expect("valid rates"),
            samples: Vec::new(),
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn timing(&self) -> &'static Timing {
        self.model.timing()
    }

    pub fn beam(&self) -> BeamPos {
        BeamPos {
            frame: self.frame,
            line: self.line,
            cycle: u16::from(self.cycle),
            hpos: ((u32::from(self.cycle) * 4 + self.timing().x_offset) % self.timing().width())
                as u16,
        }
    }

    /// The byte of the VIC's last memory access (the CPU sees it on open
    /// bus addresses).
    pub fn last_bus(&self) -> u8 {
        self.last_bus
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Screen memory start as a VIC address (VM13–VM9).
    pub fn screen_addr(&self) -> u16 {
        (u16::from(self.regs[5] >> 4) << 10) | (u16::from(self.regs[2] & 0x80) << 2)
    }

    fn char_addr(&self) -> u16 {
        u16::from(self.regs[5] & 0x0f) << 10
    }

    pub fn columns(&self) -> u8 {
        self.regs[2] & 0x7f
    }

    pub fn rows(&self) -> u8 {
        (self.regs[3] >> 1) & 0x3f
    }

    fn char_height(&self) -> u8 {
        if self.regs[3] & 1 != 0 {
            16
        } else {
            8
        }
    }

    /// Side-effect-free register read (reads have no side effects anyway).
    pub fn peek(&self, reg: u8) -> u8 {
        let reg = reg & 0x0f;
        match reg {
            3 => (self.regs[3] & 0x7f) | (((self.line & 1) as u8) << 7),
            4 => (self.line >> 1) as u8,
            // No light pen; the pot counters stop where the paddles' RC
            // network charges (255: nothing connected).
            6 | 7 => 0,
            8 | 9 => (self.pots[usize::from(reg - 8)] >> 8) as u8,
            _ => self.regs[usize::from(reg)],
        }
    }

    pub fn read(&mut self, reg: u8) -> u8 {
        self.peek(reg)
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        self.regs[usize::from(reg & 0x0f)] = v;
    }

    /// Paddle resistances on POTX/POTY (0 = none, 65535 = open).
    pub fn set_pots(&mut self, pots: [u16; 2]) {
        self.pots = pots;
    }

    fn color_of(&self, kind: u8, char_color: u8) -> u8 {
        match kind {
            0 => self.regs[15] >> 4,
            1 => self.regs[15] & 7,
            2 => char_color,
            _ => self.regs[14] >> 4,
        }
    }

    /// The 8 pixel colours of a character from its pattern and colour.
    fn character(&self, pattern: u8, color: u8) -> [u8; 8] {
        let mut px = [0; 8];
        if color & 8 != 0 {
            for i in 0..4 {
                let kind = (pattern >> (6 - 2 * i)) & 3;
                let c = self.color_of(kind, color & 7);
                px[2 * i] = c;
                px[2 * i + 1] = c;
            }
        } else {
            // Bit 3 of $900F clear: reverse mode swaps the two colours.
            let pattern = if self.regs[15] & 8 == 0 {
                !pattern
            } else {
                pattern
            };
            for (i, p) in px.iter_mut().enumerate() {
                *p = if pattern & (0x80 >> i) != 0 {
                    color & 7
                } else {
                    self.regs[15] >> 4
                };
            }
        }
        px
    }

    /// One CPU cycle: memory fetches, 4 pixels, sound, beam advance.
    /// Returns true when a new field starts.
    pub fn tick(&mut self, mem: &dyn VicMemory) -> bool {
        let t = self.timing();
        if self.cycle == 0 {
            self.start_line();
        }

        // Horizontal window: starts at the origin cycle, two cycles per column.
        if self.rows_active && !self.cols_active && self.cycle == self.regs[0] & 0x7f {
            self.cols_active = self.columns() > 0;
            self.col = 0;
        }
        let four = if let Some(rest) = self.pending.take() {
            rest
        } else if self.cols_active {
            let cell =
                (u16::from(self.row) * u16::from(self.columns()) + u16::from(self.col)) & 0x3fff;
            let vm = (self.screen_addr() + cell) & 0x3fff;
            let code = mem.read(vm);
            let color = mem.color(vm & 0x3ff) & 0x0f;
            let height = u16::from(self.char_height());
            let pattern = mem.read(
                (self.char_addr() + u16::from(code) * height + u16::from(self.char_line)) & 0x3fff,
            );
            self.last_bus = pattern;
            let px = self.character(pattern, color);
            self.col += 1;
            if self.col >= self.columns() {
                self.cols_active = false;
            }
            self.pending = Some([px[4], px[5], px[6], px[7]]);
            [px[0], px[1], px[2], px[3]]
        } else {
            [self.regs[15] & 7; 4]
        };
        if let Some(y) = self.line.checked_sub(t.first_line) {
            if u32::from(y) < t.height {
                let x = (u32::from(self.cycle) * 4 + t.x_offset) % t.width();
                let at = (u32::from(y) * t.width() + x) as usize;
                self.pixels[at..at + 4].copy_from_slice(&four);
            }
        }

        self.sound();

        self.cycle += 1;
        if self.cycle == t.cycles {
            self.cycle = 0;
            self.end_line();
            self.line += 1;
            if self.line == t.lines {
                self.line = 0;
                self.frame += 1;
                return true;
            }
        }
        false
    }

    fn start_line(&mut self) {
        self.cols_active = false;
        self.pending = None;
        if self.line == u16::from(self.regs[1]) * 2 && !self.rows_active {
            self.rows_active = self.rows() > 0;
            self.row = 0;
            self.char_line = 0;
        }
    }

    fn end_line(&mut self) {
        if self.rows_active {
            self.char_line += 1;
            if self.char_line >= self.char_height() {
                self.char_line = 0;
                self.row += 1;
                if self.row >= self.rows() {
                    self.rows_active = false;
                }
            }
        }
        if self.line + 1 == self.timing().lines {
            self.rows_active = false;
        }
    }

    fn sound(&mut self) {
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.prescale += 1;
            if v.prescale < VOICE_PRESCALE[i] {
                continue;
            }
            v.prescale = 0;
            let reg = self.regs[0x0a + i];
            if reg & 0x80 == 0 {
                continue;
            }
            v.counter += 1;
            if v.counter >= 0x80 {
                v.counter = reg & 0x7f;
                v.out = !v.out;
                if i == 3 && v.out {
                    // 16-bit LFSR (taps 16, 14, 13, 11) clocks the noise.
                    let bit =
                        (self.noise ^ (self.noise >> 2) ^ (self.noise >> 3) ^ (self.noise >> 5))
                            & 1;
                    self.noise = (self.noise >> 1) | (bit << 15);
                }
            }
        }
        let mut level = 0i32;
        for (i, v) in self.voices.iter().enumerate() {
            let on = self.regs[0x0a + i] & 0x80 != 0;
            let high = if i == 3 { self.noise & 1 != 0 } else { v.out };
            if on && high {
                level += 1;
            }
        }
        let volume = i32::from(self.regs[14] & 0x0f);
        let s = level * volume * 512 - 4 * 15 * 256;
        if let Some(out) = self.resampler.push(s) {
            // Bound the backlog if nobody drains it.
            if self.samples.len() < SAMPLE_RATE as usize * 4 {
                self.samples.push(out);
            }
        }
    }
}

const TAG: [u8; 4] = *b"VIC1";
const VERSION: u16 = 1;

impl Savestate for Vic {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(match self.model {
                Model::Mos6561 => 0,
                Model::Mos6560 => 1,
            });
            w.raw(&self.regs);
            w.u16(self.line);
            w.u8(self.cycle);
            w.u64(self.frame);
            w.bool(self.rows_active);
            w.u8(self.row);
            w.u8(self.char_line);
            w.bool(self.cols_active);
            w.u8(self.col);
            w.bool(self.pending.is_some());
            w.raw(&self.pending.unwrap_or_default());
            w.u8(self.last_bus);
            for v in &self.voices {
                w.u16(v.prescale);
                w.u8(v.counter);
                w.bool(v.out);
            }
            w.u16(self.noise);
            self.resampler.save(w);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let model = match c.u8()? {
            0 => Model::Mos6561,
            1 => Model::Mos6560,
            _ => return Err(StateError::Invalid("VIC model")),
        };
        if model != self.model {
            return Err(StateError::Invalid("VIC model differs"));
        }
        let t = model.timing();
        let mut v = Vic::new(model);
        c.raw_into(&mut v.regs)?;
        v.line = c.u16()?;
        v.cycle = c.u8()?;
        if v.line >= t.lines || v.cycle >= t.cycles {
            return Err(StateError::Invalid("VIC beam position"));
        }
        v.frame = c.u64()?;
        v.rows_active = c.bool()?;
        v.row = c.u8()?;
        v.char_line = c.u8()?;
        v.cols_active = c.bool()?;
        v.col = c.u8()?;
        let pending = c.bool()?;
        let mut rest = [0u8; 4];
        c.raw_into(&mut rest)?;
        v.pending = pending.then_some(rest.map(|p| p & 0x0f));
        v.last_bus = c.u8()?;
        for voice in &mut v.voices {
            voice.prescale = c.u16()?;
            voice.counter = c.u8()?;
            voice.out = c.bool()?;
        }
        v.noise = c.u16()?;
        v.resampler.load(&mut c)?;
        c.raw_into(&mut v.pixels)?;
        for p in &mut v.pixels {
            *p &= 0x0f;
        }
        v.samples = std::mem::take(&mut self.samples);
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
