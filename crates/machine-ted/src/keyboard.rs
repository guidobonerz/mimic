//! C16/Plus/4 keyboard matrix and joysticks.
//!
//! The KERNAL writes a column mask (0 = selected) to the keyboard latch at
//! `$FD30`, then reads the rows through TED `$FF08`. The joysticks share
//! those row lines: writing `$FF08` with bit 2 clear connects joystick 1,
//! bit 1 clear joystick 2 (bits 0–3 up, down, left, right; fire on bit 6
//! for joystick 1, bit 7 for joystick 2). Source: Plus/4 Programmer's
//! Reference; KERNAL key code table (`$E026` in 318004-05).
//!
//! [`MachineKey`] codes are `column * 8 + row`.
//!
//! | Col \ Row | 0     | 1    | 2    | 3      | 4     | 5   | 6   | 7     |
//! | --------- | ----- | ---- | ---- | ------ | ----- | --- | --- | ----- |
//! | 0         | INST/DEL | RETURN | £ | HELP | F1   | F2  | F3  | @     |
//! | 1         | 3     | W    | A    | 4      | Z     | S   | E   | SHIFT |
//! | 2         | 5     | R    | D    | 6      | C     | F   | T   | X     |
//! | 3         | 7     | Y    | G    | 8      | B     | H   | U   | V     |
//! | 4         | 9     | I    | J    | 0      | M     | K   | O   | N     |
//! | 5         | CRSR ↓ | P   | L    | CRSR ↑ | .     | :   | -   | ,     |
//! | 6         | CRSR ← | *   | ;    | CRSR → | ESC   | =   | +   | /     |
//! | 7         | 1     | HOME | CTRL | 2      | SPACE | C=  | Q   | RUN/STOP |

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

const fn key(col: u16, row: u16) -> MachineKey {
    MachineKey(col * 8 + row)
}

/// Named keys for tests and keymaps.
pub mod keys {
    use super::{key, MachineKey};

    pub const DEL: MachineKey = key(0, 0);
    pub const RETURN: MachineKey = key(0, 1);
    pub const HELP: MachineKey = key(0, 3);
    pub const F1: MachineKey = key(0, 4);
    pub const F2: MachineKey = key(0, 5);
    pub const F3: MachineKey = key(0, 6);
    pub const SHIFT: MachineKey = key(1, 7);
    pub const CRSR_DOWN: MachineKey = key(5, 0);
    pub const CRSR_UP: MachineKey = key(5, 3);
    pub const CRSR_LEFT: MachineKey = key(6, 0);
    pub const CRSR_RIGHT: MachineKey = key(6, 3);
    pub const ESC: MachineKey = key(6, 4);
    pub const HOME: MachineKey = key(7, 1);
    pub const CTRL: MachineKey = key(7, 2);
    pub const SPACE: MachineKey = key(7, 4);
    pub const COMMODORE: MachineKey = key(7, 5);
    pub const RUN_STOP: MachineKey = key(7, 7);
}

/// Matrix positions of the characters that need no shift.
const CHARS: [(char, u16, u16); 49] = [
    ('\n', 0, 1),
    ('\u{a3}', 0, 2),
    ('@', 0, 7),
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
    ('P', 5, 1),
    ('L', 5, 2),
    ('.', 5, 4),
    (':', 5, 5),
    ('-', 5, 6),
    (',', 5, 7),
    ('*', 6, 1),
    (';', 6, 2),
    ('=', 6, 5),
    ('+', 6, 6),
    ('/', 6, 7),
    ('1', 7, 0),
    ('2', 7, 3),
    (' ', 7, 4),
    ('Q', 7, 6),
];

/// Characters typed with SHIFT (same layout as the C64).
const SHIFTED: [(char, char); 12] = [
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
];

/// The key (and whether SHIFT is needed) for a character, upper-case
/// letters only.
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

/// Cycles (TED double clocks) a typed key stays down, and the gap before
/// the next one: two KERNAL scans (one per field) each.
const TYPE_HOLD: u64 = 80_000;
const TYPE_GAP: u64 = 80_000;

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
            events.push(key_event(t, keys::SHIFT, true));
        }
        events.push(key_event(t, k, true));
        events.push(key_event(t + TYPE_HOLD, k, false));
        if shift {
            events.push(key_event(t + TYPE_HOLD, keys::SHIFT, false));
        }
        t += TYPE_HOLD + TYPE_GAP;
    }
    (events, t)
}

/// Pressed keys as a 64-bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    pub matrix: u64,
}

impl Keyboard {
    /// Press or release; returns false for codes outside the keyboard.
    pub fn set(&mut self, k: MachineKey, down: bool) -> bool {
        if k.0 > 63 {
            return false;
        }
        let bit = 1u64 << k.0;
        if down {
            self.matrix |= bit;
        } else {
            self.matrix &= !bit;
        }
        true
    }

    /// Row levels when the columns in `select` are driven low (0 =
    /// selected) and the rows are pulled to `rows` from outside. A low line
    /// pulls every line it reaches through pressed keys low (ghosting).
    pub fn rows(&self, select: u8, rows: u8) -> u8 {
        let (mut cols, mut rows) = (select, rows);
        if self.matrix == 0 {
            return rows;
        }
        loop {
            let (old_c, old_r) = (cols, rows);
            for col in 0..8 {
                let column = (self.matrix >> (col * 8)) as u8;
                if column == 0 {
                    continue;
                }
                if cols & (1 << col) == 0 {
                    rows &= !column;
                }
                if !rows & column != 0 {
                    cols &= !(1 << col);
                }
            }
            if (cols, rows) == (old_c, old_r) {
                return rows;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_a_column_finds_the_row() {
        let mut k = Keyboard::default();
        k.set(keys::RETURN, true);
        assert_eq!(k.rows(0xfe, 0xff), 0xfd);
        assert_eq!(k.rows(0xfd, 0xff), 0xff);
        assert_eq!(k.rows(0xfe, 0xfb), 0xf9, "joystick lines add up");
    }

    #[test]
    fn characters_map_to_keys() {
        assert_eq!(key_for_char('q'), Some((key(7, 6), false)));
        assert_eq!(key_for_char('*'), Some((key(6, 1), false)));
        assert_eq!(key_for_char('"'), Some((key(7, 3), true)));
        assert_eq!(key_for_char('~'), None);
        let mut seen = [false; 64];
        for &(_, c, r) in &CHARS {
            assert!(!std::mem::replace(&mut seen[usize::from(c * 8 + r)], true));
        }
    }
}
