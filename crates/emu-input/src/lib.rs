//! Host input: keyboards and gamepads mapped onto emulated devices.
//!
//! Host-neutral [`HostEvent`]s come from the frontend (winit, browser,
//! gilrs) and are turned into machine [`emu_core::input::TimedInput`]s by a
//! [`Mapper`] following a [`Profile`]. Machines never see host devices
//! (golden rule 10); hot-plugging a gamepad only changes the mapping.

#![forbid(unsafe_code)]

#[cfg(test)]
mod names_tests {
    use super::*;

    #[test]
    fn buttons_and_axes_by_name() {
        assert_eq!(Button::from_name("South"), Some(Button::South));
        assert_eq!(Button::from_name("DPadLeft"), Some(Button::DPadLeft));
        assert_eq!(Button::from_name("Turbo"), None);
        assert_eq!(Axis::from_name("LeftY"), Some(Axis::LeftY));
    }
}

pub mod keymap;
pub mod mapper;
pub mod profile;

use serde::{Deserialize, Serialize};

pub use mapper::Mapper;
pub use profile::{JoyKeys, Profile};

/// Gamepad buttons in a layout-neutral naming (position, not label).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub enum Button {
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    Select,
    Start,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    LeftX,
    /// Positive values point down.
    LeftY,
}

impl Button {
    pub const ALL: [Button; 12] = [
        Button::South,
        Button::East,
        Button::West,
        Button::North,
        Button::LeftShoulder,
        Button::RightShoulder,
        Button::Select,
        Button::Start,
        Button::DPadUp,
        Button::DPadDown,
        Button::DPadLeft,
        Button::DPadRight,
    ];

    /// By its name as written here (`"South"`, `"DPadUp"` …).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| format!("{b:?}") == name)
    }
}

impl Axis {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "LeftX" => Some(Axis::LeftX),
            "LeftY" => Some(Axis::LeftY),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    /// A physical key, named like W3C `KeyboardEvent.code` ("KeyA", "Enter").
    Key {
        code: String,
        down: bool,
    },
    Button {
        pad: u8,
        button: Button,
        down: bool,
    },
    /// Stick position in -1.0 ..= 1.0.
    Axis {
        pad: u8,
        axis: Axis,
        value: f32,
    },
    /// Motion of the captured host mouse (`dx` right, `dy` down) and its
    /// buttons (bit 0 left, bit 1 right).
    Mouse {
        dx: i16,
        dy: i16,
        buttons: u8,
    },
    /// The host pointer over the machine's picture, not captured: its
    /// position in frame pixels of the main picture (border included;
    /// `None` off the picture) and its buttons (bit 0 left, bit 1 right).
    /// Aims a light pen or light gun (see [`profile::PointerMap`]).
    Pointer {
        pos: Option<(u16, u16)>,
        buttons: u8,
    },
}
