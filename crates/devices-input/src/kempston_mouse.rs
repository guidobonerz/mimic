//! Kempston mouse for the ZX Spectrum: the interface holds two 8-bit
//! position counters that the mouse's quadrature signals count up and
//! down, and the buttons. The computer reads X at port `$FBDF`, Y at
//! `$FFDF` (counting up when the mouse moves up) and the buttons at
//! `$FADF` (bit 0 right, bit 1 left, 0 = pressed; the other bits high).
//!
//! In [`PortLines`] the device reports X in `analog[0]`, Y in `analog[1]`
//! (low 8 bits) and the pressed buttons as pins: bit 0 left, bit 1 right.
//!
//! Source: Kempston Mouse Interface manual (ports and bits); World of
//! Spectrum FAQ "Kempston mouse".

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

/// Pin bits of the buttons in [`PortLines::pulled_low`].
pub const LEFT: u32 = 1;
pub const RIGHT: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KempstonMouse {
    x: u8,
    y: u8,
    buttons: u8,
}

impl KempstonMouse {
    pub fn new() -> Self {
        Self::default()
    }
}

impl PortDevice for KempstonMouse {
    fn kind(&self) -> PortKind {
        PortKind::KempstonMouse
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        if self.buttons & 1 != 0 {
            low |= LEFT;
        }
        if self.buttons & 2 != 0 {
            low |= RIGHT;
        }
        PortLines {
            pulled_low: low,
            analog: [u16::from(self.x), u16::from(self.y)],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    /// Host motion: `dx` right, `dy` down; the Y counter counts up for
    /// motion up.
    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::MouseRel { dx, dy, buttons } = *ev {
            self.x = self.x.wrapping_add(dx as u8);
            self.y = self.y.wrapping_sub(dy as u8);
            self.buttons = buttons;
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"KMOU", 1, |w| {
            w.u8(self.x);
            w.u8(self.y);
            w.u8(self.buttons);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"KMOU", 1)?;
        let (x, y, buttons) = (c.u8()?, c.u8()?, c.u8()?);
        *self = Self { x, y, buttons };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_wrap_and_y_counts_up() {
        let mut m = KempstonMouse::new();
        m.input(&InputEvent::MouseRel {
            dx: -3,
            dy: -2,
            buttons: 1,
        });
        let l = m.read_lines(Cycle(0));
        assert_eq!(l.analog, [253, 2]);
        assert_eq!(l.pulled_low, LEFT);
        let s = m.save_state();
        let mut n = KempstonMouse::new();
        n.load_state(&s).unwrap();
        assert_eq!(n, m);
    }
}
