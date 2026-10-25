//! Graphics data views of the VIC-II (see `emu_core::gfx`): sprites, the
//! character set, the bitmap, screen memory and the palette, decoded from
//! the chip's registers and its view of memory, side-effect free.
//!
//! Address layout after Bauer, "The MOS 6567/6569 video controller", 3.7
//! (memory pointers $D018: VM13–VM10 screen, CB13–CB11 character base,
//! CB13 bitmap base) and 3.8 (sprite pointers at screen + $3F8, 63 bytes
//! per sprite, 24 × 21 pixels; multicolour pairs: 01 = $D025, 10 = sprite
//! colour, 11 = $D026). Text and bitmap colours as in 3.7.3.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR, OPT_COUNT, OPT_INDEX, OPT_MULTICOLOR,
};
use emu_core::MemSpaceId;

use super::{Vic2, VicMemory};

/// Palette index of the gap between cells (one past the 16 colours).
const GAP: u8 = 16;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

pub const OPT_BANK: GfxOptionInfo = GfxOptionInfo {
    name: "bank",
    help: "VIC bank 0-3 ($0000/$4000/$8000/$C000; default: as CIA2 selects)",
};
const OPT_FG: GfxOptionInfo = GfxOptionInfo {
    name: "fg",
    help: "foreground colour 0-15 for character sets (default 1, white)",
};
const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "0 text, 1 multicolour text, 2 bitmap, 3 multicolour bitmap, 4 ECM text (default: as the chip is set)",
};

/// What the machine tells the decoder besides the chip: where the VIC's
/// bank lies for the CPU and in which space its bytes are reported.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    pub mem: &'a dyn VicMemory,
    /// Bank base address ($0000, $4000, $8000, $C000).
    pub bank_base: u16,
    pub space: MemSpaceId,
    /// The 16 colours.
    pub palette: &'a [Rgb8],
}

/// The views a VIC-II offers under the given ids (sprites, charset,
/// bitmap, screen, palette). Option `addr` is a CPU address: its bank is
/// the machine's to select.
pub fn views(name: &str, prefix_ids: [&'static str; 5]) -> Vec<GfxViewInfo> {
    let [sprites, charset, bitmap, screen, palette] = prefix_ids;
    vec![
        GfxViewInfo {
            id: sprites,
            kind: GfxKind::Sprites,
            name: format!("{name} sprites"),
            options: vec![OPT_INDEX, OPT_ADDR, OPT_COUNT, OPT_MULTICOLOR, OPT_BANK],
        },
        GfxViewInfo {
            id: charset,
            kind: GfxKind::Charset,
            name: format!("{name} character set"),
            options: vec![OPT_ADDR, OPT_MULTICOLOR, OPT_FG, OPT_BANK],
        },
        GfxViewInfo {
            id: bitmap,
            kind: GfxKind::Bitmap,
            name: format!("{name} bitmap"),
            options: vec![OPT_ADDR, OPT_MULTICOLOR, OPT_BANK],
        },
        GfxViewInfo {
            id: screen,
            kind: GfxKind::Screen,
            name: format!("{name} screen memory"),
            options: vec![OPT_ADDR, OPT_MODE, OPT_BANK],
        },
        GfxViewInfo {
            id: palette,
            kind: GfxKind::Palette,
            name: format!("{name} palette"),
            options: vec![],
        },
    ]
}

impl Vic2 {
    /// Screen memory offset in the bank ($D018 bits 4–7).
    pub fn screen_base(&self) -> u16 {
        self.vm_base()
    }

    /// Bitmap base offset in the bank ($D018 bit 3).
    pub fn bitmap_base(&self) -> u16 {
        u16::from(self.mem_ptrs & 0x08) << 10
    }

    /// Decode one view (`kind` from [`views`]).
    pub fn gfx_view(
        &self,
        id: &'static str,
        kind: GfxKind,
        name: String,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> GfxView {
        let mut palette = ctx.palette.to_vec();
        palette.truncate(16);
        palette.resize(16, Rgb8::default());
        palette.push(GAP_COLOR);
        let bank = ctx.bank_base;
        let (offset, params, width, height, pixels, raw) = match kind {
            GfxKind::Sprites => self.sprites_view(ctx, opts),
            GfxKind::Charset => self.charset_view(ctx, opts),
            GfxKind::Bitmap => self.bitmap_view(ctx, opts),
            GfxKind::Palette => {
                let (w, h, px) = palette_canvas(&palette[..16], 16, 16);
                return GfxView {
                    id,
                    kind,
                    name,
                    space: ctx.space,
                    addr: 0,
                    params: vec![("count", 16)],
                    width: w,
                    height: h,
                    pixels: px,
                    raw: (0..16).collect(),
                };
            }
            _ => self.screen_view(ctx, opts),
        };
        let mut params = params;
        params.insert(0, ("bank", u32::from(bank >> 14)));
        GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr: u32::from(bank) + u32::from(offset),
            params,
            width,
            height,
            pixels: GfxPixels::Indexed { pixels, palette },
            raw,
        }
    }

    fn sprite_colors(&self, n: usize, mc: bool) -> [Option<u8>; 4] {
        let c = self.sprites[n].color & 15;
        if mc {
            [
                None,
                Some(self.sprite_mm[0] & 15),
                Some(c),
                Some(self.sprite_mm[1] & 15),
            ]
        } else {
            [None, Some(c), None, None]
        }
    }

    /// Sprites 0–7 from their pointers, or `count` 64-byte blocks from
    /// `addr` drawn with sprite `index`'s colours.
    #[allow(clippy::type_complexity)]
    fn sprites_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u16, Vec<(&'static str, u32)>, u32, u32, Vec<u8>, Vec<u8>) {
        let bg = self.bg[0] & 15;
        let index = opts.get("index").map(|i| (i & 7) as usize);
        // (data offset, sprite whose colours apply)
        let mut list: Vec<(u16, usize)> = Vec::new();
        let start;
        let mut params = Vec::new();
        if let Some(a) = opts.get("addr") {
            let a = a as u16 & 0x3fc0;
            let count = opts.get("count").unwrap_or(64).clamp(1, 256) as u16;
            let n = index.unwrap_or(0);
            for i in 0..count {
                list.push(((a + i * 64) & 0x3fff, n));
            }
            start = a;
            params.push(("count", u32::from(count)));
        } else {
            let ptrs = self.screen_base() + 0x3f8;
            for n in 0..8 {
                if index.is_some_and(|i| i != n) {
                    continue;
                }
                let p = ctx.mem.read(ptrs + n as u16);
                list.push((u16::from(p) << 6, n));
            }
            start = list.first().map_or(0, |l| l.0);
            for (n, names) in SPRITE_PARAMS.iter().enumerate() {
                params.push((names[0], u32::from(ctx.mem.read(ptrs + n as u16))));
            }
            params.push(("enable", u32::from(self.sprite_enable)));
            params.push(("multicolor", u32::from(self.sprite_mc)));
            params.push(("xexpand", u32::from(self.sprite_xe)));
            params.push(("yexpand", u32::from(self.sprite_ye)));
            params.push(("priority", u32::from(self.sprite_prio)));
            for (s, names) in self.sprites.iter().zip(&SPRITE_PARAMS) {
                params.push((names[1], u32::from(s.x)));
                params.push((names[2], u32::from(s.y)));
                params.push((names[3], u32::from(s.color & 15)));
            }
            params.push(("mc0", u32::from(self.sprite_mm[0] & 15)));
            params.push(("mc1", u32::from(self.sprite_mm[1] & 15)));
        }
        let cols = if list.len() <= 8 { 8 } else { 16 };
        let mut c = Canvas::grid(list.len() as u32, cols, 24, 21, 1, GAP);
        let mut raw = Vec::with_capacity(list.len() * 64);
        for (i, &(data, n)) in list.iter().enumerate() {
            let mc = opts
                .flag("multicolor")
                .unwrap_or(self.sprite_mc & (1 << n) != 0);
            let colors = self.sprite_colors(n, mc);
            let (x0, y0) = Canvas::cell_origin(i as u32, cols, 24, 21, 1);
            c.fill(x0, y0, 24, 21, bg);
            for b in 0..64u16 {
                let v = ctx.mem.read((data + b) & 0x3fff);
                raw.push(v);
                if b >= 63 {
                    continue;
                }
                let (x, y) = (x0 + u32::from(b % 3) * 8, y0 + u32::from(b / 3));
                if mc {
                    c.row_2bpp(x, y, v, colors, 1);
                } else {
                    c.row_1bpp(x, y, v, colors[1].unwrap_or(1), None, 1);
                }
            }
        }
        (start, params, c.width, c.height, c.pixels, raw)
    }

    /// 256 characters in 32 columns.
    #[allow(clippy::type_complexity)]
    fn charset_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u16, Vec<(&'static str, u32)>, u32, u32, Vec<u8>, Vec<u8>) {
        let base = opts
            .get("addr")
            .map_or(self.char_base(), |a| a as u16 & 0x3800);
        let mc = opts.flag("multicolor").unwrap_or(self.ctrl2 & 0x10 != 0);
        let fg = opts.get("fg").map_or(1, |f| (f & 15) as u8);
        let bg = self.bg.map(|b| b & 15);
        let mut c = Canvas::grid(256, 32, 8, 8, 1, GAP);
        let mut raw = Vec::with_capacity(2048);
        for ch in 0..256u16 {
            let (x0, y0) = Canvas::cell_origin(u32::from(ch), 32, 8, 8, 1);
            for row in 0..8u16 {
                let v = ctx.mem.read((base + ch * 8 + row) & 0x3fff);
                raw.push(v);
                let y = y0 + u32::from(row);
                if mc {
                    // Multicolour text: colour RAM bit 3 set, pair 11 = colour & 7.
                    c.row_2bpp(
                        x0,
                        y,
                        v,
                        [Some(bg[0]), Some(bg[1]), Some(bg[2]), Some(fg & 7)],
                        1,
                    );
                } else {
                    c.row_1bpp(x0, y, v, fg, Some(bg[0]), 1);
                }
            }
        }
        let params = vec![
            ("charbase", u32::from(base)),
            ("multicolor", u32::from(mc)),
            ("fg", u32::from(fg)),
            ("bg0", u32::from(bg[0])),
            ("bg1", u32::from(bg[1])),
            ("bg2", u32::from(bg[2])),
        ];
        (base, params, c.width, c.height, c.pixels, raw)
    }

    /// 320 × 200 bitmap with the screen memory's (and in multicolour the
    /// colour RAM's) colours.
    #[allow(clippy::type_complexity)]
    fn bitmap_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u16, Vec<(&'static str, u32)>, u32, u32, Vec<u8>, Vec<u8>) {
        let base = opts
            .get("addr")
            .map_or(self.bitmap_base(), |a| a as u16 & 0x2000);
        let mc = opts.flag("multicolor").unwrap_or(self.ctrl2 & 0x10 != 0);
        let screen = self.screen_base();
        let bg0 = self.bg[0] & 15;
        let mut c = Canvas::new(320, 200, bg0);
        let mut raw = Vec::with_capacity(8000);
        for cell in 0..1000u16 {
            let (cx, cy) = (u32::from(cell % 40) * 8, u32::from(cell / 40) * 8);
            let sc = ctx.mem.read(screen + cell);
            let (hi, lo) = (sc >> 4, sc & 15);
            let cram = ctx.mem.color(cell) & 15;
            for row in 0..8u16 {
                let v = ctx.mem.read((base + cell * 8 + row) & 0x3fff);
                raw.push(v);
                if mc {
                    c.row_2bpp(
                        cx,
                        cy + u32::from(row),
                        v,
                        [Some(bg0), Some(hi), Some(lo), Some(cram)],
                        1,
                    );
                } else {
                    c.row_1bpp(cx, cy + u32::from(row), v, hi, Some(lo), 1);
                }
            }
        }
        let params = vec![
            ("bitmapbase", u32::from(base)),
            ("screenbase", u32::from(screen)),
            ("multicolor", u32::from(mc)),
        ];
        (base, params, 320, 200, c.pixels, raw)
    }

    /// The 40 × 25 screen memory drawn as text with the current character
    /// set and colour RAM.
    #[allow(clippy::type_complexity)]
    fn screen_view(
        &self,
        ctx: &GfxContext<'_>,
        opts: &GfxOptions,
    ) -> (u16, Vec<(&'static str, u32)>, u32, u32, Vec<u8>, Vec<u8>) {
        let base = opts
            .get("addr")
            .map_or(self.screen_base(), |a| a as u16 & 0x3c00);
        let ecm = self.ctrl1 & 0x40 != 0;
        let chip_mode = if ecm {
            4
        } else {
            u32::from(self.ctrl2 & 0x10 != 0)
        };
        let mode = opts.get("mode").unwrap_or(chip_mode);
        let chars = self.char_base();
        let bg = self.bg.map(|b| b & 15);
        let mut c = Canvas::new(320, 200, bg[0]);
        let mut raw = Vec::with_capacity(1000);
        for cell in 0..1000u16 {
            let (cx, cy) = (u32::from(cell % 40) * 8, u32::from(cell / 40) * 8);
            let code = ctx.mem.read((base + cell) & 0x3fff);
            raw.push(code);
            let cram = ctx.mem.color(cell) & 15;
            for row in 0..8u16 {
                let y = cy + u32::from(row);
                match mode {
                    1 if cram & 8 != 0 => {
                        let v = ctx.mem.read(chars + u16::from(code) * 8 + row);
                        c.row_2bpp(
                            cx,
                            y,
                            v,
                            [Some(bg[0]), Some(bg[1]), Some(bg[2]), Some(cram & 7)],
                            1,
                        );
                    }
                    4 => {
                        let v = ctx.mem.read(chars + u16::from(code & 0x3f) * 8 + row);
                        c.row_1bpp(cx, y, v, cram, Some(bg[usize::from(code >> 6)]), 1);
                    }
                    _ => {
                        let v = ctx.mem.read(chars + u16::from(code) * 8 + row);
                        c.row_1bpp(
                            cx,
                            y,
                            v,
                            cram & if mode == 1 { 7 } else { 15 },
                            Some(bg[0]),
                            1,
                        );
                    }
                }
            }
        }
        let params = vec![
            ("screenbase", u32::from(base)),
            ("charbase", u32::from(chars)),
            ("mode", mode),
            ("bg0", u32::from(bg[0])),
            ("border", u32::from(self.border_color & 15)),
        ];
        (base, params, 320, 200, c.pixels, raw)
    }
}

/// Parameter names per sprite: pointer, x, y, colour.
const SPRITE_PARAMS: [[&str; 4]; 8] = [
    ["ptr0", "x0", "y0", "color0"],
    ["ptr1", "x1", "y1", "color1"],
    ["ptr2", "x2", "y2", "color2"],
    ["ptr3", "x3", "y3", "color3"],
    ["ptr4", "x4", "y4", "color4"],
    ["ptr5", "x5", "y5", "color5"],
    ["ptr6", "x6", "y6", "color6"],
    ["ptr7", "x7", "y7", "color7"],
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vic2::{palette, Model};

    struct Ram([u8; 0x4000], [u8; 1024]);

    impl VicMemory for Ram {
        fn read(&self, addr: u16) -> u8 {
            self.0[usize::from(addr & 0x3fff)]
        }
        fn color(&self, index: u16) -> u8 {
            self.1[usize::from(index & 0x3ff)]
        }
    }

    fn ctx<'a>(ram: &'a Ram, pal: &'a [Rgb8]) -> GfxContext<'a> {
        GfxContext {
            mem: ram,
            bank_base: 0x4000,
            space: MemSpaceId(1),
            palette: pal,
        }
    }

    #[test]
    fn sprites_follow_their_pointers_and_colours() {
        let pal = palette::palette();
        let mut ram = Ram([0; 0x4000], [0; 1024]);
        let mut vic = Vic2::new(Model::Mos6569);
        vic.write(0x18, 0x10); // screen at $0400
        vic.write(0x2a, 0x07); // sprite 3 yellow
        vic.write(0x21, 0x06); // background blue
        ram.0[0x7f8 + 3] = 0x80; // sprite 3 data at $2000
        ram.0[0x2000] = 0x80; // top-left pixel
        let v = vic.gfx_view(
            "sprites",
            GfxKind::Sprites,
            "s".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new().with("index", 3),
        );
        assert_eq!((v.width, v.height), (24, 21));
        assert_eq!(v.addr, 0x6000, "bank base + sprite data");
        assert_eq!(v.param("bank"), Some(1));
        assert_eq!(v.param("ptr3"), Some(0x80));
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(pal[7]));
        assert_eq!(f.rgb_at(1, 0), Some(pal[6]));
        assert_eq!(v.raw.len(), 64);
        // All eight: a row of eight cells.
        let all = vic.gfx_view(
            "sprites",
            GfxKind::Sprites,
            "s".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new(),
        );
        assert_eq!((all.width, all.height), (8 * 25 - 1, 21));
    }

    #[test]
    fn charset_and_screen_use_the_memory_pointers() {
        let pal = palette::palette();
        let mut ram = Ram([0; 0x4000], [0; 1024]);
        let mut vic = Vic2::new(Model::Mos6569);
        vic.write(0x18, 0x14); // screen $0400, chars $1000
        ram.0[0x1000 + 8] = 0xff; // char 1, row 0
        ram.0[0x0400] = 1; // top-left cell shows char 1
        ram.1[0] = 2; // in red
        let cs = vic.gfx_view(
            "charset",
            GfxKind::Charset,
            "c".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new(),
        );
        assert_eq!(cs.param("charbase"), Some(0x1000));
        assert_eq!(cs.frame().rgb_at(9, 0), Some(pal[1]), "char 1 in white");
        let sc = vic.gfx_view(
            "screen",
            GfxKind::Screen,
            "s".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new(),
        );
        assert_eq!((sc.width, sc.height), (320, 200));
        assert_eq!(sc.frame().rgb_at(0, 0), Some(pal[2]));
        assert_eq!(sc.raw[0], 1);
        let bm = vic.gfx_view(
            "bitmap",
            GfxKind::Bitmap,
            "b".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new(),
        );
        assert_eq!(bm.raw.len(), 8000);
        let p = vic.gfx_view(
            "palette",
            GfxKind::Palette,
            "p".into(),
            &ctx(&ram, &pal),
            &GfxOptions::new(),
        );
        assert_eq!(p.frame().rgb_at(0, 0), Some(pal[0]));
    }
}
