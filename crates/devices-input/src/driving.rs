//! Atari driving controller (CX20): a knob that turns endlessly, read as
//! a 2-bit Gray code on pins 1 and 2 (16 code steps per turn), and a fire
//! button on pin 6.
//!
//! Gray code sequence (pin 1, pin 2 levels, 1 = high) turning right:
//! 11 → 10 → 00 → 01 → 11. On the Atari 2600 the pins are SWCHA D4/D5
//! (left port) or D0/D1 (right port), fire INPT4/INPT5.
//!
//! Input: [`InputEvent::MouseRel`] turns the knob with horizontal motion
//! (left button fires), [`InputEvent::Analog`] axis 0 sets its angle
//! (0–65535 = one turn), [`InputEvent::Digital`] turns it while left or
//! right is held (one code step per `cycles_per_step` cycles) and fires.
//!
//! Source: "Atari 2600 Specifications"
//! (<https://www.astro.physik.uni-potsdam.de/~mwendt/2600scene/specs.html>),
//! "Driving Controllers".

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// Pins 1 and 2 pulled low per code step (inverted Gray code 11 10 00 01).
const LOW: [u32; 4] = [0, pin::DOWN, pin::UP | pin::DOWN, pin::UP];
/// Mouse counts per code step.
const MOUSE_COUNTS: i32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Driving {
    cycles_per_step: u64,
    /// Code steps turned (mod 4 is the code) at `since`.
    position: u32,
    /// Turning direction while left/right is held (−1, 0, 1), from `since`.
    turning: i8,
    since: u64,
    /// Direction asked for by the last digital input, applied at the
    /// next line update (which brings the time).
    wanted: i8,
    mouse: i32,
    fire: bool,
}

impl Driving {
    /// A controller that turns one code step per `cycles_per_step` cycles
    /// of the machine while left or right is held.
    pub fn new(cycles_per_step: u64) -> Self {
        Self {
            cycles_per_step: cycles_per_step.max(1),
            position: 0,
            turning: 0,
            since: 0,
            wanted: 0,
            mouse: 0,
            fire: false,
        }
    }

    fn position_at(&self, now: u64) -> u32 {
        let steps = (now.saturating_sub(self.since) / self.cycles_per_step) as u32;
        match self.turning {
            1 => self.position.wrapping_add(steps),
            -1 => self.position.wrapping_sub(steps),
            _ => self.position,
        }
    }
}

impl PortDevice for Driving {
    fn kind(&self) -> PortKind {
        PortKind::AtariJoy
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut low = LOW[(self.position_at(now.0) & 3) as usize];
        if self.fire {
            low |= pin::FIRE;
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    /// The machine's line update: brings the time for a change of the
    /// turning direction.
    fn write_lines(&mut self, _lines: PortLines, now: Cycle) {
        if self.wanted != self.turning {
            self.position = self.position_at(now.0);
            self.turning = self.wanted;
            self.since = now.0;
        } else if self.turning == 0 {
            self.since = now.0;
        }
    }

    fn input(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Digital { dirs, buttons } => {
                self.wanted = if dirs.contains(Dirs::RIGHT) {
                    1
                } else if dirs.contains(Dirs::LEFT) {
                    -1
                } else {
                    0
                };
                self.fire = buttons & 1 != 0;
            }
            InputEvent::MouseRel { dx, buttons, .. } => {
                self.mouse += i32::from(dx);
                let steps = self.mouse / MOUSE_COUNTS;
                self.mouse -= steps * MOUSE_COUNTS;
                self.position = self.position.wrapping_add_signed(steps);
                self.fire = buttons & 1 != 0;
            }
            InputEvent::Analog { axis: 0, value } => {
                let turns = self.position & !15;
                self.position = turns | u32::from(value >> 12);
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"DRIV", 1, |w| {
            w.u32(self.position);
            w.u8(self.turning as u8);
            w.u64(self.since);
            w.u8(self.wanted as u8);
            w.u32(self.mouse as u32);
            w.bool(self.fire);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"DRIV", 1)?;
        let position = c.u32()?;
        let turning = c.u8()? as i8;
        let since = c.u64()?;
        let wanted = c.u8()? as i8;
        let mouse = c.u32()? as i32;
        let fire = c.bool()?;
        if !(-1..=1).contains(&turning) || !(-1..=1).contains(&wanted) {
            return Err(StateError::Invalid("driving controller"));
        }
        self.position = position;
        self.turning = turning;
        self.since = since;
        self.wanted = wanted;
        self.mouse = mouse;
        self.fire = fire;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(d: &Driving, now: u64) -> u32 {
        d.read_lines(Cycle(now)).pulled_low & (pin::UP | pin::DOWN)
    }

    #[test]
    fn mouse_steps_through_the_gray_code() {
        let mut d = Driving::new(100);
        let mut seen = vec![code(&d, 0)];
        for _ in 0..4 {
            d.input(&InputEvent::MouseRel {
                dx: MOUSE_COUNTS as i16,
                dy: 0,
                buttons: 0,
            });
            seen.push(code(&d, 0));
        }
        assert_eq!(seen, [LOW[0], LOW[1], LOW[2], LOW[3], LOW[0]]);
        // One bit changes per step.
        for w in seen.windows(2) {
            assert_eq!((w[0] ^ w[1]).count_ones(), 1);
        }
        d.input(&InputEvent::MouseRel {
            dx: -(MOUSE_COUNTS as i16),
            dy: 0,
            buttons: 1,
        });
        assert_eq!(code(&d, 0), LOW[3], "turning back");
        assert_ne!(d.read_lines(Cycle(0)).pulled_low & pin::FIRE, 0);
    }

    #[test]
    fn held_direction_turns_over_time() {
        let mut d = Driving::new(100);
        d.input(&InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 0,
        });
        d.write_lines(PortLines::default(), Cycle(1000));
        assert_eq!(code(&d, 1000), LOW[0]);
        assert_eq!(code(&d, 1100), LOW[1]);
        assert_eq!(code(&d, 1250), LOW[2]);
        d.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        });
        d.write_lines(PortLines::default(), Cycle(1250));
        assert_eq!(code(&d, 5000), LOW[2], "stops when released");
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = Driving::new(100);
        d.input(&InputEvent::MouseRel {
            dx: 21,
            dy: 0,
            buttons: 1,
        });
        let mut back = Driving::new(100);
        back.load_state(&d.save_state()).unwrap();
        assert_eq!(back, d);
    }
}
