//! The VIC-IV's picture, one visible line (of the 720 × 576 PAL frame) at a
//! time from the registers of [`crate::vic`] and the memory as it is when
//! the line is drawn (raster splits work at line precision).
//!
//! **A line.** Outside the top/bottom border positions (`$D048`, `$D04A`)
//! or with the display blanked (`$D011` bit 4) all border colour
//! (`$D020`); inside, the side borders (the internal edges the hot
//! registers set) and between them the background (`$D021`) with the text
//! or bitmap drawn from TEXTXPOS/TEXTYPOS (`$D04C`, `$D04E`): `$D05E`
//! characters of 8 pixels (2 physical pixels each without H640), rows of 8
//! pixel rows (2 lines each without V400), `$D07B` + 1 rows; each row
//! `$D058` bytes on in screen and colour RAM; the screen at `$D060`, colour
//! RAM at `$D064`, character data at `$D068` (a pointer into `$1000`–
//! `$1FFF` or `$9000`–`$9FFF` of the first 64 KB reads the character ROM, as
//! on the C64).
//!
//! **Modes.** VIC-II text (16 colours), multicolour text (`$D016` bit 4,
//! colour bit 3), extended background (`$D011` bit 6), bitmaps hires and
//! multicolour (`$D011` bit 5; the bitmap at the character pointer's 8 KB);
//! the VIC-III attributes (`$D031` bit 5: colour RAM bits 4 blink, 5
//! reverse, 6 bold (+16), 7 underline; bold and reverse together select the
//! alternate palette); the VIC-IV's 16-bit characters (`$D054` bit 0: two
//! bytes of screen and colour RAM each; 13-bit character numbers,
//! horizontal and vertical flip, GOTOX, NCM) with full-colour characters
//! (`$D054` bits 1/2: 64 bytes a character at number × 64, a byte a pixel,
//! 0 transparent, `$FF` the colour RAM colour; NCM 16 pixels of 4 bits).
//! Colours use the text/bitmap palette bank (`$D070` bits 5–4).
//!
//! **Sprites** as on the VIC-II: eight of 24 × 21 (`$D015` enable, `$D000`–
//! `$D010` positions, `$D017`/`$D01D` expansion, `$D01C` multicolour with
//! `$D025`/`$D026`, colours `$D027`–`$D02E`, `$D01B` behind the foreground),
//! sprite 0 in front; a sprite pixel is 2 × 2 physical pixels, X 24 and
//! Y 50 the display area's top left corner as on the C64 (a sprite starts
//! the line after its Y). Pointers: a byte each at `$D06C`–`$D06E`, the
//! image at the VIC bank + 64 × pointer. Collisions set `$D01E`/`$D01F`
//! and the interrupt flags 2 and 1. The border hides them.
//!
//! **Bitplanes** (VIC-III, `$D031` bit 4): see [`bitplanes`] (the C65
//! specification's "BitPlane mode" and "Display Address Translator").
//!
//! Not yet: the VIC-IV's sprite extensions (16 colours, H640, sizes, 16-bit
//! pointers), bitplane attributes, H1280, the DAT ports at `$D040`–`$D047`,
//! the raster rewrite buffer beyond GOTOX,
//! alpha blending, CHRXSCL/CHRYSCL other than the hot registers' values,
//! NTSC.
//!
//! Sources: the MEGA65 user guide, `appendix-viciv-registers.tex` (the
//! super-extended attribute mode's bit fields, full-colour and nibble
//! characters, hot registers, sprite pointers); the C65 system
//! specification (VIC-III attributes); Christian Bauer, "The MOS
//! 6567/6569 video controller (VIC-II) and its application in the
//! Commodore 64" (the VIC-II modes, sprites, priorities, collisions).

use emu_core::frame::Rgb8;

use crate::board::{Board, CHIP_RAM};
use crate::vic::{Personality, FIRST_PHYSICAL, LEFT, WIDTH};

/// A byte of chip RAM by its 28-bit address (wrapping in chip RAM).
pub(crate) fn ram(b: &Board, a: u32) -> u8 {
    b.chip[a as usize % CHIP_RAM]
}

/// A byte of character data: the character ROM for the VIC-II's ROM
/// windows of the first 64 KB.
pub(crate) fn chardata(b: &Board, a: u32) -> u8 {
    if a < 0x1_0000 && matches!(a & 0xf000, 0x1000 | 0x9000) {
        b.charrom[(a & 0x0fff) as usize]
    } else {
        ram(b, a)
    }
}

/// The colour registers' bits: 4 in the C64 personality.
pub(crate) fn col8(b: &Board, c: u8) -> u8 {
    if b.vic.personality == Personality::C64 {
        c & 0x0f
    } else {
        c
    }
}

/// Draw visible line `y` into `out` (720 pixels). `blink`: blinking
/// characters are shown now. Returns the sprite–sprite and sprite–data
/// collision bits of the line.
pub fn line(b: &Board, y: u16, blink: bool, out: &mut [Rgb8]) -> (u8, u8) {
    let v = &b.vic;
    let r = &v.regs;
    let colours = v.colours(usize::from(r[0x70] >> 4 & 3));
    let border = colours[usize::from(col8(b, r[0x20]))];
    out.fill(border);
    let (top, bottom) = (v.reg16(0x48), v.reg16(0x4a));
    if r[0x11] & 0x10 == 0 || y < top || y >= bottom {
        return (0, 0);
    }
    let left = usize::from(v.border_left).min(WIDTH);
    let right = WIDTH.saturating_sub(usize::from(v.border_right)).max(left);
    let bg = col8(b, r[0x21]);
    out[left..right].fill(colours[usize::from(bg)]);
    let mut fg = [false; WIDTH];
    if r[0x31] & 0x10 != 0 {
        bitplanes(b, y, out, &mut fg, (left, right), &colours);
    } else {
        text(b, y, blink, out, &mut fg, (left, right), &colours);
    }
    sprites(b, y, out, &fg, (left, right), &colours)
}

/// The text or bitmap of the line; `fg` marks the foreground pixels.
fn text(
    b: &Board,
    y: u16,
    blink: bool,
    out: &mut [Rgb8],
    fg: &mut [bool],
    (left, right): (usize, usize),
    colours: &[Rgb8; 256],
) {
    let v = &b.vic;
    let r = &v.regs;
    let h640 = r[0x31] & 0x80 != 0;
    let v400 = r[0x31] & 0x08 != 0;
    let line_h: u16 = if v400 { 1 } else { 2 };
    let texty = v.reg16(0x4e);
    if y < texty {
        return;
    }
    let ty = (y - texty) / line_h;
    let (row, prow) = (u32::from(ty / 8), ty % 8);
    if row > u32::from(r[0x7b]) {
        return;
    }
    let alt = v.colours(usize::from(r[0x70] & 3));
    let px_w: usize = if h640 { 1 } else { 2 };
    let textx = usize::from(v.reg16(0x4c));
    let chars = usize::from(r[0x5e]) | usize::from(r[0x63] >> 4 & 3) << 8;
    let step = u32::from(r[0x58]) | u32::from(r[0x59]) << 8;
    let screen = v.addr24(0x60) | u32::from(r[0x63] & 0x0f) << 24;
    let colptr = u32::from(r[0x64]) | u32::from(r[0x65]) << 8;
    let charptr = v.addr24(0x68);
    let chr16 = r[0x54] & 0x01 != 0;
    let bytes: u32 = if chr16 { 2 } else { 1 };
    let ecm = r[0x11] & 0x40 != 0;
    let bmm = r[0x11] & 0x20 != 0;
    let mcm = r[0x16] & 0x10 != 0;
    let attr = r[0x31] & 0x20 != 0;
    let (fcm_lo, fcm_hi) = (r[0x54] & 0x02 != 0, r[0x54] & 0x04 != 0);
    let bg = col8(b, r[0x21]);
    let mc = [bg, col8(b, r[0x22]), col8(b, r[0x23]), col8(b, r[0x24])];

    let mut x = textx;
    for col in 0..chars as u32 {
        let at = row * step + col * bytes;
        let s0 = ram(b, screen + at);
        let c0 = b.colour[((colptr + at) & 0x7fff) as usize];
        let (s1, c1) = if chr16 {
            (
                ram(b, screen + at + 1),
                b.colour[((colptr + at + 1) & 0x7fff) as usize],
            )
        } else {
            (0, 0)
        };
        let mut width = 8;
        let mut palette = colours;
        // The cell: palette indices (None: the background shows) and
        // whether each pixel counts as foreground for sprites.
        let mut pix: [Option<u8>; 16] = [None; 16];
        let mut front = [false; 16];
        if chr16 {
            if c0 & 0x10 != 0 {
                // GOTOX: the following characters start at this position.
                let pos = usize::from(s0) | usize::from(s1 & 3) << 8;
                x = textx + pos * px_w;
                continue;
            }
            let code = u32::from(s0) | u32::from(s1 & 0x1f) << 8;
            let (vflip, hflip, ncm) = (c0 & 0x80 != 0, c0 & 0x40 != 0, c0 & 0x08 != 0);
            let pr = u32::from(if vflip { 7 - prow } else { prow });
            let full = if code <= 0xff { fcm_lo } else { fcm_hi };
            let colour = c1;
            if full {
                let base = code * 64 + pr * 8;
                if ncm {
                    width = 16;
                    for i in 0..16u32 {
                        let byte = ram(b, base + i / 2);
                        let n = if i % 2 == 0 { byte & 0x0f } else { byte >> 4 };
                        pix[i as usize] =
                            (n != 0).then_some(if n == 15 { colour } else { n | (colour & 0xf0) });
                    }
                } else {
                    for i in 0..8u32 {
                        let p = ram(b, base + i);
                        pix[i as usize] = (p != 0).then_some(if p == 0xff { colour } else { p });
                    }
                }
            } else {
                let glyph = chardata(b, charptr + code * 8 + pr);
                for (i, p) in pix.iter_mut().take(8).enumerate() {
                    *p = (glyph & (0x80 >> i) != 0).then_some(colour);
                }
            }
            if hflip {
                pix[..width].reverse();
            }
            for i in 0..width {
                front[i] = pix[i].is_some();
            }
        } else if bmm {
            let base = charptr & !0x1fff;
            let byte = ram(b, base + (row * step + col) * 8 + u32::from(prow));
            if mcm {
                let cs = [bg, s0 >> 4, s0 & 0x0f, c0 & 0x0f];
                for i in 0..4 {
                    let pair = usize::from(byte >> (6 - 2 * i) & 3);
                    for k in [2 * i, 2 * i + 1] {
                        pix[k] = Some(cs[pair]);
                        front[k] = pair >= 2;
                    }
                }
            } else {
                for i in 0..8 {
                    let on = byte & (0x80 >> i) != 0;
                    pix[i] = Some(if on { s0 >> 4 } else { s0 & 0x0f });
                    front[i] = on;
                }
            }
        } else {
            let code = if ecm { s0 & 0x3f } else { s0 };
            let glyph = chardata(b, charptr + u32::from(code) * 8 + u32::from(prow));
            let fgc = c0 & 0x0f;
            if ecm {
                let back = mc[usize::from(s0 >> 6)];
                for i in 0..8 {
                    let on = glyph & (0x80 >> i) != 0;
                    pix[i] = Some(if on { fgc } else { back });
                    front[i] = on;
                }
            } else if mcm && c0 & 0x08 != 0 {
                let cs = [mc[0], mc[1], mc[2], c0 & 0x07];
                for i in 0..4 {
                    let pair = usize::from(glyph >> (6 - 2 * i) & 3);
                    for k in [2 * i, 2 * i + 1] {
                        pix[k] = (pair != 0).then_some(cs[pair]);
                        front[k] = pair >= 2;
                    }
                }
            } else {
                let mut bits = glyph;
                let mut fgc = fgc;
                if attr {
                    let (blinks, reverse, bold, underline) = (
                        c0 & 0x10 != 0,
                        c0 & 0x20 != 0,
                        c0 & 0x40 != 0,
                        c0 & 0x80 != 0,
                    );
                    if underline && prow == 7 {
                        bits = 0xff;
                    }
                    if blinks && !blink {
                        bits = 0;
                    }
                    if reverse {
                        bits = !bits;
                    }
                    if bold && reverse {
                        // Both select the alternate palette.
                        palette = &alt;
                    } else if bold {
                        fgc += 16;
                    }
                }
                for i in 0..8 {
                    let on = bits & (0x80 >> i) != 0;
                    pix[i] = on.then_some(fgc);
                    front[i] = on;
                }
            }
        }
        for i in 0..width {
            if let Some(c) = pix[i] {
                let rgb = palette[usize::from(c)];
                for k in 0..px_w {
                    let xx = x + i * px_w + k;
                    if (left..right).contains(&xx) {
                        out[xx] = rgb;
                        fg[xx] = front[i];
                    }
                }
            }
        }
        x += width * px_w;
        if x >= right {
            break;
        }
    }
}

/// The VIC-III bitplanes (`$D031` bit 4 overrides the other modes): up to
/// eight planes (`$D032` enables), 320 or 640 pixels (H640; 640 uses
/// planes 0–3), 200 lines or with V400 400; each plane laid out in 8 × 8
/// cells like a VIC-II bitmap; even planes in bank 0, odd in bank 1, the
/// address registers (`$D033`–`$D03A`) selecting an 8 KB block (bits
/// 1–3, or a 16 KB block with H640) for the even scan lines and with INT
/// and V400 (bits 5–7) for the odd ones. The colour is the planes' bits
/// (plane 0 the lowest), each XORed with `$D03B`; plane 2 is the
/// foreground for sprites.
fn bitplanes(
    b: &Board,
    y: u16,
    out: &mut [Rgb8],
    fg: &mut [bool],
    (left, right): (usize, usize),
    colours: &[Rgb8; 256],
) {
    let v = &b.vic;
    let r = &v.regs;
    let h640 = r[0x31] & 0x80 != 0;
    let v400 = r[0x31] & 0x08 != 0;
    let interlace = v400 && r[0x31] & 0x01 != 0;
    let texty = v.reg16(0x4e);
    if y < texty {
        return;
    }
    let lines: u16 = if v400 && !interlace { 400 } else { 200 };
    let line_h: u16 = if v400 && !interlace { 1 } else { 2 };
    let py = (y - texty) / line_h;
    if py >= lines {
        return;
    }
    let odd = interlace && (y - texty) % 2 == 1;
    let (width, px_w): (u32, usize) = if h640 { (640, 1) } else { (320, 2) };
    let planes = if h640 { 4 } else { 8 };
    let cells = width / 8;
    let row_off = u32::from(py / 8) * cells * 8 + u32::from(py % 8);
    let mut base = [0u32; 8];
    for (p, a) in base.iter_mut().enumerate().take(planes) {
        let reg = r[0x33 + p];
        let field = if odd { reg >> 5 } else { reg >> 1 } & 7;
        let block = if h640 {
            u32::from(field & 6) * 0x2000
        } else {
            u32::from(field) * 0x2000
        };
        *a = if p % 2 == 1 { 0x1_0000 } else { 0 } + block;
    }
    let (enable, complement) = (r[0x32], r[0x3b]);
    let textx = usize::from(v.reg16(0x4c));
    for cx in 0..cells {
        let mut bytes = [0u8; 8];
        for p in 0..8 {
            let data = if p < planes && enable & (1 << p) != 0 {
                ram(b, base[p] + row_off + cx * 8)
            } else {
                0
            };
            bytes[p] = if complement & (1 << p) != 0 {
                !data
            } else {
                data
            };
        }
        for i in 0..8 {
            let index = (0..8).fold(0u8, |acc, p| acc | (bytes[p] >> (7 - i) & 1) << p);
            let rgb = colours[usize::from(index)];
            for k in 0..px_w {
                let xx = textx + (cx as usize * 8 + i) * px_w + k;
                if (left..right).contains(&xx) {
                    out[xx] = rgb;
                    fg[xx] = bytes[2] >> (7 - i) & 1 != 0;
                }
            }
        }
    }
}

/// The VIC-II sprites of the line over the text; the collision bits.
fn sprites(
    b: &Board,
    y: u16,
    out: &mut [Rgb8],
    fg: &[bool],
    (left, right): (usize, usize),
    colours: &[Rgb8; 256],
) -> (u8, u8) {
    let r = &b.vic.regs;
    let enabled = r[0x15];
    if enabled == 0 {
        return (0, 0);
    }
    // The VIC-II raster of this physical line.
    let raster = (y + FIRST_PHYSICAL) / 2;
    let bank = u32::from(3 - (b.vic.cia2_pa & 3)) * 0x4000;
    let pointers = u32::from(r[0x6c]) | u32::from(r[0x6d]) << 8 | u32::from(r[0x6e] & 0x7f) << 16;
    // Which sprite owns each pixel (bit per sprite), for collisions.
    let mut owner = [0u8; WIDTH];
    let (mut ss, mut sd) = (0u8, 0u8);
    for s in (0..8usize).rev() {
        let bit = 1u8 << s;
        if enabled & bit == 0 {
            continue;
        }
        let sy = u16::from(r[1 + 2 * s]);
        let yexp = r[0x17] & bit != 0;
        let height = if yexp { 42 } else { 21 };
        let d = raster.wrapping_sub(sy + 1) & 0xff;
        if d >= height {
            continue;
        }
        let srow = u32::from(if yexp { d / 2 } else { d });
        let ptr = u32::from(ram(b, pointers + s as u32));
        let data = bank + ptr * 64 + srow * 3;
        let bits = u32::from(ram(b, data)) << 16
            | u32::from(ram(b, data + 1)) << 8
            | u32::from(ram(b, data + 2));
        let sx = i32::from(r[2 * s]) | if r[0x10] & bit != 0 { 256 } else { 0 };
        let x0 = i32::from(LEFT) + 2 * (sx - 24);
        let xexp = r[0x1d] & bit != 0;
        let mcm = r[0x1c] & bit != 0;
        let behind = r[0x1b] & bit != 0;
        let colour = col8(b, r[0x27 + s]);
        let (mc0, mc1) = (col8(b, r[0x25]), col8(b, r[0x26]));
        let scale = if xexp { 4 } else { 2 };
        for px in 0..24u32 {
            let c = if mcm {
                match bits >> (22 - (px & !1)) & 3 {
                    0 => None,
                    1 => Some(mc0),
                    2 => Some(colour),
                    _ => Some(mc1),
                }
            } else {
                (bits >> (23 - px) & 1 != 0).then_some(colour)
            };
            let Some(c) = c else {
                continue;
            };
            for k in 0..scale {
                let xx = x0 + px as i32 * scale + k;
                if xx < left as i32 || xx >= right as i32 {
                    continue;
                }
                let xx = xx as usize;
                if owner[xx] != 0 {
                    ss |= owner[xx] | bit;
                }
                owner[xx] |= bit;
                if fg[xx] {
                    sd |= bit;
                    if behind {
                        continue;
                    }
                }
                out[xx] = colours[usize::from(c)];
            }
        }
    }
    // Sprites drawn from 7 to 0: a lower number lies on top, except where
    // it is behind the foreground.
    (ss, sd)
}
