//! 800XL keyboard: machine key codes and typing text.
//!
//! [`MachineKey`] values 0–63 are POKEY key codes (KBCODE bits 0–5, Atari
//! 400/800/XL Technical Reference Notes, keyboard table); the keys outside
//! the scanned matrix follow:
//!
//! | Code | Key     | Wiring                       |
//! | ---- | ------- | ---------------------------- |
//! | 64   | SHIFT   | POKEY SKSTAT bit 3, KBCODE bit 6 |
//! | 65   | CONTROL | KBCODE bit 7                 |
//! | 66   | BREAK   | POKEY IRQ bit 7              |
//! | 67   | START   | GTIA CONSOL bit 0            |
//! | 68   | SELECT  | GTIA CONSOL bit 1            |
//! | 69   | OPTION  | GTIA CONSOL bit 2            |
//! | 70   | RESET   | system reset line            |

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

pub mod keys {
    use emu_core::input::MachineKey;

    pub const L: MachineKey = MachineKey(0x00);
    pub const RETURN: MachineKey = MachineKey(0x0c);
    pub const SPACE: MachineKey = MachineKey(0x21);
    pub const SHIFT: MachineKey = MachineKey(64);
    pub const CONTROL: MachineKey = MachineKey(65);
    pub const BREAK: MachineKey = MachineKey(66);
    pub const START: MachineKey = MachineKey(67);
    pub const SELECT: MachineKey = MachineKey(68);
    pub const OPTION: MachineKey = MachineKey(69);
    pub const RESET: MachineKey = MachineKey(70);
}

/// Key code and SHIFT for a character (caps lock on, as after power-up).
pub fn key_for_char(c: char) -> Option<(MachineKey, bool)> {
    let plain = |k: u16| Some((MachineKey(k), false));
    let shifted = |k: u16| Some((MachineKey(k), true));
    match c.to_ascii_uppercase() {
        'A' => plain(0x3f),
        'B' => plain(0x15),
        'C' => plain(0x12),
        'D' => plain(0x3a),
        'E' => plain(0x2a),
        'F' => plain(0x38),
        'G' => plain(0x3d),
        'H' => plain(0x39),
        'I' => plain(0x0d),
        'J' => plain(0x01),
        'K' => plain(0x05),
        'L' => plain(0x00),
        'M' => plain(0x25),
        'N' => plain(0x23),
        'O' => plain(0x08),
        'P' => plain(0x0a),
        'Q' => plain(0x2f),
        'R' => plain(0x28),
        'S' => plain(0x3e),
        'T' => plain(0x2d),
        'U' => plain(0x0b),
        'V' => plain(0x10),
        'W' => plain(0x2e),
        'X' => plain(0x16),
        'Y' => plain(0x2b),
        'Z' => plain(0x17),
        '1' => plain(0x1f),
        '2' => plain(0x1e),
        '3' => plain(0x1a),
        '4' => plain(0x18),
        '5' => plain(0x1d),
        '6' => plain(0x1b),
        '7' => plain(0x33),
        '8' => plain(0x35),
        '9' => plain(0x30),
        '0' => plain(0x32),
        '!' => shifted(0x1f),
        '"' => shifted(0x1e),
        '#' => shifted(0x1a),
        '$' => shifted(0x18),
        '%' => shifted(0x1d),
        '&' => shifted(0x1b),
        '\'' => shifted(0x33),
        '@' => shifted(0x35),
        '(' => shifted(0x30),
        ')' => shifted(0x32),
        ' ' => plain(0x21),
        '\r' | '\n' => plain(0x0c),
        ',' => plain(0x20),
        '.' => plain(0x22),
        ';' => plain(0x02),
        ':' => shifted(0x02),
        '+' => plain(0x06),
        '\\' => shifted(0x06),
        '*' => plain(0x07),
        '^' => shifted(0x07),
        '-' => plain(0x0e),
        '_' => shifted(0x0e),
        '=' => plain(0x0f),
        '|' => shifted(0x0f),
        '/' => plain(0x26),
        '?' => shifted(0x26),
        '<' => plain(0x36),
        '>' => plain(0x37),
        _ => None,
    }
}

/// Cycles a typed key stays down: two passes of POKEY's debounced scan
/// (64 × 114 cycles each) plus margin.
const TYPE_HOLD: u64 = 30_000;
/// Gap to the next key; more than the OS's repeat delay for the same key
/// (KEYDEL, 3 fields), so doubled letters register.
const TYPE_GAP: u64 = 90_000;

/// Key events typing `text` from cycle `start`; returns them and the cycle
/// after the last key.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_matrix_code_is_used_once_per_shift_state() {
        let mut seen = std::collections::BTreeSet::new();
        for c in (' '..='_').chain(['\r']) {
            if let Some((k, shift)) = key_for_char(c) {
                assert!(k.0 < 64, "{c:?}");
                assert!(seen.insert((k.0, shift)), "{c:?} doubles a key");
            }
        }
    }

    #[test]
    fn typing_holds_shift_around_the_key() {
        let (ev, end) = typed_events("A\"", 100);
        assert_eq!(ev.len(), 6);
        assert_eq!(end, 100 + 2 * (TYPE_HOLD + TYPE_GAP));
        assert!(matches!(
            ev[2].event,
            InputEvent::Key { machine_key, down: true } if machine_key == keys::SHIFT
        ));
    }
}
