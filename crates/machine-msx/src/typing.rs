//! Text typed on the keyboard (pasted listings, scripted typing; see
//! [`emu_core::Machine::typed_keys`]).
//!
//! Characters by the keys of the international MSX matrix (machine key =
//! row × 8 + bit; MSX Technical Data Book, "Keyboard matrix"): row 0 the
//! digits 0–7, row 1 `8 9 - = \ [ ] ;`, row 2 `' `` ` `` , . /`, a dead key
//! and A, B, rows 3–5 C to Z. SHIFT gives `!@#$%^&*()` on the digits and
//! `_ + | { } : " ~ < > ?` on the symbols. Lower-case letters are typed
//! plain and upper-case ones with SHIFT (CAPS off, as after power-on);
//! MSX BASIC takes its keywords in either case. A machine with another
//! national layout (Japanese, German …) has some symbols elsewhere.

use emu_core::input::{typed_chars, TimedInput, TypeTiming};

use crate::keys;

/// T-states (3.58 MHz): the BIOS scans the keyboard every field and
/// takes a key that is the same on two scans; four fields down, four up.
const TIMING: TypeTiming = TypeTiming {
    hold: 240_000,
    gap: 240_000,
    lead: 120_000,
};

/// T-states behind RETURN before the next line.
pub const LINE_PAUSE: u64 = 700_000;

/// The key of a character typed without SHIFT.
fn plain(c: char) -> Option<u16> {
    let k = keys::key;
    Some(match c {
        '0'..='7' => k(0, c as u16 - '0' as u16),
        '8' => k(1, 0),
        '9' => k(1, 1),
        '-' => k(1, 2),
        '=' => k(1, 3),
        '\\' => k(1, 4),
        '[' => k(1, 5),
        ']' => k(1, 6),
        ';' => k(1, 7),
        '\'' => k(2, 0),
        '`' => k(2, 1),
        ',' => k(2, 2),
        '.' => k(2, 3),
        '/' => k(2, 4),
        'a' | 'b' => k(2, 6 + c as u16 - 'a' as u16),
        'c'..='z' => {
            let n = c as u16 - 'c' as u16;
            k(3 + n / 8, n % 8)
        }
        ' ' => keys::SPACE,
        '\n' => keys::RETURN,
        _ => return None,
    })
}

/// The keys that type `c`, SHIFT first where it is needed.
pub fn keys_for_char(c: char) -> Option<Vec<u16>> {
    let c = if c == '\r' { '\n' } else { c };
    if let Some(k) = plain(c) {
        return Some(vec![k]);
    }
    let base = match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '|' => '\\',
        '{' => '[',
        '}' => ']',
        ':' => ';',
        '"' => '\'',
        '~' => '`',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        _ => return None,
    };
    plain(base).map(|k| vec![keys::SHIFT, k])
}

/// Key events that type `text` from T-state `start`; the events and the
/// T-state after the last key.
pub fn typed_events(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    typed_chars(text, start, TIMING, keys_for_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_and_their_keys() {
        assert_eq!(keys_for_char('a'), Some(vec![keys::key(2, 6)]));
        assert_eq!(keys_for_char('z'), Some(vec![keys::key(5, 7)]));
        assert_eq!(keys_for_char('Z'), Some(vec![keys::SHIFT, keys::key(5, 7)]));
        assert_eq!(keys_for_char('"'), Some(vec![keys::SHIFT, keys::key(2, 0)]));
        assert_eq!(keys_for_char(':'), Some(vec![keys::SHIFT, keys::key(1, 7)]));
        assert_eq!(keys_for_char('\n'), Some(vec![keys::RETURN]));
        assert_eq!(keys_for_char('\u{e4}'), None);
    }
}
