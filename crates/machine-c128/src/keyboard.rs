//! C128 keyboard: the C64's 8 × 8 matrix (CIA1 port A columns, port B
//! rows) plus three columns K0–K2 driven by VIC-IIe `$D02F` for the keypad,
//! cursor and function keys. RESTORE pulls /NMI, 40/80 DISPLAY is read
//! through MMU `$D505` bit 7 and CAPS LOCK through 8502 port bit 6; none of
//! them is in the matrix. Source: Commodore 128 Programmer's Reference
//! Guide, keyboard matrix.
//!
//! [`MachineKey`] codes: `column * 8 + row` for columns 0–10 (0–87; the
//! C64 part is identical to `machine-c64`'s), [`RESTORE`] 88,
//! [`DISPLAY_4080`] 89, [`CAPS_LOCK`] 90. The extra columns:
//!
//! | Col \ Row | 0    | 1    | 2    | 3        | 4      | 5      | 6       | 7         |
//! | --------- | ---- | ---- | ---- | -------- | ------ | ------ | ------- | --------- |
//! | 8 (K0)    | HELP | KP 8 | KP 5 | TAB      | KP 2   | KP 4   | KP 7    | KP 1      |
//! | 9 (K1)    | ESC  | KP + | KP - | LINE FEED | ENTER | KP 6   | KP 9    | KP 3      |
//! | 10 (K2)   | ALT  | KP 0 | KP . | ↑        | ↓      | ←      | →       | NO SCROLL |

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

pub const RESTORE: MachineKey = MachineKey(88);
pub const DISPLAY_4080: MachineKey = MachineKey(89);
pub const CAPS_LOCK: MachineKey = MachineKey(90);

const fn key(col: u16, row: u16) -> MachineKey {
    MachineKey(col * 8 + row)
}

/// Named keys for tests and keymaps.
pub mod keys {
    use super::{key, MachineKey};

    pub const DEL: MachineKey = key(0, 0);
    pub const RETURN: MachineKey = key(0, 1);
    pub const LSHIFT: MachineKey = key(1, 7);
    pub const RSHIFT: MachineKey = key(6, 4);
    pub const CTRL: MachineKey = key(7, 2);
    pub const SPACE: MachineKey = key(7, 4);
    pub const COMMODORE: MachineKey = key(7, 5);
    pub const RUN_STOP: MachineKey = key(7, 7);
    pub const HELP: MachineKey = key(8, 0);
    pub const TAB: MachineKey = key(8, 3);
    pub const ESC: MachineKey = key(9, 0);
    pub const ENTER: MachineKey = key(9, 4);
    pub const ALT: MachineKey = key(10, 0);
    pub const UP: MachineKey = key(10, 3);
    pub const DOWN: MachineKey = key(10, 4);
    pub const LEFT: MachineKey = key(10, 5);
    pub const RIGHT: MachineKey = key(10, 6);
    pub const NO_SCROLL: MachineKey = key(10, 7);
}

/// Matrix positions of the characters that need no shift (C64 part).
const CHARS: [(char, u16, u16); 49] = [
    ('3', 1, 0),
    ('W', 1, 1),
    ('A', 1, 2),
    ('4', 1, 3),
    ('Z', 1, 4),
    ('S', 1, 5),
    ('E', 1, 6),
    ('5', 2, 0),
    ('R', 2, 1),
    ('D', 2, 2),
    ('6', 2, 3),
    ('C', 2, 4),
    ('F', 2, 5),
    ('T', 2, 6),
    ('X', 2, 7),
    ('7', 3, 0),
    ('Y', 3, 1),
    ('G', 3, 2),
    ('8', 3, 3),
    ('B', 3, 4),
    ('H', 3, 5),
    ('U', 3, 6),
    ('V', 3, 7),
    ('9', 4, 0),
    ('I', 4, 1),
    ('J', 4, 2),
    ('0', 4, 3),
    ('M', 4, 4),
    ('K', 4, 5),
    ('O', 4, 6),
    ('N', 4, 7),
    ('+', 5, 0),
    ('P', 5, 1),
    ('L', 5, 2),
    ('-', 5, 3),
    ('.', 5, 4),
    (':', 5, 5),
    ('@', 5, 6),
    (',', 5, 7),
    ('*', 6, 1),
    (';', 6, 2),
    ('=', 6, 5),
    ('/', 6, 7),
    ('1', 7, 0),
    ('2', 7, 3),
    (' ', 7, 4),
    ('Q', 7, 6),
    ('\n', 0, 1),
    ('\u{a3}', 6, 0),
];

/// Characters typed with SHIFT (C64 layout).
const SHIFTED: [(char, char); 13] = [
    ('!', '1'),
    ('"', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('&', '6'),
    ('\'', '7'),
    ('(', '8'),
    (')', '9'),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('[', ':'),
];

/// The key (and whether SHIFT is needed) for a character.
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    let c = if c == '\r' {
        '\n'
    } else {
        c.to_ascii_uppercase()
    };
    let base = |c: char| {
        CHARS
            .iter()
            .find(|&&(k, _, _)| k == c)
            .map(|&(_, col, row)| key(col, row))
    };
    if let Some(k) = base(c) {
        return Some((k, false));
    }
    SHIFTED
        .iter()
        .find(|&&(s, _)| s == c)
        .and_then(|&(_, b)| base(b))
        .map(|k| (k, true))
}

/// Cycles a typed key stays down, and the gap before the next one: two
/// scans of the 60 Hz keyboard interrupt each.
const TYPE_HOLD: u64 = 40_000;
const TYPE_GAP: u64 = 40_000;

/// Key events that type `text` from cycle `start` (characters without a key
/// are skipped). Returns the events and the cycle after the last release.
pub fn typed_events(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    let key_event = |at: u64, k: MachineKey, down: bool| TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: k,
            down,
        },
    };
    let mut events = Vec::new();
    let mut t = start;
    for c in text.chars() {
        let Some((k, shift)) = key_for_char(c) else {
            continue;
        };
        if shift {
            events.push(key_event(t, keys::LSHIFT, true));
        }
        events.push(key_event(t, k, true));
        events.push(key_event(t + TYPE_HOLD, k, false));
        if shift {
            events.push(key_event(t + TYPE_HOLD, keys::LSHIFT, false));
        }
        t += TYPE_HOLD + TYPE_GAP;
    }
    (events, t)
}

/// Pressed keys: row bits per column, plus the keys outside the matrix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    pub columns: [u8; 11],
    pub restore: bool,
    /// 40/80 DISPLAY is a locking key: true = down (80 columns).
    pub display_80: bool,
    /// CAPS LOCK (ASCII/DIN) is a locking key: true = down.
    pub caps_lock: bool,
}

impl Keyboard {
    /// Press or release; returns false for codes outside the keyboard. The
    /// locking keys toggle on each press.
    pub fn set(&mut self, k: MachineKey, down: bool) -> bool {
        match k.0 {
            0..=87 => {
                let (col, row) = (usize::from(k.0 / 8), k.0 % 8);
                if down {
                    self.columns[col] |= 1 << row;
                } else {
                    self.columns[col] &= !(1 << row);
                }
            }
            88 => self.restore = down,
            89 if down => self.display_80 = !self.display_80,
            90 if down => self.caps_lock = !self.caps_lock,
            89 | 90 => {}
            _ => return false,
        }
        true
    }

    /// Resolve the matrix: column lines (bits 0–7 CIA1 port A, 8–10 K0–K2)
    /// and row lines (CIA1 port B) as driven, after the joysticks. A low
    /// line pulls every line it reaches through pressed keys low, until
    /// stable (ghosting with three keys).
    pub fn resolve(&self, mut cols: u16, mut rows: u8) -> (u16, u8) {
        if self.columns.iter().all(|&c| c == 0) {
            return (cols, rows);
        }
        loop {
            let (old_c, old_r) = (cols, rows);
            for (col, &keys) in self.columns.iter().enumerate() {
                if keys == 0 {
                    continue;
                }
                if cols & (1 << col) == 0 {
                    rows &= !keys;
                }
                if !rows & keys != 0 {
                    cols &= !(1 << col);
                }
            }
            if (cols, rows) == (old_c, old_r) {
                return (cols, rows);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_and_extra_columns() {
        let mut k = Keyboard::default();
        k.set(keys::RETURN, true);
        k.set(keys::HELP, true);
        assert_eq!(k.resolve(0x7fe, 0xff), (0x7fe, 0xfd), "column 0");
        assert_eq!(k.resolve(0x6ff, 0xff), (0x6ff, 0xfe), "K0");
        assert_eq!(k.resolve(0x7ff, 0xff), (0x7ff, 0xff));
    }

    #[test]
    fn locking_keys_toggle() {
        let mut k = Keyboard::default();
        assert!(k.set(DISPLAY_4080, true));
        assert!(k.set(DISPLAY_4080, false));
        assert!(k.display_80);
        k.set(DISPLAY_4080, true);
        assert!(!k.display_80);
        k.set(CAPS_LOCK, true);
        assert!(k.caps_lock);
        assert!(k.set(RESTORE, true) && k.restore);
        assert!(!k.set(MachineKey(91), true));
    }

    #[test]
    fn characters_map_to_the_c64_part() {
        assert_eq!(key_for_char('r'), Some((key(2, 1), false)));
        assert_eq!(key_for_char('"'), Some((key(7, 3), true)));
    }
}
