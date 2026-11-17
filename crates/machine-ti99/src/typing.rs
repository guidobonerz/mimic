//! Text typed on the keyboard (pasted listings, scripted typing; see
//! [`mimic_core::Machine::typed_keys`]).
//!
//! Characters by the keys of the TI-99/4A ([`crate::keyboard`]: machine
//! key = column × 8 + row). SHIFT gives `!@#$%^&*()` on the digits and
//! `+ - : < >` on `= / ; , .`; FCTN gives the characters printed on the
//! keys' fronts: `"` on P, `?` on I, `'` on O, `_` on U, `[ ]` on R and T,
//! `{ }` on F and G, `\` on Z, `|` on A, `~` on W and `` ` `` on C (TI-99/4A
//! User's Reference Guide, "Keyboard"). Letters are typed with SHIFT in
//! either case: they arrive in upper case whatever ALPHA LOCK says, and
//! TI BASIC takes its keywords in upper case only.

use mimic_core::input::{typed_chars, TimedInput, TypeTiming};

const FCTN: u16 = 4;
const SHIFT: u16 = 5;

/// CPU clocks (3 MHz): the console's keyboard scan runs in the GPL
/// interpreter and debounces; three fields down, three up.
const TIMING: TypeTiming = TypeTiming {
    hold: 150_000,
    gap: 150_000,
    lead: 50_000,
};

/// Clocks behind ENTER before the next line: TI BASIC is slow to take a
/// line in.
pub const LINE_PAUSE: u64 = 2_400_000;

/// Keys by position: columns 0–5, rows 0–7 (`-` for none).
const MATRIX: [&[u8; 8]; 6] = [
    b"= \n-----",
    b".LO92SWX",
    b",KI83DEC",
    b"MJU74FRV",
    b"NHY65GTB",
    b"/;P01AQZ",
];

/// The key with the label `c`.
fn plain(c: char) -> Option<u16> {
    let c = u8::try_from(c).ok().filter(|&c| c != b'-')?;
    (0..48u16).find(|&k| MATRIX[usize::from(k / 8)][usize::from(k % 8)] == c)
}

/// The keys that type `c`, the modifier first where one is needed.
pub fn keys_for_char(c: char) -> Option<Vec<u16>> {
    let c = match c {
        '\r' => '\n',
        c => c.to_ascii_uppercase(),
    };
    if c.is_ascii_uppercase() {
        return plain(c).map(|k| vec![SHIFT, k]);
    }
    if c == '-' {
        return plain('/').map(|k| vec![SHIFT, k]);
    }
    if let Some(k) = plain(c) {
        return Some(vec![k]);
    }
    let (modifier, base) = match c {
        '!' => (SHIFT, '1'),
        '@' => (SHIFT, '2'),
        '#' => (SHIFT, '3'),
        '$' => (SHIFT, '4'),
        '%' => (SHIFT, '5'),
        '^' => (SHIFT, '6'),
        '&' => (SHIFT, '7'),
        '*' => (SHIFT, '8'),
        '(' => (SHIFT, '9'),
        ')' => (SHIFT, '0'),
        '+' => (SHIFT, '='),
        ':' => (SHIFT, ';'),
        '<' => (SHIFT, ','),
        '>' => (SHIFT, '.'),
        '"' => (FCTN, 'P'),
        '?' => (FCTN, 'I'),
        '\'' => (FCTN, 'O'),
        '_' => (FCTN, 'U'),
        '[' => (FCTN, 'R'),
        ']' => (FCTN, 'T'),
        '{' => (FCTN, 'F'),
        '}' => (FCTN, 'G'),
        '\\' => (FCTN, 'Z'),
        '|' => (FCTN, 'A'),
        '~' => (FCTN, 'W'),
        '`' => (FCTN, 'C'),
        _ => return None,
    };
    plain(base).map(|k| vec![modifier, k])
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
        assert_eq!(keys_for_char('a'), Some(vec![SHIFT, 45]));
        assert_eq!(keys_for_char('A'), Some(vec![SHIFT, 45]));
        assert_eq!(keys_for_char('='), Some(vec![0]));
        assert_eq!(keys_for_char('+'), Some(vec![SHIFT, 0]));
        assert_eq!(keys_for_char('-'), Some(vec![SHIFT, 40]));
        assert_eq!(keys_for_char('"'), Some(vec![FCTN, 42]));
        assert_eq!(keys_for_char('\n'), Some(vec![2]));
        assert_eq!(keys_for_char(' '), Some(vec![1]));
        assert_eq!(keys_for_char('\u{e4}'), None);
    }
}
