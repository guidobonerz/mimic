//! Apple II game I/O devices: the analog joystick and a pair of paddles.
//!
//! **Lines.** The game I/O connector carries four paddle inputs (PDL0–3,
//! each a 150 kΩ potentiometer timing one of the 558's monostables), three
//! push-button inputs (PB0–2) and four annunciator outputs (Apple II
//! Reference Manual, "The Game I/O Connector"; Apple IIe Technical
//! Reference Manual, chapter 7). A joystick and a pair of paddles both use
//! PDL0/PDL1 and PB0/PB1: [`PortLines::analog`] carries PDL0 and PDL1 (0 =
//! no resistance, the left or upper end; $FFFF = 150 kΩ), `pulled_low`
//! bits 0 and 1 mean buttons 0 and 1 pressed (the buttons pull their
//! inputs high on the real connector; the bit says "pressed").
//!
//! Input: joystick — [`InputEvent::Digital`] (the stick at full
//! deflection, buttons 0 and 1 from bits 0 and 1), [`InputEvent::Analog`]
//! axis 0 X / axis 1 Y (0–65535, $8000 centre; Y up, as gamepads send it:
//! the Apple's PDL1 grows downwards), [`InputEvent::Key`]
//! `machine_key` 0–1 (buttons) and 4–7 (stick up, down, left, right).
//! Paddles — [`InputEvent::Analog`] axis 0/1 for paddle 0/1,
//! [`InputEvent::MouseRel`] turns paddle 0 (dx) and its button,
//! [`InputEvent::Digital`] buttons.

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

/// Centre of a joystick axis.
pub const CENTRE: u16 = 0x8000;
/// Paddle steps per mouse count.
const MOUSE_STEP: i32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleKind {
    Joystick,
    Paddles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleJoystick {
    kind: AppleKind,
    pad: u8,
    keys: u8,
    dirs: u8,
    key_dirs: u8,
    pos: [u16; 2],
}

impl AppleJoystick {
    pub fn new(kind: AppleKind) -> Self {
        Self {
            kind,
            pad: 0,
            keys: 0,
            dirs: 0,
            key_dirs: 0,
            pos: [CENTRE; 2],
        }
    }

    pub fn kind(&self) -> AppleKind {
        self.kind
    }

    fn axes(&self) -> [u16; 2] {
        if self.kind == AppleKind::Paddles {
            return self.pos;
        }
        let d = Dirs(self.dirs | self.key_dirs);
        let axis = |neg: Dirs, pos: Dirs, v: u16| match (d.contains(neg), d.contains(pos)) {
            (true, false) => 0,
            (false, true) => 0xffff,
            _ => v,
        };
        [
            axis(Dirs::LEFT, Dirs::RIGHT, self.pos[0]),
            axis(Dirs::UP, Dirs::DOWN, self.pos[1]),
        ]
    }
}

impl PortDevice for AppleJoystick {
    fn kind(&self) -> PortKind {
        PortKind::AppleGame
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        PortLines {
            pulled_low: u32::from((self.pad | self.keys) & 3),
            analog: self.axes(),
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        match ev {
            InputEvent::Digital { dirs, buttons } => {
                self.dirs = dirs.0 & 0x0f;
                self.pad = buttons & 3;
            }
            InputEvent::Analog { axis: 0, value } => self.pos[0] = *value,
            InputEvent::Analog { axis: 1, value } => {
                self.pos[1] = if self.kind == AppleKind::Joystick {
                    0xffff - *value
                } else {
                    *value
                };
            }
            InputEvent::MouseRel { dx, buttons, .. } if self.kind == AppleKind::Paddles => {
                let v = i32::from(self.pos[0]) + i32::from(*dx) * MOUSE_STEP;
                self.pos[0] = v.clamp(0, 0xffff) as u16;
                self.pad = buttons & 1;
            }
            InputEvent::Key { machine_key, down } => {
                let k = machine_key.0;
                let (set, bit) = match k {
                    0 | 1 => (&mut self.keys, 1 << k),
                    4 => (&mut self.key_dirs, Dirs::UP.0),
                    5 => (&mut self.key_dirs, Dirs::DOWN.0),
                    6 => (&mut self.key_dirs, Dirs::LEFT.0),
                    7 => (&mut self.key_dirs, Dirs::RIGHT.0),
                    _ => return,
                };
                if *down {
                    *set |= bit;
                } else {
                    *set &= !bit;
                }
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"A2JY", 1, |w| {
            w.u8(match self.kind {
                AppleKind::Joystick => 0,
                AppleKind::Paddles => 1,
            });
            for v in [self.pad, self.keys, self.dirs, self.key_dirs] {
                w.u8(v);
            }
            w.u16(self.pos[0]);
            w.u16(self.pos[1]);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"A2JY", 1)?;
        let kind = c.u8()?;
        if kind != u8::from(self.kind == AppleKind::Paddles) {
            return Err(StateError::Invalid("apple game device differs"));
        }
        let mut n = self.clone();
        n.pad = c.u8()? & 3;
        n.keys = c.u8()? & 3;
        n.dirs = c.u8()? & 0x0f;
        n.key_dirs = c.u8()? & 0x0f;
        n.pos = [c.u16()?, c.u16()?];
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    #[test]
    fn joystick_lines() {
        let mut j = AppleJoystick::new(AppleKind::Joystick);
        assert_eq!(j.read_lines(Cycle(0)).analog, [CENTRE; 2]);
        j.input(&InputEvent::Digital {
            dirs: Dirs(Dirs::LEFT.0 | Dirs::DOWN.0),
            buttons: 2,
        });
        let l = j.read_lines(Cycle(0));
        assert_eq!(l.analog, [0, 0xffff]);
        assert_eq!(l.pulled_low, 2);
        j.input(&InputEvent::Key {
            machine_key: MachineKey(0),
            down: true,
        });
        assert_eq!(j.read_lines(Cycle(0)).pulled_low, 3);
        let s = j.save_state();
        let mut k = AppleJoystick::new(AppleKind::Joystick);
        k.load_state(&s).unwrap();
        assert_eq!(k, j);
        assert!(AppleJoystick::new(AppleKind::Paddles)
            .load_state(&s)
            .is_err());
    }

    #[test]
    fn mouse_turns_paddle_0() {
        let mut p = AppleJoystick::new(AppleKind::Paddles);
        p.input(&InputEvent::MouseRel {
            dx: 10,
            dy: 0,
            buttons: 1,
        });
        let l = p.read_lines(Cycle(0));
        assert_eq!(l.analog[0], CENTRE + 2560);
        assert_eq!(l.pulled_low, 1);
    }
}
