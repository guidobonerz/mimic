//! Signal stage: the connection between machine and monitor, applied to the
//! raw field before the CRT stage. It changes colours the way the cable and
//! the decoder do, so it can produce colours no palette entry has:
//!
//! - **Composite** (PAL/NTSC from the frame's standard): luma and chroma
//!   share one wire. The chroma is QAM-modulated onto the colour subcarrier
//!   and decoded again; fine luma detail near the subcarrier frequency is
//!   read as colour (cross-colour, NTSC artifact colours) and chroma edges
//!   leak into luma (cross-luma dots).
//! - **S-Video** (luma/chroma): separate wires, no crosstalk; chroma still
//!   has its narrow bandwidth.
//! - **PAL delay line** (composite and S-Video on PAL): the decoder averages
//!   the chroma of each line with the line before. Pairs of lines with the
//!   same luma but different colours blend into a third colour — the "PAL
//!   colour mixing" C64 demos rely on.
//! - **Monochrome**: luma only; the profile's tint colours it later.
//! - **RGB**: no signal stage.
//!
//! Model: each line is sampled at 8× the subcarrier frequency (PAL
//! 4.43361875 MHz, NTSC 3.579545 MHz), on a grid locked to the subcarrier,
//! so carrier values are exact table entries and the decoder's carrier
//! recovery (burst lock) is perfect. The source pixels are held for their
//! duration (the video chips switch levels per pixel). Filters are box
//! filters on running sums; a box of whole subcarrier cycles nulls the
//! unwanted products of demodulation exactly. Luma is separated with a
//! notch: the decoded chroma is modulated again and subtracted. Colour
//! space is ITU-R BT.601 YUV. The carrier phase restarts every field (no
//! dot crawl), which keeps the stage a pure function of the field.
//!
//! Phase reference: frame column 0 of line 0 starts at subcarrier phase
//! 180° (the burst, −U). Machines whose pixel clock is locked to the
//! carrier put their colour-clock boundaries there — the Atari's colour
//! clock is the carrier, and hue 1 is the colour clock undelayed (in phase
//! with the burst) — so a luma pattern lit in the first half of each colour
//! clock shows hue 1 as its artifact colour, the shifted pattern the
//! complementary one.

use emu_core::frame::{
    FrameRef, LumaChroma, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};

use crate::profile::{Profile, Signal};

const PAL_FSC: f64 = 4_433_618.75;
const NTSC_FSC: f64 = 315.0e6 / 88.0;
/// Samples per subcarrier cycle.
const PHASES: usize = 8;
/// sin(22.5° + k·45°): each sample holds the signal for a whole eighth of
/// a cycle, so the carrier is taken at the middle of that interval.
const S1: f32 = 0.382_683_43; // sin 22.5°
const S2: f32 = 0.923_879_5; // sin 67.5°
const SIN: [f32; PHASES] = [S1, S2, S2, S1, -S1, -S2, -S2, -S1];

fn cos(i: usize) -> f32 {
    SIN[(i + 2) % PHASES]
}

/// The signal path a frame takes through `profile`: the monitor cannot
/// get more than the machine delivers. Monochrome monitors show luma of
/// anything; RGB outputs (C128 VDC) stay clean on colour monitors; a
/// composite-only machine (VIC-20, TED) cannot feed an S-Video input.
pub fn effective(profile: &Profile, output: OutputType) -> Signal {
    match (profile.signal, output) {
        (Signal::Monochrome, _) | (_, OutputType::Monochrome) => Signal::Monochrome,
        (_, OutputType::Rgb) | (Signal::Rgb, _) => Signal::Rgb,
        (Signal::SVideo, OutputType::Composite) => Signal::Composite,
        (s, _) => s,
    }
}

#[derive(Clone, Copy, Default)]
struct Yuv {
    y: f32,
    u: f32,
    v: f32,
}

fn to_yuv(c: Rgb8) -> Yuv {
    let (r, g, b) = (
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
    );
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    Yuv {
        y,
        u: 0.492 * (b - y),
        v: 0.877 * (r - y),
    }
}

/// `luma` 0–65535 is black–white; `chroma_amplitude` uses the same scale
/// as luma; phase 0 is the +U axis, a quarter turn is +V.
fn lc_to_yuv(lc: LumaChroma) -> Yuv {
    let a = f64::from(lc.chroma_amplitude) / 65535.0;
    let phi = f64::from(lc.chroma_phase) / 65536.0 * std::f64::consts::TAU;
    Yuv {
        y: (f64::from(lc.luma) / 65535.0) as f32,
        u: (a * phi.cos()) as f32,
        v: (a * phi.sin()) as f32,
    }
}

fn to_rgb(c: Yuv) -> Rgb8 {
    let r = c.y + 1.140 * c.v;
    let g = c.y - 0.395 * c.u - 0.581 * c.v;
    let b = c.y + 2.032 * c.u;
    // Round half up; `round()` is a library call on some targets.
    let q = |x: f32| (x * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    Rgb8 {
        r: q(r),
        g: q(g),
        b: q(b),
    }
}

/// Prefix sums of `src` into `sum` (`sum[k]` = sum of the first k samples).
fn prefix(src: &[f32], sum: &mut Vec<f64>) {
    sum.clear();
    sum.reserve(src.len() + 1);
    sum.push(0.0);
    let mut acc = 0.0f64;
    for &v in src {
        acc += f64::from(v);
        sum.push(acc);
    }
}

/// Centred box of length `len` at sample `k` from prefix sums; near the
/// line edges it averages the samples inside the line.
fn box_at(sum: &[f64], k: usize, len: usize) -> f32 {
    let n = sum.len() - 1;
    let lo = k.saturating_sub(len / 2).min(n.saturating_sub(1));
    let hi = (k + len - len / 2).min(n).max(lo + 1);
    ((sum[hi] - sum[lo]) / (hi - lo) as f64) as f32
}

/// Centred box filter of length `len` over `src` into `dst`.
fn box_filter(src: &[f32], len: usize, dst: &mut Vec<f32>, sum: &mut Vec<f64>) {
    dst.clear();
    if len <= 1 {
        dst.extend_from_slice(src);
        return;
    }
    let n = src.len();
    prefix(src, sum);
    let half = len / 2;
    let tail = len - half;
    if n <= len {
        dst.extend((0..n).map(|k| box_at(sum, k, len)));
        return;
    }
    dst.extend((0..half).map(|k| box_at(sum, k, len)));
    // Interior: whole windows.
    let inv = 1.0 / len as f64;
    dst.extend(
        sum[len..=n]
            .iter()
            .zip(&sum[..=n - len])
            .map(|(hi, lo)| ((hi - lo) * inv) as f32),
    );
    dst.extend((n + 1 - tail..n).map(|k| box_at(sum, k, len)));
}

/// Box of `width` pixels centred at `centre` over a held (piecewise
/// constant) pixel line, exactly, from the line's prefix sums `sum`.
fn box_px(sum: &[f64], vals: &[f32], centre: f64, width: f64) -> f32 {
    let w = vals.len();
    let integral = |t: f64| {
        let i = (t as usize).min(w - 1);
        sum[i] + (t - i as f64) * f64::from(vals[i])
    };
    let lo = (centre - width / 2.0).max(0.0);
    let hi = (centre + width / 2.0).min(w as f64);
    if hi - lo < 1e-9 {
        return vals[(centre as usize).min(w - 1)];
    }
    ((integral(hi) - integral(lo)) / (hi - lo)) as f32
}

/// Box length (in samples at `fs`) whose response falls to about -3 dB at
/// `bandwidth_hz`, rounded to a multiple of `multiple`.
fn box_len(fs: f64, bandwidth_hz: f64, multiple: usize) -> usize {
    if bandwidth_hz <= 0.0 {
        return 1;
    }
    let len = (0.443 * fs / bandwidth_hz / multiple as f64)
        .round()
        .max(1.0) as usize;
    len * multiple
}

/// Accessor for one YUV component.
type Channel = fn(&mut Yuv) -> &mut f32;

/// Per-frame decoder settings and line buffers reused across lines.
struct Decoder {
    pal: bool,
    /// Samples per source pixel.
    per_pixel: f64,
    /// Line period in source pixels (including blanking).
    total: f64,
    luma_len: usize,
    chroma_len: usize,
    /// Subcarrier over the longest line, from every phase.
    carrier_sin: Vec<f32>,
    carrier_cos: Vec<f32>,
    max_n: usize,
    /// Edge samples added on both sides of a line (whole carrier cycles).
    pad: usize,
    y: Vec<f32>,
    u: Vec<f32>,
    v: Vec<f32>,
    s: Vec<f32>,
    t0: Vec<f32>,
    t1: Vec<f32>,
    sum: Vec<f64>,
}

impl Decoder {
    /// Composite: hold, modulate, demodulate, separate; one value per
    /// source pixel (its centre) into `dec`.
    fn composite(&mut self, src: &[Yuv], line: usize, dec: &mut Vec<Yuv>) {
        let w = src.len();
        // First sample on the subcarrier-locked grid inside this line.
        let start = line as f64 * self.total * self.per_pixel;
        let k0 = start.ceil();
        let offset = k0 - start;
        let n = ((w as f64 * self.per_pixel - offset).ceil().max(1.0) as usize).min(self.max_n);
        // + half a cycle: see "Phase reference" above.
        let phase0 = ((k0 as u64 + PHASES as u64 / 2) % PHASES as u64) as usize;
        // The line plus `pad` samples of its edge pixels on both sides, so
        // the filters see whole windows up to the edges. `pad` is whole
        // subcarrier cycles: sample `pad` keeps the phase of `k0`.
        let pad = self.pad;
        let total_n = n + 2 * pad;
        let sin_l = &self.carrier_sin[phase0..phase0 + total_n];
        let cos_l = &self.carrier_cos[phase0..phase0 + total_n];
        // PAL: V is inverted on every other line.
        let sw = if self.pal && line % 2 == 1 { -1.0 } else { 1.0 };

        // Hold each pixel's level for the samples inside its duration.
        self.t0.resize(total_n, 0.0);
        self.u.resize(total_n, 0.0);
        self.v.resize(total_n, 0.0);
        let mut begin = 0;
        for (x, p) in src.iter().enumerate() {
            let end = if x + 1 == w {
                total_n
            } else {
                (pad + ((x + 1) as f64 * self.per_pixel - offset).ceil().max(0.0) as usize)
                    .clamp(begin, pad + n)
            };
            self.t0[begin..end].fill(p.y);
            self.u[begin..end].fill(p.u);
            self.v[begin..end].fill(p.v);
            begin = end;
        }
        // Luma bandwidth of output stage and cable.
        box_filter(&self.t0, self.luma_len, &mut self.y, &mut self.sum);

        // One wire: Y + U sin ωt ± V cos ωt.
        self.s.clear();
        self.s.extend(
            self.y
                .iter()
                .zip(&self.u)
                .zip(&self.v)
                .zip(sin_l.iter().zip(cos_l))
                .map(|(((y, u), v), (s, c))| y + u * s + sw * v * c),
        );
        // Synchronous demodulation, low-pass.
        self.t0.clear();
        self.t0
            .extend(self.s.iter().zip(sin_l).map(|(x, s)| 2.0 * x * s));
        self.t1.clear();
        self.t1
            .extend(self.s.iter().zip(cos_l).map(|(x, c)| 2.0 * sw * x * c));
        box_filter(&self.t0, self.chroma_len, &mut self.u, &mut self.sum);
        box_filter(&self.t1, self.chroma_len, &mut self.v, &mut self.sum);
        // Notch: luma is the signal minus the chroma the decoder saw.
        self.y.clear();
        self.y.extend(
            self.s
                .iter()
                .zip(&self.u)
                .zip(&self.v)
                .zip(sin_l.iter().zip(cos_l))
                .map(|(((x, u), v), (s, c))| x - (u * s + sw * v * c)),
        );

        // Back to one value per source pixel, at the pixel centre.
        dec.clear();
        for x in 0..w {
            let t = ((x as f64 + 0.5) * self.per_pixel - offset).clamp(0.0, (n - 1) as f64);
            let k = pad + t as usize;
            let k1 = k + 1;
            let f = t.fract() as f32;
            let lerp = |a: &[f32]| a[k] + (a[k1] - a[k]) * f;
            dec.push(Yuv {
                y: lerp(&self.y),
                u: lerp(&self.u),
                v: lerp(&self.v),
            });
        }
    }

    /// S-Video: separate wires and no subcarrier, so the filters run on
    /// the held pixel levels directly, in pixel units.
    fn svideo(&mut self, src: &[Yuv], dec: &mut Vec<Yuv>) {
        dec.clear();
        dec.extend_from_slice(src);
        let channels: [(Channel, usize); 3] = [
            (|c| &mut c.y, self.luma_len),
            (|c| &mut c.u, self.chroma_len),
            (|c| &mut c.v, self.chroma_len),
        ];
        for (get, len) in channels {
            if len <= 1 {
                continue;
            }
            let width = len as f64 / self.per_pixel;
            self.t0.clear();
            self.t0.extend(dec.iter_mut().map(|d| *get(d)));
            prefix(&self.t0, &mut self.sum);
            for (x, d) in dec.iter_mut().enumerate() {
                *get(d) = box_px(&self.sum, &self.t0, x as f64 + 0.5, width);
            }
        }
    }
}

/// Run the signal stage. `None` when the frame passes unchanged (RGB);
/// otherwise the decoded field, same size as the source.
pub fn process(frame: &FrameRef<'_>, profile: &Profile) -> Option<Vec<Rgb8>> {
    let signal = effective(profile, frame.signal.output);
    let (w, h) = (frame.width as usize, frame.height as usize);
    if signal == Signal::Rgb || w == 0 || h == 0 {
        return None;
    }
    // Source lines as YUV; palettes are converted once.
    let palette: Vec<Yuv> = match (frame.pixels, frame.luma_chroma) {
        (PixelData::Indexed { palette, .. }, None) => palette.iter().map(|&c| to_yuv(c)).collect(),
        _ => Vec::new(),
    };
    let load = |y: usize, dst: &mut Vec<Yuv>| {
        dst.clear();
        let row = y * w..(y + 1) * w;
        match (frame.pixels, frame.luma_chroma) {
            (_, Some(lc)) if lc.len() >= w * h => {
                dst.extend(lc[row].iter().map(|&c| lc_to_yuv(c)));
            }
            (PixelData::Indexed { pixels, .. }, None) if pixels.len() >= w * h => {
                dst.extend(
                    pixels[row]
                        .iter()
                        .map(|&i| palette.get(usize::from(i)).copied().unwrap_or_default()),
                );
            }
            (PixelData::Rgb(pixels), _) if pixels.len() >= w * h => {
                dst.extend(pixels[row].iter().map(|&c| to_yuv(c)));
            }
            _ => dst.extend(
                (0..w).map(|x| to_yuv(frame.rgb_at(x as u32, y as u32).unwrap_or_default())),
            ),
        }
    };
    let mut src = Vec::with_capacity(w);
    let mut out = Vec::with_capacity(w * h);
    if signal == Signal::Monochrome {
        for y in 0..h {
            load(y, &mut src);
            out.extend(src.iter().map(|p| {
                to_rgb(Yuv {
                    y: p.y,
                    ..Yuv::default()
                })
            }));
        }
        return Some(out);
    }
    let pclk = f64::from(frame.signal.pixel_clock_hz);
    if pclk <= 0.0 {
        return None;
    }
    let pal = frame.signal.standard == VideoStandard::Pal;
    let fsc = if pal { PAL_FSC } else { NTSC_FSC };
    let fs = fsc * PHASES as f64;
    let per_pixel = fs / pclk;
    let max_n = (w as f64 * per_pixel).ceil() as usize + 1;
    let composite = signal == Signal::Composite;
    let luma_len = box_len(fs, f64::from(profile.luma_bandwidth) * 1e6, 1);
    // Whole subcarrier cycles: nulls the luma that demodulation moves to the
    // subcarrier frequency and the chroma product at twice it.
    let chroma_len = box_len(fs, f64::from(profile.chroma_bandwidth) * 1e6, PHASES);
    let pad = (luma_len + chroma_len).div_ceil(PHASES) * PHASES;
    let carrier_len = if composite {
        max_n + 2 * pad + PHASES
    } else {
        0
    };
    let mut dc = Decoder {
        pal,
        per_pixel,
        total: f64::from(frame.signal.total_pixels.max(frame.width)),
        luma_len,
        chroma_len,
        carrier_sin: (0..carrier_len).map(|k| SIN[k % PHASES]).collect(),
        carrier_cos: (0..carrier_len).map(cos).collect(),
        max_n,
        pad,
        y: Vec::new(),
        u: Vec::new(),
        v: Vec::new(),
        s: Vec::new(),
        t0: Vec::new(),
        t1: Vec::new(),
        sum: Vec::new(),
    };
    let delay_line = pal && profile.delay_line;
    let sat = profile.saturation;

    let mut dec = Vec::with_capacity(w);
    let mut prev: Vec<(f32, f32)> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::with_capacity(w);
    for y in 0..h {
        load(y, &mut src);
        if composite {
            dc.composite(&src, y, &mut dec);
        } else {
            dc.svideo(&src, &mut dec);
        }
        cur.clear();
        for (x, d) in dec.iter().enumerate() {
            cur.push((d.u, d.v));
            let (u, v) = match prev.get(x) {
                Some(&(pu, pv)) if delay_line => ((d.u + pu) * 0.5, (d.v + pv) * 0.5),
                _ => (d.u, d.v),
            };
            out.push(to_rgb(Yuv {
                y: d.y,
                u: u * sat,
                v: v * sat,
            }));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    Some(out)
}

/// [`process`] for an RGBA8 field (the desktop's GPU upload format).
/// `None` when the frame passes unchanged.
pub fn process_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    signal: SignalInfo,
    profile: &Profile,
) -> Option<Vec<u8>> {
    if effective(profile, signal.output) == Signal::Rgb {
        return None;
    }
    let pixels: Vec<Rgb8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| Rgb8 {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    if pixels.len() != width as usize * height as usize {
        return None;
    }
    let frame = FrameRef {
        width,
        height,
        pixels: PixelData::Rgb(&pixels),
        luma_chroma: None,
        signal,
    };
    let out = process(&frame, profile)?;
    Some(out.iter().flat_map(|c| [c.r, c.g, c.b, 255]).collect())
}

#[cfg(test)]
mod tests;
