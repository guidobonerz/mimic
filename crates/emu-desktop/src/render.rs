//! The render thread: owns the GPU side ([`Gfx`]) and presents frames.
//!
//! Presenting blocks until the display's vertical blank (FIFO, one frame
//! in flight), nearly the whole refresh period. Done on the window's
//! event-loop thread, that held back every mouse and keyboard event by up
//! to a refresh and showed each frame a refresh late. So this thread waits
//! for the newest frame from the emulation, uploads it with the footer
//! strip the window composed, draws and presents, and the event loop stays
//! free to pass input on at once. After each present it records the
//! vertical blank and measures the refresh period for the emulation's
//! phase lock (`emu::phase_lock`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use video_crt::Profile;
use winit::window::Window;

use crate::emu::Shared;
use crate::gfx::Gfx;

/// The panel and footer rows under the picture, in window pixels.
pub struct Strip {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub rows: u32,
}

/// Where the picture lies in the window: the frame's size and the
/// viewport (x, y, width, height) in window pixels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Geometry {
    pub frame: (u32, u32),
    pub viewport: (u32, u32, u32, u32),
}

#[derive(Default)]
struct Commands {
    resize: Option<(u32, u32)>,
    profile: Option<Profile>,
    /// `Some(None)` hides the strip.
    strip: Option<Option<Strip>>,
}

struct Inner {
    shared: Arc<Shared>,
    commands: Mutex<Commands>,
    geometry: Mutex<Geometry>,
    stop: AtomicBool,
}

/// Handle of the render thread, held by the window.
pub struct Renderer {
    inner: Arc<Inner>,
    thread: Option<JoinHandle<()>>,
}

/// Without frames (paused machine, picker) the window is redrawn at this
/// interval, so footer, panel and resize changes show; also the pause
/// after a lost surface.
const IDLE_REDRAW: Duration = Duration::from_millis(50);
/// FIFO: how long the thread waits at a blank for a frame not there yet.
const LATE_FRAME: Duration = Duration::from_millis(5);

impl Renderer {
    /// Create the GPU side for `window` and start the thread.
    pub fn start(
        window: Arc<Window>,
        profile: Profile,
        shared: Arc<Shared>,
        scale: u32,
    ) -> Result<Self, String> {
        let gfx = Gfx::new(window, profile)?;
        let inner = Arc::new(Inner {
            shared,
            commands: Mutex::default(),
            geometry: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let worker = inner.clone();
        let thread = thread::Builder::new()
            .name("render".into())
            .spawn(move || run(gfx, &worker, scale))
            .map_err(|e| format!("render thread: {e}"))?;
        Ok(Self {
            inner,
            thread: Some(thread),
        })
    }

    fn command(&self, f: impl FnOnce(&mut Commands)) {
        if let Ok(mut c) = self.inner.commands.lock() {
            f(&mut c);
        }
        self.inner.shared.wake_render();
    }

    pub fn resize(&self, width: u32, height: u32) {
        self.command(|c| c.resize = Some((width, height)));
    }

    pub fn set_profile(&self, profile: Profile) {
        self.command(|c| c.profile = Some(profile));
    }

    /// The strip under the picture, or `None` for none.
    pub fn set_strip(&self, strip: Option<Strip>) {
        self.command(|c| c.strip = Some(strip));
    }

    /// The frame pixel under window position (`x`, `y`), if the pointer is
    /// over the picture.
    pub fn frame_pos(&self, x: f64, y: f64) -> Option<(u32, u32)> {
        let g = *self.inner.geometry.lock().ok()?;
        let (fw, fh) = g.frame;
        let (vx, vy, vw, vh) = g.viewport;
        if fw == 0 || vw == 0 {
            return None;
        }
        let (px, py) = (x - f64::from(vx), y - f64::from(vy));
        if px < 0.0 || py < 0.0 || px >= f64::from(vw) || py >= f64::from(vh) {
            return None;
        }
        Some((
            (px * f64::from(fw) / f64::from(vw)) as u32,
            (py * f64::from(fh) / f64::from(vh)) as u32,
        ))
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        self.inner.shared.wake_render();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Measures the display's refresh period from the vertical blanks.
#[derive(Default)]
struct Vsync {
    /// The first vertical blank (ns since the epoch) and the refresh
    /// periods counted since.
    start_ns: u64,
    periods: u64,
}

impl Vsync {
    /// Record a vertical blank at `at` (the acquire just returned) and
    /// refine the measured refresh period.
    ///
    /// The period is the time since a start divided by the refresh periods
    /// counted since, each interval rounded to whole periods: the blocking
    /// calls return with jitter (one late, the next early) and sometimes
    /// skip a refresh, which averages out over the long run. (Averaging
    /// only the intervals within 10 % of the nominal period kept the early
    /// returns and dropped the late ones: 18.8 ms measured on a 50 Hz
    /// display, the emulation ran 6 % fast, the audio buffer filled up and
    /// input lagged.)
    fn blank(&mut self, shared: &Shared, at: Instant) {
        let now = at.saturating_duration_since(shared.epoch).as_nanos() as u64;
        let last = shared.last_vsync_ns.swap(now, Ordering::Relaxed);
        let nominal = shared.nominal_period_ns.load(Ordering::Relaxed);
        if nominal == 0 || last == 0 {
            return;
        }
        let nominal = nominal as f64;
        let periods = ((now - last) as f64 / nominal).round() as u64;
        // A long pause (window hidden, debugger) or no pacing at all
        // (nothing blocked): measure afresh.
        if self.start_ns == 0 || periods == 0 || periods > 8 {
            self.start_ns = now;
            self.periods = 0;
            return;
        }
        self.periods += periods;
        // Enough refreshes for a stable mean (about 4 s at 60 Hz).
        if self.periods >= 240 {
            let measured = (now - self.start_ns) as f64 / self.periods as f64;
            if (measured / nominal - 1.0).abs() < 0.02 {
                shared
                    .display_period_ns
                    .store(measured as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Resize and profile commands, applied before an acquire (a resize
/// configures the surface anew); the strip at any time.
fn apply_commands(gfx: &mut Gfx, inner: &Inner, before_acquire: bool) {
    let Ok(mut c) = inner.commands.lock() else {
        return;
    };
    if before_acquire {
        if let Some((w, h)) = c.resize.take() {
            gfx.resize(w, h);
        }
    }
    if let Some(p) = c.profile.take() {
        gfx.set_profile(p);
    }
    if let Some(strip) = c.strip.take() {
        gfx.set_footer(strip.as_ref().map(|s| (s.rgba.as_slice(), s.width, s.rows)));
    }
}

/// The thread. With mailbox presents (`Gfx::presents_block` false) it
/// presents every frame the moment the emulation has it, and the display
/// shows it at its next vertical blank; nothing blocks, so no vertical
/// blank is seen and the emulation runs free at the display's nominal
/// rate. With FIFO presents it presents at every refresh with the newest
/// frame there is: the acquire (or the present) blocks until the display
/// took the previous frame, which paces the loop, and its return is taken
/// as the vertical blank for the phase lock. (The compositor releases
/// FIFO presents in bursts, so the blanks seen jitter and frames may
/// queue up: a late frame is waited for a few ms, a backlog of more than
/// one is skipped.)
fn run(mut gfx: Gfx, inner: &Inner, scale: u32) {
    let shared = &inner.shared;
    let fifo = gfx.presents_block();
    let mut vsync = Vsync::default();
    let stopped = || inner.stop.load(Ordering::Relaxed) || shared.quit.load(Ordering::Relaxed);
    while !stopped() {
        // Mailbox: wait for a frame (or redraw now and then for the strip).
        // FIFO: the acquire paces; a frame that is late by a few ms is
        // waited for, so it is not shown a refresh late.
        let (target, frame) = {
            let target = if fifo {
                let t = gfx.acquire();
                vsync.blank(shared, Instant::now());
                Some(t)
            } else {
                None
            };
            let Ok(mut q) = shared.frames.lock() else {
                return;
            };
            let until = Instant::now() + if fifo { LATE_FRAME } else { IDLE_REDRAW };
            while q.is_empty() && !stopped() {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let Ok((guard, _)) = shared.frame_ready.wait_timeout(q, left) else {
                    return;
                };
                q = guard;
            }
            while q.len() > 1 {
                q.pop_front();
                shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            (target, q.pop_front())
        };
        if stopped() {
            break;
        }
        apply_commands(&mut gfx, inner, true);
        if let Some(f) = &frame {
            gfx.upload(f, scale);
            shared.presented.fetch_add(1, Ordering::Relaxed);
        }
        if let Ok(mut g) = inner.geometry.lock() {
            *g = Geometry {
                frame: gfx.frame_size(),
                viewport: gfx.viewport(),
            };
        }
        let target = target.unwrap_or_else(|| gfx.acquire());
        let Some(target) = target else {
            thread::sleep(IDLE_REDRAW);
            continue;
        };
        gfx.draw(target);
        shared.redraws.fetch_add(1, Ordering::Relaxed);
    }
}
