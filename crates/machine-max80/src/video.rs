//! The MAX-80's picture: an HD46505 CRTC addresses 2 KB of video RAM, and
//! each character's dots come from 2 KB of font RAM. There is no character
//! ROM: the operating system loads its font from the boot disk, so the
//! screen shows nothing readable before that.
//!
//! A character is 8 dots of the 15.2 MHz dot clock wide. Codes `$00–$7F`
//! take their eight rows from the first font kilobyte (code · 8 + row) and
//! are blank below row 7. Codes `$80–$FF` take rows 0–7 from the second
//! kilobyte at (code & `$3F`) · 8 and rows 8–15 from 512 bytes further on,
//! so `$C0–$FF` repeat `$80–$BF`: room for the TRS-80's 2 × 3 block
//! graphics on a twelve-line row. Past row 15 the rows repeat.
//!
//! Lobo's table gives 120 characters a line and 264 lines a field (15.83
//! kHz, 59.97 Hz) with 80 × 24 characters of nine lines; the TRS-80
//! compatible mode shows 64 × 16. With VLATCH bit 3 every second character
//! is shown twice as wide (40 or 32 columns).
//!
//! The picture here is the displayed area, centred on a canvas of 640 ×
//! 240 dots, drawn whole at the end of each field from the registers and
//! memory as they are then. Not built: changes inside a field, the CRTC's
//! cursor (none of Lobo's systems uses it), interlace.
//!
//! Sources: see `io.rs`.

use crate::io::Geometry;

pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 240;
pub const VRAM: usize = 0x800;
pub const FONT: usize = 0x800;

/// Row `row` of the character `code`.
pub fn glyph_row(font: &[u8], code: u8, row: u32) -> u8 {
    let row = (row & 15) as usize;
    if code < 0x80 {
        return if row < 8 {
            font[usize::from(code) * 8 + row]
        } else {
            0
        };
    }
    let c = usize::from(code & 0x3f) * 8;
    if row < 8 {
        font[0x400 + c + row]
    } else {
        font[0x600 + c + row - 8]
    }
}

/// Draw the field into `pixels` (`WIDTH` × `HEIGHT`, 0 dark, 1 lit).
pub fn render(g: Option<Geometry>, wide: bool, vram: &[u8], font: &[u8], pixels: &mut [u8]) {
    pixels.fill(0);
    let Some(g) = g else {
        return;
    };
    let cols = g.cols.min(WIDTH / 8);
    let lines = (g.rows * g.row_lines).min(HEIGHT);
    let x0 = (WIDTH - cols * 8) / 2;
    let y0 = (HEIGHT - lines) / 2;
    for y in 0..lines {
        let (row, line) = (y / g.row_lines, y % g.row_lines);
        let base = u32::from(g.start) + row * g.cols;
        let out = ((y0 + y) * WIDTH + x0) as usize;
        for col in 0..cols {
            // Double width: the even character of each pair, over both.
            let shown = if wide { col & !1 } else { col };
            let code = vram[(base + shown) as usize & (VRAM - 1)];
            let bits = glyph_row(font, code, line);
            let cell = &mut pixels[out + (col * 8) as usize..][..8];
            for (d, p) in cell.iter_mut().enumerate() {
                let bit = if wide {
                    7 - ((col as usize & 1) * 8 + d) / 2
                } else {
                    7 - d
                };
                *p = bits >> bit & 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lobo() -> Geometry {
        Geometry {
            htotal: 120,
            cols: 80,
            rows: 24,
            row_lines: 9,
            lines: 264,
            start: 0,
        }
    }

    #[test]
    fn both_font_halves_and_the_tall_graphics() {
        let mut font = vec![0u8; FONT];
        font[0x41 * 8 + 7] = 0x55;
        font[0x400 + 3 * 8] = 0xaa;
        font[0x600 + 3 * 8 + 2] = 0x33;
        assert_eq!(glyph_row(&font, 0x41, 7), 0x55);
        assert_eq!(glyph_row(&font, 0x41, 8), 0, "blank below row 7");
        assert_eq!(glyph_row(&font, 0x83, 0), 0xaa);
        assert_eq!(glyph_row(&font, 0xc3, 0), 0xaa, "$C0 repeats $80");
        assert_eq!(glyph_row(&font, 0x83, 10), 0x33);
        assert_eq!(glyph_row(&font, 0x83, 16), 0xaa, "the rows repeat");
    }

    #[test]
    fn the_picture_is_centred_and_double_width_stretches() {
        let mut vram = vec![0x20u8; VRAM];
        let mut font = vec![0u8; FONT];
        font[8] = 0b1100_0001; // code 1, row 0
        vram[0] = 1;
        let mut px = vec![0u8; (WIDTH * HEIGHT) as usize];
        render(Some(lobo()), false, &vram, &font, &mut px);
        // 24 rows of 9 lines: 216 lines, 12 above.
        let at = (12 * WIDTH) as usize;
        assert_eq!(&px[at..at + 8], &[1, 1, 0, 0, 0, 0, 0, 1]);
        assert!(px[..at].iter().all(|&p| p == 0));
        render(Some(lobo()), true, &vram, &font, &mut px);
        assert_eq!(
            &px[at..at + 16],
            &[1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1]
        );
        // Without a picture the screen is dark.
        render(None, false, &vram, &font, &mut px);
        assert!(px.iter().all(|&p| p == 0));
    }
}
