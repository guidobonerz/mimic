//! CPU path: a deterministic CRT approximation for headless use, preview
//! snapshots (`filter=crt`) and CI. The GPU shader (`shaders/crt.wgsl`)
//! follows the same model.
//!
//! Geometry: every field line becomes `scale` output rows (a PAL/NTSC
//! field shows each line twice on a 576/480-line screen, so `scale = 2` is
//! the natural size). The width follows the pixel aspect ratio derived from
//! the pixel clock against the ITU-R BT.601 square-pixel rates (PAL
//! 14.75 MHz, NTSC 12.2727 MHz for 576/480 lines): for the C64 PAL
//! (7.882 MHz) that is 0.936.

use emu_core::frame::{FieldParity, FrameRef, VideoStandard};

use crate::profile::Profile;

/// RGB image, 3 bytes per pixel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// Pixel aspect ratio (width / height of one source pixel, with a field
/// line counted as two screen lines) from the signal timing.
pub fn pixel_aspect(frame: &FrameRef<'_>) -> f64 {
    let square_hz = match frame.signal.standard {
        VideoStandard::Pal => 14_750_000.0,
        VideoStandard::Ntsc => 12_272_727.0,
    };
    let lines_per_pixel = match frame.signal.field {
        FieldParity::Progressive => 2.0,
        FieldParity::Even | FieldParity::Odd => 1.0,
    };
    if frame.signal.pixel_clock_hz == 0 {
        return 1.0;
    }
    square_hz / f64::from(frame.signal.pixel_clock_hz) / lines_per_pixel
}

/// Output size for a frame at vertical `scale` (rows per field line).
pub fn output_size(frame: &FrameRef<'_>, profile: &Profile, scale: u32) -> (u32, u32) {
    let scale = scale.max(1);
    let par = if profile.aspect {
        pixel_aspect(frame)
    } else {
        1.0
    };
    let width = (f64::from(frame.width) * f64::from(scale) * par)
        .round()
        .max(1.0) as u32;
    (width, frame.height * scale)
}

fn gamma_lut(gamma: f32) -> [u8; 256] {
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        *v = (x.powf(1.0 / gamma.max(0.01)) * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    lut
}

pub fn render(frame: &FrameRef<'_>, profile: &Profile, scale: u32) -> Image {
    let scale = scale.max(1);
    let (width, height) = output_size(frame, profile, scale);

    // Source rows resampled horizontally (linear) once per source line.
    let src_w = frame.width as usize;
    let mut row = vec![[0f32; 3]; width as usize];
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    let lut = gamma_lut(profile.gamma);
    let step = f64::from(frame.width) / f64::from(width);
    let out_w = width as usize;

    for sy in 0..frame.height {
        for (x, out) in row.iter_mut().enumerate() {
            // Centre-aligned sample position.
            let pos = ((x as f64 + 0.5) * step - 0.5).max(0.0);
            let x0 = (pos.floor() as usize).min(src_w - 1);
            let x1 = (x0 + 1).min(src_w - 1);
            let t = (pos - pos.floor()) as f32;
            let a = frame.rgb_at(x0 as u32, sy).unwrap_or_default();
            let b = frame.rgb_at(x1 as u32, sy).unwrap_or_default();
            *out = [
                f32::from(a.r) * (1.0 - t) + f32::from(b.r) * t,
                f32::from(a.g) * (1.0 - t) + f32::from(b.g) * t,
                f32::from(a.b) * (1.0 - t) + f32::from(b.b) * t,
            ];
        }
        for r in 0..scale {
            // Position inside the line: 0 at the beam centre, 1 at the gap.
            let offset = if scale == 1 {
                0.0
            } else {
                (r as f32 + 0.5) / scale as f32 * 2.0 - 1.0
            }
            .abs();
            let y = sy * scale + r;
            for (x, c) in row.iter().enumerate() {
                let luma = (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) / 255.0;
                // Bright pixels make a wider beam: they lose less at the edge.
                let beam = 0.5 + 0.5 * luma;
                let falloff = (offset / beam.max(0.01)).min(1.0);
                let k = 1.0 - profile.scanlines * falloff * falloff;
                let i = (y as usize * out_w + x) * 3;
                for ch in 0..3 {
                    let v = (c[ch] * k * profile.tint[ch]).round().clamp(0.0, 255.0) as u8;
                    rgb[i + ch] = lut[usize::from(v)];
                }
            }
        }
    }
    Image { width, height, rgb }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{OutputType, PixelData, Rgb8, SignalInfo};

    const PALETTE: [Rgb8; 2] = [
        Rgb8 { r: 0, g: 0, b: 0 },
        Rgb8 {
            r: 255,
            g: 255,
            b: 255,
        },
    ];

    fn frame(pixels: &[u8], w: u32, h: u32) -> FrameRef<'_> {
        FrameRef {
            width: w,
            height: h,
            pixels: PixelData::Indexed {
                pixels,
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 7_881_984,
                active_pixels: w,
                total_pixels: 504,
                lines_per_field: 312,
                standard: VideoStandard::Pal,
                output: OutputType::SVideo,
                field: FieldParity::Progressive,
            },
        }
    }

    #[test]
    fn c64_pal_pixel_aspect() {
        let px = [0u8; 4];
        let f = frame(&px, 2, 2);
        assert!((pixel_aspect(&f) - 0.9357).abs() < 0.001);
        let mut p = Profile::preset("1702").unwrap();
        assert_eq!(output_size(&f, &p, 2), (4, 4));
        p.aspect = true;
        let big = vec![0u8; 404 * 284];
        let f = frame(&big, 404, 284);
        let (w, h) = output_size(&f, &p, 2);
        assert_eq!((w, h), (756, 568));
        let ratio = f64::from(w) / f64::from(h);
        assert!((1.30..1.36).contains(&ratio), "≈4:3 with overscan: {ratio}");
    }

    #[test]
    fn raw_profile_is_an_exact_integer_scale() {
        let px = [0, 1, 1, 0];
        let f = frame(&px, 2, 2);
        let img = render(&f, &Profile::preset("raw").unwrap(), 3);
        assert_eq!((img.width, img.height), (6, 6));
        assert_eq!(img.pixel(0, 0), [0, 0, 0]);
        assert_eq!(img.pixel(5, 0), [255, 255, 255]);
        assert_eq!(img.pixel(1, 5), [255, 255, 255]);
    }

    #[test]
    fn scanlines_darken_gaps_more_for_dark_colours() {
        let px = vec![1u8; 16];
        let f = frame(&px, 4, 4);
        let mut p = Profile::preset("raw").unwrap();
        p.scanlines = 0.8;
        let img = render(&f, &p, 4);
        let centre = img.pixel(0, 1)[0];
        let gap = img.pixel(0, 3)[0];
        assert!(gap < centre, "gap {gap} centre {centre}");
        // White: wide beam, the gap keeps most of its brightness.
        assert!(gap > 100, "{gap}");
    }

    #[test]
    fn deterministic() {
        let px: Vec<u8> = (0..64).map(|i| (i % 2) as u8).collect();
        let f = frame(&px, 8, 8);
        let p = Profile::preset("pal-tv").unwrap();
        assert_eq!(render(&f, &p, 2), render(&f, &p, 2));
    }
}
