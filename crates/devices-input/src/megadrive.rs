//! Mega Drive / Genesis control pads: the 3-button pad and the 6-button
//! pad.
//!
//! **Lines.** The console drives TH (pin 7; [`pin::TH`] in what it writes,
//! set while TH is low); the pad drives UP, DOWN, LEFT, RIGHT, TL (pin 6)
//! and TR (pin 9), pulled low for a pressed button. A 74HC157 selects by
//! TH: high gives up, down, left, right, B (TL) and C (TR); low gives up,
//! down, LEFT and RIGHT held low, A (TR) and START (TL).
//!
//! **6-button pad.** The pad counts TH's high-to-low edges: on the second
//! low phase the four directions all read low, on the following high
//! phase they carry Z, Y, X, MODE (bits 0–3), on the third low phase they
//! all read high; then the normal values again. Without an edge for about
//! 8192 68000 cycles the count starts over ([`TIMEOUT`], in master
//! clocks).
//!
//! Input: [`InputEvent::Digital`] — directions; buttons bit 0 B, bit 1 C,
//! bit 2 MODE, bit 3 START, bit 4 A, bit 5 Y, bit 6 X, bit 7 Z (the host
//! gamepad's South, East, Select, Start, West, North and shoulders); and
//! [`InputEvent::Key`] `machine_key` 0–7 for A, B, C, START, X, Y, Z,
//! MODE (keyboard).
//!
//! Sources: Charles MacDonald, *Sega Genesis hardware notes* v0.8, section
//! 3 (the pad protocol, the 6-button sequence and its time-out); Neal
//! Howland, "Genesis joystick pinouts" (1993; the 74HC157 wiring). The
//! implementation is this project's own.

use mimic_core::input::{Dirs, InputEvent};
use mimic_core::port::{PortDevice, PortKind, PortLines};
use mimic_core::savestate::{StateError, StateReader, StateWriter};
use mimic_core::Cycle;

pub mod pin {
    pub const UP: u32 = 1 << 0;
    pub const DOWN: u32 = 1 << 1;
    pub const LEFT: u32 = 1 << 2;
    pub const RIGHT: u32 = 1 << 3;
    pub const TL: u32 = 1 << 5;
    /// Console side: TH is low.
    pub const TH: u32 = 1 << 6;
    pub const TR: u32 = 1 << 8;
}

/// Button bits as kept here.
mod button {
    pub const A: u8 = 1 << 0;
    pub const B: u8 = 1 << 1;
    pub const C: u8 = 1 << 2;
    pub const START: u8 = 1 << 3;
    pub const X: u8 = 1 << 4;
    pub const Y: u8 = 1 << 5;
    pub const Z: u8 = 1 << 6;
    pub const MODE: u8 = 1 << 7;
}

/// The 6-button pad's time-out, in master clocks (8192 68000 cycles).
pub const TIMEOUT: u64 = 8192 * 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdPad {
    six: bool,
    dirs: Dirs,
    /// From [`InputEvent::Digital`].
    pad: u8,
    /// From [`InputEvent::Key`].
    keys: u8,
    th_high: bool,
    edges: u8,
    last_edge: u64,
}

impl MdPad {
    pub fn three_button() -> Self {
        Self::new(false)
    }

    pub fn six_button() -> Self {
        Self::new(true)
    }

    fn new(six: bool) -> Self {
        Self {
            six,
            dirs: Dirs::NONE,
            pad: 0,
            keys: 0,
            th_high: true,
            edges: 0,
            last_edge: 0,
        }
    }

    pub fn is_six_button(&self) -> bool {
        self.six
    }

    fn buttons(&self) -> u8 {
        let p = self.pad;
        let mut b = self.keys;
        for (bit, out) in [
            (0x01, button::B),
            (0x02, button::C),
            (0x04, button::MODE),
            (0x08, button::START),
            (0x10, button::A),
            (0x20, button::Y),
            (0x40, button::X),
            (0x80, button::Z),
        ] {
            if p & bit != 0 {
                b |= out;
            }
        }
        b
    }
}

impl PortDevice for MdPad {
    fn kind(&self) -> PortKind {
        PortKind::MegaDrivePad
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let b = self.buttons();
        let d = self.dirs;
        let on = |c: bool, p: u32| if c { p } else { 0 };
        let dirs = on(d.0 & Dirs::UP.0 != 0, pin::UP)
            | on(d.0 & Dirs::DOWN.0 != 0, pin::DOWN)
            | on(d.0 & Dirs::LEFT.0 != 0, pin::LEFT)
            | on(d.0 & Dirs::RIGHT.0 != 0, pin::RIGHT);
        let low = match (self.th_high, self.six, self.edges) {
            (true, true, 2) => {
                on(b & button::Z != 0, pin::UP)
                    | on(b & button::Y != 0, pin::DOWN)
                    | on(b & button::X != 0, pin::LEFT)
                    | on(b & button::MODE != 0, pin::RIGHT)
                    | on(b & button::B != 0, pin::TL)
                    | on(b & button::C != 0, pin::TR)
            }
            (true, _, _) => {
                dirs | on(b & button::B != 0, pin::TL) | on(b & button::C != 0, pin::TR)
            }
            (false, true, 2) => {
                pin::UP
                    | pin::DOWN
                    | pin::LEFT
                    | pin::RIGHT
                    | on(b & button::START != 0, pin::TL)
                    | on(b & button::A != 0, pin::TR)
            }
            (false, true, 3) => {
                on(b & button::START != 0, pin::TL) | on(b & button::A != 0, pin::TR)
            }
            (false, _, _) => {
                (dirs & (pin::UP | pin::DOWN))
                    | pin::LEFT
                    | pin::RIGHT
                    | on(b & button::START != 0, pin::TL)
                    | on(b & button::A != 0, pin::TR)
            }
        };
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    fn write_lines(&mut self, lines: PortLines, now: Cycle) {
        let high = lines.pulled_low & pin::TH == 0;
        if high == self.th_high {
            return;
        }
        if now.0.saturating_sub(self.last_edge) > TIMEOUT {
            self.edges = 0;
        }
        self.th_high = high;
        if !high {
            self.edges = (self.edges + 1).min(4);
            self.last_edge = now.0;
        }
        if self.edges > 3 && high {
            // The sequence is over: normal values until the time-out.
            self.edges = 4;
        }
    }

    fn input(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Digital { dirs, buttons } => {
                self.dirs = dirs;
                self.pad = buttons;
            }
            InputEvent::Key { machine_key, down } if machine_key.0 < 8 => {
                let bit = [
                    button::A,
                    button::B,
                    button::C,
                    button::START,
                    button::X,
                    button::Y,
                    button::Z,
                    button::MODE,
                ][usize::from(machine_key.0)];
                if down {
                    self.keys |= bit;
                } else {
                    self.keys &= !bit;
                }
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"MDPD", 1, |w| {
            w.bool(self.six);
            w.u8(self.dirs.0);
            w.u8(self.pad);
            w.u8(self.keys);
            w.bool(self.th_high);
            w.u8(self.edges);
            w.u64(self.last_edge);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"MDPD", 1)?;
        if c.bool()? != self.six {
            return Err(StateError::Invalid("pad type differs"));
        }
        let n = Self {
            six: self.six,
            dirs: Dirs(c.u8()? & 0x0f),
            pad: c.u8()?,
            keys: c.u8()?,
            th_high: c.bool()?,
            edges: c.u8()?.min(4),
            last_edge: c.u64()?,
        };
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mimic_core::input::MachineKey;

    fn th(p: &mut MdPad, high: bool, now: u64) -> u32 {
        p.write_lines(
            PortLines {
                pulled_low: if high { 0 } else { pin::TH },
                analog: [0; 2],
            },
            Cycle(now),
        );
        p.read_lines(Cycle(now)).pulled_low
    }

    #[test]
    fn the_3_button_pad_multiplexes_by_th() {
        let mut p = MdPad::three_button();
        p.input(&InputEvent::Digital {
            dirs: Dirs::UP,
            buttons: 0x01 | 0x10, // B and A
        });
        assert_eq!(th(&mut p, true, 0), pin::UP | pin::TL, "TH high: B on TL");
        assert_eq!(
            th(&mut p, false, 10),
            pin::UP | pin::LEFT | pin::RIGHT | pin::TR,
            "TH low: A on TR, left/right low"
        );
    }

    #[test]
    fn the_6_button_pad_answers_the_third_high_phase() {
        let mut p = MdPad::six_button();
        p.input(&InputEvent::Key {
            machine_key: MachineKey(4),
            down: true,
        }); // X
        let mut t = 1000;
        let mut seq = Vec::new();
        for high in [true, false, true, false, true, false, true] {
            t += 100;
            seq.push(th(&mut p, high, t));
        }
        assert_eq!(seq[3] & 0x0f, 0x0f, "2nd low phase: all four low");
        assert_eq!(seq[4] & 0x0f, pin::LEFT, "then X on LEFT");
        assert_eq!(seq[5] & 0x0f, 0, "3rd low phase: all high");
        assert_eq!(seq[6] & 0x0f, 0, "normal again");
        // After the time-out the sequence starts over.
        t += TIMEOUT + 1;
        th(&mut p, false, t);
        th(&mut p, true, t + 10);
        assert_eq!(th(&mut p, false, t + 20) & 0x0f, 0x0f);
    }

    #[test]
    fn savestates_round_trip() {
        let mut p = MdPad::six_button();
        th(&mut p, false, 5);
        let s = p.save_state();
        let mut q = MdPad::six_button();
        q.load_state(&s).unwrap();
        assert_eq!(p, q);
        assert!(MdPad::three_button().load_state(&s).is_err());
    }
}
