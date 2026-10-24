//! The Macintosh 128K/512K/Plus keyboard (M0110, M0110A with keypad and
//! arrows) on the VIA's serial port: the keyboard drives the clock (CB1),
//! data goes both ways on CB2, and the VIA's shift register moves the
//! bits under the external clock (ACR mode 7 to send, mode 3 to receive).
//!
//! **Protocol** (Inside Macintosh III, "The Keyboard and Keypad"; tmk,
//! "Apple M0110 Keyboard Protocol", github.com/tmk/tmk_keyboard/wiki):
//! the Mac sends a command byte, the keyboard answers with one byte.
//! Inquiry ($10) waits up to a quarter second for a key transition and
//! answers it or Null ($7B); Instant ($14) answers at once; Model Number
//! ($16) resets the keyboard and answers its model (M0110 $09, M0110A
//! $0B: bits 1–3 the model, bit 0 set); Test ($36) answers ACK ($7D). A
//! key transition is the key's raw code (bits 6–1) with bit 0 set and
//! bit 7 set on release. Keypad keys and the M0110A's arrows come as
//! $79 followed by the key's byte on the next Instant; the M0110A's
//! keypad + * / = are sent as Shift ($71, release $F1) with an arrow.
//!
//! Timing: a bit takes 400 µs from the Mac and 330 µs from the keyboard
//! (tmk), so a command is in after 3.2 ms and an answer takes 2.6 ms.
//! The answer shifts into the VIA only once the Mac has set the shift
//! register to receive.
//!
//! **Key numbers** ([`emu_core::input::MachineKey`]): the Macintosh key
//! codes as Inside Macintosh lists them (the ADB keyboard's codes): the
//! main block $00–$3A (raw code = key code), keypad $41–$5C, arrows
//! $7B left, $7C right, $7D down, $7E up. The SE's ADB keyboard uses the
//! same numbers.

use std::collections::VecDeque;

use chips_cbm::via::Via;
use emu_core::{StateError, StateReader, StateWriter};

/// CPU clocks (7.8336 MHz) for the Mac's command byte and for the
/// keyboard's answer.
pub const COMMAND_CLOCKS: u64 = 25_068;
pub const ANSWER_CLOCKS: u64 = 20_680;
/// Inquiry timeout: a quarter second.
pub const INQUIRY_CLOCKS: u64 = 1_958_400;

pub const NULL: u8 = 0x7b;
pub const ACK: u8 = 0x7d;
pub const NAK: u8 = 0x77;

/// The bytes a key transition sends (`model_a`: M0110A).
pub fn transition(key: u8, down: bool, model_a: bool) -> Vec<u8> {
    let up = if down { 0 } else { 0x80 };
    let arrow = |code: u8| vec![0x79, code | up];
    match key {
        0x00..=0x3f => vec![(key << 1 | 1) | up],
        _ if !model_a => Vec::new(),
        0x7b => arrow(0x0d),
        0x7c => arrow(0x05),
        0x7d => arrow(0x11),
        0x7e => arrow(0x1b),
        // Keypad + * / =: Shift with an arrow.
        0x45 | 0x43 | 0x4b | 0x51 => {
            let code = match key {
                0x45 => 0x0d,
                0x43 => 0x05,
                0x4b => 0x1b,
                _ => 0x11,
            };
            vec![0x71 | up, 0x79, code | up]
        }
        0x41 | 0x47 | 0x4c | 0x4e | 0x52..=0x59 | 0x5b | 0x5c => {
            vec![0x79, ((key - 0x40) << 1 | 1) | up]
        }
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// The Mac's command is coming in; complete at the clock.
    Receiving(u64),
    /// Inquiry: waiting for a key until the clock.
    Waiting(u64),
    /// The answer starts shifting at the clock.
    Answer(u8, u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    model_a: bool,
    queue: VecDeque<u8>,
    state: State,
}

impl Keyboard {
    pub fn new(model_a: bool) -> Self {
        Self {
            model_a,
            queue: VecDeque::new(),
            state: State::Idle,
        }
    }

    pub fn key(&mut self, key: u8, down: bool) {
        self.queue.extend(transition(key, down, self.model_a));
    }

    /// The Mac wrote the shift register in mode 7: a command starts.
    pub fn command_started(&mut self, now: u64) {
        self.state = State::Receiving(now + COMMAND_CLOCKS);
    }

    /// The next clock something happens (`u64::MAX`: nothing).
    pub fn next_event(&self) -> u64 {
        match self.state {
            State::Idle => u64::MAX,
            State::Receiving(t) | State::Answer(_, t) => t,
            State::Waiting(t) => {
                if self.queue.is_empty() {
                    t
                } else {
                    0
                }
            }
        }
    }

    pub fn run(&mut self, now: u64, via: &mut Via) {
        loop {
            match self.state {
                State::Receiving(t) if now >= t => {
                    let mut cmd = 0u8;
                    for _ in 0..8 {
                        let bit = via.shift_ext(true).unwrap_or(true);
                        cmd = cmd << 1 | u8::from(bit);
                    }
                    self.state = match cmd {
                        0x10 => State::Waiting(t + INQUIRY_CLOCKS),
                        0x14 => State::Answer(self.queue.pop_front().unwrap_or(NULL), t),
                        0x16 => {
                            self.queue.clear();
                            State::Answer(if self.model_a { 0x0b } else { 0x09 }, t)
                        }
                        0x36 => State::Answer(ACK, t),
                        _ => State::Answer(NAK, t),
                    };
                }
                State::Waiting(t) => {
                    if let Some(b) = self.queue.pop_front() {
                        self.state = State::Answer(b, now);
                    } else if now >= t {
                        self.state = State::Answer(NULL, t);
                    } else {
                        return;
                    }
                }
                State::Answer(b, t) if now >= t + ANSWER_CLOCKS => {
                    // The Mac must be listening (mode 3).
                    if via.acr >> 2 & 7 != 3 || via.shift_bits_left() == 0 {
                        self.state = State::Answer(b, now.saturating_sub(ANSWER_CLOCKS) + 1000);
                        return;
                    }
                    for k in (0..8).rev() {
                        via.shift_ext(b >> k & 1 != 0);
                    }
                    self.state = State::Idle;
                }
                _ => return,
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MKBD", 1, |w| {
            w.bool(self.model_a);
            w.bytes(&self.queue.iter().copied().collect::<Vec<u8>>());
            let (tag, b, t) = match self.state {
                State::Idle => (0, 0, 0),
                State::Receiving(t) => (1, 0, t),
                State::Waiting(t) => (2, 0, t),
                State::Answer(b, t) => (3, b, t),
            };
            w.u8(tag);
            w.u8(b);
            w.u64(t);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MKBD", 1)?;
        let model_a = c.bool()?;
        let queue = c.bytes()?.iter().copied().collect();
        let (tag, b, t) = (c.u8()?, c.u8()?, c.u64()?);
        let state = match tag {
            0 => State::Idle,
            1 => State::Receiving(t),
            2 => State::Waiting(t),
            3 => State::Answer(b, t),
            _ => return Err(StateError::Invalid("keyboard state")),
        };
        *self = Self {
            model_a,
            queue,
            state,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(kbd: &mut Keyboard, via: &mut Via, cmd: u8, mut now: u64) -> (u8, u64) {
        via.write(0xb, 0x1c);
        via.write(0xa, cmd);
        kbd.command_started(now);
        now += COMMAND_CLOCKS;
        kbd.run(now, via);
        via.write(0xb, 0x0c);
        via.read(0xa, 0, 0);
        while via.ifr & chips_cbm::via::ifr::SR == 0 {
            now += 1000;
            kbd.run(now, via);
            assert!(now < 10_000_000, "no answer");
        }
        (via.read(0xa, 0, 0), now)
    }

    #[test]
    fn commands_and_keys() {
        let mut via = Via::new();
        let mut kbd = Keyboard::new(true);
        assert_eq!(command(&mut kbd, &mut via, 0x16, 0).0, 0x0b);
        assert_eq!(command(&mut kbd, &mut via, 0x36, 0).0, ACK);
        assert_eq!(command(&mut kbd, &mut via, 0x14, 0).0, NULL);
        // Inquiry times out after a quarter second.
        let (v, t) = command(&mut kbd, &mut via, 0x10, 0);
        assert_eq!(v, NULL);
        assert!(t >= INQUIRY_CLOCKS);
        kbd.key(0x00, true); // A
        kbd.key(0x00, false);
        assert_eq!(command(&mut kbd, &mut via, 0x10, 0).0, 0x01);
        assert_eq!(command(&mut kbd, &mut via, 0x10, 0).0, 0x81);
        kbd.key(0x7b, true); // left arrow
        assert_eq!(command(&mut kbd, &mut via, 0x10, 0).0, 0x79);
        assert_eq!(command(&mut kbd, &mut via, 0x14, 0).0, 0x0d);
        let mut w = StateWriter::new();
        kbd.save(&mut w);
        let b = w.into_bytes();
        let mut k2 = Keyboard::new(false);
        k2.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(k2, kbd);
    }

    #[test]
    fn keypad_codes() {
        assert_eq!(transition(0x52, true, true), [0x79, 0x25]); // pad 0
        assert_eq!(transition(0x59, false, true), [0x79, 0xb3]); // pad 7 up
        assert_eq!(transition(0x45, true, true), [0x71, 0x79, 0x0d]); // pad +
        assert!(transition(0x52, true, false).is_empty());
        assert_eq!(transition(0x24, true, false), [0x49]); // Return
    }
}
