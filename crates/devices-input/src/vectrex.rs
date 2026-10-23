//! Vectrex controller-port devices: the standard controller (an analog
//! stick and four buttons) and the light pen.
//!
//! **Lines.** Pins 1–4 are the four buttons (pulled low while pressed,
//! [`pin::BUTTONS`]); the console reads them through the sound chip's I/O
//! port, and button 4 of port 2 also reaches the VIA's CA1. The stick is
//! two potentiometers; [`PortLines::analog`] carries them as X (0 left,
//! $8000 centre, $FFFF right) and Y (0 down, $FFFF up). The console finds
//! their position with its DAC and a comparator.
//!
//! **Light pen.** It plugs into port 2 and pulls button 4's line low while
//! its photo cell sees the beam. Only the console knows where the beam is:
//! the device sets [`pin::PHOTOCELL`] and reports its aim in `analog` (in
//! the console's raster picture: x, y pixels; [`NO_AIM`] off the screen),
//! and the console pulls the line when the lit beam passes close by.
//!
//! Input: controller — [`InputEvent::Digital`] (the stick at full
//! deflection; buttons 1–4 from bits 0–3, 3 and 4 also from bits 4 and 5,
//! a gamepad's West and North), [`InputEvent::Analog`] axis 0 X / axis 1
//! Y (0–65535, $8000 centre; Y up), [`InputEvent::Key`] `machine_key` 0–3
//! (buttons 1–4) and 4–7 (stick up, down, left, right, for the keyboard).
//! Light pen — [`InputEvent::LightPen`] or [`InputEvent::MouseRel`]
//! (moves the aim; button 0 holds it to the screen).
//!
//! Sources: roadsidethoughts.com, "Vectrex Programmer Notes": "6522
//! Interface Adapter", "Programmable Sound Generator" (read 2026-10-24).
//! The implementation is this project's own.

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

pub mod pin {
    /// Buttons 1–4 (pins 1–4).
    pub const BUTTON1: u32 = 1 << 0;
    pub const BUTTON4: u32 = 1 << 3;
    pub const BUTTONS: u32 = 0x0f;
    /// The device is a light pen; `analog` holds its aim.
    pub const PHOTOCELL: u32 = 1 << 8;
}

/// Stick centre.
pub const CENTRE: u16 = 0x8000;
/// Off-screen aim of the light pen.
pub const NO_AIM: u16 = 0xffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectrexKind {
    Controller,
    LightPen,
}

impl VectrexKind {
    fn code(self) -> u8 {
        match self {
            VectrexKind::Controller => 0,
            VectrexKind::LightPen => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectrexController {
    kind: VectrexKind,
    /// Buttons from joystick events and from keys (bits 0–3).
    pad: u8,
    keys: u8,
    /// Directions from joystick events and from keys.
    dirs: u8,
    key_dirs: u8,
    /// Analog position when no direction is held.
    x: u16,
    y: u16,
    /// Light pen: aim and whether it is held to the screen.
    aim: [u16; 2],
    touching: bool,
}

impl VectrexController {
    pub fn new(kind: VectrexKind) -> Self {
        Self {
            kind,
            pad: 0,
            keys: 0,
            dirs: 0,
            key_dirs: 0,
            x: CENTRE,
            y: CENTRE,
            aim: [NO_AIM; 2],
            touching: false,
        }
    }

    pub fn kind(&self) -> VectrexKind {
        self.kind
    }

    /// Stick position: held directions at full deflection, else analog.
    fn stick(&self) -> [u16; 2] {
        let d = Dirs(self.dirs | self.key_dirs);
        let axis = |neg: Dirs, pos: Dirs, analog: u16| match (d.contains(neg), d.contains(pos)) {
            (true, false) => 0,
            (false, true) => 0xffff,
            _ => analog,
        };
        [
            axis(Dirs::LEFT, Dirs::RIGHT, self.x),
            axis(Dirs::DOWN, Dirs::UP, self.y),
        ]
    }
}

impl PortDevice for VectrexController {
    fn kind(&self) -> PortKind {
        PortKind::VectrexControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        match self.kind {
            VectrexKind::Controller => PortLines {
                pulled_low: u32::from(self.pad | self.keys) & pin::BUTTONS,
                analog: self.stick(),
            },
            VectrexKind::LightPen => PortLines {
                pulled_low: pin::PHOTOCELL,
                analog: if self.touching { self.aim } else { [NO_AIM; 2] },
            },
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        match (self.kind, ev) {
            (VectrexKind::Controller, InputEvent::Digital { dirs, buttons }) => {
                self.dirs = dirs.0 & 0x0f;
                // Buttons 3 and 4 also from the gamepad's West and North.
                let b = u32::from(*buttons);
                self.pad =
                    (b & 3 | u32::from(b & 0x14 != 0) << 2 | u32::from(b & 0x28 != 0) << 3) as u8;
            }
            (VectrexKind::Controller, InputEvent::Analog { axis, value }) => match axis {
                0 => self.x = *value,
                1 => self.y = *value,
                _ => {}
            },
            (VectrexKind::Controller, InputEvent::Key { machine_key, down }) => {
                let k = machine_key.0;
                let (set, bit) = match k {
                    0..=3 => (&mut self.keys, 1 << k),
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
            (VectrexKind::LightPen, InputEvent::LightPen { x, y, trigger }) => {
                self.aim = [*x, *y];
                self.touching = *trigger;
            }
            (VectrexKind::LightPen, InputEvent::MouseRel { dx, dy, buttons }) => {
                let [x, y] = if self.aim[0] == NO_AIM {
                    [200, 250]
                } else {
                    [i32::from(self.aim[0]), i32::from(self.aim[1])]
                };
                self.aim = [
                    (x + i32::from(*dx)).clamp(0, 2000) as u16,
                    (y + i32::from(*dy)).clamp(0, 2000) as u16,
                ];
                self.touching = buttons & 1 != 0;
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"VXPD", 1, |w| {
            w.u8(self.kind.code());
            for v in [self.pad, self.keys, self.dirs, self.key_dirs] {
                w.u8(v);
            }
            for v in [self.x, self.y, self.aim[0], self.aim[1]] {
                w.u16(v);
            }
            w.bool(self.touching);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"VXPD", 1)?;
        if c.u8()? != self.kind.code() {
            return Err(StateError::Invalid("Vectrex controller kind differs"));
        }
        let mut n = self.clone();
        n.pad = c.u8()? & 0x0f;
        n.keys = c.u8()? & 0x0f;
        n.dirs = c.u8()? & 0x0f;
        n.key_dirs = c.u8()? & 0x0f;
        n.x = c.u16()?;
        n.y = c.u16()?;
        n.aim = [c.u16()?, c.u16()?];
        n.touching = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    fn lines(d: &VectrexController) -> PortLines {
        d.read_lines(Cycle(0))
    }

    #[test]
    fn stick_and_buttons() {
        let mut d = VectrexController::new(VectrexKind::Controller);
        assert_eq!(lines(&d).analog, [CENTRE, CENTRE]);
        d.input(&InputEvent::Digital {
            dirs: Dirs::LEFT | Dirs::UP,
            buttons: 0b1001,
        });
        let l = lines(&d);
        assert_eq!(l.analog, [0, 0xffff]);
        assert_eq!(l.pulled_low, pin::BUTTON1 | pin::BUTTON4);
        d.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        });
        d.input(&InputEvent::Analog {
            axis: 0,
            value: 0x9000,
        });
        assert_eq!(lines(&d).analog, [0x9000, CENTRE]);
        d.input(&InputEvent::Key {
            machine_key: MachineKey(7),
            down: true,
        });
        d.input(&InputEvent::Key {
            machine_key: MachineKey(1),
            down: true,
        });
        let l = lines(&d);
        assert_eq!(l.analog[0], 0xffff);
        assert_eq!(l.pulled_low, 2);
    }

    #[test]
    fn light_pen_reports_its_aim_while_touching() {
        let mut d = VectrexController::new(VectrexKind::LightPen);
        assert_eq!(lines(&d).analog, [NO_AIM; 2]);
        d.input(&InputEvent::LightPen {
            x: 100,
            y: 120,
            trigger: true,
        });
        let l = lines(&d);
        assert_eq!(l.pulled_low, pin::PHOTOCELL);
        assert_eq!(l.analog, [100, 120]);
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = VectrexController::new(VectrexKind::Controller);
        d.input(&InputEvent::Digital {
            dirs: Dirs::DOWN,
            buttons: 4,
        });
        let s = d.save_state();
        let mut e = VectrexController::new(VectrexKind::Controller);
        e.load_state(&s).unwrap();
        assert_eq!(e, d);
        assert!(VectrexController::new(VectrexKind::LightPen)
            .load_state(&s)
            .is_err());
    }
}
