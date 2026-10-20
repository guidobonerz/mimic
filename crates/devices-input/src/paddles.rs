//! A pair of paddles on one CBM control port: two 470 kΩ potentiometers on
//! POTX (pin 9) and POTY (pin 5) and their fire buttons on pins 3 (paddle
//! X) and 4 (paddle Y), the joystick's left and right lines. Source:
//! Commodore 64 Programmer's Reference Guide, appendix I.
//!
//! Input: [`InputEvent::Analog`] sets a paddle's position (axis 0 = X,
//! 1 = Y; 0 = turned fully one way, 65535 the other), [`InputEvent::Digital`]
//! its buttons (bit 0 = X, bit 1 = Y), and a host mouse turns paddle X
//! ([`InputEvent::MouseRel`]: horizontal motion, left/right button).

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// Paddle travel per mouse count.
const MOUSE_STEP: i32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paddles {
    /// Resistance of each pot, 0–65535 (the SID counts it as 0–255).
    pub position: [u16; 2],
    buttons: u8,
}

impl Default for Paddles {
    fn default() -> Self {
        Self::new()
    }
}

impl Paddles {
    pub fn new() -> Self {
        Self {
            position: [0x8000; 2],
            buttons: 0,
        }
    }
}

impl PortDevice for Paddles {
    fn kind(&self) -> PortKind {
        PortKind::CbmControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        if self.buttons & 1 != 0 {
            low |= pin::LEFT;
        }
        if self.buttons & 2 != 0 {
            low |= pin::RIGHT;
        }
        PortLines {
            pulled_low: low,
            analog: self.position,
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Analog { axis, value } if axis < 2 => {
                self.position[usize::from(axis)] = value;
            }
            InputEvent::Digital { buttons, .. } => self.buttons = buttons & 3,
            InputEvent::MouseRel { dx, buttons, .. } => {
                let p = i32::from(self.position[0]) + i32::from(dx) * MOUSE_STEP;
                self.position[0] = p.clamp(0, 0xffff) as u16;
                self.buttons = buttons & 3;
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"PADL", 1, |w| {
            w.u16(self.position[0]);
            w.u16(self.position[1]);
            w.u8(self.buttons);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"PADL", 1)?;
        let position = [c.u16()?, c.u16()?];
        let buttons = c.u8()? & 3;
        self.position = position;
        self.buttons = buttons;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_buttons_and_mouse() {
        let mut p = Paddles::new();
        p.input(&InputEvent::Analog {
            axis: 1,
            value: 0x1234,
        });
        p.input(&InputEvent::Digital {
            dirs: emu_core::input::Dirs::NONE,
            buttons: 2,
        });
        let l = p.read_lines(Cycle(0));
        assert_eq!(l.analog, [0x8000, 0x1234]);
        assert_eq!(l.pulled_low, pin::RIGHT);
        p.input(&InputEvent::MouseRel {
            dx: -20,
            dy: 5,
            buttons: 1,
        });
        let l = p.read_lines(Cycle(0));
        assert_eq!(l.analog[0], 0x8000 - 20 * 256);
        assert_eq!(l.pulled_low, pin::LEFT);
        p.input(&InputEvent::MouseRel {
            dx: -1000,
            dy: 0,
            buttons: 0,
        });
        assert_eq!(p.read_lines(Cycle(0)).analog[0], 0, "end stop");
        let s = p.save_state();
        let mut q = Paddles::new();
        q.load_state(&s).unwrap();
        assert_eq!(p, q);
        assert!(q.load_state(&s[..3]).is_err());
    }
}
