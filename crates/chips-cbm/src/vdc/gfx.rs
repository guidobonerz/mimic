//! Graphics data views of the VDC 8563 (see `emu_core::gfx`): the
//! character set and the 80-column screen in VDC RAM, and the 16 RGBI
//! colours, decoded from the registers, side-effect free.
//!
//! Layout after the Commodore 128 Programmer's Reference Guide, chapter 10
//! (VDC registers) and the MOS 8563 data sheet: R28 bits 5–7 are the
//! character set address (A13–A15), each character takes 16 bytes (32 when
//! R9 ≥ 16), the attribute's bit 7 selects the second 256 characters;
//! R12/R13 display start, R20/R21 attribute start, R1 columns, R6 rows,
//! R27 the address increment per row, R9 + 1 scan lines per character, R23
//! displayed lines, R25 bit 7 bitmap / bit 6 attributes, R26 the
//! foreground (no attributes) and background colours, R24 bit 6 reverse
//! screen, R29 the underline line. Attribute bits: 7 alternate set,
//! 6 reverse, 5 underline, 4 blink, 0–3 colour.
//!
//! The screen view is drawn the way [`Vdc`]'s own display is (bitmap mode
//! takes the attribute's colour as foreground), but blinking characters are
//! shown steady and the cursor is left out: a view is a picture of memory,
//! not of a frame.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR,
    OPT_COUNT,
};
use emu_core::MemSpaceId;

use super::Vdc;

/// Palette index of the gap between cells (one past the 16 colours).
const GAP: u8 = 16;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

const OPT_FG: GfxOptionInfo = GfxOptionInfo {
    name: "fg",
    help: "foreground colour 0-15 for the character set (default: R26 bits 4-7)",
};
const OPT_HEIGHT: GfxOptionInfo = GfxOptionInfo {
    name: "height",
    help: "pattern lines shown per character, 1-32 (default: R9 + 1)",
};
const OPT_ATTR: GfxOptionInfo = GfxOptionInfo {
    name: "attr",
    help: "attribute start address in VDC RAM (default: R20/R21)",
};
const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "0 text, 1 bitmap (default: R25 bit 7)",
};
const OPT_ATTRIBUTES: GfxOptionInfo = GfxOptionInfo {
    name: "attributes",
    help: "1 = use the attribute bytes, 0 = R26's colours (default: R25 bit 6)",
};
const OPT_COLUMNS: GfxOptionInfo = GfxOptionInfo {
    name: "columns",
    help: "characters per row (default: R1)",
};
const OPT_ROWS: GfxOptionInfo = GfxOptionInfo {
    name: "rows",
    help: "character rows (default: R6)",
};

/// View ids.
pub const CHARSET: &str = "vdc-charset";
pub const SCREEN: &str = "vdc-screen";
pub const PALETTE: &str = "vdc-palette";

/// What the machine tells the decoder besides the chip.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    /// The VDC RAM's memory space.
    pub space: MemSpaceId,
    /// The 16 RGBI colours.
    pub palette: &'a [Rgb8],
}

/// The views a VDC offers: `vdc-charset`, `vdc-screen`, `vdc-palette`.
pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: CHARSET,
            kind: GfxKind::Charset,
            name: "VDC character set".into(),
            options: vec![OPT_ADDR, OPT_COUNT, OPT_FG, OPT_HEIGHT],
        },
        GfxViewInfo {
            id: SCREEN,
            kind: GfxKind::Screen,
            name: "VDC 80-column screen".into(),
            options: vec![
                OPT_ADDR,
                OPT_ATTR,
                OPT_MODE,
                OPT_ATTRIBUTES,
                OPT_COLUMNS,
                OPT_ROWS,
            ],
        },
        GfxViewInfo {
            id: PALETTE,
            kind: GfxKind::Palette,
            name: "VDC palette (RGBI)".into(),
            options: vec![],
        },
    ]
}

impl Vdc {
    /// Character set address (R28 bits 5–7).
    pub fn char_base(&self) -> u16 {
        u16::from(self.regs[28] & 0xe0) << 8
    }

    /// Bytes per character pattern: 16, or 32 with more than 16 lines.
    fn char_bytes(&self) -> u16 {
        if self.regs[9] < 16 {
            16
        } else {
            32
        }
    }

    /// Bitmap mode: the byte of pixel line `line`, column `col`.
    fn bitmap_byte(&self, start: u16, stride: u16, line: u16, col: u16) -> u8 {
        self.vram(
            start
                .wrapping_add(line.wrapping_mul(stride))
                .wrapping_add(col),
        )
    }

    /// Decode view `id` (from [`views`]); `None` for an unknown id.
    pub fn gfx_view(&self, id: &str, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Option<GfxView> {
        let info = views().into_iter().find(|v| v.id == id)?;
        let mut palette = ctx.palette.to_vec();
        palette.truncate(16);
        palette.resize(16, Rgb8::default());
        let (addr, params, canvas, raw) = match info.kind {
            GfxKind::Palette => {
                let (width, height, pixels) = palette_canvas(&palette, 16, 16);
                return Some(GfxView {
                    id: info.id,
                    kind: info.kind,
                    name: info.name,
                    space: ctx.space,
                    addr: 0,
                    params: vec![("count", 16)],
                    width,
                    height,
                    pixels,
                    raw: (0..16).collect(),
                });
            }
            GfxKind::Charset => self.charset_view(opts),
            _ => self.screen_view(opts),
        };
        palette.push(GAP_COLOR);
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: ctx.space,
            addr: u32::from(addr),
            params,
            width: canvas.width,
            height: canvas.height,
            pixels: canvas.into_pixels(palette),
            raw,
        })
    }

    /// Both 256-character sets (512 characters) in 32 columns.
    #[allow(clippy::type_complexity)]
    fn charset_view(&self, opts: &GfxOptions) -> (u16, Vec<(&'static str, u32)>, Canvas, Vec<u8>) {
        let base = opts
            .get("addr")
            .map_or(self.char_base(), |a| a as u16 & 0xe000);
        let bytes = self.char_bytes();
        let count = opts.get("count").map_or(512, |n| n.clamp(1, 512)) as u16;
        let height = opts
            .get("height")
            .map_or(u16::from(self.regs[9]) + 1, |h| h.clamp(1, 32) as u16)
            .min(bytes);
        let fg = opts
            .get("fg")
            .map_or(self.regs[26] >> 4, |f| (f & 15) as u8);
        let bg = self.regs[26] & 0x0f;
        let mut c = Canvas::grid(u32::from(count), 32, 8, u32::from(height), 1, GAP);
        let mut raw = Vec::with_capacity(usize::from(count * bytes));
        for ch in 0..count {
            let (x0, y0) = Canvas::cell_origin(u32::from(ch), 32, 8, u32::from(height), 1);
            for line in 0..bytes {
                let v = self.vram(base.wrapping_add(ch * bytes + line));
                raw.push(v);
                if line < height {
                    c.row_1bpp(x0, y0 + u32::from(line), v, fg, Some(bg), 1);
                }
            }
        }
        let params = vec![
            ("charbase", u32::from(base)),
            ("count", u32::from(count)),
            ("bytesperchar", u32::from(bytes)),
            ("height", u32::from(height)),
            ("fg", u32::from(fg)),
            ("bg", u32::from(bg)),
        ];
        (base, params, c, raw)
    }

    /// The text (or bitmap) screen as the registers lay it out. Raw: the
    /// character codes (bitmap: the bitmap bytes) row by row, then the
    /// attribute bytes when attributes are on.
    #[allow(clippy::type_complexity)]
    fn screen_view(&self, opts: &GfxOptions) -> (u16, Vec<(&'static str, u32)>, Canvas, Vec<u8>) {
        let r = &self.regs;
        let start = opts.get("addr").map_or(self.r16(12), |a| a as u16);
        let attr_start = opts.get("attr").map_or(self.r16(20), |a| a as u16);
        let bitmap = opts.get("mode").map_or(r[25] & 0x80 != 0, |m| m != 0);
        let attrs = opts.flag("attributes").unwrap_or(r[25] & 0x40 != 0);
        let cols = opts
            .get("columns")
            .map_or(r[1], |c| c.min(255) as u8)
            .max(1);
        let rows = opts.get("rows").map_or(r[6], |n| n.min(255) as u8).max(1);
        let stride = u16::from(r[1]).wrapping_add(u16::from(r[27]));
        let lines = u16::from(r[9]) + 1;
        let bytes = self.char_bytes();
        let chars = self.char_base();
        let shown = r[23];
        let bg = r[26] & 0x0f;
        let reverse_screen = r[24] & 0x40 != 0;
        let underline = r[29] & 0x1f;
        let mut c = Canvas::new(u32::from(cols) * 8, u32::from(rows) * u32::from(lines), bg);
        let mut data = Vec::new();
        let mut attr_bytes = Vec::new();
        for row in 0..u16::from(rows) {
            for col in 0..u16::from(cols) {
                let cell = row.wrapping_mul(stride).wrapping_add(col);
                let attr = if attrs {
                    let a = self.vram(attr_start.wrapping_add(cell));
                    attr_bytes.push(a);
                    a
                } else {
                    r[26] >> 4
                };
                let code = u16::from(self.vram(start.wrapping_add(cell)));
                if !bitmap {
                    data.push(code as u8);
                }
                let alt = if attrs && attr & 0x80 != 0 { 256 } else { 0 };
                for sc in 0..lines {
                    let mut pattern = if bitmap {
                        self.bitmap_byte(start, stride, row * lines + sc, col)
                    } else if shown == 0 || sc < u16::from(shown) {
                        self.vram(chars.wrapping_add((code + alt) * bytes + sc))
                    } else {
                        0
                    };
                    if attrs && !bitmap {
                        if attr & 0x20 != 0 && sc == u16::from(underline) {
                            pattern = 0xff;
                        }
                        if attr & 0x40 != 0 {
                            pattern = !pattern;
                        }
                    }
                    if reverse_screen {
                        pattern = !pattern;
                    }
                    let y = u32::from(row * lines + sc);
                    c.row_1bpp(u32::from(col) * 8, y, pattern, attr & 0x0f, Some(bg), 1);
                }
            }
        }
        let params = vec![
            ("screenbase", u32::from(start)),
            ("attrbase", u32::from(attr_start)),
            ("charbase", u32::from(chars)),
            ("mode", u32::from(bitmap)),
            ("attributes", u32::from(attrs)),
            ("columns", u32::from(cols)),
            ("rows", u32::from(rows)),
            ("stride", u32::from(stride)),
            ("charheight", u32::from(lines)),
            ("fg", u32::from(r[26] >> 4)),
            ("bg", u32::from(bg)),
            ("reverse", u32::from(reverse_screen)),
        ];
        if bitmap {
            for line in 0..u16::from(rows) * lines {
                for col in 0..u16::from(cols) {
                    data.push(self.bitmap_byte(start, stride, line, col));
                }
            }
        }
        data.extend_from_slice(&attr_bytes);
        (start, params, c, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vdc::palette;
    use emu_core::frame::VideoStandard;

    fn view(v: &Vdc, id: &str, opts: &GfxOptions) -> GfxView {
        let pal = palette();
        let ctx = GfxContext {
            space: MemSpaceId(5),
            palette: &pal,
        };
        v.gfx_view(id, &ctx, opts).unwrap()
    }

    #[test]
    fn screen_with_attributes_and_charset() {
        let pal = palette();
        let mut v = Vdc::new(16, VideoStandard::Pal);
        // KERNAL layout: screen 0, attributes $0800, characters $2000.
        v.ram[0] = 1;
        v.ram[1] = 1;
        v.ram[0x0800] = 0x0d; // light yellow
        v.ram[0x0801] = 0x40 | 0x03; // reverse, light blue
        v.ram[0x2000 + 16] = 0xf0; // character 1, line 0
        v.ram[0x2000 + 256 * 16 + 16] = 0x01; // alternate character 1
        let sc = view(&v, SCREEN, &GfxOptions::new());
        assert_eq!((sc.width, sc.height), (640, 200));
        assert_eq!(sc.space, MemSpaceId(5));
        assert_eq!(sc.param("attrbase"), Some(0x0800));
        assert_eq!(sc.param("columns"), Some(80));
        assert_eq!(sc.raw.len(), 2 * 80 * 25);
        assert_eq!(sc.raw[80 * 25], 0x0d);
        let f = sc.frame();
        let row: Vec<_> = (0..16).map(|x| f.rgb_at(x, 0).unwrap()).collect();
        assert_eq!(row[0], pal[13]);
        assert_eq!(row[4], pal[0]);
        assert_eq!(row[8], pal[0], "reversed: the set bits show the background");
        assert_eq!(row[12], pal[3]);
        assert!(v
            .gfx_view(
                "nope",
                &GfxContext {
                    space: MemSpaceId(5),
                    palette: &pal
                },
                &GfxOptions::new()
            )
            .is_none());

        let cs = view(&v, CHARSET, &GfxOptions::new());
        assert_eq!(cs.addr, 0x2000);
        assert_eq!(cs.param("height"), Some(8));
        assert_eq!((cs.width, cs.height), (32 * 9 - 1, 16 * 9 - 1));
        assert_eq!(cs.raw.len(), 512 * 16);
        assert_eq!(cs.frame().rgb_at(9, 0), Some(pal[15]), "R26 foreground");
        // Character 257 (the alternate set's 1) is cell 1 of row 8.
        assert_eq!(cs.frame().rgb_at(9 + 7, 8 * 9), Some(pal[15]));
        let p = view(&v, PALETTE, &GfxOptions::new());
        assert_eq!(p.frame().rgb_at(17 * 12, 0), Some(pal[12]));
    }

    #[test]
    fn bitmap_mode_reads_rows_of_bytes() {
        let pal = palette();
        let mut v = Vdc::new(64, VideoStandard::Pal);
        v.write(0, 25);
        v.write(1, 0x80); // bitmap, no attributes
        v.ram[0] = 0x80;
        v.ram[80] = 0x40; // pixel line 1
        let bm = view(&v, SCREEN, &GfxOptions::new());
        assert_eq!(bm.param("mode"), Some(1));
        assert_eq!(bm.raw.len(), 80 * 200);
        assert_eq!(bm.raw[80], 0x40, "in memory order, row by row");
        let f = bm.frame();
        assert_eq!(f.rgb_at(0, 0), Some(pal[15]));
        assert_eq!(f.rgb_at(1, 1), Some(pal[15]));
        assert_eq!(f.rgb_at(1, 0), Some(pal[0]));
    }
}
