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
//! players cycles 2–5, LMS/JMP address cycles 6–7; nine refresh requests
//! from cycle 25 every 4 cycles, each waiting for a cycle the playfield
//! leaves free.
//!
//! Playfield DMA (fitted to Acid800 `antic_dmapattern`, `antic_pfstarttiming`
//! and `antic_pfstoptiming`): a window opens when the cycle reaches the
//! left fetch edge of the current width (plus HSCROL) and closes at its
//! right edge, both compared live, so DMACTL and HSCROL writes during the
//! line move them. Byte k of the window starts 6 cycles before it shows,
//! every 2, 4 or 8 cycles: a character mode fetches the name then (first
//! line of the mode line only) and the character data 3 cycles later, a
//! bitmap mode its data 2 cycles later. The bytes go into a 48-byte line
//! buffer at the byte's index in the window; later lines of a bitmap mode
//! line replay the buffer through the same window without DMA, and a line
//! with the playfield width set to 0 keeps the window and index running
//! but fetches nothing (Acid800 `antic_linebuffering`).

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
/// Cycle from which a mode line ending a scrolled region uses VSCROL.
const VSCROL_LATCH: u16 = 6;
/// Cycles from a playfield byte's start to its first colour clock.
const PF_LEAD: u16 = 6;

/// Playfield DMA window of the current line.
const PF_OFF: u8 = 0;
const PF_IDLE: u8 = 1;
const PF_OPEN: u8 = 2;
/// Closed; data of bytes already started still comes.
const PF_DRAIN: u8 = 3;

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
    /// HALT for the CPU in this cycle: ANTIC uses the bus (DMA). The 6502C
    /// stops on any cycle.
    pub halt: bool,
    /// RDY low: WSYNC holds the CPU. Like RDY on any NMOS 6502 this only
    /// stops read cycles — the second write of `INC WSYNC` still happens
    /// (Acid800 `antic_wsync`).
    pub wsync: bool,
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
    /// The WSYNC latch at the end of the previous cycle (RDY follows it).
    wsync_req: bool,
    /// The NMI event of cycle 7 and NMIEN at that time (the NMI goes out
    /// in cycle 8).
    nmi_event: u8,
    nmien_at_event: u8,
    /// NMIST bits set in the current cycle (immune to NMIRES).
    nmist_fresh: u8,
    /// Waiting for vertical blank after a JVB.
    jvb_wait: bool,
    /// Current display-list instruction (`None`: none fetched since power-on;
    /// it survives vertical blank).
    ir: Option<u8>,
    /// Scan line within the mode line, and the last one.
    row: u8,
    last_row: u8,
    /// The current mode line is done: the next display line fetches an
    /// instruction.
    need_instr: bool,
    /// The previous mode line had vertical scrolling enabled.
    prev_vscrol: bool,
    /// The mode line ends a vertically scrolled region: its last row is
    /// VSCROL as of each line's cycle 5.
    scroll_end: bool,
    /// Memory scan counter (12-bit increment within a 4 KB block).
    memscan: u16,
    /// Low byte of an LMS/JMP address between its two fetches.
    addr_lo: u8,
    /// Playfield of this line: mode (0 = none), fetched bytes, colour clock
    /// of byte 0, display window.
    mode: u8,
    fetched: u16,
    /// Playfield DMA window (`PF_*`), the cycle byte 0 started, whether
    /// this is the first line of the mode line, whether it only replays
    /// the line buffer, and the next byte's index in the window.
    pf_state: u8,
    pf_base: u16,
    pf_first: bool,
    pf_replay: bool,
    pf_index: u16,
    /// A refresh request waits for a free cycle.
    refresh_pending: bool,
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
            wsync_req: false,
            nmi_event: 0,
            nmien_at_event: 0,
            nmist_fresh: 0,
            jvb_wait: false,
            ir: None,
            row: 0,
            last_row: 0,
            need_instr: true,
            prev_vscrol: false,
            scroll_end: false,
            memscan: 0,
            addr_lo: 0,
            mode: 0,
            fetched: 0,
            pf_state: PF_OFF,
            pf_base: 0,
            pf_first: false,
            pf_replay: false,
            pf_index: 0,
            refresh_pending: false,
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
        self.wsync_req = false;
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
            // VCOUNT moves on at cycle 111; on the last line it shows
            // lines/2 for one cycle before wrapping (Acid800 `antic_vcount`).
            0x0b => {
                if self.cycle < 111 {
                    (self.line >> 1) as u8
                } else if self.line + 1 == lines(self.standard) && self.cycle >= 112 {
                    0
                } else {
                    ((self.line + 1) >> 1) as u8
                }
            }
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
            0x0f => self.nmist = 0x1f | (self.nmist & self.nmist_fresh),
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
        self.pf_state = PF_OFF;
        self.window = (0, 0);
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
        if !self.jvb_wait && self.need_instr {
            if self.dl_dma() {
                self.slots[1] = Slot::Instr;
            } else if let Some(ir) = self.ir {
                // Without display-list DMA the last instruction repeats,
                // DLI included (Acid800 `antic_dlistwrap`).
                self.start_mode_line(ir);
                self.slots[6] = Slot::Free;
                self.slots[7] = Slot::Free;
            }
        }
    }

    /// The instruction was fetched in cycle 1: plan the rest of the line.
    fn start_mode_line(&mut self, ir: u8) {
        self.ir = Some(ir);
        self.need_instr = false;
        let mode = ir & 0x0f;
        match mode {
            0 => {
                // Blank lines end a scrolled region like any mode line
                // (Acid800 `antic_vscroldli`).
                self.row = 0;
                self.scroll_end = self.prev_vscrol;
                self.last_row = if self.scroll_end {
                    self.vscrol
                } else {
                    (ir >> 4) & 7
                };
                self.prev_vscrol = false;
            }
            1 => {
                self.slots[6] = Slot::AddrLo;
                self.slots[7] = Slot::AddrHi;
                self.row = 0;
                self.last_row = 0;
                self.prev_vscrol = false;
                self.scroll_end = false;
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
                self.scroll_end = !vs && self.prev_vscrol;
                self.last_row = if self.scroll_end {
                    self.vscrol
                } else {
                    lines - 1
                };
                self.prev_vscrol = vs;
            }
        }
    }

    /// Set up the playfield of this line (after the instruction); the
    /// fetches happen cycle by cycle in [`Antic::playfield_dma`].
    fn plan_playfield(&mut self, first: bool) {
        let Some(ir) = self.ir else { return };
        let mode = ir & 0x0f;
        if mode < 2 || self.jvb_wait {
            return;
        }
        self.mode = mode;
        self.pf_first = first;
        self.pf_replay = !MODES[usize::from(mode)].chars && !first;
        self.pf_index = 0;
        if !self.pf_replay {
            self.fetched = 0;
        }
        self.pf_state = PF_IDLE;
    }

    /// Playfield fetch edges (colour clocks) for the current DMACTL and
    /// HSCROL: a horizontally scrolled line fetches the next wider width,
    /// shifted right by HSCROL.
    fn fetch_edges(&self) -> Option<(u16, u16)> {
        let width = self.dmactl & 3;
        let ir = self.ir?;
        if width == 0 || !self.dl_dma() {
            return None;
        }
        let hscroll = ir & 0x10 != 0;
        let (lo, hi) = width_edges(if hscroll { (width + 1).min(3) } else { width });
        let h = if hscroll { u16::from(self.hscrol) } else { 0 };
        Some((lo + h, hi + h))
    }

    /// The playfield DMA of this cycle, if any: opens and closes the
    /// window and starts the bytes in it.
    fn playfield_dma(&mut self) -> Slot {
        if self.pf_state == PF_OFF {
            return Slot::Free;
        }
        let c = self.cycle;
        let edges = self.fetch_edges();
        if self.pf_state == PF_IDLE {
            // Like the right edge, the left one is compared a cycle ahead.
            if let Some((lo, _)) = edges {
                if c + 1 + PF_LEAD == lo / 2 {
                    self.pf_state = PF_OPEN;
                    self.pf_base = c + 1;
                    self.origin = lo;
                    self.window = width_edges(self.dmactl & 3);
                }
            }
            return Slot::Free;
        }
        let info = MODES[usize::from(self.mode)];
        let stride = 80 / u16::from(info.bytes);
        let off = c - self.pf_base;
        let dma = !self.pf_replay && self.dmactl & 3 != 0 && self.dl_dma();
        let mut slot = Slot::Free;
        let data_lead = if info.chars { 3 } else { 2 };
        if let Some(d) = off.checked_sub(data_lead) {
            if dma && d.is_multiple_of(stride) && d / stride < self.pf_index {
                slot = Slot::Data((d / stride) as u8);
            }
        }
        if self.pf_state == PF_OPEN {
            if off.is_multiple_of(stride) && off / stride == self.pf_index && self.pf_index < 48 {
                if dma && info.chars && self.pf_first {
                    slot = Slot::Name(self.pf_index as u8);
                }
                self.pf_index += 1;
                if !self.pf_replay {
                    self.fetched = self.pf_index;
                }
            }
            // The right edge closes the window a cycle ahead (with the
            // width at 0 there is no edge: the window stays open).
            if let Some((_, hi)) = edges {
                if c + 1 + PF_LEAD == hi / 2 {
                    self.pf_state = PF_DRAIN;
                }
            }
        }
        slot
    }

    /// Run the DMA of the current cycle. Call before the CPU's access.
    pub fn tick(&mut self, mem: &dyn AnticMemory) -> CycleOut {
        let mut out = CycleOut::default();
        if self.cycle == 0 {
            self.plan_line();
            // Without a new instruction (continuing mode line, JVB wait, no
            // display) the line is known now; otherwise after cycle 1.
            if self.slots[1] != Slot::Instr && self.display_line() && !self.jvb_wait {
                self.plan_playfield(false);
            }
        }
        // The end of a scrolled region compares the row with VSCROL on every
        // line; a write up to cycle 5 still counts (Acid800
        // `antic_vscroldli`).
        if self.cycle == VSCROL_LATCH && self.scroll_end {
            self.last_row = self.vscrol;
        }
        // Refresh requests every 4 cycles from 25 wait for a free cycle;
        // one arriving while another waits is lost (Acid800
        // `antic_dmapattern`).
        if (25..=57).contains(&self.cycle) && (self.cycle - 25).is_multiple_of(4) {
            self.refresh_pending = true;
        }
        let fixed = self.slots[usize::from(self.cycle)];
        let pf = self.playfield_dma();
        let mut slot = if fixed != Slot::Free { fixed } else { pf };
        if slot == Slot::Free && self.refresh_pending {
            self.refresh_pending = false;
            slot = Slot::Refresh;
        }
        match slot {
            Slot::Free => {}
            Slot::Missiles => out.pm = Some((4, mem.dma_read(self.pm_addr(4)))),
            Slot::Player(p) => out.pm = Some((p, mem.dma_read(self.pm_addr(p)))),
            Slot::Instr => {
                let ir = self.dl_byte(mem);
                self.start_mode_line(ir);
                self.plan_playfield(true);
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
        out.halt = slot != Slot::Free;
        // WSYNC: a write sets a latch that clears at the end of cycle 103
        // (clearing wins over a write in that cycle); RDY follows the latch
        // two cycles late. So the CPU still runs the cycle after the write,
        // is held up to cycle 104 and runs again at 105; a write at 103
        // does not stop it, one at 104 holds it into the next line (Acid800
        // `antic_wsync`, `antic_vcount`).
        if self.cycle == WSYNC_RELEASE - 1 {
            self.wsync = false;
        }
        out.wsync = self.wsync_req;
        self.wsync_req = self.wsync;
        // NMIST shows the event from cycle 7 on, and an NMIRES in that same
        // cycle does not clear it. The NMI goes out a cycle later if NMIEN
        // allowed it in cycle 7 (a write then no longer disables it,
        // Acid800 `antic_nmist`); one enabled by a write in cycle 7 goes
        // out another cycle later (`antic_dlitiming`).
        self.nmist_fresh = 0;
        if self.cycle == NMI_CYCLE + 2 {
            out.nmi = std::mem::take(&mut self.nmi_event) != 0;
        }
        if self.cycle == NMI_CYCLE + 1 {
            let event = std::mem::take(&mut self.nmi_event);
            if self.nmien_at_event & event != 0 {
                out.nmi = true;
            } else {
                self.nmi_event = self.nmien & event;
            }
        }
        if self.cycle == NMI_CYCLE {
            let event = self.nmi_check();
            self.nmist_fresh = event;
            self.nmi_event = event;
            self.nmien_at_event = self.nmien;
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
            // Display ends; the next field starts with a new instruction
            // (or, without display-list DMA, repeats the last one).
            self.need_instr = true;
            self.jvb_wait = false;
            self.row = 0;
            self.last_row = 0;
            // The vertical-scroll state carries over: a scrolled region cut
            // off by vertical blank goes on in the next field (Acid800
            // `antic_vscroll`).
        }
        if self.line >= lines(self.standard) {
            self.line = 0;
            self.frame += 1;
            return true;
        }
        false
    }

    /// NMI events: VBI on line 248, DLI on the last line of a mode line
    /// with bit 7 (and on every line of a JVB wait). Sets NMIST; returns
    /// the event's NMIST bit (0 for none).
    fn nmi_check(&mut self) -> u8 {
        if self.line == VBLANK_LINE {
            self.nmist = (self.nmist & !0x80) | 0x40;
            return 0x40;
        } else if self.display_line() {
            let dli = match self.ir {
                Some(ir) if ir & 0x80 != 0 => self.jvb_wait || self.row == self.last_row,
                _ => false,
            };
            if dli {
                self.nmist = (self.nmist & !0x40) | 0x80;
                return 0x80;
            }
        }
        0
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
                (0..=1, true) | (8..=9, false) => None,
                (8..=9, true) => Some(row - 8),
                _ => Some(row),
            }
        } else {
            Some(row)
        };
        let row = row.map(|row| {
            if self.chactl & 0x04 != 0 {
                7 - (row & 7)
            } else {
                row & 7
            }
        });
        // An empty row is still inverted (Acid800 `antic_charcontrol`).
        let mut v = row.map_or(0, |row| mem.dma_read(base + code * 8 + row));
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
        if i >= 48 {
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
/// Version 2: WSYNC request latch, NMI timing, scrolled-region end.
const VERSION: u16 = 2;

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
            w.bool(self.wsync_req);
            w.u8(self.nmi_event);
            w.u8(self.nmist_fresh);
            w.u8(self.nmien_at_event);
            w.bool(self.jvb_wait);
            w.bool(self.prev_vscrol);
            w.bool(self.scroll_end);
            w.u8(self.pf_state);
            w.u16(self.pf_base);
            w.bool(self.pf_first);
            w.bool(self.pf_replay);
            w.u16(self.pf_index);
            w.bool(self.refresh_pending);
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
        let (version, mut c) = r.chunk(TAG, VERSION)?;
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
        n.wsync_req = if version >= 2 { c.bool()? } else { false };
        if version >= 2 {
            n.nmi_event = c.u8()?;
            n.nmist_fresh = c.u8()?;
            n.nmien_at_event = c.u8()?;
        }
        n.jvb_wait = c.bool()?;
        n.prev_vscrol = c.bool()?;
        if version >= 2 {
            n.scroll_end = c.bool()?;
            n.pf_state = c.u8()?;
            n.pf_base = c.u16()?;
            n.pf_first = c.bool()?;
            n.pf_replay = c.bool()?;
            n.pf_index = c.u16()?;
            n.refresh_pending = c.bool()?;
        }
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
            || n.pf_state > PF_DRAIN
            || n.pf_base >= CYCLES_PER_LINE
            || n.pf_index > 48
        {
            return Err(StateError::Invalid("ANTIC beam"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
