//! Mapping profiles (TOML): which host inputs drive which emulated device.
//!
//! ```toml
//! machine = "c64"
//! keyboard = "positional"          # or "none"
//!
//! [[joystick]]                     # keyboard as joystick
//! port = 1                         # PortId: C64 control port 2
//! source = "keys"
//! up = "Numpad8"
//! down = "Numpad2"
//! left = "Numpad4"
//! right = "Numpad6"
//! fire = ["Numpad0", "ControlRight"]
//!
//! [[joystick]]                     # gamepad
//! port = 1
//! source = "gamepad"
//! pad = 0
//! fire = ["South"]
//! deadzone = 0.5
//!
//! [[mouse]]                        # captured host mouse: 1351 or paddles
//! port = 0
//!
//! [[paddles]]                      # gamepad stick X/Y turns paddle X/Y,
//! port = 0                         # South/East are their buttons
//! pad = 0
//! ```

use serde::Deserialize;

use crate::Button;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyboardMode {
    #[default]
    Positional,
    None,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum JoystickSource {
    Keys {
        up: String,
        down: String,
        left: String,
        right: String,
        fire: Vec<String>,
    },
    Gamepad {
        #[serde(default)]
        pad: u8,
        #[serde(default = "default_fire")]
        fire: Vec<Button>,
        #[serde(default = "default_deadzone")]
        deadzone: f32,
    },
}

fn default_fire() -> Vec<Button> {
    vec![Button::South]
}

fn default_deadzone() -> f32 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct JoystickMap {
    /// Target port (`PortId`) of the machine.
    pub port: u8,
    #[serde(flatten)]
    pub source: JoystickSource,
}

/// The captured host mouse drives the device in a port (a mouse, or the
/// paddles: horizontal motion turns paddle X).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MouseMap {
    pub port: u8,
}

/// A gamepad's left stick turns a pair of paddles.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PaddleMap {
    pub port: u8,
    #[serde(default)]
    pub pad: u8,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub machine: String,
    #[serde(default)]
    pub keyboard: KeyboardMode,
    #[serde(default)]
    pub joystick: Vec<JoystickMap>,
    #[serde(default)]
    pub mouse: Vec<MouseMap>,
    #[serde(default)]
    pub paddles: Vec<PaddleMap>,
}

impl Profile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Built-in default: positional keyboard; for the C64 a joystick in
    /// control port 2 on the numeric keypad and on the first gamepad.
    pub fn default_for(machine: &str) -> Self {
        // The C64's game port is control port 2; the VIC-20 has one port; the
        // C16/Plus/4 joystick comes plugged into port 1, as do most Atari
        // games' joysticks.
        let port = match machine {
            "c64" | "c128" => Some(1),
            "vic20" | "c16" | "plus4" | "atari800xl" | "cpc" => Some(0),
            _ => None,
        };
        // The CPC's numeric keypad is its function keys: gamepad only.
        let keys = machine != "cpc";
        let mut joystick = match port {
            Some(port) => vec![
                JoystickMap {
                    port,
                    source: JoystickSource::Keys {
                        up: "Numpad8".into(),
                        down: "Numpad2".into(),
                        left: "Numpad4".into(),
                        right: "Numpad6".into(),
                        fire: vec!["Numpad0".into(), "ControlRight".into()],
                    },
                },
                JoystickMap {
                    port,
                    source: JoystickSource::Gamepad {
                        pad: 0,
                        fire: default_fire(),
                        deadzone: default_deadzone(),
                    },
                },
            ],
            None => Vec::new(),
        };
        if !keys {
            joystick.retain(|j| !matches!(j.source, JoystickSource::Keys { .. }));
        }
        Self {
            machine: machine.into(),
            keyboard: KeyboardMode::Positional,
            joystick,
            mouse: Vec::new(),
            paddles: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_example() {
        let p = Profile::from_toml(
            r#"
            machine = "c64"
            keyboard = "positional"

            [[joystick]]
            port = 1
            source = "keys"
            up = "Numpad8"
            down = "Numpad2"
            left = "Numpad4"
            right = "Numpad6"
            fire = ["Numpad0", "ControlRight"]

            [[joystick]]
            port = 0
            source = "gamepad"
            pad = 1
            "#,
        )
        .unwrap();
        assert_eq!(p.joystick.len(), 2);
        assert_eq!(
            p.joystick[1].source,
            JoystickSource::Gamepad {
                pad: 1,
                fire: vec![Button::South],
                deadzone: 0.5
            }
        );
    }

    #[test]
    fn rejects_typos() {
        assert!(Profile::from_toml("machine = \"c64\"\nkeybord = \"none\"").is_err());
        assert!(Profile::from_toml("machine = \"c64\"\nkeyboard = \"symbolic\"").is_err());
    }
}
