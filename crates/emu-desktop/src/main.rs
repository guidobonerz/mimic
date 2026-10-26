//! Desktop frontend: a window with the machine's picture through the CRT
//! shader, sound, keyboard and gamepads. No menus yet; everything comes
//! from the command line.
//!
//!     mimic-desktop --machine c64 --disk game.d64 --autostart
//!
//! Keys use the positional keymap of `emu-input` (see `--help`); the
//! numeric keypad and the first gamepad act as the joystick in port 2.

#![forbid(unsafe_code)]

mod audio;
mod emu;
mod footer;
mod gfx;
mod host;
mod panel;
mod picker;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use emu_core::media::{MediaKind, SlotId};
use emu_core::port::PortKind;
use emu_core::registry::MachineConfig;
use emu_core::Machine;
use emu_debug::{DebugServer, ServerConfig};
use emu_input::{Axis, Button, HostEvent, JoyKeys, Mapper, Profile as InputProfile};
use emu_preview::server::{FrameTap, InputSink, PreviewConfig, PreviewServer};
use video_crt::field::Deinterlace;
use video_crt::Profile;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Icon, Window, WindowId};

use emu::{Shared, TapeKey};
use gfx::Gfx;

/// The mimic logo (16×14 RGBA), shown as the window icon.
const ICON_PNG: &[u8] = include_bytes!("../assets/mimic-icon.png");

/// Decodes the logo and centres it on a square canvas, since some window
/// managers stretch non-square icons. `None` (no icon) if decoding fails.
/// macOS ignores window icons; there the Dock shows the app bundle's icon.
fn window_icon() -> Option<Icon> {
    let mut decoder = png::Decoder::new(ICON_PNG);
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let side = w.max(h);
    let (dx, dy) = ((side - w) / 2, (side - h) / 2);
    let mut rgba = vec![0; side * side * 4];
    for y in 0..h {
        let src = &buf[y * info.line_size..][..w * 4];
        let dst = ((y + dy) * side + dx) * 4;
        rgba[dst..dst + w * 4].copy_from_slice(src);
    }
    Icon::from_rgba(rgba, side as u32, side as u32).ok()
}

const USAGE: &str = "\
usage: mimic-desktop [options]

  --profile NAME        start profile from profiles.yaml (./ or the user's
                        config folder; --profiles FILE for another file);
                        flags given here override it. Without --machine and
                        --profile the window lists the profiles to choose
  --list-profiles       list the profiles and exit
  --save-profile NAME   store this command line's setup as a profile
  --machine ID          machine id (default: c64)
  --model NAME          model variant (C64: pal, ntsc)
  --prg FILE            program (PRG/T64, Atari XEX) to load and run
  --disk FILE           disk image for drive 8 (Atari: ATR for D1:, Amiga: DF0:,
                        Apple II: DSK/DO/PO/NIB/WOZ for slot 6 drive 1; Mac:
                        400K/800K DSK or DiskCopy 4.2, hard disk images to
                        SCSI); repeat for the next drives (a drive refusing
                        an image passes it on), extra disks are swapped
                        into the first drive with Ctrl+D
  --tape FILE           TAP image for the datasette, CAS for the Atari program
                        recorder, WAV for the Apple II cassette (a PRG is
                        recorded in the machine's tape format)
  --cart FILE           cartridge: CRT image, or a raw VIC-20 (load address
                        + ROM) or C16/Plus/4 (16/32 KB) dump; Atari CAR or
                        raw 8/16 KB ROM; ColecoVision ROM; NES iNES file;
                        SNES .sfc/.smc; Vectrex .vec/.bin
  --autostart           load and run the disk (C64) or tape (Spectrum) after boot
  --rom-dir DIR         ROM folder (default: settings.roms of profiles.yaml,
                        then $EMU_ROM_DIR)
  --rom ROLE=FILE       a single ROM image for a role (repeatable)
  --speed PERCENT       start at 10, 25, 50, 100, 150, 200, 400 or 800 % speed
  --option KEY=VALUE    machine option (repeatable)
  --net on|off          the host network for the C64's WiC64 (--option
                        userport=wic64) and Ultimate command interface
                        (--option uci=on); default on, off: no network
  --uci-dir DIR         the Ultimate command interface's DOS folder (default
                        ./ultimate, created on the first write)
  --crt PROFILE         monitor: raw, lite, 1702, 1084s, 1084s-rgb, tv, pal-tv,
                        ntsc-tv, pet-white, pet-green, ctm644, gt65, mac or a
                        .toml file, re-read when it changes (default: the
                        machine's own monitor)
  --scale N             initial window size in rows per line (default 2;
                        less if the window would not fit the screen)
  --deinterlace MODE    interlaced pictures (Amiga LACE): weave (default, both
                        fields interleaved) or bob (the current field's lines
                        doubled, no combing); Ctrl+I switches at run time
  --debug [ADDR]        serve the debug protocol (default 127.0.0.1:6510)
  --preview [ADDR]      serve the live preview in the browser
                        (default 127.0.0.1:6580)
  --vice [ADDR]         serve the VICE binary monitor protocol for VS64 & co.
                        (default 127.0.0.1:6502); implies --debug
  --rewind-mb N         memory for rewinding with Ctrl+R (default 128; 0 = off)
  --cheats DIR          cheat database: DIR/<machine>/<crc32>.toml applies to
                        the images inserted (default: $EMU_CHEATS_DIR, else
                        ./cheats if it exists)
  --cheat FILE          load a cheat file: .toml, POKE list (.pokes/.bas),
                        VICE monitor commands (.mon), address/value codes
                        (.txt) (repeatable)
  --input FILE          joystick mapping profile (TOML); default: the one
                        saved with SAVE MAPPING (<user folder>/input/<machine>.toml)
  --footer              start with the status footer shown (Ctrl+Space)
  --no-panel            start with the console panel hidden (Ctrl+P)
  --joykeys KEYS        keys that play the joystick: cursor (cursor keys,
                        Space fires; default on the Atari 2600), numpad
                        (8/2/4/6, 0 or right Ctrl fire; default elsewhere)
                        or off; also a switch on the console panel
  --joyport N           the port the joysticks (keys and gamepad) play,
                        1-based (default: the machine's game port); also a
                        switch on the panel when there are several ports
  --help

C64 keys by position on a PC keyboard: Esc = RUN/STOP, Tab = CTRL,
left Ctrl = C=, Insert = £, Delete = ↑, Page Up = RESTORE, Home = CLR/HOME,
F2/F4/F6/F8 = shifted F1/F3/F5/F7. Joystick (port 2): numeric keypad
8/2/4/6 + 0 or right Ctrl (or cursor keys + Space, see --joykeys), or the
first gamepad.

Atari 800XL keys by position: < > on - =, - = on [ ], + * on ' \\, Tab, Esc,
CapsLock, right Alt = Atari key, arrows = CONTROL + - = + *; F2 START,
F3 SELECT, F4 OPTION, F5 RESET, F6 HELP, F7/Pause BREAK.

Atari 2600 console: F1 SELECT, F2 RESET, F3 colour/black-and-white,
F4/F5 left/right difficulty A/B. Keypads (--option port1=keypad, port2):
left 1 2 3 / Q W E / A S D / Z X C, right 8 9 0 / I O P / K L ; / , . /.

F12 switches between a machine's screens (C128: 40-column VIC and
80-column VDC; the window starts on the one the 40/80 key selects).
Datasette: F9 PLAY, Shift+F9 RECORD, F10 STOP, F11 REWIND.
Ctrl+R (held) runs the machine backwards at twice real speed through the
last frames kept in memory (--rewind-mb); on release it runs on from there.
Disks: Ctrl+D puts the next disk of the --disk list into the first drive;
dropping an image file onto the window inserts it there.
Ctrl+P shows or hides the console panel: the machine's buttons and
switches (power, reset, Atari console switches, RESTORE …); click a button
(held while the mouse button is down) or a switch position. Its PAUSE
button stops the whole machine and lets it run again (lit while stopped,
also by a debugger; the title bar shows where). Its JOY KEYS
switch picks the keys that play the joystick: CURSOR (cursor keys, Space
fires), NUMPAD or OFF; JOY PORT (machines with several joystick ports)
moves the joysticks to another port; PAD FIRE picks the gamepad button that
fires (SOUTH = A on Xbox pads); SAVE MAPPING keeps these three for the
machine. Cheats (from --cheat, the cheat database or the debugger) appear
as OFF/ON switches.
Ctrl+Space shows or hides the status footer: a speed slider (click a step
or drag the knob: 10 % to 800 % of real time; sound is muted off
100 %), a snapshot button (saves the picture as <machine>-NNN.png in
settings.snapshots of profiles.yaml, else the working folder), emulated time since power-on (m:ss.t) and frame count, mouse
pointer position in frame pixels, joystick directions and drive
activity/errors.
With --option port1=mouse1351 (or paddles, driving, also port2) a click into the
window captures the mouse for the machine; the middle button releases it.
With a light pen (--option port1=lightpen on the C64/C128/Atari 8-bit,
port0=lightpen on the Amiga, expansion=lightpen on the CPC) or a Zapper /
Super Scope the pointer over the picture aims it, the left button is its button.

EMU_DESKTOP_DEBUG=1 prints frame and audio statistics every 5 s.";

struct Args {
    machine: String,
    config: MachineConfig,
    rom_dir: Option<PathBuf>,
    prg: Option<String>,
    disk: Vec<String>,
    /// Disks the first drive swaps through (Ctrl+D), filled by
    /// `build_machine`.
    disk_swap: Vec<String>,
    tape: Option<String>,
    cart: Option<String>,
    crt: Option<String>,
    deinterlace: Deinterlace,
    rewind_mb: u32,
    cheats_dir: Option<PathBuf>,
    /// `--net off`: the devices see no network.
    net_off: bool,
    /// `--uci-dir`.
    uci_dir: Option<PathBuf>,
    /// Filled by `build_machine`: the machine's host-service journal.
    host_journal: Option<emu_core::host::Journal>,
    cheat: Vec<String>,
    input: Option<PathBuf>,
    scale: u32,
    debug: Option<SocketAddr>,
    vice: Option<SocketAddr>,
    preview: Option<SocketAddr>,
    footer: bool,
    panel: bool,
    joykeys: Option<JoyKeys>,
    joyport: Option<u8>,
    /// Single ROM files by role (`--rom`).
    rom: Vec<(String, String)>,
    /// Start speed in percent.
    speed: Option<u32>,
}

const DEFAULT_DEBUG_ADDR: &str = "127.0.0.1:6510";
const DEFAULT_VICE_ADDR: &str = "127.0.0.1:6502";
const DEFAULT_PREVIEW_ADDR: &str = "127.0.0.1:6580";

fn parse_args(argv: Vec<String>) -> Result<Option<Args>, String> {
    let mut a = Args {
        machine: "c64".into(),
        config: MachineConfig::default(),
        rom_dir: None,
        prg: None,
        disk: Vec::new(),
        disk_swap: Vec::new(),
        tape: None,
        cart: None,
        crt: None,
        deinterlace: Deinterlace::Weave,
        rewind_mb: emu_debug::session::DEFAULT_BUDGET_MB,
        cheats_dir: None,
        net_off: false,
        uci_dir: None,
        host_journal: None,
        cheat: Vec::new(),
        input: None,
        scale: 2,
        debug: None,
        vice: None,
        preview: None,
        footer: false,
        panel: true,
        joykeys: None,
        joyport: None,
        rom: Vec::new(),
        speed: None,
    };
    let mut it = argv.into_iter().peekable();
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "--machine" => a.machine = value("--machine")?,
            "--model" => a.config.model = Some(value("--model")?),
            "--prg" => a.prg = Some(value("--prg")?),
            "--disk" => a.disk.push(value("--disk")?),
            "--tape" => a.tape = Some(value("--tape")?),
            "--cart" => a.cart = Some(value("--cart")?),
            "--footer" => a.footer = true,
            "--no-panel" => a.panel = false,
            "--net" => {
                a.net_off = match value("--net")?.as_str() {
                    "on" => false,
                    "off" => true,
                    other => return Err(format!("--net on|off, got '{other}'")),
                }
            }
            "--uci-dir" => a.uci_dir = Some(PathBuf::from(value("--uci-dir")?)),
            "--joyport" => {
                let v = value("--joyport")?;
                let n = v
                    .parse::<u8>()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| format!("--joyport {v}: a port number from 1"))?;
                a.joyport = Some(n - 1);
            }
            "--joykeys" => {
                let v = value("--joykeys")?;
                a.joykeys = Some(
                    JoyKeys::from_name(&v)
                        .ok_or_else(|| format!("--joykeys {v}: cursor, numpad or off"))?,
                );
            }
            "--autostart" => {
                a.config.options.insert("autostart".into(), "on".into());
            }
            "--rom-dir" => a.rom_dir = Some(value("--rom-dir")?.into()),
            "--rom" => {
                let kv = value("--rom")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--rom expects ROLE=FILE, got '{kv}'"))?;
                a.rom.push((k.into(), v.into()));
            }
            "--speed" => {
                let v = value("--speed")?;
                let pct: u32 = v.parse().map_err(|_| format!("--speed {v}: percent"))?;
                a.speed = Some(pct);
            }
            "--option" => {
                let kv = value("--option")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--option expects KEY=VALUE, got '{kv}'"))?;
                a.config.options.insert(k.into(), v.into());
            }
            "--crt" => a.crt = Some(value("--crt")?),
            "--deinterlace" => {
                let v = value("--deinterlace")?;
                a.deinterlace = match v.as_str() {
                    "weave" => Deinterlace::Weave,
                    "bob" => Deinterlace::Bob,
                    _ => return Err(format!("--deinterlace {v}: weave or bob")),
                };
            }
            "--cheats" => a.cheats_dir = Some(PathBuf::from(value("--cheats")?)),
            "--cheat" => a.cheat.push(value("--cheat")?),
            "--input" => a.input = Some(PathBuf::from(value("--input")?)),
            "--rewind-mb" => {
                let v = value("--rewind-mb")?;
                a.rewind_mb = v.parse().map_err(|_| format!("bad --rewind-mb '{v}'"))?;
            }
            "--scale" => {
                let v = value("--scale")?;
                a.scale = v.parse().map_err(|_| format!("bad scale '{v}'"))?;
            }
            flag @ ("--debug" | "--vice" | "--preview") => {
                let default = match flag {
                    "--debug" => DEFAULT_DEBUG_ADDR,
                    "--vice" => DEFAULT_VICE_ADDR,
                    _ => DEFAULT_PREVIEW_ADDR,
                };
                let addr = match it.peek() {
                    Some(v) if !v.starts_with("--") => it.next().unwrap_or_default(),
                    _ => default.to_string(),
                };
                let addr: SocketAddr = addr
                    .parse()
                    .map_err(|_| format!("bad {flag} address '{addr}'"))?;
                match flag {
                    "--debug" => a.debug = Some(addr),
                    "--vice" => a.vice = Some(addr),
                    _ => a.preview = Some(addr),
                }
            }
            "--help" | "-h" => return Ok(None),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if a.vice.is_some() && a.debug.is_none() {
        a.debug = DEFAULT_DEBUG_ADDR.parse().ok();
    }
    Ok(Some(a))
}

/// A `--crt` profile file, re-read when it changes so a profile can be
/// edited live. Polled from the window side about once a second; the
/// emulation never waits for it.
struct CrtWatch {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    checked: std::time::Instant,
}

impl CrtWatch {
    fn new(path: PathBuf) -> Self {
        Self {
            modified: Self::modified(&path),
            path,
            checked: std::time::Instant::now(),
        }
    }

    fn modified(path: &Path) -> Option<std::time::SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    /// The profile again when the file changed since the last look (at most
    /// one look a second).
    fn poll(&mut self) -> Option<Result<Profile, String>> {
        if self.checked.elapsed() < std::time::Duration::from_secs(1) {
            return None;
        }
        self.checked = std::time::Instant::now();
        let modified = Self::modified(&self.path);
        if modified.is_none() || modified == self.modified {
            return None;
        }
        self.modified = modified;
        Some(load_profile(&self.path.to_string_lossy()))
    }
}

fn load_profile(name: &str) -> Result<Profile, String> {
    if let Some(p) = Profile::preset(name) {
        return Ok(p);
    }
    let text =
        std::fs::read_to_string(name).map_err(|_| format!("unknown monitor profile '{name}'"))?;
    Profile::from_toml(&text).map_err(|e| format!("{name}: {e}"))
}

fn insert(m: &mut dyn Machine, kind: MediaKind, path: &str) -> Result<(), String> {
    let slot: SlotId = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == kind)
        .map(|s| s.id)
        .ok_or_else(|| format!("no slot for {kind:?} media (DOS ROM missing?)"))?;
    insert_into(m, slot, path)
}

fn insert_into(m: &mut dyn Machine, slot: SlotId, path: &str) -> Result<(), String> {
    let media = media_backends::image::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    m.media_slot(slot)
        .ok_or("no such slot")?
        .insert(media)
        .map_err(|e| format!("{path}: {e}"))
}

fn build_machine(args: &mut Args) -> Result<Box<dyn Machine>, String> {
    let specs = media_backends::roms::specs(&args.machine);
    let explicit: Vec<(String, Vec<u8>)> = args
        .rom
        .iter()
        .map(|(role, path)| {
            std::fs::read(path)
                .map(|d| (role.clone(), d))
                .map_err(|e| format!("--rom {role}={path}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    if !specs.is_empty() && !args.config.options.contains_key("roms") {
        let dir = args.rom_dir.clone().or_else(media_backends::roms::env_dir);
        match dir {
            Some(dir) => {
                args.config.roms =
                    media_backends::roms::load(&dir, &args.machine).map_err(|e| e.to_string())?;
            }
            // Machines whose ROMs are all optional (the SNES's sound boot
            // ROM) run without a ROM folder.
            // Single ROM files (--rom) without a folder: just those.
            None if specs.iter().all(|s| s.optional) || !explicit.is_empty() => {}
            None => {
                return Err(
                    "set EMU_ROM_DIR or --rom-dir to the folder with the machine's ROMs".into(),
                )
            }
        }
    }
    args.config.roms.extend(explicit);
    let mut m = emu_machines::registry()
        .create(&args.machine, &args.config)
        .map_err(|e| e.to_string())?;
    // Network and folder for the devices that reach outside (UCI, WiC64),
    // in a journal the debugger's recordings carry.
    let journal = media_backends::host_journal(
        !args.net_off,
        Some(args.uci_dir.clone().unwrap_or_else(|| "ultimate".into())),
    );
    m.attach_host(journal.services());
    args.host_journal = Some(journal);
    if let Some(p) = &args.prg {
        insert(m.as_mut(), MediaKind::Program, p)?;
    }
    // Each --disk goes into the next drive; the rest wait for Ctrl+D in
    // the first drive's swap list.
    if !args.disk.is_empty() {
        let drives: Vec<SlotId> = m
            .media_slots()
            .into_iter()
            .filter(|s| s.kind == MediaKind::Disk)
            .map(|s| s.id)
            .collect();
        if drives.is_empty() {
            return Err(format!(
                "{} has no disk drive (DOS ROM missing?)",
                args.machine
            ));
        }
        // A drive that refuses an image's format (a Mac floppy drive and a
        // hard disk image) passes it on to the next free one.
        let mut free = drives.clone();
        for path in args.disk.iter().take(drives.len()) {
            let mut first_err = None;
            let pos = free
                .iter()
                .position(|&slot| match insert_into(m.as_mut(), slot, path) {
                    Ok(()) => true,
                    Err(e) => {
                        first_err.get_or_insert(e);
                        false
                    }
                });
            match pos {
                Some(i) => {
                    free.remove(i);
                }
                None => return Err(first_err.unwrap_or_else(|| format!("{path}: no free drive"))),
            }
        }
        args.disk_swap = std::iter::once(args.disk[0].clone())
            .chain(args.disk.iter().skip(drives.len()).cloned())
            .collect();
    }
    if let Some(t) = &args.tape {
        let slot = m
            .media_slots()
            .into_iter()
            .find(|s| s.kind == MediaKind::Tape)
            .map(|s| s.id)
            .ok_or_else(|| format!("{} has no datasette", args.machine))?;
        let media = media_backends::image::open_tape(Path::new(t), &args.machine)
            .map_err(|e| format!("{t}: {e}"))?;
        m.media_slot(slot)
            .ok_or("no such slot")?
            .insert(media)
            .map_err(|e| format!("{t}: {e}"))?;
    }
    if let Some(c) = &args.cart {
        let slot = m
            .media_slots()
            .into_iter()
            .find(|s| s.kind == MediaKind::Cartridge)
            .map(|s| s.id)
            .ok_or_else(|| format!("{} has no expansion port", args.machine))?;
        let media =
            media_backends::image::open_cart(Path::new(c)).map_err(|e| format!("{c}: {e}"))?;
        m.media_slot(slot)
            .ok_or("no such slot")?
            .insert(media)
            .map_err(|e| format!("{c}: {e}"))?;
    }
    Ok(m)
}

fn gamepad_button(b: gilrs::Button) -> Option<Button> {
    use gilrs::Button as G;
    Some(match b {
        G::South => Button::South,
        G::East => Button::East,
        G::West => Button::West,
        G::North => Button::North,
        G::LeftTrigger => Button::LeftShoulder,
        G::RightTrigger => Button::RightShoulder,
        G::Select => Button::Select,
        G::Start => Button::Start,
        G::DPadUp => Button::DPadUp,
        G::DPadDown => Button::DPadDown,
        G::DPadLeft => Button::DPadLeft,
        G::DPadRight => Button::DPadRight,
        _ => return None,
    })
}

struct App {
    title: String,
    /// What the title bar shows now (title plus stop status).
    shown_title: String,
    profile: Profile,
    /// The `--crt` file to re-read when it changes.
    crt_watch: Option<CrtWatch>,
    scale: u32,
    shared: Arc<Shared>,
    input: Sender<HostEvent>,
    gilrs: Option<gilrs::Gilrs>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    error: Option<String>,
    /// Display refresh period from the monitor (0 = not locked) and the
    /// measured average, in ns.
    nominal_period_ns: u64,
    period_ns: f64,
    /// A shift key is down (Shift+F9 = RECORD).
    shift: bool,
    /// A port device wants the host mouse.
    wants_mouse: bool,
    /// A light pen or light gun is plugged in: the pointer over the picture
    /// aims it (no capture), with these buttons (bit 0 left, bit 1 right).
    has_pad: bool,
    wants_pointer: bool,
    pointer_buttons: u8,
    /// The host mouse is captured for the machine.
    mouse_captured: bool,
    /// Host mouse buttons while captured (bit 0 left, bit 1 right).
    mouse_buttons: u8,
    /// A control key is down (Ctrl+Space = status footer).
    ctrl: bool,
    /// The Space of a Ctrl+Space is down; its release is not sent either.
    swallow_space: bool,
    footer_on: bool,
    /// The P of a Ctrl+P is down; its release is not sent either.
    swallow_p: bool,
    swallow_i: bool,
    /// The D of a Ctrl+D is down; its release is not sent either.
    swallow_d: bool,
    /// The R of a Ctrl+R is held (rewinding); its release is not sent either.
    swallow_r: bool,
    panel_on: bool,
    /// The panel's clickable areas, its top and row height (window
    /// pixels) and number of rows.
    panel_hits: Vec<panel::Hit>,
    panel_top: f64,
    panel_row: f64,
    panel_rows: usize,
    /// The panel button held with the mouse.
    panel_held: Option<u16>,
    /// Host pointer in window pixels, while over the window.
    cursor: Option<(f64, f64)>,
    /// Captured mouse movement summed since the capture (footer).
    mouse_sum: (i32, i32),
    /// The footer's speed slider and the footer's top (window pixels),
    /// while the footer is shown.
    speed_slider: Option<footer::Slider>,
    /// The footer's snapshot button, while the footer is shown.
    snapshot_button: Option<footer::Button>,
    footer_top: f64,
    /// The slider's knob is being dragged.
    speed_drag: bool,
    /// The started machine's thread and servers (none while the picker
    /// shows).
    running: Option<Running>,
    /// The profile picker, while no machine runs yet.
    picker: Option<picker::Picker>,
    /// The command line, for starting a picked profile.
    raw_args: Vec<String>,
}

/// What must be shut down when the window closes.
struct Running {
    emu: std::thread::JoinHandle<()>,
    /// The sound output; playing stops when it is dropped.
    _audio: Option<cpal::Stream>,
    server: Option<DebugServer>,
    preview: Option<PreviewServer>,
}

/// A started machine: what the window needs.
struct Started {
    title: String,
    profile: Profile,
    /// `--crt` named a profile file (not a preset).
    crt_file: Option<PathBuf>,
    scale: u32,
    shared: Arc<Shared>,
    input: Sender<HostEvent>,
    wants_mouse: bool,
    wants_pointer: bool,
    /// The machine has a gamepad joystick (the panel offers PAD FIRE).
    has_pad: bool,
    footer: bool,
    panel: bool,
    running: Running,
}

impl App {
    /// The window size for the first frame waiting (through the monitor
    /// profile, at the largest scale up to `--scale` that fits the screen).
    fn window_size(&mut self, event_loop: &ActiveEventLoop) -> (u32, u32) {
        // Size from the machine's first frame through the monitor profile,
        // at the largest scale up to `--scale` that fits the screen (90 % of
        // its height); a picture too tall even at scale 1 (the Vectrex's)
        // gets a proportionally smaller window.
        let screen_h = event_loop
            .primary_monitor()
            .map(|m| m.size().to_logical::<f64>(m.scale_factor()).height * 0.9);
        let first = self
            .shared
            .frames
            .lock()
            .ok()
            .and_then(|q| q.front().map(|f| (f.width, f.height, f.signal)));
        let size = match first {
            Some((width, height, signal)) => {
                let r = emu_core::FrameRef {
                    width,
                    height,
                    pixels: emu_core::frame::PixelData::Rgb(&[]),
                    luma_chroma: None,
                    signal,
                };
                if let Some(max_h) = screen_h {
                    while self.scale > 1 && f64::from(height * self.scale) > max_h {
                        self.scale -= 1;
                    }
                }
                let (w, h) = video_crt::cpu::output_size(&r, &self.profile, self.scale);
                match screen_h {
                    Some(max_h) if f64::from(h) > max_h => {
                        let k = max_h / f64::from(h);
                        (
                            (f64::from(w) * k).round() as u32,
                            (f64::from(h) * k).round() as u32,
                        )
                    }
                    _ => (w, h),
                }
            }
            None => (768, 576),
        };
        size
    }

    /// Lock emulation to the display when its refresh matches the
    /// machine's frame rate.
    fn lock_display(&mut self, window: &Window) {
        // Lock emulation to the display when its refresh matches the
        // machine's frame rate within 1 % (the audio rate control absorbs
        // the difference); otherwise run on the wall clock.
        let machine_hz = self.shared.frames.lock().ok().and_then(|q| {
            q.front().map(|f| {
                let s = f.signal;
                f64::from(s.pixel_clock_hz)
                    / (f64::from(s.lines_per_field) * f64::from(s.total_pixels)).max(1.0)
            })
        });
        let display_hz = window
            .current_monitor()
            .and_then(|m| m.refresh_rate_millihertz())
            .map(|mhz| f64::from(mhz) / 1000.0);
        if let (Some(machine), Some(display)) = (machine_hz, display_hz) {
            let locked = (display / machine - 1.0).abs() < 0.01;
            if locked {
                self.nominal_period_ns = (1e9 / display) as u64;
                self.period_ns = self.nominal_period_ns as f64;
                self.shared
                    .display_period_ns
                    .store(self.nominal_period_ns, Ordering::Relaxed);
            }
            eprintln!(
                "display {display:.2} Hz, machine {machine:.2} Hz: {}",
                if locked {
                    "emulating at the display rate (smooth)"
                } else {
                    "wall clock (rates differ: motion is uneven unless the display runs at the machine's rate)"
                }
            );
        }
    }

    /// Start the machine of a picked profile in this window.
    fn pick(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        let Some(name) = self
            .picker
            .as_ref()
            .and_then(|p| p.entries.get(index))
            .map(|(n, _)| n.clone())
        else {
            return;
        };
        let mut argv = self.raw_args.clone();
        argv.extend(["--profile".to_string(), name.clone()]);
        let started = start_from(&argv);
        match started {
            Ok(Some(st)) => self.attach(st, event_loop),
            Ok(None) => {}
            Err(e) => {
                if let Some(p) = &mut self.picker {
                    p.error = Some(format!("{name}: {e}"));
                }
                self.show_picker();
            }
        }
    }

    /// Take over a started machine: its frames, input and window size.
    fn attach(&mut self, st: Started, event_loop: &ActiveEventLoop) {
        self.title = st.title;
        self.profile = st.profile;
        self.crt_watch = st.crt_file.map(CrtWatch::new);
        self.scale = st.scale;
        self.shared = st.shared;
        self.input = st.input;
        self.wants_mouse = st.wants_mouse;
        self.wants_pointer = st.wants_pointer;
        self.has_pad = st.has_pad;
        self.pointer_buttons = 0;
        self.footer_on = st.footer;
        self.panel_on = st.panel;
        self.running = Some(st.running);
        self.picker = None;
        let size = self.window_size(event_loop);
        if let Some(w) = self.window.clone() {
            let _ = w.request_inner_size(winit::dpi::LogicalSize::new(size.0, size.1));
            match Gfx::new(w.clone(), self.profile.clone()) {
                Ok(g) => self.gfx = Some(g),
                Err(e) => {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
            }
            self.lock_display(&w);
        }
    }

    /// Put the picker's picture up.
    fn show_picker(&self) {
        if let (Some(p), Ok(mut q)) = (&self.picker, self.shared.frames.lock()) {
            q.clear();
            q.push_back(p.frame());
        }
    }

    /// The picker row under the pointer.
    fn picker_row(&self) -> Option<usize> {
        let (x, y) = self.cursor?;
        let (_, fy) = self.gfx.as_ref()?.frame_pos(x, y)?;
        self.picker.as_ref()?.row_at(fy)
    }

    /// The panel control under the pointer, if the panel is shown.
    fn over_panel(&self) -> Option<panel::Hit> {
        let (x, y) = self.cursor?;
        if !self.panel_on || y < self.panel_top {
            return None;
        }
        let row = ((y - self.panel_top) / self.panel_row) as usize;
        (row < self.panel_rows)
            .then(|| panel::hit(&self.panel_hits, row, x as usize))
            .flatten()
    }

    /// Is the pointer over the footer's speed slider?
    fn over_speed(&self) -> bool {
        match (self.cursor, self.speed_slider) {
            (Some((x, y)), Some(s)) => self.footer_on && y >= self.footer_top && s.contains(x),
            _ => false,
        }
    }

    /// Is the pointer over the footer's snapshot button?
    fn over_snapshot(&self) -> bool {
        match (self.cursor, self.snapshot_button) {
            (Some((x, y)), Some(b)) => self.footer_on && y >= self.footer_top && b.contains(x),
            _ => false,
        }
    }

    /// Set the speed from the pointer's column on the slider.
    fn speed_from_pointer(&self) {
        if let (Some((x, _)), Some(s)) = (self.cursor, self.speed_slider) {
            self.shared
                .speed
                .store(s.step_at(x) as u8, Ordering::Relaxed);
        }
    }

    fn panel_send(&self, id: u16, value: u8) {
        if id == panel::PAUSE {
            // A press toggles; the release is nothing.
            if value != 0 {
                self.shared.pause_toggle.store(true, Ordering::Relaxed);
            }
            return;
        }
        if id == panel::JOY_KEYS {
            self.shared.joy_keys.store(value, Ordering::Relaxed);
            return;
        }
        if id == panel::JOY_PORT {
            self.shared.joy_port.store(value, Ordering::Relaxed);
            return;
        }
        if id == panel::PAD_FIRE {
            self.shared.pad_fire.store(value, Ordering::Relaxed);
            return;
        }
        if id == panel::SAVE_MAPPING {
            if value != 0 {
                self.shared.save_mapping.store(true, Ordering::Relaxed);
            }
            return;
        }
        if id >= panel::CHEAT_BASE {
            let i = usize::from(id - panel::CHEAT_BASE);
            let row = self
                .shared
                .cheats
                .lock()
                .ok()
                .and_then(|c| c.get(i).cloned());
            if let (Some(row), Ok(mut clicks)) = (row, self.shared.cheat_clicks.lock()) {
                clicks.push((row.id, value == 1));
            }
            return;
        }
        if let Ok(mut e) = self.shared.panel_events.lock() {
            e.push((id, value));
        }
    }

    fn capture_mouse(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        if on {
            let grabbed = w
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined));
            if grabbed.is_err() {
                return;
            }
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
        }
        w.set_cursor_visible(!on);
        self.mouse_captured = on;
        self.mouse_buttons = 0;
        self.mouse_sum = (0, 0);
    }

    /// Where the pointer is over the picture (frame pixels) and its
    /// buttons, for a light pen or light gun.
    fn send_pointer(&self) {
        let pos = self
            .cursor
            .and_then(|(x, y)| self.gfx.as_ref()?.frame_pos(x, y))
            .map(|(x, y)| (x.min(0xfffe) as u16, y.min(0xfffe) as u16));
        let _ = self.input.send(HostEvent::Pointer {
            pos,
            buttons: self.pointer_buttons,
        });
    }

    fn send_mouse(&self, dx: i16, dy: i16) {
        let _ = self.input.send(HostEvent::Mouse {
            dx,
            dy,
            buttons: self.mouse_buttons,
        });
    }
}

impl App {
    /// Live profile editing: take over the `--crt` file when it changed.
    fn reload_crt_profile(&mut self) {
        match self.crt_watch.as_mut().and_then(CrtWatch::poll) {
            Some(Ok(p)) => {
                eprintln!("monitor profile '{}' reloaded", p.name);
                if let Some(g) = &mut self.gfx {
                    g.set_profile(p.clone());
                }
                self.profile = p;
            }
            // Keep the last good profile while the file does not parse.
            Some(Err(e)) => eprintln!("monitor profile: {e}"),
            None => {}
        }
    }

    /// Record a vertical blank (redraws return from a blocking present) and
    /// refine the measured refresh period for the emulation's phase lock.
    fn track_vsync(&mut self) {
        let now = self.shared.epoch.elapsed().as_nanos() as u64;
        let last = self.shared.last_vsync_ns.swap(now, Ordering::Relaxed);
        if self.nominal_period_ns == 0 || last == 0 {
            return;
        }
        let interval = (now - last) as f64;
        let nominal = self.nominal_period_ns as f64;
        // Ignore hiccups (window hidden, dropped refresh).
        if (interval / nominal - 1.0).abs() < 0.1 {
            self.period_ns += (interval - self.period_ns) * 0.02;
            self.shared
                .display_period_ns
                .store(self.period_ns as u64, Ordering::Relaxed);
        }
    }

    fn poll_gamepads(&mut self) {
        let Some(g) = &mut self.gilrs else { return };
        while let Some(ev) = g.next_event() {
            let pad = usize::from(ev.id) as u8;
            let host = match ev.event {
                gilrs::EventType::ButtonPressed(b, _) => {
                    gamepad_button(b).map(|button| HostEvent::Button {
                        pad,
                        button,
                        down: true,
                    })
                }
                gilrs::EventType::ButtonReleased(b, _) => {
                    gamepad_button(b).map(|button| HostEvent::Button {
                        pad,
                        button,
                        down: false,
                    })
                }
                gilrs::EventType::AxisChanged(gilrs::Axis::LeftStickX, v, _) => {
                    Some(HostEvent::Axis {
                        pad,
                        axis: Axis::LeftX,
                        value: v,
                    })
                }
                // gilrs: up is positive; emu-input: down is positive.
                gilrs::EventType::AxisChanged(gilrs::Axis::LeftStickY, v, _) => {
                    Some(HostEvent::Axis {
                        pad,
                        axis: Axis::LeftY,
                        value: -v,
                    })
                }
                gilrs::EventType::Connected => {
                    eprintln!("gamepad {pad} connected: {}", g.gamepad(ev.id).name());
                    None
                }
                gilrs::EventType::Disconnected => {
                    eprintln!("gamepad {pad} disconnected");
                    None
                }
                _ => None,
            };
            if let Some(h) = host {
                let _ = self.input.send(h);
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let size = self.window_size(event_loop);
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_window_icon(window_icon())
            .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("window: {e}"));
                event_loop.exit();
                return;
            }
        };
        match Gfx::new(window.clone(), self.profile.clone()) {
            Ok(g) => self.gfx = Some(g),
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
                return;
            }
        }
        if self.picker.is_none() {
            self.lock_display(&window);
        }
        self.window = Some(window);
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.mouse_captured {
                let c = |v: f64| v.round().clamp(-32768.0, 32767.0) as i16;
                self.mouse_sum.0 = self.mouse_sum.0.wrapping_add(i32::from(c(dx)));
                self.mouse_sum.1 = self.mouse_sum.1.wrapping_add(i32::from(c(dy)));
                self.send_mouse(c(dx), c(dy));
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // The profile picker: hover highlights, a click starts.
            WindowEvent::CursorMoved { position, .. } if self.picker.is_some() => {
                self.cursor = Some((position.x, position.y));
                let row = self.picker_row();
                if let Some(p) = &mut self.picker {
                    if p.hover != row {
                        p.hover = row;
                        self.show_picker();
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if self.picker.is_some() => {
                if let Some(i) = self.picker_row() {
                    self.pick(i, event_loop);
                }
            }
            WindowEvent::KeyboardInput { .. } if self.picker.is_some() => {}
            // The footer's snapshot button: save the picture on a click.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if !self.mouse_captured && !self.speed_drag && self.over_snapshot() => {
                if state == ElementState::Pressed {
                    self.shared.snapshot.store(true, Ordering::Relaxed);
                }
            }
            // The footer's speed slider: click a step or drag the knob.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if !self.mouse_captured && (self.speed_drag || self.over_speed()) => {
                self.speed_drag = state == ElementState::Pressed;
                if self.speed_drag {
                    self.speed_from_pointer();
                }
            }
            // The console panel: a button is held while the mouse button
            // is down, a switch position is set by a click.
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } if !self.mouse_captured
                && (self.panel_held.is_some() || self.over_panel().is_some()) =>
            {
                if state == ElementState::Pressed {
                    if let Some(h) = self.over_panel() {
                        if h.position.is_none() {
                            self.panel_held = Some(h.id);
                        }
                        self.panel_send(h.id, h.position.unwrap_or(1));
                    }
                } else if let Some(id) = self.panel_held.take() {
                    self.panel_send(id, 0);
                }
            }
            // A light pen / light gun: the buttons over the picture.
            WindowEvent::MouseInput { state, button, .. }
                if self.wants_pointer
                    && !self.mouse_captured
                    && matches!(button, MouseButton::Left | MouseButton::Right) =>
            {
                let bit = if button == MouseButton::Left { 1 } else { 2 };
                if state == ElementState::Pressed {
                    self.pointer_buttons |= bit;
                } else {
                    self.pointer_buttons &= !bit;
                }
                self.send_pointer();
            }
            WindowEvent::MouseInput { state, button, .. } if self.wants_mouse => {
                let down = state == ElementState::Pressed;
                match (self.mouse_captured, button) {
                    (false, MouseButton::Left) if down => self.capture_mouse(true),
                    (true, MouseButton::Middle) if down => self.capture_mouse(false),
                    (true, MouseButton::Left | MouseButton::Right) => {
                        let bit = if button == MouseButton::Left { 1 } else { 2 };
                        if down {
                            self.mouse_buttons |= bit;
                        } else {
                            self.mouse_buttons &= !bit;
                        }
                        self.send_mouse(0, 0);
                    }
                    _ => {}
                }
            }
            WindowEvent::Focused(false) if self.mouse_captured => self.capture_mouse(false),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.speed_drag {
                    self.speed_from_pointer();
                }
                if self.wants_pointer {
                    self.send_pointer();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                if self.wants_pointer {
                    self.send_pointer();
                }
            }
            WindowEvent::DroppedFile(path) => {
                if let Ok(mut d) = self.shared.dropped_disks.lock() {
                    d.push(path.to_string_lossy().into_owned());
                }
            }
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // F12 switches between the machine's screens (C128 VIC/VDC).
                if event.physical_key == PhysicalKey::Code(KeyCode::F12) {
                    if event.state == ElementState::Pressed && !event.repeat {
                        self.shared.next_screen.store(true, Ordering::Relaxed);
                    }
                    return;
                }
                // Ctrl+Space shows or hides the status footer; the Space
                // does not reach the machine (the Ctrl key does).
                if event.physical_key == PhysicalKey::Code(KeyCode::Space)
                    && (self.ctrl || self.swallow_space)
                {
                    if event.state == ElementState::Pressed {
                        if !event.repeat {
                            self.footer_on = !self.footer_on;
                            self.shared
                                .footer_on
                                .store(self.footer_on, Ordering::Relaxed);
                        }
                        self.swallow_space = true;
                    } else {
                        self.swallow_space = false;
                    }
                    return;
                }
                // Ctrl+P shows or hides the console panel.
                if event.physical_key == PhysicalKey::Code(KeyCode::KeyP)
                    && (self.ctrl || self.swallow_p)
                {
                    if event.state == ElementState::Pressed {
                        if !event.repeat {
                            self.panel_on = !self.panel_on;
                            self.shared.panel_on.store(self.panel_on, Ordering::Relaxed);
                        }
                        self.swallow_p = true;
                    } else {
                        self.swallow_p = false;
                    }
                    return;
                }
                // Ctrl+I switches between weave and bob deinterlacing.
                if event.physical_key == PhysicalKey::Code(KeyCode::KeyI)
                    && (self.ctrl || self.swallow_i)
                {
                    if event.state == ElementState::Pressed {
                        if !event.repeat {
                            if let Ok(mut fields) = self.shared.fields.lock() {
                                fields.mode = match fields.mode {
                                    Deinterlace::Weave => Deinterlace::Bob,
                                    Deinterlace::Bob => Deinterlace::Weave,
                                };
                            }
                        }
                        self.swallow_i = true;
                    } else {
                        self.swallow_i = false;
                    }
                    return;
                }
                // Ctrl+D swaps the next disk into the first drive.
                if event.physical_key == PhysicalKey::Code(KeyCode::KeyD)
                    && (self.ctrl || self.swallow_d)
                {
                    if event.state == ElementState::Pressed {
                        if !event.repeat {
                            self.shared.next_disk.store(true, Ordering::Relaxed);
                        }
                        self.swallow_d = true;
                    } else {
                        self.swallow_d = false;
                    }
                    return;
                }
                // Ctrl+R held rewinds.
                if event.physical_key == PhysicalKey::Code(KeyCode::KeyR)
                    && (self.ctrl || self.swallow_r)
                {
                    let down = event.state == ElementState::Pressed;
                    self.swallow_r = down;
                    self.shared.rewinding.store(down, Ordering::Relaxed);
                    return;
                }
                if let PhysicalKey::Code(KeyCode::ControlLeft | KeyCode::ControlRight) =
                    event.physical_key
                {
                    self.ctrl = event.state == ElementState::Pressed;
                }
                // F9–F11 operate the datasette.
                let tape_key = match event.physical_key {
                    PhysicalKey::Code(KeyCode::F9) if self.shift => Some(TapeKey::Record),
                    PhysicalKey::Code(KeyCode::F9) => Some(TapeKey::Play),
                    PhysicalKey::Code(KeyCode::F10) => Some(TapeKey::Stop),
                    PhysicalKey::Code(KeyCode::F11) => Some(TapeKey::Rewind),
                    _ => None,
                };
                if let Some(key) = tape_key {
                    if event.state == ElementState::Pressed && !event.repeat {
                        self.shared.tape_key.store(key as u8, Ordering::Relaxed);
                    }
                    return;
                }
                if let PhysicalKey::Code(KeyCode::ShiftLeft | KeyCode::ShiftRight) =
                    event.physical_key
                {
                    self.shift = event.state == ElementState::Pressed;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    // winit names its key codes after W3C KeyboardEvent.code.
                    let _ = self.input.send(HostEvent::Key {
                        code: format!("{code:?}"),
                        down: event.state == ElementState::Pressed,
                    });
                }
            }
            WindowEvent::RedrawRequested => {
                self.shared.redraws.fetch_add(1, Ordering::Relaxed);
                self.track_vsync();
                // One frame per display refresh, in order. The emulation is
                // phase-locked to the refresh (frames are ready half a
                // refresh ahead), so normally exactly one frame waits here. A
                // backlog beyond two frames (window hidden, slow GPU) is
                // skipped, not replayed.
                if self.picker.is_some() {
                    self.show_picker();
                }
                let shared = &self.shared;
                let frame = self.shared.frames.lock().ok().and_then(|mut q| {
                    while q.len() > 2 {
                        q.pop_front();
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    q.pop_front()
                });
                self.reload_crt_profile();
                if let Some(g) = &mut self.gfx {
                    if let Some(f) = frame {
                        g.upload(&f, self.scale);
                        self.shared.presented.fetch_add(1, Ordering::Relaxed);
                    }
                    let size =
                        footer::font_size(self.window.as_ref().map_or(1.0, |w| w.scale_factor()));
                    let width = g.footer_width();
                    let row = footer::height(size);
                    // The strip under the picture: the panel, then the footer.
                    let mut strip = Vec::new();
                    let mut rows = 0;
                    if self.panel_on {
                        let mut controls = vec![panel::pause_control(
                            self.shared.paused.load(Ordering::Relaxed),
                        )];
                        if let Ok(c) = self.shared.controls.lock() {
                            controls.extend(c.iter().cloned());
                        }
                        controls.push(panel::joy_keys_control(
                            self.shared.joy_keys.load(Ordering::Relaxed),
                        ));
                        let ports = self.shared.joy_ports.load(Ordering::Relaxed);
                        if ports > 1 {
                            controls.push(panel::joy_port_control(
                                ports,
                                self.shared.joy_port.load(Ordering::Relaxed),
                            ));
                        }
                        if self.has_pad {
                            controls.push(panel::pad_fire_control(
                                self.shared.pad_fire.load(Ordering::Relaxed),
                            ));
                        }
                        controls.push(panel::save_mapping_control());
                        let mut items: Vec<panel::Item> =
                            controls.iter().map(panel::Item::from).collect();
                        if let Ok(cheats) = self.shared.cheats.lock() {
                            items.extend(cheats.iter().enumerate().map(|(i, c)| panel::Item {
                                id: panel::CHEAT_BASE + i as u16,
                                name: c.label.clone(),
                                positions: vec!["OFF".into(), "ON".into()],
                                value: u8::from(c.enabled),
                            }));
                        }
                        let (rgba, hits, n) = panel::render_items(&items, width, size);
                        strip.extend_from_slice(&rgba);
                        self.panel_hits = hits;
                        self.panel_rows = n;
                        rows += row * n as u32;
                    }
                    if self.footer_on {
                        let mouse = if self.mouse_captured {
                            footer::Mouse::Captured(self.mouse_sum.0, self.mouse_sum.1)
                        } else {
                            self.cursor
                                .and_then(|(x, y)| g.frame_pos(x, y))
                                .map_or(footer::Mouse::Outside, |(x, y)| footer::Mouse::At(x, y))
                        };
                        let info = self
                            .shared
                            .footer
                            .lock()
                            .map(|f| f.clone())
                            .unwrap_or_default();
                        let speed = usize::from(self.shared.speed.load(Ordering::Relaxed));
                        let (rgba, slider, button) =
                            footer::render(&footer::segments(&info, mouse), speed, width, size);
                        strip.extend_from_slice(&rgba);
                        rows += row;
                        self.speed_slider = Some(slider);
                        self.snapshot_button = Some(button);
                    } else {
                        self.speed_slider = None;
                        self.snapshot_button = None;
                        self.speed_drag = false;
                    }
                    if rows == 0 {
                        g.set_footer(None);
                    } else {
                        g.set_footer(Some((&strip, width, rows)));
                    }
                    let window_height = self.window.as_ref().map_or(0, |w| w.inner_size().height);
                    self.panel_top = f64::from(window_height.saturating_sub(rows));
                    self.panel_row = f64::from(row);
                    self.footer_top = f64::from(window_height.saturating_sub(row));
                    if !self.panel_on {
                        self.panel_rows = 0;
                    }
                    g.render();
                }
                if let (Some(w), Ok(status)) = (&self.window, self.shared.status.lock()) {
                    let title = match status.as_ref() {
                        Some(s) => format!("{} — {s}", self.title),
                        None => self.title.clone(),
                    };
                    if title != self.shown_title {
                        w.set_title(&title);
                        self.shown_title = title;
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.poll_gamepads();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

/// Build and start the machine of `args`: the emulation thread, sound and
/// the servers asked for.
fn start(mut args: Args, settings: &emu_profile::Settings) -> Result<Started, String> {
    let machine = build_machine(&mut args)?;
    let profile = match &args.crt {
        Some(name) => load_profile(name)?,
        None => Profile::default_for(&args.machine),
    };
    // A mouse or paddles in a control port: the captured host mouse drives
    // them (click the window to capture, middle button to release); a light
    // pen or light gun: the host pointer over the picture aims it (no
    // capture), the left button is its button.
    let mut input_profile = InputProfile::for_options(&args.machine, &args.config.options);
    // A saved mapping (SAVE MAPPING) or --input replaces the joysticks; the
    // devices the options plug in keep their mouse and pointer.
    let saved = args.input.clone().or_else(|| {
        emu_profile::user_dir()
            .map(|d| d.join("input").join(format!("{}.toml", args.machine)))
            .filter(|p| p.is_file())
    });
    if let Some(path) = saved {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut p =
            InputProfile::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if p.machine != args.machine {
            return Err(format!("{}: a mapping for {}", path.display(), p.machine));
        }
        p.mouse = input_profile.mouse;
        p.pointer = input_profile.pointer;
        input_profile = p;
    }
    let wants_pointer = !input_profile.pointer.is_empty();
    let wants_mouse = !input_profile.mouse.is_empty();
    let mut mapper = Mapper::new(input_profile);
    if let Some(keys) = args.joykeys {
        mapper.set_joy_keys(keys, emu_core::Cycle(0));
    }
    // Joystick ports of the machine (not the user port, IEC …).
    let joy_ports = machine
        .ports()
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                PortKind::CbmControl
                    | PortKind::AtariJoy
                    | PortKind::CpcJoy
                    | PortKind::AmigaGameport
                    | PortKind::ColecoControl
                    | PortKind::NesControl
                    | PortKind::SnesControl
                    | PortKind::VectrexControl
                    | PortKind::AppleGame
            )
        })
        .count()
        .min(4) as u8;
    if let Some(port) = args.joyport {
        if port >= joy_ports {
            return Err(format!(
                "--joyport {}: the machine has {joy_ports} joystick port(s)",
                port + 1
            ));
        }
        mapper.set_joy_port(port, emu_core::Cycle(0));
    }
    let joy_port = mapper.joy_port().unwrap_or(0);
    let joy_keys = JoyKeys::ALL
        .iter()
        .position(|&k| k == mapper.joy_keys())
        .unwrap_or(0) as u8;

    let shared = Arc::new(Shared::new());
    if let Ok(mut n) = shared.snapshot_name.lock() {
        *n = args.machine.clone();
    }
    if let Ok(mut fields) = shared.fields.lock() {
        fields.mode = args.deinterlace;
    }
    shared.rewind_mb.store(args.rewind_mb, Ordering::Relaxed);
    shared.footer_on.store(args.footer, Ordering::Relaxed);
    shared.panel_on.store(args.panel, Ordering::Relaxed);
    shared.joy_keys.store(joy_keys, Ordering::Relaxed);
    shared.joy_port.store(joy_port, Ordering::Relaxed);
    shared.joy_ports.store(joy_ports, Ordering::Relaxed);
    let has_pad = mapper.pad_fire().is_some();
    let pad_fire = mapper
        .pad_fire()
        .and_then(|b| panel::PAD_FIRE_BUTTONS.iter().position(|&x| x == b))
        .unwrap_or(0) as u8;
    shared.pad_fire.store(pad_fire, Ordering::Relaxed);
    // Cheats: files from --cheat now (to report errors before the window
    // opens), the database and the start images on the emulation thread.
    let main_space = machine
        .memory_spaces()
        .first()
        .map_or("cpu", |s| s.name)
        .to_string();
    let mut cheat_files = Vec::new();
    for path in &args.cheat {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let file =
            emu_cheats::import::import(emu_cheats::format_for_path(path), &text, &main_space)
                .map_err(|e| format!("{path}: {e}"))?;
        cheat_files.push(file);
    }
    if let Ok(mut c) = shared.startup_cheats.lock() {
        *c = cheat_files;
    }
    if let Ok(mut d) = shared.cheats_dir.lock() {
        *d = args.cheats_dir.clone().or_else(emu_cheats::default_dir);
    }
    if let Ok(mut j) = shared.host_journal.lock() {
        *j = args.host_journal.take();
    }
    if let Ok(mut i) = shared.startup_images.lock() {
        *i = args
            .disk
            .iter()
            .chain(&args.prg)
            .chain(&args.tape)
            .chain(&args.cart)
            .cloned()
            .collect();
    }
    let (tx, rx) = mpsc::channel();
    let audio_stream = audio::start(shared.clone());
    if audio_stream.is_none() {
        eprintln!("no audio device: running silent, paced by the clock");
    }
    let mut server = None;
    let debug = match args.debug {
        Some(bind) => {
            let config = ServerConfig {
                bind,
                vice: args.vice,
                run: true,
                realtime: true,
            };
            let (s, endpoint) = DebugServer::start_attached(config, &args.machine)
                .map_err(|e| format!("debug server: {e}"))?;
            eprintln!("debug server listening on {}", s.addr());
            if let Some(vice) = s.vice_addr() {
                eprintln!("VICE binary monitor listening on {vice}");
            }
            server = Some(s);
            Some((endpoint, args.machine.clone()))
        }
        None => None,
    };
    let mut preview = None;
    let tap = match args.preview {
        Some(bind) => {
            let tap = FrameTap::new();
            // Viewer input joins the window's own keys and gamepads.
            let input = tx.clone();
            let panel = shared.clone();
            let sink: InputSink = Arc::new(move |ev| {
                if let emu_preview::server::RemoteInput::Control { id, value } = ev {
                    if let Ok(mut e) = panel.panel_events.lock() {
                        e.push((id, value));
                    }
                } else if let Some(ev) = host::host_event(ev) {
                    let _ = input.send(ev);
                }
            });
            let p = PreviewServer::start(
                PreviewConfig { bind },
                &args.machine,
                tap.clone(),
                Some(sink),
            )
            .map_err(|e| format!("preview server: {e}"))?;
            eprintln!("live preview on http://{}/", p.addr());
            preview = Some(p);
            Some(tap)
        }
        None => None,
    };
    if let Ok(mut l) = shared.disk_swap.lock() {
        l.clone_from(&args.disk_swap);
    }
    let emu = emu::spawn(machine, mapper, rx, shared.clone(), debug, tap);

    // Wait briefly for the first frame so the window opens at the right size.
    for _ in 0..100 {
        if shared.frames.lock().is_ok_and(|q| !q.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    if let Ok(mut d) = shared.snapshot_dir.lock() {
        *d = settings.snapshots.as_ref().map(PathBuf::from);
    }
    if let Some(pct) = args.speed {
        // The slider step nearest to the percentage.
        let step = footer::SPEEDS
            .iter()
            .enumerate()
            .min_by_key(|(_, &s)| s.abs_diff(pct))
            .map_or(footer::NORMAL_SPEED, |(i, _)| i);
        shared.speed.store(step as u8, Ordering::Relaxed);
    }
    let crt_file = args
        .crt
        .as_deref()
        .filter(|name| Profile::preset(name).is_none())
        .map(PathBuf::from);
    Ok(Started {
        title: format!("mimic — {}", args.machine),
        profile,
        crt_file,
        scale: args.scale,
        shared,
        input: tx,
        wants_mouse,
        wants_pointer,
        has_pad,
        footer: args.footer,
        panel: args.panel,
        running: Running {
            emu,
            _audio: audio_stream,
            server,
            preview,
        },
    })
}

/// Profiles, then the command line, then the machine: `None` when there is
/// nothing to run (`--list-profiles`, `--help`).
fn start_from(raw: &[String]) -> Result<Option<Started>, String> {
    let mut notes = Vec::new();
    let (argv, settings) =
        match emu_profile::prepare(raw, emu_profile::Frontend::Desktop, &mut notes)? {
            emu_profile::Prepared::Run(a, s) => (a, s),
            emu_profile::Prepared::Done(text) => {
                print!("{text}");
                return Ok(None);
            }
        };
    for n in notes {
        eprintln!("{n}");
    }
    let Some(args) = parse_args(argv)? else {
        println!("{USAGE}");
        return Ok(None);
    };
    start(args, &settings).map(Some)
}

/// The picker's list when the command line names no machine and no
/// profile and a profile file has profiles.
fn picker_for(raw: &[String]) -> Option<picker::Picker> {
    if raw.iter().any(|a| {
        matches!(
            a.as_str(),
            "--machine" | "--profile" | "--list-profiles" | "--save-profile" | "--help" | "-h"
        )
    }) {
        return None;
    }
    let explicit = raw
        .iter()
        .position(|a| a == "--profiles")
        .and_then(|i| raw.get(i + 1))
        .map(PathBuf::from);
    let path = emu_profile::find_file(explicit.as_deref())?;
    let file = emu_profile::ProfileFile::load(&path).ok()?;
    if file.profiles.is_empty() {
        return None;
    }
    Some(picker::Picker {
        entries: file
            .profiles
            .iter()
            .map(|(n, p)| {
                let what = p.description.clone().unwrap_or_else(|| {
                    format!(
                        "{} {}",
                        p.machine.as_deref().unwrap_or("?"),
                        p.model.as_deref().unwrap_or("")
                    )
                });
                (n.clone(), what)
            })
            .collect(),
        file: path.display().to_string(),
        hover: None,
        error: None,
    })
}

fn run() -> Result<(), String> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let picker = picker_for(&raw);
    let (st, picker) = match picker {
        Some(p) => {
            // A window without a machine: the list of profiles.
            let (tx, _) = mpsc::channel();
            let shared = Arc::new(Shared::new());
            let st = Started {
                title: "mimic — choose a profile".into(),
                profile: Profile::preset("raw").unwrap_or_default(),
                crt_file: None,
                scale: 1,
                shared,
                input: tx,
                wants_mouse: false,
                wants_pointer: false,
                has_pad: false,
                footer: false,
                panel: false,
                running: Running {
                    emu: std::thread::spawn(|| {}),
                    _audio: None,
                    server: None,
                    preview: None,
                },
            };
            if let Ok(mut q) = st.shared.frames.lock() {
                q.push_back(p.frame());
            }
            (st, Some(p))
        }
        None => match start_from(&raw)? {
            Some(st) => (st, None),
            None => return Ok(()),
        },
    };
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        title: st.title,
        shown_title: String::new(),
        profile: st.profile,
        crt_watch: st.crt_file.map(CrtWatch::new),
        scale: st.scale,
        shared: st.shared,
        input: st.input,
        gilrs: gilrs::Gilrs::new().ok().inspect(|g| {
            for (id, pad) in g.gamepads() {
                eprintln!("gamepad {}: {}", usize::from(id), pad.name());
            }
        }),
        window: None,
        gfx: None,
        error: None,
        nominal_period_ns: 0,
        period_ns: 0.0,
        shift: false,
        wants_mouse: st.wants_mouse,
        wants_pointer: st.wants_pointer,
        has_pad: st.has_pad,
        pointer_buttons: 0,
        mouse_captured: false,
        mouse_buttons: 0,
        ctrl: false,
        swallow_space: false,
        footer_on: st.footer,
        swallow_p: false,
        swallow_i: false,
        swallow_d: false,
        swallow_r: false,
        panel_on: st.panel,
        panel_hits: Vec::new(),
        panel_top: 0.0,
        panel_row: 1.0,
        panel_rows: 0,
        panel_held: None,
        cursor: None,
        mouse_sum: (0, 0),
        speed_slider: None,
        snapshot_button: None,
        footer_top: 0.0,
        speed_drag: false,
        running: Some(st.running),
        picker,
        raw_args: raw,
    };
    let result = event_loop.run_app(&mut app).map_err(|e| e.to_string());
    app.shared.quit.store(true, Ordering::Relaxed);
    if let Some(r) = app.running.take() {
        let _ = r.emu.join();
        if let Some(s) = r.server {
            s.shutdown();
        }
        if let Some(p) = r.preview {
            p.shutdown();
        }
    }
    result?;
    app.error.map_or(Ok(()), Err)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_icon_decodes() {
        assert!(super::window_icon().is_some());
    }

    /// The `--crt` file is looked at once a second and re-read when its
    /// modification time changed; a broken file reports an error.
    #[test]
    fn crt_profile_file_is_reloaded_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("mimic-crt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("monitor.toml");
        std::fs::write(&path, "base = \"1702\"\n").unwrap();
        let mut watch = super::CrtWatch::new(path.clone());
        let past = std::time::Instant::now() - std::time::Duration::from_secs(2);
        watch.checked = past;
        assert!(watch.poll().is_none(), "unchanged");
        std::fs::write(&path, "base = \"1702\"\nbloom = 0.5\n").unwrap();
        // A change within the second waits for the next look.
        watch.modified = None;
        watch.checked = std::time::Instant::now();
        assert!(watch.poll().is_none());
        watch.checked = past;
        let p = watch.poll().expect("changed").unwrap();
        assert_eq!(p.bloom, 0.5);
        std::fs::write(&path, "bloom = \"x\"\n").unwrap();
        watch.modified = None;
        watch.checked = past;
        assert!(watch.poll().expect("changed").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
