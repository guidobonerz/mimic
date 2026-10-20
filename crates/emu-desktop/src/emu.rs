//! The emulation thread: owns the machine, maps host input, publishes the
//! latest frame and feeds the audio buffer. The UI thread never touches the
//! machine.
//!
//! Pacing: with an audio device the thread keeps ~100 ms of sound buffered
//! (the sound card is the clock); without one it follows the machine's frame
//! duration on the wall clock. Wall-clock time only decides *when* frames
//! run, never what happens in them (golden rule 3).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use emu_core::frame::SignalInfo;
use emu_core::{DebugHooks, Machine, StopReason};
use emu_input::{HostEvent, Mapper};

/// A frame converted to RGBA for upload.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub signal: SignalInfo,
}

/// State shared between the threads.
#[derive(Default)]
pub struct Shared {
    pub frame: Mutex<Option<Frame>>,
    pub audio: Mutex<VecDeque<i16>>,
    /// Set by the audio thread when a device is running.
    pub audio_live: AtomicBool,
    pub quit: AtomicBool,
    /// Last stop that was not a frame end (jam, fault), for the title bar.
    pub status: Mutex<Option<String>>,
}

/// Audio kept buffered ahead of the device (48 kHz samples).
const AUDIO_TARGET: usize = 4_800;
/// Upper bound if nobody consumes audio.
const AUDIO_MAX: usize = 48_000;

fn to_frame(m: &dyn Machine) -> Frame {
    let f = m.framebuffer();
    let mut rgba = Vec::with_capacity(f.width as usize * f.height as usize * 4);
    for y in 0..f.height {
        for x in 0..f.width {
            let c = f.rgb_at(x, y).unwrap_or_default();
            rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
        }
    }
    Frame {
        width: f.width,
        height: f.height,
        rgba,
        signal: f.signal,
    }
}

pub fn spawn(
    mut machine: Box<dyn Machine>,
    mut mapper: Mapper,
    input: Receiver<HostEvent>,
    shared: Arc<Shared>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut dbg = DebugHooks::new();
        let mut samples = Vec::new();
        let frame_time = {
            let s = machine.framebuffer().signal;
            let pixels = u64::from(s.lines_per_field) * u64::from(s.total_pixels);
            Duration::from_nanos(
                pixels.saturating_mul(1_000_000_000) / u64::from(s.pixel_clock_hz.max(1)),
            )
        };
        let mut deadline = Instant::now();
        let mut stopped = false;
        while !shared.quit.load(Ordering::Relaxed) {
            loop {
                match input.try_recv() {
                    Ok(ev) => {
                        let now = machine.now();
                        for e in mapper.handle(&ev, now) {
                            machine.input(e);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            // Run a frame when the audio buffer or the clock asks for one.
            let due = if shared.audio_live.load(Ordering::Relaxed) {
                shared.audio.lock().is_ok_and(|a| a.len() < AUDIO_TARGET)
            } else {
                Instant::now() >= deadline
            };
            if !due || stopped {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            deadline += frame_time;
            if Instant::now() > deadline + frame_time * 5 {
                deadline = Instant::now();
            }

            match machine.run_frame(&mut dbg) {
                StopReason::FrameDone => {}
                other => {
                    // A jammed or faulted machine stays frozen; the window
                    // keeps showing the last frame.
                    stopped = true;
                    if let Ok(mut s) = shared.status.lock() {
                        *s = Some(format!("{other:?}"));
                    }
                }
            }
            samples.clear();
            machine.audio(&mut samples);
            if let Ok(mut a) = shared.audio.lock() {
                a.extend(samples.iter().copied());
                let excess = a.len().saturating_sub(AUDIO_MAX);
                a.drain(..excess);
            }
            if let Ok(mut f) = shared.frame.lock() {
                *f = Some(to_frame(machine.as_ref()));
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;

    #[test]
    fn thread_produces_frames_and_stops() {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("c64", &config).unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("c64"));
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::default());
        let handle = spawn(machine, mapper, rx, shared.clone());
        tx.send(HostEvent::Key {
            code: "KeyA".into(),
            down: true,
        })
        .unwrap();
        let mut got = None;
        for _ in 0..400 {
            if let Some(f) = shared.frame.lock().unwrap().take() {
                got = Some(f);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let f = got.expect("a frame within 2 s");
        assert_eq!(f.rgba.len(), (f.width * f.height * 4) as usize);
        assert!(!shared.audio.lock().unwrap().is_empty(), "audio produced");
        shared.quit.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
