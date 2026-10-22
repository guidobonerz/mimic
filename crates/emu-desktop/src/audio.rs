//! Sound output with cpal: plays the machine's 48 kHz stream (stereo
//! frames; mono machines fill both sides) on the default device, resampled
//! linearly to its rate. A stereo or multichannel device gets left and
//! right on its first two channels and the mix on the others; a mono
//! device the mix.
//!
//! The emulation runs on the wall clock, the sound card on its own crystal;
//! the two drift apart. The resampler compensates by playing up to 0.5 %
//! faster or slower depending on how full the buffer is (inaudible).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::emu::Shared;

/// Linear resampler state from 48 kHz to the device rate.
struct Resampler {
    /// Nominal input samples per output sample.
    base: f64,
    step: f64,
    /// Buffer level, smoothed over ~2 s: frames arrive in 960-sample
    /// steps, and reacting to each would wobble the pitch.
    fill: f64,
    /// Output samples until the next rate adjustment.
    until_adjust: u32,
    pos: f64,
    prev: [f32; 2],
    next: [f32; 2],
}

impl Resampler {
    fn adjust(&mut self, shared: &Shared) {
        let fill = shared.audio.lock().map_or(0, |a| a.len()) as f64;
        self.fill += (fill - self.fill) * 0.01;
        let target = crate::emu::AUDIO_TARGET as f64;
        let error = ((self.fill - target) / target).clamp(-1.0, 1.0);
        self.step = self.base * (1.0 + 0.005 * error);
        shared.rate_ppm.store(
            ((self.step / self.base - 1.0) * 1e6) as i64,
            Ordering::Relaxed,
        );
    }

    fn sample(&mut self, shared: &Shared) -> [f32; 2] {
        if self.until_adjust == 0 {
            self.adjust(shared);
            self.until_adjust = 1024;
        }
        self.until_adjust -= 1;
        self.pos += self.step;
        while self.pos >= 1.0 {
            self.pos -= 1.0;
            self.prev = self.next;
            let s = shared.audio.lock().ok().and_then(|mut a| a.pop_front());
            match s {
                Some([l, r]) => self.next = [f32::from(l) / 32_768.0, f32::from(r) / 32_768.0],
                // Underrun: hold the level. Dropping to 0 would click,
                // the 6581 output sits on a large DC offset.
                None => {
                    shared.underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let t = self.pos as f32;
        [
            self.prev[0] + (self.next[0] - self.prev[0]) * t,
            self.prev[1] + (self.next[1] - self.prev[1]) * t,
        ]
    }
}

/// Fill one device frame from a stereo sample.
fn spread<T: Copy>(frame: &mut [T], [l, r]: [f32; 2], conv: impl Fn(f32) -> T) {
    match frame {
        [only] => *only = conv((l + r) / 2.0),
        [left, right, rest @ ..] => {
            *left = conv(l);
            *right = conv(r);
            rest.fill(conv((l + r) / 2.0));
        }
        [] => {}
    }
}

/// Start the output stream; `None` (silent, clock-paced emulation) if there
/// is no usable device.
pub fn start(shared: Arc<Shared>) -> Option<cpal::Stream> {
    let device = cpal::default_host().default_output_device()?;
    let supported = device.default_output_config().ok()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = usize::from(config.channels.max(1));
    let base = f64::from(emu_core::SAMPLE_RATE) / f64::from(config.sample_rate.0);
    let mut rs = Resampler {
        base,
        step: base,
        fill: crate::emu::AUDIO_TARGET as f64,
        until_adjust: 0,
        pos: 0.0,
        prev: [0.0; 2],
        next: [0.0; 2],
    };
    let err = |e| eprintln!("audio error: {e}");
    let s2 = shared.clone();
    let stream = match format {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(channels) {
                        spread(frame, rs.sample(&s2), |v| v);
                    }
                },
                err,
                None,
            )
            .ok()?,
        cpal::SampleFormat::I16 => device
            .build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    for frame in data.chunks_mut(channels) {
                        spread(frame, rs.sample(&s2), |v| (v * 32_767.0) as i16);
                    }
                },
                err,
                None,
            )
            .ok()?,
        _ => return None,
    };
    // Start with the target latency in the buffer, so the rate control does
    // not have to fill it slowly (crackles at start).
    if let Ok(mut a) = shared.audio.lock() {
        a.extend(std::iter::repeat_n([0, 0], crate::emu::AUDIO_TARGET));
    }
    stream.play().ok()?;
    shared.audio_live.store(true, Ordering::Relaxed);
    Some(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_frames_spread_onto_device_channels() {
        let mut mono = [0.0f32];
        spread(&mut mono, [1.0, 0.0], |v| v);
        assert_eq!(mono, [0.5]);
        let mut stereo = [0.0f32; 2];
        spread(&mut stereo, [1.0, -1.0], |v| v);
        assert_eq!(stereo, [1.0, -1.0]);
        let mut surround = [0i16; 4];
        spread(&mut surround, [0.5, 0.0], |v| (v * 100.0) as i16);
        assert_eq!(surround, [50, 0, 25, 25]);
    }
}
