//! ColecoVision controllers: the standard hand controller (joystick, two
//! fire buttons, 12-key keypad), the Super Action Controller (four buttons
//! and a speed roller), the Driving Module (Expansion Module #2: steering
//! wheel and accelerator pedal) and the Roller Controller (trackball).
//!
//! **Pins** (DE-9): 1 up, 2 down, 3 left, 4 right, 6 fire, 7 and 9 the
//! quadrature phases of a spinner, wheel or trackball axis; 5 and 8 are
//! the commons of the keypad and the joystick segment, which the console
//! pulls low one at a time. In the joystick segment pins 1–4 are the
//! directions and pin 6 the left fire button; in the keypad segment pins
//! 1–4 carry the 4-bit code of the pressed key and pin 6 the right fire
//! button. All lines are low when active.
//!
//! **Keypad codes** (the value the console reads in bits 3–0, bit 0 from
//! pin 1, bit 1 from pin 4, bit 2 from pin 2, bit 3 from pin 3): 1 $D,
//! 2 $7, 3 $C, 4 $2, 5 $3, 6 $E, 7 $5, 8 $1, 9 $B, 0 $A, * $9, # $6, no
//! key $F; the Super Action Controller's purple and blue buttons $8 and
//! $4. These are this project's reading of the BIOS keypad table as
//! commonly documented, not checked against hardware (see
//! `docs/sources.md`).
//!
//! **Quadrature**: an axis's position steps through the phases (pin 7,
//! pin 9) = 00, 10, 11, 01 (1 = high) turning one way, the reverse the
//! other way. The Roller Controller plugs into both ports: its X axis and
//! fire button are on the port-1 cable (pins 7/9, 6), its Y axis and second
//! button on the port-2 cable, which this model carries in the extra bits
//! [`pin::Y_FIRE`], [`pin::Y_A`], [`pin::Y_B`] of the same device (with
//! [`pin::Y_CABLE`] always set, so the console knows it is there). The
//! Driving Module's wheel is the axis of its port, its pedal the left fire
//! button (pin 6 in the joystick segment) — the pedal's line is this
//! project's assumption.
//!
//! Input: [`InputEvent::Digital`] — directions; buttons bit 0 left fire
//! (Super Action: yellow), bit 1 right fire (orange), bit 2 purple, bit 3
//! blue; the Driving Module turns its wheel while left or right is held
//! and gives gas with up or bit 0. [`InputEvent::Key`] `machine_key` 0–11
//! the keypad in reading order (1 2 3 4 5 6 7 8 9 * 0 #), 12 purple, 13
//! blue, 14 the right fire button (for keyboards; a gamepad has one fire
//! button in the joystick events). [`InputEvent::MouseRel`] turns the axes: the speed roller with
//! vertical motion, the wheel with horizontal motion (left button = gas),
//! the trackball with both (buttons = the two fire buttons).

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

/// Pins as `PortLines` bits (bit n = pin n + 1), plus the Roller
/// Controller's second cable.
pub mod pin {
    pub const UP: u32 = 1 << 0;
    pub const DOWN: u32 = 1 << 1;
    pub const LEFT: u32 = 1 << 2;
    pub const RIGHT: u32 = 1 << 3;
    /// Keypad segment common (driven by the console).
    pub const KEYPAD_COMMON: u32 = 1 << 4;
    pub const FIRE: u32 = 1 << 5;
    pub const SPIN_A: u32 = 1 << 6;
    /// Joystick segment common (driven by the console).
    pub const JOY_COMMON: u32 = 1 << 7;
    pub const SPIN_B: u32 = 1 << 8;
    /// Roller Controller, port-2 cable: fire, phase A, phase B.
    pub const Y_FIRE: u32 = 1 << 9;
    pub const Y_A: u32 = 1 << 10;
    pub const Y_B: u32 = 1 << 11;
    /// Always set by the Roller Controller: its port-2 cable is in use.
    pub const Y_CABLE: u32 = 1 << 12;
}

/// Keypad codes for keys 0–13 (1 2 3 4 5 6 7 8 9 * 0 # purple blue).
pub const KEYPAD_CODES: [u8; 14] = [
    0xd, 0x7, 0xc, 0x2, 0x3, 0xe, 0x5, 0x1, 0xb, 0x9, 0xa, 0x6, 0x8, 0x4,
];

/// The pins carrying keypad code bits 0–3.
const CODE_PINS: [u32; 4] = [pin::UP, pin::RIGHT, pin::DOWN, pin::LEFT];

/// Pins low for the quadrature phase of a position (inverted: 1 = low).
fn phase_low(position: u32, a: u32, b: u32) -> u32 {
    // (A, B) high levels: 00, 10, 11, 01.
    match position & 3 {
        0 => a | b,
        1 => b,
        2 => 0,
        _ => a,
    }
}

/// Which controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColecoKind {
    Standard,
    SuperAction,
    Driving,
    Roller,
}

impl ColecoKind {
    fn code(self) -> u8 {
        match self {
            ColecoKind::Standard => 0,
            ColecoKind::SuperAction => 1,
            ColecoKind::Driving => 2,
            ColecoKind::Roller => 3,
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => ColecoKind::Standard,
            1 => ColecoKind::SuperAction,
            2 => ColecoKind::Driving,
            3 => ColecoKind::Roller,
            _ => return None,
        })
    }
}

/// Mouse counts per quadrature step.
const MOUSE_COUNTS: i32 = 2;

/// One axis's steps from host motion: position at `since`, steps still
/// to play out (at most one per `cycles_per_step`, so a console sampling
/// once a line sees every phase).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Axis {
    pos: i64,
    since: u64,
    todo: i64,
}

impl Axis {
    fn at(&self, now: u64, per_step: u64) -> i64 {
        let done = (now.saturating_sub(self.since) / per_step) as i64;
        self.pos + self.todo.signum() * done.min(self.todo.abs())
    }

    /// Take the steps due by `now`, then queue `add` more.
    fn settle(&mut self, now: u64, per_step: u64, add: i64) {
        let steps = (now.saturating_sub(self.since) / per_step).min(self.todo.unsigned_abs());
        self.pos += self.todo.signum() * steps as i64;
        self.todo -= self.todo.signum() * steps as i64;
        if self.todo == 0 {
            self.since = now;
        } else {
            self.since += steps * per_step;
        }
        self.todo += add;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColecoController {
    kind: ColecoKind,
    /// Machine cycles per wheel step while left or right is held.
    cycles_per_step: u64,
    dirs: Dirs,
    /// Buttons: bit 0 left fire, 1 right fire, 2 purple, 3 blue.
    buttons: u8,
    /// The right fire button held on the keyboard.
    key_fire: bool,
    /// Pressed keypad keys (bit n = key n).
    keys: u16,
    /// The console selects the keypad segment (else the joystick one).
    keypad: bool,
    /// Axis positions from host motion (X: the port's own axis; Y: the
    /// Roller's second).
    axes: [Axis; 2],
    /// Mouse counts not yet a step, and steps not yet queued.
    mouse: [i32; 2],
    queued: [i64; 2],
    /// The Driving Module's wheel turned with directions: position at
    /// `since`.
    wheel: i64,
    /// Digital wheel turning (−1, 0, 1) since `since`.
    turning: i8,
    wanted: i8,
    since: u64,
}

impl ColecoController {
    /// `cycles_per_step`: how fast the Driving Module's wheel turns while
    /// a direction is held (machine cycles per quadrature step).
    pub fn new(kind: ColecoKind, cycles_per_step: u64) -> Self {
        Self {
            kind,
            cycles_per_step: cycles_per_step.max(1),
            dirs: Dirs::NONE,
            buttons: 0,
            key_fire: false,
            keys: 0,
            keypad: false,
            axes: [Axis::default(); 2],
            mouse: [0; 2],
            queued: [0; 2],
            wheel: 0,
            turning: 0,
            wanted: 0,
            since: 0,
        }
    }

    pub fn kind(&self) -> ColecoKind {
        self.kind
    }

    fn wheel_at(&self, now: u64) -> i64 {
        let steps = (now.saturating_sub(self.since) / self.cycles_per_step) as i64;
        self.wheel + i64::from(self.turning) * steps
    }

    fn position(&self, axis: usize, now: u64) -> u32 {
        let p = self.axes[axis].at(now, self.cycles_per_step)
            + if axis == 0 { self.wheel_at(now) } else { 0 };
        p as u32
    }

    /// The keypad code of the pressed key (the lowest one), $F for none.
    fn keypad_code(&self) -> u8 {
        let mut keys = self.keys;
        if self.kind == ColecoKind::SuperAction {
            keys |= u16::from(self.buttons >> 2 & 3) << 12;
        }
        (0..14)
            .find(|k| keys >> k & 1 != 0)
            .map_or(0xf, |k| KEYPAD_CODES[k])
    }

    /// Host motion: whole steps are queued for the next line update.
    fn mouse_axis(&mut self, axis: usize, d: i32) {
        self.mouse[axis] += d;
        let steps = self.mouse[axis] / MOUSE_COUNTS;
        self.mouse[axis] -= steps * MOUSE_COUNTS;
        self.queued[axis] += i64::from(steps);
    }
}

impl PortDevice for ColecoController {
    fn kind(&self) -> PortKind {
        PortKind::ColecoControl
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut low = 0;
        if self.keypad {
            let code = self.keypad_code();
            for (bit, p) in CODE_PINS.iter().enumerate() {
                if code >> bit & 1 == 0 {
                    low |= p;
                }
            }
            if self.buttons & 2 != 0 || self.key_fire {
                low |= pin::FIRE;
            }
        } else {
            if self.kind != ColecoKind::Driving {
                for (d, p) in [
                    (Dirs::UP, pin::UP),
                    (Dirs::DOWN, pin::DOWN),
                    (Dirs::LEFT, pin::LEFT),
                    (Dirs::RIGHT, pin::RIGHT),
                ] {
                    if self.dirs.contains(d) {
                        low |= p;
                    }
                }
            }
            let gas = self.kind == ColecoKind::Driving && self.dirs.contains(Dirs::UP);
            if self.buttons & 1 != 0 || gas {
                low |= pin::FIRE;
            }
        }
        if self.kind != ColecoKind::Standard {
            low |= phase_low(self.position(0, now.0), pin::SPIN_A, pin::SPIN_B);
        }
        if self.kind == ColecoKind::Roller {
            low |= pin::Y_CABLE | phase_low(self.position(1, now.0), pin::Y_A, pin::Y_B);
            if self.buttons & 2 != 0 {
                low |= pin::Y_FIRE;
            }
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    /// The console selects a segment (pin 5 or pin 8 low). The time (the
    /// console updates the lines every raster line) settles a change of the
    /// wheel's turning direction and starts queued host motion.
    fn write_lines(&mut self, lines: PortLines, now: Cycle) {
        if lines.pulled_low & pin::KEYPAD_COMMON != 0 {
            self.keypad = true;
        } else if lines.pulled_low & pin::JOY_COMMON != 0 {
            self.keypad = false;
        }
        if self.wanted != self.turning {
            self.wheel = self.wheel_at(now.0);
            self.turning = self.wanted;
            self.since = now.0;
        } else if self.turning == 0 {
            self.since = now.0;
        }
        for (axis, q) in self.axes.iter_mut().zip(&mut self.queued) {
            axis.settle(now.0, self.cycles_per_step, std::mem::take(q));
        }
    }

    fn input(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Digital { dirs, buttons } => {
                self.dirs = dirs;
                self.buttons = buttons & 0x0f;
                if self.kind == ColecoKind::Driving {
                    self.wanted = if dirs.contains(Dirs::RIGHT) {
                        1
                    } else if dirs.contains(Dirs::LEFT) {
                        -1
                    } else {
                        0
                    };
                }
            }
            InputEvent::Key { machine_key, down } if machine_key.0 < 14 => {
                let bit = 1 << machine_key.0;
                if down {
                    self.keys |= bit;
                } else {
                    self.keys &= !bit;
                }
            }
            InputEvent::Key { machine_key, down } if machine_key.0 == 14 => {
                self.key_fire = down;
            }
            InputEvent::MouseRel { dx, dy, buttons } => match self.kind {
                ColecoKind::Standard => {}
                ColecoKind::SuperAction => self.mouse_axis(0, i32::from(dy)),
                ColecoKind::Driving => {
                    self.mouse_axis(0, i32::from(dx));
                    self.buttons = (self.buttons & !1) | (buttons & 1);
                }
                ColecoKind::Roller => {
                    self.mouse_axis(0, i32::from(dx));
                    self.mouse_axis(1, i32::from(dy));
                    self.buttons = (self.buttons & !3) | (buttons & 3);
                }
            },
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"CVPD", 1, |w| {
            w.u8(self.kind.code());
            w.u8(self.dirs.0);
            w.u8(self.buttons);
            w.bool(self.key_fire);
            w.u16(self.keys);
            w.bool(self.keypad);
            for a in self.axes {
                w.u64(a.pos as u64);
                w.u64(a.since);
                w.u64(a.todo as u64);
            }
            for v in self.mouse {
                w.u32(v as u32);
            }
            for v in self.queued {
                w.u64(v as u64);
            }
            w.u64(self.wheel as u64);
            w.u8(self.turning as u8);
            w.u8(self.wanted as u8);
            w.u64(self.since);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"CVPD", 1)?;
        let kind = ColecoKind::from_code(c.u8()?).ok_or(StateError::Invalid("controller kind"))?;
        if kind != self.kind {
            return Err(StateError::Invalid("controller kind differs"));
        }
        let mut n = *self;
        n.dirs = Dirs(c.u8()? & 0x0f);
        n.buttons = c.u8()? & 0x0f;
        n.key_fire = c.bool()?;
        n.keys = c.u16()? & 0x3fff;
        n.keypad = c.bool()?;
        for a in &mut n.axes {
            a.pos = c.u64()? as i64;
            a.since = c.u64()?;
            a.todo = c.u64()? as i64;
        }
        for v in &mut n.mouse {
            *v = (c.u32()? as i32).clamp(-MOUSE_COUNTS, MOUSE_COUNTS);
        }
        for v in &mut n.queued {
            *v = c.u64()? as i64;
        }
        n.wheel = c.u64()? as i64;
        n.turning = (c.u8()? as i8).clamp(-1, 1);
        n.wanted = (c.u8()? as i8).clamp(-1, 1);
        n.since = c.u64()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    fn select(c: &mut ColecoController, keypad: bool, now: u64) -> u32 {
        let common = if keypad {
            pin::KEYPAD_COMMON
        } else {
            pin::JOY_COMMON
        };
        c.write_lines(
            PortLines {
                pulled_low: common,
                analog: [0; 2],
            },
            Cycle(now),
        );
        c.read_lines(Cycle(now)).pulled_low
    }

    fn key(c: &mut ColecoController, k: u16, down: bool) {
        c.input(&InputEvent::Key {
            machine_key: MachineKey(k),
            down,
        });
    }

    /// The keypad code as the console reads it (bits 3–0, 1 = high).
    fn code(low: u32) -> u8 {
        CODE_PINS
            .iter()
            .enumerate()
            .fold(0, |v, (bit, p)| v | u8::from(low & p == 0) << bit)
    }

    #[test]
    fn segments_joystick_fire_and_keypad_codes() {
        let mut c = ColecoController::new(ColecoKind::Standard, 1000);
        c.input(&InputEvent::Digital {
            dirs: Dirs::UP | Dirs::LEFT,
            buttons: 1,
        });
        assert_eq!(select(&mut c, false, 0), pin::UP | pin::LEFT | pin::FIRE);
        assert_eq!(code(select(&mut c, true, 0)), 0xf, "no key, no right fire");
        c.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 2,
        });
        assert_eq!(select(&mut c, true, 0) & pin::FIRE, pin::FIRE, "right fire");
        assert_eq!(select(&mut c, false, 0), 0);
        for (k, want) in [(0, 0xd), (4, 0x3), (9, 0x9), (10, 0xa), (11, 0x6)] {
            key(&mut c, k, true);
            assert_eq!(code(select(&mut c, true, 0)), want, "key {k}");
            key(&mut c, k, false);
        }
        // A standard controller has no purple button; a Super Action one does.
        c.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 4,
        });
        assert_eq!(code(select(&mut c, true, 0)), 0xf);
        let mut s = ColecoController::new(ColecoKind::SuperAction, 1000);
        s.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 8,
        });
        assert_eq!(code(select(&mut s, true, 0)), 0x4, "blue");
    }

    #[test]
    fn spinners_step_through_the_quadrature_phases() {
        let mut s = ColecoController::new(ColecoKind::SuperAction, 1000);
        let phase = |s: &ColecoController, now: u64| {
            s.read_lines(Cycle(now)).pulled_low & (pin::SPIN_A | pin::SPIN_B)
        };
        assert_eq!(phase(&s, 0), pin::SPIN_A | pin::SPIN_B);
        // Four steps of motion at once play out one per 1000 cycles.
        s.input(&InputEvent::MouseRel {
            dx: 0,
            dy: 4 * MOUSE_COUNTS as i16,
            buttons: 0,
        });
        select(&mut s, false, 0);
        let seen: Vec<u32> = (1..=5).map(|i| phase(&s, i * 1000)).collect();
        assert_eq!(
            seen,
            [
                pin::SPIN_B,
                0,
                pin::SPIN_A,
                pin::SPIN_A | pin::SPIN_B,
                pin::SPIN_A | pin::SPIN_B
            ]
        );
        // The Driving Module turns while right is held: one step per 1000
        // cycles; up is the gas pedal on the fire line.
        let mut d = ColecoController::new(ColecoKind::Driving, 1000);
        d.input(&InputEvent::Digital {
            dirs: Dirs::RIGHT | Dirs::UP,
            buttons: 0,
        });
        select(&mut d, false, 0);
        let low = select(&mut d, false, 2500);
        assert_eq!(low & (pin::SPIN_A | pin::SPIN_B), 0, "two steps");
        assert_eq!(
            low & (pin::FIRE | pin::RIGHT),
            pin::FIRE,
            "gas, no direction"
        );
        // The Roller Controller carries both axes.
        let mut r = ColecoController::new(ColecoKind::Roller, 1000);
        r.input(&InputEvent::MouseRel {
            dx: -(MOUSE_COUNTS as i16),
            dy: MOUSE_COUNTS as i16,
            buttons: 2,
        });
        select(&mut r, false, 0);
        let low = r.read_lines(Cycle(1000)).pulled_low;
        assert_eq!(low & (pin::SPIN_A | pin::SPIN_B), pin::SPIN_A);
        assert_eq!(
            low & (pin::Y_A | pin::Y_B | pin::Y_FIRE | pin::Y_CABLE),
            pin::Y_B | pin::Y_FIRE | pin::Y_CABLE
        );
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = ColecoController::new(ColecoKind::Roller, 1000);
        key(&mut c, 3, true);
        c.input(&InputEvent::MouseRel {
            dx: 5,
            dy: -3,
            buttons: 1,
        });
        select(&mut c, true, 10);
        let mut back = ColecoController::new(ColecoKind::Roller, 1000);
        back.load_state(&c.save_state()).unwrap();
        assert_eq!(back, c);
        let mut other = ColecoController::new(ColecoKind::Standard, 1000);
        assert!(other.load_state(&c.save_state()).is_err());
        assert!(back.load_state(b"junk").is_err());
    }
}
