//! Desktop frontend: a window with the machine's picture through the CRT
//! shader, sound, keyboard and gamepads. No menus yet; everything comes
//! from the command line.
//!
//!     emu-desktop --machine c64 --disk game.d64 --autostart
//!
//! Keys use the positional keymap of `emu-input` (see `--help`); the
//! numeric keypad and the first gamepad act as the joystick in port 2.

#![forbid(unsafe_code)]

mod audio;
mod emu;
mod gfx;
mod host;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use emu_core::media::{MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::Machine;
use emu_debug::{DebugServer, ServerConfig};
use emu_input::{Axis, Button, HostEvent, Mapper, Profile as InputProfile};
use emu_preview::server::{FrameTap, InputSink, PreviewConfig, PreviewServer};
use video_crt::Profile;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use emu::{Shared, TapeKey};
use gfx::Gfx;

const USAGE: &str = "\
usage: emu-desktop [options]

  --machine ID          machine id (default: c64)
  --model NAME          model variant (C64: pal, ntsc)
  --prg FILE            program (PRG/T64) to load and run
  --disk FILE           disk image for drive 8
  --tape FILE           TAP image for the datasette (a PRG is recorded in the
                        machine's tape format)
  --cart FILE           cartridge: CRT image, or a raw VIC-20 (load address
                        + ROM) or C16/Plus/4 (16/32 KB) dump
  --autostart           load and run the disk after boot
  --rom-dir DIR         ROM folder (default: $EMU_ROM_DIR)
  --option KEY=VALUE    machine option (repeatable)
  --crt PROFILE         monitor: raw, 1702, 1084s, 1084s-rgb, pal-tv, ntsc-tv,
                        pet-white, pet-green or a .toml file
                        (default: the machine's own monitor)
  --scale N             initial window size in rows per line (default 2)
  --debug [ADDR]        serve the debug protocol (default 127.0.0.1:6510)
  --preview [ADDR]      serve the live preview in the browser
                        (default 127.0.0.1:6580)
  --vice [ADDR]         serve the VICE binary monitor protocol for VS64 & co.
                        (default 127.0.0.1:6502); implies --debug
  --help

C64 keys by position on a PC keyboard: Esc = RUN/STOP, Tab = CTRL,
left Ctrl = C=, Insert = £, Delete = ↑, Page Up = RESTORE, Home = CLR/HOME,
F2/F4/F6/F8 = shifted F1/F3/F5/F7. Joystick (port 2): numeric keypad
8/2/4/6 + 0 or right Ctrl, or the first gamepad.

Atari 800XL keys by position: < > on - =, - = on [ ], + * on ' \\, Tab, Esc,
CapsLock, right Alt = Atari key, arrows = CONTROL + - = + *; F2 START,
F3 SELECT, F4 OPTION, F5 RESET, F6 HELP, F7/Pause BREAK.

F12 switches between a machine's screens (C128: 40-column VIC and
80-column VDC; the window starts on the one the 40/80 key selects).
Datasette: F9 PLAY, Shift+F9 RECORD, F10 STOP, F11 REWIND.
With --option port1=mouse1351 (or paddles, also port2) a click into the
window captures the mouse for the machine; the middle button releases it.

EMU_DESKTOP_DEBUG=1 prints frame and audio statistics every 5 s.";

struct Args {
    machine: String,
    config: MachineConfig,
    rom_dir: Option<PathBuf>,
    prg: Option<String>,
    disk: Option<String>,
    tape: Option<String>,
    cart: Option<String>,
    crt: Option<String>,
    scale: u32,
    debug: Option<SocketAddr>,
    vice: Option<SocketAddr>,
    preview: Option<SocketAddr>,
}

const DEFAULT_DEBUG_ADDR: &str = "127.0.0.1:6510";
const DEFAULT_VICE_ADDR: &str = "127.0.0.1:6502";
const DEFAULT_PREVIEW_ADDR: &str = "127.0.0.1:6580";

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        machine: "c64".into(),
        config: MachineConfig::default(),
        rom_dir: None,
        prg: None,
        disk: None,
        tape: None,
        cart: None,
        crt: None,
        scale: 2,
        debug: None,
        vice: None,
        preview: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "--machine" => a.machine = value("--machine")?,
            "--model" => a.config.model = Some(value("--model")?),
            "--prg" => a.prg = Some(value("--prg")?),
            "--disk" => a.disk = Some(value("--disk")?),
            "--tape" => a.tape = Some(value("--tape")?),
            "--cart" => a.cart = Some(value("--cart")?),
            "--autostart" => {
                a.config.options.insert("autostart".into(), "on".into());
            }
            "--rom-dir" => a.rom_dir = Some(value("--rom-dir")?.into()),
            "--option" => {
                let kv = value("--option")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| format!("--option expects KEY=VALUE, got '{kv}'"))?;
                a.config.options.insert(k.into(), v.into());
            }
            "--crt" => a.crt = Some(value("--crt")?),
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
    let media = media_backends::image::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    m.media_slot(slot)
        .ok_or("no such slot")?
        .insert(media)
        .map_err(|e| format!("{path}: {e}"))
}

fn build_machine(args: &mut Args) -> Result<Box<dyn Machine>, String> {
    let specs = media_backends::roms::specs(&args.machine);
    if !specs.is_empty() && !args.config.options.contains_key("roms") {
        let dir = args
            .rom_dir
            .clone()
            .or_else(media_backends::roms::env_dir)
            .ok_or("set EMU_ROM_DIR or --rom-dir to the folder with the machine's ROMs")?;
        args.config.roms =
            media_backends::roms::load(&dir, &args.machine).map_err(|e| e.to_string())?;
    }
    let mut m = emu_machines::registry()
        .create(&args.machine, &args.config)
        .map_err(|e| e.to_string())?;
    if let Some(p) = &args.prg {
        insert(m.as_mut(), MediaKind::Program, p)?;
    }
    if let Some(d) = &args.disk {
        insert(m.as_mut(), MediaKind::Disk, d)?;
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
    /// The host mouse is captured for the machine.
    mouse_captured: bool,
    /// Host mouse buttons while captured (bit 0 left, bit 1 right).
    mouse_buttons: u8,
}

impl App {
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
        // Size from the machine's first frame through the monitor profile.
        let size = self
            .shared
            .frames
            .lock()
            .ok()
            .and_then(|q| {
                q.front().map(|f| {
                    let r = emu_core::FrameRef {
                        width: f.width,
                        height: f.height,
                        pixels: emu_core::frame::PixelData::Rgb(&[]),
                        luma_chroma: None,
                        signal: f.signal,
                    };
                    video_crt::cpu::output_size(&r, &self.profile, self.scale)
                })
            })
            .unwrap_or((768, 576));
        let attrs = Window::default_attributes()
            .with_title(&self.title)
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
        self.window = Some(window);
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.mouse_captured {
                let c = |v: f64| v.round().clamp(-32768.0, 32767.0) as i16;
                self.send_mouse(c(dx), c(dy));
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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
                let shared = &self.shared;
                let frame = self.shared.frames.lock().ok().and_then(|mut q| {
                    while q.len() > 2 {
                        q.pop_front();
                        shared.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                    q.pop_front()
                });
                if let Some(g) = &mut self.gfx {
                    if let Some(f) = frame {
                        g.upload(&f, self.scale);
                        self.shared.presented.fetch_add(1, Ordering::Relaxed);
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

fn run() -> Result<(), String> {
    let Some(mut args) = parse_args()? else {
        println!("{USAGE}");
        return Ok(());
    };
    let machine = build_machine(&mut args)?;
    let profile = match &args.crt {
        Some(name) => load_profile(name)?,
        None => Profile::default_for(&args.machine),
    };
    let mut input_profile = InputProfile::default_for(&args.machine);
    // A mouse or paddles in a control port: the captured host mouse drives
    // them (click the window to capture, middle button to release).
    for (option, port) in [("port1", 0u8), ("port2", 1)] {
        if matches!(
            args.config.options.get(option).map(String::as_str),
            Some("mouse1351" | "paddles")
        ) {
            input_profile
                .mouse
                .push(emu_input::profile::MouseMap { port });
        }
    }
    let wants_mouse = !input_profile.mouse.is_empty();
    let mapper = Mapper::new(input_profile);

    let shared = Arc::new(Shared::new());
    let (tx, rx) = mpsc::channel();
    let _stream = audio::start(shared.clone());
    if _stream.is_none() {
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
            let sink: InputSink = Arc::new(move |ev| {
                if let Some(ev) = host::host_event(ev) {
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
    let emu = emu::spawn(machine, mapper, rx, shared.clone(), debug, tap);

    // Wait briefly for the first frame so the window opens at the right size.
    for _ in 0..100 {
        if shared.frames.lock().is_ok_and(|q| !q.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        title: format!("polytrace — {}", args.machine),
        shown_title: String::new(),
        profile,
        scale: args.scale,
        shared: shared.clone(),
        input: tx,
        gilrs: gilrs::Gilrs::new().ok(),
        window: None,
        gfx: None,
        error: None,
        nominal_period_ns: 0,
        period_ns: 0.0,
        shift: false,
        wants_mouse,
        mouse_captured: false,
        mouse_buttons: 0,
    };
    let result = event_loop.run_app(&mut app).map_err(|e| e.to_string());
    shared.quit.store(true, Ordering::Relaxed);
    let _ = emu.join();
    if let Some(s) = server {
        s.shutdown();
    }
    if let Some(p) = preview {
        p.shutdown();
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
