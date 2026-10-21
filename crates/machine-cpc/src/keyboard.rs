//! CPC keyboard: machine key codes and typing text.
//!
//! [`MachineKey`] values are the firmware's key numbers, `line * 8 + bit`
//! of the 10 × 8 matrix (cpcwiki, "Programming:Keyboard scanning"). The
//! line is selected by PPI port C bits 3–0; the AY's port A reads it, a 0
//! bit is a pressed key. Line 9 bits 0–5 are joystick 0, line 6 joystick 1
//! (on top of keys, as wired).
//!
//! | Line | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
//! | ---- | - | - | - | - | - | - | - | - |
//! | 0 | f. | ENTER | f3 | f6 | f9 | ↓ | → | ↑ |
//! | 1 | f0 | f2 | f1 | f5 | f8 | f7 | COPY | ← |
//! | 2 | CONTROL | \ | SHIFT | f4 | ] | RETURN | [ | CLR |
//! | 3 | . | / | : | ; | P | @ | - | ^ |
//! | 4 | , | M | K | L | I | O | 9 | 0 |
//! | 5 | SPACE | N | J | H | Y | U | 7 | 8 |
//! | 6 | V | B | F | G | T | R | 5 | 6 |
//! | 7 | X | C | D | S | W | E | 3 | 4 |
//! | 8 | Z | CAPS LOCK | A | TAB | Q | ESC | 2 | 1 |
//! | 9 | DEL | fire 3 | fire 1 | fire 2 | right | left | down | up |
//!
//! Fire 2 is the joystick's main button (BASIC's JOY() bit 4), fire 1 the
//! extra one (bit 5).

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

const fn k(line: u16, bit: u16) -> u16 {
    line * 8 + bit
}

pub mod keys {
    use emu_core::input::MachineKey;

    pub const RETURN: MachineKey = MachineKey(super::k(2, 2));
    pub const SHIFT: MachineKey = MachineKey(super::k(2, 5));
    pub const CONTROL: MachineKey = MachineKey(super::k(2, 7));
    pub const SPACE: MachineKey = MachineKey(super::k(5, 7));
    pub const ESC: MachineKey = MachineKey(super::k(8, 2));
    pub const DEL: MachineKey = MachineKey(super::k(9, 7));
    /// Joystick 0: up, down, left, right, fire 2 (main), fire 1.
    pub const JOY0: [MachineKey; 6] = [
        MachineKey(super::k(9, 0)),
        MachineKey(super::k(9, 1)),
        MachineKey(super::k(9, 2)),
        MachineKey(super::k(9, 3)),
        MachineKey(super::k(9, 4)),
        MachineKey(super::k(9, 5)),
    ];
}

/// Key and SHIFT for a character (UK layout; letters unshifted, which
/// BASIC reads as well as capitals).
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    let plain = |k: u16| Some((MachineKey(k), false));
    let shifted = |k: u16| Some((MachineKey(k), true));
    let letter = |c: char| -> Option<u16> {
        Some(match c {
            'A' => k(8, 5),
            'B' => k(6, 6),
            'C' => k(7, 6),
            'D' => k(7, 5),
            'E' => k(7, 2),
            'F' => k(6, 5),
            'G' => k(6, 4),
            'H' => k(5, 4),
            'I' => k(4, 3),
            'J' => k(5, 5),
            'K' => k(4, 5),
            'L' => k(4, 4),
            'M' => k(4, 6),
            'N' => k(5, 6),
            'O' => k(4, 2),
            'P' => k(3, 3),
            'Q' => k(8, 3),
            'R' => k(6, 2),
            'S' => k(7, 4),
            'T' => k(6, 3),
            'U' => k(5, 2),
            'V' => k(6, 7),
            'W' => k(7, 3),
            'X' => k(7, 7),
            'Y' => k(5, 3),
            'Z' => k(8, 7),
            _ => return None,
        })
    };
    if c.is_ascii_alphabetic() {
        return letter(c.to_ascii_uppercase()).map(|k| (MachineKey(k), false));
    }
    match c {
        '1' => plain(k(8, 0)),
        '2' => plain(k(8, 1)),
        '3' => plain(k(7, 1)),
        '4' => plain(k(7, 0)),
        '5' => plain(k(6, 1)),
        '6' => plain(k(6, 0)),
        '7' => plain(k(5, 1)),
        '8' => plain(k(5, 0)),
        '9' => plain(k(4, 1)),
        '0' => plain(k(4, 0)),
        '!' => shifted(k(8, 0)),
        '"' => shifted(k(8, 1)),
        '#' => shifted(k(7, 1)),
        '$' => shifted(k(7, 0)),
        '%' => shifted(k(6, 1)),
        '&' => shifted(k(6, 0)),
        '\'' => shifted(k(5, 1)),
        '(' => shifted(k(5, 0)),
        ')' => shifted(k(4, 1)),
        '_' => shifted(k(4, 0)),
        '-' => plain(k(3, 1)),
        '=' => shifted(k(3, 1)),
        '^' => plain(k(3, 0)),
        '@' => plain(k(3, 2)),
        '|' => shifted(k(3, 2)),
        '[' => plain(k(2, 1)),
        '{' => shifted(k(2, 1)),
        ']' => plain(k(2, 3)),
        '}' => shifted(k(2, 3)),
        ';' => plain(k(3, 4)),
        '+' => shifted(k(3, 4)),
        ':' => plain(k(3, 5)),
        '*' => shifted(k(3, 5)),
        ',' => plain(k(4, 7)),
        '<' => shifted(k(4, 7)),
        '.' => plain(k(3, 7)),
        '>' => shifted(k(3, 7)),
        '/' => plain(k(3, 6)),
        '?' => shifted(k(3, 6)),
        '\\' => plain(k(2, 6)),
        '`' => shifted(k(2, 6)),
        ' ' => plain(keys::SPACE.0),
        '\r' | '\n' => plain(keys::RETURN.0),
        '\x1b' => plain(keys::ESC.0),
        _ => None,
    }
}

/// T-states (4 MHz) a typed key stays down: the firmware scans the
/// keyboard 50 times a second; two scans plus margin.
const TYPE_HOLD: u64 = 200_000;
/// Gap to the next key (more than two scans, so doubled letters register).
const TYPE_GAP: u64 = 200_000;
/// SHIFT goes down a scan before the key: the firmware reads the lines in
/// order, and a key pressed with SHIFT while a scan is between line 2
/// (SHIFT) and the key's line would count unshifted.
const SHIFT_LEAD: u64 = 100_000;

/// Key events typing `text` from T-state `start`; returns them and the
/// T-state after the last key.
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
        let lead = if shift { SHIFT_LEAD } else { 0 };
        if shift {
            events.push(key_event(t, keys::SHIFT, true));
        }
        events.push(key_event(t + lead, k, true));
        events.push(key_event(t + lead + TYPE_HOLD, k, false));
        if shift {
            events.push(key_event(t + lead + TYPE_HOLD, keys::SHIFT, false));
        }
        t += lead + TYPE_HOLD + TYPE_GAP;
    }
    (events, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_once_per_shift_state() {
        let mut seen = std::collections::BTreeSet::new();
        for c in (' '..='~').chain(['\r']) {
            if c.is_ascii_lowercase() {
                continue;
            }
            if let Some((key, shift)) = key_for_char(c) {
                assert!(key.0 < 80, "{c:?}");
                assert!(seen.insert((key.0, shift)), "{c:?} doubles a key");
            }
        }
        assert_eq!(key_for_char('a'), key_for_char('A'));
    }
}
