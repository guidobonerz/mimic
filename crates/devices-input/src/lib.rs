//! Emulated input peripherals: joysticks, paddles, mice, light pens, keypads,
//! the ColecoVision controllers, the NES controllers.

#![forbid(unsafe_code)]

pub mod amiga_mouse;
pub mod amx_mouse;
pub mod coleco;
pub mod driving;
pub mod joystick;
pub mod keypad;
pub mod mouse1351;
pub mod nes;
pub mod paddles;

pub use amiga_mouse::AmigaMouse;
pub use amx_mouse::AmxMouse;
pub use coleco::{ColecoController, ColecoKind};
pub use driving::Driving;
pub use joystick::Joystick;
pub use keypad::Keypad;
pub use mouse1351::Mouse1351;
pub use nes::{NesController, NesKind};
pub use paddles::Paddles;

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
    match option.unwrap_or(default) {
        "joystick" => Ok(Some(Box::new(Joystick::new(PortKind::CbmControl)))),
        "paddles" if analog => Ok(Some(Box::new(Paddles::new()))),
        "mouse1351" if analog => Ok(Some(Box::new(Mouse1351::new()))),
        "none" => Ok(None),
        other => Err(ConfigError::Invalid(format!(
            "{name} option '{other}' ({})",
            if analog {
                "joystick, paddles, mouse1351 or none"
            } else {
                "joystick or none"
            }
        ))),
    }
}
