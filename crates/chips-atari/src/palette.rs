//! GTIA colour palette: 16 hues × 16 luminances, indexed by the colour
//! register value.
//!
//! This project's own model: hue 0 is grey; hues 1–15 step around the
//! colour wheel in equal angles, starting at gold (hue 1) and turning
//! towards red, purple, blue, cyan, green (Atari 800XL colour descriptions,
//! De Re Atari appendix). Luminance is linear in the four luminance bits;
//! chroma has a fixed amplitude. Conversion is BT.601 YUV → RGB without
//! gamma, like the VIC-II palette; the monitor's look belongs to
//! `video-crt`. M4 of Phase 5 calibrates the PAL and NTSC wheels.

use emu_core::frame::{Rgb8, VideoStandard};

/// Chroma amplitude relative to full-scale luma.
const SATURATION: f64 = 0.17;

/// Angle of hue 1 in the UV plane (degrees, U axis = 0, V = 90) and the
/// step between hues (the wheel turns clockwise).
fn wheel(standard: VideoStandard) -> (f64, f64) {
    match standard {
        VideoStandard::Ntsc => (167.0, -25.0),
        VideoStandard::Pal => (170.0, -24.0),
    }
}

pub fn palette(standard: VideoStandard) -> [Rgb8; 256] {
    let (start, step) = wheel(standard);
    let mut out = [Rgb8::default(); 256];
    for (i, c) in out.iter_mut().enumerate() {
        let hue = i >> 4;
        let lum = i & 0x0f;
        let y = 0.06 + 0.92 * lum as f64 / 15.0;
        let (u, v) = if hue == 0 {
            (0.0, 0.0)
        } else {
            let a = (start + step * (hue - 1) as f64).to_radians();
            (SATURATION * a.cos(), SATURATION * a.sin())
        };
        let to8 = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        *c = Rgb8 {
            r: to8(y + 1.140 * v),
            g: to8(y - 0.395 * u - 0.581 * v),
            b: to8(y + 2.032 * u),
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_ramp_and_hues() {
        let p = palette(VideoStandard::Ntsc);
        assert_eq!(p[0x00].r, p[0x00].b, "hue 0 is grey");
        assert!(p[0x0e].r > 230, "white");
        assert!(p[0x00].r < 30, "black");
        // Hue 3/4 is red, hue 8 blue, hue 12 green.
        let red = p[0x46];
        assert!(red.r > red.g && red.r > red.b, "{red:?}");
        let blue = p[0x86];
        assert!(blue.b > blue.r && blue.b > blue.g, "{blue:?}");
        let green = p[0xc6];
        assert!(green.g > green.r && green.g > green.b, "{green:?}");
    }
}
