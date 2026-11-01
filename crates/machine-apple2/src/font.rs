//! Character glyphs when no video ROM is supplied: this project's own 5×7
//! dot-matrix font for ASCII $20–$7F, drawn in columns 1–5 of the Apple's
//! 7×8 character cell (row 7 blank). Each row is five bits, bit 4 the
//! leftmost dot.

use emu_core::font::GLYPHS;

/// Dots of `ascii` ($20–$7F; others show the checkerboard) in cell row
/// `row` (0–7) as seven bits, bit 0 the leftmost dot (the order the video
/// shift register sends them).
pub fn row(ascii: u8, row: u8) -> u8 {
    if row >= 7 {
        return 0;
    }
    let g = match ascii {
        0x20..=0x7f => &GLYPHS[usize::from(ascii - 0x20)],
        _ => &GLYPHS[95],
    };
    // Five dots, bit 4 leftmost, into cell columns 1–5.
    let five = g[usize::from(row)];
    (0..5).fold(0, |acc, i| acc | (((five >> (4 - i)) & 1) << (i + 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_rows_are_mirrored_into_cell_columns() {
        // 'L': the left stroke is cell column 1 = bit 1.
        assert_eq!(row(b'L', 0), 0b0000_0010);
        assert_eq!(row(b'L', 6), 0b0011_1110);
        assert_eq!(row(b' ', 3), 0);
        assert_eq!(row(b'A', 7), 0, "row 7 is blank");
    }
}
