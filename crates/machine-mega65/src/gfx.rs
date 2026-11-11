//! Graphics data views (golden rule 1: the debugger sees them through
//! [`emu_core::Machine::gfx_views`]):
//!
//! - `screen`: the display area (inside the borders) as the VIC-IV draws it
//!   from memory now ([`crate::render::line`] for each of its lines),
//!   whatever the mode: text, bitmap, bitplanes, sprites.
//! - `charset`: 256 characters of 8 × 8 from the character pointer
//!   (`$D068`; option `addr`: another address, the character ROM's windows
//!   as the VIC-IV reads them), one bit a pixel, white on black.
//! - `sprites`: the eight VIC-II sprites (24 × 21, multicolour where set)
//!   from their pointers, in their colours; option `index` one of them.
//! - `palette`: the 256 colours of a palette bank (option `palette`, 0–3;
//!   default the bank text and bitmaps use).
//!
//! Views come from side-effect-free reads of [`Board`]: asking for one
//! never changes the machine.

use emu_core::frame::Rgb8;
use emu_core::gfx::{palette_canvas, Canvas, GfxPixels, OPT_ADDR, OPT_INDEX, OPT_PALETTE};
use emu_core::{GfxKind, GfxOptions, GfxView, GfxViewInfo};

use crate::board::Board;
use crate::render::{chardata, col8, line, ram};
use crate::vic::{HEIGHT, WIDTH};
use crate::{CPU_SPACE, PHYS_SPACE};

pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: "VIC-IV display area".into(),
            options: vec![],
        },
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: "VIC-IV character set".into(),
            options: vec![OPT_ADDR],
        },
        GfxViewInfo {
            id: "sprites",
            kind: GfxKind::Sprites,
            name: "VIC-IV sprites (VIC-II compatible)".into(),
            options: vec![OPT_INDEX],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "VIC-IV palette".into(),
            options: vec![OPT_PALETTE],
        },
    ]
}

pub fn view(b: &Board, id: &str, opts: &GfxOptions) -> Option<GfxView> {
    let info = views().into_iter().find(|v| v.id == id)?;
    let (space, addr, params, width, height, pixels, raw) = match id {
        "screen" => screen(b),
        "charset" => charset(b, opts),
        "sprites" => sprites(b, opts),
        _ => palette(b, opts),
    };
    Some(GfxView {
        id: info.id,
        kind: info.kind,
        name: info.name,
        space,
        addr,
        params,
        width,
        height,
        pixels,
        raw,
    })
}

type Decoded = (
    emu_core::MemSpaceId,
    u32,
    Vec<(&'static str, u32)>,
    u32,
    u32,
    GfxPixels,
    Vec<u8>,
);

fn screen(b: &Board) -> Decoded {
    let v = &b.vic;
    let (top, bottom) = (v.reg16(0x48), v.reg16(0x4a).min(HEIGHT as u16));
    let left = usize::from(v.border_left).min(WIDTH);
    let right = WIDTH.saturating_sub(usize::from(v.border_right)).max(left);
    let (w, h) = ((right - left) as u32, u32::from(bottom.saturating_sub(top)));
    let mut out = vec![Rgb8::default(); WIDTH];
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in top..bottom {
        line(b, y, true, &mut out);
        pixels.extend_from_slice(&out[left..right]);
    }
    let screen = v.addr24(0x60);
    let params = vec![
        ("scrnptr", screen),
        ("charptr", v.addr24(0x68)),
        (
            "linestep",
            u32::from(v.regs[0x58]) | u32::from(v.regs[0x59]) << 8,
        ),
        ("chrcount", u32::from(v.regs[0x5e])),
        ("rows", u32::from(v.regs[0x7b]) + 1),
        ("d011", u32::from(v.regs[0x11])),
        ("d016", u32::from(v.regs[0x16])),
        ("d031", u32::from(v.regs[0x31])),
        ("d054", u32::from(v.regs[0x54])),
    ];
    let bytes =
        (u32::from(v.regs[0x58]) | u32::from(v.regs[0x59]) << 8) * (u32::from(v.regs[0x7b]) + 1);
    let raw = (0..bytes.min(0x4000)).map(|i| ram(b, screen + i)).collect();
    (
        PHYS_SPACE,
        screen,
        params,
        w,
        h,
        GfxPixels::Rgb(pixels),
        raw,
    )
}

fn charset(b: &Board, opts: &GfxOptions) -> Decoded {
    let base = opts.get("addr").unwrap_or_else(|| b.vic.addr24(0x68));
    let mut c = Canvas::grid(256, 32, 8, 8, 1, 2);
    let mut raw = Vec::with_capacity(2048);
    for ch in 0..256u32 {
        let (x0, y0) = Canvas::cell_origin(ch, 32, 8, 8, 1);
        for row in 0..8u32 {
            let byte = chardata(b, base + ch * 8 + row);
            raw.push(byte);
            c.row_1bpp(x0, y0 + row, byte, 1, Some(0), 1);
        }
    }
    let palette = vec![
        Rgb8::default(),
        Rgb8 {
            r: 255,
            g: 255,
            b: 255,
        },
        Rgb8 {
            r: 40,
            g: 40,
            b: 80,
        },
    ];
    let (w, h) = (c.width, c.height);
    (
        PHYS_SPACE,
        base,
        vec![("addr", base), ("count", 256)],
        w,
        h,
        c.into_pixels(palette),
        raw,
    )
}

fn sprites(b: &Board, opts: &GfxOptions) -> Decoded {
    let r = &b.vic.regs;
    let only = opts.get("index").map(|i| (i & 7) as usize);
    let list: Vec<usize> = only.map_or((0..8).collect(), |i| vec![i]);
    let colours = b.vic.colours(usize::from(r[0x70] >> 4 & 3));
    let bank = u32::from(3 - (b.vic.cia2_pa & 3)) * 0x4000;
    let pointers = u32::from(r[0x6c]) | u32::from(r[0x6d]) << 8 | u32::from(r[0x6e] & 0x7f) << 16;
    let n = list.len() as u32;
    let cols = n.min(4);
    let rows = n.div_ceil(cols);
    let (cw, ch) = (24 + 1, 21 + 1);
    let (w, h) = (cols * cw - 1, rows * ch - 1);
    let mut pixels = vec![
        Rgb8 {
            r: 40,
            g: 40,
            b: 80
        };
        (w * h) as usize
    ];
    let mut raw = Vec::new();
    for (k, &s) in list.iter().enumerate() {
        let (x0, y0) = ((k as u32 % cols) * cw, (k as u32 / cols) * ch);
        let data = bank + u32::from(ram(b, pointers + s as u32)) * 64;
        let mcm = r[0x1c] >> s & 1 != 0;
        let colour = col8(b, r[0x27 + s]);
        let (m0, m1) = (col8(b, r[0x25]), col8(b, r[0x26]));
        for row in 0..21u32 {
            let bytes: Vec<u8> = (0..3).map(|i| ram(b, data + row * 3 + i)).collect();
            raw.extend_from_slice(&bytes);
            let bits = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
            for px in 0..24u32 {
                let c = if mcm {
                    match bits >> (22 - (px & !1)) & 3 {
                        0 => None,
                        1 => Some(m0),
                        2 => Some(colour),
                        _ => Some(m1),
                    }
                } else {
                    (bits >> (23 - px) & 1 != 0).then_some(colour)
                };
                let at = ((y0 + row) * w + x0 + px) as usize;
                pixels[at] = c.map_or(Rgb8::default(), |c| colours[usize::from(c)]);
            }
        }
    }
    let params = vec![
        ("count", n),
        ("enabled", u32::from(r[0x15])),
        ("multicolour", u32::from(r[0x1c])),
        ("pointers", pointers),
    ];
    (
        CPU_SPACE,
        pointers,
        params,
        w,
        h,
        GfxPixels::Rgb(pixels),
        raw,
    )
}

fn palette(b: &Board, opts: &GfxOptions) -> Decoded {
    let bank = opts
        .get("palette")
        .map_or(usize::from(b.vic.regs[0x70] >> 4 & 3), |p| (p & 3) as usize);
    let colours = b.vic.colours(bank);
    let (w, h, pixels) = palette_canvas(&colours, 16, 12);
    let raw = (0..3)
        .flat_map(|c| b.vic.palette[bank][c].to_vec())
        .collect();
    (
        PHYS_SPACE,
        0x0ffd_3100,
        vec![("bank", bank as u32), ("count", 256)],
        w,
        h,
        pixels,
        raw,
    )
}
