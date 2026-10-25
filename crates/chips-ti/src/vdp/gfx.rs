//! Graphics data views of the TMS9918A/TMS9929A (see `emu_core::gfx`): the
//! pattern generator table as tiles, the 32 sprites, the name table drawn
//! in the current mode and the 16 colours, decoded from the registers and
//! VRAM, side-effect free (VRAM is read directly, never through the data
//! port, so the address register and the read-ahead buffer stay as they
//! are).
//!
//! Table addresses and modes after the TI data manual, section 2.2
//! ("Registers": R2 name table, R3 colour table, R4 pattern generator, R5
//! sprite attributes, R6 sprite patterns, R7 colours) and section 2.4
//! ("Display modes": Graphics I with one colour byte per eight patterns,
//! Graphics II with three thirds of 256 patterns and a colour byte per
//! pattern line, the R3/R4 masks; Multicolour; Text with 6-pixel
//! columns), sprites after section 2.3 ("Sprites": four attribute bytes,
//! SIZE and MAG in R1, the early-clock bit, the $D0 terminator, the
//! quadrant order of 16×16 patterns).

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR, OPT_COUNT, OPT_INDEX,
};
use emu_core::MemSpaceId;

use super::{Tms9918, VdpModel, PALETTE};

/// Palette index of the gap between cells (one past the 16 colours).
const GAP: u8 = 16;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

/// Grey drawing of patterns without colours: white on black.
const GREY_FG: u8 = 15;
const GREY_BG: u8 = 1;

pub const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "0 Graphics I, 1 Graphics II, 2 Multicolor, 3 Text (default: as the registers select)",
};
pub const OPT_COLOR: GfxOptionInfo = GfxOptionInfo {
    name: "color",
    help: "1 = colours from the colour table (Graphics I/II), 0 = white on black (default: 1 in Graphics I/II)",
};

/// Mode numbers of [`OPT_MODE`].
pub const MODE_G1: u8 = 0;
pub const MODE_G2: u8 = 1;
pub const MODE_MC: u8 = 2;
pub const MODE_TEXT: u8 = 3;

impl VdpModel {
    /// The chip's name for view titles.
    pub fn chip_name(self) -> &'static str {
        match self {
            VdpModel::Ntsc => "TMS9918A",
            VdpModel::Pal => "TMS9929A",
        }
    }
}

/// The views a TMS9918A offers: `tiles`, `sprites`, `screen`, `palette`.
/// Option `addr` is a VRAM address.
pub fn views(name: &str) -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "tiles",
            kind: GfxKind::Tiles,
            name: format!("{name} pattern table"),
            options: vec![OPT_ADDR, OPT_COUNT, OPT_COLOR],
        },
        GfxViewInfo {
            id: "sprites",
            kind: GfxKind::Sprites,
            name: format!("{name} sprites"),
            options: vec![OPT_INDEX, OPT_ADDR],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: format!("{name} name table"),
            options: vec![OPT_ADDR, OPT_MODE],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: format!("{name} colours"),
            options: vec![],
        },
    ]
}

/// What a view returns besides its id and name.
struct Decoded {
    addr: u32,
    params: Vec<(&'static str, u32)>,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    raw: Vec<u8>,
}

impl Tms9918 {
    /// The display mode as the registers select it ([`MODE_G1`] …
    /// [`MODE_TEXT`]), in the order the renderer tests the M bits.
    pub fn display_mode(&self) -> u8 {
        if self.m1() {
            MODE_TEXT
        } else if self.m2() {
            MODE_MC
        } else if self.m3() {
            MODE_G2
        } else {
            MODE_G1
        }
    }

    fn pattern_base(&self) -> usize {
        usize::from(self.regs[4] & 7) << 11
    }

    fn color_base(&self) -> usize {
        usize::from(self.regs[3]) << 6
    }

    fn sprite_attr_base(&self) -> usize {
        usize::from(self.regs[5] & 0x7f) << 7
    }

    fn sprite_pattern_base(&self) -> usize {
        usize::from(self.regs[6] & 7) << 11
    }

    /// Decode view `id` (from [`views`]); `space` is the machine's VRAM
    /// space. None for an unknown id.
    pub fn gfx_view(&self, id: &str, space: MemSpaceId, opts: &GfxOptions) -> Option<GfxView> {
        let info = views(self.model.chip_name())
            .into_iter()
            .find(|v| v.id == id)?;
        let mut palette = PALETTE.to_vec();
        palette.push(GAP_COLOR);
        let d = match info.kind {
            GfxKind::Tiles => self.tiles_view(opts),
            GfxKind::Sprites => self.sprites_view(opts),
            GfxKind::Screen => self.screen_view(opts),
            _ => {
                let (w, h, px) = palette_canvas(&PALETTE, 16, 16);
                return Some(GfxView {
                    id: info.id,
                    kind: info.kind,
                    name: info.name,
                    space,
                    addr: 0,
                    params: vec![
                        ("count", 16),
                        ("backdrop", u32::from(self.regs[7] & 15)),
                        ("text_fg", u32::from(self.regs[7] >> 4)),
                    ],
                    width: w,
                    height: h,
                    pixels: px,
                    // No palette memory: the colours are fixed in the chip.
                    raw: (0..16).collect(),
                });
            }
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space,
            addr: d.addr,
            params: d.params,
            width: d.width,
            height: d.height,
            pixels: GfxPixels::Indexed {
                pixels: d.pixels,
                palette,
            },
            raw: d.raw,
        })
    }

    /// Patterns in rows of 32. By default the table the chip uses now: in
    /// Graphics II all three thirds (768 patterns, through the R3/R4
    /// masks), else 256 from R4. With `addr`, `count` patterns (default
    /// 256) from there, one after the other. `raw` holds the pattern bytes.
    fn tiles_view(&self, opts: &GfxOptions) -> Decoded {
        let mode = self.display_mode();
        let g2 = mode == MODE_G2;
        let colored = opts
            .flag("color")
            .unwrap_or(matches!(mode, MODE_G1 | MODE_G2))
            && matches!(mode, MODE_G1 | MODE_G2);
        let r3 = usize::from(self.regs[3]);
        let r4 = usize::from(self.regs[4]);
        let given = opts.get("addr").map(|a| a as usize & 0x3ff8);
        let base = given.unwrap_or(if g2 {
            (r4 & 4) << 11
        } else {
            self.pattern_base()
        });
        let default_count = if g2 && given.is_none() { 768 } else { 256 };
        let count = opts.get("count").unwrap_or(default_count).clamp(1, 2048) as usize;
        let color_base = if g2 {
            (r3 & 0x80) << 6
        } else {
            self.color_base()
        };
        let mut c = Canvas::grid(count as u32, 32, 8, 8, 1, GAP);
        let mut raw = Vec::with_capacity(count * 8);
        for i in 0..count {
            let (x0, y0) = Canvas::cell_origin(i as u32, 32, 8, 8, 1);
            for row in 0..8 {
                let offset = i << 3 | row;
                let (pa, ca) = if given.is_none() && g2 {
                    // The chip's addressing: third and name through the masks.
                    (
                        (r4 & 4) << 11 | (offset & ((r4 & 3) << 11 | 0x7ff)),
                        (r3 & 0x80) << 6 | (offset & ((r3 & 0x7f) << 6 | 0x3f)),
                    )
                } else if g2 {
                    (base + offset, color_base + offset)
                } else {
                    (base + offset, color_base + (i & 0xff) / 8)
                };
                let pattern = self.vram_at(pa);
                raw.push(pattern);
                let (fg, bg) = if colored {
                    let col = self.vram_at(ca);
                    (col >> 4, col & 15)
                } else {
                    (GREY_FG, GREY_BG)
                };
                c.row_1bpp(x0, y0 + row as u32, pattern, fg, Some(bg), 1);
            }
        }
        let params = vec![
            ("mode", u32::from(mode)),
            ("patterns", base as u32),
            ("colors", color_base as u32),
            ("count", count as u32),
            ("color", u32::from(colored)),
        ];
        Decoded {
            addr: base as u32,
            params,
            width: c.width,
            height: c.height,
            pixels: c.pixels,
            raw,
        }
    }

    /// The 32 sprites of the attribute table (R5, or `addr`), each in a
    /// cell of 8×8 or 16×16 pixels as SIZE selects, unmagnified (MAG is a
    /// parameter), in its colour on the backdrop colour; `index` shows one.
    /// `raw`: the attribute bytes (4 per sprite shown), then each shown
    /// sprite's pattern bytes (8 or 32).
    fn sprites_view(&self, opts: &GfxOptions) -> Decoded {
        let attrs = opts
            .get("addr")
            .map_or(self.sprite_attr_base(), |a| a as usize & 0x3f80);
        let patterns = self.sprite_pattern_base();
        let big = self.regs[1] & 0x02 != 0;
        let mag = self.regs[1] & 0x01 != 0;
        let size: u32 = if big { 16 } else { 8 };
        let index = opts.get("index").map(|i| (i & 31) as usize);
        let list: Vec<usize> = match index {
            Some(i) => vec![i],
            None => (0..32).collect(),
        };
        // Sprites before the $D0 terminator.
        let active = (0..32)
            .find(|&n| self.vram_at(attrs + n * 4) == 0xd0)
            .unwrap_or(32);
        let backdrop = self.regs[7] & 15;
        let mut c = Canvas::grid(list.len() as u32, 8, size, size, 1, GAP);
        let mut raw: Vec<u8> = list
            .iter()
            .flat_map(|&n| (0..4).map(move |k| attrs + n * 4 + k))
            .map(|a| self.vram_at(a))
            .collect();
        for (cell, &n) in list.iter().enumerate() {
            let a = attrs + n * 4;
            let name = usize::from(self.vram_at(a + 2)) & if big { 0xfc } else { 0xff };
            let color = self.vram_at(a + 3) & 15;
            let (x0, y0) = Canvas::cell_origin(cell as u32, 8, size, size, 1);
            c.fill(x0, y0, size, size, backdrop);
            let bytes = if big { 32 } else { 8 };
            for k in 0..bytes {
                let v = self.vram_at(patterns + name * 8 + k);
                raw.push(v);
                // 16×16: bytes 0–15 the left half (rows 0–15), 16–31 the right.
                let (x, y) = ((k / 16) as u32 * 8, (k % 16) as u32);
                c.row_1bpp(x0 + x, y0 + y, v, color, None, 1);
            }
        }
        let mut params = vec![
            ("attrs", attrs as u32),
            ("patterns", patterns as u32),
            ("size", size),
            ("mag", u32::from(mag)),
            ("active", active as u32),
        ];
        if let Some(n) = index {
            let a = attrs + n * 4;
            let ec = self.vram_at(a + 3);
            params.extend([
                ("index", n as u32),
                ("y", u32::from(self.vram_at(a))),
                ("x", u32::from(self.vram_at(a + 1))),
                ("name", u32::from(self.vram_at(a + 2))),
                ("color", u32::from(ec & 15)),
                ("early_clock", u32::from(ec >> 7)),
            ]);
        }
        Decoded {
            addr: attrs as u32,
            params,
            width: c.width,
            height: c.height,
            pixels: c.pixels,
            raw,
        }
    }

    /// The name table (R2, or `addr`) drawn in the current mode (or
    /// `mode`) with the pattern and colour tables, without sprites and
    /// regardless of BLANK: 256 × 192, Text 240 × 192; colour 0 shows the
    /// backdrop. `raw`: the name bytes (768, Text 960).
    fn screen_view(&self, opts: &GfxOptions) -> Decoded {
        let names = opts
            .get("addr")
            .map_or(self.name_base(), |a| a as usize & 0x3c00);
        let mode = opts
            .get("mode")
            .map_or(self.display_mode(), |m| m.min(3) as u8);
        let width: u32 = if mode == MODE_TEXT { 240 } else { 256 };
        let backdrop = self.regs[7] & 15;
        let mut pixels = Vec::with_capacity((width * 192) as usize);
        let mut line = [0u8; 256];
        for y in 0..192u16 {
            match mode {
                MODE_TEXT => self.text_line(names, y, &mut line),
                MODE_MC => self.multicolor_line(names, y, &mut line),
                MODE_G2 => self.graphics2_line(names, y, &mut line),
                _ => self.graphics1_line(names, y, &mut line),
            }
            pixels.extend(
                line[..width as usize]
                    .iter()
                    .map(|&c| if c == 0 { backdrop } else { c }),
            );
        }
        let len = if mode == MODE_TEXT { 960 } else { 768 };
        let raw = (0..len).map(|i| self.vram_at(names + i)).collect();
        let params = vec![
            ("mode", u32::from(mode)),
            ("names", names as u32),
            ("patterns", self.pattern_base() as u32),
            ("colors", self.color_base() as u32),
            ("backdrop", u32::from(backdrop)),
            ("text_fg", u32::from(self.regs[7] >> 4)),
            ("blank", u32::from(self.regs[1] & 0x40 == 0)),
        ];
        Decoded {
            addr: names as u32,
            params,
            width,
            height: 192,
            pixels,
            raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACE: MemSpaceId = MemSpaceId(1);

    fn rgb(v: &GfxView, x: u32, y: u32) -> Option<Rgb8> {
        v.frame().rgb_at(x, y)
    }

    #[test]
    fn graphics1_tiles_and_screen_use_the_tables() {
        let mut vdp = Tms9918::new(VdpModel::Ntsc);
        vdp.regs = [0, 0x40, 0x06, 0x80, 0x00, 0x36, 0x07, 0x04];
        // Pattern 1 row 0 all set; patterns 0–7 colour: fg 10, bg 2.
        vdp.vram[0x0008] = 0xff;
        vdp.vram[0x2000] = 0xa2;
        vdp.vram[0x1800] = 1; // top-left name
        let before = vdp.clone();
        let t = vdp.gfx_view("tiles", SPACE, &GfxOptions::new()).unwrap();
        assert_eq!(vdp, before, "side-effect free");
        assert_eq!((t.width, t.height), (32 * 9 - 1, 8 * 9 - 1));
        assert_eq!(t.raw.len(), 2048);
        assert_eq!(t.param("colors"), Some(0x2000));
        assert_eq!(rgb(&t, 9, 0), Some(PALETTE[10]), "pattern 1 in fg");
        assert_eq!(rgb(&t, 9, 1), Some(PALETTE[2]), "bg");
        assert_eq!(rgb(&t, 8, 0), Some(GAP_COLOR));
        let grey = vdp
            .gfx_view("tiles", SPACE, &GfxOptions::new().with("color", 0))
            .unwrap();
        assert_eq!(rgb(&grey, 9, 0), Some(PALETTE[15]));
        let s = vdp.gfx_view("screen", SPACE, &GfxOptions::new()).unwrap();
        assert_eq!((s.width, s.height), (256, 192));
        assert_eq!(s.addr, 0x1800);
        assert_eq!(s.raw.len(), 768);
        assert_eq!(s.raw[0], 1);
        assert_eq!(rgb(&s, 0, 0), Some(PALETTE[10]));
        assert_eq!(rgb(&s, 0, 1), Some(PALETTE[2]));
        let text = vdp
            .gfx_view("screen", SPACE, &GfxOptions::new().with("mode", 3))
            .unwrap();
        assert_eq!((text.width, text.raw.len()), (240, 960));
    }

    #[test]
    fn graphics2_tiles_show_three_thirds() {
        let mut vdp = Tms9918::new(VdpModel::Pal);
        // M3, pattern table $0000 (R4 = 3), colour table $2000 (R3 = $FF).
        vdp.regs = [0x02, 0x40, 0x06, 0xff, 0x03, 0x36, 0x07, 0x01];
        vdp.vram[0x0800 + 5 * 8] = 0x80; // second third, pattern 5, row 0
        vdp.vram[0x2800 + 5 * 8] = 0x6f; // its colour
        let t = vdp.gfx_view("tiles", SPACE, &GfxOptions::new()).unwrap();
        assert!(t.name.starts_with("TMS9929A"));
        assert_eq!(t.param("count"), Some(768));
        assert_eq!(t.raw.len(), 768 * 8);
        let (x, y) = Canvas::cell_origin(256 + 5, 32, 8, 8, 1);
        assert_eq!(rgb(&t, x, y), Some(PALETTE[6]));
        assert_eq!(rgb(&t, x + 1, y), Some(PALETTE[15]));
    }

    #[test]
    fn sprites_and_palette() {
        let mut vdp = Tms9918::new(VdpModel::Ntsc);
        // 16×16 magnified, attributes $1B00, patterns $3800, backdrop 4.
        vdp.regs = [0, 0x43, 0x06, 0x80, 0x00, 0x36, 0x07, 0x04];
        let a = 0x1b00 + 2 * 4;
        vdp.vram[a..a + 4].copy_from_slice(&[0x10, 0x20, 0x05, 0x89]); // name 5 → 4
        vdp.vram[0x1b00 + 3 * 4] = 0xd0;
        vdp.vram[0x3800 + 4 * 8 + 16] = 0x80; // right half, row 0
        let s = vdp
            .gfx_view("sprites", SPACE, &GfxOptions::new().with("index", 2))
            .unwrap();
        assert_eq!((s.width, s.height), (16, 16));
        assert_eq!(s.raw.len(), 4 + 32);
        assert_eq!(s.param("mag"), Some(1));
        assert_eq!(s.param("early_clock"), Some(1));
        assert_eq!(s.param("color"), Some(9));
        assert_eq!(s.param("active"), Some(3));
        assert_eq!(rgb(&s, 8, 0), Some(PALETTE[9]));
        assert_eq!(rgb(&s, 0, 0), Some(PALETTE[4]), "backdrop");
        let all = vdp.gfx_view("sprites", SPACE, &GfxOptions::new()).unwrap();
        assert_eq!((all.width, all.height), (8 * 17 - 1, 4 * 17 - 1));
        assert_eq!(all.raw.len(), 128 + 32 * 32);
        let p = vdp.gfx_view("palette", SPACE, &GfxOptions::new()).unwrap();
        assert_eq!(rgb(&p, 0, 0), Some(PALETTE[0]));
        assert_eq!(rgb(&p, 17 * 15, 0), Some(PALETTE[15]));
        assert!(vdp.gfx_view("nope", SPACE, &GfxOptions::new()).is_none());
    }
}
