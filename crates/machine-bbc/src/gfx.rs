//! Graphics data views (golden rule 1: the debugger sees them through
//! [`emu_core::Machine::gfx_views`]):
//!
//! - `screen`: the picture the 6845 and the Video ULA make from memory now,
//!   one character row after the other without borders: the start address
//!   from R12/R13 (option `addr`: another start in the CPU's memory),
//!   R1 characters by R6 rows, the ULA's pixel rate and palette, the
//!   hardware wrap-around and (B+, Master) the shadow RAM as the display
//!   sees it. Teletext through a fresh SAA5050 in all 20 half lines of a
//!   row.
//! - `palette`: the ULA's 16 logical colours as physical colours now
//!   (flashing colours in their current phase).
//! - `charset`: the SAA5050's 96 alphanumerics, then the 96 characters of
//!   the contiguous graphics set (blast-through capitals in its middle),
//!   12 × 20 half dots each.
//!
//! Everything is decoded from `peek`-style reads of [`Board`]; asking for a
//! view never changes the machine.

use chips_acorn::Saa5050;
use emu_core::frame::Rgb8;
use emu_core::gfx::{palette_canvas, Canvas, GfxPixels, OPT_ADDR};
use emu_core::{GfxKind, GfxOptions, GfxView, GfxViewInfo};

use crate::{Board, CPU_SPACE, PALETTE};

pub fn views() -> Vec<GfxViewInfo> {
    vec![
        GfxViewInfo {
            id: "screen",
            kind: GfxKind::Screen,
            name: "6845 + Video ULA screen".into(),
            options: vec![OPT_ADDR],
        },
        GfxViewInfo {
            id: "palette",
            kind: GfxKind::Palette,
            name: "Video ULA palette".into(),
            options: vec![],
        },
        GfxViewInfo {
            id: "charset",
            kind: GfxKind::Charset,
            name: "SAA5050 character set".into(),
            options: vec![],
        },
    ]
}

pub fn view(b: &Board, id: &str, opts: &GfxOptions) -> Option<GfxView> {
    let info = views().into_iter().find(|v| v.id == id)?;
    let (params, width, height, pixels, addr, raw) = match id {
        "screen" => screen(b, opts),
        "palette" => palette(b),
        _ => charset(),
    };
    Some(GfxView {
        id: info.id,
        kind: info.kind,
        name: info.name,
        space: CPU_SPACE,
        addr,
        params,
        width,
        height,
        pixels,
        raw,
    })
}

type Decoded = (Vec<(&'static str, u32)>, u32, u32, GfxPixels, u32, Vec<u8>);

fn screen(b: &Board, opts: &GfxOptions) -> Decoded {
    let r = &b.crtc.regs;
    let teletext = b.ula.teletext();
    let chars = u32::from(r[1]).clamp(1, 100);
    let rows = u32::from(r[6] & 0x7f).clamp(1, 64);
    let mut ma = (u16::from(r[12] & 0x3f) << 8) | u16::from(r[13]);
    if let Some(a) = opts.get("addr") {
        let a = (a & 0x7fff) as u16;
        ma = if teletext {
            0x2000 | (a & 0x3ff)
        } else {
            a >> 3
        };
    }
    let start = u32::from(b.screen_addr(ma, 0));
    let mut raw = Vec::new();
    if teletext {
        let (w, h) = (chars * 16, rows * 20);
        let mut c = Canvas::new(w, h, 0);
        let mut saa = Saa5050::new();
        saa.new_field(b.video.fields);
        for row in 0..rows {
            let row_ma = ma.wrapping_add((row * chars) as u16);
            let bytes: Vec<u8> = (0..chars)
                .map(|i| b.screen_byte(b.screen_addr(row_ma.wrapping_add(i as u16), 0)))
                .collect();
            for hl in 0..20u8 {
                saa.new_line(hl);
                for (i, &byte) in bytes.iter().enumerate() {
                    let mut half = [0u8; 12];
                    saa.character(byte, &mut half);
                    for x in 0..16u32 {
                        c.set(
                            i as u32 * 16 + x,
                            row * 20 + u32::from(hl),
                            half[(x * 12 / 16) as usize],
                        );
                    }
                }
            }
            raw.extend(bytes);
        }
        let params = vec![
            ("mode", 7),
            ("chars", chars),
            ("rows", rows),
            ("ma", u32::from(ma)),
        ];
        return (params, w, h, c.into_pixels(PALETTE.to_vec()), start, raw);
    }
    let n = if b.ula.fast_clock() { 8 } else { 16 };
    let lines = u32::from(r[9] & 0x1f) + 1;
    let (w, h) = (chars * n, rows * lines);
    let mut c = Canvas::new(w, h, 0);
    for row in 0..rows {
        for ra in 0..lines {
            for i in 0..chars {
                let cma = ma.wrapping_add((row * chars + i) as u16);
                let mut px = [0u8; 16];
                if ra & 8 == 0 {
                    let byte = b.screen_byte(b.screen_addr(cma, ra as u8));
                    raw.push(byte);
                    b.ula.pixels(byte, &mut px[..n as usize]);
                }
                for (x, &p) in px[..n as usize].iter().enumerate() {
                    c.set(i * n + x as u32, row * lines + ra, p);
                }
            }
        }
    }
    let rate = 2u32 << ((b.ula.ctrl >> 2) & 3);
    let params = vec![
        ("ctrl", u32::from(b.ula.ctrl)),
        ("pixel_mhz", rate),
        ("chars", chars),
        ("rows", rows),
        ("lines", lines),
        ("ma", u32::from(ma)),
    ];
    (params, w, h, c.into_pixels(PALETTE.to_vec()), start, raw)
}

fn palette(b: &Board) -> Decoded {
    let colors: Vec<Rgb8> = (0..16)
        .map(|l| PALETTE[usize::from(b.ula.colour(l))])
        .collect();
    let (w, h, pixels) = palette_canvas(&colors, 16, 16);
    let params = vec![("count", 16), ("ctrl", u32::from(b.ula.ctrl))];
    (params, w, h, pixels, 0xfe21, b.ula.palette.to_vec())
}

fn charset() -> Decoded {
    const COLS: u32 = 16;
    let count = 192u32;
    let mut c = Canvas::grid(count, COLS, 12, 20, 1, 4);
    for i in 0..count {
        let code = 0x20 + (i % 96) as u8;
        let graphics = i >= 96;
        let (x0, y0) = Canvas::cell_origin(i, COLS, 12, 20, 1);
        let mut saa = Saa5050::new();
        saa.new_field(0);
        for hl in 0..20u8 {
            saa.new_line(hl);
            let mut half = [0u8; 12];
            if graphics {
                // Graphics white (set-after): its own cell stays blank.
                saa.character(0x97, &mut half);
            }
            saa.character(code, &mut half);
            for (x, &p) in half.iter().enumerate() {
                c.set(x0 + x as u32, y0 + u32::from(hl), p);
            }
        }
    }
    let params = vec![("count", count), ("cols", COLS)];
    (
        params,
        c.width,
        c.height,
        c.into_pixels(PALETTE.to_vec()),
        0,
        Vec::new(),
    )
}
