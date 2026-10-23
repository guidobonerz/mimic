//! Vectrex controller-port devices: the standard controller (an analog
//! stick and four buttons), the light pen and the 3D Imager.
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
//! **3D Imager** ([`Imager`]). Goggles with a spinning disc of colour
//! filters in front of the eyes, in port 2. The motor runs while pin 2 is
//! high — the console drives it as an output of the sound chip's port
//! (bit 5) for pulse-width control, and as an input the line floats high;
//! the console reports it with [`pin::PIN2_HIGH`]. Once a revolution a
//! sensor at the disc's index pulls button 4's line low for a moment (the
//! console counts the rising edge, also as a CA1 interrupt). The device
//! sets [`pin::IMAGER`] and reports the disc's angle in `analog[0]` (a
//! full turn = 65 536) so the console knows which filter is in front of
//! the screen. The motor's speed and inertia (up to [`IMAGER_MAX_RPS`]
//! revolutions a second, time constant [`IMAGER_TAU_S`]) and the index
//! pulse's width ([`IMAGER_INDEX`] of a turn) are this project's choice:
//! games regulate the speed themselves (3-D Mine Storm needs a turn in
//! under 43 ms).
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
//! Interface Adapter", "Programmable Sound Generator" (read 2026-10-24);
//! for the Imager nicole.express, "Fun With 3 Colors: The Vectrex 3-D
//! Imager!" (2021), FuzzyLogic8192's "Vectrex-3D-Imager-Triangle-Demo"
//! description, and how 3-D Mine Storm drives the port (spin-up loop and
//! interrupt handler, disassembled with this project's tools). The
//! implementation is this project's own.

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
    /// The device is a 3D Imager; `analog[0]` holds the disc's angle.
    pub const IMAGER: u32 = 1 << 9;
    /// Console side: pin 2 is high (the Imager's motor runs).
    pub const PIN2_HIGH: u32 = 1 << 16;
}

/// Top speed of the Imager's disc (revolutions a second, motor always on).
pub const IMAGER_MAX_RPS: u64 = 40;
/// Time constant of the disc's speed in seconds, as a fraction (0.3 s).
pub const IMAGER_TAU_S: (u64, u64) = (3, 10);
/// Width of the index pulse as a fraction of a turn (1/36: 10°).
pub const IMAGER_INDEX: u32 = u32::MAX / 36;

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

/// Fixed-point scale of the disc's speed (angle units per cycle).
const SPEED_SCALE: u64 = 1024;
/// The disc is advanced in steps of at most this many cycles.
const STEP: u64 = 64;

/// The 3D Imager (see the module documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imager {
    /// Machine cycles a second.
    hz: u64,
    /// Disc angle (a full turn = 2³²) and speed (angle units a cycle ×
    /// [`SPEED_SCALE`]) at cycle `t`.
    angle: u32,
    speed: u64,
    t: u64,
    motor: bool,
}

impl Imager {
    /// An Imager on a console clocked at `hz` cycles a second; the disc
    /// stands still.
    pub fn new(hz: u64) -> Self {
        Self {
            hz,
            angle: 0,
            speed: 0,
            t: 0,
            motor: false,
        }
    }

    /// The speed the motor drives towards.
    fn top_speed(&self) -> u64 {
        (1u64 << 32) * IMAGER_MAX_RPS * SPEED_SCALE / self.hz.max(1)
    }

    /// Move the disc on to cycle `to`.
    fn advance(&mut self, to: u64) {
        let tau = (self.hz * IMAGER_TAU_S.0 / IMAGER_TAU_S.1).max(1) as i64;
        let target = if self.motor { self.top_speed() } else { 0 };
        while self.t < to {
            let step = (to - self.t).min(STEP);
            self.angle = self
                .angle
                .wrapping_add((self.speed * step / SPEED_SCALE) as u32);
            let diff = target as i64 - self.speed as i64;
            self.speed = (self.speed as i64 + diff * step as i64 / tau) as u64;
            self.t += step;
        }
        // A machine reset restarts its clock: follow it.
        if to < self.t {
            self.t = to;
        }
    }

    /// Revolutions a second now (tests, tools).
    pub fn rps(&self) -> f64 {
        self.speed as f64 * self.hz as f64 / SPEED_SCALE as f64 / (1u64 << 32) as f64
    }
}

impl PortDevice for Imager {
    fn kind(&self) -> PortKind {
        PortKind::VectrexControl
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut d = self.clone();
        d.advance(now.0);
        let mut low = pin::IMAGER;
        if d.angle < IMAGER_INDEX {
            low |= pin::BUTTON4;
        }
        PortLines {
            pulled_low: low,
            analog: [(d.angle >> 16) as u16, 0],
        }
    }

    fn write_lines(&mut self, lines: PortLines, now: Cycle) {
        self.advance(now.0);
        self.motor = lines.pulled_low & pin::PIN2_HIGH != 0;
    }

    fn input(&mut self, _ev: &InputEvent) {}

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"VX3D", 1, |w| {
            w.u32(self.angle);
            w.u64(self.speed);
            w.u64(self.t);
            w.bool(self.motor);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"VX3D", 1)?;
        let mut n = self.clone();
        n.angle = c.u32()?;
        n.speed = c.u64()?.min(self.top_speed());
        n.t = c.u64()?;
        n.motor = c.bool()?;
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

    #[test]
    fn imager_spins_up_and_pulses_once_a_turn() {
        let hz = 1_500_000;
        let mut d = Imager::new(hz);
        let lines = |bits| PortLines {
            pulled_low: bits,
            analog: [0; 2],
        };
        d.write_lines(lines(pin::PIN2_HIGH), Cycle(0));
        // After two seconds the disc is close to full speed.
        let mut t = 0;
        while t < 2 * hz {
            t += 1000;
            d.write_lines(lines(pin::PIN2_HIGH), Cycle(t));
        }
        assert!((d.rps() - 40.0).abs() < 1.0, "{}", d.rps());
        // Count index pulses (the line rising again) over 0.1 s: 4.
        let mut pulses = 0;
        let mut was_low = d.read_lines(Cycle(t)).pulled_low & pin::BUTTON4 != 0;
        for c in (t..t + hz / 10).step_by(50) {
            let l = d.read_lines(Cycle(c));
            assert_ne!(l.pulled_low & pin::IMAGER, 0);
            let low = l.pulled_low & pin::BUTTON4 != 0;
            if was_low && !low {
                pulses += 1;
            }
            was_low = low;
        }
        assert!((3..=5).contains(&pulses), "{pulses}");
        // Without the motor it slows down.
        d.write_lines(lines(0), Cycle(t));
        d.write_lines(lines(0), Cycle(t + hz / 2));
        assert!(d.rps() < 10.0, "{}", d.rps());
        let s = d.save_state();
        let mut e = Imager::new(hz);
        e.load_state(&s).unwrap();
        assert_eq!(e, d);
    }
}
