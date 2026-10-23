//! The rasterised picture of a vector frame: every segment adds the beam's
//! energy along its path (intensity × time per pixel), which the phosphor
//! turns into brightness with a saturating curve. No persistence here —
//! decay across frames is `video-crt`'s job.

use emu_core::frame::Rgb8;
use emu_core::Segment;

/// Picture size in pixels (the tube is 4:5, portrait).
pub const WIDTH: u32 = 400;
pub const HEIGHT: u32 = 500;

/// Energy (intensity 1.0 × cycles) per pixel at which the phosphor is at
/// 1 − e⁻¹ of full brightness. A line at full intensity and the fastest
/// common beam speed (BIOS scale $7F) gets about 0.65 cycles a pixel.
const ENERGY: f32 = 0.3;

/// The grey ramp of the picture: phosphor colour at 256 levels.
pub fn palette(phosphor: Rgb8) -> Vec<Rgb8> {
    (0..=255u32)
        .map(|i| Rgb8 {
            r: (u32::from(phosphor.r) * i / 255) as u8,
            g: (u32::from(phosphor.g) * i / 255) as u8,
            b: (u32::from(phosphor.b) * i / 255) as u8,
        })
        .collect()
}

/// Draw `segments` (coordinates in units of `unit` per pixel) into
/// `pixels` (palette indices). `energy` is scratch space of the same size.
pub fn draw(segments: &[Segment], unit: i32, energy: &mut [f32], pixels: &mut [u8]) {
    energy.fill(0.0);
    let (w, h) = (WIDTH as i32, HEIGHT as i32);
    let mut add = |x: i32, y: i32, e: f32| {
        // Two pixels wide: the spot is bigger than a pixel.
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (px, py) = (x + dx, y + dy);
            if (0..w).contains(&px) && (0..h).contains(&py) {
                energy[(py * w + px) as usize] += e;
            }
        }
    };
    for s in segments {
        let i = f32::from(s.intensity) / 255.0;
        let (x0, y0) = (s.x0 as f32 / unit as f32, s.y0 as f32 / unit as f32);
        let (x1, y1) = (s.x1 as f32 / unit as f32, s.y1 as f32 / unit as f32);
        let len = (x1 - x0).abs().max((y1 - y0).abs());
        let steps = len.ceil().max(1.0) as i32;
        let e = i * s.cycles as f32 / steps as f32;
        for n in 0..steps {
            let t = if steps == 1 {
                0.0
            } else {
                n as f32 / (steps - 1) as f32
            };
            let x = x0 + (x1 - x0) * t;
            let y = y0 + (y1 - y0) * t;
            add(x.floor() as i32, y.floor() as i32, e);
        }
    }
    for (p, &e) in pixels.iter_mut().zip(energy.iter()) {
        *p = if e <= 0.0 {
            0
        } else {
            (255.0 * (1.0 - (-e / ENERGY).exp())).round() as u8
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_dots_light_pixels() {
        let n = (WIDTH * HEIGHT) as usize;
        let mut energy = vec![0.0; n];
        let mut px = vec![0u8; n];
        let segs = [
            Segment {
                x0: 0,
                y0: 0,
                x1: 100,
                y1: 0,
                intensity: 255,
                cycles: 60,
            },
            Segment {
                x0: 200,
                y0: 200,
                x1: 200,
                y1: 200,
                intensity: 128,
                cycles: 10,
            },
        ];
        draw(&segs, 1, &mut energy, &mut px);
        assert!(px[50] > 200, "line pixel {}", px[50]);
        assert!(px[200 * WIDTH as usize + 200] > 200);
        assert_eq!(px[300 * WIDTH as usize + 300], 0);
    }
}
