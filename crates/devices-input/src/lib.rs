//! Emulated input peripherals: joysticks, paddles, mice, light pens, keypads.

#![forbid(unsafe_code)]

pub mod joystick;
pub mod mouse1351;
pub mod paddles;

pub use joystick::Joystick;
pub use mouse1351::Mouse1351;
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
