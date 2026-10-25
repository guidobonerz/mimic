//! Graphics data views of the 2C02/2C07 (see `emu_core::gfx`): both pattern
//! tables, the four nametables, the 64 OAM sprites and palette RAM,
//! decoded from the PPU's registers and its address space, side-effect
//! free: the address space is read through the machine's peek path (no
//! PPUDATA access, so the read buffer and v stay; no address on the PPU
//! bus, so a mapper watching A12 such as the MMC3 sees nothing).
//!
//! Layout after the NESdev wiki, "PPU pattern tables" (16 bytes a tile,
//! plane 0 then plane 1), "PPU nametables" (32 × 30 names, mirroring),
//! "PPU attribute tables" (a byte per 32 × 32 pixels, two bits per 16 ×
//! 16 quadrant), "PPU OAM" (Y, tile, attributes, X; 8×16 sprites take the
//! table from the tile's bit 0), "PPU palettes" ($3F10/$14/$18/$1C mirror
//! $3F00/$04/$08/$0C; colour 0 of every palette shows the backdrop) and
//! "PPU scrolling" (t and fine x).

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    grey_palette, palette_canvas, Canvas, GfxKind, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_INDEX, OPT_PALETTE,
};
use emu_core::MemSpaceId;

use super::{Ppu, PpuModel};

impl PpuModel {
    /// The chip's name for view titles.
    pub fn chip_name(self) -> &'static str {
        match self {
            PpuModel::Ntsc => "2C02",
            PpuModel::Pal => "2C07",
        }
    }
}

/// Picture palette layout: the 32 palette RAM colours, four greys, the gap.
const GREY: u8 = 32;
const GAP: u8 = 36;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

/// What the machine tells the decoder besides the PPU.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    /// The PPU address space $0000–$2FFF as the PPU sees it now (pattern
    /// tables through the mapper's CHR banking, nametables through its
    /// mirroring), read without side effects.
    pub peek: &'a dyn Fn(u16) -> u8,
    /// The PPU address space (pattern tables, nametables, palette).
    pub space: MemSpaceId,
    /// Object attribute memory.
    pub oam_space: MemSpaceId,
}

/// The views a 2C02 offers: `tiles`, `screen`, `sprites`, `palette`.
pub fn views(name: &str) -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "tiles",
            kind: GfxKind::Tiles,
            name: format!("{name} pattern tables"),
            options: vec![OPT_PALETTE],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: format!("{name} nametables"),
            options: vec![],
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
            name: format!("{name} palette RAM"),
            options: vec![],
        },
    ]
}

impl Ppu {
    /// Palette RAM entry `i` (0–31) as the PPU reads it (mirrors resolved).
    pub fn palette_entry(&self, i: usize) -> u8 {
        self.palette[Self::palette_index(i as u16)] & 0x3f
    }

    /// The 32 palette RAM colours through the colour table with the
    /// current emphasis bits (greyscale is not applied).
    fn view_colours(&self) -> Vec<Rgb8> {
        (0..32)
            .map(|i| self.colour(self.palette_entry(i)))
            .collect()
    }

    /// Decode view `id` (from [`views`]). None for an unknown id.
    pub fn gfx_view(&self, id: &str, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Option<GfxView> {
        let info = views(self.model.chip_name())
            .into_iter()
            .find(|v| v.id == id)?;
        let name = info.name;
        let colours = self.view_colours();
        if info.kind == GfxKind::Palette {
            let (w, h, pixels) = palette_canvas(&colours, 16, 16);
            return Some(GfxView {
                id: info.id,
                kind: info.kind,
                name,
                space: ctx.space,
                addr: 0x3f00,
                params: vec![("count", 32), ("emphasis", u32::from(self.mask >> 5))],
                width: w,
                height: h,
                pixels,
                raw: (0..32).map(|i| self.palette_entry(i)).collect(),
            });
        }
        let mut palette = colours;
        palette.extend(grey_palette(4));
        palette.push(GAP_COLOR);
        let (space, addr, params, canvas, raw) = match info.kind {
            GfxKind::Tiles => self.tiles_view(ctx, opts),
            GfxKind::Screen => self.screen_view(ctx),
            _ => self.sprites_view(ctx, opts),
        };
        let space = if space { ctx.space } else { ctx.oam_space };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name,
            space,
            addr,
            params,
            width: canvas.width,
            height: canvas.height,
            pixels: GfxPixels::Indexed {
                pixels: canvas.pixels,
                palette,
            },
            raw,
        })
    }

    /// Colour index of pixel `x` (0–7, left to right) of a tile row.
    fn tile_pixel(lo: u8, hi: u8, x: u32) -> u8 {
        ((lo >> (7 - x)) & 1) | ((hi >> (7 - x)) & 1) << 1
    }

    /// Both pattern tables side by side (16 × 16 tiles each, two gap
    /// columns between), in palette `palette` (0–3 background, 4–7
    /// sprites; colour 0 the backdrop) or, without it, four greys. `raw`:
    /// the 8 KB at $0000–$1FFF.
    #[allow(clippy::type_complexity)]
    fn tiles_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (bool, u32, Vec<(&'static str, u32)>, Canvas, Vec<u8>) {
        let pal = opts.get("palette").map(|p| (p & 7) as u8);
        let mut c = Canvas::new(258, 128, GAP);
        let raw: Vec<u8> = (0..0x2000u16).map(|a| (ctx.peek)(a)).collect();
        for tile in 0..512usize {
            let (table, t) = (tile / 256, tile % 256);
            let x0 = (table * 130 + (t % 16) * 8) as u32;
            let y0 = ((t / 16) * 8) as u32;
            for row in 0..8 {
                let lo = raw[tile * 16 + row];
                let hi = raw[tile * 16 + row + 8];
                for x in 0..8 {
                    let v = Self::tile_pixel(lo, hi, x);
                    let idx = match pal {
                        None => GREY + v,
                        Some(_) if v == 0 => 0,
                        Some(p) => p * 4 + v,
                    };
                    c.set(x0 + x, y0 + row as u32, idx);
                }
            }
        }
        let mut params = vec![
            ("bg_table", u32::from(self.ctrl & 0x10) << 8),
            ("sprite_table", u32::from(self.ctrl & 0x08) << 9),
        ];
        if let Some(p) = pal {
            params.push(("palette", u32::from(p)));
        }
        (true, 0, params, c, raw)
    }

    /// The four nametables at $2000/$2400/$2800/$2C00 as the cartridge
    /// mirrors them, 512 × 480 (left to right, top to bottom), with the
    /// background pattern table and attribute colours. The scroll the next
    /// frame starts from (t and fine x) is in the parameters. `raw`: the
    /// 4 KB at $2000–$2FFF.
    #[allow(clippy::type_complexity)]
    fn screen_view(
        &self,
        ctx: &GfxContext<'_>,
    ) -> (bool, u32, Vec<(&'static str, u32)>, Canvas, Vec<u8>) {
        let bg_table = u16::from(self.ctrl & 0x10) << 8;
        let raw: Vec<u8> = (0x2000..0x3000u16).map(|a| (ctx.peek)(a)).collect();
        let mut c = Canvas::new(512, 480, 0);
        for nt in 0..4usize {
            let base = nt * 0x400;
            let (ox, oy) = ((nt & 1) as u32 * 256, (nt >> 1) as u32 * 240);
            for ty in 0..30usize {
                for tx in 0..32usize {
                    let name = u16::from(raw[base + ty * 32 + tx]);
                    let attr = raw[base + 0x3c0 + (ty / 4) * 8 + tx / 4];
                    let shift = ((ty & 2) << 1) | (tx & 2);
                    let pal = (attr >> shift) & 3;
                    for row in 0..8u16 {
                        let a = bg_table | name << 4 | row;
                        let (lo, hi) = ((ctx.peek)(a), (ctx.peek)(a + 8));
                        for x in 0..8 {
                            let v = Self::tile_pixel(lo, hi, x);
                            let idx = if v == 0 { 0 } else { pal * 4 + v };
                            c.set(
                                ox + tx as u32 * 8 + x,
                                oy + ty as u32 * 8 + u32::from(row),
                                idx,
                            );
                        }
                    }
                }
            }
        }
        let t = u32::from(self.t);
        let scroll_x = ((t >> 10) & 1) * 256 + (t & 31) * 8 + u32::from(self.x);
        let scroll_y = ((t >> 11) & 1) * 240 + ((t >> 5) & 31) * 8 + ((t >> 12) & 7);
        let params = vec![
            ("bg_table", u32::from(bg_table)),
            ("base_nametable", u32::from(self.ctrl & 3)),
            ("scroll_x", scroll_x),
            ("scroll_y", scroll_y),
            ("v", u32::from(self.v)),
            ("t", t),
            ("fine_x", u32::from(self.x)),
            ("rendering", u32::from(self.rendering())),
        ];
        (true, 0x2000, params, c, raw)
    }

    /// The 64 OAM sprites, eight to a row, as 8×8 or 8×16 (PPUCTRL bit 5)
    /// with their palettes and flips on the backdrop; `index` shows one.
    /// `raw`: the OAM bytes of the sprites shown (4 each).
    #[allow(clippy::type_complexity)]
    fn sprites_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (bool, u32, Vec<(&'static str, u32)>, Canvas, Vec<u8>) {
        let h = self.sprite_height() as u32;
        let index = opts.get("index").map(|i| (i & 63) as usize);
        let list: Vec<usize> = match index {
            Some(i) => vec![i],
            None => (0..64).collect(),
        };
        let table8 = u16::from(self.ctrl & 8) << 9;
        let mut c = Canvas::grid(list.len() as u32, 8, 8, h, 1, GAP);
        let mut raw = Vec::with_capacity(list.len() * 4);
        for (cell, &n) in list.iter().enumerate() {
            let e = &self.oam[n * 4..n * 4 + 4];
            raw.extend_from_slice(e);
            let (tile, attr) = (e[1], e[2]);
            let (x0, y0) = Canvas::cell_origin(cell as u32, 8, 8, h, 1);
            c.fill(x0, y0, 8, h, 0);
            for r in 0..h {
                let row = if attr & 0x80 != 0 { h - 1 - r } else { r } as u16;
                let a = if h == 16 {
                    let t = u16::from(tile & 0xfe) + (row >> 3);
                    u16::from(tile & 1) << 12 | t << 4 | (row & 7)
                } else {
                    table8 | u16::from(tile) << 4 | row
                };
                let (lo, hi) = ((ctx.peek)(a), (ctx.peek)(a + 8));
                for x in 0..8 {
                    let px = if attr & 0x40 != 0 { 7 - x } else { x };
                    let v = Self::tile_pixel(lo, hi, px);
                    if v != 0 {
                        c.set(x0 + x, y0 + r, 16 + (attr & 3) * 4 + v);
                    }
                }
            }
        }
        let mut params = vec![("height", h), ("sprite_table", u32::from(table8))];
        if let Some(n) = index {
            let e = &self.oam[n * 4..n * 4 + 4];
            params.extend([
                ("index", n as u32),
                ("y", u32::from(e[0])),
                ("tile", u32::from(e[1])),
                ("attr", u32::from(e[2])),
                ("x", u32::from(e[3])),
                ("palette", u32::from(e[2] & 3) + 4),
                ("behind", u32::from(e[2] >> 5 & 1)),
                ("flip_h", u32::from(e[2] >> 6 & 1)),
                ("flip_v", u32::from(e[2] >> 7)),
            ]);
        }
        let addr = index.map_or(0, |n| n as u32 * 4);
        (false, addr, params, c, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PPU: MemSpaceId = MemSpaceId(1);
    const OAM: MemSpaceId = MemSpaceId(2);

    /// 8 KB pattern memory, 4 KB of nametables (four-screen).
    fn mem() -> Vec<u8> {
        vec![0; 0x3000]
    }

    fn view(ppu: &Ppu, m: &[u8], id: &str, opts: &GfxOptions) -> GfxView {
        let peek = |a: u16| m[usize::from(a) % m.len()];
        let ctx = GfxContext {
            peek: &peek,
            space: PPU,
            oam_space: OAM,
        };
        ppu.gfx_view(id, &ctx, opts).unwrap()
    }

    #[test]
    fn tiles_follow_the_palette_option() {
        let mut ppu = Ppu::new(PpuModel::Ntsc);
        let mut m = mem();
        m[0x1010] = 0x80; // table 1, tile 1, row 0, plane 0
        m[0x1018] = 0x80; // plane 1: colour 3
        ppu.palette[2 * 4 + 3] = 0x16;
        let before = ppu.clone();
        let t = view(&ppu, &m, "tiles", &GfxOptions::new());
        assert_eq!(ppu, before);
        assert_eq!((t.width, t.height), (258, 128));
        assert_eq!(t.raw.len(), 0x2000);
        assert!(t.name.starts_with("2C02"));
        assert_eq!(t.frame().rgb_at(130 + 8, 0), Some(grey_palette(4)[3]));
        let p = view(&ppu, &m, "tiles", &GfxOptions::new().with("palette", 2));
        assert_eq!(p.param("palette"), Some(2));
        assert_eq!(p.frame().rgb_at(138, 0), Some(ppu.colour(0x16)));
        assert_eq!(
            p.frame().rgb_at(139, 0),
            Some(ppu.colour(ppu.palette_entry(0))),
            "colour 0 is the backdrop"
        );
    }

    #[test]
    fn screen_uses_names_attributes_and_scroll() {
        let mut ppu = Ppu::new(PpuModel::Ntsc);
        ppu.ctrl = 0x10; // background at $1000
        ppu.t = 0x0400 | 2; // nametable 1, coarse x 2
        ppu.x = 3;
        let mut m = mem();
        // Nametable 3 ($2C00), tile (2, 2): name 1, attribute quadrant 3.
        m[0x2c00 + 2 * 32 + 2] = 1;
        m[0x2c00 + 0x3c0] = 0b1100_0000;
        m[0x1010] = 0x80;
        ppu.palette[3 * 4 + 1] = 0x2a;
        let s = view(&ppu, &m, "screen", &GfxOptions::new());
        assert_eq!((s.width, s.height), (512, 480));
        assert_eq!((s.addr, s.raw.len()), (0x2000, 0x1000));
        assert_eq!(s.param("scroll_x"), Some(256 + 16 + 3));
        assert_eq!(s.frame().rgb_at(256 + 16, 240 + 16), Some(ppu.colour(0x2a)));
        assert_eq!(s.raw[0xc00 + 66], 1);
    }

    #[test]
    fn sprites_and_palette() {
        let mut ppu = Ppu::new(PpuModel::Pal);
        ppu.ctrl = 0x20; // 8×16
        ppu.oam[5 * 4..5 * 4 + 4].copy_from_slice(&[10, 0x03, 0x41, 20]); // table 1, tiles 2/3, h-flip, palette 5
        let mut m = mem();
        m[0x1000 + 3 * 16] = 0x80; // bottom tile, row 0, leftmost pixel
        ppu.palette[0x10 + 4 + 1] = 0x21;
        let s = view(&ppu, &m, "sprites", &GfxOptions::new().with("index", 5));
        assert_eq!(s.space, OAM);
        assert_eq!((s.width, s.height), (8, 16));
        assert_eq!(s.raw, [10, 3, 0x41, 20]);
        assert_eq!(s.param("palette"), Some(5));
        assert_eq!(s.param("flip_h"), Some(1));
        assert_eq!(s.frame().rgb_at(7, 8), Some(ppu.colour(0x21)), "flipped");
        let all = view(&ppu, &m, "sprites", &GfxOptions::new());
        assert_eq!(
            (all.width, all.height, all.raw.len()),
            (71, 8 * 17 - 1, 256)
        );
        ppu.palette[0] = 0x0f;
        let p = view(&ppu, &m, "palette", &GfxOptions::new());
        assert_eq!(p.raw.len(), 32);
        assert_eq!(p.raw[0x10], 0x0f, "$3F10 mirrors $3F00");
        assert_eq!(p.frame().rgb_at(0, 0), Some(ppu.colour(0x0f)));
    }
}
