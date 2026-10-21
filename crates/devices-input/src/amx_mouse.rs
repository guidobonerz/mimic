//! AMX mouse on the CPC's joystick port: moving it closes the direction
//! lines the way a joystick would, so software counts steps by polling
//! the joystick bits; its three buttons are the fire lines (left = fire 2
//! on pin 6, the main joystick button; right = fire 1 on pin 7; middle =
//! fire 3 on pin 5).
//!
//! Each unit of host mouse motion holds its direction line low for
//! `cycles_per_step` machine cycles; motion arriving while a line is held
//! extends it. The device learns the time from [`PortDevice::write_lines`],
//! which the machine calls before reading the lines.

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// Direction lines in the order right, left, down, up.
const LINES: [u32; 4] = [pin::RIGHT, pin::LEFT, pin::DOWN, pin::UP];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmxMouse {
    cycles_per_step: u64,
    /// Motion not yet turned into line time (x right, y down).
    pending: [i32; 2],
    /// Each direction line is held low until this cycle.
    until: [u64; 4],
    buttons: u8,
    now: u64,
}

impl AmxMouse {
    pub fn new(cycles_per_step: u64) -> Self {
        Self {
            cycles_per_step: cycles_per_step.max(1),
            pending: [0; 2],
            until: [0; 4],
            buttons: 0,
            now: 0,
        }
    }

    fn settle(&mut self) {
        for (axis, &d) in self.pending.iter().enumerate() {
            if d == 0 {
                continue;
            }
            // Axis 0: right (0) / left (1); axis 1: down (2) / up (3).
            let line = 2 * axis + usize::from(d < 0);
            let start = self.until[line].max(self.now);
            self.until[line] = start + u64::from(d.unsigned_abs()) * self.cycles_per_step;
        }
        self.pending = [0; 2];
    }
}

impl PortDevice for AmxMouse {
    fn kind(&self) -> PortKind {
        PortKind::CpcJoy
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut low = 0;
        for (k, &bit) in LINES.iter().enumerate() {
            if now.0 < self.until[k] {
                low |= bit;
            }
        }
        for (mask, bit) in [(1, pin::FIRE), (2, pin::CPC_FIRE1), (4, pin::BUTTON3)] {
            if self.buttons & mask != 0 {
                low |= bit;
            }
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, now: Cycle) {
        self.now = now.0;
        self.settle();
    }

    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::MouseRel { dx, dy, buttons } = *ev {
            self.pending[0] += i32::from(dx);
            self.pending[1] += i32::from(dy);
            self.buttons = buttons & 7;
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"AMXM", 1, |w| {
            for p in self.pending {
                w.u32(p as u32);
            }
            for u in self.until {
                w.u64(u);
            }
            w.u8(self.buttons);
            w.u64(self.now);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"AMXM", 1)?;
        let mut n = *self;
        for p in n.pending.iter_mut() {
            *p = c.u32()? as i32;
        }
        for u in n.until.iter_mut() {
            *u = c.u64()?;
        }
        n.buttons = c.u8()? & 7;
        n.now = c.u64()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_holds_direction_lines() {
        let mut m = AmxMouse::new(100);
        m.input(&InputEvent::MouseRel {
            dx: 3,
            dy: -1,
            buttons: 1,
        });
        m.write_lines(PortLines::default(), Cycle(1000));
        let low = m.read_lines(Cycle(1050)).pulled_low;
        assert_eq!(low, pin::RIGHT | pin::UP | pin::FIRE);
        assert_eq!(m.read_lines(Cycle(1200)).pulled_low, pin::RIGHT | pin::FIRE);
        assert_eq!(m.read_lines(Cycle(1300)).pulled_low, pin::FIRE);
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = AmxMouse::new(100);
        m.input(&InputEvent::MouseRel {
            dx: -5,
            dy: 2,
            buttons: 6,
        });
        m.write_lines(PortLines::default(), Cycle(7));
        let s = m.save_state();
        let mut n = AmxMouse::new(100);
        n.load_state(&s).unwrap();
        assert_eq!(m, n);
    }
}
