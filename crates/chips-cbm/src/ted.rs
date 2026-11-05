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
//! Timing: [`Ted::tick`] is one double clock (dc; dot clock / 4, 1.77 MHz
//! PAL) and puts out 4 pixels; a line has 114 of them, counted from the
//! horizontal position `$FF1E` / 2 (column `x`). The line counter
//! (`$FF1C/$FF1D`, raster compare) turns over at `x` = 98 (`$FF1E` = $C4),
//! a line ahead of the picture's line for the rest of it; `tx` = `x` − 98
//! counts from there. On every line `tx` 91–100 are the five DRAM refresh
//! cycles at single clock. On the fetch lines (counter 0–203 with the
//! display on) the whole of `tx` 3–100 is single clock: one cycle in which
//! the clock comes out of double speed, three in which BA is low before a
//! DMA, forty fetch cycles (`tx` 11–90) and the refresh; `tx` 101–2 stay at
//! double speed. A single-clock CPU cycle takes two double clocks starting
//! at an odd `tx`. On the line before a character row and its first line
//! (the attribute and the pointer DMA) BA is low from `tx` 5 and the TED
//! holds the bus (AEC) for `tx` 11–90: the CPU stops at its next read and
//! gets no cycle at all while AEC is low. A CPU write that falls on the
//! sync cycle (`tx` 3) waits (see [`Ted::sync_cycle`]). `$13` bit 1 forces
//! single clock everywhere. Timers count single clocks. Sources: TED System Hardware
//! Manual 4.5.2 (the order and lengths of the DMA line's phases); the
//! positions are measured against VICE's xplus4 with
//! `tools/ted_timing.py` (`tests/tests/ted_timing.rs`).
//!
//! Data bus: [`Ted::bus`] is the byte the TED fetched in the last double
//! clock, if it fetched: in the fetch window of display lines the pattern (or
//! bitmap) byte of the column in the even double clocks, and on a row's
//! first line the attribute byte in the odd ones too. A byte the TED leaves
//! on the bus is what the CPU reads from unconnected addresses (VICE's
//! `Plus4/openio` tests: such reads change with the picture). The order of
//! the fetches within a character is this project's simple model. The tick
//! notes only which cell and line it fetched; the byte is read when asked
//! for, from the registers then (nothing can change them in between without
//! a CPU access, which replaces the bus's byte).
//!
//! Colour registers (`$15–$19`): a CPU write shows in the border from the
//! second pixel of the next double clock; its first pixel shows the old
//! and the new value ORed. The display window is put out 11 double clocks
//! (44 pixels) after the TED composes it, with the registers of that
//! moment, so a write reaches the window 44 pixels further left than the
//! border (both measured against xplus4, `ted_timing` split).
//!
//! Simplifications: the picture is computed from the registers at each
//! pixel (no fetch pipeline); refresh cycles do not slow the CPU; light pen
//! and the `$1A/$1B` character position reload are not modelled.

pub mod gfx;
pub mod overlay;
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
/// The window is put out this many pixels after the TED composes it.
const WINDOW_DELAY: u16 = 44;
/// Frame width: 32 border pixels each side of the 320-pixel window.
pub const WIDTH: u32 = 384;

/// Column (`x`) where the line counter turns over: `tx` 0.
pub const TURN: u16 = 98;
/// `tx` range clocked at single speed on fetch lines (sync, BA, fetch,
/// refresh), the refresh alone on every line, and BA/AEC on DMA lines.
const SINGLE_TX: std::ops::RangeInclusive<u16> = 3..=100;
const REFRESH_TX: std::ops::RangeInclusive<u16> = 91..=100;
const BA_TX: std::ops::RangeInclusive<u16> = 5..=90;
const AEC_TX: std::ops::RangeInclusive<u16> = 11..=90;
/// Last line counter value with fetches (and single clock).
const LAST_FETCH_LINE: u16 = 203;

/// What a double clock gives the CPU, by line kind and `tx`: [`SLOT_CPU`]
/// (it runs a cycle), [`SLOT_BA`] (BA low), [`SLOT_HOLD`] (a write
/// pending at `tx` 3 waits here). Kinds: 0 border, 1 fetch line, 2 DMA
/// line; 3–5 the same with `$13` bit 1 (single clock forced).
const SLOT_CPU: u8 = 1;
const SLOT_BA: u8 = 2;
const SLOT_HOLD: u8 = 4;

const fn in_range(r: &std::ops::RangeInclusive<u16>, v: u16) -> bool {
    v >= *r.start() && v <= *r.end()
}

const fn slot_table(kind: usize) -> [u8; TICKS_PER_LINE as usize] {
    let forced = kind >= 3;
    let base = kind % 3;
    let (fetch, dma) = (base >= 1, base == 2);
    let mut t = [0u8; TICKS_PER_LINE as usize];
    let mut tx = 0u16;
    while tx < TICKS_PER_LINE {
        let aec = dma && in_range(&AEC_TX, tx);
        let ba = dma && in_range(&BA_TX, tx);
        let single = forced || in_range(&REFRESH_TX, tx) || (fetch && in_range(&SINGLE_TX, tx));
        let mut v = 0;
        if !aec && (!single || tx % 2 == 1) {
            v |= SLOT_CPU;
        }
        if ba {
            v |= SLOT_BA;
        }
        // The write hold (see `Ted::sync_cycle`).
        let hold_end = if dma { *REFRESH_TX.start() + 1 } else { 6 };
        if fetch && !forced && tx >= 3 && tx <= hold_end {
            v |= SLOT_HOLD;
        }
        t[tx as usize] = v;
        tx += 1;
    }
    t
}

static SLOTS: [[u8; TICKS_PER_LINE as usize]; 6] = [
    slot_table(0),
    slot_table(1),
    slot_table(2),
    slot_table(3),
    slot_table(4),
    slot_table(5),
];

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
            // The A/V socket of the C16, C116 and Plus/4 carries luminance
            // and chroma separately besides composite (like the C64's 8-pin
            // socket), so a 1702/1084S takes the separated signal.
            output: OutputType::SVideo,
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

/// A fetch the TED made: the cell, its character line, and whether it was
/// the attribute byte (else the pattern or bitmap byte).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fetch {
    cell: u16,
    line: u16,
    attribute: bool,
}

/// What one double clock did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickOut {
    /// The CPU runs a cycle in this double clock.
    pub cpu: bool,
    /// BA is low: the CPU's RDY, it stops at its next read.
    pub ba: bool,
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
    /// The TED's last fetch and whether it was in the last double clock
    /// (derived each tick, not state).
    fetch: Option<Fetch>,
    fetched_now: bool,
    /// Debug overlay record (not machine state; see `crate::overlay`).
    rec: crate::overlay::Recorder<overlay::Record>,
    /// The counter line's kind (derived, see [`Ted::update_kind`]).
    kind: u8,
    /// A colour register write on its way to the picture: register,
    /// value, and whether a double clock has passed (state).
    pending: Option<(u8, u8, bool)>,
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
            fetch: None,
            fetched_now: false,
            rec: crate::overlay::Recorder(None),
            kind: 0,
            pending: None,
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

    #[inline]
    pub fn beam(&self) -> BeamPos {
        BeamPos {
            frame: self.frame,
            // The line counter ($FF1C/$FF1D, raster compare), which runs a
            // line ahead of the picture's line from column TURN.
            line: self.counter_line(),
            cycle: self.tick,
            // Frame column of this double clock's first pixel, clamped to
            // the picture.
            hpos: (self.tick * 4)
                .saturating_sub(FRAME_LEFT)
                .min(WIDTH as u16 - 1),
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
            0x1c => 0xfe | (self.counter_line() >> 8) as u8,
            0x1d => self.counter_line() as u8,
            0x1e => ((self.tick * 4) / 2) as u8,
            0x1f => {
                let flash = ((self.frame >> 1) & 0x0f) as u8;
                0x80 | (flash << 3) | self.char_line()
            }
            0x15..=0x19 => match self.pending {
                Some((r, v, _)) if usize::from(r) == reg => v,
                _ => self.regs[reg],
            },
            _ => self.regs[reg],
        }
    }

    pub fn read(&mut self, reg: u8) -> u8 {
        self.peek(reg)
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        let reg = usize::from(reg & 0x1f);
        if (0x15..=0x19).contains(&reg) {
            // Reaches the picture a double clock later (see the module
            // documentation); a write still on its way lands first.
            if let Some((r, old_v, _)) = self.pending.take() {
                self.regs[usize::from(r)] = old_v;
            }
            self.pending = Some((reg as u8, v, false));
            return;
        }
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
            0x1c => self.set_line((self.counter_line() & 0xff) | (u16::from(v & 1) << 8)),
            0x1d => self.set_line((self.counter_line() & 0x100) | u16::from(v)),
            _ => self.regs[reg] = v,
        }
        if matches!(reg, 0x06 | 0x13 | 0x1c | 0x1d) {
            self.update_kind();
        }
    }

    /// A write to the line counter (the picture's line follows it).
    fn set_line(&mut self, line: u16) {
        let lines = self.timing().lines;
        if line < lines {
            self.line = if self.tick >= TURN {
                (line + lines - 1) % lines
            } else {
                line
            };
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

    /// The line counter as `$FF1C/$FF1D` show it: a line ahead of the
    /// picture's line from column [`TURN`].
    fn counter_line(&self) -> u16 {
        if self.tick >= TURN {
            let next = self.line + 1;
            if next == self.timing().lines {
                0
            } else {
                next
            }
        } else {
            self.line
        }
    }

    /// Double clocks since the counter turned over.
    fn tx(&self) -> u16 {
        let tx = self.tick + (TICKS_PER_LINE - TURN);
        if tx >= TICKS_PER_LINE {
            tx - TICKS_PER_LINE
        } else {
            tx
        }
    }

    /// The kind of the counter's line for [`SLOTS`] (derived; refreshed
    /// when the counter turns and on writes to `$06`, `$13`, `$1C/$1D`).
    fn update_kind(&mut self) {
        let line = self.counter_line();
        let base = if self.dma_line(line) {
            2
        } else if self.fetch_line(line) {
            1
        } else {
            0
        };
        let forced = if self.regs[0x13] & 0x02 != 0 { 3 } else { 0 };
        self.kind = base + forced;
    }

    fn slot_bits(&self) -> u8 {
        SLOTS[usize::from(self.kind)][usize::from(self.tx())]
    }

    /// The TED fetches on this counter line (and clocks the CPU slowly).
    fn fetch_line(&self, line: u16) -> bool {
        self.den() && line <= LAST_FETCH_LINE
    }

    /// The attribute DMA (line before a character row) or the pointer DMA
    /// (its first line) happens on this counter line.
    fn dma_line(&self, line: u16) -> bool {
        if !self.fetch_line(line) {
            return false;
        }
        // Rows start at first_row_line() + 8 r, r = 0..24; the DMA lines
        // are the line before and the first line.
        let Some(d) = (line + 1).checked_sub(self.first_row_line()) else {
            return false;
        };
        d / 8 < 25 && d % 8 <= 1
    }

    /// Whether the CPU gets the coming double clock, and BA, before
    /// [`Ted::tick`] runs it: the machine runs the CPU's cycle first, so it
    /// reads `$FF1E` and the line counter of this double clock.
    pub fn slot(&self) -> (bool, bool) {
        self.cpu_slot()
    }

    /// `tx` 3 of a fetch line is the cycle in which the clock comes out of
    /// double speed. A CPU write that falls on it is held back to `tx` 7,
    /// on a DMA line past the DMA and the first refresh cycle to `tx` 93
    /// (measured against xplus4: four double clocks lost, on a DMA line one
    /// cycle more than its reads; reads go on as usual). Returns `tx`
    /// while such a hold may still run (`tx` 3–6, on a DMA line 3–92);
    /// the machine asks this before running the CPU.
    #[inline]
    pub fn sync_cycle(&self) -> Option<u16> {
        (self.slot_bits() & SLOT_HOLD != 0).then(|| self.tx())
    }

    /// Whether the CPU gets this double clock, and BA.
    #[inline]
    fn cpu_slot(&self) -> (bool, bool) {
        let bits = self.slot_bits();
        (bits & SLOT_CPU != 0, bits & SLOT_BA != 0)
    }

    /// One double clock.
    pub fn tick(&mut self, mem: &dyn TedMemory) -> TickOut {
        let (cpu, ba) = self.cpu_slot();
        // Timers and sound run on the single clock.
        if self.tick % 2 == 1 {
            self.clock_timers();
        }
        self.sound();
        self.draw(mem);
        let fetch = self.fetch_slot();
        self.fetched_now = fetch.is_some();
        if fetch.is_some() {
            self.fetch = fetch;
        }

        self.tick += 1;
        let mut frame_end = false;
        if self.tick == TICKS_PER_LINE {
            if self.rec.0.is_some() {
                self.record_line();
            }
            self.tick = 0;
            self.line += 1;
            if self.line == self.timing().lines {
                self.line = 0;
                self.frame += 1;
                frame_end = true;
            }
        }
        if self.tick == TURN {
            // The counter turns over with the coming double clock; the
            // raster compare flag is there for that double clock's CPU
            // cycle (measured against xplus4, `ted_timing` irqall).
            self.update_kind();
            if self.counter_line() == self.compare_line() {
                self.irq_flags |= 0x02;
            }
        }
        TickOut { cpu, ba, frame_end }
    }

    /// Overlay record at the end of a line: was it a DMA line?
    #[cold]
    #[inline(never)]
    fn record_line(&mut self) {
        let dma = self.dma_line(self.line);
        let (line, last) = (self.line, self.line + 1 == self.timing().lines);
        if let Some(r) = self.rec.0.as_deref_mut() {
            if dma {
                r.lines.set(line);
            }
            if last {
                r.end_field();
            }
        }
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

    /// Whether the TED fetched in the last double clock.
    pub fn fetched(&self) -> bool {
        self.fetched_now
    }

    /// The byte of the TED's last fetch (`None` before its first).
    pub fn bus(&self, mem: &dyn TedMemory) -> Option<u8> {
        let Fetch {
            cell,
            line,
            attribute,
        } = self.fetch?;
        let vm = self.matrix_base();
        if attribute {
            return Some(mem.fetch(vm.wrapping_add(cell), false));
        }
        let rom = self.rom_fetch();
        if self.regs[0x06] & 0x20 != 0 {
            let base = u16::from(self.regs[0x12] & 0x38) << 10;
            return Some(mem.fetch(base.wrapping_add(cell * 8 + line), rom));
        }
        let code = mem.fetch(vm.wrapping_add(0x400 + cell), false);
        let index = if self.regs[0x06] & 0x40 != 0 {
            code & 0x3f
        } else if self.regs[0x07] & 0x80 == 0 {
            code & 0x7f
        } else {
            code
        };
        let charset = u16::from(self.regs[0x13] & 0xfc) << 8;
        Some(mem.fetch(charset.wrapping_add(u16::from(index) * 8 + line), rom))
    }

    /// The fetch of this double clock (see the module documentation).
    fn fetch_slot(&self) -> Option<Fetch> {
        // Fetch and DMA lines (kinds 1, 2, 4, 5).
        let base = self.kind % 3;
        if base == 0 {
            return None;
        }
        let tx = self.tx();
        if !AEC_TX.contains(&tx) {
            return None;
        }
        let row = self.char_row()?;
        let col = ((tx - AEC_TX.start()) / 2).min(39);
        let line = u16::from(self.char_line());
        // The TED's half of a single-clock cycle (even tx); on a DMA line
        // it has the odd ones too (the attribute or pointer fetch).
        let attribute = tx % 2 == 1;
        if attribute && base != 2 {
            return None;
        }
        Some(Fetch {
            cell: row * 40 + col,
            line,
            attribute,
        })
    }

    /// The 4 pixels of this double clock.
    fn draw(&mut self, mem: &dyn TedMemory) {
        // A colour register written a double clock ago takes effect now;
        // this double clock's first pixel shows old | new.
        let mut glitch = None;
        if let Some((r, v, armed)) = self.pending {
            if armed {
                let r = usize::from(r);
                glitch = Some((r, self.regs[r] | v, v));
                self.regs[r] = v;
                self.pending = None;
            } else {
                self.pending = Some((r, v, true));
            }
        }
        let t = self.timing();
        let y = (self.line + t.lines - t.first_line) % t.lines;
        if u32::from(y) >= t.height {
            return;
        }
        let x0 = self.tick * 4;
        let row = u32::from(y) * WIDTH;
        // The window as composed now, 44 pixels to the left.
        for i in 0..4 {
            let Some(x) = (x0 + i).checked_sub(WINDOW_DELAY) else {
                continue;
            };
            if !self.in_window(x) {
                continue;
            }
            let fx = x - FRAME_LEFT;
            let c = self.pixel(mem, x);
            self.pixels[(row + u32::from(fx)) as usize] = c & 0x7f;
        }
        for i in 0..4 {
            let x = x0 + i;
            let Some(fx) = x.checked_sub(FRAME_LEFT) else {
                continue;
            };
            if u32::from(fx) >= WIDTH || self.in_window(x) {
                continue;
            }
            let c = match glitch {
                Some((r, mixed, v)) if i == 0 => {
                    self.regs[r] = mixed;
                    let c = self.pixel(mem, x);
                    self.regs[r] = v;
                    c
                }
                _ => self.pixel(mem, x),
            };
            self.pixels[(row + u32::from(fx)) as usize] = c & 0x7f;
        }
    }

    /// Pixel position `x` (from the line start, 4 a double clock) lies in
    /// the display window.
    fn in_window(&self, x: u16) -> bool {
        let (left, right) = if self.regs[0x07] & 0x08 != 0 {
            (DISPLAY_X, DISPLAY_X + 320)
        } else {
            (DISPLAY_X + 8, DISPLAY_X + 312)
        };
        self.den() && self.in_window_v() && x >= left && x < right
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
                    // Pair 10: matrix bits 0–3 with attribute bits 4–6,
                    // like pair 01 (TED data sheet, sheet 7; Promised Land's
                    // title picture against xplus4).
                    2 => (code & 0x0f) | lum_hi,
                    _ => self.regs[0x16],
                };
            }
            // Hires bitmap (TED 7360 data sheet, sheet 7): a 0 dot takes
            // the colour from matrix bits 0–3 and the luminance from
            // attribute bits 4–6, a 1 dot matrix bits 4–7 and attribute
            // bits 0–2.
            return if pattern >> bit & 1 != 0 {
                (code >> 4) | lum_lo
            } else {
                (code & 0x0f) | lum_hi
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
const VERSION: u16 = 3;

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
            // Version 2: the last fetch (the data bus's byte).
            w.bool(self.fetched_now);
            w.bool(self.fetch.is_some());
            let f = self.fetch.unwrap_or(Fetch {
                cell: 0,
                line: 0,
                attribute: false,
            });
            w.u16(f.cell);
            w.u16(f.line);
            w.bool(f.attribute);
            // Version 3: a colour register write on its way.
            let (r, v, armed) = self.pending.unwrap_or((0, 0, false));
            w.bool(self.pending.is_some());
            w.u8(r);
            w.u8(v);
            w.bool(armed);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
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
        if version >= 2 {
            t.fetched_now = c.bool()?;
            let some = c.bool()?;
            let f = Fetch {
                cell: c.u16()? % 1000,
                line: c.u16()? & 7,
                attribute: c.bool()?,
            };
            t.fetch = some.then_some(f);
        }
        if version >= 3 {
            let some = c.bool()?;
            let r = c.u8()?;
            let v = c.u8()?;
            let armed = c.bool()?;
            if some && !(0x15..=0x19).contains(&r) {
                return Err(StateError::Invalid("TED pending colour register"));
            }
            t.pending = some.then_some((r, v, armed));
        }
        t.samples = std::mem::take(&mut self.samples);
        // The overlay record is not state: keep it switched on, fresh.
        if self.rec.0.is_some() {
            t.set_overlays(&[overlay::BADLINES]);
        }
        t.update_kind();
        *self = t;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
