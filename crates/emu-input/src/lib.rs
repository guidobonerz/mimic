//! Host input: keyboards and gamepads mapped onto emulated devices.
//!
//! Host-neutral [`HostEvent`]s come from the frontend (winit, browser,
//! gilrs) and are turned into machine [`emu_core::input::TimedInput`]s by a
//! [`Mapper`] following a [`Profile`]. Machines never see host devices
//! (golden rule 10); hot-plugging a gamepad only changes the mapping.

#![forbid(unsafe_code)]

pub mod keymap;
pub mod mapper;
pub mod profile;

use serde::Deserialize;

pub use mapper::Mapper;
pub use profile::Profile;

/// Gamepad buttons in a layout-neutral naming (position, not label).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
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
}
