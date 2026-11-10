//! The TI-99/4A keyboard: an 8 × 8 matrix read through the TMS9901. The
//! column is chosen by the pins P2–P4 (CRU bits 18–20), the eight rows
//! come back on INT3–INT10 (CRU bits 3–10), low while a key is down.
//! Columns 6 and 7 are the two joysticks (fire, left, right, down, up);
//! ALPHA LOCK is read on row 4 (INT7) while P5 (CRU bit 21) is low.
//!
//! Machine key = column × 8 + row (rows in CRU bit order 3–10):
//!
//! | col | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 0 | = | space | enter | – | FCTN | SHIFT | CTRL | – |
//! | 1 | . | L | O | 9 | 2 | S | W | X |
//! | 2 | , | K | I | 8 | 3 | D | E | C |
//! | 3 | M | J | U | 7 | 4 | F | R | V |
//! | 4 | N | H | Y | 6 | 5 | G | T | B |
//! | 5 | / | ; | P | 0 | 1 | A | Q | Z |
//!
//! [`ALPHA_LOCK`] (48) toggles the lock (a latching key).
//!
//! Source: Thierry Nouspikel, "The TI-99/4A Tech Pages", keyboard (the
//! matrix and the CRU lines); Texas Instruments, *TI-99/4A Console and
//! Peripheral Expansion System Technical Data* (1983).

use emu_core::{StateError, StateReader, StateWriter};

/// The ALPHA LOCK key.
pub const ALPHA_LOCK: u16 = 48;

/// Joystick row bits in columns 6 and 7.
pub mod joy {
    pub const FIRE: u8 = 1 << 0;
    pub const LEFT: u8 = 1 << 1;
    pub const RIGHT: u8 = 1 << 2;
    pub const DOWN: u8 = 1 << 3;
    pub const UP: u8 = 1 << 4;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    /// Keys down per column (bit = row).
    pub cols: [u8; 8],
    pub alpha_lock: bool,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            cols: [0; 8],
            // Most programs want capitals: the console is usually used
            // with the lock down.
            alpha_lock: true,
        }
    }
}

impl Keyboard {
    pub fn key(&mut self, key: u16, down: bool) {
        if key == ALPHA_LOCK {
            if down {
                self.alpha_lock = !self.alpha_lock;
            }
            return;
        }
        if key >= 48 {
            return;
        }
        let (col, row) = (usize::from(key / 8), key % 8);
        if down {
            self.cols[col] |= 1 << row;
        } else {
            self.cols[col] &= !(1 << row);
        }
    }

    /// Rows pulled low for `column` (with the joysticks' lines for 6 and
    /// 7, `joys`) and the alpha lock when `alpha_select`.
    pub fn rows(&self, column: u8, joys: [u8; 2], alpha_select: bool) -> u8 {
        let c = usize::from(column & 7);
        let mut rows = self.cols[c];
        if c >= 6 {
            rows |= joys[c - 6];
        }
        if alpha_select && self.alpha_lock {
            rows |= 1 << 4;
        }
        rows
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.cols);
        w.bool(self.alpha_lock);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut cols = [0; 8];
        r.raw_into(&mut cols)?;
        self.cols = cols;
        self.alpha_lock = r.bool()?;
        Ok(())
    }
}
