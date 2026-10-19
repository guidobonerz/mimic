//! Digital joystick (Atari-style 9-pin): four direction switches and fire
//! buttons that pull their pins to ground.
//!
//! Pinout shared by the C64/VIC-20/C128 control ports, Atari 8-bit and
//! Amiga ports (Commodore 64 Programmer's Reference Guide, appendix I):
//! pin 1 up, 2 down, 3 left, 4 right, 6 fire. Amiga/Atari second buttons on
//! pin 9 (and pin 5 for a third) are reported for those machines.

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

/// Pin bits in [`PortLines::pulled_low`] (bit n = pin n + 1).
pub mod pin {
    pub const UP: u32 = 1 << 0;
    pub const DOWN: u32 = 1 << 1;
    pub const LEFT: u32 = 1 << 2;
    pub const RIGHT: u32 = 1 << 3;
    pub const BUTTON3: u32 = 1 << 4;
    pub const FIRE: u32 = 1 << 5;
    pub const BUTTON2: u32 = 1 << 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Joystick {
    kind: PortKind,
    dirs: Dirs,
    buttons: u8,
}

impl Joystick {
    /// A joystick for a port of `kind` (CbmControl, AtariJoy, AmigaGameport…).
    pub fn new(kind: PortKind) -> Self {
        Self {
            kind,
            dirs: Dirs::NONE,
            buttons: 0,
        }
    }

    fn lines(&self) -> u32 {
        let mut low = 0;
        // A real stick cannot close opposite switches; gamepads and keys can.
        // Keep both pulled, as the hardware wiring would.
        for (dir, bit) in [
            (Dirs::UP, pin::UP),
            (Dirs::DOWN, pin::DOWN),
            (Dirs::LEFT, pin::LEFT),
            (Dirs::RIGHT, pin::RIGHT),
        ] {
            if self.dirs.contains(dir) {
                low |= bit;
            }
        }
        for (mask, bit) in [(1, pin::FIRE), (2, pin::BUTTON2), (4, pin::BUTTON3)] {
            if self.buttons & mask != 0 {
                low |= bit;
            }
        }
        low
    }
}

impl PortDevice for Joystick {
    fn kind(&self) -> PortKind {
        self.kind
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        PortLines {
            pulled_low: self.lines(),
            analog: [0; 2],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        if let InputEvent::Digital { dirs, buttons } = *ev {
            self.dirs = dirs;
            self.buttons = buttons;
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"JOYS", 1, |w| {
            w.u8(self.dirs.0);
            w.u8(self.buttons);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"JOYS", 1)?;
        let dirs = Dirs(c.u8()? & 0x0f);
        let buttons = c.u8()? & 0x07;
        self.dirs = dirs;
        self.buttons = buttons;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_and_fire_pull_their_pins() {
        let mut j = Joystick::new(PortKind::CbmControl);
        assert_eq!(j.read_lines(Cycle(0)).pulled_low, 0);
        j.input(&InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        });
        assert_eq!(
            j.read_lines(Cycle(0)).pulled_low,
            pin::UP | pin::RIGHT | pin::FIRE
        );
        j.input(&InputEvent::Key {
            machine_key: emu_core::input::MachineKey(0),
            down: true,
        });
        assert_ne!(j.read_lines(Cycle(0)).pulled_low, 0, "key events ignored");
    }

    #[test]
    fn savestate_round_trip() {
        let mut j = Joystick::new(PortKind::CbmControl);
        j.input(&InputEvent::Digital {
            dirs: Dirs::LEFT,
            buttons: 1,
        });
        let s = j.save_state();
        let mut k = Joystick::new(PortKind::CbmControl);
        k.load_state(&s).unwrap();
        assert_eq!(j, k);
        assert!(k.load_state(&s[..s.len() - 1]).is_err());
        assert_eq!(j, k);
    }
}
