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
//! - PAL: the hues are not evenly spaced: hue 15 equals hue 1, the steps
//!   run about 18° and 36°. `PAL_HUES` holds the angles and `PAL_SATURATION`
//!   the amplitude as observed on Altirra 4.40's default PAL palette (bars
//!   at luminance 6, 2026-11-02; hues 11–14 from the red channel, their
//!   blue clips). The even 24° wheel used before showed the hues up to
//!   40° off: Numen's blue sky ($7x) purple and its green rooms ($Ax) blue.
//!
//! Luminance rises linearly with the four luminance bits (the hardware
//! has 8 levels, GTIA mode 9 16); chroma has a fixed amplitude. Conversion
//! is BT.601 YUV → RGB without gamma, like the VIC-II palette; the
//! monitor's look (and NTSC artifact colours) belong to `video-crt`.

use emu_core::frame::{LumaChroma, Rgb8, VideoStandard};

/// Chroma amplitude relative to full-scale luma.
const SATURATION: f64 = 0.19;
const PAL_SATURATION: f64 = 0.29;
/// Gain on PAL chroma in the composite signal: the delay line halves the
/// colour of pictures that alternate grey and hue lines. Fitted to a
/// reference picture of Numen's title from the user (2026-11-02: its
/// chroma was 1.2 to 1.5 times ours in six areas), not measured on hardware.
const PAL_COMPOSITE_GAIN: f64 = 1.3;
/// Angles of hues 1–15 on PAL (degrees, U = 0°, V = 90°).
const PAL_HUES: [f64; 15] = [
    135.2, 116.6, 81.6, 46.5, 28.4, 9.6, -26.7, -44.8, -63.4, -98.2, -151.7, -169.9, 171.8, 153.2,
    135.2,
];
/// Luma of the darkest and brightest level (black has no setup on the
/// Atari's composite output; level 15 is just below peak white).
const LUMA_MIN: f64 = 0.0;
const LUMA_MAX: f64 = 0.96;

/// NTSC: the angle of hue 1 in the UV plane (the burst, −U) and the step
/// between hues (negative: the delay turns the phase clockwise).
const NTSC_WHEEL: (f64, f64) = (180.0, -360.0 / 14.0);

/// The colour of register value `i` in YUV.
pub fn yuv(standard: VideoStandard, i: u8) -> (f64, f64, f64) {
    let hue = i >> 4;
    let lum = i & 0x0f;
    let y = LUMA_MIN + (LUMA_MAX - LUMA_MIN) * f64::from(lum) / 15.0;
    if hue == 0 {
        return (y, 0.0, 0.0);
    }
    let (angle, saturation) = match standard {
        VideoStandard::Ntsc => (NTSC_WHEEL.0 + NTSC_WHEEL.1 * f64::from(hue - 1), SATURATION),
        VideoStandard::Pal => (PAL_HUES[usize::from(hue - 1)], PAL_SATURATION),
    };
    let a = angle.to_radians();
    (y, saturation * a.cos(), saturation * a.sin())
}

/// The palette as luma and chroma: the RGB palette clips what a dark
/// colour's chroma swings below black (register $30 has no green or blue
/// left), and the composite path needs that chroma (Numen's title picture
/// alternates grey lines with hue lines of luminance 0).
pub fn luma_chroma(standard: VideoStandard) -> [LumaChroma; 256] {
    let mut out = [LumaChroma::default(); 256];
    for (i, c) in out.iter_mut().enumerate() {
        let (y, u, v) = yuv(standard, i as u8);
        let gain = match standard {
            VideoStandard::Pal => PAL_COMPOSITE_GAIN,
            VideoStandard::Ntsc => 1.0,
        };
        let phase = v.atan2(u).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
        *c = LumaChroma {
            luma: (y * 65535.0).round() as u16,
            chroma_phase: (phase * 65536.0).round() as u32 as u16,
            chroma_amplitude: (u.hypot(v) * gain * 65535.0).round() as u16,
        };
    }
    out
}

pub fn palette(standard: VideoStandard) -> [Rgb8; 256] {
    let mut out = [Rgb8::default(); 256];
    for (i, c) in out.iter_mut().enumerate() {
        let (y, u, v) = yuv(standard, i as u8);
        *c = rgb(y, u, v);
    }
    out
}

/// TIA palette (Atari 2600): 16 hues × 8 luminances, indexed by the
/// colour register value ÷ 2.
///
/// - NTSC: the TIA makes colour like GTIA (hue 1 at the burst, 25.7° per
///   hue), with three luminance bits.
/// - PAL (this project's approximation of the commonly described layout):
///   hues 0, 1, 14 and 15 are grey; the even hues 2–12 run from gold
///   through orange, red and magenta to violet, 30° apart, and each odd
///   hue is the even hue before it mirrored on the U axis (yellow-green,
///   green, teal, cyan, light blue, blue), which gives 104 distinct
///   colours.
pub fn tia(standard: VideoStandard) -> [Rgb8; 128] {
    std::array::from_fn(|i| {
        let (hue, lum) = (i >> 3, i & 7);
        let y = LUMA_MIN + (LUMA_MAX - LUMA_MIN) * lum as f64 / 7.0;
        let angle = match standard {
            VideoStandard::Ntsc if hue > 0 => Some(180.0 - 360.0 / 14.0 * (hue - 1) as f64),
            VideoStandard::Pal if (2..=13).contains(&hue) => {
                let a = 168.0 - 30.0 * ((hue - 2) / 2) as f64;
                Some(if hue % 2 == 0 { a } else { -a })
            }
            _ => None,
        };
        let (u, v) = angle.map_or((0.0, 0.0), |a: f64| {
            let r = a.to_radians();
            (SATURATION * r.cos(), SATURATION * r.sin())
        });
        rgb(y, u, v)
    })
}

fn rgb(y: f64, u: f64, v: f64) -> Rgb8 {
    let to8 = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb8 {
        r: to8(y + 1.140 * v),
        g: to8(y - 0.395 * u - 0.581 * v),
        b: to8(y + 2.032 * u),
    }
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
        // Hue 15 comes back to hue 1 on both standards.
        for standard in [VideoStandard::Ntsc, VideoStandard::Pal] {
            let d = (hue_angle(standard, 0xf6) - hue_angle(standard, 0x16)).rem_euclid(360.0);
            assert!(d < 1.0 || d > 359.0, "{d}");
        }
        // PAL: Numen's sky ($74) is blue, its rooms ($A4) green.
        let p = palette(VideoStandard::Pal);
        let (sky, room) = (p[0x74], p[0xa4]);
        assert!(sky.b > sky.r && sky.b > sky.g, "hue 7 {sky:?}");
        assert!(room.g > room.r && room.g > room.b, "hue 10 {room:?}");
    }

    #[test]
    fn luma_chroma_keeps_the_chroma_of_dark_colours() {
        let lc = luma_chroma(VideoStandard::Pal);
        assert_eq!(
            (lc[0x30].luma, lc[0x30].chroma_amplitude > 18_000),
            (0, true)
        );
        assert_eq!(lc[0x08].chroma_amplitude, 0);
        assert_eq!(lc[0x38].chroma_phase, lc[0x30].chroma_phase);
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

    #[test]
    fn tia_palettes() {
        let ntsc = tia(VideoStandard::Ntsc);
        assert_eq!(ntsc[0], Rgb8 { r: 0, g: 0, b: 0 });
        assert_eq!(ntsc[7].r, ntsc[7].b, "hue 0 is grey");
        // Hue 1 is gold: red and green over blue.
        let gold = ntsc[0x0c];
        assert!(gold.r > gold.b && gold.g > gold.b);
        let pal = tia(VideoStandard::Pal);
        for hue in [0, 1, 14, 15] {
            let c = pal[hue * 8 + 4];
            assert_eq!((c.r, c.g), (c.b, c.b), "PAL hue {hue} is grey");
        }
        let distinct: std::collections::BTreeSet<_> = pal.iter().map(|c| (c.r, c.g, c.b)).collect();
        // 12 colour hues × 8 + 8 greys = 104; clamping at black merges a few.
        assert!(distinct.len() > 96, "{}", distinct.len());
        // Hue 2 is warm (red over blue), hue 3 its green mirror.
        let (h2, h3) = (pal[2 * 8 + 4], pal[3 * 8 + 4]);
        assert!(h2.r > h2.b && h3.g > h3.r);
    }
}
