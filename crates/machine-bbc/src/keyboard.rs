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
//! BREAK is not in the matrix: it resets the CPU ([`BREAK`] as a panel
//! control).
//!
//! Source: Acorn, *BBC Microcomputer Advanced User Guide* (1983), the
//! keyboard (chapter 19, the system VIA's port A, the internal key
//! numbers).

use emu_core::{StateError, StateReader, StateWriter};

/// The panel control BREAK.
pub const BREAK: u16 = 1;

pub mod keys {
    pub const SHIFT: u16 = 0x00;
    pub const CTRL: u16 = 0x01;
    pub const RETURN: u16 = 0x49;
    pub const SPACE: u16 = 0x62;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    /// Keys down per column (bit = row).
    pub cols: [u8; 16],
}

impl Keyboard {
    pub fn key(&mut self, key: u16, down: bool) {
        let (row, col) = (usize::from(key >> 4 & 7), usize::from(key & 15));
        if col >= 10 {
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
