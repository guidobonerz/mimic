//! Graphics data views of the S-PPU (see `emu_core::gfx`): VRAM as 2/4/8
//! bpp tiles (or Mode 7's 8 bpp tiles), one BG tilemap rendered with its
//! tiles and palettes (Mode 7: the 128 × 128 map), the 128 OAM objects
//! and the 256 CGRAM colours, decoded from the registers and the chip's
//! memories, side-effect free (VRAM, CGRAM and OAM are read directly; the
//! VRAM prefetch buffer, OAM and CGRAM address latches stay as they are).
//!
//! Layout after fullsnes, "SNES PPU Video Memory (VRAM)" (bit-planes in
//! pairs, 8 words a pair; BGxSC map base and 32/64 screens; BGxNBA
//! character base; Mode 7 map in the low and 8 bpp pixels in the high
//! bytes), "SNES PPU Sprites (OBJ)" (OBSEL sizes, name base and select
//! gap, the 16-tile row stride of large objects, attributes and the
//! high table), "SNES PPU Color Palette Memory (CGRAM)" (15-bit BGR,
//! sprites from colour 128).
//!
//! Colours are shown at full brightness (INIDISP's brightness and colour
//! math are not applied). Option `addr` is a **byte** address in the VRAM
//! space (as the machine's `vram` memory space counts), not a word address.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    grey_palette, palette_canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView,
    GfxViewInfo, OPT_ADDR, OPT_BPP, OPT_COUNT, OPT_INDEX, OPT_PALETTE,
};
use emu_core::MemSpaceId;

use super::{direct_colour, rgb, SPpu, OBJ_SIZES};

const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

pub const OPT_MODE7: GfxOptionInfo = GfxOptionInfo {
    name: "mode7",
    help: "1 = Mode 7 tiles (8 bpp, a byte per pixel in the high bytes; default: in Mode 7)",
};
const OPT_TILES_ADDR: GfxOptionInfo = GfxOptionInfo {
    name: "addr",
    help: "VRAM byte address of the first tile (default: BG1's character base)",
};
const OPT_TILES_PALETTE: GfxOptionInfo = GfxOptionInfo {
    name: "palette",
    help: "CGRAM palette: 2 bpp colour index ×4, 4 bpp ×16, 8 bpp any (default: greys)",
};
const OPT_BG: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "BG 1-4 (default 1; Mode 7 has only BG1's map)",
};

/// The memory spaces the views report.
#[derive(Clone, Copy)]
pub struct GfxContext {
    pub vram_space: MemSpaceId,
    pub cgram_space: MemSpaceId,
    pub oam_space: MemSpaceId,
}

/// The views an S-PPU offers: `tiles`, `screen`, `sprites`, `palette`.
pub fn views(name: &str) -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "tiles",
            kind: GfxKind::Tiles,
            name: format!("{name} VRAM tiles"),
            options: vec![
                OPT_BPP,
                OPT_TILES_ADDR,
                OPT_TILES_PALETTE,
                OPT_COUNT,
                OPT_MODE7,
            ],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: format!("{name} BG tilemap"),
            options: vec![OPT_BG, OPT_ADDR, OPT_BPP],
        },
        GfxViewInfo {
            id: "sprites",
            kind: GfxKind::Sprites,
            name: format!("{name} sprites (OAM)"),
            options: vec![OPT_INDEX],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: format!("{name} CGRAM"),
            options: vec![OPT_PALETTE],
        },
    ]
}

/// An RGB picture being drawn.
struct Picture {
    width: u32,
    height: u32,
    px: Vec<Rgb8>,
}

impl Picture {
    fn new(width: u32, height: u32, c: Rgb8) -> Self {
        Picture {
            width,
            height,
            px: vec![c; (width * height) as usize],
        }
    }

    fn set(&mut self, x: u32, y: u32, c: Rgb8) {
        if x < self.width && y < self.height {
            self.px[(y * self.width + x) as usize] = c;
        }
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: Rgb8) {
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                self.px[(yy * self.width + xx) as usize] = c;
            }
        }
    }
}

/// `count` cells of `w` × `h` in rows of `cols` with one gap pixel.
fn grid(count: u32, cols: u32, w: u32, h: u32) -> (Picture, u32) {
    let cols = cols.clamp(1, count.max(1));
    let rows = count.max(1).div_ceil(cols);
    (
        Picture::new(cols * (w + 1) - 1, rows * (h + 1) - 1, GAP_COLOR),
        cols,
    )
}

fn origin(i: u32, cols: u32, w: u32, h: u32) -> (u32, u32) {
    ((i % cols) * (w + 1), (i / cols) * (h + 1))
}

/// The words of VRAM from word `start`, `n` of them, as bytes (low first).
fn words_as_bytes(vram: &[u16], start: usize, n: usize) -> Vec<u8> {
    (0..n)
        .flat_map(|i| vram[(start + i) & 0x7fff].to_le_bytes())
        .collect()
}

impl SPpu {
    fn view_colour(&self, i: usize) -> Rgb8 {
        rgb(self.cgram[i & 0xff], 15)
    }

    /// Colour index of pixel (px, py) of the planar tile at word `addr`.
    fn tile_pixel(&self, addr: u16, bpp: u8, px: u16, py: u16) -> u16 {
        let bit = 7 - px;
        let mut c = 0;
        for plane in 0..u16::from(bpp) / 2 {
            let w = self.vram[usize::from(addr.wrapping_add(py + plane * 8) & 0x7fff)];
            c |= ((w >> bit) & 1) << (plane * 2);
            c |= ((w >> (8 + bit)) & 1) << (plane * 2 + 1);
        }
        c
    }

    /// Decode view `id` (from [`views`]). None for an unknown id.
    pub fn gfx_view(&self, id: &str, ctx: &GfxContext, opts: &GfxOptions) -> Option<GfxView> {
        let info = views("S-PPU").into_iter().find(|v| v.id == id)?;
        let (space, addr, params, pic, raw) = match info.kind {
            GfxKind::Tiles => self.tiles_view(opts),
            GfxKind::Screen => self.screen_view(opts),
            GfxKind::Sprites => self.sprites_view(opts),
            _ => {
                let colours: Vec<Rgb8> = (0..256).map(|i| self.view_colour(i)).collect();
                let (colours, first) = match opts.get("palette") {
                    Some(p) => {
                        let p = (p & 15) as usize;
                        (colours[p * 16..p * 16 + 16].to_vec(), p * 16)
                    }
                    None => (colours, 0),
                };
                let (w, h, pixels) = palette_canvas(&colours, 16, 16);
                let n = colours.len();
                return Some(GfxView {
                    id: info.id,
                    kind: info.kind,
                    name: info.name,
                    space: ctx.cgram_space,
                    addr: first as u32 * 2,
                    params: vec![("count", n as u32), ("first", first as u32)],
                    width: w,
                    height: h,
                    pixels,
                    raw: (first..first + n)
                        .flat_map(|i| self.cgram[i].to_le_bytes())
                        .collect(),
                });
            }
        };
        let space = match space {
            0 => ctx.vram_space,
            _ => ctx.oam_space,
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space,
            addr,
            params,
            width: pic.width,
            height: pic.height,
            pixels: GfxPixels::Rgb(pic.px),
            raw,
        })
    }

    /// `count` tiles (default 1024, the BG name range) from `addr` (default
    /// BG1's character base) in rows of 32, at `bpp` (default BG1's depth
    /// in the current mode, 2 when BG1 has none); in Mode 7 (or with
    /// `mode7`) the 256 Mode 7 tiles. Colours from CGRAM palette
    /// `palette`, colour 0 the backdrop, or four/16/256 greys without it.
    /// `raw`: the tiles' bytes (Mode 7: the 32 KB of words 0–$3FFF).
    #[allow(clippy::type_complexity)]
    fn tiles_view(
        &self,
        opts: &GfxOptions,
    ) -> (u8, u32, Vec<(&'static str, u32)>, Picture, Vec<u8>) {
        if opts.flag("mode7").unwrap_or(self.mode() == 7) {
            return self.mode7_tiles(opts);
        }
        let bpp = match opts.get("bpp") {
            Some(b @ (2 | 4 | 8)) => b as u8,
            Some(_) => 2,
            None => match self.bpp(0) {
                0 => 2,
                b => b,
            },
        };
        let word = match opts.get("addr") {
            Some(a) => ((a >> 1) & 0x7fff) as u16,
            None => (u16::from(self.nba[0]) << 12) & 0x7fff,
        };
        let words = u16::from(bpp) * 4;
        let count = opts.get("count").unwrap_or(1024).clamp(1, 4096);
        let pal = opts.get("palette");
        let greys = grey_palette(1 << bpp);
        let backdrop = self.view_colour(0);
        let (mut pic, cols) = grid(count, 32, 8, 8);
        for t in 0..count {
            let base = word.wrapping_add((t as u16).wrapping_mul(words));
            let (x0, y0) = origin(t, cols, 8, 8);
            for py in 0..8 {
                for px in 0..8 {
                    let c = self.tile_pixel(base, bpp, px, py);
                    let rgb = match pal {
                        None => greys[usize::from(c)],
                        Some(_) if c == 0 => backdrop,
                        Some(p) => {
                            let first = match bpp {
                                2 => p as usize * 4,
                                4 => p as usize * 16,
                                _ => 0,
                            };
                            self.view_colour(first + usize::from(c))
                        }
                    };
                    pic.set(x0 + u32::from(px), y0 + u32::from(py), rgb);
                }
            }
        }
        let raw = words_as_bytes(
            &self.vram,
            usize::from(word),
            count as usize * usize::from(words),
        );
        let mut params = vec![
            ("bpp", u32::from(bpp)),
            ("addr", u32::from(word) * 2),
            ("count", count),
            ("mode", u32::from(self.mode())),
        ];
        if let Some(p) = pal {
            params.push(("palette", p));
        }
        (0, u32::from(word) * 2, params, pic, raw)
    }

    /// The 256 Mode 7 tiles, 8 × 8 bytes each in the high bytes of words
    /// 0–$3FFF, a colour index per pixel (CGRAM, or direct colour with
    /// CGWSEL bit 0) when `palette` is given (any value), else 256 greys.
    #[allow(clippy::type_complexity)]
    fn mode7_tiles(
        &self,
        opts: &GfxOptions,
    ) -> (u8, u32, Vec<(&'static str, u32)>, Picture, Vec<u8>) {
        let greys = grey_palette(256);
        let cgram = opts.get("palette").is_some();
        let (mut pic, cols) = grid(256, 16, 8, 8);
        for t in 0..256u32 {
            let (x0, y0) = origin(t, cols, 8, 8);
            for i in 0..64u32 {
                let c = (self.vram[(t * 64 + i) as usize] >> 8) as u8;
                let rgb = if !cgram {
                    greys[usize::from(c)]
                } else if self.cgwsel & 1 != 0 {
                    rgb(direct_colour(c, 0), 15)
                } else {
                    self.view_colour(usize::from(c))
                };
                pic.set(x0 + i % 8, y0 + i / 8, rgb);
            }
        }
        let params = vec![("bpp", 8), ("addr", 0), ("count", 256), ("mode7", 1)];
        (0, 0, params, pic, words_as_bytes(&self.vram, 0, 0x4000))
    }

    /// BG `index` (1–4) as its whole tilemap: 32 or 64 tiles each way as
    /// BGxSC sets, 8 or 16 pixels a tile as BGMODE sets, with the
    /// character base, depth (current mode, or `bpp`) and palettes; colour
    /// 0 shows the backdrop. `addr` overrides the map's VRAM byte address.
    /// Mode 7: the 128 × 128 map with its tiles (1024 × 1024). `raw`: the
    /// map's bytes (Mode 7: the 32 KB of words 0–$3FFF).
    #[allow(clippy::type_complexity)]
    fn screen_view(
        &self,
        opts: &GfxOptions,
    ) -> (u8, u32, Vec<(&'static str, u32)>, Picture, Vec<u8>) {
        if self.mode() == 7 {
            return self.mode7_screen();
        }
        let n = (opts.get("index").unwrap_or(1).clamp(1, 4) - 1) as usize;
        let bpp = match opts.get("bpp") {
            Some(b @ (2 | 4 | 8)) => b as u8,
            _ => match self.bpp(n) {
                0 => 2,
                b => b,
            },
        };
        let sc = self.bgsc[n];
        let map = match opts.get("addr") {
            Some(a) => ((a >> 1) & 0x7c00) as u16,
            None => (u16::from(sc & 0xfc) << 8) & 0x7fff,
        };
        let (wide, tall) = (sc & 1 != 0, sc & 2 != 0);
        let (tw, th) = (if wide { 64 } else { 32 }, if tall { 64 } else { 32 });
        let ts: u16 = if self.bgmode & (0x10 << n) != 0 {
            16
        } else {
            8
        };
        let chars = (u16::from(self.nba[n]) << 12) & 0x7fff;
        let words = u16::from(bpp) * 4;
        let pal_base = if self.mode() == 0 { n * 32 } else { 0 };
        let direct = bpp == 8 && self.cgwsel & 1 != 0;
        let backdrop = self.view_colour(0);
        let (w, h) = (u32::from(tw * ts), u32::from(th * ts));
        let mut pic = Picture::new(w, h, backdrop);
        let entry = |tx: u16, ty: u16| -> u16 {
            // Screens: 32 × 32 entries each, left to right then down.
            let mut off = (ty & 31) * 32 + (tx & 31);
            if wide {
                off += (tx & 32) << 5;
            }
            if tall {
                off += (ty & 32) << if wide { 6 } else { 5 };
            }
            self.vram[usize::from(map.wrapping_add(off) & 0x7fff)]
        };
        for ty in 0..th {
            for tx in 0..tw {
                let e = entry(tx, ty);
                let pal = usize::from((e >> 10) & 7);
                for iy in 0..ts {
                    for ix in 0..ts {
                        let fx = if e & 0x4000 != 0 { ts - 1 - ix } else { ix };
                        let fy = if e & 0x8000 != 0 { ts - 1 - iy } else { iy };
                        let ch =
                            (e & 0x3ff).wrapping_add(fx / 8).wrapping_add((fy / 8) * 16) & 0x3ff;
                        let addr = chars.wrapping_add(ch.wrapping_mul(words));
                        let c = self.tile_pixel(addr, bpp, fx & 7, fy & 7);
                        if c == 0 {
                            continue;
                        }
                        let rgb = if direct {
                            rgb(direct_colour(c as u8, pal as u8), 15)
                        } else {
                            self.view_colour(match bpp {
                                2 => pal_base + pal * 4 + usize::from(c),
                                4 => pal * 16 + usize::from(c),
                                _ => usize::from(c),
                            })
                        };
                        pic.set(u32::from(tx * ts + ix), u32::from(ty * ts + iy), rgb);
                    }
                }
            }
        }
        let screens = usize::from(wide) + 1;
        let screens = screens * (usize::from(tall) + 1);
        let raw = words_as_bytes(&self.vram, usize::from(map), screens * 1024);
        let params = vec![
            ("bg", n as u32 + 1),
            ("mode", u32::from(self.mode())),
            ("bpp", u32::from(bpp)),
            ("map", u32::from(map) * 2),
            ("chars", u32::from(chars) * 2),
            ("width_tiles", u32::from(tw)),
            ("height_tiles", u32::from(th)),
            ("tile_size", u32::from(ts)),
            ("hofs", u32::from(self.hofs[n] & 0x3ff)),
            ("vofs", u32::from(self.vofs[n] & 0x3ff)),
        ];
        (0, u32::from(map) * 2, params, pic, raw)
    }

    #[allow(clippy::type_complexity)]
    fn mode7_screen(&self) -> (u8, u32, Vec<(&'static str, u32)>, Picture, Vec<u8>) {
        let direct = self.cgwsel & 1 != 0;
        let backdrop = self.view_colour(0);
        let mut pic = Picture::new(1024, 1024, backdrop);
        for ty in 0..128u32 {
            for tx in 0..128u32 {
                let tile = u32::from(self.vram[(ty * 128 + tx) as usize] & 0xff);
                for i in 0..64u32 {
                    let c = (self.vram[(tile * 64 + i) as usize] >> 8) as u8;
                    if c == 0 {
                        continue;
                    }
                    let rgb = if direct {
                        rgb(direct_colour(c, 0), 15)
                    } else {
                        self.view_colour(usize::from(c))
                    };
                    pic.set(tx * 8 + i % 8, ty * 8 + i / 8, rgb);
                }
            }
        }
        let s13 = |v: u16| (((v << 3) as i16) >> 3) as i32 as u32;
        let params = vec![
            ("bg", 1),
            ("mode", 7),
            ("bpp", 8),
            ("map", 0),
            ("width_tiles", 128),
            ("height_tiles", 128),
            ("tile_size", 8),
            ("m7a", u32::from(self.m7[0])),
            ("m7b", u32::from(self.m7[1])),
            ("m7c", u32::from(self.m7[2])),
            ("m7d", u32::from(self.m7[3])),
            ("m7x", s13(self.m7x)),
            ("m7y", s13(self.m7y)),
            ("hofs", s13(self.m7hofs)),
            ("vofs", s13(self.m7vofs)),
            ("m7sel", u32::from(self.m7sel)),
        ];
        (0, 0, params, pic, words_as_bytes(&self.vram, 0, 0x4000))
    }

    /// The 128 objects in rows of 16, each in a cell of the large size
    /// (`index`: one object in a cell of its own size), drawn with its
    /// palette (CGRAM 128 + 16·n) and flips on the backdrop. `raw`: the
    /// 544 OAM bytes (`index`: its four bytes and its high-table byte).
    #[allow(clippy::type_complexity)]
    fn sprites_view(
        &self,
        opts: &GfxOptions,
    ) -> (u8, u32, Vec<(&'static str, u32)>, Picture, Vec<u8>) {
        let (small, large) = OBJ_SIZES[usize::from(self.obsel >> 5)];
        let base = u16::from(self.obsel & 7) << 13;
        let gap = (u16::from((self.obsel >> 3) & 3) + 1) << 12;
        let index = opts.get("index").map(|i| (i & 127) as usize);
        let list: Vec<usize> = match index {
            Some(i) => vec![i],
            None => (0..128).collect(),
        };
        let hi_bits = |i: usize| (self.oam[0x200 + i / 4] >> ((i & 3) * 2)) & 3;
        let size = |i: usize| if hi_bits(i) & 2 != 0 { large } else { small };
        let (cw, ch) = match index {
            Some(i) => size(i),
            None => large,
        };
        let (cw, ch) = (u32::from(cw), u32::from(ch));
        let backdrop = self.view_colour(0);
        let (mut pic, cols) = grid(list.len() as u32, 16, cw, ch);
        for (cell, &i) in list.iter().enumerate() {
            let (x0, y0) = origin(cell as u32, cols, cw, ch);
            pic.fill(x0, y0, cw, ch, backdrop);
            let (w, h) = size(i);
            let tile = u16::from(self.oam[i * 4 + 2]);
            let attr = self.oam[i * 4 + 3];
            let name = if attr & 1 != 0 { gap } else { 0 };
            let pal = usize::from((attr >> 1) & 7);
            for y in 0..h {
                let r = if attr & 0x80 != 0 { h - 1 - y } else { y };
                for x in 0..w {
                    let fx = if attr & 0x40 != 0 { w - 1 - x } else { x };
                    let chr = ((tile + (r / 8) * 16) & 0xf0) | ((tile + fx / 8) & 0x0f);
                    let addr = base.wrapping_add(name).wrapping_add(chr * 16);
                    let c = self.tile_pixel(addr, 4, fx & 7, r & 7);
                    if c != 0 {
                        pic.set(
                            x0 + u32::from(x),
                            y0 + u32::from(y),
                            self.view_colour(128 + pal * 16 + usize::from(c)),
                        );
                    }
                }
            }
        }
        let mut params = vec![
            ("obsel", u32::from(self.obsel)),
            ("base", u32::from(base) * 2),
            ("gap", u32::from(gap) * 2),
            ("small_w", u32::from(small.0)),
            ("small_h", u32::from(small.1)),
            ("large_w", u32::from(large.0)),
            ("large_h", u32::from(large.1)),
        ];
        let (addr, raw) = match index {
            Some(i) => {
                let e = &self.oam[i * 4..i * 4 + 4];
                let hi = hi_bits(i);
                let (w, h) = size(i);
                params.extend([
                    ("index", i as u32),
                    ("x", u32::from(e[0]) | u32::from(hi & 1) << 8),
                    ("y", u32::from(e[1])),
                    ("tile", u32::from(e[2])),
                    ("name_select", u32::from(e[3] & 1)),
                    ("palette", u32::from((e[3] >> 1) & 7)),
                    ("priority", u32::from((e[3] >> 4) & 3)),
                    ("flip_h", u32::from((e[3] >> 6) & 1)),
                    ("flip_v", u32::from(e[3] >> 7)),
                    ("large", u32::from(hi >> 1)),
                    ("width", u32::from(w)),
                    ("height", u32::from(h)),
                ]);
                let mut raw = e.to_vec();
                raw.push(self.oam[0x200 + i / 4]);
                (i as u32 * 4, raw)
            }
            None => (0, self.oam.clone()),
        };
        (1, addr, params, pic, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: GfxContext = GfxContext {
        vram_space: MemSpaceId(1),
        cgram_space: MemSpaceId(2),
        oam_space: MemSpaceId(3),
    };

    fn view(p: &SPpu, id: &str, opts: &GfxOptions) -> GfxView {
        p.gfx_view(id, &CTX, opts).unwrap()
    }

    fn at(v: &GfxView, x: u32, y: u32) -> Rgb8 {
        v.frame().rgb_at(x, y).unwrap()
    }

    const RED: u16 = 0x001f;
    const GREEN: u16 = 0x03e0;

    /// Mode 1; BG1 map at word $0400 (64 × 32), characters at word $1000.
    fn scene() -> SPpu {
        let mut p = SPpu::new(false);
        p.write(0x05, 0x01);
        p.write(0x07, 0x05); // map $0400, 64 wide
        p.write(0x0b, 0x01);
        // Tile 1 (4 bpp, 16 words from $1000): row 0, pixel 0 = colour 1.
        p.vram[0x1000 + 16] = 0x0080;
        p.cgram[0] = GREEN;
        p.cgram[2 * 16 + 1] = RED;
        // Second screen (x 32), row 1: tile 1, palette 2, h-flip.
        p.vram[0x0400 + 0x400 + 32] = 0x4000 | 2 << 10 | 1;
        p
    }

    #[test]
    fn tiles_default_to_bg1() {
        let p = scene();
        let before = p.clone();
        let t = view(&p, "tiles", &GfxOptions::new());
        assert_eq!(p, before, "side-effect free");
        assert_eq!(t.space, CTX.vram_space);
        assert_eq!((t.param("bpp"), t.addr), (Some(4), 0x2000));
        assert_eq!((t.width, t.height), (32 * 9 - 1, 32 * 9 - 1));
        assert_eq!(t.raw.len(), 1024 * 32);
        assert_eq!(
            at(&t, 9, 0),
            Rgb8 {
                r: 17,
                g: 17,
                b: 17
            },
            "grey 1 of 16"
        );
        let c = view(&p, "tiles", &GfxOptions::new().with("palette", 2));
        assert_eq!(at(&c, 9, 0), rgb(RED, 15));
        assert_eq!(at(&c, 10, 0), rgb(GREEN, 15), "colour 0: backdrop");
        let b = view(
            &p,
            "tiles",
            &GfxOptions::new()
                .with("bpp", 2)
                .with("addr", 0x2000)
                .with("count", 4),
        );
        assert_eq!((b.width, b.raw.len()), (4 * 9 - 1, 4 * 16));
    }

    #[test]
    fn screen_renders_the_tilemap() {
        let p = scene();
        let s = view(&p, "screen", &GfxOptions::new());
        assert_eq!((s.width, s.height), (512, 256));
        assert_eq!(s.addr, 0x0800);
        assert_eq!(s.raw.len(), 2 * 2048);
        assert_eq!(s.param("width_tiles"), Some(64));
        assert_eq!(at(&s, 256 + 7, 8), rgb(RED, 15), "flipped pixel 0");
        assert_eq!(at(&s, 256, 8), rgb(GREEN, 15));
        let bg3 = view(&p, "screen", &GfxOptions::new().with("index", 3));
        assert_eq!((bg3.param("bg"), bg3.param("bpp")), (Some(3), Some(2)));
        assert_eq!((bg3.width, bg3.height), (256, 256));
    }

    #[test]
    fn mode7_map_and_tiles() {
        let mut p = SPpu::new(false);
        p.write(0x05, 0x07);
        p.vram[129] = 0x0003; // map (1, 1) = tile 3
        p.vram[3 * 64 + 9] = 0x0500; // tile 3 pixel (1, 1) = colour 5
        p.cgram[5] = RED;
        let s = view(&p, "screen", &GfxOptions::new());
        assert_eq!((s.width, s.height, s.raw.len()), (1024, 1024, 0x8000));
        assert_eq!(at(&s, 9, 9), rgb(RED, 15));
        let t = view(&p, "tiles", &GfxOptions::new().with("palette", 0));
        assert_eq!(t.param("mode7"), Some(1));
        assert_eq!(at(&t, 3 * 9 + 1, 1), rgb(RED, 15));
    }

    #[test]
    fn sprites_and_palette() {
        let mut p = SPpu::new(false);
        p.write(0x01, 0x02 << 5 | 0x01); // 8/64, base word $2000
        let i = 6;
        p.oam[i * 4..i * 4 + 4].copy_from_slice(&[0x10, 0x20, 0x01, 0x80 | 3 << 1]);
        p.oam[0x200 + 1] = 0b10 << 4; // object 6 large
                                      // Tile $11 (row 8 of a large object): pixel 0 of its row 0 = colour 1.
        p.vram[0x2000 + 0x11 * 16] = 0x0080;
        p.cgram[128 + 3 * 16 + 1] = RED;
        let s = view(&p, "sprites", &GfxOptions::new().with("index", 6));
        assert_eq!(s.space, CTX.oam_space);
        assert_eq!((s.width, s.height), (64, 64));
        assert_eq!(s.raw.len(), 5);
        assert_eq!(
            (s.param("large"), s.param("palette"), s.param("flip_v")),
            (Some(1), Some(3), Some(1))
        );
        // v-flipped: source row 8 shows at row 64 − 1 − 8.
        assert_eq!(at(&s, 0, 55), rgb(RED, 15));
        let all = view(&p, "sprites", &GfxOptions::new());
        assert_eq!(
            (all.width, all.height, all.raw.len()),
            (16 * 65 - 1, 8 * 65 - 1, 544)
        );
        let pal = view(&p, "palette", &GfxOptions::new());
        assert_eq!((pal.space, pal.raw.len()), (CTX.cgram_space, 512));
        assert_eq!(at(&pal, 17, 17 * 11), rgb(RED, 15), "colour 177");
        let one = view(&p, "palette", &GfxOptions::new().with("palette", 11));
        assert_eq!((one.addr, one.raw.len()), (176 * 2, 32));
    }
}
