//! C64 keyboard matrix.
//!
//! 64 keys in an 8 × 8 matrix between CIA1 port A (columns, driven low by
//! the KERNAL scan) and port B (rows, read). A pressed key connects its
//! column and row line; RESTORE is not in the matrix but pulls /NMI.
//! Source: Commodore 64 Programmer's Reference Guide, keyboard matrix.
//!
//! [`MachineKey`] codes are `column * 8 + row`; [`RESTORE`] is 64.
//!
//! | PA \ PB | 0        | 1      | 2      | 3     | 4      | 5    | 6    | 7         |
//! | ------- | -------- | ------ | ------ | ----- | ------ | ---- | ---- | --------- |
//! | 0       | INST/DEL | RETURN | CRSR → | F7    | F1     | F3   | F5   | CRSR ↓    |
//! | 1       | 3        | W      | A      | 4     | Z      | S    | E    | L-SHIFT   |
//! | 2       | 5        | R      | D      | 6     | C      | F    | T    | X         |
//! | 3       | 7        | Y      | G      | 8     | B      | H    | U    | V         |
//! | 4       | 9        | I      | J      | 0     | M      | K    | O    | N         |
//! | 5       | +        | P      | L      | -     | .      | :    | @    | ,         |
//! | 6       | £        | *      | ;      | HOME  | R-SHIFT| =    | ↑    | /         |
//! | 7       | 1        | ←      | CTRL   | 2     | SPACE  | C=   | Q    | RUN/STOP  |

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

pub const RESTORE: MachineKey = MachineKey(64);

const fn key(col: u16, row: u16) -> MachineKey {
    MachineKey(col * 8 + row)
}

/// Named keys for tests and keymaps.
pub mod keys {
    use super::{key, MachineKey};

    pub const DEL: MachineKey = key(0, 0);
    pub const RETURN: MachineKey = key(0, 1);
    pub const CRSR_RIGHT: MachineKey = key(0, 2);
    pub const F7: MachineKey = key(0, 3);
    pub const F1: MachineKey = key(0, 4);
    pub const F3: MachineKey = key(0, 5);
    pub const F5: MachineKey = key(0, 6);
    pub const CRSR_DOWN: MachineKey = key(0, 7);
    pub const LSHIFT: MachineKey = key(1, 7);
    pub const RSHIFT: MachineKey = key(6, 4);
    pub const HOME: MachineKey = key(6, 3);
    pub const CTRL: MachineKey = key(7, 2);
    pub const SPACE: MachineKey = key(7, 4);
    pub const COMMODORE: MachineKey = key(7, 5);
    pub const RUN_STOP: MachineKey = key(7, 7);
}

/// Matrix positions of the characters that need no shift, for typing text
/// in tests and for symbolic keymaps.
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

/// Characters typed with SHIFT on the C64 layout.
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

/// The key (and whether SHIFT is needed) for a character, upper-case
/// letters only, as on the unshifted C64 character set.
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    // Carriage return and line feed both mean RETURN.
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

/// Cycles a typed key stays down, and the gap before the next one. The
/// KERNAL scans once per 1/60 s IRQ (~16 400 cycles); two scans per phase.
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

/// Pressed keys as a 64-bit set plus RESTORE.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    pub matrix: u64,
    pub restore: bool,
}

impl Keyboard {
    /// Press or release; returns false for codes outside the keyboard.
    pub fn set(&mut self, k: MachineKey, down: bool) -> bool {
        match k.0 {
            0..=63 => {
                let bit = 1u64 << k.0;
                if down {
                    self.matrix |= bit;
                } else {
                    self.matrix &= !bit;
                }
                true
            }
            64 => {
                self.restore = down;
                true
            }
            _ => false,
        }
    }

    pub fn pressed(&self, k: MachineKey) -> bool {
        match k.0 {
            0..=63 => self.matrix & (1u64 << k.0) != 0,
            64 => self.restore,
            _ => false,
        }
    }

    /// Resolve the matrix: given the levels driven onto port A and B (after
    /// joystick pull-downs), return the levels on both ports. A low line
    /// pulls every line it is connected to through a pressed key low; this
    /// repeats until stable, which reproduces ghosting with three or more
    /// keys.
    pub fn resolve(&self, mut a: u8, mut b: u8) -> (u8, u8) {
        if self.matrix == 0 {
            return (a, b);
        }
        loop {
            let (old_a, old_b) = (a, b);
            for col in 0..8 {
                let column = (self.matrix >> (col * 8)) as u8;
                if column == 0 {
                    continue;
                }
                if a & (1 << col) == 0 {
                    b &= !column;
                }
                if !b & column != 0 {
                    a &= !(1 << col);
                }
            }
            if (a, b) == (old_a, old_b) {
                return (a, b);
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
        // Column 0 driven low: row 1 reads low.
        assert_eq!(k.resolve(0xfe, 0xff), (0xfe, 0xfd));
        // Other column: nothing.
        assert_eq!(k.resolve(0xfd, 0xff), (0xfd, 0xff));
    }

    #[test]
    fn reverse_scan_works_too() {
        let mut k = Keyboard::default();
        k.set(key_for_char('A').unwrap().0, true); // column 1, row 2
        assert_eq!(k.resolve(0xff, 0xfb), (0xfd, 0xfb));
    }

    #[test]
    fn three_keys_ghost_a_fourth() {
        let mut k = Keyboard::default();
        k.set(key(0, 0), true);
        k.set(key(0, 1), true);
        k.set(key(1, 0), true);
        // Scanning column 1 also sees row 1 through (1,0)-(0,0)-(0,1).
        let (_, b) = k.resolve(0xfd, 0xff);
        assert_eq!(b, 0xfc);
    }

    #[test]
    fn characters_map_to_keys() {
        assert_eq!(key_for_char('r'), Some((key(2, 1), false)));
        assert_eq!(key_for_char('\n'), Some((keys::RETURN, false)));
        assert_eq!(key_for_char('"'), Some((key(7, 3), true)));
        assert_eq!(key_for_char('~'), None);
        assert!(CHARS.iter().all(|&(_, c, r)| c < 8 && r < 8));
    }

    #[test]
    fn restore_is_outside_the_matrix() {
        let mut k = Keyboard::default();
        assert!(k.set(RESTORE, true));
        assert_eq!(k.matrix, 0);
        assert!(k.pressed(RESTORE));
        assert!(!k.set(MachineKey(99), true));
    }
}
