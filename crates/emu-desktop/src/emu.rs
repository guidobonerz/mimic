//! The emulation thread: owns the machine, maps host input, publishes the
//! latest frame and feeds the audio buffer. The UI thread never touches the
//! machine.
//!
//! Pacing: frames run at the machine's exact rate on the wall clock (PAL C64:
//! 50.12 Hz), so they arrive evenly; the audio side follows by adjusting its
//! resampling rate slightly to the buffer level (see `audio`). Wall-clock
//! time only decides *when* frames run, never what happens in them (golden
//! rule 3).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
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
pub struct Shared {
    /// Time base for the timestamps below.
    pub epoch: Instant,
    /// When the window last presented (ns since `epoch`): the display's
    /// vertical blank, for phase-locking the emulation to it.
    pub last_vsync_ns: AtomicU64,
    /// Finished frames in order; the window shows each once.
    pub frames: Mutex<VecDeque<Frame>>,
    pub audio: Mutex<VecDeque<i16>>,
    /// Set by the audio thread when a device is running.
    pub audio_live: AtomicBool,
    pub quit: AtomicBool,
    /// When the display refresh is within 1 % of the machine's frame rate,
    /// the window stores its measured refresh period here (ns). Emulation then
    /// runs at that rate on the wall clock and nudges its phase so a frame is
    /// ready half a refresh before the next vertical blank. 0 = the machine's
    /// own rate. Emulation never waits for the window itself.
    pub display_period_ns: AtomicU64,
    /// Frames the window presented (diagnostics).
    pub presented: AtomicU64,
    /// Window refreshes (diagnostics).
    pub redraws: AtomicU64,
    /// Frames discarded by a backlog (diagnostics).
    pub dropped: AtomicU64,
    /// Audio diagnostics: input samples missing when the device asked, and
    /// the current playback rate correction in parts per million.
    pub underruns: AtomicU64,
    pub rate_ppm: AtomicI64,
    /// Last stop that was not a frame end (jam, fault), for the title bar.
    pub status: Mutex<Option<String>>,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_vsync_ns: AtomicU64::new(0),
            frames: Mutex::default(),
            audio: Mutex::default(),
            audio_live: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            display_period_ns: AtomicU64::new(0),
            presented: AtomicU64::new(0),
            redraws: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            rate_ppm: AtomicI64::new(0),
            status: Mutex::default(),
        }
    }
}

/// Audio kept buffered ahead of the device (48 kHz samples, 100 ms).
pub const AUDIO_TARGET: usize = 4_800;
/// Frames waiting for the window before old ones are dropped.
const MAX_QUEUED_FRAMES: usize = 4;

/// Sleep until `deadline`: the OS sleep for the bulk (its granularity can
/// be coarse), the last stretch by yielding.
fn wait_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > Duration::from_millis(2) {
            thread::sleep(left - Duration::from_millis(1));
        } else {
            thread::yield_now();
        }
    }
}
/// Shift `deadline` a little towards the middle between two vertical
/// blanks (a frame takes a few ms to emulate and must be queued before the
/// next blank). Small steps (1/8 of the error per frame) keep the frame
/// rate itself steady.
fn phase_lock(deadline: Instant, period: u64, shared: &Shared) -> Instant {
    let vsync = shared.last_vsync_ns.load(Ordering::Relaxed);
    if vsync == 0 || deadline < shared.epoch {
        return deadline;
    }
    let at = (deadline - shared.epoch).as_nanos() as i128;
    let p = i128::from(period);
    let phase = (at - i128::from(vsync)).rem_euclid(p);
    let error = phase - p / 2; // −p/2 … p/2
    let nudge = (error / 8).unsigned_abs() as u64;
    if error > 0 {
        deadline - Duration::from_nanos(nudge)
    } else {
        deadline + Duration::from_nanos(nudge)
    }
}

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
        let debug = std::env::var_os("EMU_DESKTOP_DEBUG").is_some();
        let mut report = Instant::now();
        let (mut min_fill, mut max_fill, mut frames) = (usize::MAX, 0, 0u32);
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

            if stopped {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            wait_until(deadline);
            let period = shared.display_period_ns.load(Ordering::Relaxed);
            if period == 0 {
                deadline += frame_time;
            } else {
                deadline += Duration::from_nanos(period);
                deadline = phase_lock(deadline, period, &shared);
            }
            // Fell far behind (slow host, debugger): do not catch up in a burst.
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
                min_fill = min_fill.min(a.len());
                a.extend(samples.iter().copied());
                let excess = a.len().saturating_sub(AUDIO_MAX);
                a.drain(..excess);
                max_fill = max_fill.max(a.len());
            }
            frames += 1;
            if debug && report.elapsed() >= Duration::from_secs(5) {
                eprintln!(
                    "5 s: emulated {frames}, window refreshes {}, presented {}, dropped {}, audio buffer {min_fill}..{max_fill} samples, underruns {}, rate {:+} ppm",
                    shared.redraws.swap(0, Ordering::Relaxed),
                    shared.presented.swap(0, Ordering::Relaxed),
                    shared.dropped.swap(0, Ordering::Relaxed),
                    shared.underruns.swap(0, Ordering::Relaxed),
                    shared.rate_ppm.load(Ordering::Relaxed),
                );
                report = Instant::now();
                (min_fill, max_fill, frames) = (usize::MAX, 0, 0);
            }
            let frame = to_frame(machine.as_ref());
            if let Ok(mut q) = shared.frames.lock() {
                q.push_back(frame);
                while q.len() > MAX_QUEUED_FRAMES {
                    q.pop_front();
                    shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
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
        let shared = Arc::new(Shared::new());
        let handle = spawn(machine, mapper, rx, shared.clone());
        tx.send(HostEvent::Key {
            code: "KeyA".into(),
            down: true,
        })
        .unwrap();
        let mut got = None;
        for _ in 0..400 {
            if let Some(f) = shared.frames.lock().unwrap().pop_front() {
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
