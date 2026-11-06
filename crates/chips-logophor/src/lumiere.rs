//! Lumière, the Logophor 16's video chip (SPEC.md section 8): registers
//! at FFFF:0800, the palette at FFFF:1000, and the picture from chip RAM.
//!
//! Built: modes 0 and 1 (320 × 240, 8-bit indexed or 16-bit RGB 5:6:5,
//! pixels doubled and centred), modes 2 and 3 (800 × 600, 4 or 8 bits
//! indexed; mode 2's high nibble is the left pixel; step 11), mode 4
//! (100 × 37 text, 8 × 16 font from chip RAM, blink and cursor) and the
//! 256 sprites (SPEC.md 8.3). In mode 1 the border colour is RGB 5:6:5 too,
//! FGINDEX has no effect, and a sprite "behind" shows where the
//! background is black.
//!
//! Sprites: at the start of the horizontal blank (the fixed DMA's start) the
//! machine calls [`Lumiere::select_sprites`] for the next line: up to 32
//! active sprites covering it, lowest numbers first, have their row
//! fetched into a line buffer (one slot per 32-bit word of the row, part
//! of the fixed DMA), lower numbers on top; sprite–sprite collisions are
//! found there. Putting a pixel out lays the buffer over the background:
//! palette indices ≥ FGINDEX stay in front, a sprite "behind" shows only
//! where the background is 0. Coordinates are mode pixels (modes 0/1:
//! 320 × 240, the others 800 × 600), offset by 64. **[S]** A sprite
//! touches the "foreground" (SPRCOLFG) over a background index ≥ FGINDEX,
//! or ≠ 0 while FGINDEX is 0; sprites show only inside the mode's picture
//! area.
//!
//! The picture is put out up to the beam ([`Lumiere::draw_to`]) before
//! anything changes a register, so a write in the middle of a line shows
//! from that pixel on (the Jacquard's colour splits). The framebuffer pointer
//! is taken over at the VBlank, or with a write to FBNOW from the next
//! line (split screens); from there the rows count anew (`row_addr`,
//! `row_line`). Without framebuffer DMA (DMACON bit 0) the screen shows
//! the border colour.
//!
//! Blink and cursor alternate every 32 fields (about 0.53 s) **[S]**. All
//! registers are 0 after reset (display off, border palette entry 0).

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// Picture size (SPEC.md 2): SVGA, modes 0-4.
pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 600;
/// Lines of an SVGA field.
pub const LINES: u16 = 628;
/// Mode 5's XGA picture (SPEC.md 8.1) and its field's lines.
pub const XGA_WIDTH: u32 = 1024;
pub const XGA_HEIGHT: u32 = 768;
pub const XGA_LINES: u16 = 806;

/// Register offsets in segment FFFF.
pub const VMODE: u16 = 0x0800;
pub const FBSEG: u16 = 0x0802;
pub const FBOFS: u16 = 0x0804;
pub const PITCH: u16 = 0x0806;
pub const FINEX: u16 = 0x0808;
pub const BORDER: u16 = 0x080a;
pub const FGINDEX: u16 = 0x080c;
pub const TXTBANK: u16 = 0x080e;
pub const FONTSEG: u16 = 0x0810;
pub const FONTOFS: u16 = 0x0812;
pub const CURSOR: u16 = 0x0814;
pub const CURSHAPE: u16 = 0x0816;
pub const BEAMY: u16 = 0x0818;
pub const BEAMX: u16 = 0x081a;
pub const VSTAT: u16 = 0x081c;
pub const FBNOW: u16 = 0x081e;
pub const SPRSEG: u16 = 0x0820;
pub const SPROFS: u16 = 0x0822;
pub const SPRCOLFG: u16 = 0x0824;
pub const SPRCOL: u16 = 0x0840;
pub const SPRITES: u16 = 0x2000;

/// Sprite sizes (SPEC.md 8.3).
const SPRITE_WIDTHS: [u16; 4] = [8, 16, 32, 0];
const SPRITE_HEIGHTS: [u16; 4] = [8, 16, 32, 64];
/// Sprites a line at most.
const SPRITES_PER_LINE: usize = 32;
/// A line buffer entry: colour (bits 7–0), behind (bit 8), group (12–9).
const BEHIND: u16 = 1 << 8;
pub const PALETTE: u16 = 0x1000;

/// Fields per blink phase.
const BLINK_FIELDS: u64 = 32;

/// Chip RAM as the video chip sees it (linear addresses).
pub trait ChipRam {
    fn chip_byte(&self, addr: u32) -> u8;

    /// The `len` bytes from `addr` on, when they lie in chip RAM in one
    /// piece (a fast path for whole rows; `None`: byte by byte).
    fn chip_bytes(&self, _addr: u32, _len: usize) -> Option<&[u8]> {
        None
    }
}

/// The beam: line `y` (0–627), pixel `x` (0–1059).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beam {
    pub x: u16,
    pub y: u16,
}

impl Beam {
    fn vblank(self, height: u32) -> bool {
        u32::from(self.y) >= height
    }
    fn hblank(self, width: u32) -> bool {
        u32::from(self.x) >= width
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lumiere {
    vmode: u16,
    /// This field runs with XGA timing (mode 5 at its start).
    xga: bool,
    /// Written framebuffer pointer, and the one taken over.
    fb: (u16, u16),
    fb_active: (u16, u16),
    pitch: u16,
    finex: u16,
    border: u16,
    fgindex: u16,
    txtbank: u16,
    font: (u16, u16),
    cursor: u16,
    curshape: u16,
    /// VSTAT bit 2: pointer taken over (cleared on read).
    taken_over: bool,
    palette: [u16; 256],
    /// The palette as RGB (follows `palette`; not machine state).
    rgb: [Rgb8; 256],
    fields: u64,
    /// Framebuffer DMA on (Babbage's DMACON bit 0).
    dma: bool,
    /// FBNOW was written: take the pointer over at the end of the line.
    now_pending: bool,
    /// The current row's address and the line in it.
    row_addr: u32,
    row_line: u16,
    /// The line being put out and how far (pixels).
    line: u16,
    drawn: u16,
    /// Sprites: image area, table (256 × 4 words), DMA on (DMACON bit 1),
    /// collisions, and the line buffers of this and the next line (mode
    /// pixels; 0 = no sprite pixel).
    spr_base: (u16, u16),
    sprites: Vec<u16>,
    sprite_dma: bool,
    col: [u16; 16],
    colfg: u16,
    line_cur: Vec<u16>,
    line_next: Vec<u16>,
    pixels: Vec<Rgb8>,
}

impl Default for Lumiere {
    fn default() -> Self {
        Self {
            vmode: 0,
            xga: false,
            fb: (0, 0),
            fb_active: (0, 0),
            pitch: 0,
            finex: 0,
            border: 0,
            fgindex: 0,
            txtbank: 0,
            font: (0, 0),
            cursor: 0,
            curshape: 0,
            taken_over: false,
            palette: [0; 256],
            rgb: [rgb565(0); 256],
            fields: 0,
            dma: false,
            now_pending: false,
            row_addr: 0,
            row_line: 0,
            line: 0,
            drawn: 0,
            spr_base: (0, 0),
            sprites: vec![0; 1024],
            sprite_dma: false,
            col: [0; 16],
            colfg: 0,
            line_cur: vec![0; XGA_WIDTH as usize],
            line_next: vec![0; XGA_WIDTH as usize],
            pixels: vec![Rgb8::default(); (XGA_WIDTH * XGA_HEIGHT) as usize],
        }
    }
}

/// RGB 5:6:5 to 8 bits a channel.
pub fn rgb565(c: u16) -> Rgb8 {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    Rgb8 {
        r: (r << 3 | r >> 2) as u8,
        g: (g << 2 | g >> 4) as u8,
        b: (b << 3 | b >> 2) as u8,
    }
}

/// A mode's picture area: first line, lines, lines a row (one fetched
/// row of bytes or characters).
fn area(mode: u16) -> Option<(u16, u16, u16)> {
    match mode {
        0 | 1 => Some((60, 480, 2)),
        2 | 3 => Some((0, 600, 1)),
        5 => Some((0, 768, 1)),
        4 => Some((4, 592, 16)),
        _ => None,
    }
}

impl Lumiere {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset: every register 0.
    pub fn reset(&mut self) {
        let pixels = std::mem::take(&mut self.pixels);
        *self = Self {
            pixels,
            ..Self::default()
        };
    }

    pub fn pixels(&self) -> &[Rgb8] {
        &self.pixels
    }

    /// Fields since reset.
    pub fn fields(&self) -> u64 {
        self.fields
    }

    pub fn mode(&self) -> u16 {
        self.vmode & 7
    }

    /// VMODE bit 13: the monitor shows the picture on its green phosphor
    /// tube (SPEC.md 8.2): the frame is reported as monochrome.
    pub fn green_tube(&self) -> bool {
        self.vmode & 0x2000 != 0
    }

    pub fn display_on(&self) -> bool {
        self.vmode & 0x8000 != 0
    }

    /// This field runs with XGA timing (mode 5 when it began).
    pub fn xga(&self) -> bool {
        self.xga
    }

    /// This field's picture: width, height (pixels) and lines.
    pub fn width(&self) -> u32 {
        if self.xga {
            XGA_WIDTH
        } else {
            WIDTH
        }
    }

    pub fn height(&self) -> u32 {
        if self.xga {
            XGA_HEIGHT
        } else {
            HEIGHT
        }
    }

    pub fn lines(&self) -> u16 {
        if self.xga {
            XGA_LINES
        } else {
            LINES
        }
    }

    pub fn palette(&self) -> &[u16; 256] {
        &self.palette
    }

    /// Framebuffer DMA on or off (Babbage's DMACON).
    pub fn set_dma(&mut self, on: bool) {
        self.dma = on;
    }

    /// Sprite DMA on or off (Babbage's DMACON bit 1).
    pub fn set_sprite_dma(&mut self, on: bool) {
        self.sprite_dma = on;
    }

    /// The mode line of picture line `y` and the mode's width, when `y`
    /// is in its picture area.
    fn mode_line(&self, y: u16) -> Option<(u16, usize)> {
        let (top, lines, _) = area(self.mode())?;
        if !(top..top + lines).contains(&y) {
            return None;
        }
        Some(match self.mode() {
            0 | 1 => ((y - top) / 2, 320),
            5 => (y, XGA_WIDTH as usize),
            _ => (y, WIDTH as usize),
        })
    }

    /// Choose and fetch the sprites of picture line `y` into the next
    /// line buffer; returns the chip-bus slots it took.
    pub fn select_sprites(&mut self, y: u16, mem: &dyn ChipRam) -> u16 {
        self.line_next.fill(0);
        if !self.sprite_dma || !self.display_on() {
            return 0;
        }
        let Some((ml, width)) = self.mode_line(y) else {
            return 0;
        };
        let ml = i32::from(ml);
        let mut chosen = Vec::with_capacity(SPRITES_PER_LINE);
        for n in 0..256 {
            let (w0, w1) = (self.sprites[4 * n], self.sprites[4 * n + 1]);
            if w0 & 0x8000 == 0 {
                continue;
            }
            let top = i32::from(w1 & 0x3ff) - 64;
            let h = i32::from(SPRITE_HEIGHTS[usize::from(w1 >> 10 & 3)]);
            if (top..top + h).contains(&ml) {
                chosen.push(n);
                if chosen.len() == SPRITES_PER_LINE {
                    break;
                }
            }
        }
        let base = u32::from(self.spr_base.0) << 16 | u32::from(self.spr_base.1);
        let mut slots = 0;
        // The highest number first, so lower numbers end on top.
        for &n in chosen.iter().rev() {
            let [w0, w1, w2, w3] = [0, 1, 2, 3].map(|k| self.sprites[4 * n + k]);
            let w = SPRITE_WIDTHS[usize::from(w0 >> 11 & 3)];
            if w == 0 {
                continue;
            }
            let eight = w3 & 1 != 0;
            let bytes = if eight { w } else { w / 2 };
            let h = i32::from(SPRITE_HEIGHTS[usize::from(w1 >> 10 & 3)]);
            let mut row = ml - (i32::from(w1 & 0x3ff) - 64);
            if w0 & 0x4000 != 0 {
                row = h - 1 - row;
            }
            let addr = base
                .wrapping_add(u32::from(w2) * 16)
                .wrapping_add(row as u32 * u32::from(bytes));
            slots += ((addr & 3) + u32::from(bytes)).div_ceil(4) as u16;
            let left = i32::from(w0 & 0x7ff) - 64;
            let (behind, group) = (if w1 & 0x1000 != 0 { BEHIND } else { 0 }, w3 >> 8 & 15);
            let bank = (w3 >> 4 & 15) as u8;
            for px in 0..i32::from(w) {
                let sx = left + px;
                if sx < 0 || sx as usize >= width {
                    continue;
                }
                let src = if w0 & 0x2000 != 0 {
                    i32::from(w) - 1 - px
                } else {
                    px
                } as u32;
                let v = if eight {
                    mem.chip_byte(addr.wrapping_add(src))
                } else {
                    let b = mem.chip_byte(addr.wrapping_add(src / 2));
                    let nib = if src.is_multiple_of(2) {
                        b >> 4
                    } else {
                        b & 15
                    };
                    if nib == 0 {
                        0
                    } else {
                        bank << 4 | nib
                    }
                };
                if v == 0 {
                    continue;
                }
                let old = self.line_next[sx as usize];
                let g2 = old >> 9 & 15;
                if old != 0 && group != 0 && g2 != 0 {
                    self.col[usize::from(group)] |= 1 << g2;
                    self.col[usize::from(g2)] |= 1 << group;
                }
                self.line_next[sx as usize] = u16::from(v) | behind | group << 9;
            }
        }
        slots
    }

    /// A background pixel of palette index `bg` with the sprites over it
    /// at mode pixel `mx`.
    fn over(&mut self, bg: u8, mx: usize) -> Rgb8 {
        let s = self.line_cur.get(mx).copied().unwrap_or(0);
        if s == 0 {
            return self.colour(bg);
        }
        let fg = if self.fgindex != 0 {
            u16::from(bg) >= self.fgindex
        } else {
            bg != 0
        };
        let group = s >> 9 & 15;
        if fg && group != 0 {
            self.colfg |= 1 << group;
        }
        if self.fgindex != 0 && u16::from(bg) >= self.fgindex || s & BEHIND != 0 && bg != 0 {
            self.colour(bg)
        } else {
            self.colour(s as u8)
        }
    }

    /// Register read without side effects.
    pub fn peek(&self, off: u16, beam: Beam) -> u16 {
        let off = off & !1;
        match off {
            VMODE => self.vmode,
            FBSEG => self.fb.0,
            FBOFS => self.fb.1,
            PITCH => self.pitch,
            FINEX => self.finex,
            BORDER => self.border,
            FGINDEX => self.fgindex,
            TXTBANK => self.txtbank,
            FONTSEG => self.font.0,
            FONTOFS => self.font.1,
            CURSOR => self.cursor,
            CURSHAPE => self.curshape,
            BEAMY => beam.y,
            BEAMX => beam.x,
            VSTAT => {
                u16::from(beam.vblank(self.height()))
                    | u16::from(beam.hblank(self.width())) << 1
                    | u16::from(self.taken_over) << 2
            }
            PALETTE..=0x11fe => self.palette[usize::from((off - PALETTE) / 2)],
            SPRSEG => self.spr_base.0,
            SPROFS => self.spr_base.1,
            SPRCOLFG => self.colfg,
            SPRCOL..=0x085e => self.col[usize::from((off - SPRCOL) / 2)],
            SPRITES..=0x27fe => self.sprites[usize::from((off - SPRITES) / 2)],
            _ => 0,
        }
    }

    /// Register read (VSTAT bit 2 and the collision registers clear).
    pub fn read(&mut self, off: u16, beam: Beam) -> u16 {
        let v = self.peek(off, beam);
        match off & !1 {
            VSTAT => self.taken_over = false,
            SPRCOLFG => self.colfg = 0,
            SPRCOL..=0x085e => self.col[usize::from((off & !1) - SPRCOL) / 2] = 0,
            _ => {}
        }
        v
    }

    /// Register write. The machine puts the picture out up to the beam
    /// first ([`Lumiere::draw_to`]).
    pub fn write(&mut self, off: u16, v: u16) {
        let off = off & !1;
        match off {
            VMODE => self.vmode = v & 0xa007,
            FBSEG => self.fb.0 = v,
            FBOFS => self.fb.1 = v,
            PITCH => self.pitch = v,
            FINEX => self.finex = v & 1,
            BORDER => self.border = v,
            FGINDEX => self.fgindex = v & 0xff,
            TXTBANK => self.txtbank = v & 0xf0,
            FONTSEG => self.font.0 = v,
            FONTOFS => self.font.1 = v,
            CURSOR => self.cursor = v,
            CURSHAPE => self.curshape = v & 0xff,
            FBNOW => self.now_pending = true,
            PALETTE..=0x11fe => {
                let i = usize::from((off - PALETTE) / 2);
                self.palette[i] = v;
                self.rgb[i] = rgb565(v);
            }
            SPRSEG => self.spr_base.0 = v,
            SPROFS => self.spr_base.1 = v,
            SPRITES..=0x27fe => self.sprites[usize::from((off - SPRITES) / 2)] = v,
            _ => {}
        }
    }

    fn take_over(&mut self) {
        self.fb_active = self.fb;
        self.row_addr = u32::from(self.fb.0) << 16 | u32::from(self.fb.1);
        self.row_line = 0;
    }

    /// The VBlank starts (line 600): the framebuffer pointer is taken over
    /// (VSTAT bit 2; an FBNOW take-over does not set it).
    pub fn vblank(&mut self) {
        self.take_over();
        self.taken_over = true;
        self.fields += 1;
    }

    /// Chip-bus slots the framebuffer fetch for picture line `y` takes
    /// (SPEC.md 5): the fixed DMA of the line before.
    pub fn fetch_slots(&self, y: u16) -> u16 {
        if !self.dma || !self.display_on() {
            return 0;
        }
        let Some((top, lines, per_row)) = area(self.mode()) else {
            return 0;
        };
        if !(top..top + lines).contains(&y) {
            return 0;
        }
        let first_of_row = (y - top).is_multiple_of(per_row);
        match self.mode() {
            0 => u16::from(first_of_row) * 80,
            1 => u16::from(first_of_row) * 160,
            2 => 100,
            3 => 200,
            5 => 256,
            _ => u16::from(first_of_row) * 50 + 25,
        }
    }

    fn colour(&self, index: u8) -> Rgb8 {
        self.rgb[usize::from(index)]
    }

    /// No sprite pixel in mode pixels `from..to` of the line being put
    /// out (then a background pixel is just its colour).
    fn no_sprites(&self, from: usize, to: usize) -> bool {
        self.line_cur
            .get(from..to)
            .is_some_and(|s| s.iter().all(|&v| v == 0))
    }

    /// A mode-1 pixel of RGB 5:6:5 `bg` with the sprites over it.
    fn over16(&mut self, bg: u16, mx: usize) -> Rgb8 {
        let s = self.line_cur.get(mx).copied().unwrap_or(0);
        if s == 0 || s & BEHIND != 0 && bg != 0 {
            rgb565(bg)
        } else {
            self.colour(s as u8)
        }
    }

    fn blink_on(&self) -> bool {
        (self.fields / BLINK_FIELDS).is_multiple_of(2)
    }

    /// Put out line `y` up to pixel `x` (exclusive) with the registers as
    /// they are now.
    pub fn draw_to(&mut self, y: u16, x: u16, mem: &dyn ChipRam) {
        if y != self.line || u32::from(y) >= self.height() {
            return;
        }
        let end = x.min(self.width() as u16);
        if end <= self.drawn {
            return;
        }
        let mut px = std::mem::take(&mut self.pixels);
        let base = usize::from(y) * self.width() as usize;
        let span = &mut px[base + usize::from(self.drawn)..base + usize::from(end)];
        let from = self.drawn;
        self.span(y, from, span, mem);
        self.pixels = px;
        self.drawn = end;
    }

    /// The end of line `y`: put the rest out, step the row, take an FBNOW
    /// pointer over.
    pub fn end_line(&mut self, y: u16, mem: &dyn ChipRam) {
        self.draw_to(y, self.width() as u16, mem);
        if let Some((top, lines, per_row)) = area(self.mode()) {
            if self.display_on() && (top..top + lines).contains(&y) {
                self.row_line += 1;
                if self.row_line >= per_row {
                    self.row_line = 0;
                    self.row_addr = self.row_addr.wrapping_add(u32::from(self.pitch));
                }
            }
        }
        if self.now_pending {
            self.now_pending = false;
            self.take_over();
        }
        // The sprites chosen in this line's blank show on the next.
        std::mem::swap(&mut self.line_cur, &mut self.line_next);
        self.line_next.fill(0);
        self.line = if y + 1 >= self.lines() {
            // a new field: its timing from the mode now
            self.xga = self.mode() == 5;
            0
        } else {
            y + 1
        };
        self.drawn = 0;
    }

    /// Pixels `from`.. of line `y` into `out`, the registers as they are.
    fn span(&mut self, y: u16, from: u16, out: &mut [Rgb8], mem: &dyn ChipRam) {
        let border = if self.mode() == 1 {
            rgb565(self.border)
        } else {
            self.colour(self.border as u8)
        };
        let area = area(self.mode()).filter(|(top, lines, _)| (*top..top + lines).contains(&y));
        let Some((top, _, per_row)) = area.filter(|_| self.display_on() && self.dma) else {
            out.fill(border);
            return;
        };
        let line = self.row_line.min(per_row - 1);
        if self.span_fast(from, out, mem) {
            return;
        }
        match self.mode() {
            0 => {
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i as u16;
                    *p = if (80..720).contains(&x) {
                        let mx = usize::from((x - 80) / 2);
                        let at = self.row_addr.wrapping_add(mx as u32);
                        self.over(mem.chip_byte(at), mx)
                    } else {
                        border
                    };
                }
            }
            1 => {
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i as u16;
                    *p = if (80..720).contains(&x) {
                        let mx = usize::from((x - 80) / 2);
                        let at = self.row_addr.wrapping_add(2 * mx as u32);
                        let c = u16::from_le_bytes([
                            mem.chip_byte(at),
                            mem.chip_byte(at.wrapping_add(1)),
                        ]);
                        self.over16(c, mx)
                    } else {
                        border
                    };
                }
            }
            2 => {
                // 4 bits a pixel, the high nibble the left one.
                for (i, p) in out.iter_mut().enumerate() {
                    let x = usize::from(from) + i;
                    let b = mem.chip_byte(self.row_addr.wrapping_add((x / 2) as u32));
                    let index = if x % 2 == 0 { b >> 4 } else { b & 15 };
                    *p = self.over(index, x);
                }
            }
            3 | 5 => {
                for (i, p) in out.iter_mut().enumerate() {
                    let x = usize::from(from) + i;
                    let index = mem.chip_byte(self.row_addr.wrapping_add(x as u32));
                    *p = self.over(index, x);
                }
            }
            4 => {
                let font = u32::from(self.font.0) << 16 | u32::from(self.font.1);
                let blink = self.blink_on();
                let (first, last) = (self.curshape & 15, self.curshape >> 4 & 15);
                let cursor_row = u32::from((y - top) / per_row) * 100;
                let bank = self.txtbank as u8;
                // One character cell at a time; without sprite pixels in
                // the span a pixel is just its colour.
                let plain = self.no_sprites(usize::from(from), usize::from(from) + out.len());
                let mut cell = u32::MAX;
                let (mut bits, mut fg, mut bg) = (0u8, 0u8, 0u8);
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i as u16;
                    let col = u32::from(x / 8);
                    if col != cell {
                        cell = col;
                        let at = self.row_addr.wrapping_add(2 * col);
                        let (ch, attr) = (mem.chip_byte(at), mem.chip_byte(at.wrapping_add(1)));
                        bits =
                            mem.chip_byte(font.wrapping_add(u32::from(ch) * 16 + u32::from(line)));
                        if attr & 0x80 != 0 && !blink {
                            bits = 0;
                        }
                        if cursor_row + col == u32::from(self.cursor)
                            && blink
                            && (first..=last).contains(&line)
                        {
                            bits = !bits;
                        }
                        fg = bank | attr & 15;
                        bg = bank | attr >> 4 & 7;
                    }
                    let index = if bits & (0x80 >> (x % 8)) != 0 {
                        fg
                    } else {
                        bg
                    };
                    *p = if plain {
                        self.rgb[usize::from(index)]
                    } else {
                        self.over(index, usize::from(x))
                    };
                }
            }
            _ => out.fill(border),
        }
    }

    /// [`Lumiere::span`] for the bitmap modes when the pixels' bytes lie in
    /// chip RAM in one piece: the same pixels, with the bytes taken from
    /// one slice instead of one by one through `mem`, and without looking
    /// at the sprites where the line has none; `false` when it does not
    /// apply.
    fn span_fast(&mut self, from: u16, out: &mut [Rgb8], mem: &dyn ChipRam) -> bool {
        let (from, n) = (usize::from(from), out.len());
        let to = from + n;
        match self.mode() {
            mode @ (0 | 1) => {
                // pixels 80..720 show mode pixels (x - 80) / 2, the rest
                // is border
                let (x0, x1) = (from.clamp(80, 720), to.clamp(80, 720));
                if x0 >= x1 {
                    return false;
                }
                let (m0, m1) = ((x0 - 80) / 2, (x1 - 81) / 2 + 1);
                let wide = usize::from(mode) + 1;
                let Some(row) = mem.chip_bytes(
                    self.row_addr.wrapping_add((wide * m0) as u32),
                    wide * (m1 - m0),
                ) else {
                    return false;
                };
                let border = if mode == 1 {
                    rgb565(self.border)
                } else {
                    self.colour(self.border as u8)
                };
                let plain = self.no_sprites(m0, m1);
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i;
                    *p = if (x0..x1).contains(&x) {
                        let mx = (x - 80) / 2;
                        let k = mx - m0;
                        match (mode, plain) {
                            (0, true) => self.rgb[usize::from(row[k])],
                            (0, false) => self.over(row[k], mx),
                            (_, true) => rgb565(u16::from_le_bytes([row[2 * k], row[2 * k + 1]])),
                            _ => self.over16(u16::from_le_bytes([row[2 * k], row[2 * k + 1]]), mx),
                        }
                    } else {
                        border
                    };
                }
                true
            }
            2 => {
                // 4 bits a pixel, the high nibble the left one
                let (b0, b1) = (from / 2, (to - 1) / 2 + 1);
                let Some(row) = mem.chip_bytes(self.row_addr.wrapping_add(b0 as u32), b1 - b0)
                else {
                    return false;
                };
                let plain = self.no_sprites(from, to);
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i;
                    let b = row[x / 2 - b0];
                    let index = if x % 2 == 0 { b >> 4 } else { b & 15 };
                    *p = if plain {
                        self.rgb[usize::from(index)]
                    } else {
                        self.over(index, x)
                    };
                }
                true
            }
            3 | 5 => {
                let Some(row) = mem.chip_bytes(self.row_addr.wrapping_add(from as u32), n) else {
                    return false;
                };
                if self.no_sprites(from, to) {
                    for (p, &b) in out.iter_mut().zip(row) {
                        *p = self.rgb[usize::from(b)];
                    }
                } else {
                    for (i, (p, &b)) in out.iter_mut().zip(row).enumerate() {
                        *p = self.over(b, from + i);
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"LUMI", 4, |w| {
            for v in [
                self.vmode,
                self.fb.0,
                self.fb.1,
                self.fb_active.0,
                self.fb_active.1,
                self.pitch,
                self.finex,
                self.border,
                self.fgindex,
                self.txtbank,
                self.font.0,
                self.font.1,
                self.cursor,
                self.curshape,
            ] {
                w.u16(v);
            }
            w.bool(self.taken_over);
            for p in self.palette {
                w.u16(p);
            }
            w.u64(self.fields);
            w.bool(self.dma);
            w.bool(self.now_pending);
            w.u32(self.row_addr);
            w.u16(self.row_line);
            w.u16(self.line);
            w.u16(self.drawn);
            w.u16(self.spr_base.0);
            w.u16(self.spr_base.1);
            w.bool(self.sprite_dma);
            for v in self
                .sprites
                .iter()
                .chain(&self.col)
                .chain(&self.line_cur)
                .chain(&self.line_next)
            {
                w.u16(*v);
            }
            w.u16(self.colfg);
            w.bool(self.xga);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"LUMI", 4)?;
        let mut n = Self {
            vmode: c.u16()? & 0xa007,
            xga: false,
            fb: (c.u16()?, c.u16()?),
            fb_active: (c.u16()?, c.u16()?),
            pitch: c.u16()?,
            finex: c.u16()? & 1,
            border: c.u16()?,
            fgindex: c.u16()? & 0xff,
            txtbank: c.u16()? & 0xf0,
            font: (c.u16()?, c.u16()?),
            cursor: c.u16()?,
            curshape: c.u16()? & 0xff,
            taken_over: c.bool()?,
            palette: [0; 256],
            rgb: [rgb565(0); 256],
            fields: 0,
            dma: false,
            now_pending: false,
            row_addr: 0,
            row_line: 0,
            line: 0,
            drawn: 0,
            spr_base: (0, 0),
            sprites: vec![0; 1024],
            sprite_dma: false,
            col: [0; 16],
            colfg: 0,
            line_cur: vec![0; XGA_WIDTH as usize],
            line_next: vec![0; XGA_WIDTH as usize],
            pixels: Vec::new(),
        };
        for p in &mut n.palette {
            *p = c.u16()?;
        }
        n.rgb = n.palette.map(rgb565);
        n.fields = c.u64()?;
        n.dma = c.bool()?;
        n.now_pending = c.bool()?;
        n.row_addr = c.u32()?;
        n.row_line = c.u16()? & 15;
        n.line = c.u16()?.min(XGA_LINES - 1);
        n.drawn = c.u16()?.min(XGA_WIDTH as u16);
        n.spr_base = (c.u16()?, c.u16()?);
        n.sprite_dma = c.bool()?;
        for v in n
            .sprites
            .iter_mut()
            .chain(&mut n.col)
            .chain(&mut n.line_cur)
            .chain(&mut n.line_next)
        {
            *v = c.u16()?;
        }
        n.colfg = c.u16()?;
        n.xga = c.bool()?;
        // Only now, after everything loaded: the picture stays.
        n.pixels = std::mem::take(&mut self.pixels);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    #[test]
    fn rgb565_expands_to_full_range() {
        assert_eq!(
            rgb565(0xffff),
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(rgb565(0xf800), Rgb8 { r: 255, g: 0, b: 0 });
        assert_eq!(rgb565(0x07e0), Rgb8 { r: 0, g: 255, b: 0 });
        assert_eq!(rgb565(0x0015), Rgb8 { r: 0, g: 0, b: 173 });
    }

    #[test]
    fn registers_vstat_and_the_pointer_latch() {
        let mut l = Lumiere::new();
        l.write(VMODE, 0xffff);
        assert_eq!(l.peek(VMODE, Beam::default()), 0xa007, "unused bits");
        l.write(FBSEG, 2);
        l.write(PALETTE + 2 * 255, 0x1234);
        assert_eq!(l.peek(PALETTE + 510, Beam::default()), 0x1234);
        assert_eq!(l.fb_active, (0, 0), "not before the VBlank");
        let beam = Beam { x: 900, y: 610 };
        assert_eq!(l.peek(VSTAT, beam), 0b011);
        l.vblank();
        assert_eq!(l.fb_active, (2, 0));
        assert_eq!(l.read(VSTAT, beam), 0b111);
        assert_eq!(l.read(VSTAT, beam), 0b011, "bit 2 clears on read");
        assert_eq!((l.peek(BEAMX, beam), l.peek(BEAMY, beam)), (900, 610));
        l.reset();
        assert_eq!(l.peek(VMODE, beam), 0);
    }

    #[test]
    fn a_write_mid_line_shows_from_its_pixel_and_fbnow_restarts_the_rows() {
        let ram = Ram((0..=255u8).cycle().take(0x2_0000).collect());
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.write(VMODE, 0x8000);
        l.write(PITCH, 320);
        l.write(PALETTE + 2 * 7, 0xffff);
        l.write(BORDER, 7);
        l.vblank();
        for y in 0..100 {
            l.end_line(y, &ram);
        }
        // Line 100: border white up to x 40, then palette 7 changed to red.
        l.draw_to(100, 40, &ram);
        l.write(PALETTE + 2 * 7, 0xf800);
        l.end_line(100, &ram);
        let p = |x: usize| l.pixels()[100 * 800 + x];
        assert_eq!((p(39), p(40)), (rgb565(0xffff), rgb565(0xf800)));
        // Row (100 − 60) / 2 = 20 of the picture: byte 20 × 320 = 6400.
        assert_eq!(p(80), rgb565(l.palette[(6400 % 256) as usize]));
        // FBNOW at line 101: line 102 shows row 0 of the new pointer.
        l.write(FBSEG, 1);
        l.write(FBNOW, 0);
        l.end_line(101, &ram);
        l.end_line(102, &ram);
        assert_eq!(
            l.pixels()[102 * 800 + 82],
            rgb565(l.palette[(0x1_0001 % 256) as usize])
        );
        assert_eq!(l.fetch_slots(103), 0, "an odd picture line fetches nothing");
        assert_eq!(l.fetch_slots(104), 80);
        l.set_dma(false);
        assert_eq!(l.fetch_slots(104), 0);
    }
}

#[cfg(test)]
mod sprite_tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    /// Mode 0 over a zero framebuffer at 0001:0000 (mode-0 row 1 holds
    /// colour 9 everywhere), palette entry i = i, sprites from 0002:0000.
    fn setup() -> (Lumiere, Ram) {
        let mut ram = vec![0; 0x3_0000];
        ram[0x1_0000 + 320..0x1_0000 + 640].fill(9);
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.set_sprite_dma(true);
        l.write(VMODE, 0x8000);
        l.write(FBSEG, 1);
        l.write(PITCH, 320);
        l.write(SPRSEG, 2);
        for i in 0..256u16 {
            l.write(PALETTE + 2 * i, i);
        }
        l.vblank();
        (l, Ram(ram))
    }

    fn sprite(l: &mut Lumiere, n: u16, words: [u16; 4]) {
        for (k, w) in words.iter().enumerate() {
            l.write(SPRITES + 8 * n + 2 * k as u16, *w);
        }
    }

    /// Run lines 0..`upto` with the sprite choice in each blank; the
    /// slots of the last choice.
    fn run(l: &mut Lumiere, ram: &Ram, upto: u16) -> u16 {
        let mut slots = 0;
        for y in 0..upto {
            slots = l.select_sprites(y + 1, ram);
            l.end_line(y, ram);
        }
        slots
    }

    /// The palette index shown at mode pixel (mx, my) of mode 0.
    fn at(l: &Lumiere, mx: u16, my: u16) -> u16 {
        let p = l.pixels()[usize::from(60 + 2 * my) * 800 + usize::from(80 + 2 * mx)];
        (0..256u16).find(|i| rgb565(*i) == p).unwrap_or(0xffff)
    }

    #[test]
    fn priority_transparency_and_flip() {
        let (mut l, mut ram) = setup();
        // Image 0 (8 x 8, 8 bit): colour 3, its first column 0 (clear).
        for row in 0..8 {
            ram.0[0x2_0000 + row * 8..0x2_0000 + row * 8 + 8]
                .copy_from_slice(&[0, 3, 3, 3, 3, 3, 3, 3]);
        }
        // Image at offset 4 (64 bytes on): colour 5 everywhere.
        ram.0[0x2_0040..0x2_0080].fill(5);
        sprite(&mut l, 0, [0x8000 | (64 + 10), 64 + 20, 0, 1]);
        sprite(&mut l, 1, [0x8000 | (64 + 12), 64 + 20, 4, 1]);
        // Sprite 2 flipped in X: its clear column on the right.
        sprite(&mut l, 2, [0xa000 | (64 + 40), 64 + 20, 0, 1]);
        run(&mut l, &ram, 110);
        assert_eq!(at(&l, 10, 20), 0, "sprite 0's clear pixel: background");
        assert_eq!(at(&l, 11, 20), 3);
        assert_eq!(at(&l, 13, 20), 3, "sprite 0 on top of sprite 1");
        assert_eq!(at(&l, 18, 20), 5, "sprite 1 beyond sprite 0");
        assert_eq!((at(&l, 40, 20), at(&l, 47, 20)), (3, 0), "flipped");
        assert_eq!(at(&l, 11, 28), 0, "8 lines high");
    }

    #[test]
    fn four_bit_banks_behind_and_fgindex() {
        let (mut l, mut ram) = setup();
        ram.0[0x2_0000..0x2_0020].fill(0x11); // 4 bit, 8 wide: 4 bytes a row
        sprite(&mut l, 0, [0x8000 | (64 + 10), 64, 0, 0x0020]); // bank 2
        sprite(
            &mut l,
            1,
            [0x8000 | (64 + 30), 0x1000 | (64 + 1), 0, 0x0020],
        ); // behind
        run(&mut l, &ram, 70);
        assert_eq!(at(&l, 12, 0), 0x21, "bank 2, pixel 1");
        assert_eq!(at(&l, 32, 1), 9, "behind a background of colour 9");
        assert_eq!(at(&l, 32, 2), 0x21, "over a background of 0");
        // FGINDEX 9: colour 9 in front, and the foreground collision.
        let (mut l, mut ram) = setup();
        ram.0[0x2_0000..0x2_0040].fill(7);
        l.write(FGINDEX, 9);
        sprite(&mut l, 0, [0x8000 | (64 + 10), 64, 0, 0x0301]); // group 3
        run(&mut l, &ram, 70);
        assert_eq!(at(&l, 12, 1), 9);
        assert_eq!(at(&l, 12, 0), 7);
        assert_eq!(l.read(SPRCOLFG, Beam::default()), 1 << 3);
        assert_eq!(l.read(SPRCOLFG, Beam::default()), 0, "cleared on read");
    }

    #[test]
    fn sprite_collisions_and_the_limit_of_32() {
        let (mut l, mut ram) = setup();
        ram.0[0x2_0000..0x2_0040].fill(4);
        sprite(&mut l, 0, [0x8000 | (64 + 10), 64 + 30, 0, 0x0101]); // group 1
        sprite(&mut l, 1, [0x8000 | (64 + 14), 64 + 30, 0, 0x0201]); // group 2
                                                                     // 40 more sprites on line 50, 8 apart: only 32 can show.
        for n in 0..40u16 {
            sprite(&mut l, 2 + n, [0x8000 | (64 + 8 * n), 64 + 50, 0, 1]);
        }
        run(&mut l, &ram, 60 + 2 * 50 + 1);
        assert_eq!(l.read(SPRCOL + 2, Beam::default()), 1 << 2);
        assert_eq!(l.read(SPRCOL + 4, Beam::default()), 1 << 1);
        assert_eq!(l.peek(SPRCOL + 2, Beam::default()), 0, "cleared on read");
        let shown = (0..40u16).filter(|n| at(&l, 8 * n + 1, 50) == 4).count();
        assert_eq!(shown, 32, "the lowest 32 numbers");
        assert_eq!(at(&l, 8 * 31 + 1, 50), 4);
        assert_eq!(at(&l, 8 * 32 + 1, 50), 0);
        // Slots: 32 sprites of 8 bytes, 2 words each.
        let (mut l, ram) = (l, ram);
        assert_eq!(l.select_sprites(60 + 2 * 50, &ram), 64);
        l.set_sprite_dma(false);
        assert_eq!(l.select_sprites(60 + 2 * 50, &ram), 0);
    }
}

#[cfg(test)]
mod mode1_tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    #[test]
    fn mode_1_shows_rgb565_with_sprites_and_border() {
        let mut ram = vec![0; 0x3_0000];
        // Mode-1 row 1: red everywhere; sprite images at 0002:0000.
        for x in 0..320 {
            ram[0x1_0000 + 640 + 2 * x..][..2].copy_from_slice(&0xf800u16.to_le_bytes());
        }
        ram[0x2_0000..0x2_0040].fill(5);
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.set_sprite_dma(true);
        l.write(VMODE, 0x8001);
        l.write(FBSEG, 1);
        l.write(PITCH, 640);
        l.write(BORDER, 0x07e0);
        l.write(SPRSEG, 2);
        l.write(PALETTE + 10, 0x001f);
        // Sprite 0 in front at (10, 0), sprite 1 behind at (30, 0).
        for (n, w) in [
            [0x8000 | (64 + 10), 64, 0, 1],
            [0x8000 | (64 + 30), 0x1000 | 64, 0, 1],
        ]
        .iter()
        .enumerate()
        {
            for (k, v) in w.iter().enumerate() {
                l.write(SPRITES + 8 * n as u16 + 2 * k as u16, *v);
            }
        }
        l.vblank();
        let ram = Ram(ram);
        for y in 0..70 {
            l.select_sprites(y + 1, &ram);
            l.end_line(y, &ram);
        }
        let at = |mx: usize, my: usize| l.pixels()[(60 + 2 * my) * 800 + 80 + 2 * mx];
        assert_eq!(at(0, 1), rgb565(0xf800));
        assert_eq!(at(0, 2), rgb565(0));
        assert_eq!(
            l.pixels()[60 * 800 + 10],
            rgb565(0x07e0),
            "border in RGB 5:6:5"
        );
        assert_eq!((at(12, 0), at(12, 1)), (rgb565(0x001f), rgb565(0x001f)));
        assert_eq!(
            (at(32, 0), at(32, 1)),
            (rgb565(0x001f), rgb565(0xf800)),
            "behind red"
        );
    }
}

#[cfg(test)]
mod hires_tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    fn show(mode: u16, pitch: u16, ram: &Ram) -> Lumiere {
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.write(VMODE, 0x8000 | mode);
        l.write(FBSEG, 1);
        l.write(PITCH, pitch);
        for i in 0..256u16 {
            l.write(PALETTE + 2 * i, i);
        }
        l.vblank();
        for y in 0..600 {
            l.end_line(y, ram);
        }
        l
    }

    #[test]
    fn mode_5_switches_the_next_field_to_xga() {
        let mut ram = vec![0; 0x1_0000 + 1024 * 768];
        ram[0x1_0000] = 9;
        ram[0x1_0000 + 1024 * 767 + 1023] = 99;
        let ram = Ram(ram);
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.write(VMODE, 0x8005);
        l.write(FBSEG, 1);
        l.write(PITCH, 1024);
        for i in 0..256u16 {
            l.write(PALETTE + 2 * i, i);
        }
        // The field it was switched in keeps SVGA's 628 lines.
        assert!(!l.xga());
        for y in 0..LINES {
            l.end_line(y, &ram);
        }
        assert!(l.xga());
        assert_eq!((l.width(), l.height(), l.lines()), (1024, 768, 806));
        assert_eq!(l.fetch_slots(0), 256);
        l.vblank();
        for y in 0..XGA_LINES {
            l.end_line(y, &ram);
        }
        assert_eq!(l.pixels()[0], rgb565(9));
        assert_eq!(l.pixels()[767 * 1024 + 1023], rgb565(99));
        assert!(l.xga());
        // back to mode 3: the next field is SVGA again
        l.write(VMODE, 0x8003);
        for y in 0..XGA_LINES {
            l.end_line(y, &ram);
        }
        assert!(!l.xga());
        assert_eq!((l.width(), l.height(), l.lines()), (800, 600, 628));
    }

    #[test]
    fn modes_2_and_3_fill_800_by_600() {
        let mut ram = vec![0; 0x1_0000 + 800 * 600];
        ram[0x1_0000] = 7; // mode 3: pixel (0, 0)
        ram[0x1_0000 + 800 * 599 + 799] = 200; // (799, 599)
        let l = show(3, 800, &Ram(ram));
        assert_eq!(l.pixels()[0], rgb565(7));
        assert_eq!(l.pixels()[599 * 800 + 799], rgb565(200));
        assert_eq!(l.fetch_slots(10), 200);
        let mut ram = vec![0; 0x1_0000 + 400 * 600];
        ram[0x1_0000] = 0x5a; // mode 2: pixels 5 and 10
        ram[0x1_0000 + 400 * 2 + 399] = 0x0f;
        let l = show(2, 400, &Ram(ram));
        assert_eq!((l.pixels()[0], l.pixels()[1]), (rgb565(5), rgb565(10)));
        assert_eq!(
            (l.pixels()[2 * 800 + 798], l.pixels()[2 * 800 + 799]),
            (rgb565(0), rgb565(15))
        );
        assert_eq!(l.fetch_slots(10), 100);
    }
}

#[cfg(test)]
mod fast_span_tests {
    use super::*;

    /// Chip RAM byte by byte only, and with whole rows (the fast path).
    struct Bytes(Vec<u8>);
    impl ChipRam for Bytes {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }
    struct Rows(Vec<u8>);
    impl ChipRam for Rows {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
        fn chip_bytes(&self, a: u32, len: usize) -> Option<&[u8]> {
            self.0.get(a as usize..a as usize + len)
        }
    }

    /// Every bitmap mode, with and without sprites, with writes in the
    /// middle of lines and pointers running off the end of chip RAM: the
    /// fast path puts out the same pixels and collisions.
    #[test]
    fn whole_rows_give_the_same_picture() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let ram: Vec<u8> = (0..0x10_0000).map(|_| rnd() as u8).collect();
        for mode in [0u16, 1, 2, 3, 4, 5] {
            for sprites in [false, true] {
                let run = |mem: &dyn ChipRam| {
                    let mut l = Lumiere::new();
                    l.set_dma(true);
                    l.set_sprite_dma(sprites);
                    l.write(VMODE, 0x8000 | mode);
                    l.write(BORDER, 0x1f3);
                    l.write(PITCH, 1000);
                    l.write(FBSEG, 0x0e);
                    l.write(FBOFS, 0x2000);
                    l.write(FONTSEG, 1);
                    l.write(SPRSEG, 3);
                    for i in 0..256u16 {
                        l.write(PALETTE + 2 * i, i.wrapping_mul(0x9e37));
                    }
                    for n in 0..8u16 {
                        l.write(SPRITES + 8 * n, 0x8000 | (n * 80 + 70) | (n % 3) << 11);
                        l.write(SPRITES + 8 * n + 2, (n * 37 + 60) | 3 << 10 | (n & 1) << 12);
                        l.write(SPRITES + 8 * n + 4, n * 5);
                        l.write(SPRITES + 8 * n + 6, (n & 1) | n << 8);
                    }
                    // two fields: the second has mode 5's timing
                    for _ in 0..2 {
                        l.vblank();
                        for y in 0..l.lines() {
                            l.select_sprites(y + 1, mem);
                            if y % 7 == 3 {
                                l.draw_to(y, y * 3 % 900, mem);
                                l.write(PALETTE + 2 * (y & 255), y);
                                l.draw_to(y, y * 5 % 1100, mem);
                                l.write(BORDER, y);
                            }
                            l.end_line(y, mem);
                        }
                    }
                    (l.pixels().to_vec(), l.col, l.colfg)
                };
                let (a, b) = (run(&Bytes(ram.clone())), run(&Rows(ram.clone())));
                assert!(a == b, "mode {mode}, sprites {sprites}");
            }
        }
    }
}
