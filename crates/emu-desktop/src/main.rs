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

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use emu_core::media::{MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::Machine;
use emu_input::{Axis, Button, HostEvent, Mapper, Profile as InputProfile};
use video_crt::Profile;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use emu::Shared;
use gfx::Gfx;

const USAGE: &str = "\
usage: emu-desktop [options]

  --machine ID          machine id (default: c64)
  --prg FILE            program (PRG/T64) to load and run
  --disk FILE           disk image for drive 8
  --autostart           load and run the disk after boot
  --rom-dir DIR         ROM folder (default: $EMU_ROM_DIR)
  --option KEY=VALUE    machine option (repeatable)
  --crt PROFILE         monitor: raw, 1702, 1084s, pal-tv or a .toml file
                        (default: the machine's own monitor)
  --scale N             initial window size in rows per line (default 2)
  --help

C64 keys by position on a PC keyboard: Esc = RUN/STOP, Tab = CTRL,
left Ctrl = C=, Insert = £, Delete = ↑, Page Up = RESTORE, Home = CLR/HOME,
F2/F4/F6/F8 = shifted F1/F3/F5/F7. Joystick (port 2): numeric keypad
8/2/4/6 + 0 or right Ctrl, or the first gamepad.";

struct Args {
    machine: String,
    config: MachineConfig,
    rom_dir: Option<PathBuf>,
    prg: Option<String>,
    disk: Option<String>,
    crt: Option<String>,
    scale: u32,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        machine: "c64".into(),
        config: MachineConfig::default(),
        rom_dir: None,
        prg: None,
        disk: None,
        crt: None,
        scale: 2,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "--machine" => a.machine = value("--machine")?,
            "--prg" => a.prg = Some(value("--prg")?),
            "--disk" => a.disk = Some(value("--disk")?),
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
            "--help" | "-h" => return Ok(None),
            other => return Err(format!("unknown option '{other}'")),
        }
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
    profile: Profile,
    scale: u32,
    shared: Arc<Shared>,
    input: Sender<HostEvent>,
    gilrs: Option<gilrs::Gilrs>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    error: Option<String>,
}

impl App {
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
            .frame
            .lock()
            .ok()
            .and_then(|f| {
                f.as_ref().map(|f| {
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
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gfx {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    // winit names its key codes after W3C KeyboardEvent.code.
                    let _ = self.input.send(HostEvent::Key {
                        code: format!("{code:?}"),
                        down: event.state == ElementState::Pressed,
                    });
                }
            }
            WindowEvent::RedrawRequested => {
                let frame = self.shared.frame.lock().ok().and_then(|mut f| f.take());
                if let Some(g) = &mut self.gfx {
                    if let Some(f) = frame {
                        g.upload(&f, self.scale);
                    }
                    g.render();
                }
                if let (Some(w), Ok(status)) = (&self.window, self.shared.status.lock()) {
                    if let Some(s) = status.as_ref() {
                        w.set_title(&format!("{} — stopped: {s}", self.title));
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
    let mapper = Mapper::new(InputProfile::default_for(&args.machine));

    let shared = Arc::new(Shared::default());
    let (tx, rx) = mpsc::channel();
    let _stream = audio::start(shared.clone());
    if _stream.is_none() {
        eprintln!("no audio device: running silent, paced by the clock");
    }
    let emu = emu::spawn(machine, mapper, rx, shared.clone());

    // Wait briefly for the first frame so the window opens at the right size.
    for _ in 0..100 {
        if shared.frame.lock().is_ok_and(|f| f.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        title: format!("polytrace — {}", args.machine),
        profile,
        scale: args.scale,
        shared: shared.clone(),
        input: tx,
        gilrs: gilrs::Gilrs::new().ok(),
        window: None,
        gfx: None,
        error: None,
    };
    let result = event_loop.run_app(&mut app).map_err(|e| e.to_string());
    shared.quit.store(true, Ordering::Relaxed);
    let _ = emu.join();
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
