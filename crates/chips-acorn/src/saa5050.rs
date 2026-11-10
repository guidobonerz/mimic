//! Mullard SAA5050 teletext character generator: the BBC Micro's MODE 7.
//!
//! Each character cell is 6 dots by 10 lines; characters are 5 × 9 dots
//! in it (the left column and the bottom line blank). A row of 40 bytes
//! (7 bits) is decoded left to right; codes `&00`–`&1F` are control codes
//! shown as spaces (or as the held graphics character):
//!
//! | code | function | code | function |
//! | --- | --- | --- | --- |
//! | 01–07 | alphanumerics red … white | 11–17 | graphics red … white |
//! | 08 | flash | 09 | steady |
//! | 0C | normal height | 0D | double height |
//! | 18 | conceal | 19 / 1A | contiguous / separated graphics |
//! | 1C | black background | 1D | new background (the current colour) |
//! | 1E | hold graphics | 1F | release graphics |
//!
//! "Set-at" codes act on their own cell (steady, normal height, conceal,
//! contiguous, separated, the backgrounds, hold); the others from the
//! next cell on. Each line starts with white alphanumerics on black,
//! steady, normal height, contiguous, no hold.
//!
//! Graphics mode shows codes `&20`–`&3F` and `&60`–`&7F` as 2 × 3 mosaic
//! blocks (bits 0, 1 the top pair, 2, 3 the middle, 4, 6 the bottom;
//! columns of 3 dots, rows of 3, 4 and 3 lines); separated graphics leave
//! the left column and the bottom line of each block empty; `&40`–`&5F`
//! "blast through" as alphanumerics. Hold graphics shows the last mosaic
//! character in place of control codes. Double height: a row with `&0D`
//! shows the upper halves of its double-height characters, the row after
//! it the lower halves (its normal-height characters stay blank). Flash
//! alternates 3 : 1 on and off at about 0.75 Hz. Concealed characters show
//! as spaces (there is no reveal).
//!
//! Character rounding doubles the resolution to 12 half dots by 20 half
//! lines: where two dots touch only diagonally, half dots fill the corner
//! in the half lines next to the diagonal.
//!
//! The character set is Fig. 11 of the data sheet (English: `&23` £,
//! `&5B` ←, `&5C` ½, `&5D` →, `&5E` ↑, `&5F` #, `&60` —, `&7B` ¼, `&7C`
//! ‖, `&7D` ¾, `&7E` ÷, `&7F` a block), read from the scan dot by dot.
//!
//! Sources: Mullard, *SAA5050 Series Teletext Character Generator* data
//! sheet (July 1982): the block diagram and description, Table 1
//! (character data input decoding), Fig. 9 (graphics character), Fig. 10
//! (character format), Fig. 11 (the English character set), character
//! rounding and the flash oscillator. The set-at/set-after rules are those
//! of the teletext standard the chip implements (Level 1). The
//! implementation is this project's own.

use emu_core::{StateError, StateReader, StateWriter};

/// Fig. 11: rows of 5 dots (bit 4 the left one), 9 rows, codes &20–&7F.
const FONT: [[u8; 9]; 96] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 20
    [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04, 0x00, 0x00], // 21
    [0x0a, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 22
    [0x06, 0x09, 0x08, 0x1c, 0x08, 0x08, 0x1f, 0x00, 0x00], // 23
    [0x0e, 0x15, 0x14, 0x0e, 0x05, 0x15, 0x0e, 0x00, 0x00], // 24
    [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03, 0x00, 0x00], // 25
    [0x08, 0x14, 0x14, 0x08, 0x15, 0x12, 0x0d, 0x00, 0x00], // 26
    [0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // 27
    [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02, 0x00, 0x00], // 28
    [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08, 0x00, 0x00], // 29
    [0x04, 0x15, 0x0e, 0x04, 0x0e, 0x15, 0x04, 0x00, 0x00], // 2A
    [0x00, 0x04, 0x04, 0x1f, 0x04, 0x04, 0x00, 0x00, 0x00], // 2B
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08, 0x00], // 2C
    [0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00], // 2D
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00], // 2E
    [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00, 0x00, 0x00], // 2F
    [0x04, 0x0a, 0x11, 0x11, 0x11, 0x0a, 0x04, 0x00, 0x00], // 30
    [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e, 0x00, 0x00], // 31
    [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f, 0x00, 0x00], // 32
    [0x1f, 0x01, 0x02, 0x06, 0x01, 0x11, 0x0e, 0x00, 0x00], // 33
    [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02, 0x00, 0x00], // 34
    [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e, 0x00, 0x00], // 35
    [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e, 0x00, 0x00], // 36
    [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08, 0x00, 0x00], // 37
    [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e, 0x00, 0x00], // 38
    [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c, 0x00, 0x00], // 39
    [0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00], // 3A
    [0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x04, 0x08, 0x00], // 3B
    [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02, 0x00, 0x00], // 3C
    [0x00, 0x00, 0x1f, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00], // 3D
    [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08, 0x00, 0x00], // 3E
    [0x0e, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04, 0x00, 0x00], // 3F
    [0x0e, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0e, 0x00, 0x00], // 40
    [0x04, 0x0a, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x00, 0x00], // 41
    [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e, 0x00, 0x00], // 42
    [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e, 0x00, 0x00], // 43
    [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e, 0x00, 0x00], // 44
    [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f, 0x00, 0x00], // 45
    [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10, 0x00, 0x00], // 46
    [0x0e, 0x11, 0x10, 0x10, 0x13, 0x11, 0x0f, 0x00, 0x00], // 47
    [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11, 0x00, 0x00], // 48
    [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e, 0x00, 0x00], // 49
    [0x01, 0x01, 0x01, 0x01, 0x01, 0x11, 0x0e, 0x00, 0x00], // 4A
    [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11, 0x00, 0x00], // 4B
    [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f, 0x00, 0x00], // 4C
    [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11, 0x00, 0x00], // 4D
    [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x00, 0x00], // 4E
    [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e, 0x00, 0x00], // 4F
    [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10, 0x00, 0x00], // 50
    [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d, 0x00, 0x00], // 51
    [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11, 0x00, 0x00], // 52
    [0x0e, 0x11, 0x10, 0x0e, 0x01, 0x11, 0x0e, 0x00, 0x00], // 53
    [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00], // 54
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e, 0x00, 0x00], // 55
    [0x11, 0x11, 0x11, 0x0a, 0x0a, 0x04, 0x04, 0x00, 0x00], // 56
    [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a, 0x00, 0x00], // 57
    [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11, 0x00, 0x00], // 58
    [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00], // 59
    [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f, 0x00, 0x00], // 5A
    [0x00, 0x04, 0x08, 0x1f, 0x08, 0x04, 0x00, 0x00, 0x00], // 5B
    [0x10, 0x10, 0x10, 0x10, 0x16, 0x01, 0x02, 0x04, 0x07], // 5C
    [0x00, 0x04, 0x02, 0x1f, 0x02, 0x04, 0x00, 0x00, 0x00], // 5D
    [0x00, 0x04, 0x0e, 0x15, 0x04, 0x04, 0x00, 0x00, 0x00], // 5E
    [0x0a, 0x0a, 0x1f, 0x0a, 0x1f, 0x0a, 0x0a, 0x00, 0x00], // 5F
    [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00], // 60
    [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f, 0x00, 0x00], // 61
    [0x10, 0x10, 0x1e, 0x11, 0x11, 0x11, 0x1e, 0x00, 0x00], // 62
    [0x00, 0x00, 0x0f, 0x10, 0x10, 0x10, 0x0f, 0x00, 0x00], // 63
    [0x01, 0x01, 0x0f, 0x11, 0x11, 0x11, 0x0f, 0x00, 0x00], // 64
    [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0e, 0x00, 0x00], // 65
    [0x02, 0x04, 0x04, 0x0e, 0x04, 0x04, 0x04, 0x00, 0x00], // 66
    [0x00, 0x00, 0x0f, 0x11, 0x11, 0x11, 0x0f, 0x01, 0x0e], // 67
    [0x10, 0x10, 0x1e, 0x11, 0x11, 0x11, 0x11, 0x00, 0x00], // 68
    [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e, 0x00, 0x00], // 69
    [0x04, 0x00, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x08], // 6A
    [0x08, 0x08, 0x09, 0x0a, 0x0c, 0x0a, 0x09, 0x00, 0x00], // 6B
    [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e, 0x00, 0x00], // 6C
    [0x00, 0x00, 0x1a, 0x15, 0x15, 0x15, 0x15, 0x00, 0x00], // 6D
    [0x00, 0x00, 0x1e, 0x11, 0x11, 0x11, 0x11, 0x00, 0x00], // 6E
    [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e, 0x00, 0x00], // 6F
    [0x00, 0x00, 0x1e, 0x11, 0x11, 0x11, 0x1e, 0x10, 0x10], // 70
    [0x00, 0x00, 0x0f, 0x11, 0x11, 0x11, 0x0f, 0x01, 0x01], // 71
    [0x00, 0x00, 0x0b, 0x0c, 0x08, 0x08, 0x08, 0x00, 0x00], // 72
    [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e, 0x00, 0x00], // 73
    [0x04, 0x04, 0x0e, 0x04, 0x04, 0x04, 0x02, 0x00, 0x00], // 74
    [0x00, 0x00, 0x11, 0x11, 0x11, 0x11, 0x0f, 0x00, 0x00], // 75
    [0x00, 0x00, 0x11, 0x11, 0x0a, 0x0a, 0x04, 0x00, 0x00], // 76
    [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a, 0x00, 0x00], // 77
    [0x00, 0x00, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x00, 0x00], // 78
    [0x00, 0x00, 0x11, 0x11, 0x11, 0x11, 0x0f, 0x01, 0x0e], // 79
    [0x00, 0x00, 0x1f, 0x02, 0x04, 0x08, 0x1f, 0x00, 0x00], // 7A
    [0x08, 0x08, 0x08, 0x08, 0x09, 0x03, 0x05, 0x07, 0x01], // 7B
    [0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x00, 0x00], // 7C
    [0x18, 0x04, 0x18, 0x04, 0x19, 0x03, 0x05, 0x07, 0x01], // 7D
    [0x00, 0x04, 0x00, 0x1f, 0x00, 0x04, 0x00, 0x00, 0x00], // 7E
    [0x1f, 0x1f, 0x1f, 0x1f, 0x1f, 0x1f, 0x1f, 0x00, 0x00], // 7F
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Attrs {
    fg: u8,
    bg: u8,
    graphics: bool,
    separated: bool,
    flash: bool,
    double: bool,
    hold: bool,
    conceal: bool,
}

impl Attrs {
    fn line_start() -> Self {
        Self {
            fg: 7,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saa5050 {
    attrs: Attrs,
    /// The held mosaic character and whether it was separated.
    held: Option<(u8, bool)>,
    /// Half line within the character row (0–19).
    line: u8,
    /// This row has a double-height code; this row shows lower halves.
    row_double: bool,
    lower: bool,
    /// Flash characters are shown now.
    flash_on: bool,
}

/// The dot row `r` (0–9) of glyph `code`.
fn glyph_row(code: u8, r: u8) -> u8 {
    if !(0x20..0x80).contains(&code) || r >= 9 {
        return 0;
    }
    FONT[usize::from(code - 0x20)][usize::from(r)]
}

/// Mosaic dots of `code` in dot row `r` (6 bits, bit 5 the left dot).
fn mosaic_row(code: u8, r: u8, separated: bool) -> u8 {
    let (block, last) = match r {
        0..=2 => (0, 2),
        3..=6 => (1, 6),
        _ => (2, 9),
    };
    let pairs = [
        code & 3,
        (code >> 2) & 3,
        ((code >> 4) & 1) | ((code >> 5) & 2),
    ];
    let pair = pairs[block];
    if separated && r == last {
        return 0;
    }
    let (left, right) = if separated {
        (0b011_000, 0b000_011)
    } else {
        (0b111_000, 0b000_111)
    };
    (if pair & 1 != 0 { left } else { 0 }) | (if pair & 2 != 0 { right } else { 0 })
}

/// 12 half dots (bit 11 the left one) of an alphanumeric glyph at half
/// line `hl` (0–19 of the 10-line cell), rounded.
fn alpha_half_dots(code: u8, hl: u8) -> u16 {
    let r = hl / 2;
    let row = glyph_row(code, r);
    let other = if hl % 2 == 1 {
        glyph_row(code, r + 1)
    } else if r > 0 {
        glyph_row(code, r - 1)
    } else {
        0
    };
    let dot = |v: u8, c: u16| c < 5 && v >> (4 - c) & 1 != 0;
    let mut out = 0u16;
    // Glyph column c is cell column c + 1: half dots 2c + 2 and 2c + 3,
    // bit 11 − n for half dot n.
    let half = |n: u16| 1u16 << (11 - n);
    for c in 0..5u16 {
        if dot(row, c) {
            out |= half(2 * c + 2) | half(2 * c + 3);
        }
    }
    for c in 0..4u16 {
        let (a, b, x, y) = (
            dot(row, c),
            dot(row, c + 1),
            dot(other, c),
            dot(other, c + 1),
        );
        // This row's dot at c and the other row's at c + 1 touch only at
        // the corner: half a dot to the right of c here.
        if a && y && !b && !x {
            out |= half(2 * c + 4);
        }
        // This row's dot at c + 1, the other row's at c: half a dot to the
        // left of c + 1 here.
        if b && x && !a && !y {
            out |= half(2 * c + 3);
        }
    }
    out
}

impl Saa5050 {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new field: rows start again; `field` counts fields (flash).
    pub fn new_field(&mut self, field: u64) {
        self.row_double = false;
        self.lower = false;
        self.line = 0;
        // About 0.75 Hz, on three quarters of the time (64 fields).
        self.flash_on = field % 64 < 48;
    }

    /// A new scan line at half line `hl` (0–19) of its character row.
    pub fn new_line(&mut self, hl: u8) {
        if hl < self.line {
            // A new character row.
            self.lower = self.row_double && !self.lower;
            self.row_double = false;
        }
        self.line = hl.min(19);
        self.attrs = Attrs::line_start();
        self.held = None;
    }

    /// One character: 12 half dots of physical colours (0–7).
    pub fn character(&mut self, byte: u8, out: &mut [u8; 12]) {
        let code = byte & 0x7f;
        let mut a = self.attrs;
        // Set-at codes change this cell.
        match code {
            0x09 => a.flash = false,
            0x0c => {
                a.double = false;
                self.held = None;
            }
            0x18 => a.conceal = true,
            0x19 => a.separated = false,
            0x1a => a.separated = true,
            0x1c => a.bg = 0,
            0x1d => a.bg = a.fg,
            0x1e => a.hold = true,
            _ => {}
        }
        if code == 0x0d {
            self.row_double = true;
        }
        let shown = a;
        let dots = if code < 0x20 {
            match (shown.hold, self.held) {
                (true, Some((h, sep))) => self.mosaic_dots(h, sep, shown.double),
                _ => 0,
            }
        } else if shown.graphics && code & 0x20 != 0 {
            self.held = Some((code, shown.separated));
            self.mosaic_dots(code, shown.separated, shown.double)
        } else {
            alpha_half_dots(code, self.half_line(shown.double))
        };
        let visible =
            !(shown.conceal || (shown.flash && !self.flash_on)) && (!self.lower || shown.double);
        for (i, px) in out.iter_mut().enumerate() {
            *px = if visible && dots >> (11 - i) & 1 != 0 {
                shown.fg
            } else {
                shown.bg
            };
        }
        // Set-after codes change the cells after this one.
        match code {
            0x01..=0x07 => {
                a.fg = code;
                a.graphics = false;
                a.conceal = false;
                self.held = None;
            }
            0x08 => a.flash = true,
            0x0d => {
                a.double = true;
                self.held = None;
            }
            0x11..=0x17 => {
                a.fg = code - 0x10;
                a.graphics = true;
                a.conceal = false;
            }
            0x1f => a.hold = false,
            _ => {}
        }
        self.attrs = a;
    }

    /// The half line of the glyph shown on this scan line.
    fn half_line(&self, double: bool) -> u8 {
        if double {
            self.line / 2 + if self.lower { 10 } else { 0 }
        } else {
            self.line
        }
    }

    fn mosaic_dots(&self, code: u8, separated: bool, double: bool) -> u16 {
        let row = mosaic_row(code, self.half_line(double) / 2, separated);
        let mut out = 0u16;
        for c in 0..6 {
            if row >> (5 - c) & 1 != 0 {
                out |= 0b11 << (10 - 2 * c);
            }
        }
        out
    }

    pub fn save(&self, w: &mut StateWriter) {
        let a = &self.attrs;
        w.u8(a.fg);
        w.u8(a.bg);
        for b in [
            a.graphics,
            a.separated,
            a.flash,
            a.double,
            a.hold,
            a.conceal,
        ] {
            w.bool(b);
        }
        w.bool(self.held.is_some());
        let (h, s) = self.held.unwrap_or((0, false));
        w.u8(h);
        w.bool(s);
        w.u8(self.line);
        for b in [self.row_double, self.lower, self.flash_on] {
            w.bool(b);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Self::new();
        n.attrs.fg = r.u8()? & 7;
        n.attrs.bg = r.u8()? & 7;
        n.attrs.graphics = r.bool()?;
        n.attrs.separated = r.bool()?;
        n.attrs.flash = r.bool()?;
        n.attrs.double = r.bool()?;
        n.attrs.hold = r.bool()?;
        n.attrs.conceal = r.bool()?;
        let held = r.bool()?;
        let h = r.u8()?;
        let s = r.bool()?;
        n.held = held.then_some((h & 0x7f, s));
        n.line = r.u8()?.min(19);
        n.row_double = r.bool()?;
        n.lower = r.bool()?;
        n.flash_on = r.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &mut Saa5050, bytes: &[u8], hl: u8) -> Vec<[u8; 12]> {
        s.new_line(hl);
        bytes
            .iter()
            .map(|&b| {
                let mut out = [0; 12];
                s.character(b, &mut out);
                out
            })
            .collect()
    }

    fn lit(cell: &[u8; 12]) -> String {
        cell.iter()
            .map(|&c| if c != 0 { '#' } else { '.' })
            .collect()
    }

    #[test]
    fn letters_come_from_the_table() {
        let mut s = Saa5050::new();
        s.new_field(0);
        // 'A' row 0 is ..#.. : glyph column 2, cell column 3.
        let cells = line(&mut s, b"A", 0);
        assert_eq!(lit(&cells[0]), "......##....");
        assert_eq!(cells[0][6], 7, "white on black");
    }

    #[test]
    fn colour_codes_act_on_the_next_cell() {
        let mut s = Saa5050::new();
        s.new_field(0);
        let cells = line(&mut s, &[0x01, b'H', 0x1d, b' '], 2);
        assert!(
            cells[0].iter().all(|&c| c == 0),
            "the control cell: a space"
        );
        assert!(cells[1].contains(&1), "a red H");
        assert!(cells[2].iter().all(|&c| c == 1), "new background is set-at");
        assert!(cells[3].iter().all(|&c| c == 1));
    }

    #[test]
    fn mosaics_and_separated_graphics() {
        let mut s = Saa5050::new();
        s.new_field(0);
        let cells = line(&mut s, &[0x17, 0x7f, 0x1a, 0x7f], 0);
        assert_eq!(lit(&cells[1]), "############");
        assert_eq!(
            lit(&cells[3]),
            "..####..####",
            "separated: left column empty"
        );
        // Blast-through: &41 in graphics mode is an A.
        let cells = line(&mut s, &[0x17, b'A'], 0);
        assert_eq!(lit(&cells[1]), "......##....");
    }

    #[test]
    fn hold_graphics_repeats_the_last_mosaic() {
        let mut s = Saa5050::new();
        s.new_field(0);
        let cells = line(&mut s, &[0x17, 0x7f, 0x1e, 0x11], 0);
        assert_eq!(lit(&cells[2]), "############", "held under the hold code");
        assert_eq!(
            lit(&cells[3]),
            "############",
            "and under the colour change"
        );
        assert!(
            cells[3].contains(&7),
            "still white: graphics red comes after"
        );
    }

    #[test]
    fn double_height_spans_two_rows() {
        let mut s = Saa5050::new();
        s.new_field(0);
        let top = line(&mut s, &[0x0d, b'A', b'B'], 0);
        assert_eq!(lit(&top[1]), "......##....");
        for hl in 1..20 {
            line(&mut s, &[0x0d, b'A'], hl);
        }
        // The next row: lower halves only, normal height blank.
        let bottom = line(&mut s, &[b'N', 0x0d, b'A'], 0);
        assert_eq!(lit(&bottom[0]), "............");
        let a_lower = alpha_half_dots(b'A', 10);
        let shown: u16 = bottom[2].iter().fold(0, |v, &c| v << 1 | u16::from(c != 0));
        assert_eq!(shown, a_lower);
    }

    #[test]
    fn rounding_fills_diagonals() {
        // '/' steps one dot per row: rounding adds half dots.
        let plain: u32 = (0..12)
            .map(|hl| alpha_half_dots(b'/', hl).count_ones())
            .sum();
        let unrounded: u32 = (0..6)
            .map(|r| 4 * u32::from(glyph_row(b'/', r)).count_ones())
            .sum();
        assert!(plain > unrounded, "{plain} > {unrounded}");
    }

    #[test]
    fn savestate_round_trip() {
        let mut s = Saa5050::new();
        s.new_field(5);
        line(&mut s, &[0x13, 0x7f, 0x1e], 4);
        let mut w = StateWriter::new();
        s.save(&mut w);
        let bytes = w.into_bytes();
        let mut t = Saa5050::new();
        t.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(s, t);
    }
}
