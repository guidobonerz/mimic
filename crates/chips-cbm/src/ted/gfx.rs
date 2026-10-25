//! Graphics data views of the TED 7360/8360 (see `emu_core::gfx`): the
//! character set, the bitmap, the screen (video matrix) and the 128-entry
//! palette, decoded from the chip's registers and its view of memory,
//! side-effect free.
//!
//! Layout after the "TED 7360R0 preliminary data sheet" register map and
//! the Plus/4 Programmer's Reference: `$FF12` bits 3–5 bitmap base
//! (A13–A15) and bit 2 character/bitmap data from ROM, `$FF13` bits 2–7
//! character base (A10–A15), `$FF14` bits 3–7 video matrix (attributes,
//! character codes at `+$400`), `$FF15–$FF18` background colours,
//! `$FF06` ECM/BMM, `$FF07` bit 7 256 characters and bit 4 multicolour.
//! Attribute bytes: bits 0–3 hue, 4–6 luminance, 7 flash. Bitmap colours
//! come from the code byte (hue: high nibble for 0 bits, low nibble for
//! 1 bits) and the attribute byte (their luminances: bits 4–6 and 0–2);
//! multicolour pairs 00 `$FF15`, 01/10 those two, 11 `$FF16`.
//!
//! Flashing characters and the hardware cursor are shown steady (not
//! flashing, no cursor): a view is a picture of memory, not of a frame.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR,
    OPT_COUNT, OPT_MULTICOLOR,
};
use emu_core::MemSpaceId;

use super::{Ted, TedMemory};

/// Palette index of the gap between cells (one past the 128 colours).
const GAP: u8 = 128;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

const OPT_FG: GfxOptionInfo = GfxOptionInfo {
    name: "fg",
    help: "colour 0-127 for the character set (default black, white on a dark background)",
};
const OPT_ROM: GfxOptionInfo = GfxOptionInfo {
    name: "rom",
    help: "1 = character/bitmap data from ROM at $8000-$FFFF, 0 = RAM (default: $FF12 bit 2)",
};
const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "0 text, 1 multicolour text, 4 ECM text (default: as the chip is set)",
};

/// What the machine tells the decoder besides the chip.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    pub mem: &'a dyn TedMemory,
    /// Space of the reported addresses (the CPU's).
    pub space: MemSpaceId,
    /// The 128 colours (`luminance << 4 | hue`).
    pub palette: &'a [Rgb8],
}

/// The views a TED offers under the ids `charset`, `bitmap`, `screen`,
/// `palette`. Option `addr` is a CPU address.
pub fn views(name: &str) -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: format!("{name} character set"),
            options: vec![OPT_ADDR, OPT_COUNT, OPT_MULTICOLOR, OPT_FG, OPT_ROM],
        },
        GfxViewInfo {
            id: "bitmap",
            kind: GfxKind::Bitmap,
            name: format!("{name} bitmap"),
            options: vec![OPT_ADDR, OPT_MULTICOLOR, OPT_ROM],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: format!("{name} video matrix"),
            options: vec![OPT_ADDR, OPT_MODE, OPT_ROM],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: format!("{name} palette"),
            options: vec![],
        },
    ]
}

/// One decoded picture before it becomes a [`GfxView`].
struct Part {
    addr: u16,
    params: Vec<(&'static str, u32)>,
    canvas: Canvas,
    raw: Vec<u8>,
}

impl Ted {
    /// Character base (`$FF13` bits 2–7).
    pub fn char_base(&self) -> u16 {
        u16::from(self.regs[0x13] & 0xfc) << 8
    }

    /// Bitmap base (`$FF12` bits 3–5).
    pub fn bitmap_base(&self) -> u16 {
        u16::from(self.regs[0x12] & 0x38) << 10
    }

    fn bg(&self, n: usize) -> u8 {
        self.regs[0x15 + n] & 0x7f
    }

    fn view_rom(&self, opts: &GfxOptions) -> bool {
        opts.flag("rom").unwrap_or(self.rom_fetch())
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
        palette.truncate(128);
        palette.resize(128, Rgb8::default());
        if kind == GfxKind::Palette {
            let (width, height, pixels) = palette_canvas(&palette, 16, 16);
            return GfxView {
                id,
                kind,
                name,
                space: ctx.space,
                addr: 0,
                params: vec![("count", 128), ("hues", 16), ("luminances", 8)],
                width,
                height,
                pixels,
                raw: (0..128).collect(),
            };
        }
        palette.push(GAP_COLOR);
        let part = match kind {
            GfxKind::Charset => self.charset_view(ctx, opts),
            GfxKind::Bitmap => self.bitmap_view(ctx, opts),
            _ => self.screen_view(ctx, opts),
        };
        GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr: u32::from(part.addr),
            params: part.params,
            width: part.canvas.width,
            height: part.canvas.height,
            pixels: part.canvas.into_pixels(palette),
            raw: part.raw,
        }
    }

    /// 128 characters (256 with `$FF07` bit 7) in 32 columns.
    fn charset_view(&self, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Part {
        let base = opts
            .get("addr")
            .map_or(self.char_base(), |a| a as u16 & 0xfc00);
        let rom = self.view_rom(opts);
        let chip_count = if self.regs[0x07] & 0x80 != 0 {
            256
        } else {
            128
        };
        let count = opts.get("count").map_or(chip_count, |n| n.clamp(1, 256)) as u16;
        let mc = opts
            .flag("multicolor")
            .unwrap_or(self.regs[0x07] & 0x10 != 0);
        let bg0 = self.bg(0);
        // Bright backgrounds (luminance 4–7 of a colour other than black)
        // get black characters, dark ones white.
        let bright = bg0 & 0x0f != 0 && bg0 >> 4 >= 4;
        let fg = opts
            .get("fg")
            .map_or(if bright { 0x00 } else { 0x71 }, |f| (f & 0x7f) as u8);
        let mut c = Canvas::grid(u32::from(count), 32, 8, 8, 1, GAP);
        let mut raw = Vec::with_capacity(usize::from(count) * 8);
        for ch in 0..count {
            let (x0, y0) = Canvas::cell_origin(u32::from(ch), 32, 8, 8, 1);
            for row in 0..8u16 {
                let v = ctx.mem.fetch(base.wrapping_add(ch * 8 + row), rom);
                raw.push(v);
                let y = y0 + u32::from(row);
                if mc {
                    let colors = [bg0, self.bg(1), self.bg(2), fg & 0x77];
                    c.row_2bpp(x0, y, v, colors.map(Some), 1);
                } else {
                    c.row_1bpp(x0, y, v, fg, Some(bg0), 1);
                }
            }
        }
        let params = vec![
            ("charbase", u32::from(base)),
            ("rom", u32::from(rom)),
            ("count", u32::from(count)),
            ("multicolor", u32::from(mc)),
            ("fg", u32::from(fg)),
            ("bg0", u32::from(bg0)),
            ("bg1", u32::from(self.bg(1))),
            ("bg2", u32::from(self.bg(2))),
        ];
        Part {
            addr: base,
            params,
            canvas: c,
            raw,
        }
    }

    /// 320 × 200 bitmap with the video matrix's colours.
    fn bitmap_view(&self, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Part {
        let base = opts
            .get("addr")
            .map_or(self.bitmap_base(), |a| a as u16 & 0xe000);
        let rom = self.view_rom(opts);
        let mc = opts
            .flag("multicolor")
            .unwrap_or(self.regs[0x07] & 0x10 != 0);
        let vm = self.matrix_base();
        let bg0 = self.bg(0);
        let mut c = Canvas::new(320, 200, bg0);
        let mut raw = Vec::with_capacity(8000);
        for cell in 0..1000u16 {
            let (cx, cy) = (u32::from(cell % 40) * 8, u32::from(cell / 40) * 8);
            let attr = ctx.mem.fetch(vm.wrapping_add(cell), false);
            let code = ctx.mem.fetch(vm.wrapping_add(0x400 + cell), false);
            let zero = (code >> 4) | (attr & 0x70);
            let one = (code & 0x0f) | ((attr & 0x07) << 4);
            for row in 0..8u16 {
                let v = ctx.mem.fetch(base.wrapping_add(cell * 8 + row), rom);
                raw.push(v);
                let y = cy + u32::from(row);
                if mc {
                    let colors = [bg0, zero, one, self.bg(1)];
                    c.row_2bpp(cx, y, v, colors.map(Some), 1);
                } else {
                    c.row_1bpp(cx, y, v, one, Some(zero), 1);
                }
            }
        }
        let params = vec![
            ("bitmapbase", u32::from(base)),
            ("matrixbase", u32::from(vm)),
            ("rom", u32::from(rom)),
            ("multicolor", u32::from(mc)),
            ("bg0", u32::from(bg0)),
            ("bg1", u32::from(self.bg(1))),
        ];
        Part {
            addr: base,
            params,
            canvas: c,
            raw,
        }
    }

    /// The 40 × 25 video matrix drawn as text with the current character
    /// set. Raw: the 1000 attribute bytes, then the 1000 character codes.
    fn screen_view(&self, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Part {
        let vm = opts
            .get("addr")
            .map_or(self.matrix_base(), |a| a as u16 & 0xf800);
        let rom = self.view_rom(opts);
        let ecm_on = self.regs[0x06] & 0x40 != 0;
        let chip_mode = if ecm_on {
            4
        } else {
            u32::from(self.regs[0x07] & 0x10 != 0)
        };
        let mode = opts.get("mode").unwrap_or(chip_mode);
        let (ecm, mcm) = (mode == 4, mode == 1);
        let charset = self.char_base();
        let reverse_allowed = self.regs[0x07] & 0x80 == 0 && !ecm;
        let bg0 = self.bg(0);
        let mut c = Canvas::new(320, 200, bg0);
        let mut attrs = Vec::with_capacity(2000);
        let mut codes = Vec::with_capacity(1000);
        for cell in 0..1000u16 {
            let (cx, cy) = (u32::from(cell % 40) * 8, u32::from(cell / 40) * 8);
            let attr = ctx.mem.fetch(vm.wrapping_add(cell), false);
            let code = ctx.mem.fetch(vm.wrapping_add(0x400 + cell), false);
            attrs.push(attr);
            codes.push(code);
            let index = if ecm {
                code & 0x3f
            } else if reverse_allowed {
                code & 0x7f
            } else {
                code
            };
            let fg = attr & 0x7f;
            for row in 0..8u16 {
                let y = cy + u32::from(row);
                let mut pattern = ctx
                    .mem
                    .fetch(charset.wrapping_add(u16::from(index) * 8 + row), rom);
                if reverse_allowed && code & 0x80 != 0 {
                    pattern = !pattern;
                }
                if mcm && attr & 0x08 != 0 {
                    let colors = [bg0, self.bg(1), self.bg(2), fg & 0x77];
                    c.row_2bpp(cx, y, pattern, colors.map(Some), 1);
                } else {
                    let bg = if ecm {
                        self.bg(usize::from(code >> 6))
                    } else {
                        bg0
                    };
                    c.row_1bpp(cx, y, pattern, fg, Some(bg), 1);
                }
            }
        }
        let params = vec![
            ("matrixbase", u32::from(vm)),
            ("charbase", u32::from(charset)),
            ("rom", u32::from(rom)),
            ("mode", mode),
            ("reverse", u32::from(reverse_allowed)),
            ("bg0", u32::from(bg0)),
            ("border", u32::from(self.regs[0x19] & 0x7f)),
        ];
        attrs.extend_from_slice(&codes);
        Part {
            addr: vm,
            params,
            canvas: c,
            raw: attrs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ted::{palette, Model};

    /// 64 KB of RAM; ROM reads (`rom` and `$8000+`) return `$AA`.
    struct Mem(Vec<u8>);

    impl TedMemory for Mem {
        fn fetch(&self, addr: u16, rom: bool) -> u8 {
            if rom && addr >= 0x8000 {
                0xaa
            } else {
                self.0[usize::from(addr)]
            }
        }
    }

    fn view(ted: &Ted, kind: GfxKind, mem: &Mem, opts: &GfxOptions) -> GfxView {
        let pal = palette::palette();
        let ctx = GfxContext {
            mem,
            space: MemSpaceId(0),
            palette: &pal,
        };
        ted.gfx_view("v", kind, "v".into(), &ctx, opts)
    }

    #[test]
    fn screen_and_charset_from_the_registers() {
        let pal = palette::palette();
        let mut ted = Ted::new(Model::Mos8360);
        ted.write(0x14, 0x08); // matrix $0800, codes at $0C00
        ted.write(0x13, 0x10); // characters $1000
        ted.write(0x15, 0x71); // white background
        let mut mem = Mem(vec![0; 0x10000]);
        mem.0[0x1000 + 8] = 0x80; // char 1, row 0
        mem.0[0x0800] = 0x32; // attribute: red, luminance 3
        mem.0[0x0c00] = 1;
        mem.0[0x0c01] = 0x81; // reversed char 1
        mem.0[0x0801] = 0x32;
        let sc = view(&ted, GfxKind::Screen, &mem, &GfxOptions::new());
        assert_eq!((sc.width, sc.height), (320, 200));
        assert_eq!(sc.addr, 0x0800);
        assert_eq!(sc.raw.len(), 2000);
        assert_eq!((sc.raw[0], sc.raw[1000]), (0x32, 1));
        let f = sc.frame();
        assert_eq!(f.rgb_at(0, 0), Some(pal[0x32]));
        assert_eq!(f.rgb_at(1, 0), Some(pal[0x71]));
        assert_eq!(f.rgb_at(8, 0), Some(pal[0x71]), "reversed");
        assert_eq!(f.rgb_at(9, 0), Some(pal[0x32]));

        let cs = view(&ted, GfxKind::Charset, &mem, &GfxOptions::new());
        assert_eq!(cs.param("charbase"), Some(0x1000));
        assert_eq!(cs.param("count"), Some(128));
        assert_eq!(cs.raw.len(), 1024);
        assert_eq!((cs.width, cs.height), (32 * 9 - 1, 4 * 9 - 1));
        assert_eq!(cs.frame().rgb_at(9, 0), Some(pal[0]), "black on white");
        let rom = view(
            &ted,
            GfxKind::Charset,
            &mem,
            &GfxOptions::new().with("addr", 0xd000).with("rom", 1),
        );
        assert_eq!(rom.raw[0], 0xaa);
        assert_eq!(rom.addr, 0xd000);
    }

    #[test]
    fn bitmap_colours_and_palette() {
        let pal = palette::palette();
        let mut ted = Ted::new(Model::Mos8360);
        ted.write(0x14, 0x08);
        ted.write(0x12, 0x08); // bitmap $2000
        let mut mem = Mem(vec![0; 0x10000]);
        mem.0[0x2000] = 0x80;
        mem.0[0x0c00] = 0x25; // hue 2 for 0 bits, hue 5 for 1 bits
        mem.0[0x0800] = 0x63; // luminance 6 for 0 bits, 3 for 1 bits
        let bm = view(&ted, GfxKind::Bitmap, &mem, &GfxOptions::new());
        assert_eq!(bm.param("bitmapbase"), Some(0x2000));
        assert_eq!(bm.raw.len(), 8000);
        assert_eq!(bm.frame().rgb_at(0, 0), Some(pal[0x35]));
        assert_eq!(bm.frame().rgb_at(1, 0), Some(pal[0x62]));
        let p = view(&ted, GfxKind::Palette, &mem, &GfxOptions::new());
        assert_eq!(p.param("count"), Some(128));
        assert_eq!((p.width, p.height), (16 * 17 - 1, 8 * 17 - 1));
        assert_eq!(p.frame().rgb_at(17 * 3, 17 * 7), Some(pal[0x73]));
    }
}
