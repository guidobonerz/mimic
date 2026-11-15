//! Text typed on the keyboard (pasted listings, scripted typing; see
//! [`emu_core::Machine::typed_keys`]).
//!
//! The machine keys are the unshifted characters of the IIe's US keyboard
//! ([`crate::keyboard`]), so a character is its own key, or SHIFT and the
//! key it sits on. Upper-case letters are typed with SHIFT: they arrive in
//! upper case whatever the IIe's CAPS LOCK says, and Applesoft takes its
//! keywords in upper case only. Lower-case letters are typed plain (upper
//! case on the II and II+, and on the IIe while CAPS LOCK is down, as at
//! power-on).

use emu_core::input::{typed_chars, TimedInput, TypeTiming};

use crate::keyboard::keys;

/// CPU cycles (1.02 MHz). The encoder latches a key the moment it goes
/// down, and the latch holds one character: the gap is the time the
/// monitor's input routine needs to fetch it and echo it.
const TIMING: TypeTiming = TypeTiming {
    hold: 20_000,
    gap: 20_000,
    lead: 2_000,
};

/// Cycles behind RETURN before the next line: Applesoft takes the line in
/// and the screen scrolls.
pub const LINE_PAUSE: u64 = 250_000;

/// The unshifted key whose shifted character is `c` (US layout).
fn unshifted(c: char) -> Option<char> {
    Some(match c {
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
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

/// The keys that type `c`, SHIFT first where it is needed.
pub fn keys_for_char(c: char) -> Option<Vec<u16>> {
    match c {
        '\n' | '\r' => Some(vec![keys::RETURN]),
        'A'..='Z' => Some(vec![keys::SHIFT, c.to_ascii_lowercase() as u16]),
        'a'..='z' | '0'..='9' | ' ' => Some(vec![c as u16]),
        '-' | '=' | '[' | ']' | '\\' | ';' | '\'' | ',' | '.' | '/' | '`' => Some(vec![c as u16]),
        _ => unshifted(c).map(|base| vec![keys::SHIFT, base as u16]),
    }
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
        assert_eq!(keys_for_char('a'), Some(vec![u16::from(b'a')]));
        assert_eq!(keys_for_char('A'), Some(vec![keys::SHIFT, u16::from(b'a')]));
        assert_eq!(
            keys_for_char('"'),
            Some(vec![keys::SHIFT, u16::from(b'\'')])
        );
        assert_eq!(keys_for_char('\n'), Some(vec![keys::RETURN]));
        assert_eq!(keys_for_char('\u{e4}'), None);
        let (events, end) = typed_events("1\"", 100);
        assert_eq!(events.len(), 2 + 4);
        assert_eq!(end, 100 + 2 * 40_000 + 2_000);
    }
}
