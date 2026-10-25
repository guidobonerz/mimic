//! Graphics data views of the VIC 6560/6561 (see `emu_core::gfx`): the
//! character set, the screen matrix and the palette, decoded from the
//! chip's registers and its view of memory, side-effect free.
//!
//! Layout after the VIC-20 Programmer's Reference Guide, chapter 3 and
//! appendix "The 6560 video interface chip": `$9002` bit 7 is VM9 and bits
//! 0–6 the columns, `$9003` bits 1–6 the rows and bit 0 the 8 × 16
//! characters, `$9005` bits 4–7 VM13–VM10 (screen) and 0–3 CB13–CB10
//! (characters), `$900E` bits 4–7 the auxiliary colour, `$900F` bits 4–7
//! the background, bit 3 normal (1) / reverse (0) and 0–2 the border.
//! Colour RAM bits 0–2 are the character colour, bit 3 selects
//! multicolour: pairs 00 background, 01 border, 10 character colour,
//! 11 auxiliary colour.
//!
//! Addresses in views are CPU addresses: the machine maps between the
//! VIC's 14-bit view and the CPU's map ([`GfxContext::to_cpu`],
//! [`GfxContext::from_cpu`]).

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR,
    OPT_MULTICOLOR,
};
use emu_core::MemSpaceId;

use super::{Vic, VicMemory};

/// Palette index of the gap between cells (one past the 16 colours).
const GAP: u8 = 16;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

const OPT_FG: GfxOptionInfo = GfxOptionInfo {
    name: "fg",
    help: "character colour 0-7 for the character set (default blue, white on a blue background)",
};
const OPT_HEIGHT: GfxOptionInfo = GfxOptionInfo {
    name: "height",
    help: "character height 8 or 16 (default: as $9003 bit 0 sets)",
};
const OPT_COLUMNS: GfxOptionInfo = GfxOptionInfo {
    name: "columns",
    help: "columns of the screen matrix (default: $9002)",
};
const OPT_ROWS: GfxOptionInfo = GfxOptionInfo {
    name: "rows",
    help: "rows of the screen matrix (default: $9003)",
};

/// What the machine tells the decoder besides the chip.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    pub mem: &'a dyn VicMemory,
    pub space: MemSpaceId,
    /// The 16 colours.
    pub palette: &'a [Rgb8],
    /// CPU address of a VIC address (`$0000–$3FFF`).
    pub to_cpu: fn(u16) -> u32,
    /// VIC address of a CPU address (option `addr`).
    pub from_cpu: fn(u32) -> u16,
}

/// The views a VIC offers under the ids `charset`, `screen`, `palette`.
pub fn views(name: &str) -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: format!("{name} character set"),
            options: vec![OPT_ADDR, OPT_MULTICOLOR, OPT_FG, OPT_HEIGHT],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: format!("{name} screen matrix"),
            options: vec![OPT_ADDR, OPT_COLUMNS, OPT_ROWS, OPT_HEIGHT],
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
    /// VIC address of the first raw byte.
    addr: u16,
    params: Vec<(&'static str, u32)>,
    canvas: Canvas,
    raw: Vec<u8>,
}

impl Vic {
    /// Character base as a VIC address (CB13–CB10).
    pub fn char_base(&self) -> u16 {
        self.char_addr()
    }

    fn background(&self) -> u8 {
        self.regs[15] >> 4
    }

    fn border(&self) -> u8 {
        self.regs[15] & 7
    }

    fn aux(&self) -> u8 {
        self.regs[14] >> 4
    }

    fn view_height(&self, opts: &GfxOptions) -> u16 {
        match opts.get("height") {
            Some(h) if h >= 16 => 16,
            Some(_) => 8,
            None => u16::from(self.char_height()),
        }
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
        if kind == GfxKind::Palette {
            let (width, height, pixels) = palette_canvas(&palette, 16, 16);
            return GfxView {
                id,
                kind,
                name,
                space: ctx.space,
                addr: 0,
                params: vec![("count", 16)],
                width,
                height,
                pixels,
                raw: (0..16).collect(),
            };
        }
        palette.push(GAP_COLOR);
        let part = match kind {
            GfxKind::Charset => self.charset_view(ctx, opts),
            _ => self.screen_view(ctx, opts),
        };
        GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr: (ctx.to_cpu)(part.addr),
            params: part.params,
            width: part.canvas.width,
            height: part.canvas.height,
            pixels: part.canvas.into_pixels(palette),
            raw: part.raw,
        }
    }

    /// 256 characters in 32 columns, without reverse mode.
    fn charset_view(&self, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Part {
        let base = opts
            .get("addr")
            .map_or(self.char_addr(), |a| (ctx.from_cpu)(a) & 0x3fff);
        let height = self.view_height(opts);
        let mc = opts.flag("multicolor").unwrap_or(false);
        let bg = self.background();
        let default_fg = if bg == 6 { 1 } else { 6 };
        let fg = opts.get("fg").map_or(default_fg, |f| (f & 7) as u8);
        let (border, aux) = (self.border(), self.aux());
        let mut c = Canvas::grid(256, 32, 8, u32::from(height), 1, GAP);
        let mut raw = Vec::with_capacity(256 * usize::from(height));
        for ch in 0..256u16 {
            let (x0, y0) = Canvas::cell_origin(u32::from(ch), 32, 8, u32::from(height), 1);
            for row in 0..height {
                let v = ctx.mem.read((base + ch * height + row) & 0x3fff);
                raw.push(v);
                let y = y0 + u32::from(row);
                if mc {
                    c.row_2bpp(x0, y, v, [Some(bg), Some(border), Some(fg), Some(aux)], 1);
                } else {
                    c.row_1bpp(x0, y, v, fg, Some(bg), 1);
                }
            }
        }
        let params = vec![
            ("charbase", (ctx.to_cpu)(base)),
            ("height", u32::from(height)),
            ("multicolor", u32::from(mc)),
            ("fg", u32::from(fg)),
            ("bg", u32::from(bg)),
            ("border", u32::from(border)),
            ("aux", u32::from(aux)),
        ];
        Part {
            addr: base,
            params,
            canvas: c,
            raw,
        }
    }

    /// The screen matrix drawn as the chip draws it (colour RAM, multicolour
    /// cells, reverse mode, 8 × 16 characters), without the border. Raw:
    /// the character codes, then their colour nibbles.
    fn screen_view(&self, ctx: &GfxContext<'_>, opts: &GfxOptions) -> Part {
        let base = opts
            .get("addr")
            .map_or(self.screen_addr(), |a| (ctx.from_cpu)(a) & 0x3fff);
        let cols = opts
            .get("columns")
            .map_or(self.columns(), |c| c as u8 & 0x7f)
            .max(1);
        let rows = opts
            .get("rows")
            .map_or(self.rows(), |r| r as u8 & 0x3f)
            .max(1);
        let height = self.view_height(opts);
        let chars = self.char_addr();
        let cells = u16::from(cols) * u16::from(rows);
        let mut c = Canvas::new(
            u32::from(cols) * 8,
            u32::from(rows) * u32::from(height),
            self.background(),
        );
        let mut codes = Vec::with_capacity(usize::from(cells));
        let mut colors = Vec::with_capacity(usize::from(cells));
        for cell in 0..cells {
            let vm = (base + cell) & 0x3fff;
            let code = ctx.mem.read(vm);
            let color = ctx.mem.color(vm & 0x3ff) & 0x0f;
            codes.push(code);
            colors.push(color);
            let x0 = u32::from(cell % u16::from(cols)) * 8;
            let y0 = u32::from(cell / u16::from(cols)) * u32::from(height);
            for line in 0..height {
                let pattern = ctx
                    .mem
                    .read((chars + u16::from(code) * height + line) & 0x3fff);
                for (x, p) in self.character(pattern, color).into_iter().enumerate() {
                    c.set(x0 + x as u32, y0 + u32::from(line), p);
                }
            }
        }
        let params = vec![
            ("screenbase", (ctx.to_cpu)(base)),
            ("colorbase", 0x9400 | u32::from(base & 0x3ff)),
            ("charbase", (ctx.to_cpu)(chars)),
            ("columns", u32::from(cols)),
            ("rows", u32::from(rows)),
            ("height", u32::from(height)),
            ("bg", u32::from(self.background())),
            ("border", u32::from(self.border())),
            ("aux", u32::from(self.aux())),
            ("reverse", u32::from(self.regs[15] & 8 == 0)),
        ];
        codes.extend_from_slice(&colors);
        Part {
            addr: base,
            params,
            canvas: c,
            raw: codes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vic::{palette, Model};

    struct Ram([u8; 0x4000], [u8; 1024]);

    impl VicMemory for Ram {
        fn read(&self, addr: u16) -> u8 {
            self.0[usize::from(addr & 0x3fff)]
        }
        fn color(&self, index: u16) -> u8 {
            self.1[usize::from(index & 0x3ff)]
        }
    }

    /// The VIC-20 map: VIC `$2000–$3FFF` is CPU `$0000–$1FFF`, VIC
    /// `$0000–$1FFF` is CPU `$8000–$9FFF`.
    fn to_cpu(a: u16) -> u32 {
        if a & 0x2000 != 0 {
            u32::from(a & 0x1fff)
        } else {
            0x8000 | u32::from(a & 0x1fff)
        }
    }

    fn from_cpu(a: u32) -> u16 {
        if a & 0x8000 != 0 {
            a as u16 & 0x1fff
        } else {
            0x2000 | (a as u16 & 0x1fff)
        }
    }

    fn ctx<'a>(ram: &'a Ram, pal: &'a [Rgb8]) -> GfxContext<'a> {
        GfxContext {
            mem: ram,
            space: MemSpaceId(0),
            palette: pal,
            to_cpu,
            from_cpu,
        }
    }

    fn view(vic: &Vic, kind: GfxKind, ram: &Ram, opts: &GfxOptions) -> GfxView {
        let pal = palette::palette();
        vic.gfx_view("v", kind, "v".into(), &ctx(ram, &pal), opts)
    }

    /// The PAL KERNAL's set-up: 22 × 23 at `$1E00`, characters at `$8000`.
    fn kernal(vic: &mut Vic) {
        for (reg, value) in [(2, 0x96), (3, 0x2e), (5, 0xf0), (14, 0x20), (15, 0x1b)] {
            vic.write(reg, value);
        }
    }

    #[test]
    fn charset_and_screen_follow_the_registers() {
        let pal = palette::palette();
        let mut vic = Vic::new(Model::Mos6561);
        kernal(&mut vic);
        let mut ram = Ram([0; 0x4000], [0; 1024]);
        ram.0[8] = 0x80; // char 1, row 0, leftmost pixel
        ram.0[0x3e00] = 1; // screen $1E00, first cell: char 1
        ram.1[0x200] = 2; // colour RAM $9600: red
        let cs = view(&vic, GfxKind::Charset, &ram, &GfxOptions::new());
        assert_eq!(cs.addr, 0x8000);
        assert_eq!(cs.param("charbase"), Some(0x8000));
        assert_eq!((cs.width, cs.height), (32 * 9 - 1, 8 * 9 - 1));
        assert_eq!(cs.raw.len(), 2048);
        assert_eq!(cs.frame().rgb_at(9, 0), Some(pal[6]), "char 1 in blue");
        assert_eq!(cs.frame().rgb_at(10, 0), Some(pal[1]), "white background");

        let sc = view(&vic, GfxKind::Screen, &ram, &GfxOptions::new());
        assert_eq!(sc.addr, 0x1e00);
        assert_eq!((sc.width, sc.height), (22 * 8, 23 * 8));
        assert_eq!(sc.param("colorbase"), Some(0x9600));
        assert_eq!(sc.raw.len(), 2 * 22 * 23);
        assert_eq!((sc.raw[0], sc.raw[22 * 23]), (1, 2));
        let f = sc.frame();
        assert_eq!(f.rgb_at(0, 0), Some(pal[2]));
        assert_eq!(f.rgb_at(1, 0), Some(pal[1]));

        // Multicolour cell: pairs 01 border (cyan), 11 auxiliary (red).
        ram.0[8] = 0b0111_0000;
        ram.1[0x200] = 8 | 5;
        let f = view(&vic, GfxKind::Screen, &ram, &GfxOptions::new());
        assert_eq!(f.frame().rgb_at(0, 0), Some(pal[3]));
        assert_eq!(f.frame().rgb_at(2, 0), Some(pal[2]));
        assert_eq!(f.frame().rgb_at(4, 0), Some(pal[1]));
    }

    #[test]
    fn options_browse_memory_and_double_height() {
        let pal = palette::palette();
        let mut vic = Vic::new(Model::Mos6560);
        kernal(&mut vic);
        vic.write(3, 0x2f); // 8 × 16
        let mut ram = Ram([0; 0x4000], [0; 1024]);
        ram.0[0x3000 + 16 + 15] = 0xff; // CPU $1000: char 1, row 15
        let cs = view(
            &vic,
            GfxKind::Charset,
            &ram,
            &GfxOptions::new().with("addr", 0x1000).with("fg", 0),
        );
        assert_eq!(cs.addr, 0x1000);
        assert_eq!(cs.param("height"), Some(16));
        assert_eq!(cs.raw.len(), 4096);
        assert_eq!(cs.frame().rgb_at(9, 15), Some(pal[0]));
        let sc = view(
            &vic,
            GfxKind::Screen,
            &ram,
            &GfxOptions::new().with("rows", 2),
        );
        assert_eq!((sc.width, sc.height), (22 * 8, 32));
        let p = view(&vic, GfxKind::Palette, &ram, &GfxOptions::new());
        assert_eq!(p.frame().rgb_at(17, 0), Some(pal[1]));
        assert_eq!(p.raw.len(), 16);
    }
}
