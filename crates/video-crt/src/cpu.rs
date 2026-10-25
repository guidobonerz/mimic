//! CPU path: a deterministic CRT approximation for headless use, preview
//! snapshots (`filter=crt`) and CI. The GPU shader (`shaders/crt.wgsl`)
//! follows the same model, step by step.
//!
//! Geometry: every field line becomes `scale` output rows (a PAL/NTSC
//! field shows each line twice on a 576/480-line screen, so `scale = 2` is
//! the natural size). The width follows the pixel aspect ratio derived from
//! the pixel clock against the ITU-R BT.601 square-pixel rates (PAL
//! 14.75 MHz, NTSC 12.2727 MHz for 576/480 lines): for the C64 PAL
//! (7.882 MHz) that is 0.936. [`render_size`] renders to any output size
//! (non-integer scales); the scanline count always follows the emulated
//! line count, never the output height.
//!
//! The CRT stage, per output pixel (centre `(x + 0.5, y + 0.5)`):
//!
//! 1. **Curvature** (`curvature` k): the screen position `c` in −1…1 is
//!    moved outward, `c · (1 + k·|c|²) / (1 + k)`: the middle of each
//!    edge stays, the picture bulges, the corners fall off the glass
//!    (black).
//! 2. **Beam**: the field line under the (curved) position, linearly
//!    resampled along the line; the beam is narrower for dark colours, so
//!    `scanlines` darkens the gap between lines more for dark pixels.
//! 3. **Bloom** (halation): a fixed 7-tap glow of the source (centre 0.2,
//!    ±1 pixel 0.15, ±2 pixels 0.1, the lines above and below 0.15), its
//!    square times `bloom` added on top of the beam: bright areas glow
//!    into their neighbours and fill the gaps between scanlines, dark
//!    areas hardly glow.
//! 4. **Tint** (monochrome phosphors).
//! 5. **Mask** in screen pixels (fixed to the glass, not curved): within a
//!    triad of `mask_pitch` pixels each phosphor's weight is a triangle,
//!    1 at its stripe centre falling to 0 one stripe away (the three
//!    weights add up to 1 at every position, so non-integer pitches blend
//!    instead of aliasing); a channel keeps `1 − strength·(1 − weight)`.
//!    Aperture grille: stripes only. Shadow mask: every row of `pitch / 2`
//!    pixels shifted by half a triad (dot triads). Slot mask: rows of
//!    `2·pitch` pixels shifted by half a triad, the last eighth of a row a
//!    dark gap (`1 − strength`). Half the lost light is made up by a gain
//!    of `1 / (1 − strength/3)`.
//! 6. **Vignette**: `1 − vignette · |c|² / 2` (1 − vignette in the corners).
//! 7. Clamp, display gamma.
//! 8. **Persistence** (stateful [`Crt`] only): each value is the brighter
//!    of the new picture and the previous output times `persistence`, in
//!    display (gamma-encoded) values: a still picture stays as it is,
//!    moving bright objects leave decaying trails.
//!
//! Everything is plain arithmetic on the frame: no randomness, no clock.
//! Sources: see `docs/sources.md` ("CRT stage").

use emu_core::frame::{FieldParity, FrameRef, PixelData, VideoStandard};

use crate::profile::{Mask, Profile};

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

/// Glow tap weights: centre, ±1 pixel, ±2 pixels, the lines above/below.
/// They add up to 1. The shader uses the same numbers.
pub const GLOW: [f32; 4] = [0.2, 0.15, 0.1, 0.15];

/// Per-channel mask factors at triad phase `t` (0…1), before the gain.
fn mask_weights(t: f32, strength: f32) -> [f32; 3] {
    let mut m = [0f32; 3];
    for (i, v) in m.iter_mut().enumerate() {
        let centre = (i as f32 + 0.5) / 3.0;
        let d = (t - centre).abs();
        let d = d.min(1.0 - d) * 3.0;
        let w = (1.0 - d).clamp(0.0, 1.0);
        *v = 1.0 - strength * (1.0 - w);
    }
    m
}

/// Mask row of screen row centre `y`: (shifted by half a triad, slot gap).
fn mask_row(mask: Mask, pitch: f32, y: f32) -> (bool, bool) {
    match mask {
        Mask::None | Mask::Aperture => (false, false),
        Mask::Shadow => {
            let row = (y / (pitch * 0.5)).floor();
            (row.rem_euclid(2.0) >= 1.0, false)
        }
        Mask::Slot => {
            let h = pitch * 2.0;
            let row = (y / h).floor();
            (row.rem_euclid(2.0) >= 1.0, y / h - row > 0.875)
        }
    }
}

/// Mask factors (gain included) of the screen pixel centred at (`x`, `y`)
/// in output pixels; all 1 without a mask.
pub fn mask_at(profile: &Profile, x: f32, y: f32) -> [f32; 3] {
    let s = profile.mask_strength.clamp(0.0, 1.0);
    if profile.mask == Mask::None || s == 0.0 {
        return [1.0; 3];
    }
    let pitch = profile.mask_pitch.max(1.0);
    let (shift, gap) = mask_row(profile.mask, pitch, y);
    mask_factors(x, pitch, s, shift, gap)
}

fn mask_factors(x: f32, pitch: f32, s: f32, shift: bool, gap: bool) -> [f32; 3] {
    let u = x / pitch + if shift { 0.5 } else { 0.0 };
    let gain = 1.0 / (1.0 - s / 3.0);
    let gap = if gap { 1.0 - s } else { 1.0 };
    mask_weights(u - u.floor(), s).map(|m| m * gain * gap)
}

/// The source field as floats (0…255) and, with bloom, its glow.
struct Source {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
    glow: Vec<[f32; 3]>,
}

impl Source {
    fn new(frame: &FrameRef<'_>, bloom: bool) -> Self {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let mut px = Vec::with_capacity(w * h);
        for y in 0..frame.height {
            for x in 0..frame.width {
                let c = frame.rgb_at(x, y).unwrap_or_default();
                px.push([f32::from(c.r), f32::from(c.g), f32::from(c.b)]);
            }
        }
        let mut glow = Vec::new();
        if bloom {
            glow.reserve(w * h);
            let at = |x: isize, y: isize| -> [f32; 3] {
                let x = x.clamp(0, w as isize - 1) as usize;
                let y = y.clamp(0, h as isize - 1) as usize;
                px[y * w + x]
            };
            for y in 0..h as isize {
                for x in 0..w as isize {
                    let taps = [
                        (at(x, y), GLOW[0]),
                        (at(x - 1, y), GLOW[1]),
                        (at(x + 1, y), GLOW[1]),
                        (at(x - 2, y), GLOW[2]),
                        (at(x + 2, y), GLOW[2]),
                        (at(x, y - 1), GLOW[3]),
                        (at(x, y + 1), GLOW[3]),
                    ];
                    let mut g = [0f32; 3];
                    for (c, k) in taps {
                        for ch in 0..3 {
                            g[ch] += c[ch] * k;
                        }
                    }
                    glow.push(g);
                }
            }
        }
        Self { w, h, px, glow }
    }
}

/// Render at vertical `scale` (see [`output_size`]). Stateless: the
/// profile's `persistence` needs [`Crt`].
pub fn render(frame: &FrameRef<'_>, profile: &Profile, scale: u32) -> Image {
    let (width, height) = output_size(frame, profile, scale);
    render_size(frame, profile, width, height)
}

/// Render to an output of `width` × `height` pixels (any scale, also
/// non-integer). Stateless: the profile's `persistence` needs [`Crt`].
pub fn render_size(frame: &FrameRef<'_>, profile: &Profile, width: u32, height: u32) -> Image {
    let decoded = crate::signal::process(frame, profile);
    let decoded_frame;
    let frame = match &decoded {
        Some(pixels) => {
            decoded_frame = FrameRef {
                pixels: PixelData::Rgb(pixels),
                luma_chroma: None,
                ..*frame
            };
            &decoded_frame
        }
        None => frame,
    };
    let (width, height) = (width.max(1), height.max(1));
    let mut rgb = vec![0u8; width as usize * height as usize * 3];
    if frame.width == 0 || frame.height == 0 {
        return Image { width, height, rgb };
    }
    let bloom = profile.bloom.clamp(0.0, 1.0);
    let src = Source::new(frame, bloom > 0.0);
    let lut = gamma_lut(profile.gamma);
    let curvature = profile.curvature.clamp(0.0, 0.25);
    let vignette = profile.vignette.clamp(0.0, 1.0);
    let (wf, hf) = (f64::from(width), f64::from(height));
    let step = f64::from(frame.width) / wf;
    let scale_y = hf / f64::from(frame.height);
    let out_w = width as usize;

    // Mask factors per column for the two row phases (plain, shifted).
    let strength = profile.mask_strength.clamp(0.0, 1.0);
    let masked = profile.mask != Mask::None && strength > 0.0;
    let pitch = profile.mask_pitch.max(1.0);
    let mask_cols: [Vec<[f32; 3]>; 2] = if masked {
        [false, true].map(|shift| {
            (0..width)
                .map(|x| mask_factors(x as f32 + 0.5, pitch, strength, shift, false))
                .collect()
        })
    } else {
        [Vec::new(), Vec::new()]
    };

    let stage = Stage {
        src: &src,
        lut: &lut,
        scanlines: profile.scanlines,
        bloom,
        tint: profile.tint,
        vignette,
    };
    // Where along the line (x0, x1, weight of x1) and which field line
    // with the position inside it, for a point in output pixels.
    // Positions are never negative here, so truncation is `floor` (which
    // is a library call on x86-64 without SSE4.1).
    let column = |fx: f64| {
        let pos = (fx * step - 0.5).max(0.0);
        let whole = pos as usize;
        let x0 = whole.min(src.w - 1);
        (x0, (x0 + 1).min(src.w - 1), (pos - whole as f64) as f32)
    };
    let line = |fy: f64| {
        let ly = fy.max(0.0) / scale_y;
        let whole = ly as usize;
        let sy = whole.min(src.h - 1);
        // Position inside the line: 0 at the beam centre, 1 at the gap.
        let offset = if scale_y > 1.0 {
            ((ly - whole as f64) * 2.0 - 1.0).abs() as f32
        } else {
            0.0
        };
        (sy, offset)
    };
    // Screen position −1…1.
    let screen = |o: u32, size: f64| (2.0 * (f64::from(o) + 0.5) / size - 1.0) as f32;
    let flat_cols: Vec<((usize, usize, f32), f32)> = if curvature > 0.0 {
        Vec::new()
    } else {
        (0..width)
            .map(|x| (column(f64::from(x) + 0.5), screen(x, wf).powi(2)))
            .collect()
    };

    let draw_row = |oy: u32, out_row: &mut [u8]| {
        let (shift, gap) = if masked {
            mask_row(profile.mask, pitch, oy as f32 + 0.5)
        } else {
            (false, false)
        };
        let row_mask = &mask_cols[usize::from(shift)];
        let row_gap = if gap { 1.0 - strength } else { 1.0 };
        let mask = |ox: usize| {
            if masked {
                let m = row_mask[ox];
                Some([m[0] * row_gap, m[1] * row_gap, m[2] * row_gap])
            } else {
                None
            }
        };
        if curvature == 0.0 {
            let (sy, offset) = line(f64::from(oy) + 0.5);
            let cy2 = screen(oy, hf).powi(2);
            let cells = out_row.as_chunks_mut::<3>().0.iter_mut().zip(&flat_cols);
            for (ox, (out, &(col, cx2))) in cells.enumerate() {
                *out = stage.shade(col, sy, offset, cx2 + cy2, mask(ox));
            }
        } else {
            let cy = screen(oy, hf);
            for (ox, out) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let cx = screen(ox as u32, wf);
                let f = (1.0 + curvature * (cx * cx + cy * cy)) / (1.0 + curvature);
                let c = [cx * f, cy * f];
                if c[0].abs() > 1.0 || c[1].abs() > 1.0 {
                    out.fill(lut[0]);
                    continue;
                }
                let fx = f64::from((c[0] + 1.0) * 0.5) * wf;
                let fy = f64::from((c[1] + 1.0) * 0.5) * hf;
                let (sy, offset) = line(fy);
                let r2 = c[0] * c[0] + c[1] * c[1];
                *out = stage.shade(column(fx), sy, offset, r2, mask(ox));
            }
        }
    };
    for_rows(&mut rgb, out_w * 3, draw_row);
    Image { width, height, rgb }
}

/// Run `draw(row index, row bytes)` over every row. Rows are independent,
/// so on native targets bands of rows go to scoped threads; the result is
/// the same as drawing them one after the other.
fn for_rows<F>(rgb: &mut [u8], row_bytes: usize, draw: F)
where
    F: Fn(u32, &mut [u8]) + Sync,
{
    let rows = rgb.len() / row_bytes.max(1);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8);
        if threads > 1 && rows >= 64 {
            let band = rows.div_ceil(threads);
            let draw = &draw;
            std::thread::scope(|scope| {
                for (b, chunk) in rgb.chunks_mut(band * row_bytes).enumerate() {
                    scope.spawn(move || {
                        for (i, row) in chunk.chunks_exact_mut(row_bytes).enumerate() {
                            draw((b * band + i) as u32, row);
                        }
                    });
                }
            });
            return;
        }
    }
    for (i, row) in rgb.chunks_exact_mut(row_bytes).enumerate().take(rows) {
        draw(i as u32, row);
    }
}

/// `x.round().clamp(0.0, 255.0) as u8` (ties away from zero), the same
/// for every input (NaN gives 0), without the library call `round` is on
/// x86-64 without SSE4.1: adding 2²³ rounds to the nearest integer (ties
/// to even), a tie that went down is corrected.
#[inline(always)]
fn round_u8(x: f32) -> usize {
    const M: f32 = 8_388_608.0;
    let x = x.clamp(0.0, 255.0);
    let r = (x + M) - M;
    let r = if x - r == 0.5 { r + 1.0 } else { r };
    r as u8 as usize
}

/// The per-pixel part of the CRT stage (beam, bloom, tint, mask,
/// vignette, gamma) once the source position is known.
struct Stage<'a> {
    src: &'a Source,
    lut: &'a [u8; 256],
    scanlines: f32,
    bloom: f32,
    tint: [f32; 3],
    vignette: f32,
}

impl Stage<'_> {
    /// `col` = (x0, x1, weight of x1) on field line `sy`; `offset` inside
    /// the line (0 centre, 1 gap); `r2` = |c|² of the screen position.
    #[inline(always)]
    fn shade(
        &self,
        col: (usize, usize, f32),
        sy: usize,
        offset: f32,
        r2: f32,
        mask: Option<[f32; 3]>,
    ) -> [u8; 3] {
        let (x0, x1, t) = col;
        let row = sy * self.src.w;
        let a = self.src.px[row + x0];
        let b = self.src.px[row + x1];
        let c = [
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ];
        let luma = (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) / 255.0;
        // Bright pixels make a wider beam: they lose less at the edge.
        let beam = 0.5 + 0.5 * luma;
        let falloff = (offset / beam.max(0.01)).min(1.0);
        let k = 1.0 - self.scanlines * falloff * falloff;
        let mut v = [c[0] * k, c[1] * k, c[2] * k];
        if self.bloom > 0.0 {
            let ga = self.src.glow[row + x0];
            let gb = self.src.glow[row + x1];
            for ch in 0..3 {
                let g = ga[ch] * (1.0 - t) + gb[ch] * t;
                v[ch] += self.bloom * g * g / 255.0;
            }
        }
        for (x, tint) in v.iter_mut().zip(self.tint) {
            *x *= tint;
        }
        if let Some(m) = mask {
            for (x, m) in v.iter_mut().zip(m) {
                *x *= m;
            }
        }
        if self.vignette > 0.0 {
            let vig = (1.0 - self.vignette * 0.5 * r2).max(0.0);
            for x in &mut v {
                *x *= vig;
            }
        }
        v.map(|x| self.lut[round_u8(x)])
    }
}

/// Stateful CPU renderer: keeps the previous output for phosphor
/// persistence. The history lives here, never in machine state, so
/// savestates stay CRT-independent. Use one per picture stream; call
/// [`Crt::reset`] when the stream jumps (machine reset, savestate load).
#[derive(Clone, Debug, Default)]
pub struct Crt {
    last: Option<Image>,
}

impl Crt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the previous output.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// [`render`] with persistence.
    pub fn render(&mut self, frame: &FrameRef<'_>, profile: &Profile, scale: u32) -> &Image {
        let (width, height) = output_size(frame, profile, scale);
        self.render_size(frame, profile, width, height)
    }

    /// [`render_size`] with persistence. A size change starts a new
    /// history.
    pub fn render_size(
        &mut self,
        frame: &FrameRef<'_>,
        profile: &Profile,
        width: u32,
        height: u32,
    ) -> &Image {
        let mut img = render_size(frame, profile, width, height);
        let keep = profile.persistence.clamp(0.0, 1.0);
        if let Some(prev) = &self.last {
            if keep > 0.0 && (prev.width, prev.height) == (img.width, img.height) {
                apply_persistence(&mut img.rgb, &prev.rgb, keep);
            }
        }
        self.last.insert(img)
    }
}

/// Phosphor decay on display values: `out = max(out, prev · keep)`.
pub fn apply_persistence(out: &mut [u8], prev: &[u8], keep: f32) {
    for (o, &p) in out.iter_mut().zip(prev) {
        let d = (f32::from(p) * keep).round() as u8;
        *o = (*o).max(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{OutputType, Rgb8, SignalInfo};

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

    fn raw() -> Profile {
        Profile::preset("raw").unwrap()
    }

    fn sum(img: &Image) -> u64 {
        img.rgb.iter().map(|&v| u64::from(v)).sum()
    }

    #[test]
    fn c64_pal_pixel_aspect() {
        let px = [0u8; 4];
        let f = frame(&px, 2, 2);
        assert!((pixel_aspect(&f) - 0.9357).abs() < 0.001);
        let mut p = Profile::preset("1702").unwrap();
        p.aspect = false;
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
        let img = render(&f, &raw(), 3);
        assert_eq!((img.width, img.height), (6, 6));
        assert_eq!(img.pixel(0, 0), [0, 0, 0]);
        assert_eq!(img.pixel(5, 0), [255, 255, 255]);
        assert_eq!(img.pixel(1, 5), [255, 255, 255]);
        // Scale 1 is the frame itself.
        let img = render(&f, &raw(), 1);
        assert_eq!(img.rgb, [0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0]);
    }

    #[test]
    fn scanlines_darken_gaps_more_for_dark_colours() {
        let px = vec![1u8; 16];
        let f = frame(&px, 4, 4);
        let mut p = raw();
        p.scanlines = 0.8;
        let img = render(&f, &p, 4);
        let centre = img.pixel(0, 1)[0];
        let gap = img.pixel(0, 3)[0];
        assert!(gap < centre, "gap {gap} centre {centre}");
        // White: wide beam, the gap keeps most of its brightness.
        assert!(gap > 100, "{gap}");
    }

    #[test]
    fn scanlines_follow_the_line_count_at_non_integer_scale() {
        let px = vec![1u8; 8 * 10];
        let f = frame(&px, 8, 10);
        let mut p = raw();
        p.scanlines = 0.9;
        for h in [25, 27, 33] {
            let img = render_size(&f, &p, 12, h);
            let col: Vec<u8> = (0..h).map(|y| img.pixel(3, y)[0]).collect();
            // A local maximum per field line (the beam centres).
            let peaks = (0..h as usize)
                .filter(|&y| {
                    let l = if y == 0 { 0 } else { col[y - 1] };
                    let r = col.get(y + 1).copied().unwrap_or(0);
                    col[y] > l && col[y] >= r
                })
                .count();
            assert_eq!(peaks, 10, "height {h}: {col:?}");
        }
    }

    #[test]
    fn aperture_mask_separates_the_phosphors() {
        let px = vec![1u8; 16];
        let f = frame(&px, 4, 4);
        let mut p = raw();
        p.mask = Mask::Aperture;
        p.mask_strength = 1.0;
        p.mask_pitch = 3.0;
        let img = render(&f, &p, 3);
        // Pitch 3: one pixel per phosphor stripe.
        assert_eq!(img.pixel(0, 0), [255, 0, 0]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0]);
        assert_eq!(img.pixel(2, 0), [0, 0, 255]);
        assert_eq!(img.pixel(3, 5), [255, 0, 0], "stripes run down");
        // Weaker mask: the other phosphors keep 1 − strength (+ gain).
        p.mask_strength = 0.3;
        let img = render(&f, &p, 3);
        let [r, g, b] = img.pixel(0, 0);
        assert!(r == 255 && g == b && (180..200).contains(&g), "{r} {g} {b}");
    }

    #[test]
    fn mask_weights_blend_at_non_integer_pitch() {
        let p = Profile {
            mask: Mask::Aperture,
            mask_strength: 1.0,
            mask_pitch: 2.5,
            ..raw()
        };
        for x in 0..40 {
            let m = mask_at(&p, x as f32 + 0.5, 0.5);
            let total: f32 = m.iter().sum();
            // Weights add up to 1 (times the gain 1.5) everywhere.
            assert!((total - 1.5).abs() < 1e-4, "x {x}: {m:?}");
        }
    }

    #[test]
    fn shadow_mask_staggers_rows_and_slot_mask_has_gaps() {
        let mut p = Profile {
            mask: Mask::Shadow,
            mask_strength: 1.0,
            mask_pitch: 6.0,
            ..raw()
        };
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        // Rows of 3 pixels, every second shifted by half a triad (3 px).
        assert!(close(mask_at(&p, 0.5, 0.5), mask_at(&p, 3.5, 3.5)));
        assert!(!close(mask_at(&p, 0.5, 0.5), mask_at(&p, 0.5, 3.5)));
        p.mask = Mask::Slot;
        // Rows of 12 pixels; the last one and a half are the gap.
        let lit: f32 = mask_at(&p, 0.5, 5.5).iter().sum();
        let gap: f32 = mask_at(&p, 0.5, 11.5).iter().sum();
        assert!(lit > 1.0 && gap == 0.0, "{lit} {gap}");
        assert!(close(mask_at(&p, 0.5, 0.5), mask_at(&p, 3.5, 12.5)));
    }

    #[test]
    fn curvature_rounds_the_corners_and_keeps_the_middle() {
        let px = vec![1u8; 32 * 24];
        let f = frame(&px, 32, 24);
        let mut p = raw();
        p.curvature = 0.2;
        let img = render(&f, &p, 2);
        assert_eq!((img.width, img.height), (64, 48));
        assert_eq!(img.pixel(0, 0), [0, 0, 0], "corner off the glass");
        assert_eq!(img.pixel(63, 47), [0, 0, 0]);
        assert_eq!(img.pixel(32, 24), [255; 3]);
        // The middle of each edge stays on the picture.
        assert_eq!(img.pixel(0, 24), [255; 3]);
        assert_eq!(img.pixel(32, 0), [255; 3]);
        // Lines bend: a horizontal line near the top moves inward at the
        // sides (the picture bulges).
        let mut px = vec![0u8; 32 * 24];
        px[2 * 32..3 * 32].fill(1);
        let f = frame(&px, 32, 24);
        let img = render(&f, &p, 2);
        let row_of = |x: u32| (0..48).find(|&y| img.pixel(x, y)[0] > 128).unwrap();
        assert!(row_of(8) > row_of(32), "{} {}", row_of(8), row_of(32));
    }

    #[test]
    fn vignette_darkens_the_corners() {
        let px = vec![1u8; 16 * 16];
        let f = frame(&px, 16, 16);
        let mut p = raw();
        p.vignette = 0.5;
        let img = render(&f, &p, 2);
        let centre = img.pixel(16, 16)[0];
        let corner = img.pixel(0, 0)[0];
        assert!(centre > 250, "{centre}");
        assert!((120..150).contains(&corner), "{corner}");
    }

    #[test]
    fn bloom_glows_into_neighbours_and_scanline_gaps() {
        // A white column on black.
        let mut px = vec![0u8; 16 * 8];
        for y in 0..8 {
            px[y * 16 + 8] = 1;
        }
        let f = frame(&px, 16, 8);
        let mut p = raw();
        p.scanlines = 0.6;
        let plain = render(&f, &p, 1);
        p.bloom = 0.8;
        let glow = render(&f, &p, 1);
        assert_eq!(plain.pixel(10, 4), [0, 0, 0]);
        assert!(glow.pixel(10, 4)[0] > 0, "glow two pixels away");
        assert!(glow.pixel(9, 4)[0] > glow.pixel(10, 4)[0]);
        assert_eq!(glow.pixel(13, 4), [0, 0, 0], "fixed reach");
        // Dark areas hardly glow (the glow is squared).
        let grey = [1u8; 64];
        let f = frame(&grey, 8, 8);
        let mut p = raw();
        p.scanlines = 0.9;
        let gap_plain = render(&f, &p, 4).pixel(4, 3)[0];
        p.bloom = 0.5;
        let gap_glow = render(&f, &p, 4).pixel(4, 3)[0];
        assert!(gap_glow > gap_plain, "{gap_glow} {gap_plain}");
    }

    #[test]
    fn persistence_keeps_a_decaying_trail_in_the_stateful_renderer() {
        let white = [1u8; 16];
        let black = [0u8; 16];
        let mut p = raw();
        p.persistence = 0.5;
        // Stateless: no history.
        assert_eq!(sum(&render(&frame(&black, 4, 4), &p, 1)), 0);
        let mut crt = Crt::new();
        crt.render(&frame(&white, 4, 4), &p, 1);
        let first = crt.render(&frame(&black, 4, 4), &p, 1).pixel(1, 1);
        assert_eq!(first, [128; 3]);
        let second = crt.render(&frame(&black, 4, 4), &p, 1).pixel(1, 1);
        assert_eq!(second, [64; 3]);
        // A still picture stays as it is.
        let mut crt = Crt::new();
        for _ in 0..3 {
            assert_eq!(
                crt.render(&frame(&white, 4, 4), &p, 1).pixel(0, 0),
                [255; 3]
            );
        }
        // A size change starts a new history.
        let img = crt.render(&frame(&black, 4, 4), &p, 2).clone();
        assert_eq!(sum(&img), 0);
        // So does a reset; no persistence keeps nothing.
        crt.render(&frame(&white, 4, 4), &p, 1);
        crt.reset();
        assert_eq!(sum(crt.render(&frame(&black, 4, 4), &p, 1)), 0);
        p.persistence = 0.0;
        crt.render(&frame(&white, 4, 4), &p, 1);
        assert_eq!(sum(crt.render(&frame(&black, 4, 4), &p, 1)), 0);
    }

    #[test]
    fn every_preset_renders_at_odd_sizes() {
        let px: Vec<u8> = (0..40 * 30).map(|i| (i % 3 == 0) as u8).collect();
        let f = frame(&px, 40, 30);
        for name in Profile::PRESETS {
            let p = Profile::preset(name).unwrap();
            for (w, h) in [(1, 1), (37, 29), (95, 71)] {
                let img = render_size(&f, &p, w, h);
                assert_eq!(img.rgb.len(), (w * h * 3) as usize, "{name}");
            }
        }
    }

    #[test]
    fn fast_rounding_matches_round() {
        let mut x = -3.0f32;
        while x < 260.0 {
            for v in [x, x.next_up(), x.next_down()] {
                assert_eq!(
                    round_u8(v),
                    usize::from(v.round().clamp(0.0, 255.0) as u8),
                    "{v}"
                );
            }
            x += 0.125;
        }
        for v in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            0.499_999_97,
            254.5,
        ] {
            assert_eq!(
                round_u8(v),
                usize::from(v.round().clamp(0.0, 255.0) as u8),
                "{v}"
            );
        }
    }

    #[test]
    fn deterministic() {
        let px: Vec<u8> = (0..64).map(|i| (i % 2) as u8).collect();
        let f = frame(&px, 8, 8);
        for name in Profile::PRESETS {
            let p = Profile::preset(name).unwrap();
            assert_eq!(render(&f, &p, 2), render(&f, &p, 2), "{name}");
            let (mut a, mut b) = (Crt::new(), Crt::new());
            for _ in 0..2 {
                assert_eq!(a.render(&f, &p, 3), b.render(&f, &p, 3), "{name}");
            }
        }
    }
}
