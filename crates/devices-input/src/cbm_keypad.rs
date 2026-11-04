//! Numeric keypads on a Commodore control port, read as joystick lines:
//! each puts a code for the pressed key on pins 1–4 and 6 (the computer
//! reads it as the five low bits of its port register, 1 = high).
//!
//! - Rushware keypad (16 keys): the code on the direction pins with the
//!   fire pin low while a key is down; nothing down: all pins high.
//! - Coplin keypad (12 keys: digits, P, R): a code on all five pins; all
//!   high when nothing is down.
//! - Atari CX85 numeric keypad (17 keys): a code on all five pins; all
//!   high when nothing is down, which the test program cannot tell from
//!   its "-" key (code 31) — the keypad's own strobe line is not modelled.
//!   Its "0" and "Yes" keys share code 28 in the test program's table;
//!   "Yes" gets the unused 24 here.
//! - Cardco Cardkey (16 keys): a four-bit code on the direction pins with
//!   the fire pin high while a key is down and low otherwise.
//!
//! One key at a time: the last one pressed wins until it is released.
//!
//! Input: [`InputEvent::Key`] with `machine_key` from [`key`].
//!
//! Source: the VICE test programs `keypad/*-c64.asm`, whose decode tables
//! and keypad pictures give the code of every key (read as behaviour; the
//! keypads' own documentation was not available).

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// Machine keys of the keypads (the host keymap puts them on the PC's
/// numeric keypad and a few letters).
pub mod key {
    pub const DIGIT0: u16 = 0;
    pub const DOT: u16 = 10;
    pub const ENTER: u16 = 11;
    pub const PLUS: u16 = 12;
    pub const MINUS: u16 = 13;
    pub const MULTIPLY: u16 = 14;
    pub const DIVIDE: u16 = 15;
    pub const HASH: u16 = 16;
    pub const ESCAPE: u16 = 17;
    pub const NO: u16 = 18;
    pub const YES: u16 = 19;
    pub const DELETE: u16 = 20;
    pub const P: u16 = 21;
    pub const R: u16 = 22;
    pub const COUNT: u16 = 23;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CbmKeypadKind {
    Rushware,
    Coplin,
    Cx85,
    Cardkey,
}

impl CbmKeypadKind {
    pub fn name(self) -> &'static str {
        match self {
            CbmKeypadKind::Rushware => "rushware",
            CbmKeypadKind::Coplin => "coplin",
            CbmKeypadKind::Cx85 => "cx85",
            CbmKeypadKind::Cardkey => "cardkey",
        }
    }

    /// The five line bits (bit 0 pin 1 … bit 3 pin 4, bit 4 pin 6; 1 =
    /// high) for a key, or `None` when the keypad has no such key.
    fn code(self, k: u16) -> Option<u8> {
        use key::*;
        let digit = |n: u8| u16::from(n);
        let table: &[(u16, u8)] = match self {
            CbmKeypadKind::Rushware => &[
                (ENTER, 0),
                (DOT, 1),
                (MULTIPLY, 2),
                (DIVIDE, 3),
                (MINUS, 4),
                (PLUS, 5),
                (9, 6),
                (8, 7),
                (7, 8),
                (6, 9),
                (5, 10),
                (4, 11),
                (3, 12),
                (2, 13),
                (1, 14),
                (DIGIT0, 15),
            ],
            CbmKeypadKind::Coplin => &[
                (R, 15),
                (DIGIT0, 17),
                (P, 18),
                (5, 19),
                (3, 21),
                (9, 22),
                (6, 23),
                (1, 25),
                (7, 26),
                (4, 27),
                (2, 29),
                (8, 30),
            ],
            CbmKeypadKind::Cx85 => &[
                (ESCAPE, 12),
                (DELETE, 16),
                (4, 17),
                (5, 18),
                (6, 19),
                (NO, 20),
                (7, 21),
                (8, 22),
                (9, 23),
                (YES, 24),
                (1, 25),
                (2, 26),
                (3, 27),
                (DIGIT0, 28),
                (DOT, 29),
                (ENTER, 30),
                (MINUS, 31),
            ],
            CbmKeypadKind::Cardkey => &[
                (DIGIT0, 0x10),
                (1, 0x11),
                (2, 0x12),
                (3, 0x13),
                (4, 0x14),
                (5, 0x15),
                (6, 0x16),
                (7, 0x17),
                (8, 0x18),
                (9, 0x19),
                (PLUS, 0x1a),
                (MINUS, 0x1b),
                (DIVIDE, 0x1c),
                (MULTIPLY, 0x1d),
                (DOT, 0x1e),
                (ENTER, 0x1f),
            ],
        };
        let _ = digit;
        table.iter().find(|(key, _)| *key == k).map(|(_, c)| *c)
    }

    /// The lines with nothing pressed.
    fn idle(self) -> u8 {
        match self {
            CbmKeypadKind::Cardkey => 0x0f,
            _ => 0x1f,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CbmKeypad {
    kind: CbmKeypadKind,
    /// Keys down, in the order they went down.
    down: Vec<u16>,
}

impl CbmKeypad {
    pub fn new(kind: CbmKeypadKind) -> Self {
        Self {
            kind,
            down: Vec::new(),
        }
    }

    fn lines(&self) -> u8 {
        self.down
            .iter()
            .rev()
            .find_map(|&k| self.kind.code(k))
            .unwrap_or(self.kind.idle())
    }
}

impl PortDevice for CbmKeypad {
    fn kind(&self) -> PortKind {
        PortKind::CbmControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let code = self.lines();
        let mut low = u32::from(!code & 0x0f);
        if code & 0x10 == 0 {
            low |= pin::FIRE;
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::Key { machine_key, down } = *ev {
            let k = machine_key.0;
            if k >= key::COUNT {
                return;
            }
            self.down.retain(|&d| d != k);
            if down {
                self.down.push(k);
            }
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"CKPD", 1, |w| {
            w.u8(self.down.len() as u8);
            for &k in &self.down {
                w.u16(k);
            }
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"CKPD", 1)?;
        let n = c.u8()?;
        let mut down = Vec::new();
        for _ in 0..n {
            let k = c.u16()?;
            if k < key::COUNT {
                down.push(k);
            }
        }
        self.down = down;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    /// The five bits as the C64 reads them from its port register.
    fn port(k: &CbmKeypad) -> u8 {
        let low = k.read_lines(Cycle(0)).pulled_low;
        let dirs = !(low as u8) & 0x0f;
        dirs | if low & pin::FIRE != 0 { 0 } else { 0x10 }
    }

    fn press(k: &mut CbmKeypad, key: u16, down: bool) {
        k.input(&InputEvent::Key {
            machine_key: MachineKey(key),
            down,
        });
    }

    #[test]
    fn codes_as_the_test_programs_expect() {
        let mut r = CbmKeypad::new(CbmKeypadKind::Rushware);
        assert_eq!(port(&r), 0x1f);
        press(&mut r, 7, true);
        assert_eq!(port(&r), 8);
        press(&mut r, key::ENTER, true);
        assert_eq!(port(&r), 0, "the last key pressed shows");
        press(&mut r, key::ENTER, false);
        assert_eq!(port(&r), 8);
        let mut c = CbmKeypad::new(CbmKeypadKind::Coplin);
        press(&mut c, key::P, true);
        assert_eq!(port(&c), 18);
        let mut x = CbmKeypad::new(CbmKeypadKind::Cx85);
        press(&mut x, key::DOT, true);
        assert_eq!(port(&x), 29);
        let mut k = CbmKeypad::new(CbmKeypadKind::Cardkey);
        assert_eq!(port(&k), 0x0f);
        press(&mut k, key::MULTIPLY, true);
        assert_eq!(port(&k), 0x1d);
    }

    #[test]
    fn savestate_round_trip() {
        let mut k = CbmKeypad::new(CbmKeypadKind::Cx85);
        press(&mut k, 5, true);
        press(&mut k, key::NO, true);
        let mut n = CbmKeypad::new(CbmKeypadKind::Cx85);
        n.load_state(&k.save_state()).unwrap();
        assert_eq!(k, n);
    }
}
