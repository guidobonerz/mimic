//! The VIC-IV's registers: the KEY register that selects the I/O
//! personality, the VIC-II raster counter with its compare interrupt, the
//! VIC-III `$D030`/`$D031` bits, the VIC-IV display registers with the
//! "hot registers" that derive them, the four palette banks. The picture is
//! drawn from them by [`crate::render`].
//!
//! **KEY (`$D02F`).** Two writes in a row select the personality: `$A5`
//! `$96` the C65's (VIC-III), `$47` `$53` the MEGA65's (VIC-IV), `$45`
//! `$54` the MEGA65 Ethernet one; any other write the C64's (VIC-II).
//!
//! **C64 personality.** `$D030` is the C128's 2 MHz bit (it does not touch
//! the VIC-III's `$D030`); `$D040`–`$D07F` mirror `$D000`–`$D03F`; the
//! VIC-III/IV registers and the palette are hidden (their values stay); the
//! colour registers `$D020`–`$D02E` keep 4 bits, as on the VIC-II.
//!
//! **Coordinates.** The VIC-IV's position registers (top and bottom border
//! `$D048`/`$D04A`, the text start `$D04C`/`$D04E`, the side border width
//! `$D05C`) count here in the pixels and lines of the 720 × 576 visible
//! PAL frame. The VIC-II/III display area is the middle 640 × 400: side
//! borders of 40 pixels, a top border of 88 lines. (The guide gives the
//! frame and these sizes, not the hardware's register values; those are
//! to be read on a real MEGA65.)
//!
//! **Hot registers.** With HOTREG (`$D05D` bit 7, set at reset) a write
//! to `$D011`, `$D016`, `$D018`, `$D031` or the VIC bank bits of CIA 2
//! port A recomputes, as the guide lists: the borders (24 rows: the top
//! border 8 lines lower, the bottom one 8 higher; 38 columns: the left
//! border 14 pixels wider, the right one 17), the text position (left
//! edge plus twice XSCROLL; the top border less 6 plus twice YSCROLL, so
//! YSCROLL 3 puts row 0 at the top edge as on the C64), the row step and
//! characters a row (40, or 80 with H640), the rows (25 or 50 with V400),
//! the screen pointer (bank + VS × 1 KB), the low 16 bits of the character
//! pointer (bank + CB × 2 KB), the sprite pointer address (the end of the
//! 1 KB or, with H640, 2 KB screen) and colour pointer 0. Written while
//! HOTREG is clear, the update waits until HOTREG is set again; writing 0
//! to HOTREG cancels it.
//!
//! **Palette.** Four banks of 256 entries for red, green and blue (`$D100`,
//! `$D200`, `$D300`), the nybbles of each value swapped. `$D070` bits 7–6
//! the bank the CPU sees, 5–4 the bank text and bitmaps use, 1–0 the
//! alternate one (bold and reverse together). With `$D030` bit 2 clear
//! colours 0–15 come from the palette ROM (the system palette).
//!
//! Sources: the MEGA65 user guide (`modes.tex`: the KEY register;
//! `appendix-memorymap.tex`: the personalities; `appendix-viciv-
//! registers.tex`: frame timing, hot registers, the memory pointers;
//! `appendix-system-palette.tex`: colours 0–15), the register descriptions
//! of the MEGA65 core (`@IO` lines of `viciv.vhdl`); the C65 system
//! specification (VIC-III registers).

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// The I/O personality of `$D000`–`$DFFF`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Personality {
    #[default]
    C64,
    C65,
    Mega65,
    Ethernet,
}

impl Personality {
    pub fn index(self) -> u8 {
        match self {
            Personality::C64 => 0,
            Personality::C65 => 1,
            Personality::Ethernet => 2,
            Personality::Mega65 => 3,
        }
    }

    pub fn from_index(i: u8) -> Option<Self> {
        Some(match i {
            0 => Personality::C64,
            1 => Personality::C65,
            2 => Personality::Ethernet,
            3 => Personality::Mega65,
            _ => return None,
        })
    }
}

/// Raster lines of a PAL field (the VIC-II's count).
pub const LINES: u16 = 312;
/// The visible frame.
pub const WIDTH: usize = 720;
pub const HEIGHT: usize = 576;
/// The VIC-II/III display area's left and top edge in the visible frame.
pub const LEFT: u16 = 40;
pub const TOP: u16 = 88;
/// The physical line of the frame shown as the first visible line: VIC-II
/// raster 51 (the C64's first display line) is the top edge.
pub const FIRST_PHYSICAL: u16 = 2 * 51 - TOP;

/// The palette ROM: colours 0–15 (4 bits each, the guide's system palette).
const ROM_PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0],
    [15, 15, 15],
    [15, 0, 0],
    [0, 15, 15],
    [15, 0, 15],
    [0, 15, 0],
    [0, 0, 15],
    [15, 15, 0],
    [15, 6, 0],
    [10, 4, 0],
    [15, 7, 7],
    [5, 5, 5],
    [8, 8, 8],
    [9, 15, 9],
    [9, 9, 15],
    [11, 11, 11],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vic {
    /// `$D000`–`$D07F` as written (and as the hot registers set them).
    pub regs: [u8; 0x80],
    /// Palette banks: red, green, blue registers.
    pub palette: [[[u8; 256]; 3]; 4],
    pub personality: Personality,
    /// The last KEY write.
    key_prev: u8,
    /// C64 personality: the C128 2 MHz bit at `$D030`.
    pub c128_fast: bool,
    /// The raster line now and the compare line.
    pub raster: u16,
    pub raster_cmp: u16,
    /// `$D019` bits 0–3.
    pub irq_flags: u8,
    /// The side borders' inner edges (internal registers).
    pub border_left: u16,
    pub border_right: u16,
    /// A hot register was written while HOTREG was clear.
    hot_pending: bool,
    /// CIA 2 port A as the VIC bank bits last saw it.
    pub cia2_pa: u8,
}

impl Default for Vic {
    fn default() -> Self {
        Self::new()
    }
}

impl Vic {
    pub fn new() -> Self {
        let mut regs = [0; 0x80];
        // The hypervisor leaves the machine at full speed (FAST, VFAST).
        regs[0x31] = 0x40;
        regs[0x54] = 0x40;
        regs[0x11] = 0x1b;
        regs[0x16] = 0xc8;
        regs[0x18] = 0x15;
        // Side border width; HOTREG on.
        regs[0x5c] = LEFT as u8;
        regs[0x5d] = 0x80;
        let mut v = Self {
            regs,
            palette: [[[0; 256]; 3]; 4],
            personality: Personality::C64,
            key_prev: 0,
            c128_fast: false,
            raster: 0,
            raster_cmp: 0,
            irq_flags: 0,
            border_left: LEFT,
            border_right: LEFT,
            hot_pending: false,
            cia2_pa: 0x03,
        };
        v.hot();
        v
    }

    /// The VIC asks for an interrupt.
    pub fn irq(&self) -> bool {
        self.irq_flags & self.regs[0x1a] & 0x0f != 0
    }

    /// A new raster line began.
    pub fn line(&mut self, raster: u16) {
        self.raster = raster;
        if raster == self.raster_cmp {
            self.irq_flags |= 1;
        }
    }

    /// A 12-bit register pair (LSB, low nybble of the MSB).
    pub fn reg16(&self, lo: usize) -> u16 {
        u16::from(self.regs[lo]) | u16::from(self.regs[lo + 1] & 0x0f) << 8
    }

    fn set16(&mut self, lo: usize, v: u16) {
        self.regs[lo] = v as u8;
        self.regs[lo + 1] = (self.regs[lo + 1] & 0xf0) | (v >> 8) as u8 & 0x0f;
    }

    /// A 24-bit address register (three bytes, LSB first).
    pub fn addr24(&self, lo: usize) -> u32 {
        u32::from(self.regs[lo])
            | u32::from(self.regs[lo + 1]) << 8
            | u32::from(self.regs[lo + 2]) << 16
    }

    fn set24(&mut self, lo: usize, v: u32) {
        self.regs[lo] = v as u8;
        self.regs[lo + 1] = (v >> 8) as u8;
        self.regs[lo + 2] = (v >> 16) as u8;
    }

    /// The hot register update (see the module documentation).
    pub fn hot(&mut self) {
        if self.regs[0x5d] & 0x80 == 0 {
            self.hot_pending = true;
            return;
        }
        self.hot_pending = false;
        let r = self.regs;
        let (h640, v400) = (r[0x31] & 0x80 != 0, r[0x31] & 0x08 != 0);
        let rsel = r[0x11] & 0x08 != 0;
        let csel = r[0x16] & 0x08 != 0;
        let (xscl, yscl) = (u16::from(r[0x16] & 7), u16::from(r[0x11] & 7));
        let rows_off = if rsel { 0 } else { 8 };
        self.set16(0x48, TOP + rows_off);
        self.set16(0x4a, TOP + 400 - rows_off);
        let side = self.reg16(0x5c);
        self.border_left = side + if csel { 0 } else { 14 };
        self.border_right = side + if csel { 0 } else { 17 };
        self.set16(0x4c, LEFT + 2 * xscl);
        self.set16(0x4e, TOP - 6 + 2 * yscl);
        self.regs[0x5b] = u8::from(!v400);
        let width: u16 = if h640 { 80 } else { 40 };
        self.regs[0x58] = width as u8;
        self.regs[0x59] = 0;
        self.regs[0x5e] = width as u8;
        self.regs[0x63] &= !0x30;
        self.regs[0x7b] = if v400 { 49 } else { 24 };
        let bank = u32::from(3 - (self.cia2_pa & 3)) * 0x4000;
        let screen = bank + u32::from(r[0x18] >> 4) * 0x400;
        self.set24(0x60, screen);
        self.regs[0x63] &= 0xf0;
        let chars = bank + u32::from(r[0x18] >> 1 & 7) * 0x800;
        self.regs[0x68] = chars as u8;
        self.regs[0x69] = (chars >> 8) as u8;
        self.regs[0x64] = 0;
        self.regs[0x65] = 0;
        let sprites = screen + if h640 { 0x7f8 } else { 0x3f8 };
        self.regs[0x6c] = sprites as u8;
        self.regs[0x6d] = (sprites >> 8) as u8;
        self.regs[0x6e] = (self.regs[0x6e] & 0x80) | (sprites >> 16) as u8 & 0x7f;
    }

    /// CIA 2 port A changed (the VIC bank).
    pub fn set_cia2_pa(&mut self, pa: u8) {
        let changed = pa & 3 != self.cia2_pa & 3;
        self.cia2_pa = pa;
        if changed {
            self.hot();
        }
    }

    /// The register `reg` (0–`$7F`) as the personality decodes it: `None`
    /// where nothing answers.
    fn decode(p: Personality, reg: u16) -> Option<usize> {
        let r = usize::from(reg);
        if p == Personality::C64 && r >= 0x2f {
            let m = r & 0x3f;
            return (m < 0x2f).then_some(m);
        }
        Some(r)
    }

    /// Register `reg` (0–`$7F`) read in personality `p`.
    pub fn peek(&self, p: Personality, reg: u16) -> u8 {
        if p == Personality::C64 && reg == 0x30 {
            return 0xfe | u8::from(self.c128_fast);
        }
        let Some(r) = Self::decode(p, reg) else {
            return 0xff;
        };
        match r {
            0x11 => (self.regs[0x11] & 0x7f) | ((self.raster >> 1) as u8 & 0x80),
            0x12 => self.raster as u8,
            0x19 => self.irq_flags | 0x70 | if self.irq() { 0x80 } else { 0 },
            0x1a => self.regs[0x1a] | 0xf0,
            0x2f => 0xff,
            // The physical raster (two a VIC-II line).
            0x52 => (2 * self.raster) as u8,
            0x53 => (self.regs[0x53] & 0xf0) | ((2 * self.raster) >> 8) as u8 & 0x0f,
            _ => self.regs[r],
        }
    }

    pub fn read(&mut self, p: Personality, reg: u16) -> u8 {
        let v = self.peek(p, reg);
        // The collision registers clear when read.
        if let Some(r @ (0x1e | 0x1f)) = Self::decode(p, reg) {
            self.regs[r] = 0;
        }
        v
    }

    /// Collisions a line found: sprite–sprite and sprite–data bits. The
    /// interrupt flag comes with the first bit since the last read.
    pub fn collide(&mut self, ss: u8, sd: u8) {
        if ss != 0 {
            if self.regs[0x1e] == 0 {
                self.irq_flags |= 4;
            }
            self.regs[0x1e] |= ss;
        }
        if sd != 0 {
            if self.regs[0x1f] == 0 {
                self.irq_flags |= 2;
            }
            self.regs[0x1f] |= sd;
        }
    }

    /// Register `reg` written in personality `p`.
    pub fn write(&mut self, p: Personality, reg: u16, v: u8) {
        if reg == 0x2f {
            self.personality = match (self.key_prev, v) {
                (0xa5, 0x96) => Personality::C65,
                (0x47, 0x53) => Personality::Mega65,
                (0x45, 0x54) => Personality::Ethernet,
                _ => Personality::C64,
            };
            self.key_prev = v;
            return;
        }
        if p == Personality::C64 && reg == 0x30 {
            self.c128_fast = v & 1 != 0;
            return;
        }
        let Some(r) = Self::decode(p, reg) else {
            return;
        };
        match r {
            0x11 => {
                self.regs[0x11] = v;
                self.raster_cmp = (self.raster_cmp & 0xff) | u16::from(v & 0x80) << 1;
                self.hot();
            }
            0x12 => self.raster_cmp = (self.raster_cmp & 0x100) | u16::from(v),
            0x19 => self.irq_flags &= !(v & 0x0f),
            // The collision registers are read only.
            0x1e | 0x1f => {}
            0x16 | 0x18 | 0x31 => {
                self.regs[r] = v;
                self.hot();
            }
            0x20..=0x2e if p == Personality::C64 => self.regs[r] = v & 0x0f,
            0x5d => {
                let was = self.regs[0x5d] & 0x80 != 0;
                self.regs[0x5d] = v;
                if v & 0x80 == 0 {
                    // Writing 0 to HOTREG cancels a pending update.
                    if !was {
                        self.hot_pending = false;
                    }
                } else if !was && self.hot_pending {
                    self.hot();
                }
            }
            _ => self.regs[r] = v,
        }
    }

    /// The palette bank the CPU sees at `$D100`–`$D3FF`.
    fn mapped_bank(&self) -> usize {
        usize::from(self.regs[0x70] >> 6)
    }

    pub fn palette_write(&mut self, offset: u16, v: u8) {
        let b = self.mapped_bank();
        self.palette[b][usize::from(offset >> 8)][usize::from(offset & 0xff)] = v;
    }

    pub fn palette_peek(&self, offset: u16) -> u8 {
        self.palette[self.mapped_bank()][usize::from(offset >> 8)][usize::from(offset & 0xff)]
    }

    /// The 256 colours of palette `bank` as shown now.
    pub fn colours(&self, bank: usize) -> [Rgb8; 256] {
        let rom = self.regs[0x30] & 0x04 == 0;
        let p = &self.palette[bank & 3];
        std::array::from_fn(|i| {
            if rom && i < 16 {
                let [r, g, b] = ROM_PALETTE[i];
                return Rgb8 {
                    r: r * 17,
                    g: g * 17,
                    b: b * 17,
                };
            }
            let swap = |v: u8| v.rotate_left(4);
            Rgb8 {
                r: swap(p[0][i]),
                g: swap(p[1][i]),
                b: swap(p[2][i]),
            }
        })
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.regs);
        for bank in &self.palette {
            for p in bank {
                w.raw(p);
            }
        }
        w.u8(self.personality.index());
        w.u8(self.key_prev);
        w.bool(self.c128_fast);
        w.u16(self.raster);
        w.u16(self.raster_cmp);
        w.u8(self.irq_flags);
        w.u16(self.border_left);
        w.u16(self.border_right);
        w.bool(self.hot_pending);
        w.u8(self.cia2_pa);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut v = Vic::new();
        r.raw_into(&mut v.regs)?;
        for bank in &mut v.palette {
            for p in bank {
                r.raw_into(p)?;
            }
        }
        v.personality =
            Personality::from_index(r.u8()?).ok_or(StateError::Invalid("VIC-IV personality"))?;
        v.key_prev = r.u8()?;
        v.c128_fast = r.bool()?;
        v.raster = r.u16()? % LINES;
        v.raster_cmp = r.u16()? & 0x1ff;
        v.irq_flags = r.u8()? & 0x0f;
        v.border_left = r.u16()?.min(WIDTH as u16);
        v.border_right = r.u16()?.min(WIDTH as u16);
        v.hot_pending = r.bool()?;
        v.cia2_pa = r.u8()?;
        *self = v;
        Ok(())
    }
}
