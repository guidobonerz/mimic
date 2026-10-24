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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use emu_core::frame::SignalInfo;
use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::{ControlId, ControlInfo, Machine, ScreenId, StopReason};
use emu_debug::protocol::Command;
use emu_debug::{Endpoint, Service, Session};
use emu_input::{HostEvent, JoyKeys, Mapper};
use emu_preview::server::FrameTap;
use video_crt::field::FieldStage;

use crate::footer::{Footer, Tracker};

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
    /// Interlace weaving for [`Shared::frames`].
    pub fields: Mutex<FieldStage>,
    /// Stereo frames (left, right) at 48 kHz; mono machines fill both.
    pub audio: Mutex<VecDeque<[i16; 2]>>,
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
    /// Set by the window (F12): show the machine's next screen (C128: VIC
    /// and VDC).
    pub next_screen: AtomicBool,
    /// Set by the window (F9–F11): a datasette key ([`TapeKey`]), 0 = none.
    pub tape_key: AtomicU8,
    /// The disks the first drive swaps through (from `--disk`), the first
    /// one inserted at start.
    pub disk_swap: Mutex<Vec<String>>,
    /// Set by the window (Ctrl+D): put the next disk of
    /// [`Shared::disk_swap`] into the first drive.
    pub next_disk: AtomicBool,
    /// Image files dropped onto the window, for the first drive.
    pub dropped_disks: Mutex<Vec<String>>,
    /// Set by the window (Ctrl+Space): the status footer is shown, so the
    /// emulation publishes [`Shared::footer`] after every frame.
    pub footer_on: AtomicBool,
    pub footer: Mutex<Footer>,
    /// Set by the window (Ctrl+P): the console panel is shown, so the
    /// emulation publishes [`Shared::controls`] after every frame.
    pub panel_on: AtomicBool,
    pub controls: Mutex<Vec<ControlInfo>>,
    /// Panel clicks from the window, (control, value); queued as input.
    pub panel_events: Mutex<Vec<(u16, u8)>>,
    /// Which host keys play the joystick (index into `JoyKeys::ALL`); set by
    /// the window's panel, applied to the mapper by the emulation thread.
    pub joy_keys: AtomicU8,
    /// The port the joysticks play (0-based), and how many joystick ports
    /// the machine has (the panel offers the switch from two on).
    pub joy_port: AtomicU8,
    pub joy_ports: AtomicU8,
    /// Set by the window's PAUSE button: stop the machine, or let it run
    /// again. Goes through the debug session like the debugger's `pause`
    /// and `resume`, so attached debuggers see the stop.
    pub pause_toggle: AtomicBool,
    /// The machine is stopped (by the button or a debugger); published by
    /// the emulation thread for the button's face.
    pub paused: AtomicBool,
    /// Emulation speed set with the footer's slider: an index into
    /// [`crate::footer::SPEEDS`]. Off 100 % the sound is muted.
    pub speed: AtomicU8,
    /// Set by the footer's snapshot button: save the shown screen as PNG
    /// (named after [`Shared::snapshot_name`] in the working folder).
    pub snapshot: AtomicBool,
    pub snapshot_name: Mutex<String>,
    /// Where snapshots go (`settings.snapshots`; none: the working folder).
    pub snapshot_dir: Mutex<Option<std::path::PathBuf>>,
    /// The last snapshot's message for the footer and when it was saved.
    pub snapshot_done: Mutex<Option<(String, Instant)>>,
}

/// Datasette keys from the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TapeKey {
    Play = 1,
    Record = 2,
    Stop = 3,
    Rewind = 4,
}

impl TapeKey {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Play,
            2 => Self::Record,
            3 => Self::Stop,
            4 => Self::Rewind,
            _ => return None,
        })
    }

    fn command(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Record => "record",
            Self::Stop => "stop",
            Self::Rewind => "rewind",
        }
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_vsync_ns: AtomicU64::new(0),
            frames: Mutex::default(),
            fields: Mutex::default(),
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
            next_screen: AtomicBool::new(false),
            tape_key: AtomicU8::new(0),
            disk_swap: Mutex::default(),
            next_disk: AtomicBool::new(false),
            dropped_disks: Mutex::default(),
            footer_on: AtomicBool::new(false),
            footer: Mutex::default(),
            panel_on: AtomicBool::new(false),
            controls: Mutex::default(),
            panel_events: Mutex::default(),
            joy_keys: AtomicU8::new(0),
            joy_port: AtomicU8::new(0),
            joy_ports: AtomicU8::new(0),
            pause_toggle: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            speed: AtomicU8::new(crate::footer::NORMAL_SPEED as u8),
            snapshot: AtomicBool::new(false),
            snapshot_name: Mutex::new(String::from("mimic")),
            snapshot_dir: Mutex::new(None),
            snapshot_done: Mutex::new(None),
        }
    }
}

/// Audio kept buffered ahead of the device (48 kHz frames, 100 ms).
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

/// The screen the window shows: the one chosen with F12, else the one the
/// machine marks active (C128: the 40/80 key).
fn shown_screen(m: &dyn Machine, chosen: Option<ScreenId>) -> ScreenId {
    let screens = m.screens();
    chosen
        .filter(|c| screens.iter().any(|s| s.id == *c))
        .or_else(|| screens.iter().find(|s| s.active).map(|s| s.id))
        .unwrap_or(ScreenId::MAIN)
}

/// The screen after `current` in the machine's list (wrapping).
/// Put the image at `path` into the machine's first disk drive (the old
/// disk is ejected, which writes it back through its backend); returns
/// the message for the title bar.
fn swap_disk(m: &mut dyn Machine, path: &str) -> String {
    let Some(info) = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Disk)
    else {
        return "no disk drive".into();
    };
    let name = file_name(path);
    let media = match media_backends::image::open(std::path::Path::new(path)) {
        Ok(m) if m.kind == emu_core::media::MediaKind::Disk => m,
        Ok(_) => return format!("{name}: not a disk image"),
        Err(e) => return format!("{name}: {e}"),
    };
    let Some(slot) = m.media_slot(info.id) else {
        return "no disk drive".into();
    };
    let _ = slot.eject();
    match slot.insert(media) {
        Ok(()) => format!("{} {name}", info.name),
        Err(e) => format!("{name}: {e}"),
    }
}

/// Put a dropped cartridge image into the machine's cartridge slot.
fn insert_cart(m: &mut dyn Machine, path: &str) -> String {
    let name = file_name(path);
    let Some(info) = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Cartridge)
    else {
        return format!("{name}: no cartridge slot");
    };
    let media = match media_backends::image::open_cart(std::path::Path::new(path)) {
        Ok(m) => m,
        Err(e) => return format!("{name}: {e}"),
    };
    let Some(slot) = m.media_slot(info.id) else {
        return format!("{name}: no cartridge slot");
    };
    let _ = slot.eject();
    match slot.insert(media) {
        Ok(()) => format!("{} {name}", info.name),
        Err(e) => format!("{name}: {e}"),
    }
}

/// Frames the first drive stays empty between taking a disk out and putting
/// the next one in (about 2 s, as by hand). Software that polls the drive
/// now and then (Desert Dream checks DF0: about once a second) only sees a
/// change when its poll finds the drive empty: with a disk inside, the head
/// step of the poll resets the drive's change signal right away.
const SWAP_FRAMES: u64 = 100;

/// Take the disk out of the machine's first disk drive; returns the
/// drive's name.
fn eject_first(m: &mut dyn Machine) -> Option<&'static str> {
    let info = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == emu_core::media::MediaKind::Disk)?;
    let _ = m.media_slot(info.id)?.eject();
    Some(info.name)
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

fn next_screen(m: &dyn Machine, current: ScreenId) -> ScreenId {
    let screens = m.screens();
    let i = screens.iter().position(|s| s.id == current).unwrap_or(0);
    screens[(i + 1) % screens.len()].id
}

fn to_frame(m: &dyn Machine, screen: ScreenId, fields: &mut FieldStage) -> Frame {
    let Some(f) = m.screen(screen) else {
        return to_frame(m, ScreenId::MAIN, fields);
    };
    // Interlaced fields (Amiga LACE) are woven into full frames; the
    // field parity stays in the signal, so the aspect counts one row per
    // line.
    if let Some((width, height, px)) = fields.process(&f) {
        let rgba = px.iter().flat_map(|c| [c.r, c.g, c.b, 255]).collect();
        return Frame {
            width,
            height,
            rgba,
            signal: f.signal,
        };
    }
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

/// Status line for the title bar while the machine is stopped.
fn stop_status(reason: StopReason, pc: Option<u32>) -> String {
    let at = pc.map(|pc| format!(" at ${pc:04X}")).unwrap_or_default();
    match reason {
        StopReason::Breakpoint(id) => format!("breakpoint {}{at}", id.0),
        StopReason::Watchpoint {
            id, addr, write, ..
        } => {
            let kind = if write { "write" } else { "read" };
            format!("watchpoint {} ({kind} ${addr:04X}){at}", id.0)
        }
        StopReason::CpuJam { device, pc } => format!("{} jammed at ${pc:04X}", device.0),
        StopReason::Fault { device, reason } => format!("{} fault: {reason}", device.0),
        StopReason::FrameDone | StopReason::StepDone => format!("paused{at}"),
    }
}

fn main_pc(m: &mut dyn Machine) -> Option<u32> {
    let dev = *m.devices().first()?;
    m.debug_target(dev)?.regs().get("PC")
}

/// Run the emulation thread. With `debug`, the debug server's endpoint is
/// served from here: debugger commands run between frames, and while the
/// debugger holds the machine the window shows the current (partial) frame
/// after every change.
pub fn spawn(
    machine: Box<dyn Machine>,
    mut mapper: Mapper,
    input: Receiver<HostEvent>,
    shared: Arc<Shared>,
    debug: Option<(Endpoint, String)>,
    preview: Option<FrameTap>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let (mut endpoint, machine_id) = match debug {
            Some((e, id)) => (Some(e), id),
            None => (None, String::new()),
        };
        let mut session = Session::new(machine, machine_id);
        session.set_running(true);
        session.set_host(Box::new(crate::host::FrontendHost));
        if let Some(tap) = preview.clone() {
            session.set_frame_hook(Box::new(move |m, running| tap.publish(m, running)));
        }
        let mut samples = Vec::new();
        let frame_time = session.frame_duration();
        let mut deadline = Instant::now();
        let mut shown_at = None;
        let debug_stats = std::env::var_os("EMU_DESKTOP_DEBUG").is_some();
        let mut report = Instant::now();
        let mut chosen: Option<ScreenId> = None;
        // Index of the disk in the first drive within `Shared::disk_swap`.
        let mut disk_at = 0usize;
        // A disk on its way into the first drive and the frame it goes in;
        // frames run so far (the swap pause counts emulated frames).
        let mut pending_disk: Option<(String, u64)> = None;
        let mut frames_run = 0u64;
        let mut tracker = Tracker::default();
        let (mut min_fill, mut max_fill, mut frames) = (usize::MAX, 0, 0u32);
        while !shared.quit.load(Ordering::Relaxed) {
            loop {
                match input.try_recv() {
                    Ok(ev) => {
                        let now = session.machine().now();
                        for e in mapper.handle(&ev, now) {
                            tracker.observe(&e);
                            session.input(e);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            if let Some(key) = TapeKey::from_u8(shared.tape_key.swap(0, Ordering::Relaxed)) {
                let m = session.machine_mut();
                let tape = m
                    .media_slots()
                    .into_iter()
                    .find(|s| s.kind == emu_core::media::MediaKind::Tape);
                if let Some(slot) = tape.and_then(|s| m.media_slot(s.id)) {
                    if let Err(e) = slot.control(key.command()) {
                        *shared.status.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(format!("tape: {e}"));
                    }
                }
            }
            let mut disk = None;
            if shared.next_disk.swap(false, Ordering::Relaxed) {
                if let Ok(list) = shared.disk_swap.lock() {
                    if list.len() > 1 {
                        disk_at = (disk_at + 1) % list.len();
                        disk = Some(list[disk_at].clone());
                    } else {
                        // Say why nothing happens instead of ignoring the key.
                        *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(
                            "Ctrl+D: no other disk (give --disk more than once or drop an image on the window)"
                                .into(),
                        );
                    }
                }
            }
            let dropped = shared
                .dropped_disks
                .lock()
                .map(|mut d| std::mem::take(&mut *d))
                .unwrap_or_default();
            // Cartridges go straight into the cartridge slot (the machine
            // powers up again with them), anything else into the drive.
            let (carts, dropped): (Vec<String>, Vec<String>) = dropped.into_iter().partition(|p| {
                media_backends::image::kind_for(std::path::Path::new(p))
                    == Some(emu_core::media::MediaKind::Cartridge)
            });
            if let Some(path) = carts.last() {
                let msg = insert_cart(session.machine_mut(), path);
                *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
            }
            if let Some(path) = dropped.into_iter().last() {
                if let Ok(mut list) = shared.disk_swap.lock() {
                    disk_at = match list.iter().position(|p| *p == path) {
                        Some(i) => i,
                        None => {
                            list.push(path.clone());
                            list.len() - 1
                        }
                    };
                }
                disk = Some(path);
            }
            if let Some(path) = disk {
                // Out now, in after SWAP_FRAMES emulated frames.
                let msg = match eject_first(session.machine_mut()) {
                    Some(drive) => format!("{drive} empty, inserting {} …", file_name(&path)),
                    None => "no disk drive".into(),
                };
                *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
                pending_disk = Some((path, frames_run + SWAP_FRAMES));
            }
            if let Some((path, due)) = &pending_disk {
                if frames_run >= *due {
                    let msg = swap_disk(session.machine_mut(), path);
                    *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
                    pending_disk = None;
                }
            }
            if shared.next_screen.swap(false, Ordering::Relaxed) {
                let current = shown_screen(session.machine(), chosen);
                chosen = Some(next_screen(session.machine(), current));
                shown_at = None;
            }
            let clicks = shared
                .panel_events
                .lock()
                .map(|mut e| std::mem::take(&mut *e))
                .unwrap_or_default();
            for (id, value) in clicks {
                let at = session.machine().now();
                session.input(TimedInput {
                    at,
                    target: InputTarget::Panel,
                    event: InputEvent::Control {
                        id: ControlId(id),
                        value,
                    },
                });
            }
            if shared.panel_on.load(Ordering::Relaxed) {
                let controls = session.machine().controls();
                if let Ok(mut c) = shared.controls.lock() {
                    *c = controls;
                }
            }
            let wanted = JoyKeys::ALL
                .get(usize::from(shared.joy_keys.load(Ordering::Relaxed)))
                .copied()
                .unwrap_or(JoyKeys::Off);
            let port = shared.joy_port.load(Ordering::Relaxed);
            if mapper.joy_port().is_some_and(|p| p != port) {
                let now = session.machine().now();
                for e in mapper.set_joy_port(port, now) {
                    tracker.observe(&e);
                    session.input(e);
                }
            }
            if wanted != mapper.joy_keys() {
                let now = session.machine().now();
                for e in mapper.set_joy_keys(wanted, now) {
                    tracker.observe(&e);
                    session.input(e);
                }
            }
            let screen = shown_screen(session.machine(), chosen);
            if shared.snapshot.swap(false, Ordering::Relaxed) {
                let msg = save_snapshot(&shared, session.machine(), screen);
                if let Ok(mut s) = shared.snapshot_done.lock() {
                    *s = Some((msg, Instant::now()));
                }
            }
            if shared.footer_on.load(Ordering::Relaxed) {
                let mut f = tracker.snapshot(session.machine_mut(), frame_time.as_secs_f64() * 1e3);
                // The snapshot's file name stays in the footer for 4 s.
                if let Ok(s) = shared.snapshot_done.lock() {
                    f.note = s
                        .as_ref()
                        .filter(|(_, at)| at.elapsed() < Duration::from_secs(4))
                        .map(|(m, _)| m.clone());
                }
                if let Ok(mut s) = shared.footer.lock() {
                    *s = f;
                }
            }

            let was_running = session.is_running();
            if shared.pause_toggle.swap(false, Ordering::Relaxed) {
                let cmd = if was_running {
                    Command::Pause
                } else {
                    Command::Resume
                };
                let _ = session.command(cmd);
                match &endpoint {
                    Some(ep) => ep.publish(&mut session),
                    None => drop(session.take_events()),
                }
            }
            if let Some(ep) = &endpoint {
                let wait = (!was_running).then_some(Duration::from_millis(10));
                if ep.service(&mut session, wait) == Service::Shutdown {
                    endpoint = None;
                }
            }
            shared
                .paused
                .store(!session.is_running(), Ordering::Relaxed);
            if !session.is_running() {
                // Held by the debugger or stopped by a jam: show what the
                // machine looks like now, once per change.
                let now = session.machine().now();
                if shown_at != Some(now) {
                    shown_at = Some(now);
                    push_frame(&shared, session.machine(), screen);
                    let pc = main_pc(session.machine_mut());
                    let reason = session.last_stop().unwrap_or(StopReason::StepDone);
                    if let Ok(mut s) = shared.status.lock() {
                        *s = Some(stop_status(reason, pc));
                    }
                }
                if endpoint.is_none() {
                    thread::sleep(Duration::from_millis(10));
                }
                continue;
            }
            if !was_running {
                // Resumed: start pacing afresh.
                deadline = Instant::now();
                shown_at = None;
                if let Ok(mut s) = shared.status.lock() {
                    *s = None;
                }
            }

            let percent = crate::footer::speed_percent(shared.speed.load(Ordering::Relaxed));
            wait_until(deadline);
            let period = shared.display_period_ns.load(Ordering::Relaxed);
            if percent != 100 {
                deadline += frame_time.mul_f64(100.0 / f64::from(percent));
            } else if period == 0 {
                deadline += frame_time;
            } else {
                deadline += Duration::from_nanos(period);
                deadline = phase_lock(deadline, period, &shared);
            }
            // Fell far behind (slow host, debugger): do not catch up in a burst.
            if Instant::now() > deadline + frame_time * 5 {
                deadline = Instant::now();
            }

            session.run_slice();
            frames_run += 1;
            match &endpoint {
                Some(ep) => ep.publish(&mut session),
                None => drop(session.take_events()),
            }
            samples.clear();
            let format = session.machine_mut().audio(&mut samples);
            if let Some(tap) = &preview {
                tap.publish_audio(&samples, format);
            }
            if percent != 100 {
                // Sound at another speed would only crackle: muted.
                if let Ok(mut a) = shared.audio.lock() {
                    a.clear();
                }
            } else if let Ok(mut a) = shared.audio.lock() {
                min_fill = min_fill.min(a.len());
                if format.channels == 2 {
                    a.extend(samples.as_chunks::<2>().0.iter().copied());
                } else {
                    a.extend(samples.iter().map(|&s| [s, s]));
                }
                let excess = a.len().saturating_sub(AUDIO_MAX);
                a.drain(..excess);
                max_fill = max_fill.max(a.len());
            }
            frames += 1;
            if debug_stats && report.elapsed() >= Duration::from_secs(5) {
                eprintln!(
                    "5 s: emulated {frames}, window refreshes {}, presented {}, dropped {}, audio buffer {min_fill}..{max_fill} frames, underruns {}, rate {:+} ppm",
                    shared.redraws.swap(0, Ordering::Relaxed),
                    shared.presented.swap(0, Ordering::Relaxed),
                    shared.dropped.swap(0, Ordering::Relaxed),
                    shared.underruns.swap(0, Ordering::Relaxed),
                    shared.rate_ppm.load(Ordering::Relaxed),
                );
                report = Instant::now();
                (min_fill, max_fill, frames) = (usize::MAX, 0, 0);
            }
            push_frame(&shared, session.machine(), screen);
        }
    })
}

/// Save the shown screen (the raw picture, as the debugger and preview
/// see it) as `<name>-NNN.png` with the first unused number in the
/// snapshot folder (`settings.snapshots` of the profile file, else the
/// working folder); returns the footer's message.
fn save_snapshot(shared: &Shared, machine: &dyn Machine, screen: ScreenId) -> String {
    let frame = machine
        .screen(screen)
        .unwrap_or_else(|| machine.framebuffer());
    let png = emu_preview::encode_png(&frame);
    let prefix = shared
        .snapshot_name
        .lock()
        .map(|n| n.clone())
        .unwrap_or_else(|_| "mimic".into());
    let dir = shared
        .snapshot_dir
        .lock()
        .ok()
        .and_then(|d| d.clone())
        .unwrap_or_default();
    if !dir.as_os_str().is_empty() {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return format!("snapshot failed: {}: {e}", dir.display());
        }
    }
    let Some(path) = (1..10_000)
        .map(|n| dir.join(format!("{prefix}-{n:03}.png")))
        .find(|p| !p.exists())
    else {
        return "snapshot: no free file name".into();
    };
    match std::fs::write(&path, png) {
        Ok(()) => format!("saved {}", path.display()),
        Err(e) => format!("snapshot failed: {e}"),
    }
}

fn push_frame(shared: &Shared, machine: &dyn Machine, screen: ScreenId) {
    let frame = match shared.fields.lock() {
        Ok(mut fields) => to_frame(machine, screen, &mut fields),
        Err(_) => return,
    };
    if let Ok(mut q) = shared.frames.lock() {
        q.push_back(frame);
        while q.len() > MAX_QUEUED_FRAMES {
            q.pop_front();
            shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
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
        let handle = spawn(machine, mapper, rx, shared.clone(), None, None);
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

    #[test]
    fn the_pause_button_stops_and_resumes_the_machine() {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("c64", &config).unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("c64"));
        let (_tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        let handle = spawn(machine, mapper, rx, shared.clone(), None, None);
        let status = || shared.status.lock().unwrap().clone();
        assert!(eventually(|| !shared.frames.lock().unwrap().is_empty()));
        assert!(!shared.paused.load(Ordering::Relaxed));

        shared.pause_toggle.store(true, Ordering::Relaxed);
        assert!(
            eventually(|| status().is_some_and(|s| s.starts_with("paused at $"))),
            "{:?}",
            status()
        );
        assert!(eventually(|| shared.paused.load(Ordering::Relaxed)));
        // Stopped: no new frames after the one showing the stop.
        thread::sleep(Duration::from_millis(100));
        shared.frames.lock().unwrap().clear();
        thread::sleep(Duration::from_millis(100));
        assert!(
            shared.frames.lock().unwrap().is_empty(),
            "frames while paused"
        );

        shared.pause_toggle.store(true, Ordering::Relaxed);
        assert!(eventually(|| status().is_none()), "{:?}", status());
        assert!(eventually(|| !shared.paused.load(Ordering::Relaxed)));
        assert!(eventually(|| !shared.frames.lock().unwrap().is_empty()));

        shared.quit.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[test]
    fn another_speed_mutes_the_sound_and_still_runs() {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("c64", &config).unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("c64"));
        let (_tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        // Top speed (800 %).
        shared.speed.store(7, Ordering::Relaxed);
        let handle = spawn(machine, mapper, rx, shared.clone(), None, None);
        assert!(eventually(|| shared.frames.lock().unwrap().len() >= 2));
        assert!(shared.audio.lock().unwrap().is_empty(), "muted");
        shared
            .speed
            .store(crate::footer::NORMAL_SPEED as u8, Ordering::Relaxed);
        assert!(eventually(|| !shared.audio.lock().unwrap().is_empty()));
        shared.quit.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[test]
    fn ctrl_d_swaps_the_next_disk_into_the_first_drive() {
        let dir = std::env::temp_dir().join(format!("mimic-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.adf"), dir.join("b.adf"));
        for p in [&a, &b] {
            std::fs::write(p, vec![0u8; 901_120]).unwrap();
        }
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("amiga", &config).unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("amiga"));
        let (_tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        // Top speed (800 %): the swap waits 100 emulated frames.
        shared.speed.store(7, Ordering::Relaxed);
        let status = || shared.status.lock().unwrap().clone();
        // One disk only: the key says why nothing happens.
        *shared.disk_swap.lock().unwrap() = vec![a.to_string_lossy().into_owned()];
        let handle = spawn(machine, mapper, rx, shared.clone(), None, None);
        shared.next_disk.store(true, Ordering::Relaxed);
        assert!(
            eventually(|| status().is_some_and(|s| s.contains("no other disk"))),
            "{:?}",
            status()
        );
        // Two disks: the drive is empty for a while, then the second goes
        // in, then the first again.
        shared
            .disk_swap
            .lock()
            .unwrap()
            .push(b.to_string_lossy().into_owned());
        shared.next_disk.store(true, Ordering::Relaxed);
        assert!(
            eventually(|| status().is_some_and(|s| s.contains("empty, inserting b.adf"))),
            "{:?}",
            status()
        );
        assert!(
            eventually_for(60, || status().is_some_and(|s| s.ends_with("b.adf"))),
            "{:?}",
            status()
        );
        shared.next_disk.store(true, Ordering::Relaxed);
        assert!(
            eventually_for(60, || status().is_some_and(|s| s.ends_with("a.adf"))),
            "{:?}",
            status()
        );
        shared.quit.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Poll `f` for up to `secs` seconds.
    fn eventually_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..secs * 100 {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Poll `f` for up to 5 s.
    fn eventually(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..500 {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn debugger_pauses_the_thread_and_the_title_shows_it() {
        use std::io::{BufRead, BufReader, Write};

        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("c64", &config).unwrap();
        let (server, endpoint) = emu_debug::DebugServer::start_attached(
            emu_debug::ServerConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                vice: None,
                run: true,
                realtime: true,
            },
            "c64",
        )
        .unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("c64"));
        let (_tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        let handle = spawn(
            machine,
            mapper,
            rx,
            shared.clone(),
            Some((endpoint, "c64".into())),
            None,
        );

        let stream = std::net::TcpStream::connect(server.addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("\"hello\""), "{line}");
        let mut call = |req: &str| {
            writer.write_all(req.as_bytes()).unwrap();
            writer.write_all(b"\n").unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.contains("\"id\"") {
                    assert!(line.contains("\"ok\":true"), "{line}");
                    return line;
                }
            }
        };

        assert!(eventually(|| !shared.frames.lock().unwrap().is_empty()));
        call(r#"{"id":1,"cmd":"pause"}"#);
        let status = || shared.status.lock().unwrap().clone();
        assert!(
            eventually(|| status().is_some_and(|s| s.starts_with("paused at $"))),
            "{:?}",
            status()
        );
        // Stepping while paused updates the window.
        shared.frames.lock().unwrap().clear();
        call(r#"{"id":2,"cmd":"step_line"}"#);
        assert!(eventually(|| !shared.frames.lock().unwrap().is_empty()));
        call(r#"{"id":3,"cmd":"resume"}"#);
        assert!(eventually(|| status().is_none()), "{:?}", status());

        shared.quit.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        server.shutdown();
    }
}
