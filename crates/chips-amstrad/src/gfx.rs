//! Graphics data views of the CPC's CRTC 6845 and Gate Array (see
//! `emu_core::gfx`): the screen memory as the two chips address it now,
//! and the pens' hardware colours; side-effect free.
//!
//! Addressing (cpcwiki, "CRTC" and "Gate Array"): the CRTC counts the
//! memory address MA from R12/R13 at the frame's start, R1 characters a
//! row, R6 displayed rows of R9 + 1 scan lines (RA), adding R1 at the end
//! of each row. For each character the Gate Array fetches two bytes at
//! `MA13–12 · RA2–0 · MA9–0 · 0/1` ([`video_address`]) from the first
//! 64 KB of RAM — whatever the 6128's RAM configuration maps for the
//! Z80 — and decodes them in the current mode ([`byte_pens`]). The view
//! decodes the whole displayed area with the registers as they are now;
//! splits and mode or ink changes during the frame (rasters) are not
//! reproduced, and the CRTC type 0 skew is ignored.

use emu_core::frame::Rgb8;
use emu_core::gfx::{
    palette_canvas, GfxKind, GfxOptionInfo, GfxOptions, GfxPixels, GfxView, GfxViewInfo, OPT_ADDR,
};
use emu_core::MemSpaceId;

use crate::crtc::Crtc;
use crate::gate_array::{byte_pens, video_address, GateArray};

pub const OPT_MODE: GfxOptionInfo = GfxOptionInfo {
    name: "mode",
    help: "screen mode 0-3 (default: the Gate Array's mode in effect)",
};
pub const OPT_CHARS: GfxOptionInfo = GfxOptionInfo {
    name: "chars",
    help: "characters (2 bytes) per line (default: CRTC R1)",
};
pub const OPT_ROWS: GfxOptionInfo = GfxOptionInfo {
    name: "rows",
    help: "character rows (default: CRTC R6)",
};
pub const OPT_LINES: GfxOptionInfo = GfxOptionInfo {
    name: "lines",
    help: "scan lines per row 1-32 (default: CRTC R9 + 1; RA bits 3-4 do not reach the address)",
};

/// Most characters per line and rows a screen view decodes.
const MAX_CHARS: u32 = 128;
const MAX_ROWS: u32 = 128;

/// What the machine tells the decoder besides the chips.
#[derive(Clone, Copy)]
pub struct GfxContext<'a> {
    /// The first 64 KB of RAM, the Gate Array's view.
    pub ram: &'a [u8],
    /// The memory space `ram` is reported in (RAM banks 0–3 at 0–&FFFF).
    pub space: MemSpaceId,
    /// The 32 hardware colours.
    pub palette: &'a [Rgb8],
}

/// The views: `screen` and `palette`.
pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Bitmap,
            name: "CPC screen memory (CRTC + Gate Array)".into(),
            options: vec![OPT_ADDR, OPT_MODE, OPT_CHARS, OPT_ROWS, OPT_LINES],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "Gate Array pens".into(),
            options: vec![],
        },
    ]
}

/// The CRTC start address of a RAM address (option `addr`): MA13–12
/// from bits 15–14, MA9–0 from bits 10–1; bits 13–11 are the row address
/// and count as 0.
pub fn ma_of_address(addr: u32) -> u16 {
    let a = addr as u16;
    (a >> 14 & 3) << 12 | (a >> 1 & 0x3ff)
}

const PEN_NAMES: [&str; 16] = [
    "pen0", "pen1", "pen2", "pen3", "pen4", "pen5", "pen6", "pen7", "pen8", "pen9", "pen10",
    "pen11", "pen12", "pen13", "pen14", "pen15",
];

/// Decode one view of [`views`].
pub fn gfx_view(
    crtc: &Crtc,
    ga: &GateArray,
    id: &'static str,
    kind: GfxKind,
    name: String,
    ctx: &GfxContext<'_>,
    opts: &GfxOptions,
) -> GfxView {
    let mut palette = ctx.palette.to_vec();
    palette.resize(32, Rgb8::default());
    if kind == GfxKind::Palette {
        let colors: Vec<Rgb8> = ga
            .inks
            .iter()
            .map(|&i| palette[usize::from(i & 31)])
            .collect();
        let (w, h, pixels) = palette_canvas(&colors, 17, 16);
        let mut params: Vec<(&'static str, u32)> = PEN_NAMES
            .iter()
            .zip(ga.inks)
            .map(|(&n, i)| (n, u32::from(i)))
            .collect();
        params.push(("border", u32::from(ga.inks[16])));
        params.push(("mode", u32::from(ga.mode)));
        params.push(("pens", [16, 4, 2, 4][usize::from(ga.mode & 3)]));
        return GfxView {
            id,
            kind,
            name,
            space: ctx.space,
            addr: 0,
            params,
            width: w,
            height: h,
            pixels,
            raw: ga.inks.to_vec(),
        };
    }
    let r = &crtc.regs;
    let mode = opts.get("mode").map_or(ga.mode & 3, |m| (m & 3) as u8);
    let chars = opts
        .get("chars")
        .unwrap_or(u32::from(r[1]))
        .clamp(1, MAX_CHARS);
    let rows = opts
        .get("rows")
        .unwrap_or(u32::from(r[6]))
        .clamp(1, MAX_ROWS);
    let lines = opts
        .get("lines")
        .unwrap_or(u32::from(r[9] & 0x1f) + 1)
        .clamp(1, 32);
    let start = opts
        .get("addr")
        .map_or(u16::from(r[12]) << 8 | u16::from(r[13]), ma_of_address)
        & 0x3fff;
    let per_byte = byte_pens(mode, 0).1 as u32;
    let width = chars * 2 * per_byte;
    let height = rows * lines;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    let mut raw = Vec::with_capacity((chars * 2 * height) as usize);
    let byte = |a: u16| ctx.ram.get(usize::from(a)).copied().unwrap_or(0);
    for row in 0..rows {
        let ma_row = start.wrapping_add((row * chars) as u16);
        for ra in 0..lines {
            for c in 0..chars {
                let ma = ma_row.wrapping_add(c as u16) & 0x3fff;
                let a = video_address(ma, ra as u8);
                for b in [byte(a), byte(a | 1)] {
                    raw.push(b);
                    let (px, n) = byte_pens(mode, b);
                    pixels.extend(px[..n].iter().map(|&p| ga.inks[usize::from(p)] & 31));
                }
            }
        }
    }
    let params = vec![
        ("ma", u32::from(start)),
        ("page", u32::from(start >> 12 & 3) << 14),
        ("mode", u32::from(mode)),
        ("chars", chars),
        ("rows", rows),
        ("lines", lines),
        ("width", width),
        ("height", height),
        ("r1", u32::from(r[1])),
        ("r6", u32::from(r[6])),
        ("r9", u32::from(r[9])),
        ("r12", u32::from(r[12])),
        ("r13", u32::from(r[13])),
        ("border", u32::from(ga.inks[16])),
    ];
    GfxView {
        id,
        kind,
        name,
        space: ctx.space,
        addr: u32::from(video_address(start, 0)),
        params,
        width,
        height,
        pixels: GfxPixels::Indexed { pixels, palette },
        raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crtc::CrtcType;
    use crate::palette::palette;

    /// The firmware's CRTC settings: 40 × 25 characters of 8 lines from
    /// &C000.
    fn crtc() -> Crtc {
        let mut c = Crtc::new(CrtcType::T1);
        for (r, v) in [63, 40, 46, 0x8e, 38, 0, 25, 30, 0, 7, 0, 0, 0x30, 0]
            .into_iter()
            .enumerate()
        {
            c.select(r as u8);
            c.write(v);
        }
        c
    }

    fn view(crtc: &Crtc, ga: &GateArray, ram: &[u8], id: &str, opts: &GfxOptions) -> GfxView {
        let pal = palette();
        let info = views().into_iter().find(|v| v.id == id).unwrap();
        let ctx = GfxContext {
            ram,
            space: MemSpaceId(1),
            palette: &pal,
        };
        gfx_view(crtc, ga, info.id, info.kind, info.name, &ctx, opts)
    }

    #[test]
    fn screen_follows_the_crtc_and_the_mode() {
        let c = crtc();
        let mut ga = GateArray::new();
        for (pen, colour) in [(0, 20), (1, 24), (2, 2), (3, 6)] {
            ga.write(pen);
            ga.write(0x40 | colour);
        }
        ga.mode = 1;
        let mut ram = vec![0u8; 0x10000];
        ram[0xc000] = 0b1000_1000; // pixel 0 pen 3
        ram[0xc800] = 0b0100_0000; // line 1, pixel 1 pen 1
        ram[0xc000 + 80] = 0b0000_1000; // row 1 (40 characters on), pen 2
        let v = view(&c, &ga, &ram, "screen", &GfxOptions::new());
        assert_eq!((v.width, v.height), (320, 200));
        assert_eq!(v.addr, 0xc000);
        assert_eq!(v.param("ma"), Some(0x3000));
        assert_eq!(v.param("page"), Some(0xc000));
        assert_eq!(v.param("mode"), Some(1));
        assert_eq!(v.raw.len(), 80 * 200);
        assert_eq!(v.raw[0], 0b1000_1000);
        let pal = palette();
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(pal[6]));
        assert_eq!(f.rgb_at(1, 0), Some(pal[20]));
        assert_eq!(f.rgb_at(1, 1), Some(pal[24]));
        assert_eq!(f.rgb_at(0, 8), Some(pal[2]));
        // Mode 0 from another address: 160 pixels of 16 pens.
        ram[0x4000] = 0b1000_0000; // pixel 0: pen 1
        let m0 = view(
            &c,
            &ga,
            &ram,
            "screen",
            &GfxOptions::new().with("mode", 0).with("addr", 0x4000),
        );
        assert_eq!((m0.width, m0.addr), (160, 0x4000));
        assert_eq!(m0.param("ma"), Some(0x1000));
        assert_eq!(m0.frame().rgb_at(0, 0), Some(pal[24]));
        // Mode 2 with a smaller geometry.
        let m2 = view(
            &c,
            &ga,
            &ram,
            "screen",
            &GfxOptions::new()
                .with("mode", 2)
                .with("chars", 10)
                .with("rows", 2)
                .with("lines", 4),
        );
        assert_eq!((m2.width, m2.height), (160, 8));
        assert_eq!(m2.raw.len(), 20 * 8);
        assert_eq!(m2.frame().rgb_at(0, 0), Some(pal[24]), "bit 7: pen 1");
        assert_eq!(m2.frame().rgb_at(4, 0), Some(pal[24]), "bit 3: pen 1");
        assert_eq!(m2.frame().rgb_at(1, 0), Some(pal[20]));
    }

    #[test]
    fn palette_lists_pens_and_border() {
        let c = crtc();
        let mut ga = GateArray::new();
        ga.write(5);
        ga.write(0x40 | 12);
        ga.write(0x10);
        ga.write(0x40 | 4);
        let v = view(&c, &ga, &[], "palette", &GfxOptions::new());
        assert_eq!(v.param("pen5"), Some(12));
        assert_eq!(v.param("border"), Some(4));
        assert_eq!(v.raw.len(), 17);
        assert_eq!((v.width, v.height), (17 * 17 - 1, 16));
        let pal = palette();
        assert_eq!(v.frame().rgb_at(5 * 17, 0), Some(pal[12]));
        assert_eq!(v.frame().rgb_at(16 * 17, 0), Some(pal[4]));
    }

    #[test]
    fn addresses_mix_ma_and_ra() {
        assert_eq!(video_address(0x3000, 0), 0xc000);
        assert_eq!(video_address(0x3001, 1), 0xc802);
        assert_eq!(video_address(0x33ff, 7), 0xfffe);
        assert_eq!(ma_of_address(0xc802), 0x3001);
    }
}
