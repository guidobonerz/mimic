//! The KC85's keyboard: a separate unit with a remote-control emitter IC
//! (U807) that sends each key as a serial word of pulses; the base unit
//! turns every pulse into a strobe at PIO port B, and CAOS measures the
//! time between two pulses with CTC channel 3.
//!
//! A word is seven bits, the lowest first: after a start pulse every bit
//! is the pause before the next pulse, **5.12 ms for 0, 7.168 ms for 1**;
//! a pause of **14.336 ms** ends the word, and the pulse after it is the
//! next word's start (the word repeats while the key is held). CAOS takes
//! pauses of 3.3–6.1 ms as 0, up to 11.2 ms as 1, up to 17.8 ms as the
//! word's end, and a silence of 20.6 ms as "key released" (CAOS 4.2's
//! interrupt routines at `$E339` and `$E345`: CTC channel 3 as a timer
//! with prescaler 256 and time constant 143). CAOS 3.1 (`$E189`,
//! `$E199`) has no lower and no upper limit: until that silence has
//! passed it takes any pulse as part of the word. So the emitter here
//! ends the last word with a pulse and then keeps quiet for 25 ms before
//! it sends another key.
//!
//! The word: key number · 2, plus 1 while SHIFT is **not** pressed. CAOS
//! looks `word XOR 1` up in its key table (two codes per key, plain and
//! shifted). [`MachineKey`] = the key number, 0 to 63; 64 is SHIFT.
//!
//! | Key | plain / SHIFT | | Key | plain / SHIFT | | Key | plain / SHIFT | | Key | plain / SHIFT |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 0 | W | | 16 | T | | 32 | I | | 48 | R |
//! | 1 | A | | 17 | F | | 33 | J | | 49 | D |
//! | 2 | 2 `"` | | 18 | 5 % | | 34 | 8 ( | | 50 | 4 $ |
//! | 3 | cursor left | | 19 | P | | 35 | SPACE `[` | | 51 | `_` `\` |
//! | 4 | HOME / CLS | | 20 | DEL | | 36 | K | | 52 | + ; |
//! | 5 | - = | | 21 | 0 @ | | 37 | , < | | 53 | / ? |
//! | 6 | F2 / F8 | | 22 | F5 / F11 | | 38 | STOP | | 54 | F4 / F10 |
//! | 7 | Y | | 23 | V | | 39 | M | | 55 | C |
//! | 8 | E | | 24 | U | | 40 | Z | | 56 | Q |
//! | 9 | S | | 25 | H | | 41 | G | | 57 | SHIFT LOCK |
//! | 10 | 3 # | | 26 | 7 ' | | 42 | 6 & | | 58 | 1 ! |
//! | 11 | ^ `]` | | 27 | O | | 43 | (none) | | 59 | cursor down |
//! | 12 | CLR | | 28 | INS | | 44 | L | | 60 | cursor up |
//! | 13 | : * | | 29 | 9 ) | | 45 | . > | | 61 | cursor right |
//! | 14 | F3 / F9 | | 30 | BRK | | 46 | F6 / F12 | | 62 | F1 / F7 |
//! | 15 | X | | 31 | N | | 47 | B | | 63 | ENTER |
//!
//! Letters come out in capitals without SHIFT (CAOS' default; SHIFT LOCK
//! swaps that).
//!
//! Sources: KC85/3 and KC85/4 Systemhandbuch (keyboard, serial
//! transmission); the pauses CAOS accepts and the key numbers are read
//! from the user's CAOS ROMs (interrupt routines and key table KTAB).

use mimic_core::input::{typed_chars, TimedInput, TypeTiming};
use mimic_core::{StateError, StateReader, StateWriter};

pub const SHIFT: u16 = 64;
pub const ENTER: u16 = 63;
pub const SPACE: u16 = 35;
pub const BRK: u16 = 30;
pub const STOP: u16 = 38;

/// Key numbers of the printable characters: plain and with SHIFT.
const CHARS: [(u16, char, char); 44] = [
    (0, 'W', 'w'),
    (1, 'A', 'a'),
    (2, '2', '"'),
    (5, '-', '='),
    (7, 'Y', 'y'),
    (8, 'E', 'e'),
    (9, 'S', 's'),
    (10, '3', '#'),
    (11, '^', ']'),
    (13, ':', '*'),
    (15, 'X', 'x'),
    (16, 'T', 't'),
    (17, 'F', 'f'),
    (18, '5', '%'),
    (19, 'P', 'p'),
    (21, '0', '@'),
    (23, 'V', 'v'),
    (24, 'U', 'u'),
    (25, 'H', 'h'),
    (26, '7', '\''),
    (27, 'O', 'o'),
    (29, '9', ')'),
    (31, 'N', 'n'),
    (32, 'I', 'i'),
    (33, 'J', 'j'),
    (34, '8', '('),
    (35, ' ', '['),
    (36, 'K', 'k'),
    (37, ',', '<'),
    (39, 'M', 'm'),
    (40, 'Z', 'z'),
    (41, 'G', 'g'),
    (42, '6', '&'),
    (44, 'L', 'l'),
    (45, '.', '>'),
    (47, 'B', 'b'),
    (48, 'R', 'r'),
    (49, 'D', 'd'),
    (50, '4', '$'),
    (51, '_', '\\'),
    (52, '+', ';'),
    (53, '/', '?'),
    (55, 'C', 'c'),
    (56, 'Q', 'q'),
];

/// The keys that type `c`: SHIFT first where it is needed.
pub fn keys_for_char(c: char) -> Option<Vec<u16>> {
    match c {
        '\n' | '\r' => return Some(vec![ENTER]),
        '1' => return Some(vec![58]),
        '!' => return Some(vec![SHIFT, 58]),
        _ => {}
    }
    CHARS.iter().find_map(|&(key, plain, shifted)| {
        if c == plain {
            Some(vec![key])
        } else if c == shifted {
            Some(vec![SHIFT, key])
        } else {
            None
        }
    })
}

/// Key events that type `text` from clock `start` of a CPU running at
/// `hz`: a key is held for 90 ms (a word, its end and the next word's
/// start) and released for 45 ms (CAOS forgets the key after 20.6 ms of
/// silence, so a doubled letter counts twice).
pub fn typed_events(text: &str, start: u64, hz: u32) -> (Vec<TimedInput>, u64) {
    let ms = u64::from(hz) / 1000;
    let timing = TypeTiming {
        hold: 90 * ms,
        gap: 45 * ms,
        lead: 5 * ms,
    };
    typed_chars(text, start, timing, keys_for_char)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Nothing is being sent.
    Idle,
    /// A word's pulses: the pause before the next pulse ends at `next`;
    /// `sent` bits have been sent.
    Word { word: u8, sent: u8, next: u64 },
    /// The pause after a word.
    End { next: u64 },
    /// Silence after the last word, so that CAOS sees the key released
    /// before another word starts.
    Rest { until: u64 },
}

/// The emitter in the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    /// Keys held, by number.
    held: u64,
    shift: bool,
    /// The key pressed last, if it is still held.
    key: Option<u8>,
    phase: Phase,
    /// The pauses in CPU clocks.
    zero: u64,
    one: u64,
    end: u64,
    rest: u64,
}

impl Keyboard {
    pub fn new(hz: u32) -> Self {
        let us = |n: u64| u64::from(hz) * n / 1_000_000;
        Self {
            held: 0,
            shift: false,
            key: None,
            phase: Phase::Idle,
            zero: us(5120),
            one: us(7168),
            end: us(14336),
            rest: us(25_000),
        }
    }

    /// A key goes down or up.
    pub fn key(&mut self, key: u16, down: bool) {
        match key {
            SHIFT => self.shift = down,
            0..=63 => {
                let bit = 1u64 << key;
                if down {
                    self.held |= bit;
                    self.key = Some(key as u8);
                } else {
                    self.held &= !bit;
                    if self.key == Some(key as u8) {
                        // Another key that is still down takes over.
                        self.key = (self.held != 0).then(|| self.held.trailing_zeros() as u8);
                    }
                }
            }
            _ => {}
        }
    }

    /// All keys up (the keyboard keeps sending the word it is in).
    pub fn release_all(&mut self) {
        self.held = 0;
        self.shift = false;
        self.key = None;
    }

    fn word(&self) -> Option<u8> {
        self.key.map(|k| k << 1 | u8::from(!self.shift))
    }

    fn pause(&self, word: u8, bit: u8) -> u64 {
        if word >> bit & 1 != 0 {
            self.one
        } else {
            self.zero
        }
    }

    /// Run to CPU clock `now`. True if a pulse fell into the time since
    /// the last call (at most one: callers come by every instruction).
    pub fn run(&mut self, now: u64) -> bool {
        match self.phase {
            Phase::Idle => match self.word() {
                Some(word) => {
                    self.phase = Phase::Word {
                        word,
                        sent: 0,
                        next: now + self.pause(word, 0),
                    };
                    true
                }
                None => false,
            },
            Phase::Word { word, sent, next } if now >= next => {
                let sent = sent + 1;
                self.phase = if sent < 7 {
                    Phase::Word {
                        word,
                        sent,
                        next: next + self.pause(word, sent),
                    }
                } else {
                    Phase::End {
                        next: next + self.end,
                    }
                };
                true
            }
            Phase::End { next } if now >= next => {
                // The pulse that ends the word starts the next one while
                // a key is held.
                self.phase = match self.word() {
                    Some(word) => Phase::Word {
                        word,
                        sent: 0,
                        next: next + self.pause(word, 0),
                    },
                    None => Phase::Rest {
                        until: next + self.rest,
                    },
                };
                true
            }
            Phase::Rest { until } if now >= until => {
                self.phase = Phase::Idle;
                false
            }
            _ => false,
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u64(self.held);
        w.bool(self.shift);
        w.u8(self.key.unwrap_or(0xff));
        match self.phase {
            Phase::Idle => w.u8(0),
            Phase::Word { word, sent, next } => {
                w.u8(1);
                w.u8(word);
                w.u8(sent);
                w.u64(next);
            }
            Phase::End { next } => {
                w.u8(2);
                w.u64(next);
            }
            Phase::Rest { until } => {
                w.u8(3);
                w.u64(until);
            }
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let held = r.u64()?;
        let shift = r.bool()?;
        let key = match r.u8()? {
            0xff => None,
            k @ 0..=63 => Some(k),
            _ => return Err(StateError::Invalid("KC85 key")),
        };
        let phase = match r.u8()? {
            0 => Phase::Idle,
            1 => {
                let (word, sent) = (r.u8()?, r.u8()?);
                if word > 127 || sent > 6 {
                    return Err(StateError::Invalid("KC85 keyboard word"));
                }
                Phase::Word {
                    word,
                    sent,
                    next: r.u64()?,
                }
            }
            2 => Phase::End { next: r.u64()? },
            3 => Phase::Rest { until: r.u64()? },
            _ => return Err(StateError::Invalid("KC85 keyboard phase")),
        };
        self.held = held;
        self.shift = shift;
        self.key = key;
        self.phase = phase;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clocks of the pulses while `kb` runs to `until`.
    fn pulses(kb: &mut Keyboard, until: u64) -> Vec<u64> {
        (0..until).filter(|&t| kb.run(t)).collect()
    }

    #[test]
    fn a_word_is_seven_pauses_and_repeats_while_the_key_is_held() {
        let mut kb = Keyboard::new(1_000_000);
        kb.key(1, true);
        // Key 1 without SHIFT: word 3, bits 1 1 0 0 0 0 0.
        let p = pulses(&mut kb, 70_000);
        let pauses: Vec<u64> = p.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(
            pauses[..9],
            [7168, 7168, 5120, 5120, 5120, 5120, 5120, 14336, 7168]
        );
        kb.key(1, false);
        let rest = pulses(&mut kb, 200_000);
        // The word in flight is finished and ended; then silence.
        assert!(kb.phase == Phase::Idle);
        // A key right after the last word waits for the silence.
        let mut kb = Keyboard::new(1_000_000);
        kb.key(1, true);
        assert!(kb.run(0));
        kb.key(1, false);
        let end = *pulses(&mut kb, 60_000).last().unwrap();
        kb.key(2, true);
        let start = (end + 1..end + 100_000).find(|&t| kb.run(t)).unwrap();
        assert!(start - end >= 25_000, "{}", start - end);
        assert!(rest.len() <= 8, "{}", rest.len());
    }

    #[test]
    fn shift_clears_the_lowest_bit() {
        let mut kb = Keyboard::new(1_000_000);
        kb.key(SHIFT, true);
        assert_eq!(kb.word(), None, "SHIFT alone sends nothing");
        kb.key(1, true);
        assert_eq!(kb.word(), Some(2));
        kb.key(SHIFT, false);
        assert_eq!(kb.word(), Some(3));
    }

    #[test]
    fn characters_and_their_keys() {
        assert_eq!(keys_for_char('A'), Some(vec![1]));
        assert_eq!(keys_for_char('a'), Some(vec![SHIFT, 1]));
        assert_eq!(keys_for_char('"'), Some(vec![SHIFT, 2]));
        assert_eq!(keys_for_char('1'), Some(vec![58]));
        assert_eq!(keys_for_char('\n'), Some(vec![ENTER]));
        assert_eq!(keys_for_char('\u{e4}'), None);
        // Every printable ASCII character but the ones the keyboard
        // lacks.
        for c in ' '..='~' {
            let missing = "`{|}~".contains(c);
            assert_eq!(keys_for_char(c).is_none(), missing, "{c:?}");
        }
    }
}
