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
        /// The left stick also goes out as an analog position
        /// (`InputEvent::Analog`: axis 0 X from 0 left to 65535 right,
        /// axis 1 Y from 0 down to 65535 up) and no longer as directions;
        /// the D-pad stays digital. For analog sticks such as the
        /// Vectrex's.
        #[serde(default)]
        analog: bool,
    },
}

fn default_fire() -> Vec<Button> {
    vec![Button::South]
}

fn default_deadzone() -> f32 {
    0.5
}

/// Which host keys play the keyboard joystick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JoyKeys {
    Off,
    /// Numeric keypad 8/2/4/6, fire on 0 or right Ctrl.
    Numpad,
    /// Cursor keys, fire on Space.
    Cursor,
}

impl JoyKeys {
    pub const ALL: [JoyKeys; 3] = [JoyKeys::Off, JoyKeys::Numpad, JoyKeys::Cursor];

    pub fn name(self) -> &'static str {
        match self {
            JoyKeys::Off => "off",
            JoyKeys::Numpad => "numpad",
            JoyKeys::Cursor => "cursor",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The key source, `None` for [`JoyKeys::Off`].
    pub fn source(self) -> Option<JoystickSource> {
        let keys = |up: &str, down: &str, left: &str, right: &str, fire: &[&str]| {
            Some(JoystickSource::Keys {
                up: up.into(),
                down: down.into(),
                left: left.into(),
                right: right.into(),
                fire: fire.iter().map(|f| (*f).into()).collect(),
            })
        };
        match self {
            JoyKeys::Off => None,
            JoyKeys::Numpad => keys(
                "Numpad8",
                "Numpad2",
                "Numpad4",
                "Numpad6",
                &["Numpad0", "ControlRight"],
            ),
            JoyKeys::Cursor => keys(
                "ArrowUp",
                "ArrowDown",
                "ArrowLeft",
                "ArrowRight",
                &["Space"],
            ),
        }
    }

    /// The default for a machine: cursor keys where there is no keyboard to
    /// type on (Atari 2600), none on the CPC (its numeric keypad is the
    /// function keys), the numeric keypad elsewhere.
    pub fn default_for(machine: &str) -> Self {
        match machine {
            "atari2600" | "coleco" | "nes" | "snes" | "vectrex" => JoyKeys::Cursor,
            "cpc" => JoyKeys::Off,
            _ => JoyKeys::Numpad,
        }
    }
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

    /// Built-in default: positional keyboard; a joystick (for the C64 in
    /// control port 2) on the first gamepad and on the keys of
    /// [`JoyKeys::default_for`].
    pub fn default_for(machine: &str) -> Self {
        // The C64's game port is control port 2; the VIC-20 has one port; the
        // C16/Plus/4 joystick comes plugged into port 1, as do most Atari
        // games' joysticks.
        let port = match machine {
            "c64" | "c128" => Some(1),
            "vic20" | "c16" | "plus4" | "atari8" | "atari2600" | "cpc" | "coleco" | "nes"
            | "snes" | "vectrex" | "apple2" => Some(0),
            // The Amiga's joystick port is game port 1 (port 0 is the mouse's).
            "amiga" => Some(1),
            _ => None,
        };
        let mut joystick = Vec::new();
        if let Some(port) = port {
            if let Some(source) = JoyKeys::default_for(machine).source() {
                joystick.push(JoystickMap { port, source });
            }
            joystick.push(JoystickMap {
                port,
                source: JoystickSource::Gamepad {
                    pad: 0,
                    fire: default_fire(),
                    deadzone: default_deadzone(),
                    // The Vectrex's and the Apple II's sticks are analog.
                    analog: matches!(machine, "vectrex" | "apple2"),
                },
            });
        }
        Self {
            machine: machine.into(),
            keyboard: KeyboardMode::Positional,
            joystick,
            // The Amiga's mouse sits in game port 0, the Mac's in its
            // mouse port.
            mouse: if matches!(machine, "amiga" | "mac") {
                vec![MouseMap { port: 0 }]
            } else {
                Vec::new()
            },
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
                deadzone: 0.5,
                analog: false
            }
        );
    }

    #[test]
    fn rejects_typos() {
        assert!(Profile::from_toml("machine = \"c64\"\nkeybord = \"none\"").is_err());
        assert!(Profile::from_toml("machine = \"c64\"\nkeyboard = \"symbolic\"").is_err());
    }
}
