//! The Game Boy's picture processing unit (DMG and Pocket): a liquid
//! crystal display of 160 × 144 pixels in four shades, a background of
//! 8 × 8 tiles from one of two 32 × 32 maps, a window over it, and 40
//! objects (sprites) of 8 × 8 or 8 × 16 pixels, ten to a line.
//!
//! **Timing.** One dot is one clock of the machine (4.194304 MHz). A line
//! has 456 dots, a frame 154 lines (59.73 Hz). On the 144 visible lines
//! the unit is in mode 2 for 80 dots (it searches the object table for
//! this line's objects), then in mode 3 while it draws (172 dots, more
//! with a fine scroll and with objects on the line), then in mode 0 (the
//! horizontal blank) for the rest. Lines 144–153 are mode 1, the vertical
//! blank. The video RAM cannot be read or written in mode 3, the object
//! table neither in mode 2 nor 3.
//!
//! **Registers** (`$FF40`–`$FF4B` without `$FF46`, the machine's):
//!
//! | Address | Name | Bits |
//! | --- | --- | --- |
//! | `$FF40` | LCDC | 7 display on, 6 window map `$9C00`, 5 window on, 4 tile data at `$8000` (else `$8800`, signed), 3 background map `$9C00`, 2 objects 8 × 16, 1 objects on, 0 background and window on |
//! | `$FF41` | STAT | 6–3 interrupt on LY = LYC, mode 2, mode 1, mode 0; 2 LY = LYC; 1–0 the mode |
//! | `$FF42`, `$FF43` | SCY, SCX | the background's scroll |
//! | `$FF44` | LY | the line (read only) |
//! | `$FF45` | LYC | the line to compare |
//! | `$FF47` | BGP | the shades of the background's colours 0–3, two bits each |
//! | `$FF48`, `$FF49` | OBP0, OBP1 | the objects' two palettes (colour 0 is transparent) |
//! | `$FF4A`, `$FF4B` | WY, WX | the window's place (WX is 7 more than the column) |
//!
//! **Interrupts.** The vertical blank's when line 144 begins. STAT's
//! sources are ORed to one line and the interrupt is requested when that
//! line rises, so a source does not interrupt while another holds the
//! line ("STAT blocking").
//!
//! **What the processor sees.** A read or write of the processor falls
//! some clocks before the end of its machine cycle, where interrupts are
//! sampled; with the unit's events on the cycles' ends, STAT's mode bits
//! and the memory locks' release therefore follow the unit's own mode one
//! dot late (a read at a mode's first dot still gets the old one), while
//! the interrupt comes at the mode's first dot. LY changes at a line's
//! dot 0; its comparison with LYC is false for the line's first four
//! dots. Reading closes with the unit's own mode (the object table at
//! dot 0, the video RAM at dot 80); writing follows the mode as read, and
//! the object table takes a write at dot 80, between modes 2 and 3. With
//! the display off the comparison's result is kept; switching on compares
//! at once, and the first line starts four dots in, in mode 0 until its
//! mode 3. Mode 2's interrupt source also fires when line 144 begins.
//! (All measured by the mooneye test suite's `ppu` tests.)
//!
//! **Picture.** Drawn a pixel a dot during mode 3 from the registers as
//! they are at that dot, so a write between two lines and one inside a
//! line take effect where they would. Not modelled: the pixel pipeline's
//! own delays (a write inside mode 3 lands some dots too early; mode 3's
//! length with objects is taken from a rule, see `object_dots`, and with
//! the window not at all), the object table's corruption bug.
//!
//! An object's attributes: bit 7 behind the background's colours 1–3, 6
//! flipped vertically, 5 horizontally, 4 palette OBP1. Of two objects on a
//! pixel the one with the smaller X wins, with the same X the earlier one
//! in the table.
//!
//! Sources: the Pan Docs (<https://gbdev.io/pandocs/>: LCDC, STAT, the
//! modes and their timing, scrolling, the window, OAM, object priority,
//! STAT blocking); Joonas Javanainen, *Game Boy: Complete Technical
//! Reference* (the registers) and his mooneye test suite
//! (<https://github.com/Gekkio/mooneye-test-suite>: what its tests'
//! comments state about real machines). The implementation is this project's own.

use mimic_core::frame::Rgb8;
use mimic_core::{Savestate, StateError, StateReader, StateWriter};

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 144;
/// Dots a line.
pub const LINE_DOTS: u16 = 456;
/// Lines a frame.
pub const LINES: u8 = 154;

/// The four shades of the DMG's display, lightest first (greenish, as
/// the reflective panel looks).
pub const PALETTE_DMG: [Rgb8; 4] = [
    Rgb8 {
        r: 0x9b,
        g: 0xbc,
        b: 0x0f,
    },
    Rgb8 {
        r: 0x8b,
        g: 0xac,
        b: 0x0f,
    },
    Rgb8 {
        r: 0x30,
        g: 0x62,
        b: 0x30,
    },
    Rgb8 {
        r: 0x0f,
        g: 0x38,
        b: 0x0f,
    },
];

/// The Game Boy Pocket's grey display.
pub const PALETTE_POCKET: [Rgb8; 4] = [
    Rgb8 {
        r: 0xe0,
        g: 0xe0,
        b: 0xd0,
    },
    Rgb8 {
        r: 0xa8,
        g: 0xa8,
        b: 0x98,
    },
    Rgb8 {
        r: 0x60,
        g: 0x60,
        b: 0x58,
    },
    Rgb8 {
        r: 0x18,
        g: 0x18,
        b: 0x18,
    },
];

/// The interrupts the unit requests, as bits of IF.
pub const INT_VBLANK: u8 = 0x01;
pub const INT_STAT: u8 = 0x02;

const LCDC_ON: u8 = 0x80;
const LCDC_WIN_MAP: u8 = 0x40;
const LCDC_WIN_ON: u8 = 0x20;
const LCDC_DATA: u8 = 0x10;
const LCDC_BG_MAP: u8 = 0x08;
const LCDC_OBJ_16: u8 = 0x04;
const LCDC_OBJ_ON: u8 = 0x02;
const LCDC_BG_ON: u8 = 0x01;

/// Dots of mode 2, and the dot the first pixel leaves the pipeline.
const MODE2_DOTS: u16 = 80;
const FIRST_PIXEL: u16 = 92;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GbPpu {
    pub vram: Vec<u8>,
    pub oam: [u8; 160],
    pub lcdc: u8,
    /// STAT's interrupt enables (bits 6–3).
    stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
    /// The dot in the line.
    pub dot: u16,
    pub mode: u8,
    /// Dots of this line's mode 3.
    mode3: u16,
    /// The mode one dot ago: what the processor reads lags the unit's
    /// own state (see "What the processor sees").
    seen: u8,
    /// The latched comparison of LY with LYC (STAT bit 2).
    lyc_flag: bool,
    /// The first line after the display was switched on.
    first_line: bool,
    /// STAT's interrupt line.
    line: bool,
    /// The window's own line counter and whether WY was met this frame.
    window_line: u8,
    window_met: bool,
    window_used: bool,
    /// This line's objects (indices into the table) and their number.
    objects: [u8; 10],
    object_count: u8,
    /// Pixels drawn of this line.
    x: u8,
    /// Interrupts to request ([`GbPpu::take_irq`]).
    irq: u8,
    /// Frames since power-on.
    pub fields: u64,
    /// The picture: a shade (0–3) a pixel.
    frame: Vec<u8>,
}

impl Default for GbPpu {
    fn default() -> Self {
        Self::new()
    }
}

impl GbPpu {
    /// The unit at power-on: the display off.
    pub fn new() -> Self {
        Self {
            vram: vec![0; 0x2000],
            oam: [0; 160],
            lcdc: 0,
            stat: 0,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0,
            obp0: 0,
            obp1: 0,
            wy: 0,
            wx: 0,
            dot: 0,
            mode: 0,
            mode3: 172,
            seen: 0,
            lyc_flag: false,
            first_line: false,
            line: false,
            window_line: 0,
            window_met: false,
            window_used: false,
            objects: [0; 10],
            object_count: 0,
            x: 0,
            irq: 0,
            fields: 0,
            frame: vec![0; WIDTH * HEIGHT],
        }
    }

    pub fn on(&self) -> bool {
        self.lcdc & LCDC_ON != 0
    }

    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    /// The interrupts requested since the last call (bits of IF).
    pub fn take_irq(&mut self) -> u8 {
        std::mem::take(&mut self.irq)
    }

    /// LY as it reads: line 153 shows 0 after its first dots.
    fn ly_read(&self) -> u8 {
        if self.ly == 153 && self.dot >= 4 {
            0
        } else {
            self.ly
        }
    }

    /// What LY compares as with LYC at this dot; `None` while the
    /// comparison is forced false: in the first four dots of a line whose
    /// LY changed, and on line 153 around LY's early change to 0.
    fn lyc_compare(&self) -> Option<bool> {
        if self.ly == 153 {
            match self.dot {
                0..=3 | 8..=11 => None,
                4..=7 => Some(self.lyc == 153),
                _ => Some(self.lyc == 0),
            }
        } else if self.ly != 0 && self.dot < 4 {
            None
        } else {
            Some(self.ly == self.lyc)
        }
    }

    fn update_line(&mut self) {
        if self.on() {
            self.lyc_flag = self.lyc_compare().unwrap_or(false);
        }
        // Mode 2's source also pulses when line 144 begins.
        let line = self.lyc_flag && self.stat & 0x40 != 0
            || self.on()
                && (self.mode == 2 && self.stat & 0x20 != 0
                    || self.mode == 1 && self.stat & 0x10 != 0
                    || self.mode == 0 && self.stat & 0x08 != 0
                    || self.ly == 144 && self.dot < 4 && self.stat & 0x20 != 0);
        if line && !self.line {
            self.irq |= INT_STAT;
        }
        self.line = line;
    }

    /// A register of `$FF40`–`$FF4B`.
    pub fn read(&self, addr: u16) -> u8 {
        match addr & 0xff {
            0x40 => self.lcdc,
            0x41 => {
                let mode = if self.on() { self.seen } else { 0 };
                0x80 | self.stat | u8::from(self.lyc_flag) << 2 | mode
            }
            0x42 => self.scy,
            0x43 => self.scx,
            0x44 => self.ly_read(),
            0x45 => self.lyc,
            0x47 => self.bgp,
            0x48 => self.obp0,
            0x49 => self.obp1,
            0x4a => self.wy,
            0x4b => self.wx,
            _ => 0xff,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr & 0xff {
            0x40 => {
                let was_on = self.on();
                self.lcdc = v;
                if was_on && !self.on() {
                    // Off: line 0, the picture blank.
                    self.ly = 0;
                    self.dot = 0;
                    self.mode = 0;
                    self.frame.fill(0);
                } else if !was_on && self.on() {
                    // On: line 0 starts four dots in and without its
                    // mode 2.
                    self.ly = 0;
                    self.dot = 4;
                    self.mode = 0;
                    self.seen = 0;
                    self.first_line = true;
                    self.window_line = 0;
                    self.window_met = false;
                    self.object_count = 0;
                    self.mode3 = 172;
                }
            }
            0x41 => self.stat = v & 0x78,
            0x42 => self.scy = v,
            0x43 => self.scx = v,
            0x45 => self.lyc = v,
            0x47 => self.bgp = v,
            0x48 => self.obp0 = v,
            0x49 => self.obp1 = v,
            0x4a => self.wy = v,
            0x4b => self.wx = v,
            _ => {}
        }
        self.update_line();
    }

    /// The video RAM can be read: not in mode 3. The lock closes with
    /// the unit's own mode and opens with the mode the processor sees; on
    /// the first line after switching on it closes late too.
    pub fn vram_free(&self) -> bool {
        !self.on() || self.seen != 3 && (self.first_line || self.mode != 3)
    }

    /// The object table can be read: not in modes 2 and 3.
    pub fn oam_free(&self) -> bool {
        !self.on() || self.seen < 2 && (self.first_line || self.mode < 2)
    }

    /// The video RAM can be written: by the mode the processor sees.
    pub fn vram_writable(&self) -> bool {
        !self.on() || self.seen != 3
    }

    /// The object table can be written: by the mode the processor sees,
    /// and for a moment between modes 2 and 3.
    pub fn oam_writable(&self) -> bool {
        !self.on() || self.seen < 2 || self.seen == 2 && self.mode == 3
    }

    /// Put the beam somewhere (the state the boot ROM leaves).
    pub fn set_position(&mut self, ly: u8, dot: u16) {
        self.ly = ly % LINES;
        self.dot = dot % LINE_DOTS;
        self.mode = match (self.ly, self.dot) {
            (144.., _) => 1,
            (_, 0..=79) => 2,
            (_, 80..=251) => 3,
            _ => 0,
        };
        self.x = if self.mode == 0 { WIDTH as u8 } else { 0 };
        self.seen = self.mode;
        self.first_line = false;
        self.update_line();
        self.irq = 0;
    }

    /// The objects of this line: the first ten in the table's order.
    fn search(&mut self) {
        let height = if self.lcdc & LCDC_OBJ_16 != 0 { 16 } else { 8 };
        self.object_count = 0;
        for i in 0..40u8 {
            let y = i16::from(self.oam[usize::from(i) * 4]) - 16;
            let line = i16::from(self.ly);
            if line >= y && line < y + height && self.object_count < 10 {
                self.objects[usize::from(self.object_count)] = i;
                self.object_count += 1;
            }
        }
    }

    /// The dots this line's objects add to mode 3. Taken from the left:
    /// each object stops the background's fetcher for six dots, and the
    /// first one on a background tile waits until that tile's fetch is
    /// through: up to five dots more, the nearer the object's left edge
    /// is to the tile's (Pan Docs, "OBJ penalty algorithm"; mooneye's
    /// `intr_2_mode0_timing_sprites` measures it).
    fn object_dots(&self) -> u16 {
        if self.lcdc & LCDC_OBJ_ON == 0 {
            return 0;
        }
        let mut xs = [0u8; 10];
        let n = usize::from(self.object_count);
        for (x, &i) in xs.iter_mut().zip(&self.objects[..n]) {
            *x = self.oam[usize::from(i) * 4 + 1];
        }
        xs[..n].sort_unstable();
        let window = self.lcdc & LCDC_WIN_ON != 0 && self.window_met;
        let mut tiles = 0u128;
        let mut dots = 0;
        for &x in &xs[..n] {
            if x >= 168 {
                continue;
            }
            // The object's place in the background's (or window's) row of
            // tiles; an object at X = 0 counts as at a tile's edge.
            let (at, key) = if window && u16::from(x) > u16::from(self.wx) {
                (u16::from(x) - u16::from(self.wx) - 1, 64)
            } else {
                (u16::from(x) + u16::from(self.scx), 0)
            };
            let tile = 1u128 << (key + (at >> 3));
            if tiles & tile == 0 {
                tiles |= tile;
                let pos = if x == 0 { 0 } else { at & 7 };
                dots += 5u16.saturating_sub(pos);
            }
            dots += 6;
        }
        dots
    }

    /// The two bits of pixel (`x`, `y`) of the tile at map entry `tile`.
    fn tile_pixel(&self, tile: u8, x: u8, y: u8, objects: bool) -> u8 {
        let base = if objects || self.lcdc & LCDC_DATA != 0 {
            usize::from(tile) * 16
        } else {
            (0x1000 + i32::from(tile as i8) * 16) as usize
        };
        let at = base + usize::from(y) * 2;
        let bit = 7 - x;
        (self.vram[at] >> bit & 1) | (self.vram[at + 1] >> bit & 1) << 1
    }

    /// One pixel of the line being drawn.
    fn pixel(&mut self) {
        let x = self.x;
        let mut colour = 0;
        if self.lcdc & LCDC_BG_ON != 0 {
            let window = self.lcdc & LCDC_WIN_ON != 0
                && self.window_met
                && u16::from(x) + 7 >= u16::from(self.wx)
                && self.wx < 167;
            let (map, px, py) = if window {
                self.window_used = true;
                (
                    self.lcdc & LCDC_WIN_MAP != 0,
                    (u16::from(x) + 7 - u16::from(self.wx)) as u8,
                    self.window_line,
                )
            } else {
                (
                    self.lcdc & LCDC_BG_MAP != 0,
                    x.wrapping_add(self.scx),
                    self.ly.wrapping_add(self.scy),
                )
            };
            let base = if map { 0x1c00 } else { 0x1800 };
            let tile = self.vram[base + usize::from(py / 8) * 32 + usize::from(px / 8)];
            colour = self.tile_pixel(tile, px & 7, py & 7, false);
        }
        let mut shade = self.bgp >> (colour * 2) & 3;
        if self.lcdc & LCDC_OBJ_ON != 0 {
            let height = if self.lcdc & LCDC_OBJ_16 != 0 { 16 } else { 8 };
            // The smallest X wins, then the earlier one in the table.
            let mut best: Option<(u8, u8, u8)> = None;
            for &i in &self.objects[..usize::from(self.object_count)] {
                let o = &self.oam[usize::from(i) * 4..usize::from(i) * 4 + 4];
                let left = i16::from(o[1]) - 8;
                let col = i16::from(x) - left;
                if !(0..8).contains(&col) || best.is_some_and(|b| b.0 <= o[1]) {
                    continue;
                }
                let mut row = (i16::from(self.ly) - (i16::from(o[0]) - 16)) as u8;
                if o[3] & 0x40 != 0 {
                    row = height - 1 - row;
                }
                let col = if o[3] & 0x20 != 0 {
                    7 - col as u8
                } else {
                    col as u8
                };
                let tile = if height == 16 {
                    (o[2] & 0xfe) | row >> 3
                } else {
                    o[2]
                };
                let c = self.tile_pixel(tile, col, row & 7, true);
                if c != 0 {
                    best = Some((o[1], c, o[3]));
                }
            }
            if let Some((_, c, attr)) = best {
                if attr & 0x80 == 0 || colour == 0 {
                    let palette = if attr & 0x10 != 0 {
                        self.obp1
                    } else {
                        self.obp0
                    };
                    shade = palette >> (c * 2) & 3;
                }
            }
        }
        self.frame[usize::from(self.ly) * WIDTH + usize::from(x)] = shade;
        self.x += 1;
    }

    /// One dot. True when a frame is complete (line 144 begins, or with
    /// the display off as much time has passed).
    pub fn tick(&mut self) -> bool {
        let mut done = false;
        if !self.on() {
            // The display is off; time still passes in frames.
            self.dot += 1;
            if self.dot == LINE_DOTS {
                self.dot = 0;
                self.window_line += 1;
                if self.window_line == LINES {
                    self.window_line = 0;
                    self.fields += 1;
                    done = true;
                }
            }
            return done;
        }
        self.seen = self.mode;
        self.dot += 1;
        if self.dot == LINE_DOTS {
            self.dot = 0;
            if self.window_used {
                self.window_line = self.window_line.wrapping_add(1);
                self.window_used = false;
            }
            self.ly += 1;
            self.first_line = false;
            if self.ly == LINES {
                self.ly = 0;
                self.window_line = 0;
                self.window_met = false;
            }
            if self.ly == 144 {
                self.mode = 1;
                self.irq |= INT_VBLANK;
                self.fields += 1;
                done = true;
            } else if self.ly < 144 {
                self.mode = 2;
            }
        }
        if self.ly < 144 {
            if self.dot == 0 && self.ly == self.wy {
                self.window_met = true;
            }
            if self.dot == MODE2_DOTS {
                self.search();
                self.mode = 3;
                self.x = 0;
                self.mode3 = 172 + u16::from(self.scx & 7) + self.object_dots();
                if self.ly == self.wy {
                    self.window_met = true;
                }
            }
            if self.mode == 3 {
                if self.dot >= FIRST_PIXEL && usize::from(self.x) < WIDTH {
                    self.pixel();
                }
                if self.dot == MODE2_DOTS + self.mode3 {
                    self.mode = 0;
                }
            }
        }
        self.update_line();
        done
    }
}

const STATE_TAG: [u8; 4] = *b"GBPP";

impl Savestate for GbPpu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.bytes(&self.vram);
            w.bytes(&self.oam);
            for v in [
                self.lcdc,
                self.stat,
                self.scy,
                self.scx,
                self.ly,
                self.lyc,
                self.bgp,
                self.obp0,
                self.obp1,
                self.wy,
                self.wx,
                self.mode,
                self.window_line,
                self.object_count,
                self.x,
                self.irq,
            ] {
                w.u8(v);
            }
            w.u16(self.dot);
            w.u16(self.mode3);
            for v in [
                self.line,
                self.window_met,
                self.window_used,
                self.lyc_flag,
                self.first_line,
            ] {
                w.bool(v);
            }
            w.u8(self.seen);
            w.bytes(&self.objects);
            w.u64(self.fields);
            w.bytes(&self.frame);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        let vram = c.bytes()?;
        let oam = c.bytes()?;
        if vram.len() != n.vram.len() || oam.len() != n.oam.len() {
            return Err(StateError::Invalid("Game Boy PPU memory size"));
        }
        n.vram.copy_from_slice(vram);
        n.oam.copy_from_slice(oam);
        n.lcdc = c.u8()?;
        n.stat = c.u8()? & 0x78;
        n.scy = c.u8()?;
        n.scx = c.u8()?;
        n.ly = c.u8()?;
        n.lyc = c.u8()?;
        n.bgp = c.u8()?;
        n.obp0 = c.u8()?;
        n.obp1 = c.u8()?;
        n.wy = c.u8()?;
        n.wx = c.u8()?;
        n.mode = c.u8()? & 3;
        n.window_line = c.u8()?;
        n.object_count = c.u8()?;
        n.x = c.u8()?;
        n.irq = c.u8()?;
        n.dot = c.u16()?;
        n.mode3 = c.u16()?;
        n.line = c.bool()?;
        n.window_met = c.bool()?;
        n.window_used = c.bool()?;
        n.lyc_flag = c.bool()?;
        n.first_line = c.bool()?;
        n.seen = c.u8()? & 3;
        let objects = c.bytes()?;
        let fields = c.u64()?;
        let frame = c.bytes()?;
        if objects.len() != 10
            || frame.len() != n.frame.len()
            || n.ly >= LINES
            || n.dot >= LINE_DOTS
            || n.object_count > 10
            || usize::from(n.x) > WIDTH
            || objects.iter().any(|o| *o >= 40)
            || frame.iter().any(|p| *p > 3)
        {
            return Err(StateError::Invalid("Game Boy PPU state"));
        }
        n.objects.copy_from_slice(objects);
        n.fields = fields;
        n.frame.copy_from_slice(frame);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(p: &mut GbPpu) {
        while !p.tick() {}
    }

    /// A unit with the display on at the start of line 0.
    fn on() -> GbPpu {
        let mut p = GbPpu::new();
        p.bgp = 0xe4;
        p.obp0 = 0xe4;
        p.obp1 = 0x1b;
        p.write(0xff40, 0x91);
        p
    }

    #[test]
    fn a_frame_is_154_lines_of_456_dots() {
        let mut p = on();
        frame(&mut p);
        assert_eq!((p.ly, p.dot, p.mode), (144, 0, 1));
        let mut dots = 0u32;
        loop {
            dots += 1;
            if p.tick() {
                break;
            }
        }
        assert_eq!(dots, 456 * 154);
        assert_eq!(p.fields, 2);
        // With the display off time passes in frames too.
        let mut p = GbPpu::new();
        let mut dots = 0u32;
        loop {
            dots += 1;
            if p.tick() {
                break;
            }
        }
        assert_eq!(dots, 456 * 154);
    }

    #[test]
    fn the_modes_of_a_line() {
        let mut p = on();
        frame(&mut p);
        // To the start of line 0 of the next frame.
        while p.ly != 0 {
            p.tick();
        }
        assert_eq!((p.dot, p.mode), (0, 2));
        assert!(!p.oam_free() && p.vram_free());
        let mut starts = Vec::new();
        let mut mode = p.mode;
        for _ in 0..456 {
            p.tick();
            if p.mode != mode {
                mode = p.mode;
                starts.push((p.ly, p.dot, mode));
            }
        }
        assert_eq!(starts, [(0, 80, 3), (0, 252, 0), (1, 0, 2)]);
        // A fine scroll and objects on the line lengthen mode 3.
        p.scx = 5;
        p.lcdc |= LCDC_OBJ_ON;
        p.oam[0] = 17;
        p.oam[4] = 17;
        while p.mode != 3 {
            p.tick();
        }
        assert!(!p.vram_free());
        while p.mode == 3 {
            p.tick();
        }
        // Two objects at X = 0: six dots each, five more for the first.
        assert_eq!(p.dot, 80 + 172 + 5 + 11 + 6);
        // The processor reads the new mode a dot late, and the locks open
        // with what it reads.
        assert_eq!(p.read(0xff41) & 3, 3);
        assert!(!p.vram_free() && !p.vram_writable());
        p.tick();
        assert_eq!(p.read(0xff41) & 3, 0);
        assert!(p.vram_free() && p.oam_free() && p.oam_writable());
        // The next line: reading closes at dot 0, writing with the mode
        // read; a write to the object table passes at dot 80.
        while p.dot != 0 {
            p.tick();
        }
        assert_eq!(p.read(0xff41) & 3, 0);
        assert!(!p.oam_free() && p.oam_writable());
        for _ in 0..4 {
            p.tick();
        }
        assert_eq!(p.read(0xff41) & 3, 2);
        assert!(!p.oam_writable());
        while p.dot != 80 {
            p.tick();
        }
        assert!(p.oam_writable() && !p.oam_free());
        assert!(!p.vram_free() && p.vram_writable());
        for _ in 0..4 {
            p.tick();
        }
        assert!(!p.oam_writable() && !p.vram_writable());
    }

    #[test]
    fn objects_lengthen_mode_3_by_where_they_stand() {
        // The dots of mode 3 with objects at these X on line 0.
        let dots = |xs: &[u8], scx: u8| {
            let mut p = on();
            p.write(0xff40, 0x93);
            p.scx = scx;
            for (i, x) in xs.iter().enumerate() {
                p.oam[i * 4] = 16;
                p.oam[i * 4 + 1] = *x;
            }
            while p.mode != 3 {
                p.tick();
            }
            let mut n = 0;
            while p.mode == 3 {
                p.tick();
                n += 1;
            }
            n - 172 - u16::from(scx & 7)
        };
        assert_eq!(dots(&[], 0), 0);
        // Six dots, and up to five more by the place in the tile.
        for (x, want) in [(0, 11), (1, 10), (3, 8), (4, 7), (5, 6), (7, 6), (8, 11)] {
            assert_eq!(dots(&[x], 0), want, "X = {x}");
        }
        assert_eq!(dots(&[167], 0), 6);
        assert_eq!(dots(&[168], 0), 0, "off the screen");
        assert_eq!(dots(&[5], 3), 11, "the scroll moves the tiles");
        // Only the first object on a tile waits for it, in any order.
        assert_eq!(dots(&[0; 10], 0), 11 + 9 * 6);
        assert_eq!(dots(&[72, 64, 56, 48, 40, 32, 24, 16, 8, 0], 0), 110);
        assert_eq!(dots(&[3, 1], 0), 10 + 6);
        // Objects switched off cost nothing.
        let mut p = on();
        p.oam[0] = 16;
        while p.mode != 3 {
            p.tick();
        }
        let mut n = 0;
        while p.mode == 3 {
            p.tick();
            n += 1;
        }
        assert_eq!(n, 172);
    }

    #[test]
    fn switching_on_and_the_kept_comparison() {
        // Line 0 after switching on: four dots short, mode 0 until mode 3.
        let mut p = on();
        assert_eq!(p.read(0xff41) & 7, 4);
        let mut dots = 0;
        while p.mode != 3 {
            p.tick();
            dots += 1;
        }
        assert_eq!(dots, 76);
        assert!(p.vram_free() && p.oam_free(), "closes with the mode read");
        while p.ly == 0 {
            p.tick();
            dots += 1;
        }
        assert_eq!(dots, 452);
        // The comparison is false in a line's first four dots.
        p.write(0xff45, 1);
        assert_eq!(p.read(0xff41) & 4, 0);
        for _ in 0..4 {
            p.tick();
        }
        assert_eq!(p.read(0xff41) & 4, 4);
        // Off: the result stays, whatever LYC becomes; on compares at once.
        p.write(0xff41, 0x40);
        p.take_irq();
        p.write(0xff40, 0x11);
        assert_eq!(p.read(0xff41), 0xc4);
        p.write(0xff45, 0);
        assert_eq!(p.read(0xff41), 0xc4);
        p.write(0xff40, 0x91);
        assert_eq!(p.read(0xff41), 0xc4);
        assert_eq!(p.take_irq(), 0, "the line never fell");
        p.write(0xff40, 0x11);
        p.write(0xff45, 9);
        p.write(0xff40, 0x91);
        assert_eq!(p.read(0xff41), 0xc0);
        // Mode 2's source also fires when line 144 begins.
        p.write(0xff41, 0x20);
        while p.ly != 144 {
            p.tick();
            p.take_irq();
        }
        assert!(p.line, "mode 2 at line 144");
    }

    #[test]
    fn interrupts_and_stat_blocking() {
        let mut p = on();
        assert_eq!(p.take_irq(), 0);
        frame(&mut p);
        assert_eq!(p.take_irq(), INT_VBLANK);
        // LY = LYC on line 2, and mode 0: one request while LYC holds
        // the line, one more for the next line's mode 0.
        p.lyc = 2;
        p.write(0xff41, 0x48);
        p.take_irq();
        while p.ly != 2 || p.dot != 4 {
            p.tick();
        }
        assert_eq!(p.take_irq() & INT_STAT, INT_STAT, "LY = LYC");
        assert_eq!(p.read(0xff41) & 0x07, 0x04 | 2);
        while p.ly == 2 {
            p.tick();
            assert_eq!(p.take_irq(), 0, "mode 0 is blocked");
        }
        while p.mode != 0 {
            p.tick();
        }
        assert_eq!(p.take_irq(), INT_STAT);
        // Line 153 reads as 0 early.
        p.write(0xff41, 0);
        while p.ly != 153 {
            p.tick();
        }
        assert_eq!(p.read(0xff44), 153);
        for _ in 0..4 {
            p.tick();
        }
        assert_eq!(p.read(0xff44), 0);
    }

    /// Tile 1: colour 3 everywhere; tile 2: colour 1 in the left half.
    fn tiles(p: &mut GbPpu) {
        for row in 0..8 {
            p.vram[16 + row * 2] = 0xff;
            p.vram[17 + row * 2] = 0xff;
            p.vram[32 + row * 2] = 0xf0;
        }
    }

    #[test]
    fn background_window_and_scroll() {
        let mut p = on();
        tiles(&mut p);
        // Map $9800: tile 1 at (0,0), tile 2 at (1,0); map $9C00 for the
        // window: tile 2 everywhere.
        p.vram[0x1800] = 1;
        p.vram[0x1801] = 2;
        for i in 0..0x400 {
            p.vram[0x1c00 + i] = 2;
        }
        frame(&mut p);
        frame(&mut p);
        let f = p.frame().to_vec();
        assert_eq!(f[..12], [3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1]);
        assert_eq!(f[12], 0);
        assert_eq!(f[8 * WIDTH], 0, "row 8 is tile 0");
        // Scrolled by (4, 0) and the palette reversed.
        p.scx = 4;
        p.bgp = 0x1b;
        frame(&mut p);
        assert_eq!(p.frame()[..9], [0, 0, 0, 0, 2, 2, 2, 2, 3]);
        // The window from line 16, column 80, with its own lines.
        p.scx = 0;
        p.bgp = 0xe4;
        p.wy = 16;
        p.wx = 87;
        p.write(0xff40, 0x91 | 0x60);
        frame(&mut p);
        let f = p.frame();
        assert_eq!(f[15 * WIDTH + 80], 0, "above the window");
        assert_eq!(f[16 * WIDTH + 79..16 * WIDTH + 85], [0, 1, 1, 1, 1, 0]);
        // The background off: white.
        p.write(0xff40, 0x90);
        frame(&mut p);
        assert!(p.frame().iter().all(|s| *s == 0));
    }

    #[test]
    fn objects_their_priority_and_flips() {
        let mut p = on();
        tiles(&mut p);
        p.write(0xff40, 0x93);
        p.obp1 = 0x9b;
        // Object 0 at (16, 8) with tile 2 (left half colour 1), object 1
        // at (12, 8) with tile 1 and palette OBP1, object 2 behind the
        // background, object 3 flipped horizontally.
        p.oam[..16].copy_from_slice(&[
            24, 24, 2, 0x00, 24, 20, 1, 0x10, 40, 8, 1, 0x80, 56, 8, 2, 0x20,
        ]);
        frame(&mut p);
        frame(&mut p);
        let f = p.frame();
        let row = |y: usize, x: usize, n: usize| f[y * WIDTH + x..y * WIDTH + x + n].to_vec();
        // Object 1 (the smaller X) covers object 0 where both are; OBP1
        // makes its colour 3 shade 2.
        assert_eq!(row(8, 12, 9), [2, 2, 2, 2, 2, 2, 2, 2, 0]);
        // Behind the background: colour 0 everywhere, so it shows.
        assert_eq!(row(24, 0, 2), [3, 3]);
        // Flipped: the right half.
        assert_eq!(row(40, 0, 8), [0, 0, 0, 0, 1, 1, 1, 1]);
        // 8 × 16: tile 2 on top, tile 3 (empty) below; flipped vertically
        // the rows swap.
        p.write(0xff40, 0x97);
        p.oam[..16].fill(0);
        p.oam[..4].copy_from_slice(&[16, 8, 3, 0x40]);
        frame(&mut p);
        let f = p.frame();
        assert_eq!(f[0], 0);
        assert_eq!(f[8 * WIDTH], 1);
        // More than ten on a line: the first ten.
        for i in 0..12 {
            p.oam[i * 4..i * 4 + 4].copy_from_slice(&[16, 8 + 8 * i as u8, 1, 0]);
        }
        p.write(0xff40, 0x93);
        frame(&mut p);
        let f = p.frame();
        assert_eq!((f[9 * 8], f[10 * 8]), (3, 0));
    }

    #[test]
    fn the_display_off_and_the_savestate() {
        let mut p = on();
        tiles(&mut p);
        p.vram[0x1800] = 1;
        frame(&mut p);
        for _ in 0..3000 {
            p.tick();
        }
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = GbPpu::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, p);
        assert!(GbPpu::new()
            .load(&mut StateReader::new(&bytes[..bytes.len() - 9]))
            .is_err());
        p.write(0xff40, 0x11);
        assert_eq!((p.ly, p.read(0xff41) & 3), (0, 0));
        assert!(p.vram_free() && p.oam_free());
        assert!(p.frame().iter().all(|s| *s == 0));
    }
}
