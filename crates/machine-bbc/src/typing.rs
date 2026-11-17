//! Text typed on the keyboard (pasted listings, scripted typing; see
//! [`mimic_core::Machine::typed_keys`]).
//!
//! Characters by the keys of the BBC's own layout ([`crate::keyboard`]:
//! machine key = row × 16 + column of the matrix): the digits' shifted
//! characters are `!"#$%&'()`, `=` is SHIFT `-`, `+` SHIFT `;`, `*` SHIFT
//! `:`. Letters are typed without SHIFT in either case: with CAPS LOCK on,
//! as after power-on, they arrive in upper case, which is what BBC BASIC
//! takes its keywords in.

use mimic_core::input::{typed_chars, TimedInput, TypeTiming};

use crate::keyboard::keys;

/// 2 MHz cycles: the MOS scans the keyboard on its 100 Hz interrupt and
/// debounces over two of them; three fields down, three up.
const TIMING: TypeTiming = TypeTiming {
    hold: 120_000,
    gap: 120_000,
    lead: 40_000,
};

/// Cycles behind RETURN before the next line.
pub const LINE_PAUSE: u64 = 400_000;

/// Matrix codes of the letters A to Z.
const LETTERS: [u16; 26] = [
    0x41, 0x64, 0x52, 0x32, 0x22, 0x43, 0x53, 0x54, 0x25, 0x45, 0x46, 0x56, 0x65, 0x55, 0x36, 0x37,
    0x10, 0x33, 0x51, 0x23, 0x35, 0x63, 0x21, 0x42, 0x44, 0x61,
];

/// Matrix codes of the digits 0 to 9.
const DIGITS: [u16; 10] = [0x27, 0x30, 0x31, 0x11, 0x12, 0x13, 0x34, 0x24, 0x15, 0x26];

/// The key of a character typed without SHIFT.
fn plain(c: char) -> Option<u16> {
    Some(match c {
        'A'..='Z' => LETTERS[(c as u8 - b'A') as usize],
        '0'..='9' => DIGITS[(c as u8 - b'0') as usize],
        ' ' => keys::SPACE,
        '\n' => keys::RETURN,
        '-' => 0x17,
        '^' => 0x18,
        '\\' => 0x78,
        '@' => 0x47,
        '[' => 0x38,
        '_' => 0x28,
        ']' => 0x58,
        ';' => 0x57,
        ':' => 0x48,
        ',' => 0x66,
        '.' => 0x67,
        '/' => 0x68,
        _ => return None,
    })
}

/// The keys that type `c`, SHIFT first where it is needed.
pub fn keys_for_char(c: char) -> Option<Vec<u16>> {
    let c = match c {
        '\r' => '\n',
        c => c.to_ascii_uppercase(),
    };
    if let Some(k) = plain(c) {
        return Some(vec![k]);
    }
    let base = match c {
        '!' => '1',
        '"' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '&' => '6',
        '\'' => '7',
        '(' => '8',
        ')' => '9',
        '=' => '-',
        '~' => '^',
        '|' => '\\',
        '{' => '[',
        '\u{a3}' => '_',
        '}' => ']',
        '+' => ';',
        '*' => ':',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        _ => return None,
    };
    plain(base).map(|k| vec![keys::SHIFT, k])
}

/// Key events that type `text` from cycle `start`; the events and the
/// cycle after the last key.
pub fn typed_events(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    typed_chars(text, start, TIMING, keys_for_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_and_their_keys() {
        assert_eq!(keys_for_char('q'), Some(vec![0x10]));
        assert_eq!(keys_for_char('Q'), Some(vec![0x10]));
        assert_eq!(keys_for_char('"'), Some(vec![keys::SHIFT, 0x31]));
        assert_eq!(keys_for_char('='), Some(vec![keys::SHIFT, 0x17]));
        assert_eq!(keys_for_char('\n'), Some(vec![keys::RETURN]));
        assert_eq!(keys_for_char('\u{e4}'), None);
        // Every key once.
        let mut seen = std::collections::BTreeSet::new();
        for c in (' '..='_').chain(['\n']) {
            if let Some(k) = plain(c) {
                assert!(seen.insert(k), "{c:?} doubles a key");
            }
        }
        assert_eq!(seen.len(), 26 + 10 + 2 + 12);
    }
}
