//! Monitor profiles: which connection and how strong each CRT effect is.
//! Built-in presets; users override any field in TOML.
//!
//! ```toml
//! name = "my 1084s"
//! base = "1084s"        # start from a preset
//! scanlines = 0.6
//! ```

use serde::Deserialize;

/// The connection a profile models. Phase 2 implements none of the signal
/// stages yet; composite profiles render like RGB until they arrive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    #[default]
    Rgb,
    CompositePal,
    CompositeNtsc,
    SVideo,
    Monochrome,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub signal: Signal,
    /// Correct the pixel aspect ratio from the signal timing.
    pub aspect: bool,
    /// Scanline darkness 0.0 (none) – 1.0 (black gaps); dark pixels get
    /// thinner beams than bright ones.
    pub scanlines: f32,
    /// Display gamma applied on output (1.0 = unchanged).
    pub gamma: f32,
    /// Tint for monochrome monitors (linear RGB multipliers).
    pub tint: [f32; 3],
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "raw".into(),
            signal: Signal::Rgb,
            aspect: false,
            scanlines: 0.0,
            gamma: 1.0,
            tint: [1.0; 3],
        }
    }
}

impl Profile {
    pub fn preset(name: &str) -> Option<Self> {
        let base = Self {
            name: name.into(),
            ..Self::default()
        };
        Some(match name {
            // Sharp pixels, no correction: what regression tests compare.
            "raw" => base,
            "1702" => Self {
                signal: Signal::SVideo,
                aspect: true,
                scanlines: 0.35,
                gamma: 1.1,
                ..base
            },
            "1084s" => Self {
                signal: Signal::SVideo,
                aspect: true,
                scanlines: 0.45,
                gamma: 1.1,
                ..base
            },
            "pal-tv" => Self {
                signal: Signal::CompositePal,
                aspect: true,
                scanlines: 0.25,
                gamma: 1.15,
                ..base
            },
            _ => return None,
        })
    }

    pub const PRESETS: [&'static str; 4] = ["raw", "1702", "1084s", "pal-tv"];

    /// Parse a TOML profile; `base = "<preset>"` starts from a preset.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct WithBase {
            base: Option<String>,
        }
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let base: WithBase = table
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())?;
        let mut merged = match &base.base {
            Some(b) => {
                let p = Self::preset(b).ok_or_else(|| format!("unknown base profile '{b}'"))?;
                toml::Table::try_from(Serialized::from(&p)).map_err(|e| e.to_string())?
            }
            None => toml::Table::new(),
        };
        for (k, v) in table {
            if k != "base" {
                merged.insert(k, v);
            }
        }
        merged
            .try_into()
            .map_err(|e: toml::de::Error| e.to_string())
    }

    /// The monitor a machine shipped with (desktop default).
    pub fn default_for(machine: &str) -> Self {
        let name = match machine {
            "c64" | "c128" | "vic20" | "c16" | "plus4" => "1702",
            _ => "raw",
        };
        Self::preset(name).unwrap_or_default()
    }
}

/// Serializable mirror of [`Profile`] for merging TOML over presets.
#[derive(serde::Serialize)]
struct Serialized<'a> {
    name: &'a str,
    signal: &'static str,
    aspect: bool,
    scanlines: f32,
    gamma: f32,
    tint: [f32; 3],
}

impl<'a> From<&'a Profile> for Serialized<'a> {
    fn from(p: &'a Profile) -> Self {
        Self {
            name: &p.name,
            signal: match p.signal {
                Signal::Rgb => "rgb",
                Signal::CompositePal => "composite-pal",
                Signal::CompositeNtsc => "composite-ntsc",
                Signal::SVideo => "s-video",
                Signal::Monochrome => "monochrome",
            },
            aspect: p.aspect,
            scanlines: p.scanlines,
            gamma: p.gamma,
            tint: p.tint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_exist_and_differ() {
        for name in Profile::PRESETS {
            assert_eq!(Profile::preset(name).unwrap().name, name);
        }
        assert!(Profile::preset("nope").is_none());
        assert_eq!(Profile::default_for("c64").name, "1702");
    }

    #[test]
    fn toml_overrides_a_preset() {
        let p = Profile::from_toml("name = \"mine\"\nbase = \"1084s\"\nscanlines = 0.9\n").unwrap();
        assert_eq!(p.name, "mine");
        assert_eq!(p.scanlines, 0.9);
        assert!(p.aspect, "inherited from 1084s");
        assert!(Profile::from_toml("scanline = 1").is_err(), "typo");
        assert!(Profile::from_toml("base = \"x\"").is_err());
    }
}
