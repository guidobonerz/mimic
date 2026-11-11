//! The BBC Micro keyboard: a matrix of 10 columns and 8 rows behind the
//! system VIA's port A. PA0–PA3 select the column, PA4–PA6 the row; PA7
//! reads 1 while that key is down (with the keyboard enabled, addressable
//! latch bit 3 low). With the latch bit high the keyboard scans itself and
//! CA2 rises while any key of rows 1–7 is down. Row 0 holds SHIFT and CTRL
//! (columns 0 and 1) and the start-up option links (columns 2–9, all
//! open here: MODE 7, no boot).
//!
//! Machine key = row × 16 + column, the MOS's internal key number:
//!
//! | row | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 0 | SHIFT | CTRL | link | link | link | link | link | link | link | link |
//! | 1 | Q | 3 | 4 | 5 | f4 | 8 | f7 | - | ^ | ← |
//! | 2 | f0 | W | E | T | 7 | I | 9 | 0 | _ | ↓ |
//! | 3 | 1 | 2 | D | R | 6 | U | O | P | [ | ↑ |
//! | 4 | CAPS LOCK | A | X | F | Y | J | K | @ | : | RETURN |
//! | 5 | SHIFT LOCK | S | C | G | H | N | L | ; | ] | DELETE |
//! | 6 | TAB | Z | SPACE | V | B | M | , | . | / | COPY |
//! | 7 | ESCAPE | f1 | f2 | f3 | f5 | f6 | f8 | f9 | \ | → |
//!
//! The Master adds a keypad in columns 10–12 (`A`–`C`):
//!
//! | row | A | B | C |
//! | --- | --- | --- | --- |
//! | 1 | 6 | 7 | |
//! | 2 | 8 | 9 | |
//! | 3 | + | - | RETURN |
//! | 4 | / | DELETE | . |
//! | 5 | # | * | , |
//! | 6 | 0 | 1 | 3 |
//! | 7 | 4 | 5 | 2 |
//!
//! The Model B and B+ have no columns there: a keypad key types the
//! main keyboard's key with the same character where one exists without
//! SHIFT (digits, `-`, `/`, `.`, `,`, RETURN, DELETE), else nothing.
//!
//! BREAK is not in the matrix: it resets the CPU ([`BREAK`] as a panel
//! control).
//!
//! Source: Acorn, *BBC Microcomputer Advanced User Guide* (1983), the
//! keyboard (chapter 19, the system VIA's port A, the internal key
//! numbers); the Master's keypad: Acorn, *Master Reference Manual* part 1
//! (the INKEY numbers of the keypad keys).

use emu_core::{StateError, StateReader, StateWriter};

/// The panel control BREAK.
pub const BREAK: u16 = 1;

pub mod keys {
    pub const SHIFT: u16 = 0x00;
    pub const CTRL: u16 = 0x01;
    pub const RETURN: u16 = 0x49;
    pub const SPACE: u16 = 0x62;
    /// BREAK as a key (outside the matrix): pressing it resets the CPU
    /// like the panel control [`super::BREAK`].
    pub const BREAK: u16 = 0x100;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    /// Keys down per column (bit = row).
    pub cols: [u8; 16],
    /// The keyboard has the Master's keypad (columns 10–12).
    pub keypad: bool,
}

/// Keypad key → the main keyboard's key with that character (Model B,
/// B+).
const KEYPAD_FALLBACK: [(u16, u16); 15] = [
    (0x6a, 0x27), // 0
    (0x6b, 0x30), // 1
    (0x7c, 0x31), // 2
    (0x6c, 0x11), // 3
    (0x7a, 0x12), // 4
    (0x7b, 0x13), // 5
    (0x1a, 0x34), // 6
    (0x1b, 0x24), // 7
    (0x2a, 0x15), // 8
    (0x2b, 0x26), // 9
    (0x3b, 0x17), // -
    (0x4a, 0x68), // /
    (0x4c, 0x67), // .
    (0x5c, 0x66), // ,
    (0x3c, 0x49), // RETURN
];

impl Keyboard {
    pub fn key(&mut self, mut key: u16, down: bool) {
        if !self.keypad && (10..13).contains(&(key & 15)) {
            key = match KEYPAD_FALLBACK.iter().find(|&&(k, _)| k == key) {
                Some(&(_, main)) => main,
                None if key == 0x4b => 0x59,
                None => return,
            };
        }
        let (row, col) = (usize::from(key >> 4 & 7), usize::from(key & 15));
        // Columns 10–12: the Master's keypad.
        if col >= 13 {
            return;
        }
        if down {
            self.cols[col] |= 1 << row;
        } else {
            self.cols[col] &= !(1 << row);
        }
    }

    /// The key at PA0–PA6 (column, row) is down.
    pub fn down(&self, pa: u8) -> bool {
        let (col, row) = (usize::from(pa & 15), pa >> 4 & 7);
        self.cols[col] >> row & 1 != 0
    }

    /// A key of rows 1–7 is down: in `column`, or in any column.
    pub fn any(&self, column: Option<u8>) -> bool {
        match column {
            Some(c) => self.cols[usize::from(c & 15)] & 0xfe != 0,
            None => self.cols.iter().any(|c| c & 0xfe != 0),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.cols);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut c = [0; 16];
        r.raw_into(&mut c)?;
        self.cols = c;
        Ok(())
    }
}
