//! Graphics data views of the Mega Drive VDP (see `mimic_core::gfx`): the
//! patterns as tiles, planes A and B and the window as whole name tables,
//! the sprites in link order and the 64 CRAM colours. Decoded from the
//! registers, VRAM and CRAM directly, so the address register and the read
//! buffer stay as they are.
//!
//! Layouts after MacDonald's *Sega Genesis VDP documentation* (sections
//! 12–15, 17): 4-bit patterns of 32 bytes, name table words
//! `pccvhnnnnnnnnnnn`, plane sizes by register 16, the bases in registers
//! 2, 3, 4 (window: 64 cells wide in H40, 32 in H32), the 8-byte sprite
//! entries with their link field and column-major cells.

use mimic_core::frame::Rgb8;
use mimic_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR, OPT_COUNT, OPT_INDEX, OPT_PALETTE,
};
use mimic_core::MemSpaceId;

use super::{MdVdp, Shade};

/// Palette indices past the 64 CRAM colours.
const GAP: u8 = 64;
const CLEAR: u8 = 65;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

pub const OPT_PLANE: GfxOptionInfo = GfxOptionInfo {
    name: "plane",
    help: "0 plane A, 1 plane B, 2 window (default: the view's own)",
};

/// The views: `tiles`, `plane_a`, `plane_b`, `window`, `sprites`,
/// `palette`. Option `addr` is a VRAM address.
pub fn views() -> Vec<GfxViewInfo> {
    let v = |id, kind, name: &str, options| GfxViewInfo {
        id,
        kind,
        name: format!("VDP {name}"),
        options,
    };
    vec![
        v(
            "tiles",
            GfxKind::Tiles,
            "patterns",
            vec![OPT_ADDR, OPT_COUNT, OPT_PALETTE],
        ),
        v("plane_a", GfxKind::Screen, "plane A", vec![OPT_ADDR]),
        v("plane_b", GfxKind::Screen, "plane B", vec![OPT_ADDR]),
        v("window", GfxKind::Screen, "window", vec![OPT_ADDR]),
        v(
            "sprites",
            GfxKind::Sprites,
            "sprites",
            vec![OPT_INDEX, OPT_ADDR],
        ),
        v("palette", GfxKind::Palette, "colours", vec![]),
    ]
}

struct Decoded {
    space: MemSpaceId,
    addr: u32,
    params: Vec<(&'static str, u32)>,
    width: u32,
    height: u32,
    pixels: GfxPixels,
    raw: Vec<u8>,
}

impl MdVdp {
    /// The 64 CRAM colours, the gap and the transparent colour.
    fn view_palette(&self) -> Vec<Rgb8> {
        let mut p: Vec<Rgb8> = (0..64).map(|i| self.rgb(i, Shade::Normal)).collect();
        p.push(GAP_COLOR);
        p.push(Rgb8::default());
        p
    }

    fn vram_bytes(&self, addr: u32, len: u32) -> Vec<u8> {
        (0..len)
            .map(|i| self.vram[((addr + i) & 0xffff) as usize])
            .collect()
    }

    /// Draw the pattern of name table word `entry` (name, palette line,
    /// flips) at (`x`, `y`); transparent pixels get `clear`.
    fn draw_pattern(&self, c: &mut Canvas, x: u32, y: u32, entry: u16, clear: Option<u8>) {
        let base = u32::from(entry & 0x7ff) * 32;
        let line = (entry >> 13 & 3) as u8;
        for py in 0..8u32 {
            let ry = if entry & 0x1000 != 0 { 7 - py } else { py };
            for px in 0..8u32 {
                let rx = if entry & 0x0800 != 0 { 7 - px } else { px };
                let b = self.vram[((base + ry * 4 + rx / 2) & 0xffff) as usize];
                let idx = if rx & 1 == 0 { b >> 4 } else { b & 15 };
                if idx != 0 {
                    c.set(x + px, y + py, line << 4 | idx);
                } else if let Some(k) = clear {
                    c.set(x + px, y + py, k);
                }
            }
        }
    }

    /// The view `id` with options `opts`; `vram` and `cram` are the
    /// machine's spaces for them.
    pub fn gfx_view(
        &self,
        id: &str,
        opts: &GfxOptions,
        vram: MemSpaceId,
        cram: MemSpaceId,
    ) -> Option<GfxView> {
        let info = views().into_iter().find(|v| v.id == id)?;
        let d = match id {
            "tiles" => self.tiles_view(opts, vram),
            "plane_a" => self.plane_view(0, opts, vram),
            "plane_b" => self.plane_view(1, opts, vram),
            "window" => self.plane_view(2, opts, vram),
            "sprites" => self.sprites_view(opts, vram),
            "palette" => self.palette_view(cram),
            _ => return None,
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: d.space,
            addr: d.addr,
            params: d.params,
            width: d.width,
            height: d.height,
            pixels: d.pixels,
            raw: d.raw,
        })
    }

    fn tiles_view(&self, opts: &GfxOptions, vram: MemSpaceId) -> Decoded {
        let addr = opts.get("addr").unwrap_or(0) & 0xffe0;
        let max = (0x10000 - addr) / 32;
        let count = opts.get("count").unwrap_or(max).clamp(1, max);
        let line = opts.get("palette").unwrap_or(0).min(3) as u8;
        let cols = 32;
        let mut c = Canvas::grid(count, cols, 8, 8, 1, GAP);
        for i in 0..count {
            let (x, y) = Canvas::cell_origin(i, cols, 8, 8, 1);
            let entry = u16::from(line) << 13 | ((addr / 32 + i) & 0x7ff) as u16;
            self.draw_pattern(&mut c, x, y, entry, Some(CLEAR));
        }
        Decoded {
            space: vram,
            addr,
            params: vec![("count", count), ("palette", u32::from(line))],
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(self.view_palette()),
            raw: self.vram_bytes(addr, count * 32),
        }
    }

    /// Base, width and height in cells of plane 0 (A), 1 (B), 2 (window).
    fn plane_layout(&self, plane: u8) -> (u32, u32, u32) {
        let (cols, rows) = self.plane_size();
        match plane {
            0 => (u32::from(self.regs[2] & 0x38) << 10, cols, rows),
            1 => (u32::from(self.regs[4] & 0x07) << 13, cols, rows),
            _ if self.h40() => (u32::from(self.regs[3] & 0x3c) << 10, 64, 32),
            _ => (u32::from(self.regs[3] & 0x3e) << 10, 32, 32),
        }
    }

    fn plane_view(&self, plane: u8, opts: &GfxOptions, vram: MemSpaceId) -> Decoded {
        let plane = opts.get("plane").map_or(plane, |p| p.min(2) as u8);
        let (base, cols, rows) = self.plane_layout(plane);
        let base = opts.get("addr").map_or(base, |a| a & 0xfffe);
        let mut c = Canvas::new(cols * 8, rows * 8, CLEAR);
        let backdrop = self.regs[7] & 63;
        for row in 0..rows {
            for col in 0..cols {
                let a = base + (row * cols + col) * 2;
                let entry = self.word(a as usize);
                self.draw_pattern(&mut c, col * 8, row * 8, entry, Some(backdrop));
            }
        }
        let (hs_a, hs_b) = self.hscroll();
        let vs = u32::from(self.vsram[usize::from(plane == 1)]);
        let mut params = vec![
            ("plane", u32::from(plane)),
            ("width", cols),
            ("height", rows),
        ];
        if plane < 2 {
            params.push(("hscroll", if plane == 0 { hs_a } else { hs_b }));
            params.push(("vscroll", vs));
        }
        Decoded {
            space: vram,
            addr: base,
            params,
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(self.view_palette()),
            raw: self.vram_bytes(base, cols * rows * 2),
        }
    }

    fn sat_base(&self) -> u32 {
        if self.h40() {
            u32::from(self.regs[5] & 0x7e) << 9
        } else {
            u32::from(self.regs[5] & 0x7f) << 9
        }
    }

    /// The sprite numbers in link order from sprite 0.
    fn sprite_order(&self, sat: u32) -> Vec<u32> {
        let max = if self.h40() { 80 } else { 64 };
        let mut order = Vec::new();
        let mut n = 0u32;
        while order.len() < max && !order.contains(&n) {
            order.push(n);
            let link = u32::from(self.vram[((sat + n * 8 + 3) & 0xffff) as usize] & 0x7f);
            if link == 0 || link as usize >= max {
                break;
            }
            n = link;
        }
        order
    }

    fn sprites_view(&self, opts: &GfxOptions, vram: MemSpaceId) -> Decoded {
        let sat = opts.get("addr").map_or(self.sat_base(), |a| a & 0xfff8);
        let order = match opts.get("index") {
            Some(i) => vec![i.min(79)],
            None => self.sprite_order(sat),
        };
        let cols = 8;
        let count = order.len() as u32;
        let mut c = Canvas::grid(count, cols, 32, 32, 1, GAP);
        for (i, &n) in order.iter().enumerate() {
            let e = sat + n * 8;
            let size = self.vram[((e + 2) & 0xffff) as usize];
            let (hs, vs) = (u32::from(size >> 2 & 3) + 1, u32::from(size & 3) + 1);
            let entry = self.word(e as usize + 4);
            let (ox, oy) = Canvas::cell_origin(i as u32, cols, 32, 32, 1);
            c.fill(ox, oy, 32, 32, CLEAR);
            for cx in 0..hs {
                for cy in 0..vs {
                    // Cells are column-major; the flips mirror their places.
                    let dx = if entry & 0x0800 != 0 { hs - 1 - cx } else { cx };
                    let dy = if entry & 0x1000 != 0 { vs - 1 - cy } else { cy };
                    let name = (u32::from(entry & 0x7ff) + cx * vs + cy) & 0x7ff;
                    let cell = (entry & 0xf800) | name as u16;
                    self.draw_pattern(&mut c, ox + dx * 8, oy + dy * 8, cell, None);
                }
            }
        }
        let raw = order
            .iter()
            .flat_map(|&n| self.vram_bytes(sat + n * 8, 8))
            .collect();
        Decoded {
            space: vram,
            addr: sat,
            params: vec![("count", count), ("sat", sat)],
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(self.view_palette()),
            raw,
        }
    }

    fn palette_view(&self, cram: MemSpaceId) -> Decoded {
        let colors: Vec<Rgb8> = (0..64).map(|i| self.rgb(i, Shade::Normal)).collect();
        let (width, height, pixels) = palette_canvas(&colors, 16, 12);
        Decoded {
            space: cram,
            addr: 0,
            params: vec![("colors", 64)],
            width,
            height,
            pixels,
            raw: self.cram.iter().flat_map(|w| w.to_be_bytes()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use mimic_core::gfx::{GfxOptions, GfxPixels, GfxView};
    use mimic_core::MemSpaceId;

    const VRAM: MemSpaceId = MemSpaceId(1);
    const CRAM: MemSpaceId = MemSpaceId(2);

    fn reg(v: &mut MdVdp, r: u8, value: u8) {
        v.write_control(0x8000 | u16::from(r) << 8 | u16::from(value));
    }

    fn command(v: &mut MdVdp, code: u8, addr: u16) {
        v.write_control(u16::from(code & 3) << 14 | (addr & 0x3fff));
        v.write_control(u16::from(code & 0x3c) << 2 | addr >> 14);
    }

    fn vdp() -> MdVdp {
        let mut v = MdVdp::new(false);
        for (r, val) in [
            (2, 0x30),
            (3, 0x2c),
            (4, 0x07),
            (5, 0x54),
            (12, 0x81),
            (15, 2),
            (16, 0x01),
        ] {
            reg(&mut v, r, val);
        }
        // Pattern 1: solid colour 1; CRAM 1 red, 17 green.
        command(&mut v, 1, 32);
        for _ in 0..16 {
            v.write_data(0x1111);
        }
        command(&mut v, 3, 2);
        v.write_data(0x000e);
        command(&mut v, 3, 34);
        v.write_data(0x00e0);
        v
    }

    fn pixel(view: &GfxView, x: u32, y: u32) -> Rgb8 {
        let GfxPixels::Indexed { pixels, palette } = &view.pixels else {
            panic!("indexed")
        };
        palette[usize::from(pixels[(y * view.width + x) as usize])]
    }

    #[test]
    fn tiles_and_the_palette() {
        let v = vdp();
        let t = v
            .gfx_view("tiles", &GfxOptions::new().with("count", 4), VRAM, CRAM)
            .unwrap();
        assert_eq!((t.width, t.height), (35, 8));
        assert_eq!(
            pixel(&t, 9, 0),
            Rgb8 { r: 255, g: 0, b: 0 },
            "tile 1 in palette line 0"
        );
        let t1 = v
            .gfx_view(
                "tiles",
                &GfxOptions::new().with("count", 4).with("palette", 1),
                VRAM,
                CRAM,
            )
            .unwrap();
        assert_eq!(pixel(&t1, 9, 0), Rgb8 { r: 0, g: 255, b: 0 });
        let p = v
            .gfx_view("palette", &GfxOptions::new(), VRAM, CRAM)
            .unwrap();
        assert_eq!(p.raw.len(), 128);
        assert_eq!(p.space, CRAM);
    }

    #[test]
    fn plane_a_the_window_and_the_sprites_in_link_order() {
        let mut v = vdp();
        command(&mut v, 1, 0xc002);
        v.write_data(0x2001); // plane A cell (1, 0): pattern 1, line 1
        let a = v
            .gfx_view("plane_a", &GfxOptions::new(), VRAM, CRAM)
            .unwrap();
        assert_eq!((a.width, a.height), (512, 256), "64 × 32 cells");
        assert_eq!(pixel(&a, 8, 0), Rgb8 { r: 0, g: 255, b: 0 });
        assert_eq!(a.addr, 0xc000);
        let w = v
            .gfx_view("window", &GfxOptions::new(), VRAM, CRAM)
            .unwrap();
        assert_eq!((w.width, w.addr), (512, 0xb000));
        // Sprites 0 → 3 → 1, then the end; sprite 3 is 2 × 2 cells.
        for (n, size, link) in [(0u16, 0u8, 3u8), (3, 0x05, 1), (1, 0, 0)] {
            command(&mut v, 1, 0xa800 + n * 8 + 2);
            v.write_data(u16::from(size) << 8 | u16::from(link));
            v.write_data(0x0001);
        }
        let s = v
            .gfx_view("sprites", &GfxOptions::new(), VRAM, CRAM)
            .unwrap();
        assert_eq!(s.param("count"), Some(3));
        assert_eq!(&s.raw[8..16][2..4], &[0x05, 0x01], "sprite 3 second");
        assert_eq!(
            pixel(&s, 33, 0),
            Rgb8 { r: 255, g: 0, b: 0 },
            "its first cell (pattern 1)"
        );
        let one = v
            .gfx_view("sprites", &GfxOptions::new().with("index", 1), VRAM, CRAM)
            .unwrap();
        assert_eq!(one.param("count"), Some(1));
    }
}
