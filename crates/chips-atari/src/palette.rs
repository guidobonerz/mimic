//! GTIA colour palette: 16 hues × 16 luminances, indexed by the colour
//! register value (bit 0 only matters in GTIA mode 9).
//!
//! This project's own model of how GTIA makes colour (Altirra Hardware
//! Reference Manual, "GTIA: colour generation"; De Re Atari, appendix on
//! colour): hue 0 has no chroma; hue 1 is in phase with the colour burst
//! (the −U axis: gold); every further hue delays the subcarrier by a
//! fixed step, turning the colour towards red, purple, blue, cyan and
//! green.
//!
//! - NTSC: the delay line is set (colour pot) so that hue 15 comes back
//!   near hue 1: 14 steps of 360°/14 ≈ 25.7°.
//! - PAL: the 15 hues are spread evenly around the circle, 24° apart,
//!   starting a step past the burst.
//!
//! Luminance rises linearly with the four luminance bits (the hardware
//! has 8 levels, GTIA mode 9 16); chroma has a fixed amplitude. Conversion
//! is BT.601 YUV → RGB without gamma, like the VIC-II palette; the
//! monitor's look (and NTSC artifact colours) belong to `video-crt`.

use emu_core::frame::{Rgb8, VideoStandard};

/// Chroma amplitude relative to full-scale luma.
const SATURATION: f64 = 0.19;
/// Luma of the darkest and brightest level (black has no setup on the
/// Atari's composite output; level 15 is just below peak white).
const LUMA_MIN: f64 = 0.0;
const LUMA_MAX: f64 = 0.96;

/// Angle of hue 1 in the UV plane (degrees; U = 0°, V = 90°) and the step
/// between hues (negative: the delay turns the phase clockwise).
fn wheel(standard: VideoStandard) -> (f64, f64) {
    match standard {
        // The burst is at 180° (−U).
        VideoStandard::Ntsc => (180.0, -360.0 / 14.0),
        VideoStandard::Pal => (180.0 - 12.0, -24.0),
    }
}

/// The colour of register value `i` in YUV.
pub fn yuv(standard: VideoStandard, i: u8) -> (f64, f64, f64) {
    let (start, step) = wheel(standard);
    let hue = i >> 4;
    let lum = i & 0x0f;
    let y = LUMA_MIN + (LUMA_MAX - LUMA_MIN) * f64::from(lum) / 15.0;
    if hue == 0 {
        return (y, 0.0, 0.0);
    }
    let a = (start + step * f64::from(hue - 1)).to_radians();
    (y, SATURATION * a.cos(), SATURATION * a.sin())
}

pub fn palette(standard: VideoStandard) -> [Rgb8; 256] {
    let mut out = [Rgb8::default(); 256];
    for (i, c) in out.iter_mut().enumerate() {
        let (y, u, v) = yuv(standard, i as u8);
        let to8 = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        *c = Rgb8 {
            r: to8(y + 1.140 * v),
            g: to8(y - 0.395 * u - 0.581 * v),
            b: to8(y + 2.032 * u),
        };
    }
    out
}

/// TIA palette (Atari 2600): 16 hues × 8 luminances, indexed by the
/// colour register value ÷ 2. The TIA generates colour like GTIA with
/// three luminance bits (provisional for PAL, whose hue layout differs).
pub fn tia(standard: VideoStandard) -> [Rgb8; 128] {
    let full = palette(standard);
    std::array::from_fn(|i| full[((i >> 3) << 4) | ((i & 7) << 1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue_angle(standard: VideoStandard, i: u8) -> f64 {
        let (_, u, v) = yuv(standard, i);
        v.atan2(u).to_degrees().rem_euclid(360.0)
    }

    #[test]
    fn grey_ramp() {
        for s in [VideoStandard::Ntsc, VideoStandard::Pal] {
            let p = palette(s);
            assert_eq!(p[0x00], Rgb8 { r: 0, g: 0, b: 0 }, "black");
            assert!(p[0x0e].r > 220 && p[0x0e].r == p[0x0e].b, "white");
            for l in 1..16 {
                assert!(p[l].r > p[l - 1].r, "luma rises");
            }
        }
    }

    #[test]
    fn hues_follow_the_colour_wheel() {
        let p = palette(VideoStandard::Ntsc);
        // Gold, red, purple, blue, cyan, green as on a TV.
        let gold = p[0x18];
        assert!(gold.r > gold.b && gold.g > gold.b, "hue 1 {gold:?}");
        let red = p[0x46];
        assert!(red.r > red.g && red.r > red.b, "hue 4 {red:?}");
        let blue = p[0x86];
        assert!(blue.b > blue.r && blue.b > blue.g, "hue 8 {blue:?}");
        let green = p[0xc6];
        assert!(green.g > green.r && green.g > green.b, "hue 12 {green:?}");
        // NTSC hue 15 comes back to hue 1; PAL steps 24°.
        let d = (hue_angle(VideoStandard::Ntsc, 0xf6) - hue_angle(VideoStandard::Ntsc, 0x16))
            .rem_euclid(360.0);
        assert!(d < 1.0 || d > 359.0, "{d}");
        let step = (hue_angle(VideoStandard::Pal, 0x26) - hue_angle(VideoStandard::Pal, 0x36))
            .rem_euclid(360.0);
        assert!((step - 24.0).abs() < 0.01, "{step}");
    }

    #[test]
    fn same_luma_across_hues() {
        // Colours of one luminance have the same Y (what the delay line in a
        // PAL TV relies on).
        for i in 1..16u8 {
            let (y, _, _) = yuv(VideoStandard::Pal, (i << 4) | 0x08);
            let (y0, _, _) = yuv(VideoStandard::Pal, 0x08);
            assert!((y - y0).abs() < 1e-9);
        }
    }
}
