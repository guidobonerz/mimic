//! Sound output with cpal: plays the machine's 48 kHz mono stream on the
//! default device, resampled linearly to its rate and channel count.
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
    prev: f32,
    next: f32,
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

    fn sample(&mut self, shared: &Shared) -> f32 {
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
                Some(s) => self.next = f32::from(s) / 32_768.0,
                // Underrun: hold the level. Dropping to 0 would click,
                // the 6581 output sits on a large DC offset.
                None => {
                    shared.underruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.prev + (self.next - self.prev) * self.pos as f32
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
        prev: 0.0,
        next: 0.0,
    };
    let err = |e| eprintln!("audio error: {e}");
    let s2 = shared.clone();
    let stream = match format {
        cpal::SampleFormat::F32 => device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(channels) {
                        frame.fill(rs.sample(&s2));
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
                        frame.fill((rs.sample(&s2) * 32_767.0) as i16);
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
        a.extend(std::iter::repeat_n(0, crate::emu::AUDIO_TARGET));
    }
    stream.play().ok()?;
    shared.audio_live.store(true, Ordering::Relaxed);
    Some(stream)
}
