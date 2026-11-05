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
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use emu_core::frame::{FrameRef, SignalInfo};
use emu_core::input::{InputEvent, InputTarget, TimedInput};
use emu_core::{ControlId, ControlInfo, Machine, ScreenId, StopReason};
use emu_debug::protocol::Command;
use emu_debug::{Endpoint, Service, Session};
use emu_input::{HostEvent, JoyKeys, Mapper};
use emu_preview::server::FrameTap;
use video_crt::field::FieldStage;

use std::collections::BTreeMap;

use crate::footer::{Footer, Tracker};

/// A frame converted to RGBA for upload.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The machine's luma and chroma, for colours RGB cannot hold.
    pub luma_chroma: Option<Vec<emu_core::frame::LumaChroma>>,
    pub signal: SignalInfo,
}

/// State shared between the threads.
pub struct Shared {
    /// Time base for the timestamps below.
    pub epoch: Instant,
    /// When the window last presented (ns since `epoch`): the display's
    /// vertical blank, for phase-locking the emulation to it.
    pub last_vsync_ns: AtomicU64,
    /// Finished frames in order; the render thread shows the newest.
    pub frames: Mutex<VecDeque<Frame>>,
    /// Signalled with a new frame or a command for the render thread.
    pub frame_ready: Condvar,
    /// Interlace weaving for [`Shared::frames`].
    pub fields: Mutex<FieldStage>,
    /// Stereo frames (left, right) at 48 kHz; mono machines fill both.
    pub audio: Mutex<VecDeque<[i16; 2]>>,
    /// Set by the audio thread when a device is running.
    pub audio_live: AtomicBool,
    pub quit: AtomicBool,
    /// When the display refresh is within 1 % of the machine's frame rate,
    /// the window stores the display's nominal refresh period here (ns) and
    /// the render thread its measured one. Emulation then runs at that rate
    /// on the wall clock and nudges its phase so a frame is ready shortly
    /// before the next vertical blank. 0 = the machine's own rate. Emulation
    /// never waits for the window itself.
    pub nominal_period_ns: AtomicU64,
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
    /// The machine has a drive, datasette or cartridge slot: the side
    /// panel's handle shows.
    pub has_media: AtomicBool,
    /// The machine's registry id (a tape PRG is recorded in its format).
    pub machine_id: Mutex<String>,
    /// Clicks in the side panel, for the emulation thread.
    pub media_commands: Mutex<Vec<MediaCommand>>,
    /// The image file in each media slot, by slot id.
    pub media_names: Mutex<BTreeMap<u8, String>>,
    /// The side panel's file dialog is open.
    pub dialog_open: AtomicBool,
    /// Where the last file dialog ended.
    pub last_folder: Mutex<Option<std::path::PathBuf>>,
    /// Set by the window (Ctrl+D): put the next disk of
    /// [`Shared::disk_swap`] into the first drive.
    pub next_disk: AtomicBool,
    /// The gamepad fire button (position in `panel::PAD_FIRE_BUTTONS`), set
    /// by the panel's PAD FIRE switch.
    pub pad_fire: AtomicU8,
    /// Set by the panel's SAVE MAPPING button.
    pub save_mapping: AtomicBool,
    /// The machine's freezes (cheats) for the panel, published while it is
    /// shown; clicks on them (freeze id, on).
    pub cheats: Mutex<Vec<CheatRow>>,
    pub cheat_clicks: Mutex<Vec<(u32, bool)>>,
    /// Taken by the emulation thread at start: the cheat database folder,
    /// cheat files to apply and the images inserted at start (their
    /// database entries apply too).
    pub cheats_dir: Mutex<Option<std::path::PathBuf>>,
    /// The machine's host-service journal, handed to the debug session.
    pub host_journal: Mutex<Option<emu_core::host::Journal>>,
    pub startup_cheats: Mutex<Vec<emu_cheats::CheatFile>>,
    pub startup_images: Mutex<Vec<String>>,
    /// Held by the window (Ctrl+R): run backwards through the rewind ring.
    pub rewinding: AtomicBool,
    /// Memory budget of the rewind ring in MB (0 = off), read at start.
    pub rewind_mb: AtomicU32,
    /// Image files dropped onto the window, for the first drive.
    pub dropped_disks: Mutex<Vec<String>>,
    /// Set by the window (Ctrl+Space): the status footer is shown, so the
    /// emulation publishes [`Shared::footer`] after every frame.
    pub footer_on: AtomicBool,
    pub footer: Mutex<Footer>,
    /// Set by the window (Shift+Alt+←): the console panel is open, so the
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
    /// `--warp-load`: at 100 % the machine runs as fast as it can while a
    /// disk drive works (LED or motor), muted, like VICE's autostart warp.
    pub warp_load: AtomicBool,
    /// Published by the emulation thread: running at warp now.
    pub warping: AtomicBool,
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
    /// Wake the render thread (a new frame or a command).
    pub fn wake_render(&self) {
        self.frame_ready.notify_all();
    }

    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_vsync_ns: AtomicU64::new(0),
            frames: Mutex::default(),
            frame_ready: Condvar::new(),
            fields: Mutex::default(),
            audio: Mutex::default(),
            audio_live: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            nominal_period_ns: AtomicU64::new(0),
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
            has_media: AtomicBool::new(false),
            machine_id: Mutex::default(),
            media_commands: Mutex::default(),
            media_names: Mutex::default(),
            dialog_open: AtomicBool::new(false),
            last_folder: Mutex::default(),
            rewinding: AtomicBool::new(false),
            pad_fire: AtomicU8::new(0),
            save_mapping: AtomicBool::new(false),
            cheats: Mutex::default(),
            cheat_clicks: Mutex::default(),
            cheats_dir: Mutex::default(),
            host_journal: Mutex::default(),
            startup_cheats: Mutex::default(),
            startup_images: Mutex::default(),
            rewind_mb: AtomicU32::new(emu_debug::session::DEFAULT_BUDGET_MB),
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
            warp_load: AtomicBool::new(true),
            warping: AtomicBool::new(false),
            snapshot: AtomicBool::new(false),
            snapshot_name: Mutex::new(String::from("mimic")),
            snapshot_dir: Mutex::new(None),
            snapshot_done: Mutex::new(None),
        }
    }
}

/// Audio kept buffered ahead of the device (48 kHz frames, 100 ms).
pub const AUDIO_TARGET: usize = 4_800;
/// Frames waiting for the render thread before old ones are dropped.
const MAX_QUEUED_FRAMES: usize = 4;

/// Hand the host input waiting in `input` to the machine, timed now;
/// false when the window has gone.
fn drain_input(
    input: &Receiver<HostEvent>,
    mapper: &mut Mapper,
    tracker: &mut Tracker,
    session: &mut Session,
) -> bool {
    loop {
        match input.try_recv() {
            Ok(ev) => {
                let now = session.machine().now();
                for e in mapper.handle(&ev, now) {
                    tracker.observe(&e);
                    session.input(e);
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

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
/// Shift `deadline` a little towards `target` ns after a vertical blank
/// (the frame must be emulated and queued before the next blank; the later
/// it starts, the less input waits for it). Small steps (1/8 of the error
/// per frame) keep the frame rate itself steady.
fn phase_lock(deadline: Instant, period: u64, target: u64, shared: &Shared) -> Instant {
    let vsync = shared.last_vsync_ns.load(Ordering::Relaxed);
    if vsync == 0 || deadline < shared.epoch {
        return deadline;
    }
    let at = (deadline - shared.epoch).as_nanos() as i128;
    let p = i128::from(period);
    let phase = (at - i128::from(vsync)).rem_euclid(p);
    let error = phase - i128::from(target); // -target … p - target
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
        Ok(()) => String::new(),
        Err(e) => format!("{name}: {e}"),
    }
}

/// Show what a media command has to say in the title bar: only trouble, the
/// panel and footer name what is mounted. Success clears an old message,
/// but not the debugger's stop.
fn report_media(shared: &Shared, msg: String) {
    if msg.is_empty() && shared.paused.load(Ordering::Relaxed) {
        return;
    }
    *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = (!msg.is_empty()).then_some(msg);
}

fn join_messages(msg: String, more: String) -> String {
    if msg.is_empty() {
        more
    } else {
        format!("{msg}; {more}")
    }
}

/// What the side panel asks of a media slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    Mount {
        slot: u8,
        kind: emu_core::media::MediaKind,
        path: String,
    },
    Eject {
        slot: u8,
    },
    /// Write the slot's changes through to its image file.
    Save {
        slot: u8,
    },
}

/// An image opened the way a slot of `kind` takes it.
fn open_media(
    kind: emu_core::media::MediaKind,
    path: &str,
    machine_id: &str,
) -> Result<emu_core::media::Media, String> {
    use emu_core::media::MediaKind;
    let p = std::path::Path::new(path);
    let media = match kind {
        MediaKind::Tape => media_backends::image::open_tape(p, machine_id),
        MediaKind::Cartridge => media_backends::image::open_cart(p),
        _ => media_backends::image::open(p),
    }
    .map_err(|e| e.to_string())?;
    if media.kind != kind {
        return Err(format!("not a {kind:?} image").to_lowercase());
    }
    Ok(media)
}

fn slot_info(m: &dyn Machine, slot: u8) -> Option<emu_core::media::MediaSlotInfo> {
    m.media_slots().into_iter().find(|s| s.id.0 == slot)
}

/// Remember or forget the image in the first slot of `kind`.
fn note_first(
    shared: &Shared,
    m: &dyn Machine,
    kind: emu_core::media::MediaKind,
    path: Option<&str>,
) {
    let Some(info) = m.media_slots().into_iter().find(|s| s.kind == kind) else {
        return;
    };
    if let Ok(mut names) = shared.media_names.lock() {
        match path {
            Some(p) => {
                names.insert(info.id.0, file_name(p));
            }
            None => {
                names.remove(&info.id.0);
            }
        }
    }
}

/// An image on its way into a full slot: it goes in once the slot has been
/// empty for a moment ([`SWAP_FRAMES`]).
struct PendingMount {
    slot: u8,
    media: emu_core::media::Media,
    path: String,
    due: u64,
}

/// Put `media` into its slot; returns the message for the title bar.
fn finish_mount(
    shared: &Shared,
    m: &mut dyn Machine,
    slot: u8,
    media: emu_core::media::Media,
    path: &str,
) -> String {
    let name = file_name(path);
    let Some(info) = slot_info(m, slot) else {
        return format!("{name}: no such slot");
    };
    let Some(target) = m.media_slot(info.id) else {
        return format!("{name}: no such slot");
    };
    match target.insert(media) {
        Ok(()) => {
            if let Ok(mut names) = shared.media_names.lock() {
                names.insert(slot, name.clone());
            }
            String::new()
        }
        Err(e) => format!("{name}: {e}"),
    }
}

/// Carry out a side panel command; a mount into a full slot comes back
/// pending. Returns the message for the title bar.
fn media_command(
    shared: &Shared,
    m: &mut dyn Machine,
    command: MediaCommand,
    frame: u64,
) -> (String, Option<PendingMount>) {
    match command {
        MediaCommand::Mount { slot, kind, path } => {
            let machine_id = shared
                .machine_id
                .lock()
                .map(|s| s.clone())
                .unwrap_or_default();
            let name = file_name(&path);
            // Open first: a file that is no image leaves the old one in.
            let media = match open_media(kind, &path, &machine_id) {
                Ok(media) => media,
                Err(e) => return (format!("{name}: {e}"), None),
            };
            let Some(info) = slot_info(m, slot) else {
                return (format!("{name}: no such slot"), None);
            };
            if !info.inserted {
                return (finish_mount(shared, m, slot, media, &path), None);
            }
            if let Some(target) = m.media_slot(info.id) {
                let _ = target.eject();
            }
            if let Ok(mut names) = shared.media_names.lock() {
                names.remove(&slot);
            }
            let pending = PendingMount {
                slot,
                media,
                path,
                due: frame + SWAP_FRAMES,
            };
            (String::new(), Some(pending))
        }
        MediaCommand::Eject { slot } => {
            let Some(info) = slot_info(m, slot) else {
                return ("no such slot".into(), None);
            };
            if let Some(target) = m.media_slot(info.id) {
                let _ = target.eject();
            }
            if let Ok(mut names) = shared.media_names.lock() {
                names.remove(&slot);
            }
            (String::new(), None)
        }
        MediaCommand::Save { slot } => {
            let Some(info) = slot_info(m, slot) else {
                return ("no such slot".into(), None);
            };
            let done = m.media_slot(info.id).map(|target| target.flush());
            let msg = match done {
                Some(Ok(())) => format!("{}: saved to its file", info.name),
                Some(Err(e)) => format!("{}: not saved: {e}", info.name),
                None => "no such slot".into(),
            };
            (msg, None)
        }
    }
}

/// One freeze as the panel shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatRow {
    pub id: u32,
    pub label: String,
    pub enabled: bool,
}

/// The session's freezes for the panel.
fn cheat_rows(session: &mut Session) -> Vec<CheatRow> {
    let Ok(list) = session.command(Command::FreezeList) else {
        return Vec::new();
    };
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    let addr = f["addr"].as_u64().unwrap_or(0);
                    let label = match f["label"].as_str() {
                        Some(l) => format!("{} ${addr:04X}", l.to_uppercase()),
                        None => format!("CHEAT ${addr:04X}"),
                    };
                    CheatRow {
                        id: f["freeze"].as_u64().unwrap_or(0) as u32,
                        label,
                        enabled: f["enabled"].as_bool().unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Apply the cheat database's entry for an image just inserted; a status
/// line if there was one.
fn cheats_for_path(session: &mut Session, path: &str) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    match session.cheats_for_image(&data) {
        Ok(Some(game)) => Some(format!("cheats for {game}")),
        Ok(None) => None,
        Err(e) => Some(format!("cheats: {e}")),
    }
}

/// Keep the current joystick mapping for the machine:
/// `<user folder>/input/<machine>.toml`.
fn save_mapping(mapper: &Mapper, machine: &str) -> String {
    let Some(dir) = emu_profile::user_dir() else {
        return "no user folder for the mapping".into();
    };
    let path = dir.join("input").join(format!("{machine}.toml"));
    let result = std::fs::create_dir_all(dir.join("input"))
        .and_then(|()| std::fs::write(&path, mapper.profile().to_toml()));
    match result {
        Ok(()) => format!("mapping saved to {}", path.display()),
        Err(e) => format!("{}: {e}", path.display()),
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
        Ok(()) => String::new(),
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
            luma_chroma: None,
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
        luma_chroma: f.luma_chroma.map(<[_]>::to_vec),
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
            let idle_tap = tap.clone();
            session.set_overlay_wants(tap.overlay_wants());
            session.set_frame_hook(Box::new(move |m, running| tap.frame(m, running)));
            session.set_idle_hook(Box::new(move |m| idle_tap.idle(m)));
        }
        // Cheats: the database, files from --cheat, the start images' entries.
        if let Ok(mut d) = shared.cheats_dir.lock() {
            session.set_cheats_dir(d.take());
        }
        if let Some(j) = shared.host_journal.lock().ok().and_then(|mut j| j.take()) {
            session.set_host_journal(j);
        }
        let files = shared
            .startup_cheats
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default();
        for f in &files {
            if let Err(e) = session.apply_cheat_file(f) {
                eprintln!("cheats: {e}");
            }
        }
        let images = shared
            .startup_images
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default();
        for path in &images {
            if let Some(msg) = cheats_for_path(&mut session, path) {
                eprintln!("{msg}");
            }
        }
        let machine_name = shared
            .snapshot_name
            .lock()
            .map(|n| n.clone())
            .unwrap_or_default();
        let rewind_mb = shared.rewind_mb.load(Ordering::Relaxed);
        if rewind_mb > 0 {
            session.rewind_config(Some(1), Some(rewind_mb));
        }
        // Ctrl+R held: the machine was running before and resumes after.
        let mut rewind_resume = false;
        let mut samples = Vec::new();
        let frame_time = session.frame_duration();
        let mut deadline = Instant::now();
        // Wall time a frame takes to emulate and queue (a slowly decaying
        // maximum, ns): the phase lock starts the frame as late as that
        // allows before the next vertical blank.
        let mut slice_ns = 0.0f64;
        let mut shown_at = None;
        let debug_stats = std::env::var_os("EMU_DESKTOP_DEBUG").is_some();
        let mut report = Instant::now();
        let mut chosen: Option<ScreenId> = None;
        // Index of the disk in the first drive within `Shared::disk_swap`.
        let mut disk_at = 0usize;
        // A disk on its way into the first drive and the frame it goes in;
        // frames run so far (the swap pause counts emulated frames).
        let mut pending_disk: Option<(String, u64)> = None;
        let mut pending_mount: Option<PendingMount> = None;
        let mut frames_run = 0u64;
        // Frames left at warp after a disk drive was last seen working.
        let mut warp_hold = 0u32;
        // Frames since the machine last made a sound.
        let mut quiet_frames = 0u32;
        // The picture's fingerprint and, one bit a frame, which of the
        // last 64 frames changed it.
        let (mut picture_hash, mut picture_moves) = (0u64, 0u64);
        // Set once the loaded program shows that it runs (sound, or a
        // moving picture) outside the warp: from then on the warp stays
        // off until a disk goes in, like VICE's autostart warp. A demo
        // that loads its next part in the background (with its picture
        // standing or its music pausing) keeps its own pace.
        let mut program_running = false;
        let mut drive_seen = false;
        let mut tracker = Tracker::default();
        let (mut min_fill, mut max_fill, mut frames) = (usize::MAX, 0, 0u32);
        while !shared.quit.load(Ordering::Relaxed) {
            if !drain_input(&input, &mut mapper, &mut tracker, &mut session) {
                return;
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
            let commands = shared
                .media_commands
                .lock()
                .map(|mut c| std::mem::take(&mut *c))
                .unwrap_or_default();
            for command in commands {
                // A second mount replaces one still waiting for its slot.
                let (msg, pending) =
                    media_command(&shared, session.machine_mut(), command, frames_run);
                if pending.is_some() {
                    pending_mount = pending;
                }
                report_media(&shared, msg);
            }
            if pending_mount.as_ref().is_some_and(|p| frames_run >= p.due) {
                if let Some(p) = pending_mount.take() {
                    let mut msg =
                        finish_mount(&shared, session.machine_mut(), p.slot, p.media, &p.path);
                    if let Some(c) = cheats_for_path(&mut session, &p.path) {
                        msg = join_messages(msg, c);
                    }
                    report_media(&shared, msg);
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
                let mut msg = insert_cart(session.machine_mut(), path);
                note_first(
                    &shared,
                    session.machine(),
                    emu_core::media::MediaKind::Cartridge,
                    Some(path),
                );
                if let Some(c) = cheats_for_path(&mut session, path) {
                    msg = join_messages(msg, c);
                }
                report_media(&shared, msg);
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
                    Some(_) => String::new(),
                    None => "no disk drive".into(),
                };
                note_first(
                    &shared,
                    session.machine(),
                    emu_core::media::MediaKind::Disk,
                    None,
                );
                report_media(&shared, msg);
                pending_disk = Some((path, frames_run + SWAP_FRAMES));
            }
            if let Some((path, due)) = &pending_disk {
                if frames_run >= *due {
                    let mut msg = swap_disk(session.machine_mut(), path);
                    note_first(
                        &shared,
                        session.machine(),
                        emu_core::media::MediaKind::Disk,
                        Some(path),
                    );
                    if let Some(c) = cheats_for_path(&mut session, path) {
                        msg = join_messages(msg, c);
                    }
                    report_media(&shared, msg);
                    pending_disk = None;
                    program_running = false;
                    drive_seen = false;
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
            let toggles = shared
                .cheat_clicks
                .lock()
                .map(|mut c| std::mem::take(&mut *c))
                .unwrap_or_default();
            for (freeze, enabled) in toggles {
                let _ = session.command(Command::FreezeToggle {
                    freeze,
                    enabled: Some(enabled),
                });
            }
            if shared.panel_on.load(Ordering::Relaxed) {
                let mut controls = session.machine().controls();
                controls.retain(|c| !c.on_picture);
                if let Ok(mut c) = shared.controls.lock() {
                    *c = controls;
                }
                let rows = cheat_rows(&mut session);
                if let Ok(mut c) = shared.cheats.lock() {
                    *c = rows;
                }
            }
            let fire = crate::panel::PAD_FIRE_BUTTONS
                .get(usize::from(shared.pad_fire.load(Ordering::Relaxed)))
                .copied();
            if let (Some(fire), Some(now_fire)) = (fire, mapper.pad_fire()) {
                if fire != now_fire {
                    let now = session.machine().now();
                    for e in mapper.set_pad_fire(fire, now) {
                        session.input(e);
                    }
                }
            }
            if shared.save_mapping.swap(false, Ordering::Relaxed) {
                let msg = save_mapping(&mapper, &machine_name);
                *shared.status.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
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
            // The side panel's handle shows whether or not the footer does.
            {
                let mut f = tracker.snapshot(session.machine_mut(), frame_time.as_secs_f64() * 1e3);
                // The snapshot's file name stays in the footer for 4 s.
                if let Ok(s) = shared.snapshot_done.lock() {
                    f.note = s
                        .as_ref()
                        .filter(|(_, at)| at.elapsed() < Duration::from_secs(4))
                        .map(|(m, _)| m.clone());
                }
                if f.note.is_none() && shared.warping.load(Ordering::Relaxed) {
                    f.note = Some("warp: loading".into());
                }
                if let Ok(mut s) = shared.footer.lock() {
                    *s = f;
                }
            }

            // Ctrl+R: two states back per frame time (twice real speed),
            // silent; on release the machine runs on from there.
            if shared.rewinding.load(Ordering::Relaxed) && rewind_mb > 0 {
                if !rewind_resume {
                    rewind_resume = session.is_running();
                }
                if session.rewind_back(2).is_ok() {
                    push_frame(&shared, session.machine(), screen);
                }
                match &endpoint {
                    Some(ep) => ep.publish(&mut session),
                    None => drop(session.take_events()),
                }
                if let Ok(mut a) = shared.audio.lock() {
                    a.clear();
                }
                shown_at = Some(session.machine().now());
                thread::sleep(frame_time);
                continue;
            }
            if std::mem::take(&mut rewind_resume) {
                let _ = session.command(Command::Resume);
                match &endpoint {
                    Some(ep) => ep.publish(&mut session),
                    None => drop(session.take_events()),
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
                session.idle();
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
            let warp = warp_hold > 0
                && quiet_frames >= WARP_QUIET
                && still_picture(picture_moves)
                && !program_running
                && percent == 100
                && shared.warp_load.load(Ordering::Relaxed);
            if debug_stats && warp != shared.warping.load(Ordering::Relaxed) {
                eprintln!(
                    "frame {frames_run}: warp {} (quiet {quiet_frames}, moving {})",
                    if warp { "on" } else { "off" },
                    picture_moves.count_ones()
                );
            }
            shared.warping.store(warp, Ordering::Relaxed);
            let period = shared.display_period_ns.load(Ordering::Relaxed);
            if warp {
                // No wait; pacing starts afresh when the drive is done.
                deadline = Instant::now();
            } else if percent != 100 {
                wait_until(deadline);
                deadline += frame_time.mul_f64(100.0 / f64::from(percent));
            } else if period == 0 {
                wait_until(deadline);
                deadline += frame_time;
            } else {
                wait_until(deadline);
                deadline += Duration::from_nanos(period);
                // The render thread takes the newest frame at the blank: the
                // frame must be there by then, with 2 ms to spare.
                let margin = 2_000_000.0;
                let target = (period as f64 - slice_ns * 1.25 - margin)
                    .clamp(period as f64 / 8.0, period as f64 * 0.8);
                deadline = phase_lock(deadline, period, target as u64, &shared);
            }
            // Fell far behind (slow host, debugger): do not catch up in a burst.
            if Instant::now() > deadline + frame_time * 5 {
                deadline = Instant::now();
            }
            // Input that came during the wait goes into this frame, not the
            // next (a frame less from mouse to picture).
            if !drain_input(&input, &mut mapper, &mut tracker, &mut session) {
                return;
            }

            let slice_start = Instant::now();
            session.run_slice();
            frames_run += 1;
            warp_hold = if disk_working(session.machine_mut()) {
                WARP_HOLD
            } else {
                warp_hold.saturating_sub(1)
            };
            match &endpoint {
                Some(ep) => ep.publish(&mut session),
                None => drop(session.take_events()),
            }
            samples.clear();
            let format = session.machine_mut().audio(&mut samples);
            quiet_frames = if audible(&samples) {
                0
            } else {
                quiet_frames.saturating_add(1)
            };
            let hash = picture_fingerprint(&session.machine().framebuffer());
            picture_moves = (picture_moves << 1) | u64::from(hash != picture_hash);
            picture_hash = hash;
            drive_seen |= warp_hold == WARP_HOLD;
            if drive_seen
                && !warp
                && (quiet_frames == 0 || picture_moves.count_ones() >= ANIMATION_FRAMES)
            {
                // Sound or an animation after a load began (typing and a
                // cursor change too few frames): the loaded program runs.
                program_running = true;
            }
            if let Some(tap) = &preview {
                tap.publish_audio(&samples, format);
            }
            if percent != 100 || warp {
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
            let took = slice_start.elapsed().as_nanos() as f64;
            slice_ns = took.max(slice_ns * 0.98);
        }
    })
}

/// Frames the warp lasts after a disk drive was last seen working (the
/// LED goes out between files for a moment).
const WARP_HOLD: u32 = 25;

/// Frames without sound before a working disk drive may start the warp:
/// a demo's music plays on while its trackloader runs, and warp mutes it.
const WARP_QUIET: u32 = 50;

/// Frames of the last 64 that may change the picture while the warp still
/// starts: a blinking cursor or a loader's counter, not an animation (a
/// demo drawing its next effect while it loads, Promised Land's curtain
/// on the Plus/4, must keep its speed, sound or not).
const WARP_MOVING_FRAMES: u32 = 6;

/// Frames of the last 64 that change the picture in an animation, which
/// tells the loaded program is running.
const ANIMATION_FRAMES: u32 = 32;

/// Is the picture (nearly) still over the last 64 frames?
fn still_picture(moves: u64) -> bool {
    moves.count_ones() <= WARP_MOVING_FRAMES
}

/// A cheap fingerprint of a frame's pixels.
fn picture_fingerprint(frame: &FrameRef) -> u64 {
    let bytes: &[u8] = match &frame.pixels {
        emu_core::frame::PixelData::Indexed { pixels, .. } => pixels,
        emu_core::frame::PixelData::Rgb(px) => {
            return px.iter().fold(0xcbf2_9ce4_8422_2325, |h: u64, p| {
                (h ^ u64::from(u32::from_le_bytes([p.r, p.g, p.b, 0])))
                    .wrapping_mul(0x0000_0100_0000_01b3)
            })
        }
    };
    let (words, rest) = bytes.as_chunks::<8>();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        h = (h ^ u64::from_le_bytes(*w))
            .wrapping_mul(0x0000_0100_0000_01b3)
            .rotate_left(29);
    }
    for &b in rest {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Peak-to-peak level from which a frame's samples count as sound; a
/// constant output of any height (a SID's DC level) stays below it.
const AUDIBLE_RANGE: i32 = 64;

/// Does this frame's output carry sound?
fn audible(samples: &[i16]) -> bool {
    let (low, high) = samples
        .iter()
        .fold((i16::MAX, i16::MIN), |(low, high), &s| {
            (low.min(s), high.max(s))
        });
    i32::from(high) - i32::from(low) >= AUDIBLE_RANGE
}

/// Is a disk drive working (activity LED or motor)?
fn disk_working(m: &mut dyn Machine) -> bool {
    m.media_slots()
        .into_iter()
        .filter(|s| s.kind == emu_core::media::MediaKind::Disk && s.inserted)
        .any(|s| m.media_slot(s.id).is_some_and(|d| d.status().active))
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
    shared.wake_render();
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::registry::MachineConfig;

    #[test]
    fn warp_needs_a_still_picture() {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let m = emu_machines::registry().create("c64", &config).unwrap();
        let frame = m.framebuffer();
        assert_eq!(picture_fingerprint(&frame), picture_fingerprint(&frame));
        let other = FrameRef {
            pixels: emu_core::frame::PixelData::Indexed {
                pixels: &[1, 2, 3, 4, 5, 6, 7, 8, 9],
                palette: &[],
            },
            ..frame
        };
        assert_ne!(picture_fingerprint(&frame), picture_fingerprint(&other));
        // A cursor or a counter: a few changed frames still allow the warp,
        // an animation (Promised Land's curtain) does not.
        assert!(still_picture(0));
        assert!(still_picture(0b1000_0000_0000_0001_0000_0001));
        assert!(!still_picture(u64::MAX));
        assert!(!still_picture((1 << (WARP_MOVING_FRAMES + 1)) - 1));
        assert!(u64::MAX.count_ones() >= ANIMATION_FRAMES);
    }

    #[test]
    fn cheats_show_as_panel_rows() {
        let mut config = MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let machine = emu_machines::registry().create("c64", &config).unwrap();
        let mut session = Session::new(machine, "c64");
        assert!(cheat_rows(&mut session).is_empty());
        let file = emu_cheats::import::import("pokes", "POKE 53280,2 : REM border", "cpu").unwrap();
        session.apply_cheat_file(&file).unwrap();
        let rows = cheat_rows(&mut session);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "BORDER $D020");
        assert!(rows[0].enabled);
        assert_eq!(cheats_for_path(&mut session, "missing.d64"), None);
    }

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
    fn only_a_changing_output_counts_as_sound() {
        assert!(!audible(&[]));
        assert!(!audible(&[7000; 960]), "a DC level is silence");
        assert!(!audible(&[7000, 7063, 7000]), "below the floor");
        assert!(audible(&[7000, 7064, 7000]));
    }

    #[test]
    #[ignore = "needs C64 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
    fn a_working_disk_drive_runs_at_warp() {
        let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
        let mut config = MachineConfig {
            roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
            ..MachineConfig::default()
        };
        config.options.insert("drive8".into(), "1541".into());
        config.options.insert("autostart".into(), "on".into());
        let mut machine = emu_machines::registry().create("c64", &config).unwrap();
        // An empty (unformatted) disk: LOAD"*" keeps the drive busy until
        // it gives up.
        let slot = machine
            .media_slots()
            .into_iter()
            .find(|s| s.kind == emu_core::media::MediaKind::Disk)
            .unwrap()
            .id;
        let disk = media_backends::image::from_bytes(
            "empty.d64",
            vec![0; 174_848],
            emu_core::media::MediaKind::Disk,
        );
        machine.media_slot(slot).unwrap().insert(disk).unwrap();
        let mapper = Mapper::new(emu_input::Profile::default_for("c64"));
        let (_tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        let handle = spawn(machine, mapper, rx, shared.clone(), None, None);
        assert!(
            eventually_for(30, || shared.warping.load(Ordering::Relaxed)),
            "warp while the drive works"
        );
        assert!(
            eventually_for(60, || !shared.warping.load(Ordering::Relaxed)),
            "back to 100 % afterwards"
        );
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
        let mounted = |name: &str| {
            shared
                .media_names
                .lock()
                .unwrap()
                .values()
                .any(|n| n == name)
        };
        assert!(eventually(|| !mounted("a.adf")), "the drive empties first");
        assert!(eventually_for(60, || mounted("b.adf")));
        assert_eq!(status(), None, "the title bar names no image");
        shared.next_disk.store(true, Ordering::Relaxed);
        assert!(eventually_for(60, || mounted("a.adf")));
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
