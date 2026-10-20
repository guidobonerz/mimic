//! Commodore 1351 mouse in proportional mode.
//!
//! The mouse watches the SID's pot measurement and charges POTX/POTY so
//! that the count lands on its position: bits 1–6 of the POT registers hold
//! the X and Y position modulo 64 (bit 0 is noise, bit 7 unused); software
//! takes the difference between two reads. Moving the mouse up increases Y.
//! The left button pulls pin 6 (fire), the right button pin 1 (up). Source:
//! Commodore 1351 Mouse User's Manual (proportional mode, POTX/POTY).
//!
//! Here the position is handed to the machine as the resistance that makes
//! the SID count exactly `position << 1` (bit 0 noise-free: deterministic).

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mouse1351 {
    /// Position counters (only the low 6 bits reach the computer).
    pub x: u16,
    pub y: u16,
    buttons: u8,
}

impl Mouse1351 {
    pub fn new() -> Self {
        Self::default()
    }

    fn pot(v: u16) -> u16 {
        ((v & 63) << 1) << 8
    }
}

impl PortDevice for Mouse1351 {
    fn kind(&self) -> PortKind {
        PortKind::CbmControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        if self.buttons & 1 != 0 {
            low |= pin::FIRE;
        }
        if self.buttons & 2 != 0 {
            low |= pin::UP;
        }
        PortLines {
            pulled_low: low,
            analog: [Self::pot(self.x), Self::pot(self.y)],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    /// Host mouse motion: `dx` right, `dy` down (screen convention).
    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::MouseRel { dx, dy, buttons } = *ev {
            self.x = self.x.wrapping_add_signed(dx);
            self.y = self.y.wrapping_add_signed(dy.wrapping_neg());
            self.buttons = buttons & 3;
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"M351", 1, |w| {
            w.u16(self.x);
            w.u16(self.y);
            w.u8(self.buttons);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"M351", 1)?;
        let (x, y, buttons) = (c.u16()?, c.u16()?, c.u8()? & 3);
        *self = Self { x, y, buttons };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_in_pot_bits_1_to_6() {
        let mut m = Mouse1351::new();
        m.input(&InputEvent::MouseRel {
            dx: 70,
            dy: -3,
            buttons: 3,
        });
        let l = m.read_lines(Cycle(0));
        // X 70 mod 64 = 6, Y +3 (moved up): SID counts 12 and 6.
        assert_eq!(l.analog.map(|a| a >> 8), [12, 6]);
        assert_eq!(l.pulled_low, pin::FIRE | pin::UP);
        m.input(&InputEvent::MouseRel {
            dx: -71,
            dy: 4,
            buttons: 0,
        });
        // X -1 → 63, Y -1 → 63.
        assert_eq!(m.read_lines(Cycle(0)).analog.map(|a| a >> 8), [126, 126]);
        let s = m.save_state();
        let mut n = Mouse1351::new();
        n.load_state(&s).unwrap();
        assert_eq!(m, n);
    }
}
