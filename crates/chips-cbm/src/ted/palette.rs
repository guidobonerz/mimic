//! TED colours: 16 hues × 8 luminances (colour register bits 0–3 hue,
//! bits 4–6 luminance). Hue 0 is black at every luminance, hue 1 a grey
//! ramp; the others are chroma vectors at fixed angles.
//!
//! Hue names and order from the Commodore Plus/4 user's guide. The angles
//! and levels are this project's approximation (no measurements); the
//! conversion is BT.601 YUV → RGB. CRT colour reproduction belongs to
//! `video-crt`.

use emu_core::frame::Rgb8;

/// Chroma angle in 1/16 turns per hue, `None` for no chroma.
const HUES: [Option<u8>; 16] = [
    None,     // 0 black
    None,     // 1 white (grey ramp)
    Some(5),  // 2 red
    Some(13), // 3 cyan
    Some(2),  // 4 purple
    Some(10), // 5 green
    Some(0),  // 6 blue
    Some(8),  // 7 yellow
    Some(6),  // 8 orange
    Some(7),  // 9 brown
    Some(9),  // 10 yellow-green
    Some(3),  // 11 pink
    Some(12), // 12 blue-green
    Some(15), // 13 light blue
    Some(1),  // 14 dark blue
    Some(11), // 15 light green
];

/// The 128 colours, indexed by `luminance << 4 | hue`.
pub fn palette() -> [Rgb8; 128] {
    let mut out = [Rgb8::default(); 128];
    for (i, c) in out.iter_mut().enumerate() {
        let hue = i & 15;
        let lum = (i >> 4) as f64;
        if hue == 0 {
            continue;
        }
        let y = 0.12 + lum * 0.12;
        let (u, v) = match HUES[hue] {
            Some(a) => {
                let angle = f64::from(a) * std::f64::consts::TAU / 16.0;
                (0.12 * angle.cos(), 0.12 * angle.sin())
            }
            None => (0.0, 0.0),
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
    fn black_grey_and_brightness() {
        let p = palette();
        for lum in 0..8 {
            assert_eq!(p[lum << 4], Rgb8::default(), "hue 0 is black");
            let g = p[(lum << 4) | 1];
            assert!(g.r == g.g && g.g == g.b, "hue 1 is grey");
        }
        assert!(p[0x71].r > 240, "white");
        let lum = |c: Rgb8| u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
        assert!(lum(p[0x62]) > lum(p[0x22]), "brighter red");
    }
}
