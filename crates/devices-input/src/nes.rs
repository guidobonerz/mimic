//! NES controller-port devices: the standard controller, the Zapper light
//! gun and the Arkanoid controller (Vaus). The Four Score is part of the
//! console model (`machine-nes`): it adds two ports.
//!
//! **Lines.** The console drives OUT (the strobe, $4016 bit 0) and a CLK
//! pulse on every read of the port's register; a device drives the data
//! lines D0, D3 and D4. In [`PortLines::pulled_low`] a data bit set means
//! the console reads a 1 there (the lines pass an inverting buffer, so a
//! device pulls them low for that); in what the console writes, [`pin::OUT`]
//! set means the strobe is high and [`pin::CLK`] marks the clock pulse.
//!
//! **Standard controller.** A 4021 shift register: while OUT is high it
//! keeps loading the buttons (reads return A), when OUT falls the buttons
//! are latched, and each CLK shifts the next one out on D0, in the order
//! A, B, Select, Start, Up, Down, Left, Right; after eight the official
//! controller returns 1s.
//!
//! **Zapper.** D4 is the trigger (1 while pulled), D3 the light sensor
//! (0 while it sees light). Whether the photodiode sees light depends on
//! the picture, which only the console knows: the device sets
//! [`pin::PHOTODIODE`] and reports where it points in `analog` (frame
//! pixels x, y; $FFFF off the screen), and the console decides D3.
//!
//! **Arkanoid controller.** A potentiometer read as an 8-bit value: OUT
//! latches it, then each CLK shifts it out on D3 from bit 7, inverted; D4
//! is the button. The knob's range is [`VAUS_MIN`]–[`VAUS_MAX`].
//!
//! Input: standard controller — [`InputEvent::Digital`] (directions;
//! buttons bit 0 A, bit 1 B, bit 2 Select, bit 3 Start) and
//! [`InputEvent::Key`] `machine_key` 0–3 (A, B, Select, Start; for the
//! keyboard). Zapper — [`InputEvent::LightPen`] (aim in frame pixels and
//! trigger) or [`InputEvent::MouseRel`] (moves the aim, button 0 the
//! trigger). Arkanoid — [`InputEvent::MouseRel`] (horizontal motion turns
//! the knob, button 0), [`InputEvent::Analog`] axis 0 (knob position),
//! [`InputEvent::Digital`] button bit 0.
//!
//! Sources: NESdev wiki, "Standard controller", "Controller reading",
//! "Zapper", "Arkanoid controller" (read 2026-10-23). The implementation
//! is this project's own.

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

pub mod pin {
    /// Data lines (set: the console reads 1).
    pub const D0: u32 = 1 << 0;
    pub const D3: u32 = 1 << 3;
    pub const D4: u32 = 1 << 4;
    /// The device has a light sensor; `analog` holds its aim.
    pub const PHOTODIODE: u32 = 1 << 8;
    /// Console side: the strobe is high.
    pub const OUT: u32 = 1 << 16;
    /// Console side: a clock pulse (a read of the port).
    pub const CLK: u32 = 1 << 17;
}

/// The Arkanoid controller's knob range.
pub const VAUS_MIN: u8 = 0x54;
pub const VAUS_MAX: u8 = 0xf4;

/// Off-screen aim.
pub const NO_AIM: u16 = 0xffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NesKind {
    Standard,
    Zapper,
    Arkanoid,
}

impl NesKind {
    fn code(self) -> u8 {
        match self {
            NesKind::Standard => 0,
            NesKind::Zapper => 1,
            NesKind::Arkanoid => 2,
        }
    }
}

/// One device in an NES controller port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NesController {
    kind: NesKind,
    /// Standard: buttons from the joystick events (A B Select Start Up
    /// Down Left Right = bits 0–7) and from the keyboard (bits 0–3).
    pad: u8,
    keys: u8,
    /// Strobe level.
    out: bool,
    /// The shift register and the bits shifted out since the strobe fell.
    shift: u8,
    count: u8,
    /// Zapper: aim (frame pixels) and trigger; Arkanoid: knob and button.
    x: u16,
    y: u16,
    trigger: bool,
    knob: u8,
}

impl NesController {
    pub fn new(kind: NesKind) -> Self {
        Self {
            kind,
            pad: 0,
            keys: 0,
            out: false,
            shift: 0,
            count: 0,
            x: NO_AIM,
            y: NO_AIM,
            trigger: false,
            knob: (VAUS_MIN / 2) + (VAUS_MAX / 2),
        }
    }

    pub fn kind(&self) -> NesKind {
        self.kind
    }

    fn buttons(&self) -> u8 {
        self.pad | self.keys
    }

    fn load(&mut self) {
        self.shift = match self.kind {
            NesKind::Standard => self.buttons(),
            NesKind::Arkanoid => self.knob,
            NesKind::Zapper => 0,
        };
        self.count = 0;
    }
}

fn dirs_bits(d: Dirs) -> u8 {
    let mut b = 0;
    for (dir, bit) in [
        (Dirs::UP, 4),
        (Dirs::DOWN, 5),
        (Dirs::LEFT, 6),
        (Dirs::RIGHT, 7),
    ] {
        if d.contains(dir) {
            b |= 1 << bit;
        }
    }
    b
}

impl PortDevice for NesController {
    fn kind(&self) -> PortKind {
        PortKind::NesControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        let mut analog = [0; 2];
        match self.kind {
            NesKind::Standard => {
                let bit = if self.out {
                    self.buttons() & 1 != 0
                } else if self.count < 8 {
                    self.shift >> self.count & 1 != 0
                } else {
                    true
                };
                if bit {
                    low |= pin::D0;
                }
            }
            NesKind::Zapper => {
                low |= pin::PHOTODIODE;
                if self.trigger {
                    low |= pin::D4;
                }
                analog = [self.x, self.y];
            }
            NesKind::Arkanoid => {
                let value = if self.out { self.knob } else { self.shift };
                let bit = if self.count < 8 && !self.out {
                    value << self.count & 0x80 != 0
                } else {
                    value & 0x80 != 0
                };
                // Sent inverted: a 0 bit reads as 1.
                if !bit {
                    low |= pin::D3;
                }
                if self.trigger {
                    low |= pin::D4;
                }
            }
        }
        PortLines {
            pulled_low: low,
            analog,
        }
    }

    fn write_lines(&mut self, lines: PortLines, _now: Cycle) {
        let out = lines.pulled_low & pin::OUT != 0;
        if out || self.out {
            // While the strobe is high the register keeps loading; the
            // value at its fall is the one shifted out.
            self.load();
        }
        self.out = out;
        if lines.pulled_low & pin::CLK != 0 && !out && self.count < 8 {
            self.count += 1;
        }
    }

    fn input(&mut self, ev: &InputEvent) {
        match (self.kind, ev) {
            (NesKind::Standard, InputEvent::Digital { dirs, buttons }) => {
                self.pad = dirs_bits(*dirs) | (buttons & 0x0f);
            }
            (NesKind::Standard, InputEvent::Key { machine_key, down }) if machine_key.0 < 4 => {
                let bit = 1 << machine_key.0;
                if *down {
                    self.keys |= bit;
                } else {
                    self.keys &= !bit;
                }
            }
            (NesKind::Zapper, InputEvent::LightPen { x, y, trigger }) => {
                self.x = *x;
                self.y = *y;
                self.trigger = *trigger;
            }
            (NesKind::Zapper, InputEvent::MouseRel { dx, dy, buttons }) => {
                let (x, y) = if self.x == NO_AIM {
                    (128, 120)
                } else {
                    (i32::from(self.x), i32::from(self.y))
                };
                self.x = (x + i32::from(*dx)).clamp(0, 400) as u16;
                self.y = (y + i32::from(*dy)).clamp(0, 300) as u16;
                self.trigger = buttons & 1 != 0;
            }
            (NesKind::Arkanoid, InputEvent::MouseRel { dx, buttons, .. }) => {
                self.knob = (i32::from(self.knob) + i32::from(*dx))
                    .clamp(i32::from(VAUS_MIN), i32::from(VAUS_MAX))
                    as u8;
                self.trigger = buttons & 1 != 0;
            }
            (NesKind::Arkanoid, InputEvent::Analog { axis: 0, value }) => {
                let span = u32::from(VAUS_MAX - VAUS_MIN);
                self.knob = VAUS_MIN + (u32::from(*value) * span / 65535) as u8;
            }
            (NesKind::Arkanoid, InputEvent::Digital { buttons, .. }) => {
                self.trigger = buttons & 1 != 0;
            }
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"NESP", 1, |w| {
            w.u8(self.kind.code());
            for v in [self.pad, self.keys, self.shift, self.count, self.knob] {
                w.u8(v);
            }
            w.bool(self.out);
            w.u16(self.x);
            w.u16(self.y);
            w.bool(self.trigger);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"NESP", 1)?;
        if c.u8()? != self.kind.code() {
            return Err(StateError::Invalid("NES controller kind differs"));
        }
        let mut n = self.clone();
        n.pad = c.u8()?;
        n.keys = c.u8()? & 0x0f;
        n.shift = c.u8()?;
        n.count = c.u8()?.min(8);
        n.knob = c.u8()?.clamp(VAUS_MIN, VAUS_MAX);
        n.out = c.bool()?;
        n.x = c.u16()?;
        n.y = c.u16()?;
        n.trigger = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    fn write(d: &mut NesController, bits: u32) {
        d.write_lines(
            PortLines {
                pulled_low: bits,
                analog: [0; 2],
            },
            Cycle(0),
        );
    }

    /// Strobe, then read `n` bits of `mask` as the console does.
    fn read_bits(d: &mut NesController, n: usize, mask: u32) -> Vec<bool> {
        write(d, pin::OUT);
        write(d, 0);
        (0..n)
            .map(|_| {
                let b = d.read_lines(Cycle(0)).pulled_low & mask != 0;
                write(d, pin::CLK);
                write(d, 0);
                b
            })
            .collect()
    }

    #[test]
    fn standard_controller_shifts_out_the_buttons() {
        let mut d = NesController::new(NesKind::Standard);
        d.input(&InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        });
        d.input(&InputEvent::Key {
            machine_key: MachineKey(3),
            down: true,
        });
        let bits = read_bits(&mut d, 10, pin::D0);
        assert_eq!(
            bits,
            [true, false, false, true, true, false, false, true, true, true],
            "A, B, Select, Start, Up, Down, Left, Right, then 1s"
        );
        // While the strobe is high, every read returns A.
        write(&mut d, pin::OUT);
        for _ in 0..3 {
            assert_ne!(d.read_lines(Cycle(0)).pulled_low & pin::D0, 0);
            write(&mut d, pin::OUT | pin::CLK);
        }
    }

    #[test]
    fn zapper_reports_aim_and_trigger() {
        let mut d = NesController::new(NesKind::Zapper);
        let l = d.read_lines(Cycle(0));
        assert_ne!(l.pulled_low & pin::PHOTODIODE, 0);
        assert_eq!(l.analog, [NO_AIM, NO_AIM]);
        d.input(&InputEvent::LightPen {
            x: 100,
            y: 50,
            trigger: true,
        });
        let l = d.read_lines(Cycle(0));
        assert_eq!(l.analog, [100, 50]);
        assert_ne!(l.pulled_low & pin::D4, 0);
        d.input(&InputEvent::MouseRel {
            dx: -10,
            dy: 5,
            buttons: 0,
        });
        let l = d.read_lines(Cycle(0));
        assert_eq!(l.analog, [90, 55]);
        assert_eq!(l.pulled_low & pin::D4, 0);
    }

    #[test]
    fn arkanoid_sends_the_knob_inverted() {
        let mut d = NesController::new(NesKind::Arkanoid);
        d.input(&InputEvent::Analog { axis: 0, value: 0 });
        // VAUS_MIN = $54 = 0101 0100, inverted on the wire.
        let bits = read_bits(&mut d, 8, pin::D3);
        assert_eq!(bits, [true, false, true, false, true, false, true, true]);
        d.input(&InputEvent::MouseRel {
            dx: 1000,
            dy: 0,
            buttons: 1,
        });
        assert_eq!(d.knob, VAUS_MAX);
        assert_ne!(d.read_lines(Cycle(0)).pulled_low & pin::D4, 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = NesController::new(NesKind::Standard);
        d.input(&InputEvent::Digital {
            dirs: Dirs::LEFT,
            buttons: 2,
        });
        read_bits(&mut d, 3, pin::D0);
        let s = d.save_state();
        let mut e = NesController::new(NesKind::Standard);
        e.load_state(&s).unwrap();
        assert_eq!(e, d);
        let mut z = NesController::new(NesKind::Zapper);
        assert!(z.load_state(&s).is_err());
    }
}
