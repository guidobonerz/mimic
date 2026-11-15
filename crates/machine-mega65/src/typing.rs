//! Text typed on the keyboard (pasted listings, scripted typing; see
//! [`emu_core::Machine::typed_keys`]).
//!
//! The MEGA65's main keys sit in the C64's matrix ([`crate::keyboard`]:
//! machine key = column × 8 + row), so a character is typed as on the
//! C64: letters in either case without SHIFT (upper case in the power-on
//! character set), `!"#$%&'()` as the shifted digits 1 to 9, `< > ? [ ]`
//! as the shifted `, . / : ;`.

use emu_core::input::{typed_chars, TimedInput, TypeTiming};

use crate::keyboard::keys;

/// Master clocks (40.5 MHz): the keyboard is scanned once a frame; three
/// frames down, three up.
const TIMING: TypeTiming = TypeTiming {
    hold: 2_430_000,
    gap: 2_430_000,
    lead: 0,
};

/// Master clocks behind RETURN before the next line.
pub const LINE_PAUSE: u64 = 10_000_000;

/// Keys by position: columns 0–7, rows 0–7 (`~` for keys without a
/// character of their own, `£` written as `\`).
const MATRIX: [&[u8; 8]; 8] = [
    b"~\n~~~~~~",
    b"3WA4ZSE~",
    b"5RD6CFTX",
    b"7YG8BHUV",
    b"9IJ0MKON",
    b"+PL-.:@,",
    b"\\*;~~=~/",
    b"1~~2 ~Q~",
];

/// The key with the label `c`.
fn plain(c: char) -> Option<u16> {
    let c = match c {
        '\u{a3}' => b'\\',
        '~' | '\\' => return None,
        c => u8::try_from(c).ok()?,
    };
    (0..64u16).find(|&k| MATRIX[usize::from(k / 8)][usize::from(k % 8)] == c)
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
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '[' => ':',
        ']' => ';',
        _ => return None,
    };
    plain(base).map(|k| vec![keys::LSHIFT, k])
}

/// Key events that type `text` from clock `start`; the events and the
/// clock after the last key.
pub fn typed_events(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    typed_chars(text, start, TIMING, keys_for_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_and_their_keys() {
        assert_eq!(keys_for_char('a'), Some(vec![keys::key(1, 2)]));
        assert_eq!(keys_for_char('Q'), Some(vec![keys::key(7, 6)]));
        assert_eq!(keys_for_char('\n'), Some(vec![keys::RETURN]));
        assert_eq!(
            keys_for_char('"'),
            Some(vec![keys::LSHIFT, keys::key(7, 3)])
        );
        assert_eq!(
            keys_for_char(')'),
            Some(vec![keys::LSHIFT, keys::key(4, 0)])
        );
        assert_eq!(keys_for_char('='), Some(vec![keys::key(6, 5)]));
        assert_eq!(keys_for_char('~'), None);
    }
}
