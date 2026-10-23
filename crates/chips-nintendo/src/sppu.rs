//! The SNES's S-PPU (the 5C77 "PPU1" and 5C78 "PPU2" pair): 64 KB VRAM,
//! 256 colours of CGRAM, 544 bytes of OAM; BG modes 0–7, sprites,
//! windows, colour math, mosaic, hires (modes 5/6, pseudo-hires) and
//! interlace.
//!
//! **Rendering** happens a line at a time ([`SPpu::render_line`]) with
//! the registers as they are when the machine calls it (after that line's
//! HDMA); mid-line register changes are not seen.
//!
//! **Picture**: [`WIDTH`] × [`LINES`] RGB, in interlace × [`HEIGHT`] with
//! the two fields woven: 512 columns (a normal pixel is two, hires shows
//! the sub screen on even and the main screen on odd columns), a row per
//! line (so the CRT stage sees the real line count). Lines past 224 in
//! 224-line mode are black.
//!
//! **Registers** ($21xx, given here by the low byte): $00 INIDISP, $01
//! OBSEL, $02/$03 OAMADD, $04 OAMDATA, $05 BGMODE, $06 MOSAIC, $07–$0A
//! BGnSC, $0B/$0C BGnNBA, $0D–$14 scroll (write twice; $0D/$0E also
//! M7HOFS/M7VOFS), $15 VMAIN, $16/$17 VMADD, $18/$19 VMDATA, $1A M7SEL,
//! $1B–$20 M7A–M7Y (write twice), $21 CGADD, $22 CGDATA, $23–$25 window
//! selects, $26–$29 window positions, $2A/$2B window logic, $2C/$2D TM/TS,
//! $2E/$2F TMW/TSW, $30 CGWSEL, $31 CGADSUB, $32 COLDATA, $33 SETINI;
//! reads $34–$36 MPY, $37 SLHV, $38 RDOAM, $39/$3A RDVRAM, $3B RDCGRAM,
//! $3C/$3D OPHCT/OPVCT, $3E STAT77, $3F STAT78.
//!
//! Not emulated: mid-line register changes, VRAM/CGRAM/OAM access
//! restrictions outside blanking (writes always land), the Mode 7
//! multiplier's busy period (MPY always gives M7A × M7B), EXTBG outside
//! Mode 7.
//!
//! Sources: Martin Korth, fullsnes, "SNES Picture Processing Unit (PPU)"
//! and its subchapters, "SNES Memory OAM/VRAM/CGRAM Access", "SNES PPU
//! Timers and Status", "SNES Timing H/V Events" (read 2026-10-23); anomie's
//! register document for offset-per-tile. The implementation is this
//! project's own.

use emu_core::frame::Rgb8;
use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

pub const WIDTH: usize = 512;
pub const LINES: usize = 239;
pub const HEIGHT: usize = 2 * LINES;

/// Sprite sizes (small, large) for OBSEL bits 5–7.
const OBJ_SIZES: [((u16, u16), (u16, u16)); 8] = [
    ((8, 8), (16, 16)),
    ((8, 8), (32, 32)),
    ((8, 8), (64, 64)),
    ((16, 16), (32, 32)),
    ((16, 16), (64, 64)),
    ((32, 32), (64, 64)),
    ((16, 32), (32, 64)),
    ((16, 32), (32, 32)),
];

/// Layers: BG1–BG4, OBJ, backdrop (colour-math enable bit order).
const BG1: usize = 0;
const OBJ: usize = 4;
const BACK: usize = 5;

/// The layers of a line: BG1–BG4 (256 or, in hires, 512 pixels), the
/// sprites (256), and their order front to back.
struct Layers<'a> {
    bgs: &'a [Vec<Px>],
    obj: &'a [Px; 256],
    order: &'a [(usize, u8)],
}

/// A pixel of a layer: 15-bit colour, priority, and (OBJ) whether colour
/// math may apply (palettes 4–7).
#[derive(Clone, Copy, Default)]
struct Px {
    colour: u16,
    prio: u8,
    opaque: bool,
    math: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SPpu {
    pub vram: Vec<u16>,
    pub cgram: [u16; 256],
    pub oam: Vec<u8>,
    pub inidisp: u8,
    obsel: u8,
    oam_reload: u16,
    oam_addr: u16,
    oam_rotate: bool,
    oam_latch: u8,
    pub bgmode: u8,
    mosaic: u8,
    bgsc: [u8; 4],
    nba: [u8; 4],
    hofs: [u16; 4],
    vofs: [u16; 4],
    m7hofs: u16,
    m7vofs: u16,
    bg_old: u8,
    m7_old: u8,
    vmain: u8,
    pub vmaddr: u16,
    vram_buf: u16,
    m7sel: u8,
    m7: [u16; 4],
    m7x: u16,
    m7y: u16,
    cgadd: u8,
    cg_high: bool,
    cg_latch: u8,
    wsel: [u8; 3],
    wpos: [u8; 4],
    wlog: [u8; 2],
    tm: u8,
    ts: u8,
    tmw: u8,
    tsw: u8,
    cgwsel: u8,
    cgadsub: u8,
    fixed: u16,
    pub setini: u8,
    /// STAT77 bits 6/7 (range, time over).
    obj_flags: u8,
    hlatch: u16,
    vlatch: u16,
    h_high: bool,
    v_high: bool,
    latched: bool,
    pub field: bool,
    pub pal: bool,
    ppu1_bus: u8,
    ppu2_bus: u8,
    frame: Vec<Rgb8>,
}

fn rgb(c: u16, bright: u8) -> Rgb8 {
    let f = |v: u16| -> u8 {
        let v = (u32::from(v & 31) * (u32::from(bright) + 1)) / 16;
        ((v << 3) | (v >> 2)) as u8
    };
    if bright == 0 {
        return Rgb8::default();
    }
    Rgb8 {
        r: f(c),
        g: f(c >> 5),
        b: f(c >> 10),
    }
}

impl SPpu {
    pub fn new(pal: bool) -> Self {
        Self {
            vram: vec![0; 0x8000],
            cgram: [0; 256],
            oam: vec![0; 544],
            inidisp: 0x80,
            obsel: 0,
            oam_reload: 0,
            oam_addr: 0,
            oam_rotate: false,
            oam_latch: 0,
            bgmode: 0,
            mosaic: 0,
            bgsc: [0; 4],
            nba: [0; 4],
            hofs: [0; 4],
            vofs: [0; 4],
            m7hofs: 0,
            m7vofs: 0,
            bg_old: 0,
            m7_old: 0,
            vmain: 0x0f,
            vmaddr: 0,
            vram_buf: 0,
            m7sel: 0,
            m7: [0xffff; 4],
            m7x: 0,
            m7y: 0,
            cgadd: 0,
            cg_high: false,
            cg_latch: 0,
            wsel: [0; 3],
            wpos: [0; 4],
            wlog: [0; 2],
            tm: 0,
            ts: 0,
            tmw: 0,
            tsw: 0,
            cgwsel: 0,
            cgadsub: 0,
            fixed: 0,
            setini: 0,
            obj_flags: 0,
            hlatch: 0x1ff,
            vlatch: 0x1ff,
            h_high: false,
            v_high: false,
            latched: false,
            field: false,
            pal,
            ppu1_bus: 0,
            ppu2_bus: 0,
            frame: vec![Rgb8::default(); WIDTH * HEIGHT],
        }
    }

    /// The reset line: forced blank, the registers that reset (fullsnes
    /// "I/O map": INIDISP $8x, others keep their values).
    pub fn reset(&mut self) {
        self.inidisp = 0x80;
        self.setini = 0;
    }

    /// The picture: [`SPpu::height`] rows of [`WIDTH`].
    pub fn frame(&self) -> &[Rgb8] {
        &self.frame[..WIDTH * self.height()]
    }

    /// Rows of the picture: 239, or 478 in interlace.
    pub fn height(&self) -> usize {
        if self.interlace() {
            HEIGHT
        } else {
            LINES
        }
    }

    /// The picture row of line `y` (1–239) in the current field.
    pub fn row(&self, y: u16) -> usize {
        let line = usize::from(y.max(1)) - 1;
        if self.interlace() {
            line * 2 + usize::from(self.field)
        } else {
            line
        }
    }

    pub fn forced_blank(&self) -> bool {
        self.inidisp & 0x80 != 0
    }

    /// Picture lines: 224 or 239 (SETINI bit 2).
    pub fn visible_lines(&self) -> u16 {
        if self.setini & 4 != 0 {
            239
        } else {
            224
        }
    }

    pub fn interlace(&self) -> bool {
        self.setini & 1 != 0
    }

    // ---- start of vertical blank / frame

    /// Vertical blank begins: OAMADD reloads (unless in forced blank).
    pub fn start_vblank(&mut self) {
        if !self.forced_blank() {
            self.oam_addr = self.oam_reload << 1;
        }
    }

    /// Line 0: a new field (the interlace field toggles), the sprite
    /// overflow flags clear (unless in forced blank).
    pub fn start_frame(&mut self) {
        self.field = !self.field;
        if !self.forced_blank() {
            self.obj_flags = 0;
        }
    }

    /// Latch the H/V counters (SLHV read, WRIO bit 7 falling, light gun).
    pub fn latch(&mut self, h: u16, v: u16) {
        self.hlatch = h;
        self.vlatch = v;
        self.latched = true;
    }

    // ---- VRAM address translation

    fn vram_word(&self) -> usize {
        let a = self.vmaddr;
        let t = match (self.vmain >> 2) & 3 {
            0 => a,
            1 => (a & 0xff00) | ((a & 0x001f) << 3) | ((a >> 5) & 7),
            2 => (a & 0xfe00) | ((a & 0x003f) << 3) | ((a >> 6) & 7),
            _ => (a & 0xfc00) | ((a & 0x007f) << 3) | ((a >> 7) & 7),
        };
        usize::from(t & 0x7fff)
    }

    fn vram_step(&mut self) {
        let step = match self.vmain & 3 {
            0 => 1,
            1 => 32,
            _ => 128,
        };
        self.vmaddr = self.vmaddr.wrapping_add(step);
    }

    fn write_twice_scroll(&mut self, n: usize, vertical: bool, v: u8) {
        if vertical {
            self.vofs[n] = (u16::from(v) << 8 | u16::from(self.bg_old)) & 0x3ff;
        } else {
            // The fine bits come from bits 8–10 of the register as it was,
            // which the first of the two writes has just set; so it keeps
            // 11 bits (only 10 are used).
            self.hofs[n] =
                (u16::from(v) << 8 | u16::from(self.bg_old & !7) | ((self.hofs[n] >> 8) & 7))
                    & 0x7ff;
        }
        self.bg_old = v;
    }

    fn m7_write(&mut self, v: u8) -> u16 {
        let r = u16::from(v) << 8 | u16::from(self.m7_old);
        self.m7_old = v;
        r
    }

    /// A write to $2100 + `reg`.
    pub fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x00 => self.inidisp = v,
            0x01 => self.obsel = v,
            0x02 => {
                self.oam_reload = (self.oam_reload & 0x100) | u16::from(v);
                self.oam_addr = self.oam_reload << 1;
            }
            0x03 => {
                self.oam_reload = (self.oam_reload & 0xff) | (u16::from(v & 1) << 8);
                self.oam_rotate = v & 0x80 != 0;
                self.oam_addr = self.oam_reload << 1;
            }
            0x04 => {
                let a = usize::from(self.oam_addr & 0x3ff);
                if a >= 0x200 {
                    self.oam[0x200 | (a & 0x1f)] = v;
                } else if a & 1 == 0 {
                    self.oam_latch = v;
                } else {
                    self.oam[a - 1] = self.oam_latch;
                    self.oam[a] = v;
                }
                self.oam_addr = (self.oam_addr + 1) & 0x3ff;
            }
            0x05 => self.bgmode = v,
            0x06 => self.mosaic = v,
            0x07..=0x0a => self.bgsc[usize::from(reg - 7)] = v,
            0x0b => self.nba[..2].copy_from_slice(&[v & 0x0f, v >> 4]),
            0x0c => self.nba[2..].copy_from_slice(&[v & 0x0f, v >> 4]),
            0x0d..=0x14 => {
                let n = usize::from((reg - 0x0d) / 2);
                let vertical = (reg - 0x0d) & 1 != 0;
                if n == 0 {
                    let r = self.m7_write(v) & 0x1fff;
                    if vertical {
                        self.m7vofs = r;
                    } else {
                        self.m7hofs = r;
                    }
                }
                self.write_twice_scroll(n, vertical, v);
            }
            0x15 => self.vmain = v,
            0x16 => {
                self.vmaddr = (self.vmaddr & 0xff00) | u16::from(v);
                self.vram_buf = self.vram[self.vram_word()];
            }
            0x17 => {
                self.vmaddr = (self.vmaddr & 0x00ff) | (u16::from(v) << 8);
                self.vram_buf = self.vram[self.vram_word()];
            }
            0x18 | 0x19 => {
                let w = self.vram_word();
                let high = reg == 0x19;
                self.vram[w] = if high {
                    (self.vram[w] & 0x00ff) | (u16::from(v) << 8)
                } else {
                    (self.vram[w] & 0xff00) | u16::from(v)
                };
                if high == (self.vmain & 0x80 != 0) {
                    self.vram_step();
                }
            }
            0x1a => self.m7sel = v,
            0x1b..=0x1e => {
                let r = self.m7_write(v);
                self.m7[usize::from(reg - 0x1b)] = r;
            }
            0x1f => self.m7x = self.m7_write(v) & 0x1fff,
            0x20 => self.m7y = self.m7_write(v) & 0x1fff,
            0x21 => {
                self.cgadd = v;
                self.cg_high = false;
            }
            0x22 => {
                if self.cg_high {
                    self.cgram[usize::from(self.cgadd)] =
                        (u16::from(v & 0x7f) << 8) | u16::from(self.cg_latch);
                    self.cgadd = self.cgadd.wrapping_add(1);
                } else {
                    self.cg_latch = v;
                }
                self.cg_high = !self.cg_high;
            }
            0x23..=0x25 => self.wsel[usize::from(reg - 0x23)] = v,
            0x26..=0x29 => self.wpos[usize::from(reg - 0x26)] = v,
            0x2a | 0x2b => self.wlog[usize::from(reg - 0x2a)] = v,
            0x2c => self.tm = v,
            0x2d => self.ts = v,
            0x2e => self.tmw = v,
            0x2f => self.tsw = v,
            0x30 => self.cgwsel = v,
            0x31 => self.cgadsub = v,
            0x32 => {
                let i = u16::from(v & 31);
                if v & 0x20 != 0 {
                    self.fixed = (self.fixed & !0x001f) | i;
                }
                if v & 0x40 != 0 {
                    self.fixed = (self.fixed & !0x03e0) | (i << 5);
                }
                if v & 0x80 != 0 {
                    self.fixed = (self.fixed & !0x7c00) | (i << 10);
                }
            }
            0x33 => self.setini = v,
            _ => {}
        }
    }

    fn mpy(&self) -> u32 {
        let a = i32::from(self.m7[0] as i16);
        let b = i32::from((self.m7[1] >> 8) as i8);
        (a * b) as u32 & 0xff_ffff
    }

    /// Side-effect-free view of $2100 + `reg` (reads $34–$3F).
    pub fn peek(&self, reg: u8) -> u8 {
        match reg {
            0x34 => self.mpy() as u8,
            0x35 => (self.mpy() >> 8) as u8,
            0x36 => (self.mpy() >> 16) as u8,
            0x38 => self.oam_byte(),
            0x39 => self.vram_buf as u8,
            0x3a => (self.vram_buf >> 8) as u8,
            0x3b => self.cg_byte(),
            0x3c => {
                if self.h_high {
                    (self.hlatch >> 8) as u8 & 1 | (self.ppu2_bus & 0xfe)
                } else {
                    self.hlatch as u8
                }
            }
            0x3d => {
                if self.v_high {
                    (self.vlatch >> 8) as u8 & 1 | (self.ppu2_bus & 0xfe)
                } else {
                    self.vlatch as u8
                }
            }
            0x3e => self.obj_flags | (self.ppu1_bus & 0x10) | 1,
            0x3f => {
                u8::from(self.field) << 7
                    | u8::from(self.latched) << 6
                    | (self.ppu2_bus & 0x20)
                    | u8::from(self.pal) << 4
                    | 3
            }
            _ => self.ppu1_bus,
        }
    }

    fn oam_byte(&self) -> u8 {
        let a = usize::from(self.oam_addr & 0x3ff);
        if a >= 0x200 {
            self.oam[0x200 | (a & 0x1f)]
        } else {
            self.oam[a]
        }
    }

    fn cg_byte(&self) -> u8 {
        let c = self.cgram[usize::from(self.cgadd)];
        if self.cg_high {
            (c >> 8) as u8 & 0x7f | (self.ppu2_bus & 0x80)
        } else {
            c as u8
        }
    }

    /// A read of $2100 + `reg`. `h`, `v`: the counters now (for SLHV).
    pub fn read(&mut self, reg: u8, h: u16, v: u16, wrio7: bool) -> u8 {
        match reg {
            0x34..=0x36 => {
                let r = self.peek(reg);
                self.ppu1_bus = r;
                r
            }
            0x37 => {
                if wrio7 {
                    self.latch(h, v);
                }
                self.ppu1_bus
            }
            0x38 => {
                let r = self.oam_byte();
                self.oam_addr = (self.oam_addr + 1) & 0x3ff;
                self.ppu1_bus = r;
                r
            }
            0x39 | 0x3a => {
                let r = self.peek(reg);
                if (reg == 0x3a) == (self.vmain & 0x80 != 0) {
                    self.vram_buf = self.vram[self.vram_word()];
                    self.vram_step();
                }
                self.ppu1_bus = r;
                r
            }
            0x3b => {
                let r = self.cg_byte();
                if self.cg_high {
                    self.cgadd = self.cgadd.wrapping_add(1);
                }
                self.cg_high = !self.cg_high;
                self.ppu2_bus = r;
                r
            }
            0x3c => {
                let r = self.peek(reg);
                self.h_high = !self.h_high;
                self.ppu2_bus = r;
                r
            }
            0x3d => {
                let r = self.peek(reg);
                self.v_high = !self.v_high;
                self.ppu2_bus = r;
                r
            }
            0x3e => {
                let r = self.peek(reg);
                self.ppu1_bus = r;
                r
            }
            0x3f => {
                let r = self.peek(reg);
                self.latched = false;
                self.h_high = false;
                self.v_high = false;
                self.ppu2_bus = r;
                r
            }
            _ => self.ppu1_bus,
        }
    }

    // ---- rendering

    fn mode(&self) -> u8 {
        self.bgmode & 7
    }

    fn hires(&self) -> bool {
        matches!(self.mode(), 5 | 6)
    }

    /// Bits per pixel of BG `n` in the current mode (0: not present).
    fn bpp(&self, n: usize) -> u8 {
        match (self.mode(), n) {
            (0, _) => 2,
            (1, 0 | 1) => 4,
            (1, 2) => 2,
            (2, 0 | 1) => 4,
            (3, 0) => 8,
            (3, 1) => 4,
            (4, 0) => 8,
            (4, 1) => 2,
            (5, 0) => 4,
            (5, 1) => 2,
            (6, 0) => 4,
            _ => 0,
        }
    }

    /// The window mask of layer `l` (0–3 BG, 4 OBJ, 5 math) at `x`.
    fn window(&self, l: usize, x: u8) -> bool {
        let sel = (self.wsel[l / 2] >> ((l & 1) * 4)) & 0x0f;
        let logic = (self.wlog[l / 4] >> ((l & 3) * 2)) & 3;
        let w = |i: usize| -> Option<bool> {
            let bits = (sel >> (2 * i)) & 3;
            if bits & 2 == 0 {
                return None;
            }
            let (l, r) = (self.wpos[2 * i], self.wpos[2 * i + 1]);
            let inside = l <= x && x <= r;
            Some(inside != (bits & 1 != 0))
        };
        match (w(0), w(1)) {
            (None, None) => false,
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => match logic {
                0 => a | b,
                1 => a & b,
                2 => a ^ b,
                _ => !(a ^ b),
            },
        }
    }

    fn mosaic_size(&self) -> u16 {
        u16::from(self.mosaic >> 4) + 1
    }

    fn map_entry(&self, n: usize, tx: u16, ty: u16) -> u16 {
        let sc = self.bgsc[n];
        let base = u16::from(sc & 0xfc) << 8;
        let mut off = (ty & 31) * 32 + (tx & 31);
        match sc & 3 {
            1 => off += (tx & 32) << 5,
            2 => off += (ty & 32) << 5,
            3 => off += ((tx & 32) << 5) + ((ty & 32) << 6),
            _ => {}
        }
        self.vram[usize::from(base.wrapping_add(off) & 0x7fff)]
    }

    /// The colour index of pixel (px, py) of 8x8 character `ch` of BG `n`.
    fn bg_pixel(&self, n: usize, bpp: u8, ch: u16, px: u16, py: u16) -> u16 {
        let base = u16::from(self.nba[n]) << 12;
        let words = u16::from(bpp) * 4;
        let addr = base.wrapping_add(ch.wrapping_mul(words)).wrapping_add(py);
        let bit = 7 - px;
        let mut c = 0;
        for plane in 0..u16::from(bpp) / 2 {
            let w = self.vram[usize::from(addr.wrapping_add(plane * 8) & 0x7fff)];
            c |= ((w >> bit) & 1) << (plane * 2);
            c |= ((w >> (8 + bit)) & 1) << (plane * 2 + 1);
        }
        c
    }

    /// Offset-per-tile: the scroll of BG `n` for the tile column at screen
    /// x (modes 2, 4, 6).
    fn opt_scroll(&self, n: usize, x: u16) -> (u16, u16) {
        let (mut h, mut v) = (self.hofs[n] & 0x3ff, self.vofs[n]);
        let col = (x + (self.hofs[n] & 7)) >> 3;
        if col == 0 {
            return (h, v);
        }
        let tx = ((self.hofs[2] >> 3) + col - 1) & 63;
        let ty = (self.vofs[2] >> 3) & 63;
        let bit = 0x2000 << n;
        let first = self.map_entry(2, tx, ty);
        if self.mode() == 4 {
            if first & bit != 0 {
                if first & 0x8000 != 0 {
                    v = first & 0x3ff;
                } else {
                    h = (first & 0x3f8) | (h & 7);
                }
            }
        } else {
            let second = self.map_entry(2, tx, ty + 1);
            if first & bit != 0 {
                h = (first & 0x3f8) | (h & 7);
            }
            if second & bit != 0 {
                v = second & 0x3ff;
            }
        }
        (h, v)
    }

    /// One line of BG `n` (0–3) in modes 0–6, `width` pixels (512 in
    /// hires).
    fn render_bg(&self, n: usize, y: u16, out: &mut [Px]) {
        let bpp = self.bpp(n);
        let width = out.len() as u16;
        let hires = width == 512;
        let big = self.bgmode & (0x10 << n) != 0;
        let (tw, th) = match (big, hires) {
            (false, false) => (8, 8),
            (true, false) => (16, 16),
            (false, true) => (16, 8),
            (true, true) => (16, 16),
        };
        let mosaic = if self.mosaic & (1 << n) != 0 {
            self.mosaic_size()
        } else {
            1
        };
        let line = y - 1;
        let my = line - line % mosaic;
        let opt = matches!(self.mode(), 2 | 4 | 6) && n < 2;
        let pal_base = match (self.mode(), bpp) {
            (0, _) => (n as u16) * 32,
            _ => 0,
        };
        let direct = bpp == 8 && self.cgwsel & 1 != 0;
        let mut x = 0;
        while x < width {
            let sx = if hires { x / 2 } else { x };
            let msx = sx - sx % mosaic;
            let (hofs, vofs) = if opt {
                self.opt_scroll(n, msx)
            } else {
                (self.hofs[n] & 0x3ff, self.vofs[n])
            };
            let cx = if hires {
                (hofs * 2).wrapping_add(msx * 2 + (x & 1))
            } else {
                hofs.wrapping_add(msx)
            };
            let cy = vofs.wrapping_add(my) & 0x3ff;
            let tx = (cx / tw) & 63;
            let ty = (cy / th) & 63;
            let e = self.map_entry(n, tx, ty);
            let (mut ix, mut iy) = (cx % tw, cy % th);
            if e & 0x4000 != 0 {
                ix = tw - 1 - ix;
            }
            if e & 0x8000 != 0 {
                iy = th - 1 - iy;
            }
            let ch = (e & 0x3ff).wrapping_add(ix / 8).wrapping_add((iy / 8) * 16) & 0x3ff;
            let c = self.bg_pixel(n, bpp, ch, ix & 7, iy & 7);
            let pal = (e >> 10) & 7;
            let prio = ((e >> 13) & 1) as u8;
            out[usize::from(x)] = if c == 0 {
                Px::default()
            } else if direct {
                Px {
                    colour: direct_colour(c as u8, pal as u8),
                    prio,
                    opaque: true,
                    math: true,
                }
            } else {
                let idx = match bpp {
                    2 => pal_base + pal * 4 + c,
                    4 => pal * 16 + c,
                    _ => c,
                };
                Px {
                    colour: self.cgram[usize::from(idx & 0xff)],
                    prio,
                    opaque: true,
                    math: true,
                }
            };
            x += 1;
        }
    }

    /// Mode 7: BG1 (and with EXTBG, BG2 with bit 7 as priority).
    fn render_mode7(&self, y: u16, bg1: &mut [Px; 256], bg2: &mut [Px; 256]) {
        let s13 = |v: u16| i32::from(((v << 3) as i16) >> 3);
        let [a, b, c, d] = self.m7.map(|v| i32::from(v as i16));
        let (cx, cy) = (s13(self.m7x), s13(self.m7y));
        let (hofs, vofs) = (s13(self.m7hofs), s13(self.m7vofs));
        // ORG = (scroll − centre) wrapped into −1024 … 1023 (fullsnes).
        let clip = |v: i32| {
            if v < 0 {
                (v & !0x1c00) | 0x1c00
            } else {
                v & !0x1c00
            }
        };
        let (ox, oy) = (clip(hofs - cx), clip(vofs - cy));
        let mosaic = if self.mosaic & 1 != 0 {
            self.mosaic_size()
        } else {
            1
        };
        let mut sy = i32::from(y - (y - 1) % mosaic);
        if self.m7sel & 2 != 0 {
            sy ^= 0xff;
        }
        let over = self.m7sel >> 6;
        let direct = self.cgwsel & 1 != 0;
        let extbg = self.setini & 0x40 != 0;
        for x in 0..256i32 {
            let mx = x - x % i32::from(mosaic);
            let sx = if self.m7sel & 1 != 0 { mx ^ 0xff } else { mx };
            let vx =
                ((a * ox) & !0x3f) + ((b * oy) & !0x3f) + cx * 256 + ((b * sy) & !0x3f) + a * sx;
            let vy =
                ((c * ox) & !0x3f) + ((d * oy) & !0x3f) + cy * 256 + ((d * sy) & !0x3f) + c * sx;
            let (px, py) = (vx >> 8, vy >> 8);
            let outside = !(0..1024).contains(&px) || !(0..1024).contains(&py);
            let tile = if outside {
                match over {
                    2 => {
                        bg1[x as usize] = Px::default();
                        bg2[x as usize] = Px::default();
                        continue;
                    }
                    3 => 0,
                    _ => self.vram[(((py & 1023) >> 3) * 128 + ((px & 1023) >> 3)) as usize] & 0xff,
                }
            } else {
                self.vram[((py >> 3) * 128 + (px >> 3)) as usize] & 0xff
            };
            let addr = (tile * 64) as usize + ((py & 7) * 8 + (px & 7)) as usize;
            let col = (self.vram[addr & 0x7fff] >> 8) as u8;
            bg1[x as usize] = if col == 0 {
                Px::default()
            } else {
                Px {
                    colour: if direct {
                        direct_colour(col, 0)
                    } else {
                        self.cgram[usize::from(col)]
                    },
                    prio: 0,
                    opaque: true,
                    math: true,
                }
            };
            bg2[x as usize] = if !extbg || col & 0x7f == 0 {
                Px::default()
            } else {
                Px {
                    colour: self.cgram[usize::from(col & 0x7f)],
                    prio: col >> 7,
                    opaque: true,
                    math: true,
                }
            };
        }
    }

    /// The sprites of line `y`; sets the range/time overflow flags.
    fn render_obj(&mut self, y: u16, out: &mut [Px; 256]) {
        let (small, large) = OBJ_SIZES[usize::from(self.obsel >> 5)];
        let base = u16::from(self.obsel & 7) << 13;
        let gap = (u16::from((self.obsel >> 3) & 3) + 1) << 12;
        let first = if self.oam_rotate {
            usize::from((self.oam_reload >> 1) & 0x7f)
        } else {
            0
        };
        let line = (y - 1) as u8;
        let mut found = Vec::with_capacity(32);
        for k in 0..128 {
            let i = (first + k) & 127;
            let hi = (self.oam[0x200 + i / 4] >> ((i & 3) * 2)) & 3;
            let (w, h) = if hi & 2 != 0 { large } else { small };
            let oy = self.oam[i * 4 + 1];
            let row = line.wrapping_sub(oy);
            if u16::from(row) >= h {
                continue;
            }
            let x = i32::from(self.oam[i * 4]) | if hi & 1 != 0 { -256 } else { 0 };
            if x > -(w as i32) && x < 256 || x == -256 {
                if found.len() == 32 {
                    self.obj_flags |= 0x40;
                    break;
                }
                found.push((i, x, w, h, row));
            }
        }
        let mut slivers = 0;
        for &(i, x, w, h, row) in found.iter().rev() {
            let tile = u16::from(self.oam[i * 4 + 2]);
            let attr = self.oam[i * 4 + 3];
            let name = u16::from(attr & 1) << 8;
            let pal = u16::from((attr >> 1) & 7);
            let prio = (attr >> 4) & 3;
            let r = if attr & 0x80 != 0 {
                h - 1 - u16::from(row)
            } else {
                u16::from(row)
            };
            for col in 0..w / 8 {
                let sx = x + i32::from(col) * 8;
                if sx <= -8 || sx >= 256 {
                    continue;
                }
                slivers += 1;
                if slivers > 34 {
                    self.obj_flags |= 0x80;
                    break;
                }
                let tc = if attr & 0x40 != 0 {
                    w / 8 - 1 - col
                } else {
                    col
                };
                let ch = ((tile + (r / 8) * 16) & 0xf0) | ((tile + tc) & 0x0f);
                let addr = base + if name != 0 { gap } else { 0 } + ch * 16 + (r & 7);
                for px in 0..8 {
                    let ox = sx + px;
                    if !(0..256).contains(&ox) {
                        continue;
                    }
                    let bit = if attr & 0x40 != 0 { px } else { 7 - px };
                    let lo = self.vram[usize::from(addr & 0x7fff)];
                    let hi = self.vram[usize::from(addr.wrapping_add(8) & 0x7fff)];
                    let c = ((lo >> bit) & 1)
                        | ((lo >> (8 + bit)) & 1) << 1
                        | ((hi >> bit) & 1) << 2
                        | ((hi >> (8 + bit)) & 1) << 3;
                    if c != 0 {
                        out[ox as usize] = Px {
                            colour: self.cgram[usize::from(128 + pal * 16 + c)],
                            prio,
                            opaque: true,
                            math: pal >= 4,
                        };
                    }
                }
            }
            if slivers > 34 {
                break;
            }
        }
    }

    /// Draw line `y` (1–239).
    pub fn render_line(&mut self, y: u16) {
        if y == 0 || usize::from(y) > LINES {
            return;
        }
        let rows = [self.row(y)];
        let bright = self.inidisp & 0x0f;
        if self.forced_blank() || y > self.visible_lines() || bright == 0 {
            for r in rows {
                self.frame[r * WIDTH..(r + 1) * WIDTH].fill(Rgb8::default());
            }
            // The sprite scan still runs (overflow flags) unless blanked.
            if !self.forced_blank() && y <= self.visible_lines() {
                let mut obj = [Px::default(); 256];
                self.render_obj(y, &mut obj);
            }
            return;
        }
        let hires = self.hires();
        let w = if hires { 512 } else { 256 };
        let mut bgs = vec![vec![Px::default(); w]; 4];
        let mut obj = [Px::default(); 256];
        let enabled = self.tm | self.ts;
        if self.mode() == 7 {
            let mut b1 = [Px::default(); 256];
            let mut b2 = [Px::default(); 256];
            if enabled & 3 != 0 {
                self.render_mode7(y, &mut b1, &mut b2);
            }
            bgs[0].copy_from_slice(&b1);
            bgs[1].copy_from_slice(&b2);
        } else {
            for (n, bg) in bgs.iter_mut().enumerate() {
                if self.bpp(n) != 0 && enabled & (1 << n) != 0 {
                    self.render_bg(n, y, bg);
                }
            }
        }
        self.render_obj(y, &mut obj);
        let order = self.priority_order();
        let layers = Layers {
            bgs: &bgs,
            obj: &obj,
            order: &order,
        };
        let row_px = self.compose(&layers, w, bright);
        for r in rows {
            self.frame[r * WIDTH..(r + 1) * WIDTH].copy_from_slice(&row_px);
        }
    }

    /// Layers from front to back: (layer, priority).
    fn priority_order(&self) -> Vec<(usize, u8)> {
        let o = |p| (OBJ, p);
        let b = |n, p| (BG1 + n, p);
        match self.mode() {
            0 => vec![
                o(3),
                b(0, 1),
                b(1, 1),
                o(2),
                b(0, 0),
                b(1, 0),
                o(1),
                b(2, 1),
                b(3, 1),
                o(0),
                b(2, 0),
                b(3, 0),
            ],
            1 => {
                let mut v = Vec::new();
                let bg3_top = self.bgmode & 8 != 0;
                if bg3_top {
                    v.push(b(2, 1));
                }
                v.extend([o(3), b(0, 1), b(1, 1), o(2), b(0, 0), b(1, 0), o(1)]);
                if !bg3_top {
                    v.push(b(2, 1));
                }
                v.extend([o(0), b(2, 0)]);
                v
            }
            7 => vec![o(3), o(2), b(1, 1), o(1), b(0, 0), o(0), b(1, 0)],
            _ => vec![o(3), b(0, 1), o(2), b(1, 1), o(1), b(0, 0), o(0), b(1, 0)],
        }
    }

    /// Pick the front pixel of the layers in `mask` (TM or TS) outside
    /// their disabled window areas (`wmask`).
    fn front(&self, l: &Layers, x: usize, sx: usize, mask: u8, wmask: u8) -> Option<(usize, Px)> {
        for &(layer, prio) in l.order {
            if mask & (1 << layer) == 0 {
                continue;
            }
            if wmask & (1 << layer) != 0 && self.window(layer, sx as u8) {
                continue;
            }
            let p = if layer == OBJ {
                l.obj[sx]
            } else {
                l.bgs[layer][x]
            };
            if p.opaque && p.prio == prio {
                return Some((layer, p));
            }
        }
        None
    }

    fn compose(&self, l: &Layers, w: usize, bright: u8) -> Vec<Rgb8> {
        let hires = w == 512;
        let pseudo = self.setini & 8 != 0;
        let mut out = vec![Rgb8::default(); WIDTH];
        for (x2, px) in out.iter_mut().enumerate() {
            let sx = x2 / 2;
            let bx = if hires { x2 } else { sx };
            // In hires the even half-pixels come from the sub screen.
            let sub_half = (hires || pseudo) && x2 & 1 == 0;
            let main = self.front(l, bx | usize::from(hires), sx, self.tm, self.tmw);
            let sub = if self.cgwsel & 2 != 0 || sub_half {
                self.front(l, bx & !usize::from(hires), sx, self.ts, self.tsw)
            } else {
                None
            };
            let (main_layer, main_px) = main.unwrap_or((
                BACK,
                Px {
                    colour: self.cgram[0],
                    ..Px::default()
                },
            ));
            if sub_half {
                let c = sub.map_or(self.cgram[0], |(_, p)| p.colour);
                *px = rgb(c, bright);
                continue;
            }
            let win = self.window(5, sx as u8);
            let black = match self.cgwsel >> 6 {
                0 => false,
                1 => !win,
                2 => win,
                _ => true,
            };
            let math_on = match (self.cgwsel >> 4) & 3 {
                0 => true,
                1 => win,
                2 => !win,
                _ => false,
            };
            let layer_math = match main_layer {
                OBJ => main_px.math && self.cgadsub & 0x10 != 0,
                l => self.cgadsub & (1 << l) != 0,
            };
            let mut c = if black { 0 } else { main_px.colour };
            if math_on && layer_math {
                let (sc, sub_backdrop) = match sub {
                    Some((_, p)) => (p.colour, false),
                    None => (self.fixed, true),
                };
                let half =
                    self.cgadsub & 0x40 != 0 && !black && (!sub_backdrop || self.cgwsel & 2 == 0);
                c = colour_math(c, sc, self.cgadsub & 0x80 != 0, half);
            }
            *px = rgb(c, bright);
        }
        out
    }
}

fn direct_colour(c: u8, pal: u8) -> u16 {
    let c = u16::from(c);
    let p = u16::from(pal);
    let r = ((c & 7) << 2) | ((p & 1) << 1);
    let g = (((c >> 3) & 7) << 2) | (p & 2);
    let b = (((c >> 6) & 3) << 3) | (p & 4);
    r | g << 5 | b << 10
}

fn colour_math(a: u16, b: u16, sub: bool, half: bool) -> u16 {
    let mut out = 0;
    for shift in [0, 5, 10] {
        let (x, y) = (i32::from((a >> shift) & 31), i32::from((b >> shift) & 31));
        let mut v = if sub { x - y } else { x + y };
        if half {
            v >>= 1;
        }
        out |= (v.clamp(0, 31) as u16) << shift;
    }
    out
}

const STATE_TAG: [u8; 4] = *b"SPPU";

impl Savestate for SPpu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in &self.vram {
                w.u16(*v);
            }
            for v in &self.cgram {
                w.u16(*v);
            }
            w.raw(&self.oam);
            for v in [
                self.inidisp,
                self.obsel,
                self.oam_latch,
                self.bgmode,
                self.mosaic,
                self.bg_old,
                self.m7_old,
                self.vmain,
                self.m7sel,
                self.cgadd,
                self.cg_latch,
                self.tm,
                self.ts,
                self.tmw,
                self.tsw,
                self.cgwsel,
                self.cgadsub,
                self.setini,
                self.obj_flags,
                self.ppu1_bus,
                self.ppu2_bus,
            ] {
                w.u8(v);
            }
            w.raw(&self.bgsc);
            w.raw(&self.nba);
            w.raw(&self.wsel);
            w.raw(&self.wpos);
            w.raw(&self.wlog);
            for v in self.hofs.iter().chain(&self.vofs).chain(&self.m7) {
                w.u16(*v);
            }
            for v in [
                self.oam_reload,
                self.oam_addr,
                self.m7hofs,
                self.m7vofs,
                self.vmaddr,
                self.vram_buf,
                self.m7x,
                self.m7y,
                self.fixed,
                self.hlatch,
                self.vlatch,
            ] {
                w.u16(v);
            }
            for v in [
                self.oam_rotate,
                self.cg_high,
                self.h_high,
                self.v_high,
                self.latched,
                self.field,
            ] {
                w.bool(v);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut p = self.clone();
        for v in &mut p.vram {
            *v = c.u16()?;
        }
        for v in &mut p.cgram {
            *v = c.u16()? & 0x7fff;
        }
        c.raw_into(&mut p.oam)?;
        for v in [
            &mut p.inidisp,
            &mut p.obsel,
            &mut p.oam_latch,
            &mut p.bgmode,
            &mut p.mosaic,
            &mut p.bg_old,
            &mut p.m7_old,
            &mut p.vmain,
            &mut p.m7sel,
            &mut p.cgadd,
            &mut p.cg_latch,
            &mut p.tm,
            &mut p.ts,
            &mut p.tmw,
            &mut p.tsw,
            &mut p.cgwsel,
            &mut p.cgadsub,
            &mut p.setini,
            &mut p.obj_flags,
            &mut p.ppu1_bus,
            &mut p.ppu2_bus,
        ] {
            *v = c.u8()?;
        }
        c.raw_into(&mut p.bgsc)?;
        c.raw_into(&mut p.nba)?;
        c.raw_into(&mut p.wsel)?;
        c.raw_into(&mut p.wpos)?;
        c.raw_into(&mut p.wlog)?;
        for v in p
            .hofs
            .iter_mut()
            .chain(p.vofs.iter_mut())
            .chain(p.m7.iter_mut())
        {
            *v = c.u16()?;
        }
        for v in [
            &mut p.oam_reload,
            &mut p.oam_addr,
            &mut p.m7hofs,
            &mut p.m7vofs,
            &mut p.vmaddr,
            &mut p.vram_buf,
            &mut p.m7x,
            &mut p.m7y,
            &mut p.fixed,
            &mut p.hlatch,
            &mut p.vlatch,
        ] {
            *v = c.u16()?;
        }
        for v in [
            &mut p.oam_rotate,
            &mut p.cg_high,
            &mut p.h_high,
            &mut p.v_high,
            &mut p.latched,
            &mut p.field,
        ] {
            *v = c.bool()?;
        }
        p.oam_addr &= 0x3ff;
        p.oam_reload &= 0x1ff;
        *self = p;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
