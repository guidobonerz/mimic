//! ZX Spectrum keyboard: an 8 × 5 matrix read through port `$FE`.
//! Address lines A8–A15 select a half-row (0 = selected), data bits 0–4
//! the keys (0 = pressed). [`MachineKey`] = row · 5 + column.
//!
//! | Row (address line) | D0 | D1 | D2 | D3 | D4 |
//! | --- | --- | --- | --- | --- | --- |
//! | 0 (A8) | CAPS SHIFT | Z | X | C | V |
//! | 1 (A9) | A | S | D | F | G |
//! | 2 (A10) | Q | W | E | R | T |
//! | 3 (A11) | 1 | 2 | 3 | 4 | 5 |
//! | 4 (A12) | 0 | 9 | 8 | 7 | 6 |
//! | 5 (A13) | P | O | I | U | Y |
//! | 6 (A14) | ENTER | L | K | J | H |
//! | 7 (A15) | SPACE | SYMBOL SHIFT | M | N | B |
//!
//! The Sinclair joysticks of the Interface 2 and the cursor joystick are
//! wired to keys ([`joystick_keys`]).
//!
//! Source: Sinclair ZX Spectrum Service Manual (keyboard matrix); ZX
//! Spectrum 48K manual, chapter 23 (IN 65278 …).

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

/// The key code of a matrix position.
pub const fn key(row: u16, col: u16) -> u16 {
    row * 5 + col
}

pub mod keys {
    use super::key;
    use emu_core::input::MachineKey;

    pub const CAPS_SHIFT: MachineKey = MachineKey(key(0, 0));
    pub const SYMBOL_SHIFT: MachineKey = MachineKey(key(7, 1));
    pub const ENTER: MachineKey = MachineKey(key(6, 0));
    pub const SPACE: MachineKey = MachineKey(key(7, 0));
}

const ROWS: [&str; 8] = [
    "\0ZXCV", "ASDFG", "QWERT", "12345", "09876", "POIUY", "\nLKJH", " \0MNB",
];

/// The key for a character and the shift it needs: letters (upper case
/// with CAPS SHIFT), digits, space, newline (ENTER) and the symbols on the
/// keys with SYMBOL SHIFT (`"` P, `,` N, `.` M, `;` O, `:` Z, `+` K, `-` J,
/// `=` L, `*` B, `/` V, `<` R, `>` T, `(` 8, `)` 9, `$` 4, `?` C, `!` 1,
/// `@` 2, `#` 3, `%` 5, `&` 6, `'` 7, `_` 0, `^` H).
pub fn key_for_char(c: char) -> Option<(MachineKey, Option<MachineKey>)> {
    let find = |ch: char| {
        ROWS.iter().enumerate().find_map(|(row, cols)| {
            cols.chars()
                .position(|x| x == ch)
                .map(|col| MachineKey(key(row as u16, col as u16)))
        })
    };
    if c == '\0' {
        return None;
    }
    if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' || c == '\n' {
        return find(c.to_ascii_uppercase()).map(|k| (k, None));
    }
    if c.is_ascii_uppercase() {
        return find(c).map(|k| (k, Some(keys::CAPS_SHIFT)));
    }
    let base = match c {
        '"' => 'P',
        ',' => 'N',
        '.' => 'M',
        ';' => 'O',
        ':' => 'Z',
        '+' => 'K',
        '-' => 'J',
        '=' => 'L',
        '*' => 'B',
        '/' => 'V',
        '<' => 'R',
        '>' => 'T',
        '(' => '8',
        ')' => '9',
        '$' => '4',
        '?' => 'C',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '%' => '5',
        '&' => '6',
        '\'' => '7',
        '_' => '0',
        '^' => 'H',
        _ => return None,
    };
    find(base).map(|k| (k, Some(keys::SYMBOL_SHIFT)))
}

/// The keys a joystick closes for (up, down, left, right, fire), by
/// interface: Sinclair 1 (Interface 2, keys 9 8 6 7 0), Sinclair 2
/// (keys 4 3 1 2 5), cursor (keys 7 6 5 8 0).
pub fn joystick_keys(interface: usize) -> [u16; 5] {
    match interface {
        // Sinclair 1: 6 left, 7 right, 8 down, 9 up, 0 fire.
        0 => [key(4, 1), key(4, 2), key(4, 4), key(4, 3), key(4, 0)],
        // Sinclair 2: 1 left, 2 right, 3 down, 4 up, 5 fire.
        1 => [key(3, 3), key(3, 2), key(3, 0), key(3, 1), key(3, 4)],
        // Cursor: 5 left, 6 down, 7 up, 8 right, 0 fire.
        _ => [key(4, 3), key(4, 4), key(3, 4), key(4, 2), key(4, 0)],
    }
}

/// Key events that type `text`, one key at a time, held long enough for
/// the ROM's keyboard scan (once a frame, with its repeat delay); unknown
/// characters are skipped. `hz` is the CPU clock. Returns the events and
/// the clock after the last release.
pub fn typed_events(text: &str, from: u64, hz: u64) -> (Vec<TimedInput>, u64) {
    let frame = hz / 50;
    let hold = frame * 4;
    let gap = frame * 4;
    let mut events = Vec::new();
    let mut t = from;
    let press = |key: MachineKey, down: bool, at: u64| TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: key,
            down,
        },
    };
    for c in text.chars() {
        let Some((k, shift)) = key_for_char(c) else {
            continue;
        };
        if let Some(s) = shift {
            events.push(press(s, true, t));
        }
        events.push(press(k, true, t + 1));
        events.push(press(k, false, t + hold));
        if let Some(s) = shift {
            events.push(press(s, false, t + hold + 1));
        }
        t += hold + gap;
    }
    (events, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_positions() {
        assert_eq!(key_for_char('a'), Some((MachineKey(key(1, 0)), None)));
        assert_eq!(
            key_for_char('A'),
            Some((MachineKey(key(1, 0)), Some(keys::CAPS_SHIFT)))
        );
        assert_eq!(key_for_char('6'), Some((MachineKey(key(4, 4)), None)));
        assert_eq!(
            key_for_char('"'),
            Some((MachineKey(key(5, 0)), Some(keys::SYMBOL_SHIFT)))
        );
        assert_eq!(key_for_char('\n'), Some((keys::ENTER, None)));
        assert_eq!(key_for_char(' '), Some((keys::SPACE, None)));
        assert_eq!(key_for_char('~'), None);
        let (events, end) = typed_events("aA", 0, 3_500_000);
        assert_eq!(events.len(), 2 + 4);
        assert!(end > 0);
    }

    #[test]
    fn joystick_wiring() {
        // Sinclair 1 fire is 0, cursor up is 7, Sinclair 2 left is 1.
        assert_eq!(joystick_keys(0)[4], key(4, 0));
        assert_eq!(joystick_keys(2)[0], key(4, 3));
        assert_eq!(joystick_keys(1)[2], key(3, 0));
    }
}
