//! The MAX-80's keyboard: an 8 × 8 matrix read at offsets `$800–$8FF` of
//! the 4 KB block, laid out as the TRS-80's. The address lines A0–A7 each
//! select a row; a read returns the OR of the selected rows, a pressed key
//! as 1. [`MachineKey`] = row · 8 + bit.
//!
//! | Row (offset) | bit 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 0 (`$801`) | @ | A | B | C | D | E | F | G |
//! | 1 (`$802`) | H | I | J | K | L | M | N | O |
//! | 2 (`$804`) | P | Q | R | S | T | U | V | W |
//! | 3 (`$808`) | X | Y | Z | [ | \ | ] | ^ | _ |
//! | 4 (`$810`) | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
//! | 5 (`$820`) | 8 | 9 | : | ; | , | - | . | / |
//! | 6 (`$840`) | ENTER | CLEAR | BREAK | ↑ | ↓ | ← | → | SPACE |
//! | 7 (`$880`) | SHIFT | F1 | F2 | F3 | F4 | ESC | – | CTRL |
//!
//! The numeric keypad shares its positions with the main keys. Shifted
//! digits give `! " # $ % & ' ( )`, SHIFT `: ; , - . /` give `* + < = >
//! ?`, SHIFT `@ [ \ ] ^` give `` ` { | } ~ ``. Letters come in lower case
//! without SHIFT. The systems take the key at CLEAR's place as backspace.
//!
//! Sources: see `io.rs` (the diagnostic disk's keyboard test names every
//! position).

use mimic_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use mimic_core::Cycle;

pub const fn key(row: u16, bit: u16) -> u16 {
    row * 8 + bit
}

pub mod keys {
    use mimic_core::input::MachineKey;

    pub const ENTER: MachineKey = MachineKey(super::key(6, 0));
    pub const CLEAR: MachineKey = MachineKey(super::key(6, 1));
    pub const BREAK: MachineKey = MachineKey(super::key(6, 2));
    pub const UP: MachineKey = MachineKey(super::key(6, 3));
    pub const DOWN: MachineKey = MachineKey(super::key(6, 4));
    pub const LEFT: MachineKey = MachineKey(super::key(6, 5));
    pub const RIGHT: MachineKey = MachineKey(super::key(6, 6));
    pub const SPACE: MachineKey = MachineKey(super::key(6, 7));
    pub const SHIFT: MachineKey = MachineKey(super::key(7, 0));
    pub const F1: MachineKey = MachineKey(super::key(7, 1));
    pub const ESC: MachineKey = MachineKey(super::key(7, 5));
    pub const CTRL: MachineKey = MachineKey(super::key(7, 7));
}

/// The matrix read at block offset `off`: the OR of the rows whose address
/// bit is set.
pub fn read(rows: &[u8; 8], off: u16) -> u8 {
    rows.iter()
        .enumerate()
        .filter(|(row, _)| off & (1 << row) != 0)
        .fold(0, |v, (_, r)| v | r)
}

/// The key for a character and whether it needs SHIFT.
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    const ROWS: [&str; 6] = [
        "@abcdefg",
        "hijklmno",
        "pqrstuvw",
        "xyz[\\]^_",
        "01234567",
        "89:;,-./",
    ];
    let plain = |c: char| {
        ROWS.iter().enumerate().find_map(|(row, cols)| {
            cols.chars()
                .position(|x| x == c)
                .map(|bit| MachineKey(key(row as u16, bit as u16)))
        })
    };
    match c {
        '\n' | '\r' => return Some((keys::ENTER, false)),
        ' ' => return Some((keys::SPACE, false)),
        '\x08' => return Some((keys::CLEAR, false)),
        '\x1b' => return Some((keys::ESC, false)),
        _ => {}
    }
    if let Some(k) = plain(c) {
        return Some((k, false));
    }
    let base = match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '"' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '&' => '6',
        '\'' => '7',
        '(' => '8',
        ')' => '9',
        '*' => ':',
        '+' => ';',
        '<' => ',',
        '=' => '-',
        '>' => '.',
        '?' => '/',
        '`' => '@',
        '{' => '[',
        '|' => '\\',
        '}' => ']',
        '~' => '^',
        _ => return None,
    };
    plain(base).map(|k| (k, true))
}

/// Key events that type `text`: each key held `hold` clocks, then `gap`
/// clocks up. Returns the events and the clock after the last release.
pub fn typed_events(text: &str, from: u64, hold: u64, gap: u64) -> (Vec<TimedInput>, u64) {
    let press = |key: MachineKey, down: bool, at: u64| TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: key,
            down,
        },
    };
    let mut events = Vec::new();
    let mut t = from;
    for c in text.chars() {
        let Some((key, shift)) = key_for_char(c) else {
            continue;
        };
        if shift {
            events.push(press(keys::SHIFT, true, t));
        }
        events.push(press(key, true, t + 1));
        events.push(press(key, false, t + hold));
        if shift {
            events.push(press(keys::SHIFT, false, t + hold + 1));
        }
        t += hold + gap;
    }
    (events, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix() {
        assert_eq!(key_for_char('a'), Some((MachineKey(key(0, 1)), false)));
        assert_eq!(key_for_char('Z'), Some((MachineKey(key(3, 2)), true)));
        assert_eq!(key_for_char('9'), Some((MachineKey(key(5, 1)), false)));
        assert_eq!(key_for_char('"'), Some((MachineKey(key(4, 2)), true)));
        assert_eq!(key_for_char('?'), Some((MachineKey(key(5, 7)), true)));
        assert_eq!(key_for_char('['), Some((MachineKey(key(3, 3)), false)));
        assert_eq!(key_for_char('~'), Some((MachineKey(key(3, 6)), true)));
        let mut rows = [0u8; 8];
        rows[1] = 0x04;
        rows[6] = 0x01;
        assert_eq!(read(&rows, 0x802), 0x04);
        assert_eq!(read(&rows, 0x842), 0x05);
        assert_eq!(read(&rows, 0x801), 0);
    }
}
