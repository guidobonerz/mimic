//! Proportional mice on a Commodore control port: the Amiga mouse, the
//! Atari ST mouse (also the later Atari CX80 trak-ball) and the Atari CX22
//! trak-ball (also the early CX80), plugged into a C64, C128, VIC-20 or
//! Plus/4 and read as joystick lines by software with its own driver.
//!
//! Each axis puts two signals on the direction pins; the computer polls
//! them and counts the steps:
//!
//! - Amiga: a quadrature pair per axis, V on pin 1 (up), H on pin 2 (down),
//!   VQ on pin 3 (left), HQ on pin 4 (right). The pair runs through the
//!   Gray code 00, 10, 11, 01 (pulse, quadrature) for one direction.
//! - Atari ST: the same kind of pairs, XB on pin 1, XA on pin 2, YA on
//!   pin 3, YB on pin 4.
//! - Atari CX22: one pulse line and one direction level per axis, X
//!   direction on pin 1, X pulse on pin 2, Y direction on pin 3, Y pulse
//!   on pin 4; the pulse toggles with every step, the level is high for
//!   right and down.
//!
//! Buttons: left on pin 6 (fire), right on pin 9, middle (Amiga) on pin 5.
//!
//! Host motion (one unit = one step) is played out one step per
//! `cycles_per_step` machine cycles per axis, so a driver polling at least
//! that often sees every phase. The device learns the time from
//! [`PortDevice::write_lines`].
//!
//! Source: the VICE test program `propmouse` (`prop.s`): its pin maps and
//! decode tables for the three devices, which the unit tests replay. The
//! Amiga pair order follows the Amiga Hardware Reference Manual (see
//! [`crate::amiga_mouse`]).

use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::amiga_mouse::{phase_lines, Axis};
use crate::joystick::pin;

/// Which mouse is plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropMouseKind {
    Amiga,
    AtariSt,
    Cx22,
}

impl PropMouseKind {
    /// The option name (`--option port1=...`).
    pub fn name(self) -> &'static str {
        match self {
            PropMouseKind::Amiga => "amigamouse",
            PropMouseKind::AtariSt => "stmouse",
            PropMouseKind::Cx22 => "cx22",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropMouse {
    kind: PropMouseKind,
    cycles_per_step: u64,
    /// Horizontal, vertical.
    axes: [Axis; 2],
    pending: [i32; 2],
    buttons: u8,
    now: u64,
}

impl PropMouse {
    pub fn new(kind: PropMouseKind, cycles_per_step: u64) -> Self {
        Self {
            kind,
            cycles_per_step: cycles_per_step.max(1),
            axes: [Axis::default(); 2],
            pending: [0; 2],
            buttons: 0,
            now: 0,
        }
    }

    /// The levels on pins 1–4 (bit 0 = pin 1) at `now`, from the axis
    /// positions in steps. The Gray-code senses are the ones the
    /// `propmouse` decoder counts as right and down.
    fn levels(&self, now: u64) -> u32 {
        let per = self.cycles_per_step;
        let x = self.axes[0].at(now, per);
        let y = self.axes[1].at(now, per);
        let bit = |level: bool, pin: u32| if level { pin } else { 0 };
        match self.kind {
            PropMouseKind::Amiga => {
                let (h, hq) = phase_lines(-x);
                let (v, vq) = phase_lines(y);
                bit(v, pin::UP) | bit(h, pin::DOWN) | bit(vq, pin::LEFT) | bit(hq, pin::RIGHT)
            }
            PropMouseKind::AtariSt => {
                let (xa, xb) = phase_lines(-x);
                let (ya, yb) = phase_lines(-y);
                bit(xb, pin::UP) | bit(xa, pin::DOWN) | bit(ya, pin::LEFT) | bit(yb, pin::RIGHT)
            }
            PropMouseKind::Cx22 => {
                // The level says where the motion goes; the pulse is the
                // step count's parity. The level turns before the first
                // pulse of a new direction, which the `propmouse` decoder
                // counts as a step of its own (one count per reversal).
                let dir = |axis: &Axis| axis.last_direction() >= 0;
                bit(dir(&self.axes[0]), pin::UP)
                    | bit(x & 1 != 0, pin::DOWN)
                    | bit(dir(&self.axes[1]), pin::LEFT)
                    | bit(y & 1 != 0, pin::RIGHT)
            }
        }
    }
}

impl PortDevice for PropMouse {
    fn kind(&self) -> PortKind {
        PortKind::CbmControl
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        // A level of 1 is a high pin; the CIA reads a pulled-low pin as 0.
        let mut low = !self.levels(now.0) & (pin::UP | pin::DOWN | pin::LEFT | pin::RIGHT);
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
        w.chunk(*b"PMSE", 1, |w| {
            for a in &self.axes {
                w.u64(a.pos as u64);
                w.u64(a.since);
                w.u64(a.todo as u64);
                w.u8(a.dir as u8);
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
        let (_, mut c) = r.chunk(*b"PMSE", 1)?;
        let mut n = *self;
        for a in n.axes.iter_mut() {
            a.pos = c.u64()? as i64;
            a.since = c.u64()?;
            a.todo = c.u64()? as i64;
            a.dir = c.u8()? as i8;
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

    /// The decoder of the VICE `propmouse` test (`prop.s`): a table per
    /// device indexed by the previous and current pair of lines, giving
    /// no change, +1, -1 or an error. `map` reorders the joystick bits
    /// (0 up, 1 down, 2 left, 3 right) into the decoder's order.
    struct Decoder {
        map: [u8; 16],
        table: [u8; 16],
    }

    const AMIGA: Decoder = Decoder {
        // from bits 3=hq,2=vq,1=h,0=v to 3=hq,2=h,1=v,0=vq
        map: [
            0b0000, 0b0010, 0b0100, 0b0110, 0b0001, 0b0011, 0b0101, 0b0111, 0b1000, 0b1010,
            0b1100, 0b1110, 0b1001, 0b1011, 0b1101, 0b1111,
        ],
        table: [
            0x00, 0xff, 0x01, 0x80, 0x01, 0x00, 0x80, 0xff, 0xff, 0x80, 0x00, 0x01, 0x80, 0x01,
            0xff, 0x00,
        ],
    };
    const ST: Decoder = Decoder {
        // from bits 3=yb,2=ya,1=xa,0=xb to 3=xb,2=xa,1=yb,0=ya
        map: [
            0b0000, 0b1000, 0b0100, 0b1100, 0b0001, 0b1001, 0b0101, 0b1101, 0b0010, 0b1010,
            0b0110, 0b1110, 0b0011, 0b1011, 0b0111, 0b1111,
        ],
        table: AMIGA.table,
    };
    const CX22: Decoder = Decoder {
        // from bits 3=y,2=ydir,1=x,0=xdir to 3=x,2=xdir,1=y,0=ydir
        map: [
            0b0000, 0b0100, 0b1000, 0b1100, 0b0001, 0b0101, 0b1001, 0b1101, 0b0010, 0b0110,
            0b1010, 0b1110, 0b0011, 0b0111, 0b1011, 0b1111,
        ],
        table: [
            0x00, 0x80, 0xff, 0x80, 0xff, 0x00, 0x80, 0x01, 0xff, 0x80, 0x00, 0x01, 0x80, 0x01,
            0x80, 0x00,
        ],
    };

    /// The test's `prop_update`: the joystick sample (bit 0 up … bit 3
    /// right, 1 = high) against the previous one, per axis: x from the
    /// high pair of the mapped bits, y from the low pair.
    fn decode(d: &Decoder, prev: u8, cur: u8) -> (i32, i32) {
        let (p, c) = (d.map[prev as usize], d.map[cur as usize]);
        let step = |pp: u8, cc: u8| match d.table[usize::from((pp << 2) | cc)] {
            0x00 => 0,
            0x01 => 1,
            0xff => -1,
            _ => panic!("decoder error for {pp:02b} -> {cc:02b}"),
        };
        (step(p >> 2, c >> 2), step(p & 3, c & 3))
    }

    /// Move `dx`, `dy` and poll every step: what the test would count.
    fn run(kind: PropMouseKind, d: &Decoder, dx: i16, dy: i16) -> (i32, i32) {
        let mut m = PropMouse::new(kind, 100);
        m.write_lines(PortLines::default(), Cycle(0));
        let lines = |m: &PropMouse, t: u64| (!m.read_lines(Cycle(t)).pulled_low & 0x0f) as u8;
        let mut prev = lines(&m, 0);
        m.input(&InputEvent::MouseRel { dx, dy, buttons: 0 });
        m.write_lines(PortLines::default(), Cycle(1000));
        let (mut x, mut y) = (0, 0);
        for t in (1000..3000).step_by(50) {
            let cur = lines(&m, t);
            let (sx, sy) = decode(d, prev, cur);
            x += sx;
            y += sy;
            prev = cur;
        }
        (x, y)
    }

    #[test]
    fn the_test_decoder_counts_our_steps() {
        let mut wrong = Vec::new();
        for (kind, d) in [
            (PropMouseKind::Amiga, &AMIGA),
            (PropMouseKind::AtariSt, &ST),
            (PropMouseKind::Cx22, &CX22),
        ] {
            for (dx, dy) in [(5, 0), (-3, 0), (0, 4), (0, -6), (2, -2)] {
                let got = run(kind, d, dx, dy);
                let want = (i32::from(dx), i32::from(dy));
                // The CX22's direction level turns a sample before its
                // first pulse: the decoder counts a reversal once more.
                let slack = i32::from(kind == PropMouseKind::Cx22);
                let off = |g: i32, w: i32| (g - w).abs() > slack;
                if off(got.0, want.0) || off(got.1, want.1) {
                    wrong.push(format!("{kind:?} ({dx}, {dy}) decoded as {got:?}"));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("; "));
    }

    #[test]
    fn buttons_pull_their_pins() {
        let mut m = PropMouse::new(PropMouseKind::AtariSt, 100);
        m.input(&InputEvent::MouseRel {
            dx: 0,
            dy: 0,
            buttons: 3,
        });
        let low = m.read_lines(Cycle(0)).pulled_low;
        assert_ne!(low & pin::FIRE, 0);
        assert_ne!(low & pin::BUTTON2, 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = PropMouse::new(PropMouseKind::Cx22, 100);
        m.input(&InputEvent::MouseRel {
            dx: 7,
            dy: -2,
            buttons: 1,
        });
        m.write_lines(PortLines::default(), Cycle(500));
        let s = m.save_state();
        let mut n = PropMouse::new(PropMouseKind::Cx22, 100);
        n.load_state(&s).unwrap();
        assert_eq!(m, n);
    }
}
