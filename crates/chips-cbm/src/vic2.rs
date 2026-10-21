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
//! more cycles before the fetch of sprites 0–2, which therefore come two
//! cycles later (60/62/64 instead of 58/60/62). X coordinates are VIC-II
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
};

impl Model {
    pub fn timing(self) -> &'static Timing {
        match self {
            Model::Mos6569 => &PAL,
            Model::Mos6567R8 => &NTSC,
        }
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

    /// First cycle (p-access) of sprite `n`'s fetch slot.
    fn sprite_slot(&self, n: usize) -> u8 {
        if n < 3 {
            self.cycles - 5 + 2 * n as u8
        } else {
            1 + 2 * (n as u8 - 3)
        }
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
}

/// Graphics data waiting to be loaded into the sequencer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pending {
    g: u8,
    c: u16,
    /// Pixels until the load.
    delay: u8,
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

    // Bus.
    ba: bool,
    ba_low_cycles: u8,
    last_bus: u8,
    raster_irq_done: bool,
    /// Which sprite is fetched in the current cycle's phi2, if any.
    phi2_sprite: Option<usize>,

    pixels: Vec<u8>,
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
            ba: true,
            ba_low_cycles: 0,
            last_bus: 0xff,
            raster_irq_done: false,
            phi2_sprite: None,
            pixels: vec![0; (t.width * t.height) as usize],
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
            line: self.raster,
            cycle: u16::from(self.cycle - 1),
            hpos: self.timing().cycle_x(self.cycle),
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
        if self.bad_line {
            self.display_state = true;
        }
    }

    fn vm_base(&self) -> u16 {
        u16::from(self.mem_ptrs >> 4) << 10
    }

    fn char_base(&self) -> u16 {
        u16::from((self.mem_ptrs >> 1) & 7) << 11
    }

    fn check_raster_irq(&mut self) {
        if self.raster == self.raster_cmp {
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

        // The raster compare runs in cycle 1, for line 0 one cycle later.
        if c == 1 && self.raster != 0 {
            self.check_raster_irq();
        }
        if c == 2 && self.raster == 0 {
            self.check_raster_irq();
        }
        self.update_bad_line();

        // Sprite Y expansion and DMA start (cycles 55/56).
        if c == 55 {
            for (n, s) in self.sprites.iter_mut().enumerate() {
                if self.sprite_ye & (1 << n) != 0 {
                    s.exp = !s.exp;
                }
            }
        }
        if c == 55 || c == 56 {
            for n in 0..8 {
                let s = &mut self.sprites[n];
                if self.sprite_enable & (1 << n) != 0 && s.y == self.raster as u8 && !s.dma {
                    s.dma = true;
                    s.mcbase = 0;
                    if self.sprite_ye & (1 << n) != 0 {
                        s.exp = false;
                    }
                }
            }
        }
        if c == 58 {
            for s in self.sprites.iter_mut() {
                s.mc = s.mcbase;
                if s.dma && s.y == self.raster as u8 {
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
                if s.exp {
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
                }
            } else if c == slot % t.cycles + 1 && self.sprites[n].dma {
                phi1_value = self.sprite_fetch(mem, n);
                self.phi2_sprite = Some(n);
            }
        }
        if (11..=15).contains(&c) {
            phi1_value = mem.read(0x3f00 | u16::from(self.refresh));
            self.refresh = self.refresh.wrapping_sub(1);
        }
        if (16..=55).contains(&c) {
            phi1_value = self.g_access(mem);
        }
        self.last_bus = phi1_value;
    }

    fn sprite_fetch(&mut self, mem: &dyn VicMemory, n: usize) -> u8 {
        let s = &mut self.sprites[n];
        let v = mem.read(u16::from(s.ptr) << 6 | u16::from(s.mc));
        s.mc = (s.mc + 1) & 63;
        s.data = (s.data << 8 | u32::from(v)) & 0xff_ffff;
        v
    }

    fn g_access(&mut self, mem: &dyn VicMemory) -> u8 {
        let ecm = self.ctrl1 & 0x40 != 0;
        let bmm = self.ctrl1 & 0x20 != 0;
        let (addr, c) = if self.display_state {
            let c = self.matrix[self.vmli];
            let addr = if bmm {
                (u16::from(self.mem_ptrs & 0x08) << 10) | (self.vc << 3) | u16::from(self.rc)
            } else {
                self.char_base() | ((c & 0xff) << 3) | u16::from(self.rc)
            };
            (addr, c)
        } else {
            (0x3fff, 0)
        };
        let addr = if ecm { addr & 0x39ff } else { addr };
        let g = mem.read(addr);
        if self.display_state {
            self.vc = (self.vc + 1) & 0x3ff;
            self.vmli = (self.vmli + 1) % 40;
        }
        // Loaded into the shifter XSCROLL + 4 pixels into this cycle.
        let p = Pending {
            g,
            c,
            delay: 4 + (self.ctrl2 & 7),
        };
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
        }
        if self.bad_line && (15..=54).contains(&c) {
            // The first three cycles after BA the CPU may still own the bus;
            // the VIC-II then reads $FF.
            let v = if self.ba_low_cycles > 3 {
                let addr = self.vm_base() | self.vc;
                u16::from(mem.read(addr)) | u16::from(mem.color(self.vc)) << 8
            } else {
                0xff | u16::from(mem.color(self.vc)) << 8
            };
            self.matrix[self.vmli] = v;
        }

        self.draw();

        // Vertical border, cycle 63 rules (the last cycle of the line).
        if c == t.cycles {
            let (top, bottom) = self.border_rows();
            if self.raster == bottom {
                self.vert_border = true;
            } else if self.raster == top && self.den() {
                self.vert_border = false;
            }
        }

        if c == t.cycles {
            self.cycle = 1;
            self.raster += 1;
            if self.raster == t.lines {
                self.raster = 0;
                self.frame += 1;
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

        for p in p0..p0 + 8 {
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
            }

            let (gfx_color, gfx_fg) = self.graphics_pixel();
            let (sprite, colliding) = self.sprite_pixel(x);

            if colliding.count_ones() >= 2 {
                if self.coll_mm == 0 {
                    self.irq_flags |= irq::SPRITE_SPRITE;
                }
                self.coll_mm |= colliding;
            }
            if gfx_fg && colliding != 0 {
                if self.coll_md == 0 {
                    self.irq_flags |= irq::SPRITE_DATA;
                }
                self.coll_md |= colliding;
            }

            let mut color = gfx_color;
            if let Some((n, sc)) = sprite {
                if !(gfx_fg && self.sprite_prio & (1 << n) != 0) {
                    color = sc;
                }
            }
            if self.main_border {
                color = self.border_color;
            }

            if let Some(row) = row {
                let col = p.wrapping_sub(t.visible_p0);
                if u32::from(col) < t.width {
                    self.pixels[row + usize::from(col)] = color;
                }
            }
        }
    }

    /// Next pixel of the graphics sequencer: (colour, is foreground).
    fn graphics_pixel(&mut self) -> (u8, bool) {
        if let Some(p) = &mut self.pending[0] {
            if p.delay == 0 {
                self.sr = p.g;
                self.sr_c = p.c;
                self.sr_mc_phase = false;
                self.pending[0] = self.pending[1].take();
            }
        }
        for p in self.pending.iter_mut().flatten() {
            p.delay = p.delay.saturating_sub(1);
        }

        let ecm = self.ctrl1 & 0x40 != 0;
        let bmm = self.ctrl1 & 0x20 != 0;
        let mcm = self.ctrl2 & 0x10 != 0;
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
            (b, b != 0)
        };

        let out = match (ecm, bmm, mcm) {
            (false, false, false) => {
                if bits != 0 {
                    color
                } else {
                    self.bg[0]
                }
            }
            (false, false, true) => {
                if !multicolor {
                    if bits != 0 {
                        color & 7
                    } else {
                        self.bg[0]
                    }
                } else {
                    match bits {
                        0 => self.bg[0],
                        1 => self.bg[1],
                        2 => self.bg[2],
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
                0 => self.bg[0],
                1 => cc >> 4,
                2 => cc & 0x0f,
                _ => color,
            },
            (true, false, false) => {
                if bits != 0 {
                    color
                } else {
                    self.bg[usize::from(cc >> 6)]
                }
            }
            // Invalid modes output black; collisions still work.
            _ => 0,
        };
        (out, fg)
    }

    /// Sprite output at X: (winning sprite and colour, mask of opaque sprites).
    fn sprite_pixel(&mut self, x: u16) -> (Option<(usize, u8)>, u8) {
        let mut winner = None;
        let mut opaque = 0u8;
        for n in 0..8 {
            let s = &mut self.sprites[n];
            if !s.display {
                continue;
            }
            if s.bits_left == 0 && s.x == x {
                s.shift = s.data;
                s.bits_left = 24;
                s.xexp_phase = false;
                s.mc_phase = false;
            }
            if s.bits_left == 0 {
                continue;
            }
            let mc = self.sprite_mc & (1 << n) != 0;
            let xe = self.sprite_xe & (1 << n) != 0;
            // New data bits only every second pixel when X-expanded.
            if !s.xexp_phase {
                if mc {
                    if !s.mc_phase {
                        s.current = (s.shift >> 22) as u8 & 3;
                    }
                } else {
                    s.current = ((s.shift >> 23) & 1) as u8 * 2;
                }
            }
            let value = s.current;
            let advance = !xe || s.xexp_phase;
            if xe {
                s.xexp_phase = !s.xexp_phase;
            }
            if advance {
                if mc {
                    if s.mc_phase {
                        s.shift <<= 2;
                        s.bits_left -= 2;
                    }
                    s.mc_phase = !s.mc_phase;
                } else {
                    s.shift <<= 1;
                    s.bits_left -= 1;
                }
                s.shift &= 0xff_ffff;
            }
            if value == 0 {
                continue;
            }
            opaque |= 1 << n;
            if winner.is_none() {
                let color = match value {
                    1 => self.sprite_mm[0],
                    3 => self.sprite_mm[1],
                    _ => s.color,
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
            0x11 => (self.ctrl1 & 0x7f) | ((self.raster >> 1) as u8 & 0x80),
            0x12 => self.raster as u8,
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
        match reg & 0x3f {
            0x1e => self.coll_mm = 0,
            0x1f => self.coll_md = 0,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
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
                self.sprite_ye = v;
                for (n, s) in self.sprites.iter_mut().enumerate() {
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
            0x20 => self.border_color = v & 0x0f,
            0x21..=0x24 => self.bg[usize::from(reg - 0x21)] = v & 0x0f,
            0x25 | 0x26 => self.sprite_mm[usize::from(reg - 0x25)] = v & 0x0f,
            0x27..=0x2e => self.sprites[usize::from(reg - 0x27)].color = v & 0x0f,
            // Light pen and collision registers are read-only.
            _ => {}
        }
    }

    /// Latch the beam position into the light-pen registers (LP input
    /// falling edge; only the first per frame counts, Bauer 3.11).
    pub fn light_pen(&mut self) {
        if self.irq_flags & irq::LIGHT_PEN != 0 {
            return;
        }
        let x = self.timing().cycle_x(self.cycle);
        self.lp_x = (x >> 1) as u8;
        self.lp_y = self.raster as u8;
        self.irq_flags |= irq::LIGHT_PEN;
    }
}

const TAG: [u8; 4] = *b"6569";
/// 2: VIC-II type byte first (version 1 states are 6569).
const VERSION: u16 = 2;

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
                        w.u8(p.delay);
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
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let model = match if version < 2 { 0 } else { c.u8()? } {
            0 => Model::Mos6569,
            1 => Model::Mos6567R8,
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
            let delay = c.u8()?;
            if delay > 11 {
                return Err(StateError::Invalid("VIC-II sequencer"));
            }
            *p = some.then_some(Pending { g, c: cdata, delay });
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
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
