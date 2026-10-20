//! Sound output with cpal: plays the machine's 48 kHz mono stream on the
//! default device, resampled linearly to its rate and channel count.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::emu::Shared;

/// Linear resampler state from 48 kHz to the device rate.
struct Resampler {
    step: f64,
    pos: f64,
    prev: f32,
    next: f32,
}

impl Resampler {
    fn sample(&mut self, shared: &Shared) -> f32 {
        self.pos += self.step;
        while self.pos >= 1.0 {
            self.pos -= 1.0;
            self.prev = self.next;
            let s = shared
                .audio
                .lock()
                .ok()
                .and_then(|mut a| a.pop_front())
                .unwrap_or(0);
            self.next = f32::from(s) / 32_768.0;
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
    let mut rs = Resampler {
        step: f64::from(emu_core::SAMPLE_RATE) / f64::from(config.sample_rate.0),
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
    stream.play().ok()?;
    shared.audio_live.store(true, Ordering::Relaxed);
    Some(stream)
}
