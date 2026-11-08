//! TED colours: 16 hues × 8 luminances (colour register bits 0–3 hue,
//! bits 4–6 luminance). Hue 0 is black at every luminance, hue 1 a grey
//! ramp; the others add a chroma vector at a fixed angle.
//!
//! A signal model: the TED puts out a luminance level per luminance step
//! and, for hues 2–15, a colour subcarrier of one amplitude at a phase per
//! hue (it makes the chroma digitally, so the amplitude does not change
//! with the hue). The colour is that signal decoded as BT.601 YUV and
//! clipped to RGB. The levels and phases are the ones xplus4 puts out
//! where nothing clips (`tests/data/ted-palette.png`, made by
//! `tools/ted_timing.py vice … palette`; observed output, no emulator
//! code): the luminance steps of the grey hue, the phase of each hue
//! (rounded to whole degrees; hues 2/3, 4/5, 6/7, 9/13, 10/14 and 11/12 lie
//! opposite each other). The single amplitude, 0.25, is this project's
//! (**[S]**: chosen against that picture, mean distance 5/255 per
//! channel). No measurement of a real TED's signal was at hand; the TED
//! data sheet gives no levels. CRT colour reproduction belongs to
//! `video-crt`.

use emu_core::frame::Rgb8;

/// Luminance level per luminance step (0–1).
const LUMA: [f64; 8] = [
    32.0 / 255.0,
    48.0 / 255.0,
    64.0 / 255.0,
    80.0 / 255.0,
    120.0 / 255.0,
    144.0 / 255.0,
    192.0 / 255.0,
    1.0,
];

/// Chroma phase in degrees (U axis = 0°, V axis = 90°) per hue, `None`
/// for no chroma.
const PHASE: [Option<u16>; 16] = [
    None,      // 0 black
    None,      // 1 white (grey ramp)
    Some(106), // 2 red
    Some(286), // 3 cyan
    Some(52),  // 4 purple
    Some(232), // 5 green
    Some(352), // 6 blue
    Some(172), // 7 yellow
    Some(132), // 8 orange
    Some(153), // 9 brown
    Some(190), // 10 yellow-green
    Some(78),  // 11 pink
    Some(258), // 12 blue-green
    Some(333), // 13 light blue
    Some(10),  // 14 dark blue
    Some(210), // 15 light green
];

/// Chroma amplitude (the length of the UV vector, 0–1 scale).
const CHROMA: f64 = 0.25;

/// The 128 colours, indexed by `luminance << 4 | hue`.
pub fn palette() -> [Rgb8; 128] {
    let mut out = [Rgb8::default(); 128];
    for (i, c) in out.iter_mut().enumerate() {
        let hue = i & 15;
        if hue == 0 {
            continue;
        }
        let y = LUMA[i >> 4];
        let (u, v) = match PHASE[hue] {
            Some(a) => {
                let angle = f64::from(a).to_radians();
                (CHROMA * angle.cos(), CHROMA * angle.sin())
            }
            None => (0.0, 0.0),
        };
        // u = 0.492 (B − Y), v = 0.877 (R − Y).
        let r = y + v / 0.877;
        let b = y + u / 0.492;
        let g = (y - 0.299 * r - 0.114 * b) / 0.587;
        let to8 = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        *c = Rgb8 {
            r: to8(r),
            g: to8(g),
            b: to8(b),
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
        assert_eq!(
            p[0x71],
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            },
            "white"
        );
        let lum = |c: Rgb8| u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
        assert!(lum(p[0x62]) > lum(p[0x22]), "brighter red");
    }

    #[test]
    fn saturated_where_nothing_clips() {
        // Red at luminance 3: R well above G and B.
        let red = palette()[0x32];
        assert!(red.r > 140 && red.g < 70 && red.b < 90, "{red:?}");
        // Blue at luminance 3.
        let blue = palette()[0x36];
        assert!(blue.b > 180 && blue.r < 100, "{blue:?}");
    }
}
