//! MOS 6569 (PAL) and 6567R8 (NTSC) VIC-II.
//!
//! Cycle-exact model after Christian Bauer, "The MOS 6567/6569 video
//! controller (VIC-II) and its application in the Commodore 64" (1996):
//! access slots per cycle, VC/VCBASE/RC/VMLI logic, display/idle state, bad
//! lines with BA, sprite DMA (MC/MCBASE/expansion flip-flop), the border
//! flip-flops and collision detection. The implementation is this project's
//! own; register descriptions follow the same document and the C64
//! Programmer's Reference Guide.
//!
//! Cycles are numbered 1–63 (6569) or 1–65 (6567R8) as in Bauer's timing
//! diagrams. Both types share the layout up to cycle 58; the 6567R8 has two
//! more cycles, one before the refresh (sprite fetches 3–7 at 65/2/4/6/8)
//! and one before sprite 0 (fetches at 59/61/63; Bauer's diagram puts them
//! at 60/62/64, the VICE test program VICII/phi1timing measured a 6567R8
//! otherwise). X coordinates are VIC-II
//! sprite coordinates, 8 pixels per cycle: cycle 1 starts at X = `$194`
//! (6569, wrapping after `$1F7`) or `$19C` (6567R8, wrapping after `$1FF`,
//! with the counter repeating `$18C–$18F` twice to fill 520 pixels; Bauer
//! 3.6.3, 6567R8 diagram). From cycle 14 on both types show the same X at
//! the same cycle.
//!
//! Per cycle the machine calls [`Vic2::phi1`] (VIC memory access, BA for
//! this cycle), lets the CPU run if BA allows, then [`Vic2::phi2`] (second
//! VIC access if it owns the bus, pixel output, advance).
//!
//! | Reg       | Function                                                        |
//! | --------- | --------------------------------------------------------------- |
//! | $00–$0F   | sprite 0–7 X (bits 0–7), Y                                      |
//! | $10       | sprite X bit 8                                                  |
//! | $11       | 7 RST8, 6 ECM, 5 BMM, 4 DEN, 3 RSEL, 0–2 YSCROLL                |
//! | $12       | raster (read) / raster compare (write), bits 0–7                |
//! | $13/$14   | light pen X/2, Y                                                |
//! | $15       | sprite enable                                                   |
//! | $16       | 4 MCM, 3 CSEL, 0–2 XSCROLL (bits 6–7 read 1)                    |
//! | $17       | sprite Y expansion                                              |
//! | $18       | 4–7 VM13–VM10, 1–3 CB13–CB11 (bit 0 reads 1)                    |
//! | $19       | IRQ latch: 0 raster, 1 sprite-data, 2 sprite-sprite, 3 light pen, 7 IRQ |
//! | $1A       | IRQ enable                                                      |
//! | $1B       | sprite-data priority (1 = behind foreground)                    |
//! | $1C       | sprite multicolour                                              |
//! | $1D       | sprite X expansion                                              |
//! | $1E/$1F   | sprite-sprite / sprite-data collisions (cleared on read)        |
//! | $20–$2E   | border, background 0–3, sprite MC 0–1, sprite colours (4 bits)  |
//! | $2F–$3F   | unused, read `$FF`                                              |

pub mod gfx;
pub mod overlay;
pub mod palette;

use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use emu_core::{BeamPos, Savestate, StateError, StateReader, StateWriter};

/// VIC-II type. Model variants are configuration, not savestate: a state
/// only loads into a VIC-II of the same type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    /// PAL-B.
    #[default]
    Mos6569,
    /// NTSC-M, the common NTSC type from 1983 on.
    Mos6567R8,
    /// PAL-B, HMOS-II (C64C): the 6569's timing; the first pixel after a
    /// colour register write shows light grey ("grey dot").
    Mos8565,
    /// NTSC-M, HMOS-II (C64C): the 6567R8's timing with the grey dot.
    Mos8562,
}

/// Line and frame geometry of a VIC-II type (Bauer 3.4, 3.6.3).
#[derive(Debug, PartialEq, Eq)]
pub struct Timing {
    pub lines: u16,
    pub cycles: u8,
    /// X coordinate at the start of cycle 1.
    pub first_x: u16,
    /// The X counter wraps to 0 here.
    pub x_wrap: u16,
    /// Linear position (`first_x` + pixel in line) from which the X counter
    /// repeats `$18C–$18F` for 8 extra pixels (6567R8 only).
    pub x_stall: Option<u16>,
    /// Visible window of the frame buffer: first pixel within the line,
    /// width, first raster line, height. It is what a monitor shows of the
    /// overscan, centred like VICE's normal border: 32 pixels of border
    /// left and right of the 320-pixel display window; PAL 35 lines above
    /// and 37 below the 200 display lines. The window may wrap past the last
    /// raster line to line 0.
    pub visible_p0: u16,
    pub width: u32,
    pub first_line: u16,
    pub height: u32,
    /// Dot clock; the CPU clock is an eighth of it.
    pub pixel_clock_hz: u32,
    pub standard: VideoStandard,
    /// First of the two cycles that start sprite DMA; the Y expansion
    /// flip-flop turns in its phi2. 55 on the 6569, 56 on the 6567R8
    /// (VICE test programs VICII/spriteenable: "modify sprite enable on
    /// cycle 55 (56 on NTSC)").
    pub dma_check: u8,
    /// Cycle of sprite 0's p-access: 58 on the 6569, 59 on the 6567R8
    /// (whose sprite 3 fetch then straddles the line: p-access in cycle 65).
    /// Measured by VICE test program VICII/phi1timing on a 6567R8: two idle
    /// cycles between sprite 7's fetch and the refresh, three between the
    /// last g-access and sprite 0 (Bauer's diagram has p0 at 60).
    pub sprite0_slot: u8,
}

pub const PAL: Timing = Timing {
    lines: 312,
    cycles: 63,
    first_x: 0x194,
    x_wrap: 504,
    x_stall: None,
    visible_p0: 92,
    width: 384,
    first_line: 16,
    height: 272,
    pixel_clock_hz: 7_881_984,
    standard: VideoStandard::Pal,
    dma_check: 55,
    sprite0_slot: 58,
};

/// 6567R8: visible lines 41–262 and 0–12 (first/last vblank line 13/40),
/// X `$1E8–$18B`.
pub const NTSC: Timing = Timing {
    lines: 263,
    cycles: 65,
    first_x: 0x19c,
    x_wrap: 512,
    x_stall: Some(0x200 + 0x18c),
    visible_p0: 92,
    width: 384,
    first_line: 41,
    height: 235,
    pixel_clock_hz: 8_181_816,
    standard: VideoStandard::Ntsc,
    dma_check: 56,
    sprite0_slot: 59,
};

impl Model {
    pub fn timing(self) -> &'static Timing {
        match self {
            Model::Mos6569 | Model::Mos8565 => &PAL,
            Model::Mos6567R8 | Model::Mos8562 => &NTSC,
        }
    }

    /// The HMOS-II types show a light grey pixel where a colour register
    /// changes (VICE test program VICII/greydot; colorsplit references
    /// `-8565`).
    pub fn grey_dot(self) -> bool {
        matches!(self, Model::Mos8565 | Model::Mos8562)
    }
}

impl Timing {
    /// CPU clock (φ0 = dot clock / 8).
    pub fn cpu_hz(&self) -> u32 {
        self.pixel_clock_hz / 8
    }

    pub fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: self.pixel_clock_hz,
            active_pixels: self.width,
            total_pixels: u32::from(self.cycles) * 8,
            lines_per_field: u32::from(self.lines),
            standard: self.standard,
            output: OutputType::SVideo,
            field: FieldParity::Progressive,
        }
    }

    /// X coordinate of pixel `p` (0 = first pixel of cycle 1) in a line.
    pub fn x_at(&self, p: u16) -> u16 {
        let l = self.first_x + p;
        let l = match self.x_stall {
            Some(s) if l >= s + 12 => l - 8,
            Some(s) if l >= s => s + (l - s) % 4,
            _ => l,
        };
        l % self.x_wrap
    }

    /// X coordinate of the first pixel of `cycle`.
    pub fn cycle_x(&self, cycle: u8) -> u16 {
        self.x_at((u16::from(cycle) - 1) * 8)
    }

    /// Frame column of the first pixel `cycle` draws, clamped to the
    /// picture (the beam position's `hpos`).
    #[inline]
    pub fn cycle_column(&self, cycle: u8) -> u16 {
        let p0 = (u16::from(cycle.max(1)) - 1) * 8;
        p0.saturating_sub(self.visible_p0)
            .min(self.width as u16 - 1)
    }

    /// First cycle (p-access) of sprite `n`'s fetch slot: sprite 0 at
    /// `sprite0_slot`, the others every two cycles after it, around the
    /// end of the line.
    fn sprite_slot(&self, n: usize) -> u8 {
        (self.sprite0_slot + 2 * n as u8 - 1) % self.cycles + 1
    }

    /// Is `cycle` within `[start, start + len)` on the cycle ring?
    fn in_ring(&self, cycle: u8, start: u8, len: u8) -> bool {
        let d = (i16::from(cycle) - i16::from(start)).rem_euclid(i16::from(self.cycles));
        d < i16::from(len)
    }

    /// Frame-buffer row of a raster line, if visible.
    fn row(&self, raster: u16) -> Option<usize> {
        let row = (raster + self.lines - self.first_line) % self.lines;
        (u32::from(row) < self.height).then_some(usize::from(row))
    }
}

/// Colour sources of a pixel before the colour registers are looked up:
/// 0–15 a colour number from video/colour RAM, the rest a register.
mod src {
    pub const BG0: u8 = 16;
    pub const BORDER: u8 = 20;
    pub const MM0: u8 = 21;
    pub const SPRITE0: u8 = 23;
}

pub mod irq {
    pub const RASTER: u8 = 0x01;
    pub const SPRITE_DATA: u8 = 0x02;
    pub const SPRITE_SPRITE: u8 = 0x04;
    pub const LIGHT_PEN: u8 = 0x08;
}

/// What the VIC-II sees of memory: a 16 KB bank (RAM or character ROM, as
/// the machine wires it) and the colour RAM on the upper 4 data lines.
pub trait VicMemory {
    /// 14-bit address within the current bank.
    fn read(&self, addr: u16) -> u8;
    /// Colour RAM nibble at a 10-bit index.
    fn color(&self, index: u16) -> u8;
    /// The byte on the CPU's data bus in this cycle, if the machine knows
    /// it: in the three cycles after BA falls a c-access gets its colour
    /// bits from there.
    fn cpu_data(&self) -> Option<u8> {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sprite {
    x: u16,
    y: u8,
    color: u8,
    ptr: u8,
    mc: u8,
    mcbase: u8,
    dma: bool,
    display: bool,
    /// Y expansion flip-flop.
    exp: bool,
    /// The three bytes fetched for the next line.
    data: u32,
    /// Shifter: remaining data, bits left and expansion/multicolour phase.
    shift: u32,
    bits_left: u8,
    xexp_phase: bool,
    mc_phase: bool,
    /// Current 2-bit (or 1-bit) value being output.
    current: u8,
    /// Multicolour mode on the previous pixel.
    was_mc: bool,
    /// MCBASE was crunched in this line's cycle 15 (only between cycles 15
    /// and 16, so not saved).
    crunched: bool,
}

/// Graphics data waiting to be loaded into the sequencer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pending {
    g: u8,
    c: u16,
    /// Pixels since the g-access.
    age: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vic2 {
    model: Model,

    // Registers.
    sprite_x_msb: u8,
    ctrl1: u8,
    raster_cmp: u16,
    lp_x: u8,
    lp_y: u8,
    sprite_enable: u8,
    ctrl2: u8,
    sprite_ye: u8,
    mem_ptrs: u8,
    irq_flags: u8,
    irq_mask: u8,
    sprite_prio: u8,
    sprite_mc: u8,
    sprite_xe: u8,
    coll_mm: u8,
    coll_md: u8,
    border_color: u8,
    bg: [u8; 4],
    sprite_mm: [u8; 2],
    sprites: [Sprite; 8],

    // Beam.
    raster: u16,
    cycle: u8,
    frame: u64,

    // Sequencer.
    vc: u16,
    vcbase: u16,
    rc: u8,
    vmli: usize,
    display_state: bool,
    bad_lines_enabled: bool,
    bad_line: bool,
    matrix: [u16; 40],
    pending: [Option<Pending>; 2],
    sr: u8,
    sr_c: u16,
    sr_mc_phase: bool,
    sr_pair: u8,
    refresh: u8,

    // Border.
    main_border: bool,
    vert_border: bool,
    /// The vertical border flip-flop as it was at the left edge: it stops
    /// the sequencer's loads for the whole line (VICE test program
    /// VICII/border/vborder2: set after the left edge, the line is shown).
    no_load: bool,

    // Bus.
    ba: bool,
    ba_low_cycles: u8,
    last_bus: u8,
    raster_irq_done: bool,
    /// The raster compare value at the start of the previous cycle.
    cmp_held: u16,
    /// Which sprite is fetched in the current cycle's phi2, if any.
    phi2_sprite: Option<usize>,

    pixels: Vec<u8>,
    /// The last four pixels of the previous cycle: index into `pixels`
    /// (`usize::MAX` off the picture) and colour source ([`src`]), resolved
    /// with the registers as they are after this cycle's CPU write.
    lag: [(usize, u8); 4],
    /// Colour sources whose register the CPU wrote in this cycle (bit
    /// `src - src::BG0`), and their values before it: the first pixel of
    /// the new colour shows the old one (6569/6567) or light grey
    /// (8565/8562).
    written: u16,
    before: [u8; 15],
    /// $D01B, $D01D and $D01C as they were before this cycle's CPU write
    /// (equal to the registers between cycles, so not saved).
    cycle_regs: [u8; 3],
    /// Sprite X positions as they were before this cycle's CPU write: the
    /// comparison uses them for the cycle's first four pixels (VICE test
    /// program VICII/spritesplit/ss-xpos). Equal to the registers between
    /// cycles, so not saved.
    cycle_sprite_x: [u16; 8],
    /// $D011 and $D016 as they were before this cycle's CPU write (equal to
    /// the registers between cycles, so not saved).
    cycle_mode: (u8, u8),
    /// $D011 as the next g-access sees it.
    g_ctrl1: u8,
    /// The first bit of a pair during a change of MCM (only within a cycle,
    /// so not saved).
    late_bit: u8,
    /// Sprites whose DMA started in this cycle's phi1 (only between phi1
    /// and phi2, so not saved).
    dma_started: u8,
    /// Collisions of the previous cycle's last four pixels, not yet in
    /// the registers.
    coll_late: (u8, u8),
    /// The sprite whose row fetch completed in this cycle's phi2 (only
    /// within a cycle, so not saved).
    fetch_done: Option<usize>,
    /// Sprites whose p-access is in this cycle (only within a cycle, so
    /// not saved).
    fetching: u8,
    /// The byte of a CPU access to a register in this cycle: it is on the
    /// internal bus in phi2 (only within a cycle, so not saved).
    reg_bus: Option<u8>,

    /// The light pen latched in this frame (the LP input works once a
    /// frame, Bauer 3.11).
    lp_done: bool,
    /// The LP line is held low from outside (control port 1's fire button,
    /// CIA 1 PB4): a pen's pulse makes no edge then.
    lp_low: bool,
    /// A light pen edge waits for the next cycle.
    lp_pending: bool,
    /// Where a light pen points (configuration from the port device, not
    /// state: the machine sets it again after loading) and the colour
    /// indices bright enough to light it (bit n = colour n).
    pen: Option<PenTarget>,
    pen_lit: u16,

    /// Debug overlay record (not machine state; see `crate::overlay`).
    rec: crate::overlay::Recorder<overlay::Record>,
}

/// The beam position at which a light pen sees its pixel drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PenTarget {
    raster: u16,
    cycle: u8,
    /// Frame column and the pixel's index in [`Vic2::pixels`].
    col: u16,
    index: usize,
}

impl Default for Vic2 {
    fn default() -> Self {
        Self::new(Model::default())
    }
}

impl Vic2 {
    pub fn new(model: Model) -> Self {
        let t = model.timing();
        Self {
            model,
            sprite_x_msb: 0,
            ctrl1: 0,
            raster_cmp: 0,
            lp_x: 0,
            lp_y: 0,
            sprite_enable: 0,
            ctrl2: 0,
            sprite_ye: 0,
            mem_ptrs: 0,
            irq_flags: 0,
            irq_mask: 0,
            sprite_prio: 0,
            sprite_mc: 0,
            sprite_xe: 0,
            coll_mm: 0,
            coll_md: 0,
            border_color: 0,
            bg: [0; 4],
            sprite_mm: [0; 2],
            sprites: [Sprite::default(); 8],
            raster: 0,
            cycle: 1,
            frame: 0,
            vc: 0,
            vcbase: 0,
            rc: 0,
            vmli: 0,
            display_state: false,
            bad_lines_enabled: false,
            bad_line: false,
            matrix: [0; 40],
            pending: [None; 2],
            sr: 0,
            sr_c: 0,
            sr_mc_phase: false,
            sr_pair: 0,
            refresh: 0xff,
            main_border: true,
            vert_border: true,
            no_load: true,
            ba: true,
            ba_low_cycles: 0,
            last_bus: 0xff,
            raster_irq_done: false,
            cmp_held: 0,
            phi2_sprite: None,
            pixels: vec![0; (t.width * t.height) as usize],
            lag: [(usize::MAX, 0); 4],
            written: 0,
            before: [0; 15],
            cycle_regs: [0; 3],
            cycle_sprite_x: [0; 8],
            cycle_mode: (0, 0),
            late_bit: 0,
            g_ctrl1: 0,
            dma_started: 0,
            coll_late: (0, 0),
            fetch_done: None,
            fetching: 0,
            reg_bus: None,
            lp_done: false,
            lp_low: false,
            lp_pending: false,
            pen: None,
            pen_lit: 0,
            rec: crate::overlay::Recorder(None),
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn timing(&self) -> &'static Timing {
        self.model.timing()
    }

    #[inline]
    pub fn beam(&self) -> BeamPos {
        BeamPos {
            frame: self.frame,
            line: self.raster,
            cycle: u16::from(self.cycle - 1),
            hpos: self.timing().cycle_column(self.cycle),
        }
    }

    pub fn raster(&self) -> u16 {
        self.raster
    }

    /// Cycle within the line, 1–63 (6569) or 1–65 (6567R8).
    pub fn cycle(&self) -> u8 {
        self.cycle
    }

    /// BA output for the current cycle (false = CPU must stop on reads).
    pub fn ba(&self) -> bool {
        self.ba
    }

    /// AEC in this cycle's φ2: the CPU drives the address bus. Three cycles
    /// after BA goes low the VIC-II takes the bus; a CPU halted on a read
    /// then selects no chip (its repeated read of e.g. an ICR has no
    /// effect). Source: Christian Bauer, "The MOS 6567/6569 video
    /// controller", section 3.6.
    pub fn aec(&self) -> bool {
        self.ba_low_cycles <= 3
    }

    /// IRQ output.
    pub fn irq(&self) -> bool {
        self.irq_flags & self.irq_mask & 0x0f != 0
    }

    /// The byte the VIC-II last read: what the CPU sees on an open bus.
    pub fn last_bus(&self) -> u8 {
        self.last_bus
    }

    /// Indexed pixels of the visible window, `width × height` of the timing.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    fn den(&self) -> bool {
        self.ctrl1 & 0x10 != 0
    }

    fn yscroll(&self) -> u16 {
        u16::from(self.ctrl1 & 7)
    }

    fn update_bad_line(&mut self) {
        if self.raster == 0x30 && self.den() {
            self.bad_lines_enabled = true;
        }
        self.bad_line = self.bad_lines_enabled
            && (0x30..=0xf7).contains(&self.raster)
            && self.raster & 7 == self.yscroll();
    }

    fn vm_base(&self) -> u16 {
        u16::from(self.mem_ptrs >> 4) << 10
    }

    fn char_base(&self) -> u16 {
        u16::from((self.mem_ptrs >> 1) & 7) << 11
    }

    /// RASTER as read: in cycle 1 of line 0 it still holds the last line
    /// (VICE test program VICII/split-tests/lightpen, dumps of real chips;
    /// the raster compare of line 0 also waits for cycle 2).
    fn raster_reg(&self) -> u16 {
        if self.raster == 0 && self.cycle == 1 {
            self.timing().lines - 1
        } else {
            self.raster
        }
    }

    fn check_raster_irq(&mut self) {
        self.compare_raster(self.raster_cmp);
    }

    fn compare_raster(&mut self, cmp: u16) {
        if self.raster == cmp {
            if !self.raster_irq_done {
                self.raster_irq_done = true;
                self.irq_flags |= irq::RASTER;
            }
        } else {
            self.raster_irq_done = false;
        }
    }

    /// First half of a cycle: VIC-II memory access and the BA level that
    /// applies to the CPU in this cycle.
    pub fn phi1(&mut self, mem: &dyn VicMemory) {
        let c = self.cycle;
        let t = self.timing();
        if std::mem::take(&mut self.lp_pending) {
            self.light_pen_sampled();
        }
        // The light pen input is shut off from the last line of the frame
        // to cycle 2 of line 0 (an edge sampled before cycle 3 is lost); in
        // cycle 2 a line still held low latches X = cycle 3's minus 2 (the
        // dumps of 6569, 8565, 6567 and 8562 in VICE test program
        // VICII/split-tests/lightpen).
        if self.raster == 0 && c == 2 && self.lp_low {
            let x = t.x_at(16 - 2);
            self.latch_light_pen(x);
        }

        // The raster compare runs in cycle 1, for line 0 one cycle later,
        // with the compare value of the previous cycle's start: a $D012
        // write in the last cycle of a line comes too late for the next
        // line (VICE test program VICII/rasterirq/rasterirq_hold).
        if c == 1 && self.raster != 0 || c == 2 && self.raster == 0 {
            self.compare_raster(self.cmp_held);
        }
        self.cmp_held = self.raster_cmp;
        self.update_bad_line();

        // Vertical border: the comparison works in every cycle of the line,
        // before the CPU's write of this cycle, besides the check at the
        // left edge (VICE test programs VICII/border/vborder2: RSEL cleared
        // for four cycles anywhere in line 247, up to its cycle 63, closes
        // the border; a write in cycle 63 counts for the next line).
        let (top, bottom) = self.border_rows();
        if self.raster == bottom {
            self.vert_border = true;
        } else if self.raster == top && self.den() {
            self.vert_border = false;
        }

        // Sprite DMA start (cycles 55/56); the Y expansion flip-flop turns
        // in cycle 55's phi2 (see `phi2`).
        if c == t.dma_check || c == t.dma_check + 1 {
            self.dma_started = self.sprite_dma_check();
        }
        if c == 58 {
            for (n, s) in self.sprites.iter_mut().enumerate() {
                s.mc = s.mcbase;
                // A sprite disabled again before cycle 58 is not shown, but
                // its DMA runs and takes its cycles (VICE test programs
                // VICII/spriteenable 1, 2 and 4).
                if s.dma && s.y == self.raster as u8 && self.sprite_enable & (1 << n) != 0 {
                    s.display = true;
                }
                if !s.dma {
                    s.display = false;
                }
            }
            if self.rc == 7 {
                self.vcbase = self.vc;
                if !self.bad_line {
                    self.display_state = false;
                }
            }
            if self.display_state {
                self.rc = (self.rc + 1) & 7;
            }
        }
        if c == 14 {
            self.vc = self.vcbase;
            self.vmli = 0;
            if self.bad_line {
                self.rc = 0;
            }
        }
        if c == 15 {
            for s in self.sprites.iter_mut() {
                if s.exp {
                    s.mcbase = (s.mcbase + 2) & 63;
                }
            }
        }
        if c == 16 {
            for s in self.sprites.iter_mut() {
                // A crunched MCBASE (cycle 15) takes no further step.
                if s.exp && !std::mem::take(&mut s.crunched) {
                    s.mcbase = (s.mcbase + 1) & 63;
                }
                // DMA ends here; the row already fetched is still shown, the
                // display turns off with the line evaluation in cycle 58.
                if s.mcbase == 63 {
                    s.dma = false;
                }
            }
        }
        for (n, s) in self.sprites.iter_mut().enumerate() {
            // The expansion flip-flop stays set while MxYE is clear.
            if self.sprite_ye & (1 << n) == 0 {
                s.exp = true;
            }
        }

        // BA: bad line from cycle 12 to 54; sprites from 3 cycles before their
        // slot to the end of its second cycle.
        let mut ba_low = self.bad_line && (12..=54).contains(&c);
        for n in 0..8 {
            let start = (t.sprite_slot(n) + t.cycles - 1 - 3) % t.cycles + 1;
            if self.sprites[n].dma && t.in_ring(c, start, 5) {
                ba_low = true;
            }
        }
        self.ba = !ba_low;
        self.ba_low_cycles = if ba_low {
            self.ba_low_cycles.saturating_add(1)
        } else {
            0
        };

        // phi1 access.
        self.phi2_sprite = None;
        let mut phi1_value = 0xff;
        for n in 0..8 {
            let slot = t.sprite_slot(n);
            if c == slot {
                let p = mem.read(self.vm_base() | 0x3f8 | n as u16);
                self.sprites[n].ptr = p;
                phi1_value = p;
                if self.sprites[n].dma {
                    self.phi2_sprite = Some(n);
                    self.fetching |= 1 << n;
                } else {
                    self.sprites[n].data = 0xff_ffff;
                }
            } else if c == slot % t.cycles + 1 && self.sprites[n].dma {
                phi1_value = self.sprite_fetch(mem, n);
                self.phi2_sprite = Some(n);
            } else if c == slot % t.cycles + 1 {
                // Without DMA the sprite's data latches still take what is
                // on the bus in its slot: $FF in both CPU halves (or the
                // byte of a CPU access to a VIC-II register, see `phi2`) and
                // the idle access in between. A sprite at X >= $164 is shown
                // in the line DMA starts and displays that (VICE test
                // programs VICII/sb_sprite_fetch, readme).
                let addr = if self.ctrl1 & 0x40 != 0 {
                    0x39ff
                } else {
                    0x3fff
                };
                let g = mem.read(addr);
                let s = &mut self.sprites[n];
                s.data = s.data & 0xff_00ff | u32::from(g) << 8;
            }
        }
        if (11..=15).contains(&c) {
            phi1_value = mem.read(0x3f00 | u16::from(self.refresh));
            self.refresh = self.refresh.wrapping_sub(1);
        }
        if (16..=55).contains(&c) {
            phi1_value = self.g_access(mem);
        }
        // The display logic sees a bad line one cycle after BA does: a
        // bad line made by a $D011 write in cycle 15 has BA low from cycle
        // 16 and display state from cycle 17's g-access (VICE test program
        // VICII/colorfetchbug, readme).
        // A bad line made after cycle 54 no longer switches the display on
        // (VICE test programs VICII/fldscroll: $D011 written in cycle 54
        // makes the line a display line, in cycle 56 not).
        if self.bad_line && c <= 55 {
            self.display_state = true;
        }
        self.last_bus = phi1_value;
    }

    /// Sprite DMA start in cycles 55 and 56, with $D015 as it is before the
    /// CPU's write of the cycle.
    /// Returns the sprites whose DMA starts.
    fn sprite_dma_check(&mut self) -> u8 {
        let mut started = 0;
        for n in 0..8 {
            let s = &mut self.sprites[n];
            if self.sprite_enable & (1 << n) != 0 && s.y == self.raster as u8 && !s.dma {
                started |= 1 << n;
                s.dma = true;
                s.mcbase = 0;
                if self.sprite_ye & (1 << n) != 0 {
                    s.exp = false;
                }
            }
        }
        started
    }

    fn sprite_fetch(&mut self, mem: &dyn VicMemory, n: usize) -> u8 {
        let s = &mut self.sprites[n];
        let v = mem.read(u16::from(s.ptr) << 6 | u16::from(s.mc));
        s.mc = (s.mc + 1) & 63;
        s.data = (s.data << 8 | u32::from(v)) & 0xff_ffff;
        v
    }

    fn g_access(&mut self, mem: &dyn VicMemory) -> u8 {
        // ECM and BMM reach the address one cycle after the output: the
        // access after a write still uses the old mode. On the older types
        // that holds for switching off only (VICE test program
        // VICII/vicii_timing).
        let mode = if self.model.grey_dot() {
            self.g_ctrl1
        } else {
            self.ctrl1 | self.g_ctrl1
        };
        let ecm = mode & 0x40 != 0;
        let bmm = mode & 0x20 != 0;
        let (addr, c) = if self.display_state {
            let c = self.matrix[self.vmli];
            let addr = if bmm {
                (u16::from(self.mem_ptrs & 0x08) << 10) | (self.vc << 3) | u16::from(self.rc)
            } else {
                self.char_base() | ((c & 0xff) << 3) | u16::from(self.rc)
            };
            (addr, c)
        } else if self.bad_line && self.cycle > 17 {
            // The cycle a bad line is made in idle state (DMA delay): the
            // display logic has not switched yet, but the idle access
            // already comes from elsewhere — $38FF on the 6569 and 6567R8,
            // $3807 on the 8565/8562 (VICE test program VICII/vsp-tester,
            // readme with measurements of real chips); the accesses of cycles 16 and 17 stay
            // $3FFF (VICII/colorfetchbug).
            (
                if self.model.grey_dot() {
                    0x3807
                } else {
                    0x38ff
                },
                0,
            )
        } else {
            (0x3fff, 0)
        };
        let addr = if ecm { addr & 0x39ff } else { addr };
        let g = mem.read(addr);
        if self.display_state {
            self.vc = (self.vc + 1) & 0x3ff;
            self.vmli = (self.vmli + 1) % 40;
        }
        let p = Pending { g, c, age: 0 };
        if self.pending[0].is_none() {
            self.pending[0] = Some(p);
        } else {
            self.pending[1] = Some(p);
        }
        g
    }

    /// Second half of a cycle: the VIC-II's own phi2 access (c-access on
    /// bad lines, sprite data), pixel output and advance to the next cycle.
    /// Returns true when a new field starts.
    pub fn phi2(&mut self, mem: &dyn VicMemory) -> bool {
        let c = self.cycle;
        let t = self.timing();
        // The Y expansion flip-flop turns in cycle 55 with $D017 as it is
        // after the CPU's write; a $D017 set as late as cycle 55 still
        // counts (VICE test programs VICII/spritecrunch/spritecrunch2). A
        // sprite whose DMA started in this cycle keeps its cleared
        // flip-flop.
        if c == t.dma_check {
            let started = std::mem::take(&mut self.dma_started);
            for (n, s) in self.sprites.iter_mut().enumerate() {
                if self.sprite_ye & (1 << n) != 0 && started & (1 << n) == 0 {
                    s.exp = !s.exp;
                }
            }
        }
        if let Some(v) = self.reg_bus.take() {
            for n in 0..8 {
                let slot = t.sprite_slot(n);
                let s = &mut self.sprites[n];
                if s.dma {
                    continue;
                }
                if c == slot {
                    s.data = s.data & 0x00_ffff | u32::from(v) << 16;
                } else if c == slot % t.cycles + 1 {
                    s.data = s.data & 0xff_ff00 | u32::from(v);
                }
            }
        }
        if let Some(n) = self.phi2_sprite {
            let v = if self.ba_low_cycles > 3 {
                self.sprite_fetch(mem, n)
            } else {
                let s = &mut self.sprites[n];
                s.mc = (s.mc + 1) & 63;
                s.data = (s.data << 8 | 0xff) & 0xff_ffff;
                0xff
            };
            self.last_bus = v;
            // The row's third byte arrives with this cycle's pixel 3: a
            // sprite triggered again before that shows nothing (VICE test
            // program VICII/spritex/testsuite 14-17, Krestage 3).
            if c != t.sprite_slot(n) {
                self.fetch_done = Some(n);
            }
        }
        // A c-access needs BA low since phi1: a $D011 write that makes the
        // line a bad line in this cycle starts the accesses in the next
        // (VICE test program VICII/colorfetchbug, readme).
        if self.bad_line && (15..=54).contains(&c) && self.ba_low_cycles > 0 {
            // The first three cycles after BA the CPU may still own the bus;
            // the VIC-II then reads $FF and takes the colour bits from the
            // low nibble of the CPU's data bus: a CPU halted on an opcode
            // fetch of TXA ($8A) gives colour 10 (VICE test program
            // VICII/colorfetchbug).
            let v = if self.ba_low_cycles > 3 {
                let addr = self.vm_base() | self.vc;
                u16::from(mem.read(addr)) | u16::from(mem.color(self.vc)) << 8
            } else {
                let color = mem
                    .cpu_data()
                    .map_or_else(|| mem.color(self.vc), |d| d & 0x0f);
                0xff | u16::from(color) << 8
            };
            self.matrix[self.vmli] = v;
            if let Some(r) = self.rec.0.as_deref_mut() {
                r.mark_bad();
            }
        }

        self.draw();
        if let Some(pen) = self.pen {
            if pen.cycle == c && pen.raster == self.raster {
                self.pen_sees(pen);
            }
        }

        if c == t.cycles {
            if let Some(r) = self.rec.0.as_deref_mut() {
                r.end_line(self.raster);
                if self.raster + 1 == t.lines {
                    r.end_field();
                }
            }
            self.cycle = 1;
            self.raster += 1;
            if self.raster == t.lines {
                self.raster = 0;
                self.frame += 1;
                self.lp_done = false;
                self.vcbase = 0;
                self.bad_lines_enabled = false;
                return true;
            }
        } else {
            self.cycle = c + 1;
        }
        false
    }

    fn border_rows(&self) -> (u16, u16) {
        if self.ctrl1 & 0x08 != 0 {
            (51, 251)
        } else {
            (55, 247)
        }
    }

    fn border_cols(&self) -> (u16, u16) {
        if self.ctrl2 & 0x08 != 0 {
            (24, 344)
        } else {
            (31, 335)
        }
    }

    fn draw(&mut self) {
        let t = self.timing();
        let p0 = (u16::from(self.cycle) - 1) * 8;
        let (left, right) = self.border_cols();
        let (top, bottom) = self.border_rows();
        let row = t.row(self.raster).map(|r| r * t.width as usize);
        let mut out = [(usize::MAX, 0u8); 8];
        let (mm, md) = std::mem::take(&mut self.coll_late);
        self.collide(mm, md);

        for (i, p) in (p0..p0 + 8).enumerate() {
            let x = t.x_at(p);

            // Border flip-flops.
            if x == right {
                self.main_border = true;
            }
            if x == left {
                if self.raster == bottom {
                    self.vert_border = true;
                } else if self.raster == top && self.den() {
                    self.vert_border = false;
                }
                if !self.vert_border {
                    self.main_border = false;
                }
                self.no_load = self.vert_border;
            }

            let (gfx_color, gfx_fg) = self.graphics_pixel(i);
            let (sprite, colliding) = self.sprite_pixel(x, i);

            // Collisions reach the registers with the pixel output, four
            // pixels after the cycle's own: those of the last four pixels
            // count from the next cycle (VICE test programs
            // VICII/spritecollisions/*-collision-cycle).
            let (mm, md) = (
                if colliding.count_ones() >= 2 {
                    colliding
                } else {
                    0
                },
                if gfx_fg { colliding } else { 0 },
            );
            if i < 4 {
                self.collide(mm, md);
            } else {
                self.coll_late.0 |= mm;
                self.coll_late.1 |= md;
            }

            let mut color = gfx_color;
            // $D01B takes effect from the cycle's pixel 2 (VICE test
            // program VICII/spritesplit/ss-pri).
            let prio = if i < 2 {
                self.cycle_regs[0]
            } else {
                self.sprite_prio
            };
            if let Some((n, sc)) = sprite {
                if !(gfx_fg && prio & (1 << n) != 0) {
                    color = sc;
                }
            }
            if self.main_border {
                color = src::BORDER;
            }

            if let Some(row) = row {
                let col = p.wrapping_sub(t.visible_p0);
                if u32::from(col) < t.width {
                    out[i] = (row + usize::from(col), color);
                }
            }
        }

        // The colour registers act on the output four pixels before the
        // cycle's own pixels: the previous cycle's last four pixels and
        // this cycle's first four get the registers after this cycle's
        // write (VICE test programs VICII/colorsplit, spritesplit). The
        // first of them still shows the old colour if its register was just
        // written; the HMOS-II types show light grey there.
        let lag = self.lag;
        for (k, &(index, source)) in lag.iter().chain(&out[..4]).enumerate() {
            if index != usize::MAX {
                let bit = source.wrapping_sub(src::BG0);
                self.pixels[index] = if k == 0 && bit < 15 && self.written & 1 << bit != 0 {
                    if self.model.grey_dot() {
                        15
                    } else {
                        self.before[usize::from(bit)]
                    }
                } else {
                    self.resolve(source)
                };
            }
        }
        self.lag.copy_from_slice(&out[4..]);
        self.written = 0;
        self.fetch_done = None;
        self.fetching = 0;
        self.cycle_regs = [self.sprite_prio, self.sprite_xe, self.sprite_mc];
        self.cycle_sprite_x = self.sprites.map(|s| s.x);
        self.g_ctrl1 = self.cycle_mode.0;
        self.cycle_mode = (self.ctrl1, self.ctrl2);
    }

    /// Sprite-sprite (`mm`) and sprite-data (`md`) collisions: the first
    /// one since the register was read raises the interrupt flag.
    fn collide(&mut self, mm: u8, md: u8) {
        if mm != 0 {
            if self.coll_mm == 0 {
                self.irq_flags |= irq::SPRITE_SPRITE;
            }
            self.coll_mm |= mm;
        }
        if md != 0 {
            if self.coll_md == 0 {
                self.irq_flags |= irq::SPRITE_DATA;
            }
            self.coll_md |= md;
        }
    }

    /// Colour number of a colour source.
    fn resolve(&self, source: u8) -> u8 {
        match source {
            0..=15 => source,
            src::BORDER => self.border_color,
            s if s < src::BORDER => self.bg[usize::from(s - src::BG0)],
            s if s < src::SPRITE0 => self.sprite_mm[usize::from(s - src::MM0)],
            s => self.sprites[usize::from(s - src::SPRITE0) & 7].color,
        }
    }

    /// Next pixel of the graphics sequencer: (colour, is foreground).
    fn graphics_pixel(&mut self, i: usize) -> (u8, bool) {
        // Loaded into the shifter XSCROLL + 4 pixels after the g-access, with
        // XSCROLL as it is at that pixel: a byte whose position a change has
        // passed is lost (VICE test program VICII/vicii_timing).
        let due = 4 + (self.ctrl2 & 7);
        if self.pending[0].is_some_and(|p| p.age > due && p.age >= 12) {
            self.pending[0] = self.pending[1].take();
        }
        if self.pending[1].is_some_and(|p| p.age == due) {
            self.pending[0] = self.pending[1].take();
        }
        if let Some(p) = self.pending[0] {
            if p.age == due {
                // While the vertical border flip-flop is set the sequencer
                // loads nothing: the shifter runs empty and the colour data
                // of the last load stays (visible where the side border is
                // open; VICE test programs VICII/border, readme.txt:
                // border-bm-idle, hvborder1/2).
                if !self.no_load {
                    self.sr = p.g;
                    self.sr_c = p.c;
                    self.sr_mc_phase = false;
                }
                self.pending[0] = self.pending[1].take();
            }
        }
        for p in self.pending.iter_mut().flatten() {
            p.age += 1;
        }

        // A mode change reaches the output late, in the cycle of the write:
        // ECM and BMM four pixels on the HMOS-II types, on the others two
        // when switched off; MCM on all of them four black pixels of a
        // multicolour cell when switched on, three of high-resolution pairs when switched off (VICE
        // test programs VICII/vicii_timing, VICII/videomode, references
        // compared with real chips).
        let (old1, old2) = self.cycle_mode;
        let mode_bit = |new: u8, old: u8, bit: u8, grey: bool| {
            let (n, o) = (new & bit != 0, old & bit != 0);
            let late = match (grey, o && !n) {
                (true, _) => 4,
                (false, true) => 2,
                (false, false) => 0,
            };
            if i < late {
                o
            } else {
                n
            }
        };
        let grey = self.model.grey_dot();
        let ecm = mode_bit(self.ctrl1, old1, 0x40, grey);
        let bmm = mode_bit(self.ctrl1, old1, 0x20, grey);
        let mcm = self.ctrl2 & 0x10 != 0;
        let mcm_late = i < 3 + usize::from(mcm) && (self.ctrl2 ^ old2) & 0x10 != 0;
        let c = self.sr_c;
        let cc = (c & 0xff) as u8;
        let color = (c >> 8) as u8 & 0x0f;
        let multicolor = mcm && (bmm || color & 0x08 != 0);

        let (bits, fg) = if multicolor {
            if !self.sr_mc_phase {
                self.sr_pair = self.sr >> 6;
                self.sr <<= 2;
            }
            self.sr_mc_phase = !self.sr_mc_phase;
            (self.sr_pair, self.sr_pair & 2 != 0)
        } else {
            let b = self.sr >> 7;
            self.sr <<= 1;
            if i & 1 == 0 {
                self.late_bit = b;
            }
            if mcm_late && (bmm || color & 0x08 != 0) {
                (self.late_bit, self.late_bit != 0)
            } else {
                (b, b != 0)
            }
        };

        let out = match (ecm, bmm, mcm) {
            (false, false, false) => {
                if bits != 0 {
                    color
                } else {
                    src::BG0
                }
            }
            (false, false, true) => {
                if !multicolor {
                    if bits != 0 {
                        color & 7
                    } else {
                        src::BG0
                    }
                } else {
                    match bits {
                        0 => src::BG0,
                        1 => src::BG0 + 1,
                        2 => src::BG0 + 2,
                        _ => color & 7,
                    }
                }
            }
            (false, true, false) => {
                if bits != 0 {
                    cc >> 4
                } else {
                    cc & 0x0f
                }
            }
            (false, true, true) => match bits {
                0 => src::BG0,
                1 => cc >> 4,
                2 => cc & 0x0f,
                _ => color,
            },
            (true, false, false) => {
                if bits != 0 {
                    color
                } else {
                    src::BG0 + (cc >> 6)
                }
            }
            // Invalid modes output black; collisions still work.
            _ => 0,
        };
        if mcm_late && multicolor && !bmm {
            return (0, false);
        }
        (out, fg)
    }

    /// Sprite output at X for pixel `i` of the cycle: (winning sprite and
    /// colour, mask of opaque sprites).
    ///
    /// The shifter moves one bit per pixel (every second pixel when
    /// X-expanded); in multicolour mode the two top bits are latched every
    /// second step. A write to $D01D takes effect from the cycle's pixel 2,
    /// to $D01C from pixel 3. Multicolour switched on at the start of a
    /// step latches first on the step after, so the last high-resolution
    /// value stays one step longer; a pair latched on the
    /// last step still shows its second step (VICE test programs
    /// VICII/spritesplit, compared pixel by pixel with their references).
    fn sprite_pixel(&mut self, x: u16, i: usize) -> (Option<(usize, u8)>, u8) {
        let t = self.timing();
        let [_, old_xe, old_mc] = self.cycle_regs;
        let xe_reg = if i < 2 { old_xe } else { self.sprite_xe };
        let mc_reg = if i < 3 { old_mc } else { self.sprite_mc };
        let mut winner = None;
        let mut opaque = 0u8;
        for n in 0..8 {
            let s = &mut self.sprites[n];
            // A row being shifted out when the display turns off in cycle 58
            // still runs to its end (VICE test programs VICII/border/hvborder).
            if !s.display && s.bits_left == 0 {
                continue;
            }
            let mc = mc_reg & (1 << n) != 0;
            let xe = xe_reg & (1 << n) != 0;
            let sx = if i < 4 { self.cycle_sprite_x[n] } else { s.x };
            // No trigger while the row is fetched: in the p-access cycle
            // and up to pixel 2 of the next (VICE test programs
            // VICII/spritex/testsuite 14-16 on PAL and NTSC).
            // The pixel before the p-access already shows nothing (VICE test
            // programs VICII/spritegap: logs of real chips).
            let gap = i == 7 && t.sprite_slot(n) == self.cycle % t.cycles + 1;
            let not_yet =
                gap || self.fetching & (1 << n) != 0 || (i < 3 && self.fetch_done == Some(n));
            if s.display && s.bits_left == 0 && sx == x && !not_yet {
                if let Some(r) = self.rec.0.as_deref_mut() {
                    r.sprite_shown(n, self.raster, x, self.sprite_xe & (1 << n) != 0);
                }
                // The fetched row is shifted out once: a second X match in
                // the same line shows nothing (VICE test program
                // VICII/spritesplit/ss-xpos).
                s.shift = std::mem::take(&mut s.data);
                s.bits_left = 24;
                s.xexp_phase = false;
                s.mc_phase = false;
                s.was_mc = mc;
            }
            if s.bits_left == 0 {
                continue;
            }
            // Switched on at the start of a step, multicolour latches on
            // the next step; in the middle of an expanded step, at the next.
            if mc && !s.was_mc {
                s.mc_phase = !s.xexp_phase;
            }
            s.was_mc = mc;
            if !xe {
                s.xexp_phase = false;
            }
            // A step starts on every pixel, every second one when expanded.
            if !s.xexp_phase {
                if !mc {
                    s.current = ((s.shift >> 23) & 1) as u8 * 2;
                } else if !s.mc_phase {
                    s.current = (s.shift >> 22) as u8 & 3;
                }
            }
            let value = s.current;
            let step_ends = !xe || s.xexp_phase;
            if xe {
                s.xexp_phase = !s.xexp_phase;
            }
            if step_ends {
                s.shift = (s.shift << 1) & 0xff_ffff;
                s.bits_left -= 1;
                if mc {
                    s.mc_phase = !s.mc_phase;
                    // The second half of a pair latched on the last step.
                    if s.bits_left == 0 && s.mc_phase {
                        s.bits_left = 1;
                    }
                }
            }
            if value == 0 || gap {
                continue;
            }
            opaque |= 1 << n;
            if winner.is_none() {
                let color = match value {
                    1 => src::MM0,
                    3 => src::MM0 + 1,
                    _ => src::SPRITE0 + n as u8,
                };
                winner = Some((n, color));
            }
        }
        (winner, opaque)
    }

    /// Side-effect-free register read.
    pub fn peek(&self, reg: u8) -> u8 {
        let reg = reg & 0x3f;
        match reg {
            0x00..=0x0f => {
                let s = &self.sprites[usize::from(reg >> 1)];
                if reg & 1 == 0 {
                    s.x as u8
                } else {
                    s.y
                }
            }
            0x10 => self.sprite_x_msb,
            0x11 => (self.ctrl1 & 0x7f) | ((self.raster_reg() >> 1) as u8 & 0x80),
            0x12 => self.raster_reg() as u8,
            0x13 => self.lp_x,
            0x14 => self.lp_y,
            0x15 => self.sprite_enable,
            0x16 => self.ctrl2 | 0xc0,
            0x17 => self.sprite_ye,
            0x18 => self.mem_ptrs | 0x01,
            0x19 => {
                let any = if self.irq() { 0x80 } else { 0 };
                self.irq_flags | 0x70 | any
            }
            0x1a => self.irq_mask | 0xf0,
            0x1b => self.sprite_prio,
            0x1c => self.sprite_mc,
            0x1d => self.sprite_xe,
            0x1e => self.coll_mm,
            0x1f => self.coll_md,
            0x20 => self.border_color | 0xf0,
            0x21..=0x24 => self.bg[usize::from(reg - 0x21)] | 0xf0,
            0x25 | 0x26 => self.sprite_mm[usize::from(reg - 0x25)] | 0xf0,
            0x27..=0x2e => self.sprites[usize::from(reg - 0x27)].color | 0xf0,
            _ => 0xff,
        }
    }

    /// CPU read: collision registers clear on read.
    pub fn read(&mut self, reg: u8) -> u8 {
        let v = self.peek(reg);
        self.reg_bus = Some(v);
        match reg & 0x3f {
            0x1e => self.coll_mm = 0,
            0x1f => self.coll_md = 0,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        self.reg_bus = Some(v);
        let reg = reg & 0x3f;
        match reg {
            0x00..=0x0f => {
                let n = usize::from(reg >> 1);
                if reg & 1 == 0 {
                    let msb = u16::from(self.sprite_x_msb >> n & 1) << 8;
                    self.sprites[n].x = msb | u16::from(v);
                } else {
                    self.sprites[n].y = v;
                }
            }
            0x10 => {
                self.sprite_x_msb = v;
                for (n, s) in self.sprites.iter_mut().enumerate() {
                    s.x = (s.x & 0xff) | (u16::from(v >> n & 1) << 8);
                }
            }
            0x11 => {
                self.ctrl1 = v & 0x7f;
                self.raster_cmp = (self.raster_cmp & 0xff) | (u16::from(v & 0x80) << 1);
                self.update_bad_line();
                self.check_raster_irq();
            }
            0x12 => {
                self.raster_cmp = (self.raster_cmp & 0x100) | u16::from(v);
                self.check_raster_irq();
            }
            0x15 => self.sprite_enable = v,
            0x16 => self.ctrl2 = v & 0x1f,
            0x17 => {
                let cleared = self.sprite_ye & !v;
                self.sprite_ye = v;
                for (n, s) in self.sprites.iter_mut().enumerate() {
                    // Sprite crunching: Y expansion cleared in cycle 15 while
                    // the expansion flip-flop is clear mixes MCBASE with MC
                    // (Linus Akesson, "Sprite crunching"; VICE test programs
                    // VICII/spritecrunch).
                    if self.cycle == 15 && cleared & (1 << n) != 0 && !s.exp {
                        s.mcbase = (0x2a & (s.mcbase & s.mc)) | (0x15 & (s.mcbase | s.mc));
                        s.crunched = true;
                    }
                    if v & (1 << n) == 0 {
                        s.exp = true;
                    }
                }
            }
            0x18 => self.mem_ptrs = v & 0xfe,
            0x19 => self.irq_flags &= !(v & 0x0f),
            0x1a => self.irq_mask = v & 0x0f,
            0x1b => self.sprite_prio = v,
            0x1c => self.sprite_mc = v,
            0x1d => self.sprite_xe = v,
            0x20..=0x2e => {
                self.colour_written(reg);
                self.write_colour(reg, v);
            }
            // Light pen and collision registers are read-only.
            _ => {}
        }
    }

    /// Note the old value of colour register `reg` at its first write in
    /// this cycle.
    fn colour_written(&mut self, reg: u8) {
        let bit = match reg {
            0x20 => src::BORDER - src::BG0,
            0x21..=0x24 => reg - 0x21,
            0x25 | 0x26 => src::MM0 - src::BG0 + reg - 0x25,
            _ => src::SPRITE0 - src::BG0 + reg - 0x27,
        };
        if self.written & 1 << bit == 0 {
            self.written |= 1 << bit;
            self.before[usize::from(bit)] = self.peek(reg) & 0x0f;
        }
    }

    fn write_colour(&mut self, reg: u8, v: u8) {
        let v = v & 0x0f;
        match reg {
            0x20 => self.border_color = v,
            0x21..=0x24 => self.bg[usize::from(reg - 0x21)] = v,
            0x25 | 0x26 => self.sprite_mm[usize::from(reg - 0x25)] = v,
            _ => self.sprites[usize::from(reg - 0x27) & 7].color = v,
        }
    }

    /// Latch the beam position into the light-pen registers (LP input
    /// falling edge now, at the first pixel of the current cycle).
    ///
    /// The edge is sampled in the next cycle: the registers get that
    /// cycle's raster line and first X coordinate (two pixels less on the
    /// HMOS-II types). An edge in the last line of the frame is ignored
    /// (VICE test programs VICII/lp-trigger/test2, test2new and
    /// VICII/split-tests/lightpen with its dumps of real chips).
    pub fn light_pen(&mut self) {
        if self.raster + 1 != self.timing().lines {
            self.lp_pending = true;
        }
    }

    /// The light pen edge of the previous cycle reaches the registers.
    fn light_pen_sampled(&mut self) {
        let t = self.timing();
        if self.raster == 0 && self.cycle < 3 {
            return;
        }
        let p = (u16::from(self.cycle) - 1) * 8;
        let x = match (self.model.grey_dot(), p) {
            (false, _) => t.x_at(p),
            (true, 2..) => t.x_at(p - 2),
            // Two pixels before cycle 1: the end of the previous line.
            (true, _) => t.x_at(8 * u16::from(t.cycles) - 2),
        };
        self.latch_light_pen(x);
    }

    /// LPX = X/2, LPY = the raster line (bits 0–7) and IRQ latch bit 3;
    /// only the first trigger of a frame counts (Bauer 3.11).
    fn latch_light_pen(&mut self, x: u16) {
        if self.lp_done {
            return;
        }
        self.lp_done = true;
        self.lp_x = (x >> 1) as u8;
        self.lp_y = self.raster as u8;
        self.irq_flags |= irq::LIGHT_PEN;
    }

    /// The LP input's level from the rest of the port (`low`: the fire
    /// button of control port 1 or CIA 1 PB4 pulls it down). A falling
    /// edge latches the beam position now.
    pub fn set_lp_line(&mut self, low: bool) {
        if low && !self.lp_low {
            self.light_pen();
        }
        self.lp_low = low;
    }

    /// A light pen points at frame pixel (`x`, `y`) of [`Vic2::pixels`]
    /// (`None`: no pen, or off the picture); it sees light where the beam
    /// draws a colour whose bit is set in `lit`. In the cycle the beam
    /// draws that pixel, a bright one sends a pulse on LP: the registers
    /// latch the X coordinate of that pixel (2-pixel resolution) and the
    /// raster line. The pen's own response delay is not modelled.
    pub fn set_light_pen(&mut self, aim: Option<(u16, u16)>, lit: u16) {
        let t = self.timing();
        self.pen_lit = lit;
        self.pen = aim.and_then(|(x, y)| {
            if u32::from(x) >= t.width || u32::from(y) >= t.height {
                return None;
            }
            let p = x + t.visible_p0;
            let cycle = (p / 8 + 1) as u8;
            (cycle <= t.cycles).then_some(PenTarget {
                raster: (y + t.first_line) % t.lines,
                cycle,
                col: x,
                index: usize::from(y) * t.width as usize + usize::from(x),
            })
        });
    }

    /// The beam drew the pen's pixel in this cycle.
    #[cold]
    #[inline(never)]
    fn pen_sees(&mut self, pen: PenTarget) {
        let colour = self.pixels[pen.index] & 0x0f;
        if self.pen_lit & 1 << colour != 0 && !self.lp_low {
            let t = self.timing();
            let x = t.x_at(pen.col + t.visible_p0);
            self.latch_light_pen(x);
        }
    }
}

const TAG: [u8; 4] = *b"6569";
/// 2: VIC-II type byte first (version 1 states are 6569). 3: the light
/// pen's once-a-frame flag and the LP line level. 4: types 8565/8562 and
/// the four pixels waiting for their colour. 5: collisions waiting for
/// the registers. 6: a light pen edge waiting for the next cycle. 7: the
/// sequencer's load gate and the held raster compare value.
const VERSION: u16 = 7;

fn save_sprite(w: &mut StateWriter, s: &Sprite) {
    w.u16(s.x);
    w.u8(s.y);
    w.u8(s.color);
    w.u8(s.ptr);
    w.u8(s.mc);
    w.u8(s.mcbase);
    w.bool(s.dma);
    w.bool(s.display);
    w.bool(s.exp);
    w.u32(s.data);
    w.u32(s.shift);
    w.u8(s.bits_left);
    w.bool(s.xexp_phase);
    w.bool(s.mc_phase);
    w.u8(s.current);
}

fn load_sprite(r: &mut StateReader<'_>) -> Result<Sprite, StateError> {
    let s = Sprite {
        x: r.u16()? & 0x1ff,
        y: r.u8()?,
        color: r.u8()? & 0x0f,
        ptr: r.u8()?,
        mc: r.u8()? & 63,
        mcbase: r.u8()? & 63,
        dma: r.bool()?,
        display: r.bool()?,
        exp: r.bool()?,
        data: r.u32()? & 0xff_ffff,
        shift: r.u32()? & 0xff_ffff,
        bits_left: r.u8()?,
        xexp_phase: r.bool()?,
        mc_phase: r.bool()?,
        current: r.u8()? & 3,
        was_mc: false,
        crunched: false,
    };
    if s.bits_left > 24 {
        return Err(StateError::Invalid("VIC-II sprite shifter"));
    }
    Ok(s)
}

impl Savestate for Vic2 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(match self.model {
                Model::Mos6569 => 0,
                Model::Mos6567R8 => 1,
                Model::Mos8565 => 2,
                Model::Mos8562 => 3,
            });
            w.u8(self.sprite_x_msb);
            w.u8(self.ctrl1);
            w.u16(self.raster_cmp);
            w.u8(self.lp_x);
            w.u8(self.lp_y);
            w.u8(self.sprite_enable);
            w.u8(self.ctrl2);
            w.u8(self.sprite_ye);
            w.u8(self.mem_ptrs);
            w.u8(self.irq_flags);
            w.u8(self.irq_mask);
            w.u8(self.sprite_prio);
            w.u8(self.sprite_mc);
            w.u8(self.sprite_xe);
            w.u8(self.coll_mm);
            w.u8(self.coll_md);
            w.u8(self.border_color);
            w.raw(&self.bg);
            w.raw(&self.sprite_mm);
            for s in &self.sprites {
                save_sprite(w, s);
            }
            w.u16(self.raster);
            w.u8(self.cycle);
            w.u64(self.frame);
            w.u16(self.vc);
            w.u16(self.vcbase);
            w.u8(self.rc);
            w.u8(self.vmli as u8);
            w.bool(self.display_state);
            w.bool(self.bad_lines_enabled);
            w.bool(self.bad_line);
            for m in &self.matrix {
                w.u16(*m);
            }
            for p in &self.pending {
                match p {
                    Some(p) => {
                        w.bool(true);
                        w.u8(p.g);
                        w.u16(p.c);
                        w.u8(p.age);
                    }
                    None => {
                        w.bool(false);
                        w.u8(0);
                        w.u16(0);
                        w.u8(0);
                    }
                }
            }
            w.u8(self.sr);
            w.u16(self.sr_c);
            w.bool(self.sr_mc_phase);
            w.u8(self.sr_pair);
            w.u8(self.refresh);
            w.bool(self.main_border);
            w.bool(self.vert_border);
            w.bool(self.ba);
            w.u8(self.ba_low_cycles);
            w.u8(self.last_bus);
            w.bool(self.raster_irq_done);
            w.u8(self.phi2_sprite.map_or(0xff, |n| n as u8));
            w.bytes(&self.pixels);
            w.bool(self.lp_done);
            w.bool(self.lp_low);
            for &(index, source) in &self.lag {
                w.u32(index as u32);
                w.u8(source);
            }
            for s in &self.sprites {
                w.bool(s.was_mc);
            }
            w.u8(self.coll_late.0);
            w.u8(self.coll_late.1);
            w.bool(self.lp_pending);
            w.bool(self.no_load);
            w.u16(self.cmp_held);
            w.u8(self.g_ctrl1);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let model = match if version < 2 { 0 } else { c.u8()? } {
            0 => Model::Mos6569,
            1 => Model::Mos6567R8,
            2 => Model::Mos8565,
            3 => Model::Mos8562,
            _ => return Err(StateError::Invalid("VIC-II type")),
        };
        if model != self.model {
            return Err(StateError::Invalid("VIC-II type differs"));
        }
        let mut v = Vic2::new(model);
        let t = model.timing();
        v.sprite_x_msb = c.u8()?;
        v.ctrl1 = c.u8()? & 0x7f;
        v.raster_cmp = c.u16()? & 0x1ff;
        v.lp_x = c.u8()?;
        v.lp_y = c.u8()?;
        v.sprite_enable = c.u8()?;
        v.ctrl2 = c.u8()? & 0x1f;
        v.sprite_ye = c.u8()?;
        v.mem_ptrs = c.u8()? & 0xfe;
        v.irq_flags = c.u8()? & 0x0f;
        v.irq_mask = c.u8()? & 0x0f;
        v.sprite_prio = c.u8()?;
        v.sprite_mc = c.u8()?;
        v.sprite_xe = c.u8()?;
        v.coll_mm = c.u8()?;
        v.coll_md = c.u8()?;
        v.border_color = c.u8()? & 0x0f;
        c.raw_into(&mut v.bg)?;
        c.raw_into(&mut v.sprite_mm)?;
        for s in v.sprites.iter_mut() {
            *s = load_sprite(&mut c)?;
        }
        v.raster = c.u16()?;
        v.cycle = c.u8()?;
        v.frame = c.u64()?;
        if v.raster >= t.lines || !(1..=t.cycles).contains(&v.cycle) {
            return Err(StateError::Invalid("VIC-II beam position"));
        }
        v.vc = c.u16()? & 0x3ff;
        v.vcbase = c.u16()? & 0x3ff;
        v.rc = c.u8()? & 7;
        v.vmli = usize::from(c.u8()?);
        if v.vmli >= 40 {
            return Err(StateError::Invalid("VIC-II VMLI"));
        }
        v.display_state = c.bool()?;
        v.bad_lines_enabled = c.bool()?;
        v.bad_line = c.bool()?;
        for m in v.matrix.iter_mut() {
            *m = c.u16()? & 0x0fff;
        }
        for p in v.pending.iter_mut() {
            let some = c.bool()?;
            let g = c.u8()?;
            let cdata = c.u16()? & 0x0fff;
            let age = c.u8()?;
            if age > 15 {
                return Err(StateError::Invalid("VIC-II sequencer"));
            }
            // Before version 7 the byte is the pixels until the load.
            let age = if version < 7 {
                (4 + (v.ctrl2 & 7)).saturating_sub(age)
            } else {
                age
            };
            *p = some.then_some(Pending { g, c: cdata, age });
        }
        v.sr = c.u8()?;
        v.sr_c = c.u16()? & 0x0fff;
        v.sr_mc_phase = c.bool()?;
        v.sr_pair = c.u8()? & 3;
        v.refresh = c.u8()?;
        v.main_border = c.bool()?;
        v.vert_border = c.bool()?;
        v.ba = c.bool()?;
        v.ba_low_cycles = c.u8()?;
        v.last_bus = c.u8()?;
        v.raster_irq_done = c.bool()?;
        v.phi2_sprite = match c.u8()? {
            0xff => None,
            n if n < 8 => Some(usize::from(n)),
            _ => return Err(StateError::Invalid("VIC-II sprite slot")),
        };
        let pixels = c.bytes()?;
        if pixels.len() != v.pixels.len() || pixels.iter().any(|&p| p > 15) {
            return Err(StateError::Invalid("VIC-II pixels"));
        }
        v.pixels.copy_from_slice(pixels);
        if version >= 3 {
            v.lp_done = c.bool()?;
            v.lp_low = c.bool()?;
        } else {
            v.lp_done = v.irq_flags & irq::LIGHT_PEN != 0;
        }
        if version >= 4 {
            for l in v.lag.iter_mut() {
                let index = c.u32()?;
                let source = c.u8()?;
                let index = if index == u32::MAX {
                    usize::MAX
                } else if (index as usize) < v.pixels.len() && source <= src::SPRITE0 + 7 {
                    index as usize
                } else {
                    return Err(StateError::Invalid("VIC-II pixel colour source"));
                };
                *l = (index, source);
            }
            for s in v.sprites.iter_mut() {
                s.was_mc = c.bool()?;
            }
            if version >= 5 {
                v.coll_late = (c.u8()?, c.u8()?);
            }
            if version >= 6 {
                v.lp_pending = c.bool()?;
            }
            if version >= 7 {
                v.no_load = c.bool()?;
                v.cmp_held = c.u16()? & 0x1ff;
                v.g_ctrl1 = c.u8()? & 0x7f;
            } else {
                v.no_load = v.vert_border;
                v.cmp_held = v.raster_cmp;
                v.g_ctrl1 = v.ctrl1;
            }
        }
        v.cycle_regs = [v.sprite_prio, v.sprite_xe, v.sprite_mc];
        v.cycle_sprite_x = v.sprites.map(|s| s.x);
        v.cycle_mode = (v.ctrl1, v.ctrl2);
        // The pen's aim is configuration from the port: keep it.
        v.pen = self.pen;
        v.pen_lit = self.pen_lit;
        // The overlay record is not state: keep what is switched on, start
        // a fresh record.
        if let Some(r) = &self.rec.0 {
            let ids: Vec<&str> = [
                (r.badlines(), overlay::BADLINES),
                (r.sprites(), overlay::SPRITES),
            ]
            .into_iter()
            .filter_map(|(on, id)| on.then_some(id))
            .collect();
            v.set_overlays(&ids);
        }
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
