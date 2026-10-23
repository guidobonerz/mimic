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
