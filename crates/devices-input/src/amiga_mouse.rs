//! Amiga mouse on a game port: two quadrature signal pairs and three
//! buttons (HRM chapter 8, "Controller ports"; gameport pinout):
//!
//! - pin 1 V and pin 3 VQ: vertical pulses; pin 2 H and pin 4 HQ:
//!   horizontal pulses. Each axis steps through the Gray code
//!   00 → 01 → 11 → 10 (pulse, quadrature) one way and back the other;
//!   Denise counts the steps into JOYxDAT.
//! - pin 6 left button (CIA-A /FIRx), pin 9 right button and pin 5 middle
//!   button (Paula's POT pins).
//!
//! Host motion (one unit = one step) is played out at most one step per
//! `cycles_per_step` machine cycles per axis, so the machine sees every
//! phase if it samples the pins more often than that. The device learns
//! the time from [`PortDevice::write_lines`].

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick::pin;

/// One axis: position in steps at `since`, steps still to play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Axis {
    pos: i64,
    since: u64,
    todo: i64,
}

impl Axis {
    /// Steps taken by `now`.
    fn at(&self, now: u64, per_step: u64) -> i64 {
        let done = (now.saturating_sub(self.since) / per_step) as i64;
        self.pos + self.todo.signum() * done.min(self.todo.abs())
    }

    /// Take the steps due by `now` (a step under way keeps its start), then
    /// queue `add` more.
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

/// Pulse and quadrature line of Gray-code phase `p`.
fn phase_lines(p: i64) -> (bool, bool) {
    match p.rem_euclid(4) {
        0 => (false, false),
        1 => (true, false),
        2 => (true, true),
        _ => (false, true),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmigaMouse {
    cycles_per_step: u64,
    /// Horizontal, vertical.
    axes: [Axis; 2],
    pending: [i32; 2],
    buttons: u8,
    now: u64,
}

impl AmigaMouse {
    pub fn new(cycles_per_step: u64) -> Self {
        Self {
            cycles_per_step: cycles_per_step.max(1),
            axes: [Axis::default(); 2],
            pending: [0; 2],
            buttons: 0,
            now: 0,
        }
    }
}

impl PortDevice for AmigaMouse {
    fn kind(&self) -> PortKind {
        PortKind::AmigaGameport
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut low = 0;
        let (h, hq) = phase_lines(self.axes[0].at(now.0, self.cycles_per_step));
        let (v, vq) = phase_lines(self.axes[1].at(now.0, self.cycles_per_step));
        // A pulse line at 1 is high; the pins are open-collector, so "low"
        // means 0. The machine decodes the levels.
        for (level, bit) in [
            (v, pin::UP),
            (h, pin::DOWN),
            (vq, pin::LEFT),
            (hq, pin::RIGHT),
        ] {
            if level {
                low |= bit;
            }
        }
        for (mask, bit) in [(1, pin::FIRE), (2, pin::BUTTON2), (4, pin::BUTTON3)] {
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
        for (axis, d) in self.axes.iter_mut().zip(self.pending) {
            axis.settle(now.0, self.cycles_per_step, i64::from(d));
        }
        self.pending = [0; 2];
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
        w.chunk(*b"AMSE", 1, |w| {
            for a in &self.axes {
                w.u64(a.pos as u64);
                w.u64(a.since);
                w.u64(a.todo as u64);
            }
            for p in self.pending {
                w.u32(p as u32);
            }
            w.u8(self.buttons);
            w.u64(self.now);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"AMSE", 1)?;
        let mut n = *self;
        for a in n.axes.iter_mut() {
            a.pos = c.u64()? as i64;
            a.since = c.u64()?;
            a.todo = c.u64()? as i64;
        }
        for p in n.pending.iter_mut() {
            *p = c.u32()? as i32;
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

    /// Horizontal phase (H, HQ) from the lines.
    fn h(m: &AmigaMouse, t: u64) -> (bool, bool) {
        let low = m.read_lines(Cycle(t)).pulled_low;
        (low & pin::DOWN != 0, low & pin::RIGHT != 0)
    }

    #[test]
    fn motion_steps_through_the_gray_code() {
        let mut m = AmigaMouse::new(100);
        m.input(&InputEvent::MouseRel {
            dx: 3,
            dy: 0,
            buttons: 1,
        });
        m.write_lines(PortLines::default(), Cycle(1000));
        let phases: Vec<_> = [1000, 1100, 1200, 1300, 1400]
            .iter()
            .map(|&t| h(&m, t))
            .collect();
        assert_eq!(
            phases,
            [
                (false, false),
                (true, false),
                (true, true),
                (false, true),
                (false, true)
            ],
            "three steps, then still"
        );
        assert_ne!(m.read_lines(Cycle(0)).pulled_low & pin::FIRE, 0);
        // Back two steps.
        m.input(&InputEvent::MouseRel {
            dx: -2,
            dy: 0,
            buttons: 2,
        });
        m.write_lines(PortLines::default(), Cycle(2000));
        assert_eq!(h(&m, 2100), (true, true));
        assert_eq!(h(&m, 2200), (true, false));
        assert_ne!(m.read_lines(Cycle(0)).pulled_low & pin::BUTTON2, 0);
    }

    #[test]
    fn motion_arriving_mid_way_continues() {
        let mut m = AmigaMouse::new(100);
        m.input(&InputEvent::MouseRel {
            dx: 0,
            dy: 4,
            buttons: 0,
        });
        m.write_lines(PortLines::default(), Cycle(0));
        m.input(&InputEvent::MouseRel {
            dx: 0,
            dy: 1,
            buttons: 0,
        });
        m.write_lines(PortLines::default(), Cycle(250));
        assert_eq!(m.axes[1].at(250, 100), 2);
        assert_eq!(m.axes[1].at(10_000, 100), 5);
    }

    #[test]
    fn frequent_sampling_does_not_stall_steps() {
        let mut m = AmigaMouse::new(1000);
        m.input(&InputEvent::MouseRel {
            dx: 3,
            dy: 0,
            buttons: 0,
        });
        for t in (0..5000).step_by(454) {
            m.write_lines(PortLines::default(), Cycle(t));
        }
        assert_eq!(m.axes[0].at(5000, 1000), 3);
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = AmigaMouse::new(100);
        m.input(&InputEvent::MouseRel {
            dx: -5,
            dy: 2,
            buttons: 6,
        });
        m.write_lines(PortLines::default(), Cycle(7));
        let s = m.save_state();
        let mut n = AmigaMouse::new(100);
        n.load_state(&s).unwrap();
        assert_eq!(m, n);
    }
}
