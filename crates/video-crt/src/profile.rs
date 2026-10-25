//! Monitor profiles: which connection and how strong each CRT effect is.
//! Built-in presets; users override any field in TOML.
//!
//! ```toml
//! name = "my 1084s"
//! base = "1084s"        # start from a preset
//! scanlines = 0.6
//! mask = "aperture"     # none | shadow | aperture | slot
//! mask_strength = 0.3
//! curvature = 0.05
//! persistence = 0.4     # desktop and stateful renderers only
//! ```

use serde::Deserialize;

/// The connection a profile models (see [`crate::signal`]). PAL or NTSC
/// decoding follows the frame's video standard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    /// Clean RGB: no signal stage.
    #[default]
    Rgb,
    #[serde(alias = "composite-pal", alias = "composite-ntsc")]
    Composite,
    SVideo,
    /// Luma only, coloured by `tint`.
    Monochrome,
}

/// The phosphor arrangement of the tube (see [`crate::cpu`] for the model).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mask {
    /// No mask: monochrome tubes, sharp output.
    #[default]
    None,
    /// Shadow mask (delta-gun dot triads): rows of R, G, B dots, every
    /// second row shifted by half a triad.
    Shadow,
    /// Aperture grille (Trinitron): continuous vertical R, G, B stripes.
    Aperture,
    /// Slot mask (in-line guns): vertical stripes cut into slots, the
    /// slots of neighbouring triads staggered by half a slot.
    Slot,
}

impl Mask {
    /// The shader's code for the mask kind (`Params.mask_kind`).
    pub fn code(self) -> f32 {
        match self {
            Mask::None => 0.0,
            Mask::Shadow => 1.0,
            Mask::Aperture => 2.0,
            Mask::Slot => 3.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mask::None => "none",
            Mask::Shadow => "shadow",
            Mask::Aperture => "aperture",
            Mask::Slot => "slot",
        }
    }
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
    /// Luma bandwidth in MHz (0 = unlimited).
    pub luma_bandwidth: f32,
    /// Chroma bandwidth in MHz.
    pub chroma_bandwidth: f32,
    /// PAL decoders average chroma with the previous line.
    pub delay_line: bool,
    /// Colour knob: chroma gain after decoding.
    pub saturation: f32,
    /// Phosphor mask of the tube.
    pub mask: Mask,
    /// How dark the mask makes the other phosphors' places, 0.0 – 1.0.
    pub mask_strength: f32,
    /// Output pixels per phosphor triad (any value ≥ 1, e.g. 3.0).
    pub mask_pitch: f32,
    /// Halation 0.0 – 1.0: bright areas glow into their neighbours and
    /// into the gaps between scanlines.
    pub bloom: f32,
    /// Barrel distortion of the picture 0.0 – 0.25 (curved glass).
    pub curvature: f32,
    /// Darkening towards the corners 0.0 – 1.0.
    pub vignette: f32,
    /// Phosphor decay 0.0 – 1.0: the fraction of the previous output kept
    /// per frame (a slow phosphor leaves trails). Only stateful renderers
    /// ([`crate::Crt`], the desktop) keep a frame history;
    /// [`crate::render`] ignores it.
    pub persistence: f32,
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
            luma_bandwidth: 0.0,
            chroma_bandwidth: 1.3,
            delay_line: true,
            saturation: 1.0,
            mask: Mask::None,
            mask_strength: 0.0,
            mask_pitch: 3.0,
            bloom: 0.0,
            curvature: 0.0,
            vignette: 0.0,
            persistence: 0.0,
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
            // For weak hardware and WASM: aspect and light scanlines only.
            "lite" => Self {
                aspect: true,
                scanlines: 0.25,
                ..base
            },
            // Commodore 1702 (JVC tube, in-line guns): slot mask, a fairly
            // flat face.
            "1702" => Self {
                signal: Signal::SVideo,
                aspect: true,
                scanlines: 0.35,
                gamma: 1.1,
                mask: Mask::Slot,
                mask_strength: 0.25,
                bloom: 0.15,
                curvature: 0.04,
                vignette: 0.15,
                ..base
            },
            // Commodore 1084S (Philips tube): fine dot mask.
            "1084s" => Self {
                signal: Signal::SVideo,
                aspect: true,
                scanlines: 0.45,
                gamma: 1.1,
                mask: Mask::Shadow,
                mask_strength: 0.25,
                bloom: 0.15,
                curvature: 0.04,
                vignette: 0.15,
                ..base
            },
            // RGB input of the 1084S (C128 VDC, Amiga).
            "1084s-rgb" => Self {
                aspect: true,
                scanlines: 0.45,
                gamma: 1.1,
                mask: Mask::Shadow,
                mask_strength: 0.25,
                bloom: 0.12,
                curvature: 0.04,
                vignette: 0.15,
                ..base
            },
            // A TV on composite (or RF); PAL or NTSC decoding follows the
            // frame. "tv" is the Atari 8-bit default. Coarser dot mask,
            // more curvature and glow than a monitor.
            "pal-tv" | "ntsc-tv" | "tv" => Self {
                signal: Signal::Composite,
                aspect: true,
                scanlines: 0.25,
                gamma: 1.15,
                luma_bandwidth: 4.2,
                mask: Mask::Shadow,
                mask_strength: 0.3,
                mask_pitch: 3.5,
                bloom: 0.25,
                curvature: 0.08,
                vignette: 0.25,
                ..base
            },
            // PET built-in monitors: white phosphor (2001), green (later,
            // a long-persistence P39-like phosphor).
            "pet-white" => Self {
                signal: Signal::Monochrome,
                aspect: true,
                scanlines: 0.3,
                tint: [0.85, 0.92, 1.0],
                bloom: 0.15,
                curvature: 0.06,
                vignette: 0.2,
                persistence: 0.1,
                ..base
            },
            "pet-green" => Self {
                signal: Signal::Monochrome,
                aspect: true,
                scanlines: 0.3,
                tint: [0.26, 1.0, 0.0],
                bloom: 0.2,
                curvature: 0.06,
                vignette: 0.2,
                persistence: 0.5,
                ..base
            },
            // Amstrad CTM644 colour monitor (RGB, the CPC's default): a
            // coarse stripe-like mask.
            "ctm644" => Self {
                aspect: true,
                scanlines: 0.4,
                gamma: 1.1,
                mask: Mask::Aperture,
                mask_strength: 0.25,
                bloom: 0.15,
                curvature: 0.05,
                vignette: 0.15,
                ..base
            },
            // Amstrad GT64/GT65 green monitor: the luminance of the RGB
            // picture on long-persistence green phosphor.
            "gt65" => Self {
                signal: Signal::Monochrome,
                aspect: true,
                scanlines: 0.3,
                tint: [0.2, 1.0, 0.25],
                bloom: 0.15,
                curvature: 0.05,
                vignette: 0.15,
                persistence: 0.45,
                ..base
            },
            // The compact Macintosh's built-in 9" white monitor (square
            // pixels, a sharp picture).
            "mac" => Self {
                signal: Signal::Monochrome,
                scanlines: 0.15,
                tint: [0.92, 0.95, 1.0],
                bloom: 0.05,
                curvature: 0.03,
                vignette: 0.1,
                ..base
            },
            _ => return None,
        })
    }

    pub const PRESETS: [&'static str; 13] = [
        "raw",
        "lite",
        "1702",
        "1084s",
        "1084s-rgb",
        "tv",
        "pal-tv",
        "ntsc-tv",
        "pet-white",
        "pet-green",
        "ctm644",
        "gt65",
        "mac",
    ];

    /// Parse a TOML profile; `base = "<preset>"` starts from a preset.
    /// The whole profile as TOML (every field, so it loads without `base`);
    /// what the profile editor saves.
    pub fn to_toml(&self) -> String {
        toml::to_string(&Serialized::from(self)).unwrap_or_default()
    }

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
            "pet" => "pet-green",
            "atari8" | "atari2600" | "zx81" | "apple2" => "tv",
            "cpc" => "ctm644",
            // The SNES has an RGB output (a composite TV shows colour
            // fringes on its hard edges: `--crt tv`).
            "amiga" | "snes" => "1084s-rgb",
            "mac" => "mac",
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
    luma_bandwidth: f32,
    chroma_bandwidth: f32,
    delay_line: bool,
    saturation: f32,
    mask: &'static str,
    mask_strength: f32,
    mask_pitch: f32,
    bloom: f32,
    curvature: f32,
    vignette: f32,
    persistence: f32,
}

impl<'a> From<&'a Profile> for Serialized<'a> {
    fn from(p: &'a Profile) -> Self {
        Self {
            name: &p.name,
            signal: match p.signal {
                Signal::Rgb => "rgb",
                Signal::Composite => "composite",
                Signal::SVideo => "s-video",
                Signal::Monochrome => "monochrome",
            },
            aspect: p.aspect,
            scanlines: p.scanlines,
            gamma: p.gamma,
            tint: p.tint,
            luma_bandwidth: p.luma_bandwidth,
            chroma_bandwidth: p.chroma_bandwidth,
            delay_line: p.delay_line,
            saturation: p.saturation,
            mask: p.mask.name(),
            mask_strength: p.mask_strength,
            mask_pitch: p.mask_pitch,
            bloom: p.bloom,
            curvature: p.curvature,
            vignette: p.vignette,
            persistence: p.persistence,
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
        let p = Profile::from_toml(
            "signal = \"composite-ntsc\"
delay_line = false",
        )
        .unwrap();
        assert_eq!(p.signal, Signal::Composite, "old names still parse");
        assert!(!p.delay_line);
    }

    #[test]
    fn crt_fields_parse_and_inherit() {
        let p = Profile::from_toml(
            "base = \"1702\"
mask = \"aperture\"
bloom = 0.5
",
        )
        .unwrap();
        assert_eq!(p.mask, Mask::Aperture);
        assert_eq!(p.bloom, 0.5);
        assert_eq!(p.curvature, 0.04, "inherited");
        // Old profiles without the CRT fields still load, effect-free.
        let p = Profile::from_toml(
            "scanlines = 0.5
",
        )
        .unwrap();
        assert_eq!((p.mask, p.bloom, p.persistence), (Mask::None, 0.0, 0.0));
        assert!(Profile::from_toml("mask = \"triad\"").is_err());
        // Every preset survives the TOML round trip through `base`.
        for name in Profile::PRESETS {
            let p = Profile::from_toml(&format!("base = \"{name}\"")).unwrap();
            assert_eq!(p, Profile::preset(name).unwrap(), "{name}");
        }
    }

    #[test]
    fn raw_and_lite_are_effect_free() {
        for name in ["raw", "lite"] {
            let p = Profile::preset(name).unwrap();
            assert_eq!(p.mask, Mask::None, "{name}");
            assert_eq!(
                (p.bloom, p.curvature, p.vignette, p.persistence),
                (0.0, 0.0, 0.0, 0.0),
                "{name}"
            );
        }
    }
}
