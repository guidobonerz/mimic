//! The rasterised picture of a vector frame: every segment adds the beam's
//! energy along its path (intensity × time per pixel), which the phosphor
//! turns into brightness with a saturating curve, per colour channel (a
//! segment's colour is what the viewer sees: the phosphor, or through the
//! 3D Imager's filters). No persistence here — decay across frames is
//! `video-crt`'s job.
//!
//! Two looks ([`Style`], machine option `lines`):
//! - **fine** (default): thin, dimmer lines; where the beam rests while lit
//!   (a line's end before the next one starts, a dot) it gives the phosphor
//!   more energy, and those points glow bright. Ends cut off by blanking
//!   (dashes, text) stay plain.
//! - **soft**: a one-pixel core with a soft edge and a steep curve: even,
//!   bright lines (the first look that worked; the user's "option 1",
//!   2026-10-24).

use emu_core::frame::Rgb8;
use emu_core::Segment;

/// Picture size in pixels (the tube is 4:5, portrait).
pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Fine,
    Soft,
}

/// A spot: (dx, dy, weight).
type Spot = [(i32, i32, f32)];

/// A one-pixel core with a narrow soft edge.
const SOFT_SPOT: [(i32, i32, f32); 9] = [
    (0, 0, 1.0),
    (-1, 0, 0.45),
    (1, 0, 0.45),
    (0, -1, 0.45),
    (0, 1, 0.45),
    (-1, -1, 0.15),
    (1, -1, 0.15),
    (-1, 1, 0.15),
    (1, 1, 0.15),
];

/// A one-pixel core with a faint edge.
const FINE_SPOT: [(i32, i32, f32); 5] = [
    (0, 0, 1.0),
    (-1, 0, 0.12),
    (1, 0, 0.12),
    (0, -1, 0.12),
    (0, 1, 0.12),
];

/// The glow of a bright point (fine style).
const GLOW_SPOT: [(i32, i32, f32); 9] = [
    (0, 0, 1.0),
    (-1, 0, 0.5),
    (1, 0, 0.5),
    (0, -1, 0.5),
    (0, 1, 0.5),
    (-1, -1, 0.25),
    (1, -1, 0.25),
    (-1, 1, 0.25),
    (1, 1, 0.25),
];

impl Style {
    /// Energy (intensity 1.0 × cycles) per pixel at which the phosphor is
    /// at 1 − e⁻¹ of full brightness. A line at full intensity and the
    /// fastest common beam speed (BIOS scale $7F) gets about 0.33 cycles a
    /// pixel: bright in the soft style, about two thirds in the fine one.
    fn energy(self) -> f32 {
        match self {
            Style::Fine => 0.28,
            Style::Soft => 0.15,
        }
    }

    fn spot(self) -> &'static Spot {
        match self {
            Style::Fine => &FINE_SPOT,
            Style::Soft => &SOFT_SPOT,
        }
    }
}

/// Draw `segments` (coordinates in units of `unit` per pixel) into
/// `pixels`. `energy` is scratch space of the same size.
pub fn draw(
    segments: &[Segment],
    unit: i32,
    style: Style,
    energy: &mut [[f32; 3]],
    pixels: &mut [Rgb8],
) {
    energy.fill([0.0; 3]);
    let (w, h) = (WIDTH as i32, HEIGHT as i32);
    let mut add = |x: f32, y: f32, e: [f32; 3], spot: &Spot| {
        let (x, y) = (x.floor() as i32, y.floor() as i32);
        for &(dx, dy, k) in spot {
            let (px, py) = (x + dx, y + dy);
            if (0..w).contains(&px) && (0..h).contains(&py) {
                let p = &mut energy[(py * w + px) as usize];
                for c in 0..3 {
                    p[c] += e[c] * k;
                }
            }
        }
    };
    for s in segments {
        let i = f32::from(s.intensity) / 255.0;
        let rgb = [s.color.r, s.color.g, s.color.b].map(|c| f32::from(c) / 255.0 * i);
        let (x0, y0) = (s.x0 as f32 / unit as f32, s.y0 as f32 / unit as f32);
        let (x1, y1) = (s.x1 as f32 / unit as f32, s.y1 as f32 / unit as f32);
        let len = (x1 - x0).abs().max((y1 - y0).abs());
        if style == Style::Fine && len < 1.0 {
            // The beam resting on a point.
            add(x0, y0, rgb.map(|c| c * s.cycles as f32), &GLOW_SPOT);
            continue;
        }
        let steps = len.ceil().max(1.0) as i32;
        let e = rgb.map(|c| c * s.cycles as f32 / steps as f32);
        for n in 0..steps {
            let t = if steps == 1 {
                0.0
            } else {
                n as f32 / (steps - 1) as f32
            };
            add(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, e, style.spot());
        }
    }
    let scale = style.energy();
    let level = |e: f32| {
        if e <= 0.0 {
            0
        } else {
            (255.0 * (1.0 - (-e / scale).exp())).round() as u8
        }
    };
    for (p, e) in pixels.iter_mut().zip(energy.iter()) {
        *p = Rgb8 {
            r: level(e[0]),
            g: level(e[1]),
            b: level(e[2]),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb8 = Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    };

    fn line() -> Segment {
        Segment {
            x0: 100,
            y0: 0,
            x1: 200,
            y1: 0,
            intensity: 255,
            color: WHITE,
            cycles: 33,
        }
    }

    fn picture(segs: &[Segment], style: Style) -> Vec<Rgb8> {
        let n = (WIDTH * HEIGHT) as usize;
        let mut energy = vec![[0.0; 3]; n];
        let mut px = vec![Rgb8::default(); n];
        draw(segs, 1, style, &mut energy, &mut px);
        px
    }

    #[test]
    fn soft_lines_and_dots_light_pixels() {
        let dot = Segment {
            x0: 200,
            y0: 200,
            x1: 200,
            y1: 200,
            intensity: 128,
            color: Rgb8 { r: 255, g: 0, b: 0 },
            cycles: 10,
        };
        let px = picture(&[line(), dot], Style::Soft);
        assert!(px[150].g > 200, "line pixel {:?}", px[150]);
        let d = px[200 * WIDTH as usize + 200];
        assert!(d.r > 200 && d.g == 0, "red dot {d:?}");
        assert_eq!(px[300 * WIDTH as usize + 300], Rgb8::default());
    }

    #[test]
    fn fine_lines_are_thinner_and_dimmer_and_resting_points_glow() {
        // The beam rests 6 cycles at the line's end.
        let rest = Segment {
            x0: 200,
            x1: 200,
            cycles: 6,
            ..line()
        };
        let fine = picture(&[line(), rest], Style::Fine);
        let soft = picture(&[line(), rest], Style::Soft);
        let middle = fine[150].g;
        assert!(middle < soft[150].g, "dimmer than soft: {middle}");
        assert!(middle > 100, "still visible: {middle}");
        assert!(fine[200].g > 240, "the resting point glows");
        assert!(fine[201].g > middle, "with a halo");
        // Thin: the row beside the line is much darker than the line.
        assert!(fine[WIDTH as usize + 150].g < middle / 3);
    }
}
