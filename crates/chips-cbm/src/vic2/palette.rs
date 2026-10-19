//! VIC-II colour palette computed from the chip's YUV output.
//!
//! Method after Philip "Pepto" Timmermans, "Calculating the color palette of
//! the VIC II": every colour is a luma level plus a chroma vector at one of
//! a few fixed angles with equal amplitude. Luma levels are the measured
//! 6569R3 levels on a 0–32 scale; the angles are multiples of 22.5°. The
//! conversion below (BT.601 YUV → RGB, no gamma) is this project's own; CRT
//! colour reproduction belongs to `video-crt`.

use emu_core::frame::Rgb8;

/// (luma 0–32, chroma angle in 1/16 turns, has chroma)
const COLORS: [(u8, u8, bool); 16] = [
    (0, 0, false),  // 0 black
    (32, 0, false), // 1 white
    (10, 5, true),  // 2 red          112.5°
    (20, 13, true), // 3 cyan         292.5°
    (12, 2, true),  // 4 purple        45°
    (16, 10, true), // 5 green        225°
    (8, 0, true),   // 6 blue           0°
    (24, 8, true),  // 7 yellow       180°
    (12, 6, true),  // 8 orange       135°
    (8, 7, true),   // 9 brown        157.5°
    (16, 5, true),  // 10 light red   112.5°
    (10, 0, false), // 11 dark grey
    (15, 0, false), // 12 grey
    (24, 10, true), // 13 light green 225°
    (15, 0, true),  // 14 light blue    0°
    (20, 0, false), // 15 light grey
];

/// Chroma amplitude relative to full-scale luma.
const SATURATION: f64 = 0.13;

pub fn palette() -> [Rgb8; 16] {
    let mut out = [Rgb8::default(); 16];
    for (i, &(luma, angle, chroma)) in COLORS.iter().enumerate() {
        let y = f64::from(luma) / 32.0;
        let (u, v) = if chroma {
            let a = f64::from(angle) * std::f64::consts::TAU / 16.0;
            (SATURATION * a.cos(), SATURATION * a.sin())
        } else {
            (0.0, 0.0)
        };
        let to8 = |c: f64| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        out[i] = Rgb8 {
            r: to8(y + 1.140 * v),
            g: to8(y - 0.396 * u - 0.581 * v),
            b: to8(y + 2.029 * u),
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greys_are_neutral_and_ordered() {
        let p = palette();
        assert_eq!(p[0], Rgb8 { r: 0, g: 0, b: 0 });
        assert_eq!(
            p[1],
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        for &i in &[11usize, 12, 15] {
            assert_eq!(p[i].r, p[i].g);
            assert_eq!(p[i].g, p[i].b);
        }
        assert!(p[11].r < p[12].r && p[12].r < p[15].r);
    }

    #[test]
    fn hues_point_the_right_way() {
        let p = palette();
        assert!(p[2].r > p[2].g && p[2].r > p[2].b, "red");
        assert!(p[6].b > p[6].r && p[6].b > p[6].g, "blue");
        assert!(p[5].g > p[5].r && p[5].g > p[5].b, "green");
        assert!(p[7].r > p[7].b && p[7].g > p[7].b, "yellow");
        assert!(p[3].g > p[3].r && p[3].b > p[3].r, "cyan");
    }
}
