//! Emulated input peripherals: joysticks, paddles, mice, light pens (C64/C128,
//! Atari 8-bit, Amiga, CPC), keypads,
//! the ColecoVision controllers, the NES and SNES controllers, the Apple II
//! joystick and paddles, the ZX Spectrum's Kempston mouse.

#![forbid(unsafe_code)]

pub mod amiga_mouse;
pub mod amx_mouse;
pub mod apple;
pub mod coleco;
pub mod driving;
pub mod joystick;
pub mod kempston_mouse;
pub mod keypad;
pub mod lightpen;
pub mod mouse1351;
pub mod nes;
pub mod paddles;
pub mod snes;
pub mod vectrex;

pub use amiga_mouse::AmigaMouse;
pub use amx_mouse::AmxMouse;
pub use apple::{AppleJoystick, AppleKind};
pub use coleco::{ColecoController, ColecoKind};
pub use driving::Driving;
pub use joystick::Joystick;
pub use kempston_mouse::KempstonMouse;
pub use keypad::Keypad;
pub use lightpen::{LightPen, LightPenKind};
pub use mouse1351::Mouse1351;
pub use nes::{NesController, NesKind};
pub use paddles::Paddles;
pub use snes::{SnesController, SnesKind};
pub use vectrex::{Imager, VectrexController, VectrexKind};

use emu_core::port::{PortDevice, PortKind};
use emu_core::registry::ConfigError;

/// The device a CBM control port option names: "joystick", "paddles",
/// "mouse1351" (only where the port has POT lines, `analog`) or "none";
/// without the option `default`. `name` is the option, for messages.
pub fn control_port_option(
    name: &str,
    option: Option<&str>,
    default: &str,
    analog: bool,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    cbm_port_option(name, option, default, analog, false)
}

/// [`control_port_option`], and with `light_pen` also "lightpen" (the
/// port carries the video chip's light-pen input: C64/C128 control port 1).
pub fn cbm_port_option(
    name: &str,
    option: Option<&str>,
    default: &str,
    analog: bool,
    light_pen: bool,
) -> Result<Option<Box<dyn PortDevice>>, ConfigError> {
    match option.unwrap_or(default) {
        "joystick" => Ok(Some(Box::new(Joystick::new(PortKind::CbmControl)))),
        "paddles" if analog => Ok(Some(Box::new(Paddles::new()))),
        "mouse1351" if analog => Ok(Some(Box::new(Mouse1351::new()))),
        "lightpen" if light_pen => Ok(Some(Box::new(LightPen::new(LightPenKind::Cbm)))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{name} option '{other}' ({})",
            match (analog, light_pen) {
                (true, true) => "joystick, paddles, mouse1351, lightpen or none",
                (true, false) => "joystick, paddles, mouse1351 or none",
                (false, true) => "joystick, lightpen or none",
                (false, false) => "joystick or none",
            }
        ))),
    }
}
