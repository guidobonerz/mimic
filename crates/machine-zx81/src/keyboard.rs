//! ZX81 keyboard: an 8 × 5 matrix read through port `$FE`. Address lines
//! A8–A15 select a row (0 = selected), data bits 0–4 the keys (0 =
//! pressed). [`MachineKey`] = row · 5 + column.
//!
//! | Row (address line) | D0 | D1 | D2 | D3 | D4 |
//! | --- | --- | --- | --- | --- | --- |
//! | 0 (A8) | SHIFT | Z | X | C | V |
//! | 1 (A9) | A | S | D | F | G |
//! | 2 (A10) | Q | W | E | R | T |
//! | 3 (A11) | 1 | 2 | 3 | 4 | 5 |
//! | 4 (A12) | 0 | 9 | 8 | 7 | 6 |
//! | 5 (A13) | P | O | I | U | Y |
//! | 6 (A14) | NEW LINE | L | K | J | H |
//! | 7 (A15) | SPACE | . | M | N | B |
//!
//! Source: Sinclair ZX81 Service Manual (keyboard matrix).

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

/// The key code of a matrix position.
pub const fn key(row: u16, col: u16) -> u16 {
    row * 5 + col
}

const fn k(row: u16, col: u16) -> u16 {
    key(row, col)
}

pub mod keys {
    use emu_core::input::MachineKey;

    pub const SHIFT: MachineKey = MachineKey(super::k(0, 0));
    pub const NEW_LINE: MachineKey = MachineKey(super::k(6, 0));
    pub const SPACE: MachineKey = MachineKey(super::k(7, 0));
    pub const P: MachineKey = MachineKey(super::k(5, 0));
}

/// The key for a character: letters, digits, space, newline, `.` and the
/// shifted symbols the typing helper needs (`"` is SHIFT+P, `,` SHIFT+.,
/// `-` SHIFT+J, `+` SHIFT+K, `*` SHIFT+B, `/` SHIFT+V, `=` SHIFT+L,
/// `(`, `)` SHIFT+I/O, `<`, `>` SHIFT+N/M, `;` SHIFT+X, `:` SHIFT+Z,
/// `?` SHIFT+C, `$` SHIFT+U, `£` SHIFT+... none).
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    const ROWS: [&str; 8] = [
        " ZXCV", "ASDFG", "QWERT", "12345", "09876", "POIUY", "\nLKJH", " .MNB",
    ];
    let plain = match c {
        ' ' => Some(keys::SPACE),
        '\n' => Some(keys::NEW_LINE),
        _ => {
            let u = c.to_ascii_uppercase();
            ROWS.iter().enumerate().find_map(|(row, cols)| {
                cols.chars()
                    .position(|x| x == u && x != ' ' && x != '\n')
                    .map(|col| MachineKey(k(row as u16, col as u16)))
            })
        }
    };
    if let Some(key) = plain {
        return Some((key, false));
    }
    let shifted = match c {
        '"' => 'P',
        ',' => '.',
        '-' => 'J',
        '+' => 'K',
        '*' => 'B',
        '/' => 'V',
        '=' => 'L',
        '(' => 'I',
        ')' => 'O',
        '<' => 'N',
        '>' => 'M',
        ';' => 'X',
        ':' => 'Z',
        '?' => 'C',
        '$' => 'U',
        _ => return None,
    };
    key_for_char(shifted).map(|(key, _)| (key, true))
}

/// Key events that type `text`, one key every `hold` CPU clocks (the ROM
/// debounces over a few frames); unmappable characters are skipped.
/// Returns the events and the clock after the last release.
pub fn typed_events(text: &str, from: u64) -> (Vec<TimedInput>, u64) {
    // The ROM accepts a key held for two frames and, after it, waits for
    // the keyboard to be idle for its debounce period.
    const HOLD: u64 = 3_250_000 / 50 * 5; // five fields down
    const GAP: u64 = 3_250_000 / 50 * 12; // twelve fields up
    let mut events = Vec::new();
    let mut t = from;
    for c in text.chars() {
        let Some((key, shift)) = key_for_char(c) else {
            continue;
        };
        let press = |key: MachineKey, down: bool, at: u64| TimedInput {
            at: Cycle(at),
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: key,
                down,
            },
        };
        if shift {
            events.push(press(keys::SHIFT, true, t));
        }
        events.push(press(key, true, t + 1));
        events.push(press(key, false, t + HOLD));
        if shift {
            events.push(press(keys::SHIFT, false, t + HOLD + 1));
        }
        t += HOLD + GAP;
    }
    (events, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_positions() {
        assert_eq!(key_for_char('A'), Some((MachineKey(k(1, 0)), false)));
        assert_eq!(key_for_char('v'), Some((MachineKey(k(0, 4)), false)));
        assert_eq!(key_for_char('0'), Some((MachineKey(k(4, 0)), false)));
        assert_eq!(key_for_char('"'), Some((keys::P, true)));
        assert_eq!(key_for_char('\n'), Some((keys::NEW_LINE, false)));
        assert_eq!(key_for_char(' '), Some((keys::SPACE, false)));
        assert_eq!(key_for_char('.'), Some((MachineKey(k(7, 1)), false)));
        assert_eq!(key_for_char('@'), None);
        let (events, end) = typed_events("A\"", 0);
        assert_eq!(events.len(), 2 + 4);
        assert!(end > 0);
    }
}
