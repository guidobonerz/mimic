//! The A500 keyboard: its own controller sends raw key codes serially to
//! CIA-A (KCLK on CNT, KDAT on SP) and waits for the computer's handshake
//! (HRM chapter 8, "Keyboard"; raw key codes in the HRM's keyboard
//! appendix).
//!
//! - A key code is 7 bits; bit 7 is 1 for a release. The keyboard sends
//!   bits 6–0, then bit 7, MSB first, active low (KDAT low = 1): the CPU
//!   reads SDR, inverts it and rotates it right by one.
//! - Each bit: KDAT set, KCLK low for about 20 µs, then high for about
//!   40 µs; the CIA shifts KDAT in on the rising edge.
//! - Handshake: the CPU switches CIA-A's serial port to output (CRA bit 6)
//!   for at least 85 µs, pulling KDAT low. Only then comes the next code.
//!   Without a handshake for 143 ms the keyboard sends the next code anyway
//!   (the real one resynchronises first; not modelled).
//! - At power-on: $FD (initiate power-up key stream), $FE (terminate).
//! - Caps Lock is a toggle: a press sends "down" or "up" alternately;
//!   its release sends nothing.
//! - Ctrl + both Amiga keys: reset (reported to the machine).
//!
//! Timing runs on the E clock (one [`Keyboard::tick`] per E cycle).

use std::collections::VecDeque;

use chips_cbm::cia::Cia;
use emu_core::{StateError, StateReader, StateWriter};

/// Raw key codes the machine reacts to.
pub mod code {
    pub const CAPS_LOCK: u8 = 0x62;
    pub const CTRL: u8 = 0x63;
    pub const LEFT_AMIGA: u8 = 0x66;
    pub const RIGHT_AMIGA: u8 = 0x67;
    pub const POWER_UP_START: u8 = 0xfd;
    pub const POWER_UP_END: u8 = 0xfe;
}

/// E-clock cycles (1.41 µs) of one bit phase: KDAT setup, KCLK low, high.
const SETUP: u16 = 4;
const CLK_LOW: u16 = 14;
const CLK_HIGH: u16 = 28;
/// Handshake timeout: 143 ms.
const TIMEOUT: u32 = 101_400;
/// Power-up stream after about 200 ms.
const POWER_UP_DELAY: u32 = 140_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Bit `bit` (7 … 0) of `byte`, `t` E cycles into it.
    Sending {
        byte: u8,
        bit: u8,
        t: u16,
    },
    /// Sent; waiting for the handshake (`t` E cycles so far, `seen`: the
    /// serial port is in output mode now).
    Handshake {
        t: u32,
        seen: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyboard {
    queue: VecDeque<u8>,
    state: State,
    caps: bool,
    held: [bool; 128],
    power_up: u32,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Keyboard {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            state: State::Idle,
            caps: false,
            held: [false; 128],
            power_up: POWER_UP_DELAY,
        }
    }

    /// A key went down or up. Returns true for Ctrl + Amiga + Amiga (reset).
    pub fn key(&mut self, raw: u8, down: bool) -> bool {
        let raw = raw & 0x7f;
        if raw == code::CAPS_LOCK {
            if down {
                self.caps = !self.caps;
                self.queue.push_back(raw | if self.caps { 0 } else { 0x80 });
            }
            return false;
        }
        if self.held[usize::from(raw)] == down {
            return false;
        }
        self.held[usize::from(raw)] = down;
        self.queue.push_back(raw | if down { 0 } else { 0x80 });
        down && [code::CTRL, code::LEFT_AMIGA, code::RIGHT_AMIGA]
            .iter()
            .all(|&k| self.held[usize::from(k)])
    }

    /// Keys still to send (tests, typing helpers).
    pub fn pending(&self) -> usize {
        self.queue.len() + usize::from(!matches!(self.state, State::Idle))
    }

    /// One E-clock cycle.
    pub fn tick(&mut self, cia: &mut Cia) {
        if self.power_up > 0 {
            self.power_up -= 1;
            if self.power_up == 0 {
                self.queue.push_front(code::POWER_UP_END);
                self.queue.push_front(code::POWER_UP_START);
            }
        }
        self.state = match self.state {
            State::Idle => match self.queue.pop_front() {
                Some(c) => State::Sending {
                    byte: c.rotate_left(1),
                    bit: 7,
                    t: 0,
                },
                None => State::Idle,
            },
            State::Sending { byte, bit, t } => {
                if t == 0 {
                    cia.set_sp(byte & (1 << bit) == 0);
                } else if t == SETUP {
                    cia.set_cnt(false);
                } else if t == SETUP + CLK_LOW {
                    cia.set_cnt(true);
                }
                if t + 1 < SETUP + CLK_LOW + CLK_HIGH {
                    State::Sending {
                        byte,
                        bit,
                        t: t + 1,
                    }
                } else if bit > 0 {
                    State::Sending {
                        byte,
                        bit: bit - 1,
                        t: 0,
                    }
                } else {
                    State::Handshake { t: 0, seen: false }
                }
            }
            State::Handshake { t, seen } => {
                let output = cia.ta.cr & 0x40 != 0;
                if (seen && !output) || t >= TIMEOUT {
                    cia.set_sp(true);
                    State::Idle
                } else {
                    State::Handshake {
                        t: t + 1,
                        seen: seen || output,
                    }
                }
            }
        };
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"AKBD", 1, |w| {
            w.bytes(&self.queue.iter().copied().collect::<Vec<u8>>());
            let (kind, a, b, c) = match self.state {
                State::Idle => (0, 0, 0, 0),
                State::Sending { byte, bit, t } => {
                    (1, u32::from(byte), u32::from(bit), u32::from(t))
                }
                State::Handshake { t, seen } => (2, t, u32::from(seen), 0),
            };
            w.u8(kind);
            for v in [a, b, c] {
                w.u32(v);
            }
            w.bool(self.caps);
            let held: Vec<u8> = self.held.iter().map(|&h| u8::from(h)).collect();
            w.raw(&held);
            w.u32(self.power_up);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"AKBD", 1)?;
        let mut n = Self::new();
        n.queue = c.bytes()?.iter().copied().collect();
        let kind = c.u8()?;
        let (a, b, t) = (c.u32()?, c.u32()?, c.u32()?);
        n.state = match kind {
            0 => State::Idle,
            1 if b < 8 && t < 0x1_0000 => State::Sending {
                byte: a as u8,
                bit: b as u8,
                t: t as u16,
            },
            2 => State::Handshake { t: a, seen: b != 0 },
            _ => return Err(StateError::Invalid("keyboard state")),
        };
        n.caps = c.bool()?;
        let mut held = [0u8; 128];
        c.raw_into(&mut held)?;
        for (h, v) in n.held.iter_mut().zip(held) {
            *h = v != 0;
        }
        n.power_up = c.u32()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chips_cbm::cia::{icr, CiaModel};

    /// Run the keyboard and the CIA until SDR holds a byte; return what
    /// the CPU decodes from it.
    fn receive(kb: &mut Keyboard, cia: &mut Cia) -> Option<(u8, bool)> {
        for _ in 0..2000 {
            kb.tick(cia);
            cia.tick();
            if cia.peek(0x0d, 0xff, 0xff) & icr::SP != 0 {
                cia.read(0x0d, 0xff, 0xff);
                let sdr = !cia.read(0x0c, 0xff, 0xff);
                let v = sdr.rotate_right(1);
                return Some((v & 0x7f, v & 0x80 != 0));
            }
        }
        None
    }

    fn handshake(kb: &mut Keyboard, cia: &mut Cia) {
        cia.write(0x0e, 0x40);
        for _ in 0..70 {
            kb.tick(cia);
            cia.tick();
        }
        cia.write(0x0e, 0x00);
    }

    #[test]
    fn sends_codes_serially_and_waits_for_the_handshake() {
        let mut kb = Keyboard::new();
        kb.power_up = 0;
        let mut cia = Cia::new(CiaModel::Mos8520);
        kb.key(0x20, true); // A
        kb.key(0x20, false);
        assert_eq!(receive(&mut kb, &mut cia), Some((0x20, false)));
        assert_eq!(receive(&mut kb, &mut cia), None, "no handshake yet");
        handshake(&mut kb, &mut cia);
        assert_eq!(receive(&mut kb, &mut cia), Some((0x20, true)), "release");
    }

    #[test]
    fn power_up_stream_and_caps_lock_toggle() {
        let mut kb = Keyboard::new();
        kb.power_up = 1;
        let mut cia = Cia::new(CiaModel::Mos8520);
        assert_eq!(receive(&mut kb, &mut cia), Some((0x7d, true)), "$FD");
        handshake(&mut kb, &mut cia);
        assert_eq!(receive(&mut kb, &mut cia), Some((0x7e, true)), "$FE");
        handshake(&mut kb, &mut cia);
        kb.key(code::CAPS_LOCK, true);
        kb.key(code::CAPS_LOCK, false);
        kb.key(code::CAPS_LOCK, true);
        assert_eq!(receive(&mut kb, &mut cia), Some((code::CAPS_LOCK, false)));
        handshake(&mut kb, &mut cia);
        assert_eq!(receive(&mut kb, &mut cia), Some((code::CAPS_LOCK, true)));
    }

    #[test]
    fn ctrl_amiga_amiga_resets() {
        let mut kb = Keyboard::new();
        assert!(!kb.key(code::CTRL, true));
        assert!(!kb.key(code::LEFT_AMIGA, true));
        assert!(kb.key(code::RIGHT_AMIGA, true));
    }

    #[test]
    fn savestate_round_trip() {
        let mut kb = Keyboard::new();
        let mut cia = Cia::new(CiaModel::Mos8520);
        kb.key(0x10, true);
        kb.key(0x11, true);
        for _ in 0..100 {
            kb.tick(&mut cia);
        }
        let mut w = StateWriter::new();
        kb.save(&mut w);
        let data = w.into_bytes();
        let mut n = Keyboard::new();
        n.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(n, kb);
    }
}
