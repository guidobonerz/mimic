//! Graphics data views of the PET (see `emu_core::gfx`): the character
//! ROM and the text screen in video RAM, decoded the way [`crate::video`]
//! draws them, side-effect free.
//!
//! - Character ROM (2 KB, its own memory space `chargen`): two sets of 128
//!   characters, set 0 graphics (VIA CA2 low), set 1 text (CA2 high), 8
//!   bytes each; code bit 7 shows the character inverted.
//! - Screen: the CRTC (6545 on the 4032/8032; the 2001/3032's timing chain
//!   with the same geometry) shows R6 rows of R1 characters (two per clock
//!   on the 8032) from R12/R13 on, R9 + 1 lines per row (lines 8 and 9
//!   blank); MA12 low inverts the whole screen.
//!
//! Sources: Commodore PET user manuals and schematics (character ROM
//! wiring, CA2 character set select), MOS 6545 data sheet (R1, R6, R9,
//! R12/R13).

use emu_core::frame::Rgb8;
use emu_core::gfx::{Canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxView, GfxViewInfo, OPT_ADDR};

use crate::{Pet, CHARGEN_SPACE, CPU_SPACE, PALETTE};

/// Palette index of the gap between cells.
const GAP: u8 = 2;
const GAP_COLOR: Rgb8 = Rgb8 {
    r: 0x30,
    g: 0x30,
    b: 0x30,
};

const OPT_SET: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "character set: 0 graphics, 1 text (charset default: both; screen default: as VIA CA2 selects)",
};
const OPT_INVERT: GfxOptionInfo = GfxOptionInfo {
    name: "invert",
    help: "1 = whole screen inverted (default: CRTC MA12 low)",
};

pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: "PET character ROM".into(),
            options: vec![OPT_SET],
        },
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: "PET screen".into(),
            options: vec![OPT_ADDR, OPT_SET, OPT_INVERT],
        },
    ]
}

fn palette() -> Vec<Rgb8> {
    let mut p = PALETTE.to_vec();
    p.push(GAP_COLOR);
    p
}

impl Pet {
    /// The character set VIA CA2 selects (1 = text).
    fn text_set(&self) -> bool {
        self.io.via.ca2_out()
    }

    pub(crate) fn view(&self, id: &str, opts: &GfxOptions) -> Option<GfxView> {
        let info = views().into_iter().find(|v| v.id == id)?;
        Some(match info.kind {
            GfxKind::Charset => self.charset_view(info, opts),
            _ => self.screen_view(info, opts),
        })
    }

    /// Both sets (256 characters, set 0 on top) or the one `index` picks,
    /// 32 to a row.
    fn charset_view(&self, info: GfxViewInfo, opts: &GfxOptions) -> GfxView {
        let chargen = &self.mem.roms.chargen;
        let (first, count) = match opts.get("index") {
            Some(set) => (u16::from(set != 0) * 128, 128u16),
            None => (0, 256),
        };
        let mut c = Canvas::grid(u32::from(count), 32, 8, 8, 1, GAP);
        let start = usize::from(first) * 8;
        let raw = chargen[start..start + usize::from(count) * 8].to_vec();
        for (i, ch) in raw.chunks(8).enumerate() {
            let (x0, y0) = Canvas::cell_origin(i as u32, 32, 8, 8, 1);
            for (row, &v) in ch.iter().enumerate() {
                c.row_1bpp(x0, y0 + row as u32, v, 1, Some(0), 1);
            }
        }
        GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: CHARGEN_SPACE,
            addr: u32::from(first) * 8,
            params: vec![
                ("first", u32::from(first)),
                ("count", u32::from(count)),
                ("text", u32::from(self.text_set())),
            ],
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(palette()),
            raw,
        }
    }

    /// The screen as the CRTC lays it out. Raw: the character codes.
    fn screen_view(&self, info: GfxViewInfo, opts: &GfxOptions) -> GfxView {
        let regs = &self.video.crtc.regs;
        let vram = &self.mem.vram;
        let len = vram.len();
        let cpc = (self.model.columns() / 40) as usize;
        let ma = (u16::from(regs[12]) << 8) | u16::from(regs[13]);
        let base = opts
            .get("addr")
            .map_or(usize::from(ma) * cpc, |a| a as usize)
            % len;
        let text = opts.get("index").map_or(self.text_set(), |s| s != 0);
        let invert = opts.flag("invert").unwrap_or(ma & 0x1000 == 0);
        let cols = (usize::from(regs[1]) * cpc).max(1);
        let rows = usize::from(regs[6]).max(1);
        let lines = usize::from(regs[9]) + 1;
        let set = if text { 0x400 } else { 0 };
        let chargen = &self.mem.roms.chargen;
        let mut c = Canvas::new((cols * 8) as u32, (rows * lines) as u32, 0);
        let mut raw = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let code = vram[(base + row * cols + col) % len];
                raw.push(code);
                for ra in 0..lines {
                    let mut pattern = if ra < 8 {
                        chargen[set | (usize::from(code & 0x7f) << 3) | ra]
                    } else {
                        0
                    };
                    if (code & 0x80 != 0) != invert {
                        pattern = !pattern;
                    }
                    c.row_1bpp(
                        (col * 8) as u32,
                        (row * lines + ra) as u32,
                        pattern,
                        1,
                        Some(0),
                        1,
                    );
                }
            }
        }
        GfxView {
            id: info.id,
            kind: info.kind,
            name: info.name,
            space: CPU_SPACE,
            addr: 0x8000 + base as u32,
            params: vec![
                ("screenbase", 0x8000 + base as u32),
                ("columns", cols as u32),
                ("rows", rows as u32),
                ("charheight", lines as u32),
                ("text", u32::from(text)),
                ("invert", u32::from(invert)),
            ],
            width: c.width,
            height: c.height,
            pixels: c.into_pixels(palette()),
            raw,
        }
    }
}
