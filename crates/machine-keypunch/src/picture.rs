//! The picture: the card in the punch station, drawn to scale, with its
//! printed digit rows, the holes, the characters the 029 printed along
//! the top edge, the program's field lines, the column indicator, and
//! status lines below.
//!
//! Scale: a column pitch of 0.087 in is 9 pixels; a row pitch of 0.25 in
//! is 26 pixels. The card is 7 3/8 × 3 1/4 in (763 × 338 pixels), column 1
//! 0.251 in from the left edge, row 12 0.25 in from the top edge; holes are
//! 0.055 × 0.125 in (6 × 13 pixels). Source: IBM "Reference Manual, IBM 29
//! Card Punch" (A24-3332), card dimensions; ANSI X3.21 (rectangular holes).

use emu_core::font;
use emu_core::frame::{FieldParity, OutputType, Rgb8, SignalInfo, VideoStandard};
use media::cards::COLUMNS;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 440;

const CARD_X: usize = 18;
const CARD_Y: usize = 24;
const CARD_W: usize = 763;
const CARD_H: usize = 338;
const COL_PITCH: usize = 9;
const ROW_PITCH: usize = 26;
/// Column 1's centre from the card's left edge, row 12's from its top.
const COL1: usize = 26;
const ROW12: usize = 26;
/// Status text lines.
pub const STATUS_LINES: usize = 3;
const STATUS_Y: usize = 384;

const DESK: u8 = 0;
const CARD: u8 = 1;
const CARD_PRINT: u8 = 2;
const HOLE: u8 = 3;
const INK: u8 = 4;
const TEXT: u8 = 5;
const INDICATOR: u8 = 6;
const DIM: u8 = 7;

pub const PALETTE: [Rgb8; 8] = [
    Rgb8 {
        r: 0x2b,
        g: 0x2f,
        b: 0x33,
    },
    Rgb8 {
        r: 0xf1,
        g: 0xe6,
        b: 0xc8,
    },
    Rgb8 {
        r: 0xc5,
        g: 0x8f,
        b: 0x7a,
    },
    Rgb8 {
        r: 0x15,
        g: 0x17,
        b: 0x1a,
    },
    Rgb8 {
        r: 0x1c,
        g: 0x1c,
        b: 0x28,
    },
    Rgb8 {
        r: 0xd8,
        g: 0xdd,
        b: 0xe2,
    },
    Rgb8 {
        r: 0xe0,
        g: 0x4a,
        b: 0x3a,
    },
    Rgb8 {
        r: 0x8a,
        g: 0x93,
        b: 0x9b,
    },
];

/// The picture's signal metadata (progressive, `fps` fields a second).
pub fn signal(fps: u32) -> SignalInfo {
    SignalInfo {
        pixel_clock_hz: WIDTH * HEIGHT * fps,
        active_pixels: WIDTH,
        total_pixels: WIDTH,
        lines_per_field: HEIGHT,
        standard: VideoStandard::Ntsc,
        output: OutputType::Rgb,
        field: FieldParity::Progressive,
    }
}

/// What the picture shows.
pub struct View<'a> {
    /// The card in the punch station: columns and printed characters
    /// (ASCII, 0 for none).
    pub card: Option<(&'a [u16; COLUMNS], &'a [u8; COLUMNS])>,
    /// The column at the punch dies (0-based; 80 = past the last).
    pub column: usize,
    /// Field starts (1-based columns) of the program in use.
    pub fields: &'a [usize],
    pub status: [String; STATUS_LINES],
}

fn col_x(c: usize) -> usize {
    CARD_X + COL1 + c * COL_PITCH
}

/// Row index 0 = row 12, 1 = row 11, 2 = row 0, 3–11 = rows 1–9.
fn row_y(i: usize) -> usize {
    CARD_Y + ROW12 + i * ROW_PITCH
}

pub fn render(px: &mut [u8], view: &View<'_>) {
    let w = WIDTH as usize;
    px.fill(DESK);
    let mut put = |x: usize, y: usize, c: u8| {
        if x < w && y < HEIGHT as usize {
            px[y * w + x] = c;
        }
    };
    if let Some((columns, printed)) = view.card {
        // The card, its top-left corner cut.
        for y in 0..CARD_H {
            for x in 0..CARD_W {
                if x + y >= 14 {
                    put(CARD_X + x, CARD_Y + y, CARD);
                }
            }
        }
        // Field lines of the program.
        for &f in view.fields {
            if (2..=COLUMNS).contains(&f) {
                let x = col_x(f - 1) - 5;
                for y in CARD_Y + 20..CARD_Y + CARD_H - 8 {
                    put(x, y, CARD_PRINT);
                }
            }
        }
        // Digits 0–9 printed in their rows, holes punched over them.
        for c in 0..COLUMNS {
            let cx = col_x(c);
            for i in 0..12 {
                let cy = row_y(i);
                let bit = 11 - i;
                if columns[c] >> bit & 1 != 0 {
                    for y in cy - 6..cy + 7 {
                        for x in cx - 3..cx + 3 {
                            put(x, y, HOLE);
                        }
                    }
                } else if i >= 2 {
                    let digit = if i == 2 { b'0' } else { b'0' + (i - 2) as u8 };
                    glyph(&mut put, digit, cx - 2, cy - 3, 1, CARD_PRINT);
                }
            }
            // The 029's printing along the top edge.
            if printed[c] != 0 {
                glyph(&mut put, printed[c], cx - 2, CARD_Y + 4, 2, INK);
            }
        }
    } else {
        text(
            &mut put,
            "NO CARD AT THE PUNCH STATION - FEED (F1) OR ENTER",
            CARD_X + 180,
            CARD_Y + CARD_H / 2,
            TEXT,
        );
    }
    // Column indicator under the punch dies.
    if view.column < COLUMNS {
        let cx = col_x(view.column);
        for dy in 0..8 {
            for x in cx - dy / 2..=cx + dy / 2 {
                put(x, CARD_Y + CARD_H + 4 + dy, INDICATOR);
            }
        }
    }
    for (n, line) in view.status.iter().enumerate() {
        let colour = if n == 0 { TEXT } else { DIM };
        text(&mut put, line, CARD_X, STATUS_Y + n * 16, colour);
    }
}

/// A 5 × 7 glyph at (`x`, `y`), `scale` times as tall.
fn glyph(put: &mut impl FnMut(usize, usize, u8), ch: u8, x: usize, y: usize, scale: usize, c: u8) {
    for r in 0..7 {
        let bits = font::row(ch, r as u8);
        for k in 0..5 {
            if bits & (1 << (5 - k)) != 0 {
                for s in 0..scale {
                    put(x + k, y + r * scale + s, c);
                }
            }
        }
    }
}

fn text(put: &mut impl FnMut(usize, usize, u8), s: &str, x: usize, y: usize, c: u8) {
    for (i, ch) in s.bytes().enumerate() {
        glyph(put, ch, x + i * 7, y, 1, c);
    }
}
