//! Ricoh 2C02 (NTSC) / 2C07 (PAL) picture processing unit.
//!
//! **Timing.** 341 dots a line; NTSC 262 lines (0–239 picture, 240 idle,
//! vertical blank from line 241 dot 1, pre-render line 261), PAL 312 lines
//! (pre-render line 311). On NTSC, odd frames with rendering enabled skip
//! the last dot of the pre-render line. [`Ppu::tick`] runs one dot.
//!
//! **Registers** ($2000–$2007, repeated every 8 bytes):
//! - $2000 PPUCTRL: bits 0–1 base nametable, 2 increment 32, 3 sprite
//!   pattern table ($1000), 4 background pattern table, 5 8×16 sprites,
//!   7 NMI at vertical blank.
//! - $2001 PPUMASK: bit 0 greyscale, 1/2 background/sprites in the left 8
//!   pixels, 3/4 background/sprites on, 5–7 emphasis (NTSC red, green,
//!   blue; the 2C07 swaps red and green).
//! - $2002 PPUSTATUS: bit 5 sprite overflow, 6 sprite 0 hit, 7 vertical
//!   blank (cleared by the read, which also resets the write toggle).
//! - $2003 OAMADDR, $2004 OAMDATA, $2005 PPUSCROLL, $2006 PPUADDR,
//!   $2007 PPUDATA (reads below $3F00 come through a one-byte buffer).
//!
//! Scrolling uses the internal registers v, t, x and w as the NESdev wiki
//! ("PPU scrolling") describes them. Background tiles are fetched every 8
//! dots into 16-bit shift registers; sprites are evaluated for the next
//! line at dot 257 (all 64 at once, with the hardware's overflow-check
//! bug) and their patterns fetched at dots 257–320, so a mapper watching
//! the PPU address bus (MMC3) sees the fetches at the right dots.
//!
//! The picture: [`WIDTH`] × [`HEIGHT`], 256 pixels with [`BORDER`] pixels of
//! the backdrop colour on either side, as RGB from [`crate::palette`].
//!
//! The I/O latch (what reads of write-only bits return) decays bit by bit
//! about 0.6 s after it was last driven.
//!
//! Not emulated: the sprite evaluation's per-dot timing (overflow and the
//! OAM bus are resolved at dot 257), OAM decay and the 2C07's OAM refresh,
//! the three-dot delay of $2006's second write.

use emu_core::frame::Rgb8;
use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

use crate::palette;

/// Backdrop columns either side of the 256 picture pixels (the 2C02 shows
/// 15 on the left and 11 on the right; the frame is centred).
pub const BORDER: usize = 13;
pub const WIDTH: usize = 256 + 2 * BORDER;
pub const HEIGHT: usize = 240;
pub const DOTS: u16 = 341;

/// Palette RAM at power-up: not defined by the chip; these are the values
/// blargg read from his NES (blargg_ppu_tests_2005.09.15b,
/// power_up_palette).
const POWER_UP_PALETTE: [u8; 32] = [
    0x09, 0x01, 0x00, 0x01, 0x00, 0x02, 0x02, 0x0d, 0x08, 0x10, 0x08, 0x24, 0x00, 0x00, 0x04, 0x2c,
    0x09, 0x01, 0x34, 0x03, 0x00, 0x04, 0x00, 0x14, 0x08, 0x3a, 0x00, 0x02, 0x00, 0x20, 0x2c, 0x08,
];

/// Frames (fields) after which an undriven I/O latch bit reads 0.
const LATCH_DECAY_FIELDS: u64 = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpuModel {
    /// 2C02: 262 lines, odd frames one dot shorter.
    Ntsc,
    /// 2C07: 312 lines, red and green emphasis swapped.
    Pal,
}

impl PpuModel {
    /// The pre-render line (the last).
    pub const fn prerender(self) -> u16 {
        match self {
            PpuModel::Ntsc => 261,
            PpuModel::Pal => 311,
        }
    }

    pub const fn lines(self) -> u16 {
        self.prerender() + 1
    }
}

/// The PPU's address bus: pattern tables ($0000–$1FFF) and nametables
/// ($2000–$2FFF; $3000–$3EFF arrives as $2000–$2EFF) on the cartridge
/// and the console's 2 KB of VRAM. The palette is inside the PPU.
pub trait PpuBus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, v: u8);
    fn peek(&self, addr: u16) -> u8;
    /// The PPU drives `addr` without reading ($2006 writes): mappers that
    /// watch A12 see it.
    fn set_address(&mut self, _addr: u16) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ppu {
    pub model: PpuModel,
    pub ctrl: u8,
    pub mask: u8,
    /// Bits 5–7 of PPUSTATUS.
    pub status: u8,
    pub oam_addr: u8,
    pub oam: [u8; 256],
    pub palette: [u8; 32],
    /// Current VRAM address (15 bits), temporary address, fine X, toggle.
    pub v: u16,
    pub t: u16,
    pub x: u8,
    pub w: bool,
    /// PPUDATA read buffer.
    pub buffer: u8,
    /// I/O latch and the field each bit was last driven.
    pub latch: u8,
    latch_driven: [u64; 8],
    /// The line and dot the next tick runs.
    pub line: u16,
    pub dot: u16,
    pub odd: bool,
    /// Fields since power-on.
    pub fields: u64,
    /// A PPUSTATUS read one dot before vertical blank: the flag is not set.
    suppress_vbl: bool,
    /// Writes to $2000/$2001/$2005/$2006 are ignored until the end of the
    /// first vertical blank after power-on or reset.
    warmup: bool,
    /// This odd frame skips the pre-render line's last dot.
    skip: bool,
    // Background pipeline.
    nt: u8,
    at: u8,
    pat_lo: u8,
    pat_hi: u8,
    shift_lo: u16,
    shift_hi: u16,
    attr_lo: u16,
    attr_hi: u16,
    // Sprites of the current line.
    spr_count: u8,
    spr_zero: bool,
    spr_lo: [u8; 8],
    spr_hi: [u8; 8],
    spr_attr: [u8; 8],
    spr_x: [u8; 8],
    /// Secondary OAM: the sprites found for the next line.
    sec: [u8; 32],
    sec_count: u8,
    sec_zero: bool,
    /// The dot of this line at which the sprite overflow flag rises.
    overflow_at: Option<u16>,
    frame: Vec<Rgb8>,
}

impl Ppu {
    pub fn new(model: PpuModel) -> Self {
        Self {
            model,
            ctrl: 0,
            mask: 0,
            status: 0,
            oam_addr: 0,
            oam: [0; 256],
            palette: POWER_UP_PALETTE,
            v: 0,
            t: 0,
            x: 0,
            w: false,
            buffer: 0,
            latch: 0,
            latch_driven: [0; 8],
            line: 0,
            dot: 0,
            odd: false,
            fields: 0,
            suppress_vbl: false,
            warmup: true,
            skip: false,
            nt: 0,
            at: 0,
            pat_lo: 0,
            pat_hi: 0,
            shift_lo: 0,
            shift_hi: 0,
            attr_lo: 0,
            attr_hi: 0,
            spr_count: 0,
            spr_zero: false,
            spr_lo: [0; 8],
            spr_hi: [0; 8],
            spr_attr: [0; 8],
            spr_x: [0; 8],
            sec: [0xff; 32],
            sec_count: 0,
            sec_zero: false,
            overflow_at: None,
            frame: vec![Rgb8::default(); WIDTH * HEIGHT],
        }
    }

    /// The reset line (the console's RESET button): PPUCTRL, PPUMASK, the
    /// write toggle, the read buffer and fine X clear, and register writes
    /// are ignored until the next vertical blank ends. OAM, palette and
    /// the VRAM address survive.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.mask = 0;
        self.w = false;
        self.buffer = 0;
        self.x = 0;
        self.t = 0;
        self.odd = false;
        self.warmup = true;
    }

    /// The picture, [`WIDTH`] × [`HEIGHT`].
    pub fn frame(&self) -> &[Rgb8] {
        &self.frame
    }

    /// The NMI output: vertical blank flag and NMI enable.
    pub fn nmi(&self) -> bool {
        self.status & 0x80 != 0 && self.ctrl & 0x80 != 0
    }

    pub fn rendering(&self) -> bool {
        self.mask & 0x18 != 0
    }

    fn drive_latch(&mut self, v: u8, bits: u8) {
        self.latch = (self.latch & !bits) | (v & bits);
        for b in 0..8 {
            if bits & 1 << b != 0 {
                self.latch_driven[b] = self.fields;
            }
        }
    }

    fn decay_latch(&mut self) {
        for b in 0..8 {
            if self.fields.saturating_sub(self.latch_driven[b]) >= LATCH_DECAY_FIELDS {
                self.latch &= !(1 << b);
            }
        }
    }

    fn palette_index(addr: u16) -> usize {
        let a = usize::from(addr & 0x1f);
        if a & 0x13 == 0x10 {
            a & 0x0f
        } else {
            a
        }
    }

    fn palette_read(&self, addr: u16) -> u8 {
        let c = self.palette[Self::palette_index(addr)];
        if self.mask & 1 != 0 {
            c & 0x30
        } else {
            c
        }
    }

    /// Side-effect-free view of register `reg` (0–7).
    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 7 {
            2 => (self.status & 0xe0) | (self.latch & 0x1f),
            4 => self.oam[usize::from(self.oam_addr)],
            7 => {
                if self.v & 0x3fff >= 0x3f00 {
                    (self.palette_read(self.v) & 0x3f) | (self.latch & 0xc0)
                } else {
                    self.buffer
                }
            }
            _ => self.latch,
        }
    }

    /// A CPU read of register `reg` (0–7).
    pub fn read(&mut self, reg: u8, bus: &mut dyn PpuBus) -> u8 {
        match reg & 7 {
            2 => {
                // The flag is set at line 241 dot 1; a read just before
                // that tick sees it clear and keeps it from being set.
                if self.line == 241 && self.dot == 1 {
                    self.suppress_vbl = true;
                }
                let v = (self.status & 0xe0) | (self.latch & 0x1f);
                self.status &= !0x80;
                self.w = false;
                self.drive_latch(v, 0xe0);
                v
            }
            4 => {
                let v = if self.rendering() && self.line < 240 && (1..=64).contains(&self.dot) {
                    0xff
                } else {
                    self.oam[usize::from(self.oam_addr)]
                };
                self.drive_latch(v, 0xff);
                v
            }
            7 => {
                let addr = self.v & 0x3fff;
                let v = if addr >= 0x3f00 {
                    self.buffer = bus.read(addr - 0x1000);
                    let v = self.palette_read(addr) & 0x3f;
                    self.drive_latch(v, 0x3f);
                    v | (self.latch & 0xc0)
                } else {
                    let v = self.buffer;
                    self.buffer = bus.read(addr);
                    self.drive_latch(v, 0xff);
                    v
                };
                self.increment_v(bus);
                v
            }
            _ => self.latch,
        }
    }

    /// A CPU write to register `reg` (0–7).
    pub fn write(&mut self, reg: u8, v: u8, bus: &mut dyn PpuBus) {
        self.drive_latch(v, 0xff);
        match reg & 7 {
            0 => {
                if !self.warmup {
                    self.ctrl = v;
                    self.t = (self.t & !0x0c00) | (u16::from(v & 3) << 10);
                }
            }
            1 => {
                if !self.warmup {
                    self.mask = v;
                }
            }
            3 => self.oam_addr = v,
            4 => {
                if self.rendering() && (self.line < 240 || self.line == self.model.prerender()) {
                    // During rendering the write is lost and OAMADDR
                    // skips to the next sprite.
                    self.oam_addr = self.oam_addr.wrapping_add(4);
                } else {
                    let v = if self.oam_addr & 3 == 2 { v & 0xe3 } else { v };
                    self.oam[usize::from(self.oam_addr)] = v;
                    self.oam_addr = self.oam_addr.wrapping_add(1);
                }
            }
            5 => {
                if !self.warmup {
                    if self.w {
                        self.t = (self.t & !0x73e0)
                            | (u16::from(v & 7) << 12)
                            | (u16::from(v & 0xf8) << 2);
                    } else {
                        self.t = (self.t & !0x1f) | u16::from(v >> 3);
                        self.x = v & 7;
                    }
                    self.w = !self.w;
                }
            }
            6 => {
                if !self.warmup {
                    if self.w {
                        self.t = (self.t & 0x7f00) | u16::from(v);
                        self.v = self.t;
                        bus.set_address(self.v & 0x3fff);
                    } else {
                        self.t = (self.t & 0x00ff) | (u16::from(v & 0x3f) << 8);
                    }
                    self.w = !self.w;
                }
            }
            _ => {
                let addr = self.v & 0x3fff;
                if addr >= 0x3f00 {
                    self.palette[Self::palette_index(addr)] = v & 0x3f;
                } else {
                    bus.write(addr, v);
                }
                self.increment_v(bus);
            }
        }
    }

    /// After a PPUDATA access: +1 or +32, or during rendering the glitchy
    /// coarse X and Y increments.
    fn increment_v(&mut self, bus: &mut dyn PpuBus) {
        if self.rendering() && (self.line < 240 || self.line == self.model.prerender()) {
            self.inc_x();
            self.inc_y();
        } else {
            let step = if self.ctrl & 4 != 0 { 32 } else { 1 };
            self.v = (self.v + step) & 0x7fff;
            bus.set_address(self.v & 0x3fff);
        }
    }

    fn inc_x(&mut self) {
        if self.v & 0x1f == 31 {
            self.v = (self.v & !0x1f) ^ 0x0400;
        } else {
            self.v += 1;
        }
    }

    fn inc_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000;
            return;
        }
        self.v &= !0x7000;
        let mut y = (self.v & 0x03e0) >> 5;
        if y == 29 {
            y = 0;
            self.v ^= 0x0800;
        } else if y == 31 {
            y = 0;
        } else {
            y += 1;
        }
        self.v = (self.v & !0x03e0) | (y << 5);
    }

    /// Run one dot. Returns true when the picture is complete (vertical
    /// blank begins).
    pub fn tick(&mut self, bus: &mut dyn PpuBus) -> bool {
        let pre = self.model.prerender();
        // Odd NTSC frames with rendering on skip the pre-render line's last
        // dot; rendering is checked as dot 338 comes up: enabling it one dot
        // later is too late (blargg's 10-even_odd_timing).
        if self.line == pre && self.dot == 338 {
            self.skip = self.odd && self.rendering() && self.model == PpuModel::Ntsc;
        }
        if self.line == pre && self.dot == 340 && std::mem::take(&mut self.skip) {
            self.dot = 0;
            self.line = 0;
            self.odd = false;
            self.fields += 1;
            self.decay_latch();
        }
        let (line, dot) = (self.line, self.dot);
        let mut done = false;
        if line < 240 || line == pre {
            if self.rendering() {
                self.render_dot(bus, line == pre);
            }
            if line < 240 {
                if (1..=256).contains(&dot) {
                    self.output_pixel(usize::from(dot - 1));
                } else if dot == 257 {
                    self.output_border();
                }
            }
        }
        if line == 241 && dot == 1 {
            if !self.suppress_vbl {
                self.status |= 0x80;
            }
            self.suppress_vbl = false;
            done = true;
        } else if line == pre && dot == 1 {
            self.status = 0;
            self.warmup = false;
        }

        self.dot += 1;
        if self.dot >= DOTS {
            self.dot = 0;
            self.line += 1;
            if self.line > pre {
                self.line = 0;
                self.odd = !self.odd;
                self.fields += 1;
                self.decay_latch();
            }
        }
        done
    }

    fn render_dot(&mut self, bus: &mut dyn PpuBus, prerender: bool) {
        let dot = self.dot;
        if dot == 65 {
            self.evaluate_sprites(prerender);
        }
        if self.overflow_at.is_some_and(|d| dot >= d) {
            self.overflow_at = None;
            self.status |= 0x20;
        }
        let fetching = (1..=256).contains(&dot) || (321..=336).contains(&dot);
        if fetching {
            if (2..=257).contains(&dot) || (322..=337).contains(&dot) {
                self.shift();
            }
            match (dot - 1) & 7 {
                0 => {
                    self.reload();
                    self.nt = bus.read(0x2000 | (self.v & 0x0fff));
                }
                2 => {
                    let v = self.v;
                    let a = bus.read(0x23c0 | (v & 0x0c00) | ((v >> 4) & 0x38) | ((v >> 2) & 7));
                    let shift = ((v >> 4) & 4) | (v & 2);
                    self.at = (a >> shift) & 3;
                }
                // A pattern fetch's address is on the bus a dot before the
                // read (an MMC3 sees A12 rise at dot 260, not 261).
                3 => bus.set_address(self.bg_pattern()),
                4 => self.pat_lo = bus.read(self.bg_pattern()),
                6 => self.pat_hi = bus.read(self.bg_pattern() + 8),
                7 => self.inc_x(),
                _ => {}
            }
            if dot == 256 {
                self.inc_y();
            }
        } else if dot == 257 {
            self.shift();
            self.reload();
            self.v = (self.v & !0x041f) | (self.t & 0x041f);
            self.oam_addr = 0;
            self.fetch_sprite_step(bus);
        } else if (258..=320).contains(&dot) {
            self.oam_addr = 0;
            if prerender && (280..=304).contains(&dot) {
                self.v = (self.v & !0x7be0) | (self.t & 0x7be0);
            }
            self.fetch_sprite_step(bus);
        } else if dot == 337 || dot == 339 {
            // The two unused nametable fetches at the end of the line.
            self.nt = bus.read(0x2000 | (self.v & 0x0fff));
        }
        if dot == 337 {
            self.shift();
            self.reload();
        }
    }

    fn bg_pattern(&self) -> u16 {
        (u16::from(self.ctrl & 0x10) << 8) | (u16::from(self.nt) << 4) | ((self.v >> 12) & 7)
    }

    fn shift(&mut self) {
        self.shift_lo <<= 1;
        self.shift_hi <<= 1;
        self.attr_lo <<= 1;
        self.attr_hi <<= 1;
    }

    fn reload(&mut self) {
        self.shift_lo = (self.shift_lo & 0xff00) | u16::from(self.pat_lo);
        self.shift_hi = (self.shift_hi & 0xff00) | u16::from(self.pat_hi);
        self.attr_lo = (self.attr_lo & 0xff00) | if self.at & 1 != 0 { 0xff } else { 0 };
        self.attr_hi = (self.attr_hi & 0xff00) | if self.at & 2 != 0 { 0xff } else { 0 };
    }

    fn sprite_height(&self) -> i32 {
        if self.ctrl & 0x20 != 0 {
            16
        } else {
            8
        }
    }

    /// Find the sprites of the next line in OAM with the overflow check's
    /// diagonal OAM walk. The chip does this over dots 65–256, a sprite
    /// taking 2 dots (8 when it is copied); here it is done at dot 65, and
    /// the overflow flag rises at the dot the chip would read the ninth
    /// sprite's matching byte.
    fn evaluate_sprites(&mut self, prerender: bool) {
        self.sec = [0xff; 32];
        self.sec_count = 0;
        self.sec_zero = false;
        self.overflow_at = None;
        if prerender {
            return;
        }
        let line = i32::from(self.line);
        let h = self.sprite_height();
        let in_range = |y: u8| (0..h).contains(&(line - i32::from(y)));
        let mut dot = 65u16;
        let mut n = 0usize;
        while n < 64 && self.sec_count < 8 {
            let y = self.oam[n * 4];
            dot += 2;
            if in_range(y) {
                let s = usize::from(self.sec_count) * 4;
                self.sec[s..s + 4].copy_from_slice(&self.oam[n * 4..n * 4 + 4]);
                if n == 0 {
                    self.sec_zero = true;
                }
                self.sec_count += 1;
                dot += 6;
            }
            n += 1;
        }
        let mut m = 0usize;
        while n < 64 && dot <= 256 {
            if in_range(self.oam[n * 4 + m]) {
                self.overflow_at = Some(dot);
                break;
            }
            dot += 2;
            n += 1;
            m = (m + 1) & 3;
        }
    }

    /// Dots 257–320: eight sprites, eight dots each (two garbage nametable
    /// reads, then the pattern's low and high bytes).
    /// The pattern address (low plane) of sprite slot `i` for the next line.
    fn sprite_pattern(&self, i: usize) -> u16 {
        let s = i * 4;
        let (y, tile, attr) = (self.sec[s], self.sec[s + 1], self.sec[s + 2]);
        let used = i < usize::from(self.sec_count);
        let h = self.sprite_height();
        let mut row = if used {
            i32::from(self.line) - i32::from(y)
        } else {
            0
        };
        if used && attr & 0x80 != 0 {
            row = h - 1 - row;
        }
        let row = row.clamp(0, 15) as u16;
        if h == 16 {
            let table = u16::from(tile & 1) << 12;
            let t = u16::from(tile & 0xfe) + (row >> 3);
            table | (t << 4) | (row & 7)
        } else {
            (u16::from(self.ctrl & 8) << 9) | (u16::from(tile) << 4) | (row & 7)
        }
    }

    fn fetch_sprite_step(&mut self, bus: &mut dyn PpuBus) {
        let i = usize::from((self.dot - 257) / 8);
        match (self.dot - 257) & 7 {
            0 | 2 => {
                bus.read(0x2000 | (self.v & 0x0fff));
            }
            3 => bus.set_address(self.sprite_pattern(i)),
            4 | 6 => {
                let hi = (self.dot - 257) & 7 == 6;
                let s = i * 4;
                let (attr, x) = (self.sec[s + 2], self.sec[s + 3]);
                let used = i < usize::from(self.sec_count);
                let addr = self.sprite_pattern(i);
                let mut v = bus.read(addr + if hi { 8 } else { 0 });
                if !used {
                    v = 0;
                } else if attr & 0x40 != 0 {
                    v = v.reverse_bits();
                }
                if hi {
                    self.spr_hi[i] = v;
                    self.spr_attr[i] = attr;
                    self.spr_x[i] = x;
                } else {
                    self.spr_lo[i] = v;
                }
                if self.dot == 320 - 1 {
                    self.spr_count = self.sec_count;
                    self.spr_zero = self.sec_zero;
                }
            }
            _ => {}
        }
    }

    fn colour(&self, index: u8) -> Rgb8 {
        let mut emphasis = usize::from(self.mask >> 5);
        if self.model == PpuModel::Pal {
            emphasis = (emphasis & 4) | ((emphasis & 1) << 1) | ((emphasis & 2) >> 1);
        }
        palette::table()[emphasis << 6 | usize::from(index & 0x3f)]
    }

    /// The backdrop: palette entry 0, or with rendering off and v pointing
    /// into the palette, that entry.
    fn backdrop(&self) -> u8 {
        if !self.rendering() && self.v & 0x3f00 == 0x3f00 {
            self.palette_read(self.v)
        } else {
            self.palette_read(0x3f00)
        }
    }

    fn output_pixel(&mut self, x: usize) {
        let c = if self.rendering() {
            let mut bg = 0u8;
            if self.mask & 0x08 != 0 && (x >= 8 || self.mask & 0x02 != 0) {
                let bit = 15 - self.x;
                let p = ((self.shift_lo >> bit) & 1) | (((self.shift_hi >> bit) & 1) << 1);
                if p != 0 {
                    let a = ((self.attr_lo >> bit) & 1) | (((self.attr_hi >> bit) & 1) << 1);
                    bg = (a << 2 | p) as u8;
                }
            }
            let mut spr = 0u8;
            let mut front = false;
            if self.mask & 0x10 != 0 && (x >= 8 || self.mask & 0x04 != 0) {
                for i in 0..usize::from(self.spr_count) {
                    let off = x.wrapping_sub(usize::from(self.spr_x[i]));
                    if off < 8 {
                        let b = 7 - off;
                        let p = ((self.spr_lo[i] >> b) & 1) | (((self.spr_hi[i] >> b) & 1) << 1);
                        if p != 0 {
                            let attr = self.spr_attr[i];
                            spr = 0x10 | (attr & 3) << 2 | p;
                            front = attr & 0x20 == 0;
                            if i == 0 && self.spr_zero && bg != 0 && x != 255 {
                                self.status |= 0x40;
                            }
                            break;
                        }
                    }
                }
            }
            let index = if spr != 0 && (bg == 0 || front) {
                spr
            } else {
                bg
            };
            self.palette_read(0x3f00 | u16::from(index))
        } else {
            self.backdrop()
        };
        let rgb = self.colour(c);
        self.frame[usize::from(self.line) * WIDTH + BORDER + x] = rgb;
    }

    fn output_border(&mut self) {
        let rgb = self.colour(self.backdrop());
        let row = usize::from(self.line) * WIDTH;
        self.frame[row..row + BORDER].fill(rgb);
        self.frame[row + BORDER + 256..row + WIDTH].fill(rgb);
    }
}

const STATE_TAG: [u8; 4] = *b"2C02";

impl Savestate for Ppu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [self.ctrl, self.mask, self.status, self.oam_addr] {
                w.u8(v);
            }
            w.raw(&self.oam);
            w.raw(&self.palette);
            w.u16(self.v);
            w.u16(self.t);
            w.u8(self.x);
            w.bool(self.w);
            w.u8(self.buffer);
            w.u8(self.latch);
            for d in self.latch_driven {
                w.u64(d);
            }
            w.u16(self.line);
            w.u16(self.dot);
            w.bool(self.odd);
            w.u64(self.fields);
            w.bool(self.suppress_vbl);
            w.bool(self.warmup);
            w.bool(self.skip);
            for v in [self.nt, self.at, self.pat_lo, self.pat_hi] {
                w.u8(v);
            }
            for v in [self.shift_lo, self.shift_hi, self.attr_lo, self.attr_hi] {
                w.u16(v);
            }
            w.u8(self.spr_count);
            w.bool(self.spr_zero);
            w.raw(&self.spr_lo);
            w.raw(&self.spr_hi);
            w.raw(&self.spr_attr);
            w.raw(&self.spr_x);
            w.raw(&self.sec);
            w.u8(self.sec_count);
            w.bool(self.sec_zero);
            w.u16(self.overflow_at.unwrap_or(0));
            let mut px = Vec::with_capacity(self.frame.len() * 3);
            for c in &self.frame {
                px.extend_from_slice(&[c.r, c.g, c.b]);
            }
            w.bytes(&px);
        });
    }

    /// Atomic: a failed load leaves the PPU unchanged.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut p = self.clone();
        p.ctrl = c.u8()?;
        p.mask = c.u8()?;
        p.status = c.u8()? & 0xe0;
        p.oam_addr = c.u8()?;
        c.raw_into(&mut p.oam)?;
        c.raw_into(&mut p.palette)?;
        p.v = c.u16()? & 0x7fff;
        p.t = c.u16()? & 0x7fff;
        p.x = c.u8()? & 7;
        p.w = c.bool()?;
        p.buffer = c.u8()?;
        p.latch = c.u8()?;
        for d in &mut p.latch_driven {
            *d = c.u64()?;
        }
        p.line = c.u16()?;
        p.dot = c.u16()?;
        if p.line >= p.model.lines() || p.dot >= DOTS {
            return Err(StateError::Invalid("PPU position"));
        }
        p.odd = c.bool()?;
        p.fields = c.u64()?;
        p.suppress_vbl = c.bool()?;
        p.warmup = c.bool()?;
        p.skip = c.bool()?;
        p.nt = c.u8()?;
        p.at = c.u8()? & 3;
        p.pat_lo = c.u8()?;
        p.pat_hi = c.u8()?;
        p.shift_lo = c.u16()?;
        p.shift_hi = c.u16()?;
        p.attr_lo = c.u16()?;
        p.attr_hi = c.u16()?;
        p.spr_count = c.u8()?;
        p.spr_zero = c.bool()?;
        c.raw_into(&mut p.spr_lo)?;
        c.raw_into(&mut p.spr_hi)?;
        c.raw_into(&mut p.spr_attr)?;
        c.raw_into(&mut p.spr_x)?;
        c.raw_into(&mut p.sec)?;
        p.sec_count = c.u8()?;
        p.sec_zero = c.bool()?;
        let at = c.u16()?;
        p.overflow_at = (at != 0).then_some(at);
        if p.spr_count > 8 || p.sec_count > 8 {
            return Err(StateError::Invalid("PPU sprite count"));
        }
        let px = c.bytes()?;
        if px.len() != p.frame.len() * 3 {
            return Err(StateError::Invalid("PPU frame size"));
        }
        for (d, &[r, g, b]) in p.frame.iter_mut().zip(px.as_chunks::<3>().0) {
            *d = Rgb8 { r, g, b };
        }
        *self = p;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
