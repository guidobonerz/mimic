//! Phosphor stage for vector displays (the Vectrex): the beam segments of
//! a frame ([`emu_core::VectorFrame`]) drawn with the afterglow of each
//! from the moment the beam drew it.
//!
//! **Model.** The beam excites the phosphor; the excitation decays as
//! e^(−t/τ) and gives off its energy as light while it decays. A frame
//! shows the light given off during it (what the eye gathers over one
//! frame of `F` seconds):
//!
//! - a segment drawn at time `t` (its middle) gives
//!   `1 − e^(−(F−t)/τ)` of its energy within the frame, the rest
//!   (`e^(−(F−t)/τ)`) is still glowing at the frame's end;
//! - the glow carried from earlier frames gives `1 − e^(−F/τ)` of itself
//!   and keeps `e^(−F/τ)`.
//!
//! Energy is conserved: a still picture is as bright as without the stage,
//! τ → 0 gives the machine's own picture, and moving lines leave trails
//! that fade with τ. The light goes through the same rasteriser and
//! saturating curve as the machine's picture ([`emu_core::vector::Raster`]).
//! A gap in the frame numbers (frames not shown) lets the glow decay over
//! the missing frames, an earlier frame number (a loaded savestate,
//! rewind) starts without glow. The stage keeps its own state, never machine state
//! (savestates stay CRT-independent); it is a frontend stage like
//! [`crate::field::FieldStage`].

use emu_core::frame::Rgb8;
use emu_core::vector::{Raster, VectorFrame};

#[derive(Clone, Debug, Default)]
pub struct VectorPhosphor {
    width: u32,
    height: u32,
    /// The excitation left at the end of the last frame (energy units).
    glow: Vec<[f32; 3]>,
    /// Scratch: the light of the frame.
    light: Vec<[f32; 3]>,
    out: Vec<Rgb8>,
    /// The last frame number seen.
    last: Option<u64>,
}

impl VectorPhosphor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the glow (a reset, a loaded savestate, another machine).
    pub fn reset(&mut self) {
        self.glow.iter_mut().for_each(|g| *g = [0.0; 3]);
        self.last = None;
    }

    /// The picture of `frame` at `width` × `height` pixels (the size of
    /// the machine's own picture) with a phosphor of decay time `tau_ms`.
    /// The same frame number twice (a paused machine) shows the same
    /// picture again.
    pub fn process(
        &mut self,
        frame: &VectorFrame<'_>,
        width: u32,
        height: u32,
        tau_ms: f32,
    ) -> (u32, u32, &[Rgb8]) {
        let n = (width as usize) * (height as usize);
        if (width, height) != (self.width, self.height) || self.glow.len() != n {
            self.width = width;
            self.height = height;
            self.glow = vec![[0.0; 3]; n];
            self.light = vec![[0.0; 3]; n];
            self.out = vec![Rgb8::default(); n];
            self.last = None;
        }
        if self.last == Some(frame.frame) {
            return (width, height, &self.out);
        }
        if self.last.is_some_and(|l| frame.frame < l) {
            // Back in time (a loaded savestate, rewind): no glow from the
            // frames that are gone.
            self.reset();
        }
        let missed = match self.last {
            Some(l) if frame.frame > l => (frame.frame - l - 1).min(1000) as f32,
            _ => 0.0,
        };
        self.last = Some(frame.frame);
        let hz = frame.cycle_hz.max(1) as f32;
        let f = frame.frame_cycles as f32 / hz;
        let tau = (tau_ms / 1000.0).max(1e-6);
        // The glow of frames not shown decays unseen.
        let lost = (-missed * f / tau).exp();
        let keep = (-f / tau).exp();
        for (l, g) in self.light.iter_mut().zip(self.glow.iter_mut()) {
            for c in 0..3 {
                let e = g[c] * lost;
                l[c] = e * (1.0 - keep);
                g[c] = e * keep;
            }
        }
        let unit = (frame.width / width.max(1)).max(1) as i32;
        let raster = Raster {
            width,
            height,
            unit,
            style: frame.style,
        };
        let left = |mid: f32| (-(f - (mid / hz).min(f)) / tau).exp();
        raster.deposit(frame.segments, &mut self.light, |s| 1.0 - left(s.mid()));
        raster.deposit(frame.segments, &mut self.glow, |s| left(s.mid()));
        raster.tone(&self.light, &mut self.out);
        (width, height, &self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::vector::{Segment, Style};

    const WHITE: Rgb8 = Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    };

    fn seg(x: i32, at: u32) -> Segment {
        Segment {
            x0: x,
            y0: 50,
            x1: x,
            y1: 150,
            intensity: 255,
            color: WHITE,
            cycles: 30,
            at,
        }
    }

    fn frame(segs: &[Segment], n: u64) -> VectorFrame<'_> {
        VectorFrame {
            width: 200,
            height: 200,
            segments: segs,
            phosphor: WHITE,
            cycle_hz: 1_500_000,
            frame_cycles: 30_000,
            frame: n,
            style: Style::Soft,
        }
    }

    fn raw(segs: &[Segment]) -> Vec<Rgb8> {
        let r = Raster {
            width: 200,
            height: 200,
            unit: 1,
            style: Style::Soft,
        };
        let mut e = vec![[0.0; 3]; 200 * 200];
        let mut px = vec![Rgb8::default(); 200 * 200];
        r.draw(segs, &mut e, &mut px);
        px
    }

    fn at(px: &[Rgb8], x: usize) -> u8 {
        px[100 * 200 + x].g
    }

    #[test]
    fn a_short_decay_gives_the_raw_picture() {
        let segs = [seg(20, 0), seg(60, 29_000)];
        let mut p = VectorPhosphor::new();
        let (_, _, px) = p.process(&frame(&segs, 1), 200, 200, 0.001);
        assert_eq!(px, raw(&segs).as_slice());
    }

    #[test]
    fn late_lines_show_less_now_and_glow_into_the_next_frame() {
        let segs = [seg(20, 0), seg(60, 29_000)];
        let mut p = VectorPhosphor::new();
        let first = p.process(&frame(&segs, 1), 200, 200, 6.0).2.to_vec();
        assert!(at(&first, 20) > at(&first, 60), "the late line is dimmer");
        // An empty frame: the trail of what was drawn, the late line most.
        let next = p.process(&frame(&[], 2), 200, 200, 6.0).2.to_vec();
        assert!(at(&next, 60) > at(&next, 20), "the late line glows on");
        assert!(at(&next, 60) > 0);
        // It fades.
        let later = p.process(&frame(&[], 3), 200, 200, 6.0).2.to_vec();
        assert!(at(&later, 60) < at(&next, 60));
    }

    #[test]
    fn a_still_picture_settles_at_its_raw_brightness() {
        let segs = [seg(60, 15_000)];
        let mut p = VectorPhosphor::new();
        for n in 1..40 {
            p.process(&frame(&segs, n), 200, 200, 6.0);
        }
        let px = p.process(&frame(&segs, 40), 200, 200, 6.0).2.to_vec();
        let want = at(&raw(&segs), 60);
        assert!(at(&px, 60).abs_diff(want) <= 1, "{} vs {want}", at(&px, 60));
    }

    #[test]
    fn the_same_frame_again_shows_the_same_picture_and_reset_forgets() {
        let segs = [seg(60, 29_000)];
        let mut p = VectorPhosphor::new();
        let a = p.process(&frame(&segs, 1), 200, 200, 6.0).2.to_vec();
        let b = p.process(&frame(&segs, 1), 200, 200, 6.0).2.to_vec();
        assert_eq!(a, b);
        p.process(&frame(&[], 2), 200, 200, 6.0);
        p.reset();
        let c = p.process(&frame(&[], 3), 200, 200, 6.0).2.to_vec();
        assert!(c.iter().all(|&p| p == Rgb8::default()));
    }
}
