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
//! | C/D | –                             | PENH / PENV (light pen) |
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
//! Playfield DMA follows ANTIC's DMA clock (Altirra Hardware Reference
//! Manual, 4.12 "Abnormal playfield DMA"; timing fitted to Acid800
//! `antic_dmapattern`, `antic_pfstarttiming`, `antic_pfstoptiming`): a ring
//! of 2, 4 or 8 cycles (the mode's fetch rate) in which each set bit starts
//! a playfield byte when it comes round. The playfield start injects a bit
//! a cycle ahead when the cycle reaches the left fetch edge of the current
//! width (plus HSCROL); the stop clears the bit of its own phase at the
//! right edge, and at the wide right edge, which is always active. Both
//! edges are compared live, so DMACTL and HSCROL writes during the line
//! move them. A write that shifts the stop to another phase leaves the bit
//! running: playfield DMA goes on through horizontal blank into the next
//! line (Acid800 `antic_hscrolbug`), with the bus cycles suppressed in
//! cycles 105–111 and 0 while the memory scan counter and the line buffer
//! address still count. Blank and jump lines and vertical blank clear the
//! clock.
//!
//! Each byte start advances the memory scan counter (first line of the mode
//! line) and the line buffer address; a character mode fetches the name then
//! (first line only) and the character data 3 cycles later, a bitmap mode
//! its data 2 cycles later. The 48-byte line buffer address restarts in
//! cycle 6 (this project's reading of `antic_hscrolbug`, where the bytes of
//! a runaway clock before the start move the picture by their number);
//! the playfield shows the buffer from the address of the byte that
//! started at the left edge. Later lines of a bitmap mode line replay the
//! buffer without DMA, and a line with the playfield width set to 0 keeps
//! the clock and addresses running but fetches nothing (Acid800
//! `antic_linebuffering`).

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

/// Cycle in which the line buffer address restarts.
const LBA_RESET: u16 = 6;
/// Playfield bus cycles in these cycles are suppressed (virtual DMA): the
/// clock still counts.
fn pf_suppressed(cycle: u16) -> bool {
    cycle == 0 || (105..=111).contains(&cycle)
}

/// A playfield data fetch waiting for its cycle: cycles to go, line buffer
/// index, address (bitmap modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PendingFetch {
    left: u8,
    buf: u8,
    addr: u16,
}

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
    /// The display-list counter as the field's display began (line 8): the
    /// start of the display list the OS (or a JVB) set up, for debugger
    /// views; `dlist` itself runs on through the list.
    pub dlist_start: u16,
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
    /// The 400/800 RESET key was pressed: an RNMI on the next NMI cycle.
    rnmi: bool,
    /// Waiting for vertical blank after a JVB.
    jvb_wait: bool,
    /// The last display line (247) was a hires mode line (2, 3, F): ANTIC
    /// stays in hires through vertical blank, so GTIA goes on showing
    /// players and detecting their collisions there (the "hires bug",
    /// Acid800 `antic_hiresbug`). Cleared when display starts again.
    vbl_hires: bool,
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
    /// The DMA clock: set bits are the phases (positions of `ring_pos`
    /// modulo `clk_len`) at which a playfield byte starts.
    dclk: u8,
    /// Free-running cycle position modulo 8 (114 mod 8 = 2: the ring moves
    /// two places against the line each line).
    ring_pos: u8,
    /// Ring length of the clock: 2, 4 or 8 cycles, and the mode it runs
    /// for (it runs on into a line before that line's instruction).
    clk_len: u8,
    clk_mode: u8,
    /// Line buffer address of the next byte, and of the byte that started
    /// at the playfield's left edge (shown first).
    lba: u8,
    disp_base: u8,
    /// Data fetches of started bytes.
    pending: [PendingFetch; 4],
    pending_n: u8,
    /// Address of this cycle's playfield fetch.
    pf_addr: u16,
    /// The line repeats the last instruction as a new mode line (no
    /// display-list DMA): plan it as a first line.
    repeat_first: bool,
    /// Whether this is the first line of the mode line, and whether the
    /// line only replays the line buffer.
    pf_first: bool,
    pf_replay: bool,
    /// A refresh request waits for a free cycle.
    refresh_pending: bool,
    origin: u16,
    window: (u16, u16),
    /// Character names (first line of a character mode line).
    names: [u8; 48],
    /// Graphics bytes of the current line.
    data: [u8; 48],
    slots: [Slot; CYCLES_PER_LINE as usize],
    /// PENH / PENV: the light-pen latch.
    penh: u8,
    penv: u8,
}

impl Antic {
    pub fn new(standard: VideoStandard) -> Self {
        Self {
            standard,
            dmactl: 0,
            chactl: 0,
            dlist: 0,
            dlist_start: 0,
            vbl_hires: false,
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
            rnmi: false,
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
            dclk: 0,
            ring_pos: 0,
            clk_len: 2,
            clk_mode: 0,
            lba: 0,
            disp_base: 0,
            pending: [PendingFetch::default(); 4],
            pending_n: 0,
            pf_addr: 0,
            repeat_first: false,
            pf_first: false,
            pf_replay: false,
            refresh_pending: false,
            origin: 0,
            window: (0, 0),
            names: [0; 48],
            data: [0; 48],
            slots: [Slot::Free; CYCLES_PER_LINE as usize],
            penh: 0,
            penv: 0,
        }
    }

    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    /// The 400/800 RESET key (wired to ANTIC's /RNMI, not to the system
    /// reset): NMIST bit 5 and an NMI if enabled, on the next NMI cycle.
    pub fn reset_key(&mut self) {
        self.rnmi = true;
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

    /// A pulse on the light-pen input (a joystick port's trigger line,
    /// pin 6): PENH latches the horizontal colour clock `cc` the beam is
    /// at (two per CPU cycle, the same count as the playfield's colour
    /// clocks), PENV the vertical count as VCOUNT shows it (line / 2).
    /// Every pulse latches; a light pen sends one per frame (Altirra
    /// Hardware Reference Manual, "ANTIC" — light pen registers; De Re
    /// Atari, ch. 8).
    pub fn light_pen(&mut self, cc: u16) {
        self.penh = cc as u8;
        self.penv = (self.line >> 1) as u8;
    }

    pub fn cycle(&self) -> u16 {
        self.cycle
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// The display-list instruction fetched in this cycle (cycle 1 of a
    /// line, between [`Antic::tick`] and [`Antic::end_cycle`]); `None`
    /// otherwise. For debug overlays, not used by the emulation.
    /// Vertical blank after a hires last display line: GTIA keeps players
    /// and their collisions going (see `vbl_hires`).
    #[inline]
    pub fn hires_in_vblank(&self) -> bool {
        self.vbl_hires && !self.display_line()
    }

    pub fn fetched_instr(&self) -> Option<u8> {
        (self.cycle == 1 && self.slots[1] == Slot::Instr)
            .then_some(self.ir)
            .flatten()
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
            0x0c => self.penh,
            0x0d => self.penv,
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
            0x0e => self.nmien = v & 0xe0,
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
        self.window = (0, 0);
        self.origin = 0;
        if !self.display_line() {
            // Vertical blank clears the DMA clock.
            self.clear_dma_clock();
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
                // DLI included (Acid800 `antic_dlistwrap`), as a new mode
                // line: its first line fetches (Acid800 `antic_hscrolbug`
                // test 2 reuses a mode E instruction this way).
                self.start_mode_line(ir);
                self.slots[6] = Slot::Free;
                self.slots[7] = Slot::Free;
                self.repeat_first = true;
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
            // Blank and jump lines clear the DMA clock.
            self.clear_dma_clock();
            return;
        }
        self.mode = mode;
        self.pf_first = first;
        self.pf_replay = !MODES[usize::from(mode)].chars && !first;
        if !self.pf_replay {
            self.fetched = 0;
        }
        self.clk_mode = mode;
        let len = (80 / u16::from(MODES[usize::from(mode)].bytes)) as u8;
        if len != self.clk_len {
            // A slower ring keeps the bits it had; a faster one folds them.
            let old = std::mem::replace(&mut self.dclk, 0);
            for p in 0..8 {
                if old & (1 << p) != 0 {
                    self.dclk |= 1 << (p % len);
                }
            }
            self.clk_len = len;
        }
    }

    fn clear_dma_clock(&mut self) {
        self.dclk = 0;
        self.pending_n = 0;
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

    /// The playfield DMA of this cycle, if any: runs the DMA clock (byte
    /// starts, their data fetches), then compares the start and stop edges
    /// for the next cycle.
    fn playfield_dma(&mut self) -> Slot {
        let c = self.cycle;
        let pos = self.ring_pos;
        self.ring_pos = (pos + 1) & 7;
        if c == LBA_RESET {
            self.lba = 0;
        }
        if self.dclk == 0 && self.pending_n == 0 && self.mode < 2 {
            return Slot::Free;
        }
        let suppressed = pf_suppressed(c);
        let info = MODES[usize::from(self.clk_mode.max(2))];
        let dma = self.mode >= 2 && !self.pf_replay && self.dmactl & 3 != 0 && self.dl_dma();
        let mut slot = Slot::Free;
        // Data of bytes started earlier.
        let mut i = 0;
        while i < usize::from(self.pending_n) {
            let f = &mut self.pending[i];
            f.left -= 1;
            if f.left == 0 {
                let f = *f;
                self.pending
                    .copy_within(i + 1..usize::from(self.pending_n), i);
                self.pending_n -= 1;
                if dma && !suppressed && slot == Slot::Free {
                    self.pf_addr = f.addr;
                    slot = Slot::Data(f.buf);
                }
            } else {
                i += 1;
            }
        }
        // A byte starts when a clock bit comes round.
        // The ring length is 2, 4 or 8: a mask instead of a division.
        let mask = self.clk_len.max(1) - 1;
        if self.dclk & (1 << (pos & mask)) != 0 {
            let buf = self.lba;
            self.lba = if self.lba == 47 { 0 } else { self.lba + 1 };
            let addr = self.memscan;
            // The memory scan counter moves on in the mode line's first
            // line only (later lines reuse the names or the line buffer).
            if self.pf_first {
                self.memscan = (self.memscan & 0xf000) | (self.memscan.wrapping_add(1) & 0x0fff);
                self.fetched = self.fetched.wrapping_add(1);
            }
            if dma && info.chars && self.pf_first && !suppressed && slot == Slot::Free {
                self.pf_addr = addr;
                slot = Slot::Name(buf);
            }
            if usize::from(self.pending_n) < self.pending.len() {
                self.pending[usize::from(self.pending_n)] = PendingFetch {
                    left: if info.chars { 3 } else { 2 },
                    buf,
                    addr,
                };
                self.pending_n += 1;
            }
        }
        // Start and stop edges, a cycle ahead. The start edges (colour
        // clocks 32–79) can only match in cycles 9–32, the stops (192–239)
        // in 89–112: skip the comparisons elsewhere.
        if !(9..=32).contains(&c) && !(89..=112).contains(&c) {
            return slot;
        }
        let next = (pos + 1) & 7;
        let bit = 1 << (next & mask);
        if let Some((lo, hi)) = self.fetch_edges() {
            if c + 1 + PF_LEAD == lo / 2 {
                self.dclk |= bit;
                self.origin = lo;
                self.window = width_edges(self.dmactl & 3);
                self.disp_base = self.lba;
            }
            if c + 1 + PF_LEAD == hi / 2 {
                self.dclk &= !bit;
            }
        }
        // The wide stop is always active, also with the playfield width at
        // 0 (Acid800 `antic_linebuffering`), but not without display-list
        // DMA (`antic_hscrolbug` test 2 keeps the clock running then).
        if self.mode >= 2 && self.dl_dma() && c + 1 + PF_LEAD == self.wide_stop() / 2 {
            self.dclk &= !bit;
        }
        slot
    }

    /// The wide playfield's stop edge (always active), with HSCROL.
    fn wide_stop(&self) -> u16 {
        let h = match self.ir {
            Some(ir) if ir & 0x10 != 0 => u16::from(self.hscrol),
            _ => 0,
        };
        width_edges(3).1 + h
    }

    /// Run the DMA of the current cycle. Call before the CPU's access.
    pub fn tick(&mut self, mem: &dyn AnticMemory) -> CycleOut {
        let mut out = CycleOut::default();
        if self.cycle == 0 {
            self.plan_line();
            // Without a new instruction (continuing mode line, JVB wait, no
            // display) the line is known now; otherwise after cycle 1.
            if self.slots[1] != Slot::Instr && self.display_line() && !self.jvb_wait {
                let first = std::mem::take(&mut self.repeat_first);
                self.plan_playfield(first);
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
                self.names[usize::from(i)] = mem.dma_read(self.pf_addr);
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
            let mut event = self.nmi_check();
            if std::mem::take(&mut self.rnmi) {
                // The 400/800 RESET key: NMIST bit 5 (RNMI), an NMI when
                // NMIEN bit 5 allows it (the OS enables it at boot).
                self.nmist |= 0x20;
                event |= 0x20;
            }
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
                // Mode line done (the memory scan counter counted live).
                self.fetched = 0;
                self.need_instr = true;
            } else {
                self.row = (self.row + 1) & 0x0f;
            }
        }
        self.line += 1;
        if self.line == FIRST_LINE {
            self.dlist_start = self.dlist;
            self.vbl_hires = false;
        }
        if self.line == VBLANK_LINE {
            self.vbl_hires = matches!(self.mode, 2 | 3 | 0x0f);
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
    /// A graphics byte: bitmap data, or character data for this row.
    fn fetch_data(&self, mem: &dyn AnticMemory, i: u8) -> u8 {
        let mode = self.mode;
        if !MODES[usize::from(mode)].chars {
            return mem.dma_read(self.pf_addr);
        }
        char_data(
            mem,
            mode,
            self.chbase,
            self.chactl,
            self.names[usize::from(i)],
            self.row,
        )
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
        // Both below 48: one subtraction instead of a division (per pixel).
        let mut b = usize::from(self.disp_base) + usize::from(i);
        if b >= 48 {
            b -= 48;
        }
        pixel(self.mode, self.data[b], self.names[b], sub)
    }
}

/// Character data of character mode `mode` (2–7) for character name `name`
/// on scan line `row` of the mode line: the character base from CHBASE
/// (1 KB aligned for modes 2–5, 512 bytes for 6–7), double-height rows in
/// modes 5 and 7, mode 3's descenders, and CHACTL's reflection (bit 2),
/// blanking (bit 0) and inversion (bit 1) of characters with bit 7 in
/// modes 2 and 3 (Altirra Hardware Reference Manual, "ANTIC: character
/// modes").
pub(crate) fn char_data(
    mem: &dyn AnticMemory,
    mode: u8,
    chbase: u8,
    chactl: u8,
    name: u8,
    row: u8,
) -> u8 {
    let row = u16::from(row);
    let (base, code, row) = match mode {
        2..=5 => {
            let row = if matches!(mode, 5) { row / 2 } else { row };
            (u16::from(chbase & 0xfc) << 8, u16::from(name & 0x7f), row)
        }
        _ => {
            let row = if mode == 7 { row / 2 } else { row };
            (u16::from(chbase & 0xfe) << 8, u16::from(name & 0x3f), row)
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
        if chactl & 0x04 != 0 {
            7 - (row & 7)
        } else {
            row & 7
        }
    });
    // An empty row is still inverted (Acid800 `antic_charcontrol`).
    let mut v = row.map_or(0, |row| mem.dma_read(base + code * 8 + row));
    if matches!(mode, 2 | 3) && name & 0x80 != 0 {
        if chactl & 0x01 != 0 {
            v = 0;
        }
        if chactl & 0x02 != 0 {
            v = !v;
        }
    }
    v
}

/// Playfield value of colour clock `sub` within a byte of mode `mode`
/// (graphics byte `d`, character name `name` for the character modes).
#[inline]
pub(crate) fn pixel(mode: u8, d: u8, name: u8, sub: u16) -> Playfield {
    let two = |shift: u16| (d >> shift) & 3;
    match mode {
        2 | 3 | 0x0f => Playfield::Hires(two(6 - 2 * sub)),
        4 | 5 => match two(6 - 2 * sub) {
            0 => Playfield::Background,
            3 if name & 0x80 != 0 => Playfield::Pf(3),
            v => Playfield::Pf(v - 1),
        },
        6 | 7 => {
            if d >> (7 - sub) & 1 != 0 {
                Playfield::Pf(name >> 6)
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
/// Version 3: the pending RESET-key NMI.
/// Version 4: the display list's start of the field.
/// Version 5: PENH / PENV.
const VERSION: u16 = 7;

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
            w.bool(self.rnmi);
            w.bool(self.jvb_wait);
            w.bool(self.prev_vscrol);
            w.bool(self.scroll_end);
            w.bool(self.pf_first);
            w.bool(self.pf_replay);
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
            w.u16(self.dlist_start);
            w.u8(self.penh);
            w.u8(self.penv);
            w.bool(self.vbl_hires);
            for v in [
                self.dclk,
                self.ring_pos,
                self.clk_len,
                self.clk_mode,
                self.lba,
                self.disp_base,
                self.pending_n,
            ] {
                w.u8(v);
            }
            for f in self.pending {
                w.u8(f.left);
                w.u8(f.buf);
                w.u16(f.addr);
            }
            w.u16(self.pf_addr);
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
        if version >= 3 {
            n.rnmi = c.bool()?;
        }
        n.jvb_wait = c.bool()?;
        n.prev_vscrol = c.bool()?;
        if version >= 2 {
            n.scroll_end = c.bool()?;
            if version < 7 {
                // The window model of versions 2–6: state, base, index.
                let _ = c.u8()?;
                let _ = c.u16()?;
            }
            n.pf_first = c.bool()?;
            n.pf_replay = c.bool()?;
            if version < 7 {
                let _ = c.u16()?;
            }
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
        n.dlist_start = if version >= 4 { c.u16()? } else { n.dlist };
        (n.penh, n.penv) = if version >= 5 {
            (c.u8()?, c.u8()?)
        } else {
            (0, 0)
        };
        n.vbl_hires = version >= 6 && c.bool()?;
        if version >= 7 {
            n.dclk = c.u8()?;
            n.ring_pos = c.u8()?;
            n.clk_len = c.u8()?;
            n.clk_mode = c.u8()?;
            n.lba = c.u8()?;
            n.disp_base = c.u8()?;
            n.pending_n = c.u8()?;
            for f in n.pending.iter_mut() {
                f.left = c.u8()?;
                f.buf = c.u8()?;
                f.addr = c.u16()?;
            }
            n.pf_addr = c.u16()?;
        } else {
            // Older states: the clock starts afresh with the next line.
            n.dclk = 0;
            n.pending_n = 0;
        }
        if n.line >= lines(n.standard)
            || n.cycle >= CYCLES_PER_LINE
            || n.mode > 15
            || !matches!(n.clk_len, 2 | 4 | 8)
            || n.clk_mode > 15
            || n.ring_pos > 7
            || n.lba >= 48
            || n.disp_base >= 48
            || usize::from(n.pending_n) > n.pending.len()
            || n.pending.iter().any(|f| f.buf >= 48 || f.left > 3)
        {
            return Err(StateError::Invalid("ANTIC beam"));
        }
        *self = n;
        Ok(())
    }
}

pub mod gfx;

#[cfg(test)]
mod tests;
