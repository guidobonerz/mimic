//! Graphics data views (see `emu_core::gfx`): the character generator, the
//! text screen, the lo-res and hi-res pages and the 16 lo-res colours,
//! decoded from RAM as stored and the soft switches as they are now, side
//! effect free (no soft switch is touched: the views read the switch state,
//! never the I/O page).
//!
//! **Address layout** (Sather, "Understanding the Apple II", chapter 5,
//! "The Video Scanner"; the scanner's sum in [`crate::video`]): text and
//! lo-res row `r` (0–23) starts at page + (r mod 8)·$80 + (r div 8)·40;
//! hi-res line `y` (0–191) at
//! page + (y mod 8)·$400 + ((y div 8) mod 8)·$80 + (y div 64)·40. Pages: text/lo-res $0400 and $0800, hi-res $2000 and
//! $4000; with 80STORE on PAGE2 selects the memory bank, not the page, so
//! the display stays on page 1.
//!
//! **Modes** (Sather chapters 8 and 9; Apple IIe Technical Reference
//! Manual, chapter 2, "The Video Display"): lo-res shows the low nibble of
//! a byte in the upper and the high nibble in the lower half of a
//! character row (40 × 48 blocks); hi-res shows bits 0–6 of each byte,
//! LSB left (280 × 192), bit 7 shifting the byte half a dot, which the
//! views leave out. The IIe with 80COL (and an auxiliary bank) shows the
//! auxiliary byte before the main byte at the same address: 80 columns of
//! text, and with AN3 off double lo-res (80 × 48) and double hi-res
//! (560 × 192).
//!
//! **Colours.** The machine has no colour palette: colour is what an NTSC
//! monitor makes of the dot pattern (the composite path in `video-crt`).
//! For the views the 16 lo-res colours get the RGB values the Apple IIGS
//! assigns to them (Apple IIGS Hardware Reference, "Video", the 4-bit RGB
//! table of the 16 Apple II colours), an approximation of a monitor's
//! picture. The `color` rendering of hi-res uses the simple rule of the
//! Apple II Reference Manual ("High-Resolution Graphics"): two adjacent lit
//! dots are white, a lone lit dot is violet (even column) or green (odd
//! column) with bit 7 clear, blue or orange with bit 7 set; an unlit dot is
//! black. Double hi-res colour takes four dots at a time as a lo-res
//! colour index (dot `k` of the group = bit `k`, the phase the scanner
//! keeps, see [`crate::video`]). Double lo-res shows the auxiliary nibbles
//! as their stored colour index (the real picture rotates their colours).

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo,
    OPT_ADDR,
};

use crate::mem::Aux;
use crate::{Apple2, CPU_SPACE, MAIN_SPACE};

/// The 16 lo-res colours, from the Apple IIGS's 4-bit RGB values (see
/// the module docs): black, magenta, dark blue, purple, dark green, grey 1,
/// medium blue, light blue, brown, orange, grey 2, pink, light green,
/// yellow, aquamarine, white.
pub const LORES_RGB4: [u16; 16] = [
    0x000, 0xd03, 0x009, 0xd2d, 0x072, 0x555, 0x22f, 0x6af, 0x850, 0xf60, 0xaaa, 0xf98, 0x1d0,
    0xff0, 0x4f9, 0xfff,
];

/// The lo-res colours as RGB.
pub fn lores_palette() -> [Rgb8; 16] {
    LORES_RGB4.map(|c| Rgb8 {
        r: ((c >> 8) & 15) as u8 * 17,
        g: ((c >> 4) & 15) as u8 * 17,
        b: (c & 15) as u8 * 17,
    })
}

const BLACK: u8 = 0;
const WHITE: u8 = 15;
const PURPLE: u8 = 3;
const BLUE: u8 = 6;
const ORANGE: u8 = 9;
const GREEN: u8 = 12;
/// Palette index of the gap between cells (one past the 16 colours).
const GAP: u8 = 16;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

const OPT_PAGE: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "display page 1 or 2 (default: as PAGE2 and 80STORE select)",
};
const OPT_CHARSET: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "IIe: 0 primary, 1 alternate character set (default: as ALTCHARSET is set)",
};
const OPT_COLUMNS: GfxOptionInfo = GfxOptionInfo {
    name: "columns",
    help: "40 or 80 (IIe with auxiliary memory; default: as 80COL is set)",
};
const OPT_DOUBLE: GfxOptionInfo = GfxOptionInfo {
    name: "double",
    help: "1 = double lo-res/hi-res from main + aux (default: as 80COL and AN3 select)",
};
const OPT_COLOR: GfxOptionInfo = GfxOptionInfo {
    name: "color",
    help: "1 = simple NTSC colour rule, 0 = the dots in black and white (default 0)",
};

/// Text/lo-res row `r` (0–23) offset in the page.
pub fn text_row(r: u32) -> u32 {
    (r % 8) * 0x80 + (r / 8) * 40
}

/// Hi-res line `y` (0–191) offset in the page.
pub fn hires_line(y: u32) -> u32 {
    (y % 8) * 0x400 + ((y / 8) % 8) * 0x80 + (y / 64) * 40
}

impl Apple2 {
    pub(crate) fn views(&self) -> Vec<GfxViewInfo> {
        let iie = self.model.iie();
        let mut charset_opts = vec![];
        if iie {
            charset_opts.push(OPT_CHARSET);
        }
        let mut text_opts = vec![OPT_PAGE, OPT_ADDR];
        let mut gfx_opts = vec![OPT_PAGE, OPT_ADDR];
        if self.board.mem.aux_kind != Aux::None {
            text_opts.push(OPT_COLUMNS);
            gfx_opts.push(OPT_DOUBLE);
        }
        let mut hires_opts = gfx_opts.clone();
        hires_opts.push(OPT_COLOR);
        vec![
            GfxViewInfo {
                id: "charset",
                kind: GfxKind::Charset,
                name: "Apple II character generator".into(),
                options: charset_opts,
            },
            GfxViewInfo {
                id: "screen",
                kind: GfxKind::Screen,
                name: "Apple II text page".into(),
                options: text_opts,
            },
            GfxViewInfo {
                id: "lores",
                kind: GfxKind::Bitmap,
                name: "Apple II lo-res page".into(),
                options: gfx_opts,
            },
            GfxViewInfo {
                id: "hires",
                kind: GfxKind::Bitmap,
                name: "Apple II hi-res page".into(),
                options: hires_opts,
            },
            GfxViewInfo {
                id: "palette",
                kind: GfxKind::Palette,
                name: "Apple II lo-res colours".into(),
                options: vec![],
            },
        ]
    }

    /// Whether the auxiliary bank holds `addr` (the 1 KB card: the text
    /// page only).
    fn aux_has(&self, addr: u32) -> bool {
        match self.board.mem.aux_kind {
            Aux::None => false,
            Aux::Text1k => (0x400..0x800).contains(&addr),
            Aux::Ext64k => true,
        }
    }

    fn main_byte(&self, addr: u32) -> u8 {
        self.board.mem.main[(addr & 0xffff) as usize]
    }

    fn aux_byte(&self, addr: u32) -> u8 {
        let a = addr & 0xffff;
        if self.aux_has(a) {
            self.board.mem.aux[a as usize]
        } else {
            0
        }
    }

    /// Page base: option `index` (1 or 2) or `addr`, else as the scanner
    /// shows it now. Returns (base, page number or 0 for an address).
    fn page_base(&self, opts: &GfxOptions, page1: u32) -> (u32, u32) {
        if let Some(a) = opts.get("addr") {
            return (a & 0xffff, 0);
        }
        let m = &self.board.modes;
        let page = opts
            .get("index")
            .map_or(if m.page2 && !m.store80 { 2 } else { 1 }, |i| {
                if i == 2 {
                    2
                } else {
                    1
                }
            });
        (page1 * page, page)
    }

    /// 80 columns / double graphics possible and selected.
    fn double(&self, opts: &GfxOptions, name: &str, now: bool) -> bool {
        let has_aux = self.board.mem.aux_kind != Aux::None;
        let want = match name {
            "columns" => opts.get("columns").map(|c| c == 80),
            _ => opts.flag(name),
        };
        has_aux && want.unwrap_or(now)
    }

    fn palette16(&self) -> Vec<Rgb8> {
        let mut p = lores_palette().to_vec();
        p.push(GAP_COLOR);
        p
    }

    pub(crate) fn view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = self.views().into_iter().find(|v| v.id == id)?;
        let (space, addr, params, w, h, pixels, raw) = match id {
            "charset" => self.charset_view(opts),
            "screen" => self.screen_view(opts),
            "lores" => self.lores_view(opts),
            "hires" => self.hires_view(opts),
            _ => {
                let (w, h, px) = palette_canvas(&lores_palette(), 16, 16);
                return Some(GfxView {
                    id: info.id,
                    kind: info.kind,
                    name: info.name,
                    space: CPU_SPACE,
                    addr: 0,
                    params: vec![("count", 16)],
                    width: w,
                    height: h,
                    pixels: px,
                    raw: (0..16).collect(),
                });
            }
        };
        Some(GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space,
            addr,
            params,
            width: w,
            height: h,
            pixels: GfxPixels::Indexed {
                pixels,
                palette: self.palette16(),
            },
            raw,
        })
    }

    /// 256 codes, 16 to a row, 7 × 8 dots each, as the generator and the
    /// video circuit show them now (inverse, flashing in its current phase,
    /// the IIe's set). The generator is in no address space: `raw` holds
    /// its first 2 KB (the part the video uses), or without a ROM image the
    /// rows of this project's font (bit 0 the leftmost dot).
    #[allow(clippy::type_complexity)]
    fn charset_view(
        &self,
        opts: &GfxOptions,
    ) -> (
        emu_core::MemSpaceId,
        u32,
        Vec<(&'static str, u32)>,
        u32,
        u32,
        Vec<u8>,
        Vec<u8>,
    ) {
        let v = &self.board.video;
        let iie = self.model.iie();
        let altchar = iie
            && opts
                .get("index")
                .map_or(self.board.modes.altchar, |i| i != 0);
        let mut c = Canvas::grid(256, 16, 7, 8, 1, GAP);
        for code in 0..256u32 {
            let (x0, y0) = Canvas::cell_origin(code, 16, 7, 8, 1);
            for row in 0..8u32 {
                let dots = v.char_dots(code as u8, row as u8, altchar);
                for x in 0..7 {
                    let on = dots >> x & 1 != 0;
                    c.set(x0 + x, y0 + row, if on { WHITE } else { BLACK });
                }
            }
        }
        let raw = match &v.char_rom {
            Some(rom) => rom.iter().take(0x800).copied().collect(),
            None => (0..0x800u32)
                .map(|i| crate::font::row((i / 8) as u8 & 0x7f, (i % 8) as u8))
                .collect(),
        };
        let params = vec![
            ("rom", u32::from(v.char_rom.is_some())),
            ("alternate", u32::from(altchar)),
            ("flash", u32::from(v.flash)),
        ];
        (CPU_SPACE, 0, params, c.width, c.height, c.pixels, raw)
    }

    /// The 40 × 24 (or 80 × 24) text page drawn with the character
    /// generator: 280 (560) × 192.
    #[allow(clippy::type_complexity)]
    fn screen_view(
        &self,
        opts: &GfxOptions,
    ) -> (
        emu_core::MemSpaceId,
        u32,
        Vec<(&'static str, u32)>,
        u32,
        u32,
        Vec<u8>,
        Vec<u8>,
    ) {
        let m = &self.board.modes;
        let (base, page) = self.page_base(opts, 0x400);
        let col80 = self.double(opts, "columns", m.col80);
        let cols = if col80 { 80 } else { 40 };
        let mut c = Canvas::new(cols * 7, 192, BLACK);
        let v = &self.board.video;
        for row in 0..24u32 {
            for col in 0..cols {
                let (a, aux) = if col80 {
                    (base + text_row(row) + col / 2, col % 2 == 0)
                } else {
                    (base + text_row(row) + col, false)
                };
                let code = if aux {
                    self.aux_byte(a)
                } else {
                    self.main_byte(a)
                };
                for r in 0..8u32 {
                    let dots = v.char_dots(code, r as u8, m.altchar);
                    for x in 0..7 {
                        let on = dots >> x & 1 != 0;
                        c.set(col * 7 + x, row * 8 + r, if on { WHITE } else { BLACK });
                    }
                }
            }
        }
        let mut raw: Vec<u8> = (0..0x400).map(|i| self.main_byte(base + i)).collect();
        if col80 {
            raw.extend((0..0x400).map(|i| self.aux_byte(base + i)));
        }
        let params = vec![
            ("page", page),
            ("base", base),
            ("columns", cols),
            ("alternate", u32::from(m.altchar)),
            ("flash", u32::from(v.flash)),
        ];
        (MAIN_SPACE, base, params, c.width, c.height, c.pixels, raw)
    }

    /// 40 × 48 (double: 80 × 48) blocks of 7 × 4 dots: 280 (560) × 192.
    #[allow(clippy::type_complexity)]
    fn lores_view(
        &self,
        opts: &GfxOptions,
    ) -> (
        emu_core::MemSpaceId,
        u32,
        Vec<(&'static str, u32)>,
        u32,
        u32,
        Vec<u8>,
        Vec<u8>,
    ) {
        let m = &self.board.modes;
        let (base, page) = self.page_base(opts, 0x400);
        let double = self.double(opts, "double", m.col80 && m.dhires);
        let cols = if double { 80 } else { 40 };
        let mut c = Canvas::new(cols * 7, 192, BLACK);
        for row in 0..24u32 {
            for col in 0..cols {
                let a = base + text_row(row) + if double { col / 2 } else { col };
                let byte = if double && col % 2 == 0 {
                    self.aux_byte(a)
                } else {
                    self.main_byte(a)
                };
                c.fill(col * 7, row * 8, 7, 4, byte & 15);
                c.fill(col * 7, row * 8 + 4, 7, 4, byte >> 4);
            }
        }
        let mut raw: Vec<u8> = (0..0x400).map(|i| self.main_byte(base + i)).collect();
        if double {
            raw.extend((0..0x400).map(|i| self.aux_byte(base + i)));
        }
        let params = vec![
            ("page", page),
            ("base", base),
            ("double", u32::from(double)),
            ("blocks_x", cols),
            ("blocks_y", 48),
        ];
        (MAIN_SPACE, base, params, c.width, c.height, c.pixels, raw)
    }

    /// 280 × 192 (double: 560 × 192) dots, monochrome or in colour.
    #[allow(clippy::type_complexity)]
    fn hires_view(
        &self,
        opts: &GfxOptions,
    ) -> (
        emu_core::MemSpaceId,
        u32,
        Vec<(&'static str, u32)>,
        u32,
        u32,
        Vec<u8>,
        Vec<u8>,
    ) {
        let m = &self.board.modes;
        let (base, page) = self.page_base(opts, 0x2000);
        let double = self.double(opts, "double", m.col80 && m.dhires);
        let color = opts.flag("color").unwrap_or(false);
        let width = if double { 560 } else { 280 };
        let mut c = Canvas::new(width, 192, BLACK);
        for y in 0..192u32 {
            let line = base + hires_line(y);
            // The line's dots and, per dot, its byte's bit 7.
            let mut dots = Vec::with_capacity(width as usize);
            let mut high = Vec::with_capacity(width as usize);
            for col in 0..40 {
                let a = line + col;
                let bytes: &[u8] = if double {
                    &[self.aux_byte(a), self.main_byte(a)]
                } else {
                    &[self.main_byte(a)]
                };
                for &b in bytes {
                    for bit in 0..7 {
                        dots.push(b >> bit & 1 != 0);
                        high.push(b & 0x80 != 0);
                    }
                }
            }
            for x in 0..width as usize {
                let px = if !color {
                    if dots[x] {
                        WHITE
                    } else {
                        BLACK
                    }
                } else if double {
                    let g = x & !3;
                    (0..4).fold(0, |acc, k| acc | u8::from(dots[g + k]) << k)
                } else if !dots[x] {
                    BLACK
                } else if (x > 0 && dots[x - 1]) || dots.get(x + 1) == Some(&true) {
                    WHITE
                } else {
                    match (high[x], x % 2 == 0) {
                        (false, true) => PURPLE,
                        (false, false) => GREEN,
                        (true, true) => BLUE,
                        (true, false) => ORANGE,
                    }
                };
                c.set(x as u32, y, px);
            }
        }
        let mut raw: Vec<u8> = (0..0x2000).map(|i| self.main_byte(base + i)).collect();
        if double {
            raw.extend((0..0x2000).map(|i| self.aux_byte(base + i)));
        }
        let params = vec![
            ("page", page),
            ("base", base),
            ("double", u32::from(double)),
            ("color", u32::from(color)),
        ];
        (MAIN_SPACE, base, params, c.width, c.height, c.pixels, raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;
    use emu_core::Machine;

    fn machine(model: &str) -> Apple2 {
        let mut config = MachineConfig {
            model: Some(model.into()),
            ..MachineConfig::default()
        };
        config.options.insert("roms".into(), "stub".into());
        Apple2::new(&config).unwrap()
    }

    fn rgb(v: &GfxView, x: u32, y: u32) -> Rgb8 {
        v.frame().rgb_at(x, y).unwrap()
    }

    #[test]
    fn row_layout_follows_the_scanner() {
        assert_eq!(text_row(1), 0x80);
        assert_eq!(text_row(8), 40);
        assert_eq!(text_row(23), 7 * 0x80 + 80);
        assert_eq!(hires_line(1), 0x400);
        assert_eq!(hires_line(8), 0x80);
        assert_eq!(hires_line(64), 40);
        assert_eq!(hires_line(191), 0x1c00 + 0x380 + 80);
    }

    #[test]
    fn views_are_listed() {
        let m = machine("iiplus");
        let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
        assert_eq!(ids, ["charset", "screen", "lores", "hires", "palette"]);
        assert!(m.gfx_view("nope", &GfxOptions::new()).is_none());
    }

    #[test]
    fn charset_shows_inverse_and_normal_codes() {
        let m = machine("iiplus");
        let v = m.gfx_view("charset", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (16 * 8 - 1, 16 * 9 - 1));
        assert_eq!(v.raw.len(), 0x800);
        assert_eq!(v.param("rom"), Some(0));
        // Code $A0 (space, normal) is black, code $20 (space, inverse) white.
        let (x, y) = Canvas::cell_origin(0xa0, 16, 7, 8, 1);
        assert_eq!(rgb(&v, x, y), lores_palette()[0]);
        let (x, y) = Canvas::cell_origin(0x20, 16, 7, 8, 1);
        assert_eq!(rgb(&v, x, y), lores_palette()[15]);
        let e = machine("iie");
        let alt = e
            .gfx_view("charset", &GfxOptions::new().with("index", 1))
            .unwrap();
        assert_eq!(alt.param("alternate"), Some(1));
    }

    #[test]
    fn screen_reads_the_text_page() {
        let mut m = machine("iie");
        m.board.mem.main.fill(0xa0);
        m.board.mem.aux.fill(0xa0);
        m.board.mem.main[0x400] = 0x20; // inverse space, top left
        m.board.mem.main[0x800 + 0x80] = 0x20; // page 2, row 1
        m.board.mem.aux[0x400] = 0x20; // 80 columns: aux first
        let v = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (280, 192));
        assert_eq!(v.addr, 0x400);
        assert_eq!(v.space, MAIN_SPACE);
        assert_eq!(v.param("page"), Some(1));
        assert_eq!(v.raw.len(), 0x400);
        assert_eq!(rgb(&v, 0, 0), lores_palette()[15]);
        assert_eq!(rgb(&v, 7, 0), lores_palette()[0]);
        let p2 = m
            .gfx_view("screen", &GfxOptions::new().with("index", 2))
            .unwrap();
        assert_eq!(p2.addr, 0x800);
        assert_eq!(rgb(&p2, 0, 8), lores_palette()[15]);
        // PAGE2 on without 80STORE: the default follows it.
        m.board.modes.page2 = true;
        let d = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!(d.param("page"), Some(2));
        m.board.modes.store80 = true;
        let d = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!(d.param("page"), Some(1), "80STORE: PAGE2 banks memory");
        m.board.modes.col80 = true;
        let w = m.gfx_view("screen", &GfxOptions::new()).unwrap();
        assert_eq!((w.width, w.param("columns")), (560, Some(80)));
        assert_eq!(w.raw.len(), 0x800);
        assert_eq!(rgb(&w, 0, 0), lores_palette()[15], "aux column 0");
        assert_eq!(rgb(&w, 7, 0), lores_palette()[15], "main column 1");
        assert_eq!(rgb(&w, 14, 0), lores_palette()[0], "aux column 2");
    }

    #[test]
    fn lores_blocks_take_their_nibbles() {
        let mut m = machine("iiplus");
        m.board.mem.main[0x400 + 0x80 + 3] = 0x9d; // row 1, column 3
        let v = m.gfx_view("lores", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (280, 192));
        let pal = lores_palette();
        assert_eq!(rgb(&v, 3 * 7, 8), pal[0xd], "upper block: low nibble");
        assert_eq!(rgb(&v, 3 * 7 + 6, 12), pal[9], "lower block: high nibble");
        assert_eq!(v.raw.len(), 0x400);
        assert_eq!(v.param("double"), Some(0));
    }

    #[test]
    fn hires_mono_and_colour() {
        let mut m = machine("iie");
        m.board.mem.main[0x2000] = 0x01; // dot 0 alone: violet
        m.board.mem.main[0x2001] = 0x82; // dot 8 alone, bit 7: orange
        m.board.mem.main[0x2400] = 0x03; // line 1, dots 0-1: white
        let v = m.gfx_view("hires", &GfxOptions::new()).unwrap();
        assert_eq!((v.width, v.height), (280, 192));
        assert_eq!(v.raw.len(), 0x2000);
        assert_eq!(v.addr, 0x2000);
        let pal = lores_palette();
        assert_eq!(rgb(&v, 0, 0), pal[15]);
        assert_eq!(rgb(&v, 1, 0), pal[0]);
        let c = m
            .gfx_view("hires", &GfxOptions::new().with("color", 1))
            .unwrap();
        assert_eq!(rgb(&c, 0, 0), pal[PURPLE as usize]);
        assert_eq!(rgb(&c, 8, 0), pal[BLUE as usize], "even column, bit 7");
        assert_eq!(rgb(&c, 0, 1), pal[15]);
        assert_eq!(rgb(&c, 1, 1), pal[15]);
        let p2 = m
            .gfx_view("hires", &GfxOptions::new().with("index", 2))
            .unwrap();
        assert_eq!(p2.addr, 0x4000);
        // Double hi-res: aux byte first.
        m.board.modes.col80 = true;
        m.board.modes.dhires = true;
        m.board.mem.aux[0x2000] = 0x0f; // dots 0-3: colour 15
        let d = m
            .gfx_view("hires", &GfxOptions::new().with("color", 1))
            .unwrap();
        assert_eq!((d.width, d.param("double")), (560, Some(1)));
        assert_eq!(d.raw.len(), 0x4000);
        assert_eq!(rgb(&d, 0, 0), pal[15]);
        assert_eq!(
            rgb(&d, 7, 0),
            pal[8],
            "main byte's dot 0 at dot 7: bit 3 of its group"
        );
    }

    #[test]
    fn palette_has_sixteen_colours() {
        let m = machine("ii");
        let v = m.gfx_view("palette", &GfxOptions::new()).unwrap();
        assert_eq!(v.param("count"), Some(16));
        assert_eq!(rgb(&v, 0, 0), Rgb8 { r: 0, g: 0, b: 0 });
        assert_eq!(
            rgb(&v, 15 * 17, 0),
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(v.raw.len(), 16);
    }
}
