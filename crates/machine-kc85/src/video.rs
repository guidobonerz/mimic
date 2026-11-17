//! The KC85's picture: 320 × 256 pixels out of the IRM, a foreground and
//! a background colour per cell.
//!
//! **Colour byte:** bit 7 blink, bits 6–3 the foreground (bit 3 blue, 4
//! red, 5 green, 6 the second set of hues), bits 2–0 the background
//! (blue, red, green; darker than the foreground's colours). A blinking
//! cell shows its foreground in the background's colour while the blink
//! flip-flop (CTC channel 2's output) is set and PIO B bit 7 allows it.
//!
//! **KC85/2 and /3:** eight pixels per byte, one colour byte per 8 × 4
//! pixels. The left 32 columns and the right 8 are addressed apart, and
//! the line number's bits are spread over the address (the hardware
//! takes the picture counter's bits where the IRM's address multiplexer
//! had room):
//!
//! | Area | Pixel byte offset in the IRM | Colour byte offset |
//! | --- | --- | --- |
//! | columns 0–31 | `x + y[3:2]·$20 + y[1:0]·$80 + y[7:4]·$200` | `$2800 + x + y[7:2]·$20` |
//! | columns 32–39 | `$2000 + x[2:0] + y[5:4]·8 + y[3:2]·$20 + y[1:0]·$80 + y[7:6]·$200` | `$3000 + x[2:0] + y[5:4]·8 + y[3:2]·$20 + y[7:6]·$80` |
//!
//! **KC85/4 and /5:** two images, each a pixel and a colour plane of
//! 16 KB, column by column: offset `x·$100 + y`, so one colour byte per
//! 8 × 1 pixels. Port `$84` bit 0 picks the image shown; with bit 3
//! clear the two planes together give four colours per pixel (pixel
//! plane: red, colour plane: turquoise, both: white).
//!
//! Timing: 312 lines, 256 of them picture; the KC85/2 and /3 run their
//! CPU at 1.75 MHz with 112 clocks a line (pixel clock 7 MHz), the
//! KC85/4 and /5 at 1.7734475 MHz with 113.5 (7.0938 MHz).
//!
//! Sources: KC85/3 Systemhandbuch (IRM layout, colour table); KC85/4
//! Systemhandbuch (planes, port `$84`, HIRES colours). The KC85/2's and
//! /3's address spreading is checked by what the user's CAOS ROMs draw
//! (their text is legible in both areas, and CAOS writes a character's
//! two colour bytes `$20` apart). The colour values are this project's
//! estimate of the RGB output, not measured.

use mimic_core::frame::Rgb8;

use crate::memory::{BLOCK, P84_IMAGE_SHOWN, P84_LOW_COLOUR};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 256;
pub const COLUMNS: usize = 40;
pub const LINES: u32 = 312;

const fn rgb(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 { r, g, b }
}

/// Palette: 16 foreground colours, then the 8 background colours.
pub const PALETTE: [Rgb8; 24] = [
    // Foreground: black, blue, red, magenta, green, cyan, yellow, white.
    rgb(0x00, 0x00, 0x00),
    rgb(0x00, 0x00, 0xff),
    rgb(0xff, 0x00, 0x00),
    rgb(0xff, 0x00, 0xff),
    rgb(0x00, 0xff, 0x00),
    rgb(0x00, 0xff, 0xff),
    rgb(0xff, 0xff, 0x00),
    rgb(0xff, 0xff, 0xff),
    // The second set: black, violet, orange, purple, blue-green,
    // green-blue, yellow-green, white.
    rgb(0x00, 0x00, 0x00),
    rgb(0xa0, 0x00, 0xff),
    rgb(0xff, 0xa0, 0x00),
    rgb(0xff, 0x00, 0xa0),
    rgb(0x00, 0xff, 0xa0),
    rgb(0x00, 0xa0, 0xff),
    rgb(0xa0, 0xff, 0x00),
    rgb(0xff, 0xff, 0xff),
    // Background: the first set, darker.
    rgb(0x00, 0x00, 0x00),
    rgb(0x00, 0x00, 0xa0),
    rgb(0xa0, 0x00, 0x00),
    rgb(0xa0, 0x00, 0xa0),
    rgb(0x00, 0xa0, 0x00),
    rgb(0x00, 0xa0, 0xa0),
    rgb(0xa0, 0xa0, 0x00),
    rgb(0xa0, 0xa0, 0xa0),
];

/// The four colours of the KC85/4's high colour resolution: black, red,
/// turquoise, white (palette indices).
const HIRES: [u8; 4] = [0, 2, 5, 7];

/// IRM offsets of the pixel and the colour byte of column `x` in line
/// `y` (KC85/2, /3).
pub fn offsets_3(x: usize, y: usize) -> (usize, usize) {
    if x < 32 {
        (
            x | (y >> 2 & 3) << 5 | (y & 3) << 7 | (y >> 4 & 15) << 9,
            0x2800 + (x | (y >> 2 & 0x3f) << 5),
        )
    } else {
        let x = x & 7;
        (
            0x2000 + (x | (y >> 4 & 3) << 3 | (y >> 2 & 3) << 5 | (y & 3) << 7 | (y >> 6 & 3) << 9),
            0x3000 + (x | (y >> 4 & 3) << 3 | (y >> 2 & 3) << 5 | (y >> 6 & 3) << 7),
        )
    }
}

/// Eight pixels from a pixel and a colour byte.
fn cell(pixels: u8, colour: u8, blink: bool, out: &mut [u8]) {
    let bg = 16 + (colour & 7);
    let fg = if blink && colour & 0x80 != 0 {
        bg
    } else {
        colour >> 3 & 15
    };
    for (i, p) in out.iter_mut().enumerate() {
        *p = if pixels & (0x80 >> i) != 0 { fg } else { bg };
    }
}

/// Line `y` of a KC85/2 or /3 into `out` (320 palette indices).
pub fn line_3(irm: &[u8], y: usize, blink: bool, out: &mut [u8]) {
    for x in 0..COLUMNS {
        let (p, c) = offsets_3(x, y);
        cell(irm[p], irm[c], blink, &mut out[x * 8..x * 8 + 8]);
    }
}

/// Line `y` of a KC85/4 or /5 into `out`.
pub fn line_4(irm: &[u8], port84: u8, y: usize, blink: bool, out: &mut [u8]) {
    let image = usize::from(port84 & P84_IMAGE_SHOWN) * 2 * BLOCK;
    for x in 0..COLUMNS {
        let o = image + x * 0x100 + y;
        let (p, c) = (irm[o], irm[o + BLOCK]);
        let out = &mut out[x * 8..x * 8 + 8];
        if port84 & P84_LOW_COLOUR != 0 {
            cell(p, c, blink, out);
        } else {
            for (i, px) in out.iter_mut().enumerate() {
                let bit = 0x80 >> i;
                *px = HIRES[usize::from(p & bit != 0) | usize::from(c & bit != 0) << 1];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cell_has_its_own_bytes() {
        let mut pixels = vec![false; 0x2800];
        let mut colours = vec![0u8; 0x1000];
        for y in 0..256 {
            for x in 0..COLUMNS {
                let (p, c) = offsets_3(x, y);
                assert!(p < 0x2800, "{x} {y}");
                assert!(!pixels[p], "pixel byte used twice: {x} {y}");
                pixels[p] = true;
                assert!((0x2800..0x3200).contains(&c), "{x} {y}");
                colours[c - 0x2800] += 1;
            }
        }
        assert!(pixels.iter().all(|&p| p));
        // One colour byte per four lines.
        assert!(colours[..0xa00].iter().all(|&n| n == 4));
    }

    #[test]
    fn a_blinking_cell_hides_its_foreground() {
        let mut out = [0u8; 8];
        cell(0xf0, 0x80 | 7 << 3 | 1, false, &mut out);
        assert_eq!(out, [7, 7, 7, 7, 17, 17, 17, 17]);
        cell(0xf0, 0x80 | 7 << 3 | 1, true, &mut out);
        assert_eq!(out, [17; 8]);
    }

    #[test]
    fn high_colour_resolution_mixes_the_planes() {
        let mut irm = vec![0u8; 4 * BLOCK];
        irm[0x0305] = 0b1100_0000;
        irm[BLOCK + 0x0305] = 0b1010_0000;
        let mut out = [0u8; 320];
        line_4(&irm, 0, 5, false, &mut out);
        assert_eq!(out[24..28], [7, 2, 5, 0]);
    }
}
