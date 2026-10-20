//! VIC-20 keyboard matrix.
//!
//! 64 keys in an 8 × 8 matrix between VIA2 port B (columns, driven low by
//! the KERNAL scan) and port A (rows, read). RESTORE is not in the matrix:
//! it pulls VIA1 CA1, whose interrupt reaches the CPU's /NMI. The layout
//! is the C64's; the matrix differs. Source: VIC-20 Programmer's Reference
//! Guide (keyboard matrix; KERNAL key code table at `$EC5E`).
//!
//! [`MachineKey`] codes are `column * 8 + row` (the KERNAL's key codes);
//! [`RESTORE`] is 64.
//!
//! | PB \ PA | 0     | 1      | 2    | 3    | 4    | 5    | 6       | 7        |
//! | ------- | ----- | ------ | ---- | ---- | ---- | ---- | ------- | -------- |
//! | 0       | 1     | 3      | 5    | 7    | 9    | +    | £       | INST/DEL |
//! | 1       | ←     | W      | R    | Y    | I    | P    | *       | RETURN   |
//! | 2       | CTRL  | A      | D    | G    | J    | L    | ;       | CRSR →   |
//! | 3       | STOP  | L-SHIFT| X    | V    | N    | ,    | /       | CRSR ↓   |
//! | 4       | SPACE | Z      | C    | B    | M    | .    | R-SHIFT | F1       |
//! | 5       | C=    | S      | F    | H    | K    | :    | =       | F3       |
//! | 6       | Q     | E      | T    | U    | O    | @    | ↑       | F5       |
//! | 7       | 2     | 4      | 6    | 8    | 0    | -    | HOME    | F7       |

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

pub const RESTORE: MachineKey = MachineKey(64);

const fn key(col: u16, row: u16) -> MachineKey {
    MachineKey(col * 8 + row)
}

/// Named keys for tests and keymaps.
pub mod keys {
    use super::{key, MachineKey};

    pub const DEL: MachineKey = key(0, 7);
    pub const RETURN: MachineKey = key(1, 7);
    pub const CTRL: MachineKey = key(2, 0);
    pub const CRSR_RIGHT: MachineKey = key(2, 7);
    pub const RUN_STOP: MachineKey = key(3, 0);
    pub const LSHIFT: MachineKey = key(3, 1);
    pub const CRSR_DOWN: MachineKey = key(3, 7);
    pub const SPACE: MachineKey = key(4, 0);
    pub const RSHIFT: MachineKey = key(4, 6);
    pub const F1: MachineKey = key(4, 7);
    pub const COMMODORE: MachineKey = key(5, 0);
    pub const F3: MachineKey = key(5, 7);
    pub const F5: MachineKey = key(6, 7);
    pub const HOME: MachineKey = key(7, 6);
    pub const F7: MachineKey = key(7, 7);
}

/// Matrix positions of the characters that need no shift.
const CHARS: [(char, u16, u16); 49] = [
    ('1', 0, 0),
    ('3', 0, 1),
    ('5', 0, 2),
    ('7', 0, 3),
    ('9', 0, 4),
    ('+', 0, 5),
    ('\u{a3}', 0, 6),
    ('W', 1, 1),
    ('R', 1, 2),
    ('Y', 1, 3),
    ('I', 1, 4),
    ('P', 1, 5),
    ('*', 1, 6),
    ('\n', 1, 7),
    ('A', 2, 1),
    ('D', 2, 2),
    ('G', 2, 3),
    ('J', 2, 4),
    ('L', 2, 5),
    (';', 2, 6),
    ('X', 3, 2),
    ('V', 3, 3),
    ('N', 3, 4),
    (',', 3, 5),
    ('/', 3, 6),
    (' ', 4, 0),
    ('Z', 4, 1),
    ('C', 4, 2),
    ('B', 4, 3),
    ('M', 4, 4),
    ('.', 4, 5),
    ('S', 5, 1),
    ('F', 5, 2),
    ('H', 5, 3),
    ('K', 5, 4),
    (':', 5, 5),
    ('=', 5, 6),
    ('Q', 6, 0),
    ('E', 6, 1),
    ('T', 6, 2),
    ('U', 6, 3),
    ('O', 6, 4),
    ('@', 6, 5),
    ('2', 7, 0),
    ('4', 7, 1),
    ('6', 7, 2),
    ('8', 7, 3),
    ('0', 7, 4),
    ('-', 7, 5),
];

/// Characters typed with SHIFT (same layout as the C64).
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
/// letters only, as on the unshifted character set.
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

/// Cycles a typed key stays down, and the gap before the next one. The
/// KERNAL scans once per 1/60 s IRQ (~18 500 cycles); two scans per phase.
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

    /// Resolve the matrix: given the levels driven onto the column lines
    /// (VIA2 port B, after the joystick) and row lines (port A), return the
    /// levels on both. A low line pulls every line it is connected to
    /// through a pressed key low, until stable (ghosting with three keys).
    pub fn resolve(&self, mut cols: u8, mut rows: u8) -> (u8, u8) {
        if self.matrix == 0 {
            return (cols, rows);
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
                return (cols, rows);
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
        // Column 1 driven low: row 7 reads low.
        assert_eq!(k.resolve(0xfd, 0xff), (0xfd, 0x7f));
        assert_eq!(k.resolve(0xfe, 0xff), (0xfe, 0xff));
    }

    #[test]
    fn characters_map_to_keys() {
        assert_eq!(key_for_char('a'), Some((key(2, 1), false)));
        assert_eq!(key_for_char('\r'), Some((keys::RETURN, false)));
        assert_eq!(key_for_char('"'), Some((key(7, 0), true)));
        assert_eq!(key_for_char('~'), None);
        // Every matrix position appears at most once.
        let mut seen = [false; 64];
        for &(_, c, r) in &CHARS {
            assert!(c < 8 && r < 8);
            assert!(!std::mem::replace(&mut seen[usize::from(c * 8 + r)], true));
        }
    }

    #[test]
    fn restore_is_outside_the_matrix() {
        let mut k = Keyboard::default();
        assert!(k.set(RESTORE, true));
        assert_eq!(k.matrix, 0);
        assert!(k.restore);
        assert!(!k.set(MachineKey(65), true));
    }
}
