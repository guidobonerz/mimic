//! ANTIC (CO12296): the display-list processor of the Atari 8-bit
//! computers. It fetches the display list, playfield and player/missile
//! data by DMA, stopping the 6502C with HALT for every cycle it uses, and
//! hands GTIA one playfield value per colour clock.
//!
//! Registers (`$D400`, mirrored every 16 bytes up to `$D4FF`):
//!
//! | Reg | Write                         | Read        |
//! | --- | ----------------------------- | ----------- |
//! | 0   | DMACTL                        | –           |
//! | 1   | CHACTL                        | –           |
//! | 2/3 | DLISTL / DLISTH               | –           |
//! | 4   | HSCROL                        | –           |
//! | 5   | VSCROL                        | –           |
//! | 7   | PMBASE                        | –           |
//! | 9   | CHBASE                        | –           |
//! | A   | WSYNC                         | –           |
//! | B   | –                             | VCOUNT      |
//! | C/D | –                             | PENH / PENV |
//! | E   | NMIEN                         | –           |
//! | F   | NMIRES                        | NMIST       |
//!
//! DMACTL: bits 0–1 playfield width (0 off, 1 narrow, 2 normal, 3 wide),
//! 2 missile DMA, 3 player DMA (also fetches missiles), 4 single-line
//! player resolution, 5 display-list DMA. NMIEN/NMIST: 7 DLI, 6 VBI,
//! 5 RESET key (400/800 only).
//!
//! Timing (Altirra Hardware Reference Manual, "ANTIC"; De Re Atari, ch. 2):
//! 114 cycles per scan line, two colour clocks per cycle; display lines
//! 8–247, vertical blank from line 248 (VBI NMI at cycle 7), DLI at cycle
//! 7 of the last line of a mode line, WSYNC releases the CPU at cycle 105.
//! Fixed DMA slots: missiles cycle 0, display-list instruction cycle 1,
//! players cycles 2–5, LMS/JMP address cycles 6–7, nine refresh cycles
//! from cycle 25 every 4 cycles (pushed back by playfield fetches).
//! Playfield fetches run a few cycles ahead of the colour clocks they
//! show: a character name 3 cycles, a data byte 2 cycles before.

use emu_core::frame::VideoStandard;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const CYCLES_PER_LINE: u16 = 114;
/// First and one-past-last line ANTIC can display.
pub const FIRST_LINE: u16 = 8;
pub const VBLANK_LINE: u16 = 248;
/// Cycle of the DLI and VBI NMIs within a line.
pub const NMI_CYCLE: u16 = 7;
/// The CPU runs again in this cycle after a WSYNC write.
pub const WSYNC_RELEASE: u16 = 105;

/// Memory as ANTIC's DMA sees it (no side effects).
pub trait AnticMemory {
    fn dma_read(&self, addr: u16) -> u8;
}

/// Lines per field.
pub fn lines(standard: VideoStandard) -> u16 {
    match standard {
        VideoStandard::Ntsc => 262,
        VideoStandard::Pal => 312,
    }
}

/// What the playfield shows at one colour clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playfield {
    /// Background (COLBK).
    #[default]
    Background,
    /// Playfield colour 0–3.
    Pf(u8),
    /// Hires modes 2, 3 and F: the colour clock is PF2 area; the two bits
    /// are its two half-clock pixels (bit 1 first), drawn with PF1's luma.
    Hires(u8),
}

/// DMA use of one cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Slot {
    #[default]
    Free,
    Missiles,
    Player(u8),
    Instr,
    AddrLo,
    AddrHi,
    Refresh,
    /// Character name `i` (character modes, first line).
    Name(u8),
    /// Graphics byte `i`.
    Data(u8),
}

/// Result of one cycle for the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CycleOut {
    /// HALT for the CPU in this cycle (DMA or WSYNC).
    pub halt: bool,
    /// An enabled NMI fires in this cycle.
    pub nmi: bool,
    /// Player/missile DMA: (0–3 player, 4 missiles, data).
    pub pm: Option<(u8, u8)>,
}

/// Properties of display modes 2–F (index = mode).
#[derive(Clone, Copy)]
struct ModeInfo {
    /// Scan lines per mode line.
    lines: u8,
    /// Bytes per mode line at normal width.
    bytes: u8,
    /// Character mode (name + character data).
    chars: bool,
}

const MODES: [ModeInfo; 16] = {
    const fn m(lines: u8, bytes: u8, chars: bool) -> ModeInfo {
        ModeInfo {
            lines,
            bytes,
            chars,
        }
    }
    [
        m(1, 0, false),
        m(1, 0, false),
        m(8, 40, true),
        m(10, 40, true),
        m(8, 40, true),
        m(16, 40, true),
        m(8, 20, true),
        m(16, 20, true),
        m(8, 10, false),
        m(4, 10, false),
        m(4, 20, false),
        m(2, 20, false),
        m(1, 20, false),
        m(2, 40, false),
        m(1, 40, false),
        m(1, 40, false),
    ]
};

/// Left edge (colour clock) and width in bytes factor of the playfield
/// widths 1–3 (narrow, normal, wide).
fn width_edges(width: u8) -> (u16, u16) {
    match width {
        1 => (64, 192),
        2 => (48, 208),
        _ => (32, 224),
    }
}

/// Bytes fetched for a mode at a width (narrow 4/5, wide 6/5 of normal).
fn bytes_at(mode: u8, width: u8) -> u16 {
    let normal = u16::from(MODES[usize::from(mode)].bytes);
    match width {
        1 => normal * 4 / 5,
        2 => normal,
        _ => normal * 6 / 5,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Antic {
    standard: VideoStandard,
    pub dmactl: u8,
    pub chactl: u8,
    pub dlist: u16,
    pub hscrol: u8,
    pub vscrol: u8,
    pub pmbase: u8,
    pub chbase: u8,
    pub nmien: u8,
    pub nmist: u8,
    line: u16,
    cycle: u16,
    frame: u64,
    wsync: bool,
    /// Waiting for vertical blank after a JVB.
    jvb_wait: bool,
    /// Current display-list instruction (`None`: none fetched this field).
    ir: Option<u8>,
    /// Scan line within the mode line, and the last one.
    row: u8,
    last_row: u8,
    /// The current mode line is done: the next display line fetches an
    /// instruction.
    need_instr: bool,
    /// The previous mode line had vertical scrolling enabled.
    prev_vscrol: bool,
    /// Memory scan counter (12-bit increment within a 4 KB block).
    memscan: u16,
    /// Low byte of an LMS/JMP address between its two fetches.
    addr_lo: u8,
    /// Playfield of this line: mode (0 = none), fetched bytes, colour clock
    /// of byte 0, display window.
    mode: u8,
    fetched: u16,
    origin: u16,
    window: (u16, u16),
    /// Character names (first line of a character mode line).
    names: [u8; 48],
    /// Graphics bytes of the current line.
    data: [u8; 48],
    slots: [Slot; CYCLES_PER_LINE as usize],
}

impl Antic {
    pub fn new(standard: VideoStandard) -> Self {
        Self {
            standard,
            dmactl: 0,
            chactl: 0,
            dlist: 0,
            hscrol: 0,
            vscrol: 0,
            pmbase: 0,
            chbase: 0,
            nmien: 0,
            nmist: 0x1f,
            line: 0,
            cycle: 0,
            frame: 0,
            wsync: false,
            jvb_wait: false,
            ir: None,
            row: 0,
            last_row: 0,
            need_instr: true,
            prev_vscrol: false,
            memscan: 0,
            addr_lo: 0,
            mode: 0,
            fetched: 0,
            origin: 0,
            window: (0, 0),
            names: [0; 48],
            data: [0; 48],
            slots: [Slot::Free; CYCLES_PER_LINE as usize],
        }
    }

    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    /// RESET only clears NMIEN (ANTIC has no reset pin; the OS relies on
    /// NMIs being off after power-up).
    pub fn reset(&mut self) {
        self.nmien = 0;
        self.wsync = false;
    }

    pub fn line(&self) -> u16 {
        self.line
    }

    pub fn cycle(&self) -> u16 {
        self.cycle
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Side-effect-free register read.
    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 0x0f {
            0x0b => (self.line >> 1) as u8,
            // No light pen.
            0x0c | 0x0d => 0,
            0x0f => self.nmist | 0x1f,
            _ => 0xff,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 0x0f {
            0x00 => self.dmactl = v & 0x3f,
            0x01 => self.chactl = v & 0x07,
            0x02 => self.dlist = (self.dlist & 0xff00) | u16::from(v),
            0x03 => self.dlist = (self.dlist & 0x00ff) | (u16::from(v) << 8),
            0x04 => self.hscrol = v & 0x0f,
            0x05 => self.vscrol = v & 0x0f,
            0x07 => self.pmbase = v,
            0x09 => self.chbase = v,
            0x0a => self.wsync = true,
            0x0e => self.nmien = v & 0xc0,
            0x0f => self.nmist = 0x1f,
            _ => {}
        }
    }

    fn dl_dma(&self) -> bool {
        self.dmactl & 0x20 != 0
    }

    fn display_line(&self) -> bool {
        (FIRST_LINE..VBLANK_LINE).contains(&self.line)
    }

    /// Fetch one display-list byte.
    fn dl_byte(&mut self, mem: &dyn AnticMemory) -> u8 {
        let v = mem.dma_read(self.dlist);
        // The display-list counter only counts within a 1 KB block.
        self.dlist = (self.dlist & 0xfc00) | (self.dlist.wrapping_add(1) & 0x03ff);
        v
    }

    /// Plan the fixed slots of a new line (cycle 0).
    fn plan_line(&mut self) {
        self.slots = [Slot::Free; CYCLES_PER_LINE as usize];
        self.mode = 0;
        if !self.display_line() {
            return;
        }
        if self.dmactl & 0x0c != 0 {
            self.slots[0] = Slot::Missiles;
        }
        if self.dmactl & 0x08 != 0 {
            for p in 0..4 {
                self.slots[2 + usize::from(p)] = Slot::Player(p);
            }
        }
        if self.dl_dma() && !self.jvb_wait && self.need_instr {
            self.slots[1] = Slot::Instr;
        }
    }

    /// The instruction was fetched in cycle 1: plan the rest of the line.
    fn start_mode_line(&mut self, ir: u8) {
        self.ir = Some(ir);
        self.need_instr = false;
        let mode = ir & 0x0f;
        match mode {
            0 => {
                self.row = 0;
                self.last_row = (ir >> 4) & 7;
                self.prev_vscrol = false;
            }
            1 => {
                self.slots[6] = Slot::AddrLo;
                self.slots[7] = Slot::AddrHi;
                self.row = 0;
                self.last_row = 0;
                self.prev_vscrol = false;
            }
            _ => {
                if ir & 0x40 != 0 {
                    self.slots[6] = Slot::AddrLo;
                    self.slots[7] = Slot::AddrHi;
                }
                // Vertical scrolling: the first scrolled mode line starts at
                // row VSCROL, the first one after the scrolled region ends
                // there. The row counter is 4 bits and wraps.
                let lines = MODES[usize::from(mode)].lines;
                let vs = ir & 0x20 != 0;
                self.row = if vs && !self.prev_vscrol {
                    self.vscrol
                } else {
                    0
                };
                self.last_row = if !vs && self.prev_vscrol {
                    self.vscrol
                } else {
                    lines - 1
                };
                self.prev_vscrol = vs;
            }
        }
    }

    /// Plan the playfield fetches of this line (after the instruction).
    fn plan_playfield(&mut self, first: bool) {
        let Some(ir) = self.ir else { return };
        let mode = ir & 0x0f;
        let width = self.dmactl & 3;
        if mode < 2 || width == 0 || !self.dl_dma() || self.jvb_wait {
            return;
        }
        let info = MODES[usize::from(mode)];
        let hscroll = ir & 0x10 != 0;
        let fetch_width = if hscroll { (width + 1).min(3) } else { width };
        let (lo, hi) = width_edges(width);
        let (fetch_lo, _) = width_edges(fetch_width);
        let bytes = bytes_at(mode, fetch_width);
        let cc_per_byte = 160 / u16::from(info.bytes);
        self.mode = mode;
        self.fetched = bytes;
        self.origin = fetch_lo + if hscroll { u16::from(self.hscrol) } else { 0 };
        self.window = (lo, hi);
        if !info.chars && !first {
            return;
        }
        for i in 0..bytes {
            let shown = (self.origin + i * cc_per_byte) / 2;
            if info.chars && first {
                if let Some(c) = shown.checked_sub(3) {
                    if usize::from(c) < self.slots.len() {
                        self.slots[usize::from(c)] = Slot::Name(i as u8);
                    }
                }
            }
            if let Some(c) = shown.checked_sub(2) {
                if usize::from(c) < self.slots.len() {
                    self.slots[usize::from(c)] = Slot::Data(i as u8);
                }
            }
        }
    }

    /// Nine refresh cycles from cycle 25, each pushed to the next free
    /// cycle when the playfield uses its slot.
    fn plan_refresh(&mut self) {
        let mut c = 25usize;
        for k in 0..9 {
            c = c.max(25 + 4 * k);
            while c < self.slots.len() && self.slots[c] != Slot::Free {
                c += 1;
            }
            if c >= self.slots.len() {
                break;
            }
            self.slots[c] = Slot::Refresh;
            c += 1;
        }
    }

    /// Run the DMA of the current cycle. Call before the CPU's access.
    pub fn tick(&mut self, mem: &dyn AnticMemory) -> CycleOut {
        let mut out = CycleOut::default();
        if self.cycle == 0 {
            self.plan_line();
            // Without a new instruction (continuing mode line, JVB wait, no
            // display) the line is known now; otherwise after cycle 1.
            if self.slots[1] != Slot::Instr {
                if self.display_line() && !self.jvb_wait {
                    self.plan_playfield(false);
                }
                self.plan_refresh();
            }
        }
        let slot = self.slots[usize::from(self.cycle)];
        match slot {
            Slot::Free => {}
            Slot::Missiles => out.pm = Some((4, mem.dma_read(self.pm_addr(4)))),
            Slot::Player(p) => out.pm = Some((p, mem.dma_read(self.pm_addr(p)))),
            Slot::Instr => {
                let ir = self.dl_byte(mem);
                self.start_mode_line(ir);
                self.plan_playfield(true);
                self.plan_refresh();
            }
            Slot::AddrLo => self.addr_lo = self.dl_byte(mem),
            Slot::AddrHi => {
                let hi = self.dl_byte(mem);
                let addr = u16::from_le_bytes([self.addr_lo, hi]);
                let ir = self.ir.unwrap_or(0);
                if ir & 0x0f == 1 {
                    self.dlist = addr;
                    if ir & 0x40 != 0 {
                        self.jvb_wait = true;
                    }
                } else {
                    self.memscan = addr;
                }
            }
            Slot::Refresh => {}
            Slot::Name(i) => {
                self.names[usize::from(i)] = mem.dma_read(self.scan_addr(u16::from(i)));
            }
            Slot::Data(i) => {
                let v = self.fetch_data(mem, i);
                self.data[usize::from(i)] = v;
            }
        }
        let busy = slot != Slot::Free;
        // WSYNC: the CPU waits for cycle 105.
        if self.wsync && self.cycle == WSYNC_RELEASE - 1 {
            out.halt = true;
            self.wsync = false;
        } else {
            out.halt = busy || self.wsync;
        }
        if self.cycle == NMI_CYCLE {
            out.nmi = self.nmi_check();
        }
        out
    }

    /// Advance the beam after the cycle. True when this was the last
    /// cycle of a field.
    pub fn end_cycle(&mut self) -> bool {
        self.cycle += 1;
        if self.cycle < CYCLES_PER_LINE {
            return false;
        }
        self.cycle = 0;
        self.end_line()
    }

    fn end_line(&mut self) -> bool {
        if self.display_line() && self.ir.is_some() && !self.jvb_wait {
            if self.row == self.last_row {
                // Mode line done: move the memory scan counter on.
                if let Some(ir) = self.ir {
                    if ir & 0x0f >= 2 && self.fetched > 0 {
                        self.memscan = (self.memscan & 0xf000)
                            | (self.memscan.wrapping_add(self.fetched) & 0x0fff);
                    }
                }
                self.fetched = 0;
                self.need_instr = true;
            } else {
                self.row = (self.row + 1) & 0x0f;
            }
        }
        self.line += 1;
        if self.line == VBLANK_LINE {
            // Display ends; the next field starts with a new instruction.
            self.ir = None;
            self.need_instr = true;
            self.jvb_wait = false;
            self.row = 0;
            self.last_row = 0;
            self.prev_vscrol = false;
        }
        if self.line >= lines(self.standard) {
            self.line = 0;
            self.frame += 1;
            return true;
        }
        false
    }

    /// NMIs at cycle 7: VBI on line 248, DLI on the last line of a mode
    /// line with bit 7 (and on every line of a JVB wait).
    fn nmi_check(&mut self) -> bool {
        let mut fire = false;
        if self.line == VBLANK_LINE {
            self.nmist = (self.nmist & !0x80) | 0x40;
            fire |= self.nmien & 0x40 != 0;
        } else if self.display_line() {
            let dli = match self.ir {
                Some(ir) if ir & 0x80 != 0 && self.dl_dma() => {
                    self.jvb_wait || self.row == self.last_row
                }
                _ => false,
            };
            if dli {
                self.nmist = (self.nmist & !0x40) | 0x80;
                fire |= self.nmien & 0x80 != 0;
            }
        }
        fire
    }

    fn pm_addr(&self, which: u8) -> u16 {
        let single = self.dmactl & 0x10 != 0;
        let line = self.line;
        if single {
            let base = u16::from(self.pmbase & 0xf8) << 8;
            let off = if which == 4 {
                0x300
            } else {
                0x400 + 0x100 * u16::from(which)
            };
            base + off + line
        } else {
            let base = u16::from(self.pmbase & 0xfc) << 8;
            let off = if which == 4 {
                0x180
            } else {
                0x200 + 0x80 * u16::from(which)
            };
            base + off + line / 2
        }
    }

    fn scan_addr(&self, i: u16) -> u16 {
        (self.memscan & 0xf000) | (self.memscan.wrapping_add(i) & 0x0fff)
    }

    /// A graphics byte: bitmap data, or character data for this row.
    fn fetch_data(&self, mem: &dyn AnticMemory, i: u8) -> u8 {
        let mode = self.mode;
        if !MODES[usize::from(mode)].chars {
            return mem.dma_read(self.scan_addr(u16::from(i)));
        }
        let name = self.names[usize::from(i)];
        let row = u16::from(self.row);
        let (base, code, row) = match mode {
            2..=5 => {
                let row = if matches!(mode, 5) { row / 2 } else { row };
                (
                    u16::from(self.chbase & 0xfc) << 8,
                    u16::from(name & 0x7f),
                    row,
                )
            }
            _ => {
                let row = if mode == 7 { row / 2 } else { row };
                (
                    u16::from(self.chbase & 0xfe) << 8,
                    u16::from(name & 0x3f),
                    row,
                )
            }
        };
        // Mode 3: lower-case descenders. Codes $60–$7F show their first two
        // rows at the bottom; the others leave rows 8–9 empty.
        let row = if mode == 3 {
            let descender = name & 0x60 == 0x60;
            match (row, descender) {
                (0..=1, true) => return 0,
                (8..=9, true) => row - 8,
                (8..=9, false) => return 0,
                _ => row,
            }
        } else {
            row
        };
        let row = if self.chactl & 0x04 != 0 {
            7 - (row & 7)
        } else {
            row & 7
        };
        let mut v = mem.dma_read(base + code * 8 + row);
        if matches!(mode, 2 | 3) && name & 0x80 != 0 {
            if self.chactl & 0x01 != 0 {
                v = 0;
            }
            if self.chactl & 0x02 != 0 {
                v = !v;
            }
        }
        v
    }

    /// Playfield value at colour clock `cc` of the current line.
    pub fn playfield(&self, cc: u16) -> Playfield {
        if self.mode < 2 || cc < self.window.0 || cc >= self.window.1 || cc < self.origin {
            return Playfield::Background;
        }
        let info = MODES[usize::from(self.mode)];
        let cc_per_byte = 160 / u16::from(info.bytes);
        let rel = cc - self.origin;
        let i = rel / cc_per_byte;
        if i >= self.fetched {
            return Playfield::Background;
        }
        let sub = rel % cc_per_byte;
        let d = self.data[usize::from(i)];
        let two = |shift: u16| (d >> shift) & 3;
        match self.mode {
            2 | 3 | 0x0f => Playfield::Hires(two(6 - 2 * sub)),
            4 | 5 => {
                let v = two(6 - 2 * sub);
                let name = self.names[usize::from(i)];
                match v {
                    0 => Playfield::Background,
                    3 if name & 0x80 != 0 => Playfield::Pf(3),
                    v => Playfield::Pf(v - 1),
                }
            }
            6 | 7 => {
                if d >> (7 - sub) & 1 != 0 {
                    Playfield::Pf(self.names[usize::from(i)] >> 6)
                } else {
                    Playfield::Background
                }
            }
            // 4 pixels of 4 colour clocks, 2 bits each.
            8 => pf2(two(6 - 2 * (sub / 4))),
            // 8 pixels of 2 colour clocks, 1 bit.
            9 => pf1(d >> (7 - sub / 2) & 1),
            0x0a => pf2(two(6 - 2 * (sub / 2))),
            0x0b | 0x0c => pf1(d >> (7 - sub) & 1),
            _ => pf2(two(6 - 2 * sub)),
        }
    }
}

fn pf1(bit: u8) -> Playfield {
    if bit != 0 {
        Playfield::Pf(0)
    } else {
        Playfield::Background
    }
}

fn pf2(v: u8) -> Playfield {
    match v {
        0 => Playfield::Background,
        v => Playfield::Pf(v - 1),
    }
}

fn slot_to_u8(s: Slot) -> (u8, u8) {
    match s {
        Slot::Free => (0, 0),
        Slot::Missiles => (1, 0),
        Slot::Player(p) => (2, p),
        Slot::Instr => (3, 0),
        Slot::AddrLo => (4, 0),
        Slot::AddrHi => (5, 0),
        Slot::Refresh => (6, 0),
        Slot::Name(i) => (7, i),
        Slot::Data(i) => (8, i),
    }
}

fn slot_from_u8(kind: u8, arg: u8) -> Result<Slot, StateError> {
    Ok(match kind {
        0 => Slot::Free,
        1 => Slot::Missiles,
        2 if arg < 4 => Slot::Player(arg),
        3 => Slot::Instr,
        4 => Slot::AddrLo,
        5 => Slot::AddrHi,
        6 => Slot::Refresh,
        7 if arg < 48 => Slot::Name(arg),
        8 if arg < 48 => Slot::Data(arg),
        _ => return Err(StateError::Invalid("ANTIC DMA slot")),
    })
}

const TAG: [u8; 4] = *b"ANTC";
const VERSION: u16 = 1;

impl Savestate for Antic {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for v in [
                self.dmactl,
                self.chactl,
                self.hscrol,
                self.vscrol,
                self.pmbase,
                self.chbase,
                self.nmien,
                self.nmist,
                self.row,
                self.last_row,
                self.mode,
                self.addr_lo,
            ] {
                w.u8(v);
            }
            for v in [
                self.dlist,
                self.line,
                self.cycle,
                self.memscan,
                self.fetched,
                self.origin,
                self.window.0,
                self.window.1,
            ] {
                w.u16(v);
            }
            w.u64(self.frame);
            w.bool(self.wsync);
            w.bool(self.jvb_wait);
            w.bool(self.prev_vscrol);
            w.bool(self.need_instr);
            w.bool(self.ir.is_some());
            w.u8(self.ir.unwrap_or(0));
            w.raw(&self.names);
            w.raw(&self.data);
            for s in self.slots {
                let (k, a) = slot_to_u8(s);
                w.u8(k);
                w.u8(a);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        n.dmactl = c.u8()?;
        n.chactl = c.u8()?;
        n.hscrol = c.u8()?;
        n.vscrol = c.u8()?;
        n.pmbase = c.u8()?;
        n.chbase = c.u8()?;
        n.nmien = c.u8()?;
        n.nmist = c.u8()?;
        n.row = c.u8()?;
        n.last_row = c.u8()?;
        n.mode = c.u8()?;
        n.addr_lo = c.u8()?;
        n.dlist = c.u16()?;
        n.line = c.u16()?;
        n.cycle = c.u16()?;
        n.memscan = c.u16()?;
        n.fetched = c.u16()?;
        n.origin = c.u16()?;
        n.window = (c.u16()?, c.u16()?);
        n.frame = c.u64()?;
        n.wsync = c.bool()?;
        n.jvb_wait = c.bool()?;
        n.prev_vscrol = c.bool()?;
        n.need_instr = c.bool()?;
        let has_ir = c.bool()?;
        let ir = c.u8()?;
        n.ir = has_ir.then_some(ir);
        c.raw_into(&mut n.names)?;
        c.raw_into(&mut n.data)?;
        for s in n.slots.iter_mut() {
            let k = c.u8()?;
            let a = c.u8()?;
            *s = slot_from_u8(k, a)?;
        }
        if n.line >= lines(n.standard)
            || n.cycle >= CYCLES_PER_LINE
            || n.mode > 15
            || n.fetched > 48
        {
            return Err(StateError::Invalid("ANTIC beam"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
