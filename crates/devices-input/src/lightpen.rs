//! Light pens for the C64/C128, the Atari 8-bit computers, the Amiga and
//! the Amstrad CPC.
//!
//! A light pen is a photodiode behind a lens: when the electron beam
//! passes under its tip and lights the phosphor there, the pen sends a
//! pulse to the video chip, which latches its beam counters (Commodore 64
//! Programmer's Reference Guide, "Light pen"; Amiga Hardware Reference
//! Manual, "Reading the light pen"). Whether the phosphor under the tip
//! lights up depends on the picture, which only the machine knows, so the
//! device reports where it points and the machine decides, while it draws,
//! when the beam passes there and whether the pixel is bright enough
//! ([`lit`]); then it latches the chip's light-pen registers at that
//! moment — the same split as the NES Zapper's (`crate::nes`).
//!
//! **Lines.** The sensor's output is the light-pen input of the port:
//! C64/C128 control port 1 pin 6 (shared with the fire button and CIA 1
//! PB4) to the VIC-II's LP; Atari 8-bit joystick pin 6 (trigger) to
//! ANTIC's light-pen input; Amiga game port 0 pin 6 (LPENTG) to Agnus;
//! CPC expansion port pin 47 (LPEN) to the CRTC's LPSTB. The device sets
//! [`pin::SENSOR`] and carries its aim in bits no connector has
//! ([`pin::X_SHIFT`], [`pin::Y_SHIFT`]; read them with [`aim`]), so a
//! machine without a light-pen input sees an open port and the analog
//! lines stay open (no paddle values).
//!
//! **Button** (the pen's switch), per kind:
//! - [`LightPenKind::Cbm`]: pin 3 (joystick left). Pens differed; this is
//!   a common wiring (pin 6 would latch the VIC-II itself). Not verified
//!   against a particular pen.
//! - [`LightPenKind::Atari`]: pin 1 (joystick up), as the CX70/CX75 pens
//!   are read with `STICK` — this project's reading, not verified.
//! - [`LightPenKind::Amiga`]: pin 5 (LPENPR, the POTX line; HRM table
//!   "Connector pin usage for light pen inputs"; the manual notes some
//!   makers swap pins 5 and 6).
//! - [`LightPenKind::Cpc`]: the expansion port has no line for it; the pen
//!   only strobes while its tip switch (the button) is held — this
//!   project's design choice for a pen on the expansion port.
//!
//! Input: [`InputEvent::LightPen`] (aim in frame pixels of the machine's
//! main picture, `trigger` = button; x or y $FFFF = off the screen),
//! [`InputEvent::MouseRel`] (moves the aim, button 0 the switch),
//! [`InputEvent::Digital`] button bit 0.

use emu_core::frame::Rgb8;
use emu_core::input::InputEvent;
use emu_core::port::{PortDevice, PortKind, PortLines};
use emu_core::savestate::{StateError, StateReader, StateWriter};
use emu_core::Cycle;

use crate::joystick;

pub mod pin {
    /// The device is a light pen; the aim is in the bits below.
    pub const SENSOR: u32 = 1 << 30;
    /// Aim column: 11 bits from here ([`NO_X`] = off the screen).
    pub const X_SHIFT: u32 = 10;
    /// Aim row: 9 bits from here.
    pub const Y_SHIFT: u32 = 21;
    pub const X_MASK: u32 = 0x7ff;
    pub const Y_MASK: u32 = 0x1ff;
    /// Column value for "not pointing at the picture".
    pub const NO_X: u32 = 0x7ff;
}

/// Largest aim the lines can carry.
pub const MAX_X: u16 = 0x7fe;
pub const MAX_Y: u16 = 0x1ff;

/// Off-screen aim in [`InputEvent::LightPen`].
pub const NO_AIM: u16 = 0xffff;

/// Rec. 601 luma (0–255) from which a pixel's phosphor is bright enough
/// for the pen's photodiode. The C64's light blue text (luma ≈ 107) and
/// white trigger it, its dark blue background (≈ 52) and black do not.
pub const LIGHT_LUMA: u32 = 80;

/// Does a pixel of colour `c` light the pen?
pub fn lit(c: Rgb8) -> bool {
    (299 * u32::from(c.r) + 587 * u32::from(c.g) + 114 * u32::from(c.b)) / 1000 >= LIGHT_LUMA
}

/// Which pixels of an indexed palette light the pen.
pub fn lit_colours(palette: &[Rgb8]) -> Vec<bool> {
    palette.iter().map(|&c| lit(c)).collect()
}

/// Where a light pen points, in frame pixels of the machine's main picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Aim {
    pub x: u16,
    pub y: u16,
}

/// The aim of the light pen whose lines these are; `None` for another
/// device, or a pen that points off the picture (or does not strobe: the
/// CPC pen without its switch).
pub fn aim(lines: &PortLines) -> Option<Aim> {
    let l = lines.pulled_low;
    if l & pin::SENSOR == 0 {
        return None;
    }
    let x = l >> pin::X_SHIFT & pin::X_MASK;
    if x == pin::NO_X {
        return None;
    }
    Some(Aim {
        x: x as u16,
        y: (l >> pin::Y_SHIFT & pin::Y_MASK) as u16,
    })
}

/// Is the device on these lines a light pen?
pub fn is_light_pen(lines: &PortLines) -> bool {
    lines.pulled_low & pin::SENSOR != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightPenKind {
    /// C64/C128 control port 1.
    Cbm,
    /// Atari 400/800/XL/XE joystick port.
    Atari,
    /// Amiga game port 0.
    Amiga,
    /// Amstrad CPC expansion port (a [`PortKind::Cartridge`] connector).
    Cpc,
}

impl LightPenKind {
    fn code(self) -> u8 {
        match self {
            LightPenKind::Cbm => 0,
            LightPenKind::Atari => 1,
            LightPenKind::Amiga => 2,
            LightPenKind::Cpc => 3,
        }
    }

    pub fn port_kind(self) -> PortKind {
        match self {
            LightPenKind::Cbm => PortKind::CbmControl,
            LightPenKind::Atari => PortKind::AtariJoy,
            LightPenKind::Amiga => PortKind::AmigaGameport,
            LightPenKind::Cpc => PortKind::Cartridge,
        }
    }

    /// The pen for a port of `kind`, if one fits.
    pub fn for_port(kind: PortKind) -> Option<Self> {
        match kind {
            PortKind::CbmControl => Some(LightPenKind::Cbm),
            PortKind::AtariJoy => Some(LightPenKind::Atari),
            PortKind::AmigaGameport => Some(LightPenKind::Amiga),
            PortKind::Cartridge => Some(LightPenKind::Cpc),
            _ => None,
        }
    }

    /// The pin the pen's button pulls low (none on the CPC).
    fn button_pin(self) -> u32 {
        match self {
            LightPenKind::Cbm => joystick::pin::LEFT,
            LightPenKind::Atari => joystick::pin::UP,
            LightPenKind::Amiga => joystick::pin::BUTTON3,
            LightPenKind::Cpc => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightPen {
    kind: LightPenKind,
    /// Aim in frame pixels; `NO_AIM` off the screen.
    x: u16,
    y: u16,
    button: bool,
}

impl LightPen {
    pub fn new(kind: LightPenKind) -> Self {
        Self {
            kind,
            x: NO_AIM,
            y: NO_AIM,
            button: false,
        }
    }

    pub fn kind(&self) -> LightPenKind {
        self.kind
    }

    fn on_screen(&self) -> bool {
        self.x <= MAX_X && self.y <= MAX_Y
    }
}

impl PortDevice for LightPen {
    fn kind(&self) -> PortKind {
        self.kind.port_kind()
    }

    fn read_lines(&self, _now: Cycle) -> PortLines {
        let strobes = self.on_screen() && (self.kind != LightPenKind::Cpc || self.button);
        let (x, y) = if strobes {
            (u32::from(self.x), u32::from(self.y))
        } else {
            (pin::NO_X, 0)
        };
        let mut low = pin::SENSOR | x << pin::X_SHIFT | y << pin::Y_SHIFT;
        if self.button {
            low |= self.kind.button_pin();
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    fn write_lines(&mut self, _lines: PortLines, _now: Cycle) {}

    fn input(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::LightPen { x, y, trigger } => {
                if x > MAX_X || y > MAX_Y {
                    self.x = NO_AIM;
                    self.y = NO_AIM;
                } else {
                    self.x = x;
                    self.y = y;
                }
                self.button = trigger;
            }
            InputEvent::MouseRel { dx, dy, buttons } => {
                let (x, y) = if self.on_screen() {
                    (i32::from(self.x), i32::from(self.y))
                } else {
                    (160, 120)
                };
                self.x = (x + i32::from(dx)).clamp(0, i32::from(MAX_X)) as u16;
                self.y = (y + i32::from(dy)).clamp(0, i32::from(MAX_Y)) as u16;
                self.button = buttons & 1 != 0;
            }
            InputEvent::Digital { buttons, .. } => self.button = buttons & 1 != 0,
            _ => {}
        }
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"LPEN", 1, |w| {
            w.u8(self.kind.code());
            w.u16(self.x);
            w.u16(self.y);
            w.bool(self.button);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let mut r = StateReader::new(s);
        let (_, mut c) = r.chunk(*b"LPEN", 1)?;
        if c.u8()? != self.kind.code() {
            return Err(StateError::Invalid("light pen kind differs"));
        }
        let (x, y, button) = (c.u16()?, c.u16()?, c.bool()?);
        let off = x > MAX_X || y > MAX_Y;
        self.x = if off { NO_AIM } else { x };
        self.y = if off { NO_AIM } else { y };
        self.button = button;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(d: &LightPen) -> PortLines {
        d.read_lines(Cycle(0))
    }

    #[test]
    fn reports_its_aim_and_no_analog_values() {
        let mut d = LightPen::new(LightPenKind::Cbm);
        let l = lines(&d);
        assert!(is_light_pen(&l));
        assert_eq!(aim(&l), None, "off the screen at power-on");
        assert_eq!(l.analog, [u16::MAX; 2]);
        d.input(&InputEvent::LightPen {
            x: 200,
            y: 100,
            trigger: false,
        });
        let l = lines(&d);
        assert_eq!(aim(&l), Some(Aim { x: 200, y: 100 }));
        // The aim never shows on the connector's pins.
        assert_eq!(l.pulled_low & 0x1ff, 0);
        d.input(&InputEvent::LightPen {
            x: NO_AIM,
            y: NO_AIM,
            trigger: false,
        });
        assert_eq!(aim(&lines(&d)), None);
    }

    #[test]
    fn button_pins_per_kind() {
        for (kind, pin) in [
            (LightPenKind::Cbm, joystick::pin::LEFT),
            (LightPenKind::Atari, joystick::pin::UP),
            (LightPenKind::Amiga, joystick::pin::BUTTON3),
        ] {
            let mut d = LightPen::new(kind);
            d.input(&InputEvent::LightPen {
                x: 1,
                y: 2,
                trigger: true,
            });
            assert_eq!(lines(&d).pulled_low & 0x1ff, pin, "{kind:?}");
            assert_eq!(PortDevice::kind(&d), kind.port_kind());
            assert_eq!(LightPenKind::for_port(kind.port_kind()), Some(kind));
        }
    }

    #[test]
    fn cpc_pen_strobes_only_with_its_switch() {
        let mut d = LightPen::new(LightPenKind::Cpc);
        d.input(&InputEvent::LightPen {
            x: 300,
            y: 150,
            trigger: false,
        });
        assert_eq!(aim(&lines(&d)), None);
        d.input(&InputEvent::LightPen {
            x: 300,
            y: 150,
            trigger: true,
        });
        assert_eq!(aim(&lines(&d)), Some(Aim { x: 300, y: 150 }));
    }

    #[test]
    fn mouse_moves_the_aim() {
        let mut d = LightPen::new(LightPenKind::Amiga);
        d.input(&InputEvent::MouseRel {
            dx: 10,
            dy: -20,
            buttons: 1,
        });
        assert_eq!(aim(&lines(&d)), Some(Aim { x: 170, y: 100 }));
        d.input(&InputEvent::MouseRel {
            dx: -1000,
            dy: 1000,
            buttons: 0,
        });
        assert_eq!(aim(&lines(&d)), Some(Aim { x: 0, y: MAX_Y }));
    }

    #[test]
    fn light_threshold() {
        assert!(lit(Rgb8 {
            r: 0xff,
            g: 0xff,
            b: 0xff
        }));
        assert!(!lit(Rgb8 { r: 0, g: 0, b: 0 }));
        // C64 blue background vs light blue text (VICE-like palette values).
        assert!(!lit(Rgb8 {
            r: 0x35,
            g: 0x28,
            b: 0x79
        }));
        assert!(lit(Rgb8 {
            r: 0x6c,
            g: 0x5e,
            b: 0xb5
        }));
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = LightPen::new(LightPenKind::Atari);
        d.input(&InputEvent::LightPen {
            x: 33,
            y: 44,
            trigger: true,
        });
        let s = d.save_state();
        let mut e = LightPen::new(LightPenKind::Atari);
        e.load_state(&s).unwrap();
        assert_eq!(e, d);
        let mut other = LightPen::new(LightPenKind::Cpc);
        assert!(other.load_state(&s).is_err());
        assert!(e.load_state(&s[..s.len() - 1]).is_err());
    }
}
