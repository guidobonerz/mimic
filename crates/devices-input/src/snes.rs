//! SNES controller-port devices: the joypad, the two-button mouse and the
//! Super Scope light gun. The Multiplayer 5 adaptor is part of the console
//! model (`machine-snes`).
//!
//! **Lines.** The console drives the latch (OUT0, [`pin::OUT`]), a CLK
//! pulse ([`pin::CLK`]) on each read of the port's data, and IOBit (pin 6,
//! [`pin::IO`]: set while the console leaves it high). A device drives the
//! data lines D0 (pin 4) and D1 (pin 5): in [`PortLines::pulled_low`] a set
//! bit means the console reads a 1 (the lines are active low).
//!
//! - **Joypad**: 16 bits — B, Y, Select, Start, Up, Down, Left, Right, A,
//!   X, L, R, four ID bits (0) — then 1s. The latch loads the buttons.
//! - **Mouse**: 32 bits — 8 zero bits, right and left button, the
//!   sensitivity (2 bits), ID 0001, then vertical direction (1 = up) and
//!   7-bit distance, horizontal direction (1 = left) and distance; the
//!   distances clear when read. A CLK pulse while the latch is high cycles
//!   the sensitivity 0 → 1 → 2 → 0 (only the motion scaling of the real
//!   mouse's settings is modelled: 1, 1.5 and 2 times above 2 counts).
//! - **Super Scope**: 8 bits — fire, cursor, turbo, pause, 0, 0,
//!   off-screen, noise — then 1s (bits 9–16 are its ID, all 1). Fire and
//!   pause read as pressed once per press (fire held in turbo mode). Where
//!   it points is reported like the NES Zapper: [`pin::PHOTODIODE`] and the
//!   aim in `analog` (frame pixels of the 256-wide picture, $FFFF off
//!   screen); the console latches its H/V counters when its beam passes
//!   that point while fire or cursor is held.
//!
//! Input: joypad — [`InputEvent::Digital`] (directions; buttons bit 0 B, 1
//! A, 2 Select, 3 Start, 4 Y, 5 X, 6 L, 7 R) and [`InputEvent::Key`]
//! `machine_key` 0–7 with the same order. Mouse — [`InputEvent::MouseRel`]
//! (button 0 left, 1 right). Super Scope — [`InputEvent::LightPen`] or
//! [`InputEvent::MouseRel`] (aim; button 0 fire, 1 cursor) and
//! [`InputEvent::Key`] 0 turbo switch, 1 pause.
//!
//! Sources: Martin Korth, fullsnes, "SNES Controllers Joypad", "… Mouse",
//! "… SuperScope", "SNES Controllers I/O Ports" (read 2026-10-23). The
//! implementation is this project's own.

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

pub mod pin {
    /// Data lines (set: the console reads 1).
    pub const D0: u32 = 1 << 0;
    pub const D1: u32 = 1 << 1;
    /// The device has a light sensor; `analog` holds its aim.
    pub const PHOTODIODE: u32 = 1 << 8;
    /// The light gun wants the console to latch its counters (fire or
    /// cursor held).
    pub const TRIGGER: u32 = 1 << 9;
    /// Console side: latch high.
    pub const OUT: u32 = 1 << 16;
    /// Console side: a clock pulse (a read of the port).
    pub const CLK: u32 = 1 << 17;
    /// Console side: IOBit (pin 6) left high.
    pub const IO: u32 = 1 << 18;
}

pub const NO_AIM: u16 = 0xffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnesKind {
    Pad,
    Mouse,
    SuperScope,
}

impl SnesKind {
    fn code(self) -> u8 {
        match self {
            SnesKind::Pad => 0,
            SnesKind::Mouse => 1,
            SnesKind::SuperScope => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnesController {
    kind: SnesKind,
    /// Joypad buttons (B Y Select Start Up Down Left Right A X L R as bits
    /// 15–4 of the 16-bit report) from joystick events and keys.
    pad: u16,
    keys: u16,
    out: bool,
    /// The latched report and the bits shifted out.
    shift: u32,
    count: u8,
    // Mouse.
    sensitivity: u8,
    dx: i32,
    dy: i32,
    left_dir: bool,
    up_dir: bool,
    mouse_buttons: u8,
    // Super Scope.
    x: u16,
    y: u16,
    fire: bool,
    cursor: bool,
    turbo: bool,
    pause: bool,
    fire_seen: bool,
    pause_seen: bool,
}

/// Joypad report bits for the event button bits 0–7 (B A Select Start Y X
/// L R).
const BUTTON_BITS: [u16; 8] = [
    1 << 15,
    1 << 7,
    1 << 13,
    1 << 12,
    1 << 14,
    1 << 6,
    1 << 5,
    1 << 4,
];

fn buttons_to_report(buttons: u8) -> u16 {
    (0..8)
        .filter(|b| buttons & (1 << b) != 0)
        .fold(0, |r, b| r | BUTTON_BITS[b])
}

impl SnesController {
    pub fn new(kind: SnesKind) -> Self {
        Self {
            kind,
            pad: 0,
            keys: 0,
            out: false,
            shift: 0,
            count: 0,
            sensitivity: 0,
            dx: 0,
            dy: 0,
            left_dir: false,
            up_dir: false,
            mouse_buttons: 0,
            x: NO_AIM,
            y: NO_AIM,
            fire: false,
            cursor: false,
            turbo: false,
            pause: false,
            fire_seen: false,
            pause_seen: false,
        }
    }

    pub fn kind(&self) -> SnesKind {
        self.kind
    }

    /// The report as a bit string, first bit in bit 31.
    fn report(&mut self) -> u32 {
        match self.kind {
            SnesKind::Pad => u32::from(self.pad | self.keys) << 16,
            SnesKind::Mouse => {
                let scale = |d: i32| -> u32 {
                    let a = d.unsigned_abs();
                    let a = match self.sensitivity {
                        0 => a,
                        1 if a > 2 => a * 3 / 2,
                        2 if a > 2 => a * 2,
                        _ => a,
                    };
                    a.min(127)
                };
                if self.dx != 0 {
                    self.left_dir = self.dx < 0;
                }
                if self.dy != 0 {
                    self.up_dir = self.dy < 0;
                }
                let (h, v) = (scale(self.dx), scale(self.dy));
                self.dx = 0;
                self.dy = 0;
                let b = u32::from(self.mouse_buttons);
                (b & 2) << 22
                    | (b & 1) << 22
                    | u32::from(self.sensitivity) << 20
                    | 1 << 16
                    | u32::from(self.up_dir) << 15
                    | v << 8
                    | u32::from(self.left_dir) << 7
                    | h
            }
            SnesKind::SuperScope => {
                let fire = if self.turbo {
                    self.fire
                } else {
                    self.fire && !std::mem::replace(&mut self.fire_seen, self.fire)
                };
                if !self.fire {
                    self.fire_seen = false;
                }
                let pause = self.pause && !std::mem::replace(&mut self.pause_seen, true);
                if !self.pause {
                    self.pause_seen = false;
                }
                let off = self.x == NO_AIM;
                u32::from(fire) << 31
                    | u32::from(self.cursor) << 30
                    | u32::from(self.turbo) << 29
                    | u32::from(pause) << 28
                    | u32::from(off && (fire || self.cursor)) << 25
                    | 0xff << 16
            }
        }
    }

    fn bit(&self) -> bool {
        if self.count >= 32 {
            return true;
        }
        match self.kind {
            SnesKind::Pad | SnesKind::SuperScope if self.count >= 16 => true,
            _ => self.shift >> (31 - self.count) & 1 != 0,
        }
    }
}

impl PortDevice for SnesController {
    fn kind(&self) -> PortKind {
        PortKind::SnesControl
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let mut low = 0;
        if self.bit() {
            low |= pin::D0;
        }
        let mut analog = [0; 2];
        if self.kind == SnesKind::SuperScope {
            low |= pin::PHOTODIODE;
            if self.fire || self.cursor {
                low |= pin::TRIGGER;
            }
            analog = [self.x, self.y];
        }
        PortLines {
            pulled_low: low,
            analog,
        }
    }

    fn write_lines(&mut self, lines: PortLines, _now: Cycle) {
        let out = lines.pulled_low & pin::OUT != 0;
        let clk = lines.pulled_low & pin::CLK != 0;
        if out && clk && self.kind == SnesKind::Mouse {
            self.sensitivity = (self.sensitivity + 1) % 3;
        }
        if self.out && !out {
            self.shift = self.report();
            self.count = 0;
        } else if out && !self.out {
            self.count = 0;
        }
        if out {
            // While latched the first bit is shown continuously.
            if self.kind == SnesKind::Pad {
                self.shift = u32::from(self.pad | self.keys) << 16;
            }
            self.count = 0;
        } else if clk && self.count < 32 {
            self.count += 1;
        }
        self.out = out;
    }

    fn input(&mut self, ev: &InputEvent) {
        match (self.kind, ev) {
            (SnesKind::Pad, InputEvent::Digital { dirs, buttons }) => {
                let mut r = buttons_to_report(*buttons);
                for (d, bit) in [
                    (Dirs::UP, 11),
                    (Dirs::DOWN, 10),
                    (Dirs::LEFT, 9),
                    (Dirs::RIGHT, 8),
                ] {
                    if dirs.contains(d) {
                        r |= 1 << bit;
                    }
                }
                self.pad = r;
            }
            (SnesKind::Pad, InputEvent::Key { machine_key, down }) if machine_key.0 < 8 => {
                let bit = BUTTON_BITS[usize::from(machine_key.0)];
                if *down {
                    self.keys |= bit;
                } else {
                    self.keys &= !bit;
                }
            }
            (SnesKind::Mouse, InputEvent::MouseRel { dx, dy, buttons }) => {
                self.dx += i32::from(*dx);
                self.dy += i32::from(*dy);
                self.mouse_buttons = buttons & 3;
            }
            (SnesKind::SuperScope, InputEvent::LightPen { x, y, trigger }) => {
                self.x = *x;
                self.y = *y;
                self.fire = *trigger;
            }
            (SnesKind::SuperScope, InputEvent::MouseRel { dx, dy, buttons }) => {
                let (x, y) = if self.x == NO_AIM {
                    (128, 112)
                } else {
                    (i32::from(self.x), i32::from(self.y))
                };
                self.x = (x + i32::from(*dx)).clamp(0, 255) as u16;
                self.y = (y + i32::from(*dy)).clamp(0, 238) as u16;
                self.fire = buttons & 1 != 0;
                self.cursor = buttons & 2 != 0;
            }
            (SnesKind::SuperScope, InputEvent::Key { machine_key, down }) => match machine_key.0 {
                0 if *down => self.turbo = !self.turbo,
                1 => self.pause = *down,
                _ => {}
            },
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"SNSP", 1, |w| {
            w.u8(self.kind.code());
            w.u16(self.pad);
            w.u16(self.keys);
            w.bool(self.out);
            w.u32(self.shift);
            w.u8(self.count);
            w.u8(self.sensitivity);
            w.u32(self.dx as u32);
            w.u32(self.dy as u32);
            for v in [
                self.left_dir,
                self.up_dir,
                self.fire,
                self.cursor,
                self.turbo,
                self.pause,
                self.fire_seen,
                self.pause_seen,
            ] {
                w.bool(v);
            }
            w.u8(self.mouse_buttons);
            w.u16(self.x);
            w.u16(self.y);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"SNSP", 1)?;
        if c.u8()? != self.kind.code() {
            return Err(StateError::Invalid("SNES controller kind differs"));
        }
        let mut n = self.clone();
        n.pad = c.u16()?;
        n.keys = c.u16()?;
        n.out = c.bool()?;
        n.shift = c.u32()?;
        n.count = c.u8()?.min(32);
        n.sensitivity = c.u8()? % 3;
        n.dx = c.u32()? as i32;
        n.dy = c.u32()? as i32;
        for v in [
            &mut n.left_dir,
            &mut n.up_dir,
            &mut n.fire,
            &mut n.cursor,
            &mut n.turbo,
            &mut n.pause,
            &mut n.fire_seen,
            &mut n.pause_seen,
        ] {
            *v = c.bool()?;
        }
        n.mouse_buttons = c.u8()? & 3;
        n.x = c.u16()?;
        n.y = c.u16()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::input::MachineKey;

    fn write(d: &mut SnesController, bits: u32) {
        d.write_lines(
            PortLines {
                pulled_low: bits,
                analog: [0; 2],
            },
            Cycle(0),
        );
    }

    fn read_bits(d: &mut SnesController, n: usize) -> u64 {
        write(d, pin::OUT);
        write(d, 0);
        let mut v = 0u64;
        for _ in 0..n {
            let b = d.read_lines(Cycle(0)).pulled_low & pin::D0 != 0;
            v = v << 1 | u64::from(b);
            write(d, pin::CLK);
            write(d, 0);
        }
        v
    }

    #[test]
    fn joypad_report() {
        let mut d = SnesController::new(SnesKind::Pad);
        d.input(&InputEvent::Digital {
            dirs: Dirs::UP,
            buttons: 0b0000_0011, // B, A
        });
        d.input(&InputEvent::Key {
            machine_key: MachineKey(7), // R
            down: true,
        });
        let v = read_bits(&mut d, 17);
        assert_eq!(v >> 1, 0b1000_1000_1001_0000, "B, Up, A, R; ID 0000");
        assert_eq!(v & 1, 1, "then 1s");
    }

    #[test]
    fn mouse_motion_and_sensitivity() {
        let mut d = SnesController::new(SnesKind::Mouse);
        d.input(&InputEvent::MouseRel {
            dx: -5,
            dy: 3,
            buttons: 1,
        });
        let v = read_bits(&mut d, 32);
        assert_eq!(v >> 16 & 0xffff, 0b0000_0000_0100_0001, "left button, ID 1");
        assert_eq!(v >> 8 & 0xff, 3, "down 3");
        assert_eq!(v & 0xff, 0x80 | 5, "left 5");
        assert_eq!(read_bits(&mut d, 32) & 0x7f7f, 0, "distances cleared");
        // A CLK while latched cycles the sensitivity.
        write(&mut d, pin::OUT);
        write(&mut d, pin::OUT | pin::CLK);
        write(&mut d, 0);
        assert_eq!(read_bits(&mut d, 32) >> 20 & 3, 1);
    }

    #[test]
    fn super_scope_fire_once_and_aim() {
        let mut d = SnesController::new(SnesKind::SuperScope);
        d.input(&InputEvent::LightPen {
            x: 100,
            y: 50,
            trigger: true,
        });
        let l = d.read_lines(Cycle(0));
        assert_eq!(l.analog, [100, 50]);
        assert_ne!(l.pulled_low & pin::TRIGGER, 0);
        assert_eq!(read_bits(&mut d, 16), 0x80ff, "fire once, ID $FF");
        assert_eq!(read_bits(&mut d, 16), 0x00ff, "not again while held");
        d.input(&InputEvent::Key {
            machine_key: MachineKey(0),
            down: true,
        });
        assert_eq!(read_bits(&mut d, 16) >> 8, 0xa0, "turbo: fire held");
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = SnesController::new(SnesKind::Mouse);
        d.input(&InputEvent::MouseRel {
            dx: 7,
            dy: 0,
            buttons: 2,
        });
        read_bits(&mut d, 5);
        let s = d.save_state();
        let mut e = SnesController::new(SnesKind::Mouse);
        e.load_state(&s).unwrap();
        assert_eq!(e, d);
        assert!(SnesController::new(SnesKind::Pad).load_state(&s).is_err());
    }
}
