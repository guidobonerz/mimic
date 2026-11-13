//! Graphics data views of the V9938 (see `emu_core::gfx`), decoded from
//! the registers, the palette and VRAM, side-effect free: `screen` (the
//! name table or the bitmap page in the current mode, without sprites and
//! regardless of BL and R23), `sprites` (the 32 sprites, in mode 2 with
//! their line colours), `tiles` (the pattern generator table, white on
//! black) and `palette` (the 16 entries).
//!
//! Table addresses after the Technical Data Book, chapter 1 (R2–R6, R10,
//! R11) and chapter 2 (the modes' tables), sprites after chapter 3.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR, OPT_COUNT, OPT_INDEX,
};
use emu_core::MemSpaceId;

use super::{mode, resolve, rgb777, BORDER, G7_SPRITE_COLORS, V9938, VRAM};

const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

pub const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "display mode M5-M1: 0 G1, 1 T1, 2 MC, 4 G2, 8 G3, 9 T2, 12 G4, 16 G5, 20 G6, 28 G7 (default: as the registers select)",
};
pub const OPT_PAGE: GfxOptionInfo = GfxOptionInfo {
    name: "page",
    help: "bitmap page 0-3 (G6, G7: 0-1; default: the page R2 selects)",
};

/// What a view returns besides its id and name: address, parameters,
/// width, height, picture, raw bytes.
type View = (u32, Vec<(&'static str, u32)>, u32, u32, GfxPixels, Vec<u8>);

/// The views: `tiles`, `sprites`, `screen`, `palette`. Option `addr` is a
/// VRAM address.
pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "tiles",
            kind: GfxKind::Tiles,
            name: "V9938 pattern table".into(),
            options: vec![OPT_ADDR, OPT_COUNT],
        },
        GfxViewInfo {
            id: "sprites",
            kind: GfxKind::Sprites,
            name: "V9938 sprites".into(),
            options: vec![OPT_INDEX],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: "V9938 screen".into(),
            options: vec![OPT_MODE, OPT_PAGE],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "V9938 palette".into(),
            options: vec![],
        },
    ]
}

impl V9938 {
    /// Decode view `id` (from [`views`]); `space` is the machine's VRAM
    /// space. None for an unknown id.
    pub fn gfx_view(&self, id: &str, space: MemSpaceId, opts: &GfxOptions) -> Option<GfxView> {
        let info = views().into_iter().find(|v| v.id == id)?;
        let (addr, params, width, height, pixels, raw) = match info.kind {
            GfxKind::Tiles => self.tiles_view(opts),
            GfxKind::Sprites => self.sprites_view(opts),
            GfxKind::Screen => self.screen_view(opts),
            _ => {
                let (w, h, px) = palette_canvas(&self.colors(), 16, 16);
                let raw = self
                    .palette
                    .iter()
                    .flat_map(|p| [*p as u8, (*p >> 8) as u8])
                    .collect();
                let params = vec![
                    ("count", 16),
                    ("backdrop", u32::from(self.regs[7])),
                    ("pointer", u32::from(self.regs[16])),
                ];
                (0, params, w, h, px, raw)
            }
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space,
            addr,
            params,
            width,
            height,
            pixels,
            raw,
        })
    }

    /// Patterns in rows of 32, white on black: 256 from R4 (G2 and G3: all
    /// three thirds through R4's mask), or `count` from `addr`.
    fn tiles_view(&self, opts: &GfxOptions) -> View {
        let m = self.mode();
        let thirds = matches!(m, mode::G2 | mode::G3);
        let patterns = self.pattern_base();
        let given = opts.get("addr").map(|a| a as usize & (VRAM - 8));
        let base = given.unwrap_or(if thirds {
            patterns & 0x1_e000
        } else {
            patterns
        });
        let default_count = if thirds && given.is_none() { 768 } else { 256 };
        let count = opts.get("count").unwrap_or(default_count).clamp(1, 2048) as usize;
        let mut c = Canvas::grid(count as u32, 32, 8, 8, 1, 2);
        let mut raw = Vec::with_capacity(count * 8);
        for i in 0..count {
            let (x0, y0) = Canvas::cell_origin(i as u32, 32, 8, 8, 1);
            for row in 0..8 {
                let offset = i << 3 | row;
                let a = if thirds && given.is_none() {
                    base | (offset & ((patterns & 0x1800) | 0x7ff))
                } else {
                    base + offset
                };
                let pattern = self.v(a);
                raw.push(pattern);
                c.row_1bpp(x0, y0 + row as u32, pattern, 1, Some(0), 1);
            }
        }
        let params = vec![
            ("mode", u32::from(m)),
            ("patterns", base as u32),
            ("count", count as u32),
        ];
        let white = Rgb8 {
            r: 255,
            g: 255,
            b: 255,
        };
        let (w, h) = (c.width, c.height);
        let px = c.into_pixels(vec![Rgb8::default(), white, GAP_COLOR]);
        (base as u32, params, w, h, px, raw)
    }

    /// The 32 sprites (or `index`), each in a cell of 8×8 or 16×16 pixels,
    /// unmagnified, on the backdrop colour: in mode 1 in the attribute's
    /// colour, in mode 2 each line in its colour (CC lines are drawn on
    /// their own). `raw`: the attribute bytes (4 per sprite shown), in
    /// mode 2 then the 16 colour bytes of each.
    fn sprites_view(&self, opts: &GfxOptions) -> View {
        let m = self.mode();
        let mode2 = Self::sprite_mode(m) == 2;
        let (size, mag) = self.sprite_size();
        let colors = self.sprite_attr_base() & 0x1_fc00;
        let attrs = if mode2 {
            colors + 0x200
        } else {
            self.sprite_attr_base()
        };
        let end = if mode2 { 216 } else { 208 };
        let active = (0..32)
            .find(|&n| self.v(attrs + n * 4) == end)
            .unwrap_or(32);
        let index = opts.get("index").map(|i| (i & 31) as usize);
        let list: Vec<usize> = index.map_or((0..32).collect(), |i| vec![i]);
        let pal = self.colors();
        let backdrop = self.border(m, &pal)[0];
        let cols = 8.min(list.len());
        let rows = list.len().div_ceil(cols);
        let (w, h) = (cols * (size + 1) - 1, rows * (size + 1) - 1);
        let mut px = vec![GAP_COLOR; w * h];
        let mut raw = Vec::new();
        for (cell, &n) in list.iter().enumerate() {
            let a = attrs + n * 4;
            raw.extend((0..4).map(|k| self.v(a + k)));
            if mode2 {
                raw.extend((0..16).map(|k| self.v(colors + n * 16 + k)));
            }
            let (x0, y0) = ((cell % cols) * (size + 1), (cell / cols) * (size + 1));
            for row in 0..size {
                let color = if mode2 {
                    self.v(colors + n * 16 + row)
                } else {
                    self.v(a + 3)
                } & 15;
                let rgb = if m == mode::G7 {
                    let (r, g, b) = G7_SPRITE_COLORS[usize::from(color)];
                    rgb777(r, g, b)
                } else {
                    pal[usize::from(color)]
                };
                let bits = self.sprite_row(self.v(a + 2), row, size);
                for x in 0..size {
                    let set = bits & (0x8000 >> x) != 0;
                    px[(y0 + row) * w + x0 + x] = if set && color != 0 { rgb } else { backdrop };
                }
            }
        }
        let mut params = vec![
            ("mode", u32::from(Self::sprite_mode(m))),
            ("attrs", attrs as u32),
            ("patterns", self.sprite_pattern_base() as u32),
            ("size", size as u32),
            ("mag", u32::from(mag == 2)),
            ("active", active as u32),
        ];
        if let Some(n) = index {
            let a = attrs + n * 4;
            params.extend([
                ("index", n as u32),
                ("y", u32::from(self.v(a))),
                ("x", u32::from(self.v(a + 1))),
                ("name", u32::from(self.v(a + 2))),
            ]);
        }
        (
            attrs as u32,
            params,
            w as u32,
            h as u32,
            GfxPixels::Rgb(px),
            raw,
        )
    }

    /// The screen in the current mode (or `mode`): the pattern modes'
    /// name table over the active lines, the bitmap modes' whole page of
    /// 256 lines (`page`). `raw`: the name table or the page.
    fn screen_view(&self, opts: &GfxOptions) -> View {
        let m = opts.get("mode").map_or(self.mode(), |m| m as u8 & 0x1f);
        let bitmap = matches!(m, mode::G4 | mode::G5 | mode::G6 | mode::G7);
        let wide = matches!(m, mode::T2 | mode::G5 | mode::G6);
        let page_size: usize = if matches!(m, mode::G6 | mode::G7) {
            0x1_0000
        } else {
            0x8000
        };
        let page = match opts.get("page") {
            Some(p) => (p as usize * page_size) & (VRAM - 1),
            None => self.page_base(m),
        };
        // The part of the 512 codes a mode fills, and its step.
        let (first, cells, step) = match m {
            mode::T1 => (16, 240, 2),
            mode::T2 => (16, 480, 1),
            _ if wide => (0, 512, 1),
            _ => (0, 256, 2),
        };
        let height = if bitmap {
            256
        } else {
            usize::from(self.active_lines())
        };
        let pal = self.colors();
        let border = self.border(m, &pal);
        let tp = self.regs[8] & 0x20 != 0;
        let mut pixels = Vec::with_capacity(cells * height);
        let mut line = [BORDER; 512];
        for y in 0..height {
            self.background(m, y as u8, page, &mut line);
            pixels.extend((0..cells).map(|i| {
                let x = first + i * step;
                resolve(line[x], tp, border[x & 1], &pal)
            }));
        }
        let (addr, len) = match m {
            _ if bitmap => (page, page_size),
            mode::T1 => (self.name_base(), 960),
            mode::T2 => (usize::from(self.regs[2] & 0x7c) << 10, 2160),
            _ => (self.name_base(), 768),
        };
        let raw = (0..len).map(|i| self.v(addr + i)).collect();
        let params = vec![
            ("mode", u32::from(m)),
            ("patterns", self.pattern_base() as u32),
            ("colors", self.color_base() as u32),
            ("backdrop", u32::from(self.regs[7])),
            ("scroll", u32::from(self.regs[23])),
            ("blank", u32::from(self.regs[1] & 0x40 == 0)),
        ];
        (
            addr as u32,
            params,
            cells as u32,
            height as u32,
            GfxPixels::Rgb(pixels),
            raw,
        )
    }
}
