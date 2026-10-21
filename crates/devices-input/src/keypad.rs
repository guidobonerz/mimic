//! Atari keypad (CX50 "Keyboard Controller", also sold as Kid's
//! Controller and Video Touch Pad): 12 keys in 4 rows of 3.
//!
//! The computer drives the rows on the direction pins (pin 1 = row 1 with
//! keys 1 2 3, pin 2 = 4 5 6, pin 3 = 7 8 9, pin 4 = * 0 #), one at a time
//! low. A pressed key in a low row pulls its column low: column 1 on pin 5,
//! column 2 on pin 9, column 3 on pin 6 (the fire line). The pot pins are
//! pulled up inside the keypad, so they read high at once otherwise.
//! On the Atari 2600 the columns are INPT0/INPT1/INPT4 (left port) and
//! INPT2/INPT3/INPT5 (right port), the rows SWCHA D4–D7 / D0–D3.
//!
//! Input: [`InputEvent::Key`] with `machine_key` 0–11 for the keys in
//! reading order (1 2 3 4 5 6 7 8 9 * 0 #).
//!
//! Source: "Atari 2600 Specifications"
//! (<https://www.astro.physik.uni-potsdam.de/~mwendt/2600scene/specs.html>),
//! "Keyboard Controllers".

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// Column lines, left to right.
const COLUMNS: [u32; 3] = [pin::BUTTON3, pin::BUTTON2, pin::FIRE];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keypad {
    /// Pressed keys, bit n = key n (reading order).
    keys: u16,
    /// Rows the computer drives low (bit 0 = row 1).
    rows: u8,
}

impl Keypad {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PortDevice for Keypad {
    fn kind(&self) -> PortKind {
        PortKind::AtariJoy
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        for row in 0..4 {
            if self.rows >> row & 1 == 0 {
                continue;
            }
            for (col, &line) in COLUMNS.iter().enumerate() {
                if self.keys >> (row * 3 + col) & 1 != 0 {
                    low |= line;
                }
            }
        }
        PortLines {
            pulled_low: low,
            analog: [0; 2],
        }
    }

    fn write_lines(&mut self, lines: PortLines, _now: Cycle) {
        self.rows = (lines.pulled_low & 0x0f) as u8;
    }

    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::Key { machine_key, down } = *ev {
            if machine_key.0 < 12 {
                let bit = 1 << machine_key.0;
                if down {
                    self.keys |= bit;
                } else {
                    self.keys &= !bit;
                }
            }
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"KPAD", 1, |w| {
            w.u16(self.keys);
            w.u8(self.rows);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"KPAD", 1)?;
        let keys = c.u16()? & 0x0fff;
        let rows = c.u8()? & 0x0f;
        self.keys = keys;
        self.rows = rows;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    fn press(k: &mut Keypad, key: u16, down: bool) {
        k.input(&InputEvent::Key {
            machine_key: MachineKey(key),
            down,
        });
    }

    fn rows(k: &mut Keypad, low: u32) -> u32 {
        k.write_lines(
            PortLines {
                pulled_low: low,
                analog: [u16::MAX; 2],
            },
            Cycle(0),
        );
        k.read_lines(Cycle(0)).pulled_low
    }

    #[test]
    fn a_key_pulls_its_column_in_its_row_only() {
        let mut k = Keypad::new();
        press(&mut k, 4, true); // "5": row 2, column 2
        assert_eq!(rows(&mut k, pin::UP), 0, "row 1 selected");
        assert_eq!(rows(&mut k, pin::DOWN), pin::BUTTON2);
        press(&mut k, 11, true); // "#": row 4, column 3
        assert_eq!(rows(&mut k, pin::RIGHT), pin::FIRE);
        assert_eq!(rows(&mut k, 0x0f), pin::BUTTON2 | pin::FIRE);
        press(&mut k, 4, false);
        assert_eq!(rows(&mut k, pin::DOWN), 0);
        assert_eq!(k.read_lines(Cycle(0)).analog, [0, 0], "columns pulled up");
    }

    #[test]
    fn savestate_round_trip() {
        let mut k = Keypad::new();
        press(&mut k, 9, true);
        rows(&mut k, pin::RIGHT);
        let mut back = Keypad::new();
        back.load_state(&k.save_state()).unwrap();
        assert_eq!(back, k);
        assert!(back.load_state(b"junk").is_err());
    }
}
