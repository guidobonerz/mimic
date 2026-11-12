//! Four-player adapters for the Commodore user port: two more joysticks
//! (the third and fourth player) on CIA2's port B, and for some adapters
//! on PA2 and the serial port.
//!
//! A joystick's switches pull their line low. The lines per adapter (the
//! option name in brackets):
//!
//! | Adapter | Joystick 3 | Joystick 4 |
//! | ------- | ---------- | ---------- |
//! | Classical Games / Protovision ("cga") | up, down, left, right on PB0–PB3 while the computer drives PB7 high; fire PB4 | the same lines while PB7 is low; fire PB5 |
//! | Digital Excess / Hitmen ("hit") | up, down, left, right PB0–PB3; fire PA2 | up, down, left, right PB4–PB7; fire SP2 |
//! | Kingsoft ("kingsoft") | up PA2, down PB7, left PB6, right PB5; fire PB4 | up PB3, down PB2, left PB1, right PB0; fire SP2 |
//! | Starbyte ("starbyte") | up PB3, down PB0, left PB1, right PB2; fire SP2 | up PA2, down PB5, left PB6, right PB7; fire PB4 |
//!
//! The three adapters with a button on SP2 bridge CNT1 to CNT2
//! ([`user_port::CNT_BRIDGE`]): the program shifts a byte out of CIA1,
//! whose clock shifts SP2's level into CIA2's serial register: 0 with the
//! button held, else $FF.
//!
//! The computer's machine gives the device the joysticks' input through
//! [`PortDevice::input_at`] (0: joystick 3, 1: joystick 4) and lists them
//! as two more control ports.
//!
//! Sources: the wiring as the VICE test program `testjoy` reads each
//! adapter (`testprogs/testjoy/testjoy.c`: its read routines and their
//! set-up of the two CIAs' serial ports); Protovision's description of its
//! interface (PB7 selects the joystick). See `docs/sources.md`. That the
//! Protovision adapter's fire lines are not switched by PB7 is this
//! project's reading (the test program reads each with its own PB7 level).

use emu_core::input::{Dirs, InputEvent};
use emu_core::port::{user_port, PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FourPlayerKind {
    Cga,
    Hit,
    Kingsoft,
    Starbyte,
}

/// A line of the user port a switch closes to ground.
#[derive(Clone, Copy)]
enum Line {
    Pb(u8),
    Pa2,
    Sp2,
}

impl FourPlayerKind {
    /// The names [`FourPlayerKind::parse`] knows.
    pub const NAMES: [&'static str; 4] = ["cga", "hit", "kingsoft", "starbyte"];

    /// The adapter an option or device name means ("4player" and
    /// "protovision" are the Classical Games / Protovision one).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "cga" | "4player" | "protovision" => Self::Cga,
            "hit" | "hitmen" => Self::Hit,
            "kingsoft" => Self::Kingsoft,
            "starbyte" => Self::Starbyte,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cga => "cga",
            Self::Hit => "hit",
            Self::Kingsoft => "kingsoft",
            Self::Starbyte => "starbyte",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Cga => 0,
            Self::Hit => 1,
            Self::Kingsoft => 2,
            Self::Starbyte => 3,
        }
    }

    /// Up, down, left, right and fire of joystick `n` (0 or 1). The
    /// Protovision adapter's directions are switched by PB7 (see `lines`).
    fn wiring(self, n: usize) -> [Line; 5] {
        use Line::{Pa2, Pb, Sp2};
        match (self, n) {
            (Self::Cga, 0) => [Pb(0), Pb(1), Pb(2), Pb(3), Pb(4)],
            (Self::Cga, _) => [Pb(0), Pb(1), Pb(2), Pb(3), Pb(5)],
            (Self::Hit, 0) => [Pb(0), Pb(1), Pb(2), Pb(3), Pa2],
            (Self::Hit, _) => [Pb(4), Pb(5), Pb(6), Pb(7), Sp2],
            (Self::Kingsoft, 0) => [Pa2, Pb(7), Pb(6), Pb(5), Pb(4)],
            (Self::Kingsoft, _) => [Pb(3), Pb(2), Pb(1), Pb(0), Sp2],
            (Self::Starbyte, 0) => [Pb(3), Pb(0), Pb(1), Pb(2), Sp2],
            (Self::Starbyte, _) => [Pa2, Pb(5), Pb(6), Pb(7), Pb(4)],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FourPlayer {
    kind: FourPlayerKind,
    /// Joysticks 3 and 4: directions and buttons.
    sticks: [(Dirs, u8); 2],
    /// The computer drives PB7 low (the Protovision adapter's select).
    pb7_low: bool,
}

impl FourPlayer {
    pub fn new(kind: FourPlayerKind) -> Self {
        Self {
            kind,
            sticks: [(Dirs::NONE, 0); 2],
            pb7_low: false,
        }
    }

    pub fn adapter(&self) -> FourPlayerKind {
        self.kind
    }

    fn lines(&self) -> u32 {
        let mut low = if self.kind == FourPlayerKind::Cga {
            0
        } else {
            user_port::CNT_BRIDGE
        };
        for (n, (dirs, buttons)) in self.sticks.iter().enumerate() {
            let wiring = self.kind.wiring(n);
            let held = [
                dirs.contains(Dirs::UP),
                dirs.contains(Dirs::DOWN),
                dirs.contains(Dirs::LEFT),
                dirs.contains(Dirs::RIGHT),
                buttons & 1 != 0,
            ];
            for (i, (line, held)) in wiring.into_iter().zip(held).enumerate() {
                // The Protovision adapter passes the directions of the
                // joystick PB7 selects: high joystick 3, low joystick 4.
                let switched = self.kind == FourPlayerKind::Cga && i < 4;
                if !held || (switched && (n == 1) != self.pb7_low) {
                    continue;
                }
                low |= match line {
                    Line::Pb(bit) => user_port::pb_lines(!(1 << bit)),
                    Line::Pa2 => user_port::PA2,
                    Line::Sp2 => user_port::SP2,
                };
            }
        }
        low
    }
}

impl PortDevice for FourPlayer {
    fn kind(&self) -> PortKind {
        PortKind::CbmUserPort
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        PortLines {
            pulled_low: self.lines(),
            analog: [u16::MAX; 2],
        }
    }

    fn write_lines(&mut self, lines: PortLines, _now: Cycle) {
        self.pb7_low = user_port::pb(lines.pulled_low) & 0x80 == 0;
    }

    fn input(&mut self, ev: &InputEvent) {
        self.input_at(0, ev);
    }

    fn inputs(&self) -> u8 {
        2
    }

    fn input_at(&mut self, n: u8, ev: &InputEvent) {
        if let (Some(stick), InputEvent::Digital { dirs, buttons }) =
            (self.sticks.get_mut(usize::from(n)), ev)
        {
            *stick = (*dirs, *buttons);
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"4PLY", 1, |w| {
            w.u8(self.kind.code());
            for (dirs, buttons) in self.sticks {
                w.u8(dirs.0);
                w.u8(buttons);
            }
            w.bool(self.pb7_low);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"4PLY", 1)?;
        if c.u8()? != self.kind.code() {
            return Err(StateError::Invalid("four-player adapter differs"));
        }
        let mut sticks = self.sticks;
        for s in &mut sticks {
            *s = (Dirs(c.u8()? & 0x0f), c.u8()? & 0x07);
        }
        let pb7_low = c.bool()?;
        self.sticks = sticks;
        self.pb7_low = pb7_low;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stick(d: &mut FourPlayer, n: u8, dirs: Dirs, fire: bool) {
        d.input_at(
            n,
            &InputEvent::Digital {
                dirs,
                buttons: u8::from(fire),
            },
        );
    }

    /// Port B as the computer reads it, with its own output `out`.
    fn pb(d: &mut FourPlayer, out: u8) -> u8 {
        d.write_lines(
            PortLines {
                pulled_low: user_port::pb_lines(out),
                analog: [0; 2],
            },
            Cycle(0),
        );
        user_port::pb(d.read_lines(Cycle(0)).pulled_low)
    }

    #[test]
    fn protovision_switches_the_directions_with_pb7() {
        let mut d = FourPlayer::new(FourPlayerKind::Cga);
        assert_eq!(d.inputs(), 2);
        assert_eq!(pb(&mut d, 0xff), 0xff);
        stick(&mut d, 0, Dirs::UP | Dirs::RIGHT, true);
        stick(&mut d, 1, Dirs::DOWN, true);
        // PB7 high: joystick 3; both fire lines show at any time.
        assert_eq!(pb(&mut d, 0xff), !0x09 & !0x30);
        assert_eq!(pb(&mut d, 0x7f), !0x02 & !0x30);
        assert_eq!(d.read_lines(Cycle(0)).pulled_low & user_port::CNT_BRIDGE, 0);
        // `input` is joystick 3.
        d.input(&InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        });
        assert_eq!(pb(&mut d, 0xff), !0x20);
    }

    #[test]
    fn the_other_adapters_use_pa2_and_the_serial_port() {
        let all = Dirs::UP | Dirs::DOWN | Dirs::LEFT | Dirs::RIGHT;
        for (kind, n, dirs, pb_low, extra) in [
            (FourPlayerKind::Hit, 0, Dirs::LEFT, 0x04, 0),
            (FourPlayerKind::Hit, 1, Dirs::UP, 0x10, 0),
            (FourPlayerKind::Kingsoft, 0, Dirs::UP, 0x00, user_port::PA2),
            (FourPlayerKind::Kingsoft, 0, Dirs::DOWN, 0x80, 0),
            (FourPlayerKind::Kingsoft, 1, Dirs::RIGHT, 0x01, 0),
            (FourPlayerKind::Starbyte, 0, Dirs::UP, 0x08, 0),
            (FourPlayerKind::Starbyte, 1, Dirs::UP, 0x00, user_port::PA2),
            (FourPlayerKind::Starbyte, 1, Dirs::RIGHT, 0x80, 0),
        ] {
            let mut d = FourPlayer::new(kind);
            stick(&mut d, n, dirs, false);
            let low = d.read_lines(Cycle(0)).pulled_low;
            assert_eq!(!user_port::pb(low), pb_low, "{kind:?} {n}");
            assert_eq!(
                low & (user_port::PA2 | user_port::SP2),
                extra,
                "{kind:?} {n}"
            );
            assert_ne!(low & user_port::CNT_BRIDGE, 0);
        }
        // The buttons.
        for (kind, n, line) in [
            (FourPlayerKind::Hit, 0, user_port::PA2),
            (FourPlayerKind::Hit, 1, user_port::SP2),
            (FourPlayerKind::Kingsoft, 0, user_port::pb_lines(!0x10)),
            (FourPlayerKind::Kingsoft, 1, user_port::SP2),
            (FourPlayerKind::Starbyte, 0, user_port::SP2),
            (FourPlayerKind::Starbyte, 1, user_port::pb_lines(!0x10)),
        ] {
            let mut d = FourPlayer::new(kind);
            stick(&mut d, n, Dirs::NONE, true);
            let low = d.read_lines(Cycle(0)).pulled_low & !user_port::CNT_BRIDGE;
            assert_eq!(low, line, "{kind:?} {n}");
        }
        // Every direction of both joysticks has its own line.
        let mut d = FourPlayer::new(FourPlayerKind::Hit);
        stick(&mut d, 0, all, false);
        stick(&mut d, 1, all, false);
        assert_eq!(user_port::pb(d.read_lines(Cycle(0)).pulled_low), 0);
    }

    #[test]
    fn names_and_savestate() {
        for name in FourPlayerKind::NAMES {
            assert_eq!(FourPlayerKind::parse(name).unwrap().name(), name);
        }
        assert_eq!(FourPlayerKind::parse("4player"), Some(FourPlayerKind::Cga));
        assert_eq!(FourPlayerKind::parse("multitap"), None);
        let mut d = FourPlayer::new(FourPlayerKind::Cga);
        stick(&mut d, 1, Dirs::LEFT, true);
        pb(&mut d, 0x7f);
        let state = d.save_state();
        let mut e = FourPlayer::new(FourPlayerKind::Cga);
        e.load_state(&state).unwrap();
        assert_eq!(e, d);
        let mut other = FourPlayer::new(FourPlayerKind::Hit);
        assert!(other.load_state(&state).is_err());
        assert!(e.load_state(&state[..4]).is_err());
    }
}
