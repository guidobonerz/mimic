//! The MEGA65 keyboard: the C65 matrix of nine columns and eight rows,
//! the typing event queue and the matrix registers.
//!
//! **Matrix** (machine key = column × 8 + row; columns 0–7 as on the C64,
//! read through CIA 1: port A drives the columns, port B reads the rows;
//! column 8 through bit 1 of `$D607`):
//!
//! | row | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 0 | INST/DEL | 3 | 5 | 7 | 9 | + | £ | 1 | NO SCROLL |
//! | 1 | RETURN | W | R | Y | I | P | * | ← | TAB |
//! | 2 | CRSR → | A | D | G | J | L | ; | CTRL | ALT |
//! | 3 | F7 | 4 | 6 | 8 | 0 | - | CLR/HOME | 2 | HELP |
//! | 4 | F1 | Z | C | B | M | . | right SHIFT | SPACE | F9 |
//! | 5 | F3 | S | F | H | K | : | = | MEGA | F11 |
//! | 6 | F5 | E | T | U | O | @ | ↑ | Q | F13 |
//! | 7 | CRSR ↓ | left SHIFT | X | V | N | , | / | RUN/STOP | ESC |
//!
//! Outside the matrix: [`RESTORE`] (NMI), [`CAPS_LOCK`] (a locking key; the
//! C65 reads it on bit 6 of the CPU port, low while locked), and the
//! separate cursor keys [`CRSR_LEFT`] and [`CRSR_UP`], which press right
//! SHIFT with CRSR → or CRSR ↓ in the matrix (for C64 software) and set
//! bits 0 and 1 of `$D60F`.
//!
//! **Registers** (`$D600` page, C65/MEGA65 personalities): `$D610` the
//! queue's first event as ASCII (0: empty; a write takes it off), `$D619`
//! the same as PETSCII, `$D60A` its modifiers (bits 0–6) and bit 7 queue
//! not empty, `$D611` the modifiers now (bit 0 left SHIFT, 1 right SHIFT,
//! 2 CTRL, 3 MEGA, 4 ALT, 5 NO SCROLL, 6 CAPS LOCK), `$D614` a column
//! (0–8) whose rows `$D613` reads (0 = pressed), `$D615`–`$D617` virtual
//! keys (a scan code is held until the next write; `$7F` none).
//!
//! **Typing events.** A key going down with an ASCII value (the tables of
//! [`crate::keytable`], picked by MEGA, ALT, CTRL, SHIFT in that order;
//! CAPS LOCK capitalises letters) joins the queue (at most 16 events).
//!
//! Sources: the MEGA65 user guide, `appendix-keyboard.tex` (the C65 matrix,
//! the typing event queue, the matrix registers, synthetic key events, the
//! ASCII tables); the `@IO` register descriptions of the MEGA65 core
//! (`c65uart.vhdl`: `$D607`, `$D608`, `$D60F`, `$D611`–`$D615`).

use std::collections::VecDeque;

use emu_core::{StateError, StateReader, StateWriter};

use crate::keytable;

/// Keys outside the matrix.
pub const RESTORE: u16 = 72;
pub const CAPS_LOCK: u16 = 73;
pub const CRSR_LEFT: u16 = 74;
pub const CRSR_UP: u16 = 75;

/// Matrix positions.
pub mod keys {
    pub const fn key(col: u16, row: u16) -> u16 {
        col * 8 + row
    }
    pub const DEL: u16 = key(0, 0);
    pub const RETURN: u16 = key(0, 1);
    pub const CRSR_RIGHT: u16 = key(0, 2);
    pub const CRSR_DOWN: u16 = key(0, 7);
    pub const LSHIFT: u16 = key(1, 7);
    pub const RSHIFT: u16 = key(6, 4);
    pub const CTRL: u16 = key(7, 2);
    pub const SPACE: u16 = key(7, 4);
    pub const MEGA: u16 = key(7, 5);
    pub const RUN_STOP: u16 = key(7, 7);
    pub const NO_SCROLL: u16 = key(8, 0);
    pub const ALT: u16 = key(8, 2);
}

const QUEUE_LEN: usize = 16;

/// A typing event: ASCII value and modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typed {
    pub ascii: u8,
    pub mods: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    /// Keys down per column (bit = row).
    pub cols: [u8; 9],
    pub restore: bool,
    pub caps: bool,
    pub left: bool,
    pub up: bool,
    /// `$D615`–`$D617`.
    pub virt: [u8; 3],
    /// `$D614`.
    pub select: u8,
    queue: VecDeque<Typed>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            cols: [0; 9],
            restore: false,
            caps: false,
            left: false,
            up: false,
            // $7F: no virtual key.
            virt: [0x7f; 3],
            select: 0,
            queue: VecDeque::new(),
        }
    }
}

impl Keyboard {
    fn set_bit(&mut self, k: u16, down: bool) {
        let (col, row) = (usize::from(k / 8), k % 8);
        if col < 9 {
            if down {
                self.cols[col] |= 1 << row;
            } else {
                self.cols[col] &= !(1 << row);
            }
        }
    }

    fn pressed(&self, k: u16) -> bool {
        let (col, row) = (usize::from(k / 8), k % 8);
        col < 9 && self.cols[col] >> row & 1 != 0 || self.virt.iter().any(|&v| u16::from(v) == k)
    }

    /// The modifier bits (`$D611`).
    pub fn mods(&self) -> u8 {
        [
            keys::LSHIFT,
            keys::RSHIFT,
            keys::CTRL,
            keys::MEGA,
            keys::ALT,
            keys::NO_SCROLL,
        ]
        .iter()
        .enumerate()
        .fold(u8::from(self.caps) << 6, |m, (i, &k)| {
            m | u8::from(self.pressed(k)) << i
        })
    }

    /// A host key; returns false for codes outside the keyboard.
    pub fn key(&mut self, k: u16, down: bool) -> bool {
        match k {
            0..=71 => {
                let was = self.pressed(k);
                self.set_bit(k, down);
                if down && !was {
                    self.typed(k);
                }
            }
            RESTORE => self.restore = down,
            CAPS_LOCK => {
                if down {
                    self.caps = !self.caps;
                }
            }
            CRSR_LEFT | CRSR_UP => {
                let target = if k == CRSR_LEFT {
                    self.left = down;
                    keys::CRSR_RIGHT
                } else {
                    self.up = down;
                    keys::CRSR_DOWN
                };
                self.set_bit(keys::RSHIFT, down || self.left || self.up);
                self.set_bit(target, down);
                if down {
                    self.typed(target);
                }
            }
            _ => return false,
        }
        true
    }

    /// Queue the typing event of key `k` with the modifiers now.
    fn typed(&mut self, k: u16) {
        let i = usize::from(k);
        let m = self.mods();
        let table = if m & 0x08 != 0 {
            &keytable::CBM
        } else if m & 0x10 != 0 {
            &keytable::ALT
        } else if m & 0x04 != 0 {
            &keytable::CONTROL
        } else if m & 0x03 != 0 {
            &keytable::SHIFT
        } else {
            &keytable::NORMAL
        };
        let mut ascii = table[i];
        if self.caps && m & 0x03 == 0 && ascii.is_ascii_lowercase() {
            ascii = ascii.to_ascii_uppercase();
        }
        if ascii != 0 && self.queue.len() < QUEUE_LEN {
            self.queue.push_back(Typed { ascii, mods: m });
        }
    }

    /// The rows of column `col` (0–8), bit clear = pressed.
    pub fn column(&self, col: u8) -> u8 {
        let col = usize::from(col);
        let mut rows = self.cols.get(col).copied().unwrap_or(0);
        for &v in &self.virt {
            if usize::from(v / 8) == col && v < 72 {
                rows |= 1 << (v % 8);
            }
        }
        !rows
    }

    /// CIA 1: the levels on port A (columns) and B (rows) given what drives
    /// them, with column 8 selected by `col8` (driven low). A low line
    /// pulls every line it meets through a pressed key low, until stable.
    pub fn resolve(&self, mut a: u8, mut b: u8, col8: bool) -> (u8, u8) {
        let cols: [u8; 9] = std::array::from_fn(|c| !self.column(c as u8));
        if cols.iter().all(|&c| c == 0) {
            return (a, b);
        }
        if col8 {
            b &= !cols[8];
        }
        loop {
            let (old_a, old_b) = (a, b);
            for (col, &rows) in cols.iter().take(8).enumerate() {
                if rows == 0 {
                    continue;
                }
                if a & (1 << col) == 0 {
                    b &= !rows;
                }
                if !b & rows != 0 {
                    a &= !(1 << col);
                }
            }
            if (a, b) == (old_a, old_b) {
                return (a, b);
            }
        }
    }

    /// The queue's first event.
    pub fn first(&self) -> Option<Typed> {
        self.queue.front().copied()
    }

    pub fn dequeue(&mut self) {
        self.queue.pop_front();
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.raw(&self.cols);
        w.bool(self.restore);
        w.bool(self.caps);
        w.bool(self.left);
        w.bool(self.up);
        w.raw(&self.virt);
        w.u8(self.select);
        w.u8(self.queue.len() as u8);
        for t in &self.queue {
            w.u8(t.ascii);
            w.u8(t.mods);
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut k = Keyboard::default();
        r.raw_into(&mut k.cols)?;
        k.restore = r.bool()?;
        k.caps = r.bool()?;
        k.left = r.bool()?;
        k.up = r.bool()?;
        r.raw_into(&mut k.virt)?;
        k.select = r.u8()?;
        let n = usize::from(r.u8()?);
        if n > QUEUE_LEN {
            return Err(StateError::Invalid("MEGA65 typing queue"));
        }
        for _ in 0..n {
            k.queue.push_back(Typed {
                ascii: r.u8()?,
                mods: r.u8()?,
            });
        }
        *self = k;
        Ok(())
    }
}

/// PETSCII for an ASCII typing event (`$D619`): letters swap case, the
/// function keys get the C64's codes, £ ↑ ← theirs; others as they are,
/// `$FF` where PETSCII has none.
pub fn petscii(ascii: u8) -> u8 {
    match ascii {
        b'a'..=b'z' => ascii - 0x20,
        b'A'..=b'Z' => ascii + 0x80,
        0xf1 => 0x85,
        0xf3 => 0x86,
        0xf5 => 0x87,
        0xf7 => 0x88,
        0xf2 => 0x89,
        0xf4 => 0x8a,
        0xf6 => 0x8b,
        0xf8 => 0x8c,
        0xa3 => 0x5c,
        0xaf => 0x5e,
        0x5f => 0x5f,
        b'{' | b'}' | b'|' | b'~' | b'`' | 0x5c => 0xff,
        0x80..=0xff if !matches!(ascii, 0x91 | 0x9d | 0x93 | 0x94) => 0xff,
        _ => ascii,
    }
}
