//! Vector-display output (Vectrex): the path the beam drew in one frame,
//! as straight segments with intensity. Machines with a vector display
//! deliver this through [`crate::Machine::vectors`] next to a rasterised
//! [`crate::FrameRef`]; phosphor decay and glow are post-processing in
//! `video-crt` (golden rule 9), the debugger and the preview carry the
//! segments as they are.
//!
//! [`Raster`] turns segments into a picture: every segment adds the beam's
//! energy along its path (intensity × time per pixel), which the phosphor
//! turns into brightness with a saturating curve, per colour channel. The
//! machine draws its raw picture with it (every segment at full weight);
//! `video-crt`'s phosphor stage draws the same segments with weights for
//! the light they give within the frame and the glow they carry into the
//! next one. Two looks ([`Style`]):
//! - **fine**: thin, dimmer lines; where the beam rests while lit (a
//!   line's end before the next one starts, a dot) it gives the phosphor
//!   more energy, and those points glow bright. Ends cut off by blanking
//!   (dashes, text) stay plain.
//! - **soft**: a one-pixel core with a soft edge and a steep curve: even,
//!   bright lines.

use crate::frame::Rgb8;

/// One stretch of the beam's path at a constant speed and intensity.
/// Coordinates are in the frame's units (square: one unit is the same
/// distance on both axes), origin top left, y downwards. A dot the beam
/// rests on has `x0 == x1` and `y0 == y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Segment {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// Beam intensity 0–255 (0 = off; such segments are not delivered).
    pub intensity: u8,
    /// The colour the viewer sees at full intensity: the phosphor's, or
    /// as seen through a filter in front of the screen (the Vectrex 3D
    /// Imager's colour wheel).
    pub color: Rgb8,
    /// How long the beam took, in machine cycles: with the length this
    /// gives the energy per distance, i.e. how bright the trace glows.
    pub cycles: u32,
    /// When the beam started it, in machine cycles from the frame's start
    /// (a segment carried over from the previous frame starts at 0).
    pub at: u32,
}

impl Segment {
    /// The middle of the segment in time, cycles from the frame's start.
    pub fn mid(&self) -> f32 {
        self.at as f32 + self.cycles as f32 / 2.0
    }
}

/// How segments are drawn ([`Raster`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    #[default]
    Fine,
    Soft,
}

/// The segments of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorFrame<'a> {
    /// Size of the visible area in coordinate units.
    pub width: u32,
    pub height: u32,
    pub segments: &'a [Segment],
    /// Colour of the phosphor at full intensity (segments carry the colour
    /// actually seen in [`Segment::color`]).
    pub phosphor: Rgb8,
    /// Machine cycles per second, to turn `Segment::cycles` into time.
    pub cycle_hz: u32,
    /// Machine cycles the frame lasts.
    pub frame_cycles: u32,
    /// Frames since power-on.
    pub frame: u64,
    /// The look the machine draws its own picture with.
    pub style: Style,
}

/// An owned copy of a [`VectorFrame`] (for the preview's frame copies and
/// the debugger, which must not hold the machine).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VectorBuf {
    pub width: u32,
    pub height: u32,
    pub segments: Vec<Segment>,
    pub phosphor: Rgb8,
    pub cycle_hz: u32,
    pub frame_cycles: u32,
    pub frame: u64,
    pub style: Style,
}

impl VectorBuf {
    pub fn from_frame(f: &VectorFrame<'_>) -> Self {
        Self {
            width: f.width,
            height: f.height,
            segments: f.segments.to_vec(),
            phosphor: f.phosphor,
            cycle_hz: f.cycle_hz,
            frame_cycles: f.frame_cycles,
            frame: f.frame,
            style: f.style,
        }
    }

    pub fn as_frame(&self) -> VectorFrame<'_> {
        VectorFrame {
            width: self.width,
            height: self.height,
            segments: &self.segments,
            phosphor: self.phosphor,
            cycle_hz: self.cycle_hz,
            frame_cycles: self.frame_cycles,
            frame: self.frame,
            style: self.style,
        }
    }
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
    /// at 1 − e⁻¹ of full brightness. A Vectrex line at full intensity and
    /// the fastest common beam speed (BIOS scale $7F) gets about 0.33
    /// cycles a pixel: bright in the soft style, about two thirds in the
    /// fine one.
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

/// A picture of `width` × `height` pixels, `unit` coordinate units a
/// pixel, drawn in `style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub unit: i32,
    pub style: Style,
}

impl Raster {
    /// Adds the energy of `segments`, each times `weight(segment)`, into
    /// `energy` (one entry a pixel, row by row).
    pub fn deposit(
        &self,
        segments: &[Segment],
        energy: &mut [[f32; 3]],
        weight: impl Fn(&Segment) -> f32,
    ) {
        let (w, h) = (self.width as i32, self.height as i32);
        let unit = self.unit.max(1) as f32;
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
            let k = weight(s);
            if k <= 0.0 {
                continue;
            }
            let i = f32::from(s.intensity) / 255.0 * k;
            let rgb = [s.color.r, s.color.g, s.color.b].map(|c| f32::from(c) / 255.0 * i);
            let (x0, y0) = (s.x0 as f32 / unit, s.y0 as f32 / unit);
            let (x1, y1) = (s.x1 as f32 / unit, s.y1 as f32 / unit);
            let len = (x1 - x0).abs().max((y1 - y0).abs());
            if self.style == Style::Fine && len < 1.0 {
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
                add(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, e, self.style.spot());
            }
        }
    }

    /// The phosphor's brightness for the deposited `energy`.
    pub fn tone(&self, energy: &[[f32; 3]], pixels: &mut [Rgb8]) {
        let scale = self.style.energy();
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

    /// The raw picture: every segment at full weight. `energy` is scratch
    /// space of the picture's size.
    pub fn draw(&self, segments: &[Segment], energy: &mut [[f32; 3]], pixels: &mut [Rgb8]) {
        energy.fill([0.0; 3]);
        self.deposit(segments, energy, |_| 1.0);
        self.tone(energy, pixels);
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
    const W: u32 = 400;
    const H: u32 = 400;

    fn line() -> Segment {
        Segment {
            x0: 100,
            y0: 0,
            x1: 200,
            y1: 0,
            intensity: 255,
            color: WHITE,
            cycles: 33,
            at: 0,
        }
    }

    fn picture(segs: &[Segment], style: Style) -> Vec<Rgb8> {
        let n = (W * H) as usize;
        let mut energy = vec![[0.0; 3]; n];
        let mut px = vec![Rgb8::default(); n];
        let r = Raster {
            width: W,
            height: H,
            unit: 1,
            style,
        };
        r.draw(segs, &mut energy, &mut px);
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
            at: 40,
        };
        let px = picture(&[line(), dot], Style::Soft);
        assert!(px[150].g > 200, "line pixel {:?}", px[150]);
        let d = px[200 * W as usize + 200];
        assert!(d.r > 200 && d.g == 0, "red dot {d:?}");
        assert_eq!(px[300 * W as usize + 300], Rgb8::default());
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
        assert!(fine[W as usize + 150].g < middle / 3);
    }

    #[test]
    fn weights_scale_the_deposit() {
        let r = Raster {
            width: W,
            height: H,
            unit: 1,
            style: Style::Soft,
        };
        let mut full = vec![[0.0; 3]; (W * H) as usize];
        let mut half = full.clone();
        r.deposit(&[line()], &mut full, |_| 1.0);
        r.deposit(&[line()], &mut half, |_| 0.5);
        assert!((full[150][1] - 2.0 * half[150][1]).abs() < 1e-6);
        assert_eq!(line().mid(), 16.5);
    }
}
