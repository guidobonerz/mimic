//! Physical connectors and the devices plugged into them.

use std::fmt;

use crate::cycle::Cycle;
use crate::input::InputEvent;
use crate::savestate::StateError;

/// Connector type; a port only accepts devices of the same kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PortKind {
    /// C64/C128/VIC-20/Plus4: 9-pin, digital + POTX/POTY analog
    /// (the Plus/4 uses a mini-DIN connector).
    CbmControl,
    /// 9-pin, digital + quadrature mouse + POT lines.
    AmigaGameport,
    /// 9-pin, digital + paddles via POKEY POT.
    AtariJoy,
    /// 9-pin, two joysticks via daisy-chain, AMX mouse.
    CpcJoy,
    /// ColecoVision controller port: 9-pin, joystick and keypad segments
    /// selected by the console, a quadrature input (spinner, wheel,
    /// trackball).
    ColecoControl,
    /// NES controller port: OUT (strobe) and CLK from the console, data
    /// lines D0, D3, D4 back (standard controller, Zapper, Arkanoid).
    NesControl,
    /// SNES controller port: latch, CLK and IOBit from the console, data
    /// lines D0/D1 back (joypad, mouse, Super Scope).
    SnesControl,
    /// Vectrex controller port: four buttons and two potentiometers (the
    /// analog stick), read by the console's DAC and comparator.
    VectrexControl,
    /// Apple II game I/O: four paddle inputs (two used by a joystick or a
    /// pair of paddles), push buttons, annunciators.
    AppleGame,
    /// Macintosh 128K/512K/Plus mouse port: 9-pin, quadrature X1/X2 and
    /// Y1/Y2 and one button (the SE's ADB mouse uses the same device).
    MacMouse,
    /// C64/C128/VIC-20 user port: CIA port B (PB0–PB7), PA2, the PC2
    /// handshake out and FLAG2 in, the serial lines (see
    /// [`user_port`] for the pin numbering in [`PortLines`]).
    CbmUserPort,
    Iec,
    Sio,
    Cartridge,
    Datasette,
    AmigaFloppy,
    CentronicsPrinter,
}

/// Electrical state of a connector at one instant.
///
/// Digital lines are open-collector on most of these ports, so the state is
/// expressed as the set of pins pulled low: bit `n` = pin `n + 1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PortLines {
    pub pulled_low: u32,
    /// Analog lines (POTX/POTY, paddles), 0 = minimum resistance.
    pub analog: [u16; 2],
}

/// A peripheral working at pin level with cycle timestamps.
pub trait PortDevice: Send {
    fn kind(&self) -> PortKind;

    /// What the device drives onto the pins at `now`.
    fn read_lines(&self, now: Cycle) -> PortLines;

    /// What the machine drives onto the pins at `now`.
    fn write_lines(&mut self, lines: PortLines, now: Cycle);

    /// Host-neutral input from the host layer.
    fn input(&mut self, ev: &InputEvent);

    /// Host services for devices that reach outside the machine (the
    /// WiC64's network); passed on by [`crate::Machine::attach_host`].
    /// Pin-level devices ignore them (the default).
    fn attach_host(&mut self, host: &crate::host::HostServices) {
        let _ = host;
    }

    fn save_state(&self) -> Vec<u8>;

    /// Corrupt data is rejected with an error and leaves the device unchanged.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub id: PortId,
    pub kind: PortKind,
    /// Hardware name, e.g. "control port 2", "gameport 0".
    pub name: &'static str,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortError {
    Incompatible { port: PortKind, device: PortKind },
    NoSuchPort(PortId),
    Occupied(PortId),
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortError::Incompatible { port, device } => {
                write!(f, "{device:?} device does not fit a {port:?} port")
            }
            PortError::NoSuchPort(id) => write!(f, "no port {}", id.0),
            PortError::Occupied(id) => write!(f, "port {} is already in use", id.0),
        }
    }
}

impl std::error::Error for PortError {}

/// The Commodore user port's pins in [`PortLines::pulled_low`]: bit `n`
/// is pin `n + 1` on the top row (1–12); the bottom row A–N (no G, I)
/// follows as pins 13–24. Source: C64 Programmer's Reference Guide,
/// appendix I (user port pinout).
pub mod user_port {
    /// Pin 8: CIA2 PC2, low for one cycle after each access to port B.
    pub const PC2: u32 = 1 << 7;
    /// Pin B: CIA2 /FLAG input (negative edge sets ICR bit 4).
    pub const FLAG2: u32 = 1 << 13;
    /// Pins C–L: CIA2 PB0–PB7, shifted by this many bits.
    pub const PB_SHIFT: u32 = 14;
    /// Pin M: CIA2 PA2.
    pub const PA2: u32 = 1 << 22;

    /// The port B byte from lines (a bit pulled low reads 0).
    pub fn pb(pulled_low: u32) -> u8 {
        !((pulled_low >> PB_SHIFT) as u8)
    }

    /// Lines pulling the port B bits that are 0 in `v` low.
    pub fn pb_lines(v: u8) -> u32 {
        u32::from(!v) << PB_SHIFT
    }
}
