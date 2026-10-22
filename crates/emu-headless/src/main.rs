//! Headless runner: run a machine for N frames, write snapshots and
//! savestates, or serve it to a debugger. No window, no audio.

#![forbid(unsafe_code)]

mod host;

use std::fs;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use emu_core::media::{MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Machine, StopReason};
use emu_debug::{DebugServer, ServerConfig, Session};
use emu_preview::server::FrameTap;

const USAGE: &str = "\
usage: mimic-headless [options]

  --machine ID          machine id (default: sim6502); see --list
  --model NAME          model variant
  --prg FILE            program (PRG/T64, Atari XEX) into the machine's program slot,
                        or as its \"program\" ROM role (sim6502)
  --disk FILE           disk image into the first disk drive (drive 8, Atari D1: ATR)
  --tape FILE           TAP image into the datasette, CAS into the Atari
                        program recorder (PLAY pressed); a PRG is recorded
                        in the machine's tape format first
  --cart FILE           cartridge: CRT image, or a raw VIC-20 (load address
                        + ROM) or C16/Plus/4 (16/32 KB) dump; Atari CAR or
                        raw 8/16 KB ROM
  --autostart           load and run the disk after boot (C64)
  --rom ROLE=FILE       ROM image for a role, e.g. kernal=kernal.bin (repeatable)
  --rom-dir DIR         find the machine's ROMs in DIR (default: $EMU_ROM_DIR)
  --option KEY=VALUE    machine option (repeatable)
  --frames N            frames to run (default: 1)
  --snapshot FILE.png   write the last frame as PNG
  --snapshot-every N    with --snapshot: also write FILE-<frame>.png every N frames
  --wav FILE.wav        write the audio of the run (16-bit PCM)
  --screen NAME         snapshot this screen (C128: vic or vdc; default: main)
  --crt PROFILE[:N]     snapshot through a monitor profile (raw, 1702, 1084s,
                        1084s-rgb, tv, pal-tv, ntsc-tv, pet-white, pet-green,
                        ctm644, gt65 or a .toml file) at N rows per line
                        (default 2)
  --load-state FILE     restore a savestate before running
  --save-state FILE     write a savestate after running
  --debug [ADDR]        serve the debug protocol instead of running frames
                        (default 127.0.0.1:6510; loopback only)
  --preview [ADDR]      serve the live preview: viewer page, /snapshot.png,
                        the /preview WebSocket stream and the raw /stream
                        with audio (default 127.0.0.1:6580);
                        implies --debug, and --run unless --debug is given
  --vice [ADDR]         also serve the VICE binary monitor protocol (VS64,
                        C64 Studio …; default 127.0.0.1:6502); implies --debug
  --gdb [ADDR]          also serve the GDB remote protocol (target remote;
                        default 127.0.0.1:2159); implies --debug. In GDB,
                        `monitor preview` starts the live preview
  --run                 with --debug: start running instead of paused
  --no-realtime         with --debug: run as fast as possible
  --list                list machines and exit
  --help                show this help";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    machine: String,
    config: MachineConfig,
    rom_dir: Option<String>,
    prg: Option<String>,
    disk: Option<String>,
    tape: Option<String>,
    cart: Option<String>,
    frames: u64,
    snapshot: Option<String>,
    snapshot_every: u64,
    wav: Option<String>,
    crt: Option<(String, u32)>,
    screen: Option<String>,
    load_state: Option<String>,
    save_state: Option<String>,
    debug: Option<SocketAddr>,
    vice: Option<SocketAddr>,
    gdb: Option<SocketAddr>,
    preview: Option<SocketAddr>,
    run: bool,
    realtime: bool,
    list: bool,
    help: bool,
}

const DEFAULT_DEBUG_ADDR: &str = "127.0.0.1:6510";
const DEFAULT_VICE_ADDR: &str = "127.0.0.1:6502";
const DEFAULT_GDB_ADDR: &str = "127.0.0.1:2159";
const DEFAULT_PREVIEW_ADDR: &str = "127.0.0.1:6580";

fn key_value(s: &str, flag: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("{flag} expects KEY=VALUE, got '{s}'"))
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn parse_args(argv: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        machine: "sim6502".into(),
        frames: 1,
        realtime: true,
        ..Args::default()
    };
    let mut it = argv.into_iter().peekable();
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--machine" => args.machine = value("--machine")?,
            "--model" => args.config.model = Some(value("--model")?),
            "--prg" => args.prg = Some(value("--prg")?),
            "--disk" => args.disk = Some(value("--disk")?),
            "--tape" => args.tape = Some(value("--tape")?),
            "--cart" => args.cart = Some(value("--cart")?),
            "--autostart" => {
                args.config.options.insert("autostart".into(), "on".into());
            }
            "--rom" => {
                let (role, path) = key_value(&value("--rom")?, "--rom")?;
                args.config.roms.insert(role, read(&path)?);
            }
            "--rom-dir" => args.rom_dir = Some(value("--rom-dir")?),
            "--option" => {
                let (k, v) = key_value(&value("--option")?, "--option")?;
                args.config.options.insert(k, v);
            }
            "--frames" => {
                let n = value("--frames")?;
                args.frames = n.parse().map_err(|_| format!("bad frame count '{n}'"))?;
            }
            "--snapshot" => args.snapshot = Some(value("--snapshot")?),
            "--snapshot-every" => {
                let n = value("--snapshot-every")?;
                args.snapshot_every = n.parse().map_err(|_| format!("bad frame count '{n}'"))?;
            }
            "--wav" => args.wav = Some(value("--wav")?),
            "--screen" => args.screen = Some(value("--screen")?),
            "--crt" => {
                let v = value("--crt")?;
                let (name, scale) = match v.rsplit_once(':') {
                    Some((n, s)) => (
                        n.to_string(),
                        s.parse().map_err(|_| format!("bad CRT scale '{s}'"))?,
                    ),
                    None => (v, 2),
                };
                args.crt = Some((name, scale));
            }
            "--load-state" => args.load_state = Some(value("--load-state")?),
            "--save-state" => args.save_state = Some(value("--save-state")?),
            flag @ ("--debug" | "--vice" | "--gdb" | "--preview") => {
                let default = match flag {
                    "--debug" => DEFAULT_DEBUG_ADDR,
                    "--vice" => DEFAULT_VICE_ADDR,
                    "--gdb" => DEFAULT_GDB_ADDR,
                    _ => DEFAULT_PREVIEW_ADDR,
                };
                let addr = match it.peek() {
                    Some(a) if !a.starts_with("--") => it.next().unwrap_or_default(),
                    _ => default.to_string(),
                };
                let addr: SocketAddr = addr
                    .parse()
                    .map_err(|_| format!("bad {flag} address '{addr}'"))?;
                match flag {
                    "--debug" => args.debug = Some(addr),
                    "--vice" => args.vice = Some(addr),
                    "--gdb" => args.gdb = Some(addr),
                    _ => args.preview = Some(addr),
                }
            }
            "--run" => args.run = true,
            "--no-realtime" => args.realtime = false,
            "--list" => args.list = true,
            "--help" | "-h" => args.help = true,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if args.preview.is_some() && args.debug.is_none() && args.vice.is_none() && args.gdb.is_none() {
        // Just watching: run instead of showing a paused machine.
        args.run = true;
    }
    if (args.vice.is_some() || args.gdb.is_some() || args.preview.is_some()) && args.debug.is_none()
    {
        args.debug = DEFAULT_DEBUG_ADDR.parse().ok();
    }
    Ok(args)
}

/// Fill in ROMs the machine needs from --rom-dir or EMU_ROM_DIR, unless
/// given individually or replaced by stubs.
fn with_rom_dir(args: &Args) -> Result<MachineConfig, String> {
    let mut config = args.config.clone();
    let specs = media_backends::roms::specs(&args.machine);
    let missing = specs.iter().any(|s| !config.roms.contains_key(s.role));
    if !missing || config.options.contains_key("roms") {
        return Ok(config);
    }
    let dir = match (&args.rom_dir, media_backends::roms::env_dir()) {
        (Some(d), _) => std::path::PathBuf::from(d),
        (None, Some(d)) => d,
        (None, None) => return Ok(config),
    };
    let found = media_backends::roms::load(&dir, &args.machine).map_err(|e| e.to_string())?;
    for (role, data) in found {
        config.roms.entry(role).or_insert(data);
    }
    Ok(config)
}

fn snapshot_png(machine: &dyn Machine, args: &Args) -> Result<Vec<u8>, String> {
    let frame = match &args.screen {
        None => machine.framebuffer(),
        Some(name) => {
            let screens = machine.screens();
            let id = screens.iter().find(|s| s.name == name).map(|s| s.id);
            id.and_then(|id| machine.screen(id)).ok_or_else(|| {
                let names: Vec<_> = screens.iter().map(|s| s.name).collect();
                format!("no screen '{name}' (this machine has {})", names.join(", "))
            })?
        }
    };
    Ok(match &args.crt {
        Some((name, scale)) => {
            let profile = load_profile(name)?;
            emu_preview::encode_png_crt(&frame, &profile, *scale)
        }
        None => emu_preview::encode_png(&frame),
    })
}

/// A built-in monitor profile by name, or a TOML profile file.
fn load_profile(name: &str) -> Result<video_crt::Profile, String> {
    if let Some(p) = video_crt::Profile::preset(name) {
        return Ok(p);
    }
    let text = fs::read_to_string(name).map_err(|_| {
        format!(
            "unknown monitor profile '{name}' (presets: {})",
            video_crt::Profile::PRESETS.join(", ")
        )
    })?;
    video_crt::Profile::from_toml(&text).map_err(|e| format!("{name}: {e}"))
}

fn slot_of(m: &dyn Machine, kind: MediaKind) -> Option<SlotId> {
    m.media_slots()
        .into_iter()
        .find(|s| s.kind == kind)
        .map(|s| s.id)
}

fn insert(m: &mut dyn Machine, slot: SlotId, path: &str) -> Result<(), String> {
    let media = media_backends::image::open(std::path::Path::new(path))
        .map_err(|e| format!("{path}: {e}"))?;
    m.media_slot(slot)
        .ok_or("no such slot")?
        .insert(media)
        .map_err(|e| format!("{path}: {e}"))
}

fn run(args: Args) -> Result<ExitCode, String> {
    let registry = emu_machines::registry();
    if args.list {
        for e in registry.entries() {
            println!("{:<10} {} (models: {})", e.id, e.name, e.models.join(", "));
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut config = with_rom_dir(&args)?;
    let mut machine = registry
        .create(&args.machine, &config)
        .map_err(|e| e.to_string())?;
    if let Some(path) = &args.prg {
        match slot_of(machine.as_ref(), MediaKind::Program) {
            Some(slot) => insert(machine.as_mut(), slot, path)?,
            None => {
                // Machines without a program slot take it as a ROM role.
                config.roms.insert("program".into(), read(path)?);
                machine = registry
                    .create(&args.machine, &config)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    if let Some(path) = &args.disk {
        let slot = slot_of(machine.as_ref(), MediaKind::Disk)
            .ok_or_else(|| format!("{} has no disk drive (DOS ROM missing?)", args.machine))?;
        insert(machine.as_mut(), slot, path)?;
    }
    if let Some(path) = &args.tape {
        let slot = slot_of(machine.as_ref(), MediaKind::Tape)
            .ok_or_else(|| format!("{} has no datasette", args.machine))?;
        let media = media_backends::image::open_tape(std::path::Path::new(path), &args.machine)
            .map_err(|e| format!("{path}: {e}"))?;
        machine
            .media_slot(slot)
            .ok_or("no such slot")?
            .insert(media)
            .map_err(|e| format!("{path}: {e}"))?;
    }
    if let Some(path) = &args.cart {
        let slot = slot_of(machine.as_ref(), MediaKind::Cartridge)
            .ok_or_else(|| format!("{} has no expansion port", args.machine))?;
        let media = media_backends::image::open_cart(std::path::Path::new(path))
            .map_err(|e| format!("{path}: {e}"))?;
        machine
            .media_slot(slot)
            .ok_or("no such slot")?
            .insert(media)
            .map_err(|e| format!("{path}: {e}"))?;
    }
    if let Some(path) = &args.load_state {
        machine
            .load_state(&read(path)?)
            .map_err(|e| format!("{path}: {e}"))?;
    }

    if let Some(bind) = args.debug {
        let mut session = Session::new(machine, args.machine.clone());
        let preview = Arc::new(Mutex::new(host::Preview::new(&args.machine)));
        session.set_host(Box::new(host::FrontendHost {
            preview: preview.clone(),
        }));
        let tap = args.preview.map(|_| {
            let tap = FrameTap::new();
            tap.publish(session.machine(), args.run);
            session.set_frame_hook(host::frame_hook(tap.clone()));
            tap
        });
        let server = DebugServer::start(
            session,
            ServerConfig {
                bind,
                vice: args.vice,
                gdb: args.gdb,
                run: args.run,
                realtime: args.realtime,
            },
        )
        .map_err(|e| format!("debug server: {e}"))?;
        if let Ok(mut p) = preview.lock() {
            p.control = Some(server.control());
            if let (Some(bind), Some(tap)) = (args.preview, tap) {
                p.start(bind, tap)?;
            }
        }
        eprintln!(
            "debug server for '{}' listening on {} ({})",
            args.machine,
            server.addr(),
            if args.run { "running" } else { "paused" }
        );
        if let Some(vice) = server.vice_addr() {
            eprintln!("VICE binary monitor listening on {vice}");
        }
        if let Some(gdb) = server.gdb_addr() {
            eprintln!("GDB remote protocol listening on {gdb} (target remote {gdb})");
        }
        if let Some(url) = preview.lock().ok().and_then(|p| p.url.clone()) {
            eprintln!("live preview on {url}");
        }
        server.wait();
        if let Some(p) = preview.lock().ok().and_then(|mut p| p.server.take()) {
            p.shutdown();
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut dbg = DebugHooks::new();
    let mut code = ExitCode::SUCCESS;
    let mut frames = 0;
    let mut audio = Vec::new();
    let mut format = None;
    while frames < args.frames {
        let stop = machine.run_frame(&mut dbg);
        if args.wav.is_some() {
            format = Some(machine.audio(&mut audio));
        }
        match stop {
            StopReason::FrameDone => {
                frames += 1;
                if let (Some(path), true) = (&args.snapshot, args.snapshot_every > 0) {
                    if frames % args.snapshot_every == 0 {
                        let p = std::path::Path::new(path);
                        let stem = p
                            .file_stem()
                            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
                        let file = p.with_file_name(format!("{stem}-{frames:05}.png"));
                        let png = snapshot_png(machine.as_ref(), &args)?;
                        fs::write(&file, png)
                            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
                    }
                }
            }
            other => {
                eprintln!("stopped after {frames} frame(s): {other:?}");
                code = ExitCode::from(2);
                break;
            }
        }
    }

    if let Some(path) = &args.snapshot {
        let png = snapshot_png(machine.as_ref(), &args)?;
        fs::write(path, png).map_err(|e| format!("cannot write {path}: {e}"))?;
    }
    if let (Some(path), Some(format)) = (&args.wav, format) {
        fs::write(path, wav(&audio, format)).map_err(|e| format!("cannot write {path}: {e}"))?;
    }
    if let Some(path) = &args.save_state {
        fs::write(path, machine.save_state()).map_err(|e| format!("cannot write {path}: {e}"))?;
    }
    match machine.beam() {
        Some(b) => println!(
            "{}: {frames} frame(s), beam at frame {} line {} cycle {}",
            args.machine, b.frame, b.line, b.cycle
        ),
        None => println!("{}: {frames} frame(s)", args.machine),
    }
    Ok(code)
}

/// A RIFF/WAVE file with 16-bit PCM samples.
fn wav(samples: &[i16], format: emu_core::AudioFormat) -> Vec<u8> {
    let channels = u16::from(format.channels.max(1));
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = format.sample_rate * u32::from(channels) * 2;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&format.sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    run(args).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(String::from))
    }

    #[test]
    fn wav_header() {
        let w = wav(
            &[1, -1],
            emu_core::AudioFormat {
                sample_rate: 48_000,
                channels: 1,
            },
        );
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 48);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 48_000);
        assert_eq!(&w[44..], &[1, 0, 0xff, 0xff]);
    }

    #[test]
    fn defaults() {
        let a = parse("").unwrap();
        assert_eq!(a.machine, "sim6502");
        assert_eq!(a.frames, 1);
        assert!(a.realtime);
        assert_eq!(a.debug, None);
    }

    #[test]
    fn options_and_debug_address() {
        let a = parse("--machine c64 --frames 300 --option start=0810 --debug --run").unwrap();
        assert_eq!(a.machine, "c64");
        assert_eq!(a.frames, 300);
        assert_eq!(a.config.options["start"], "0810");
        assert_eq!(a.debug, Some(DEFAULT_DEBUG_ADDR.parse().unwrap()));
        assert!(a.run);
        let a = parse("--debug 127.0.0.1:7000").unwrap();
        assert_eq!(a.debug, Some("127.0.0.1:7000".parse().unwrap()));
    }

    #[test]
    fn errors() {
        assert!(parse("--frames").is_err());
        assert!(parse("--frames x").is_err());
        assert!(parse("--option novalue").is_err());
        assert!(parse("--bogus").is_err());
        assert!(parse("--disk").is_err());
        assert!(parse("--tape").is_err());
        assert_eq!(
            parse("--cart c.crt").unwrap().cart.as_deref(),
            Some("c.crt")
        );
        assert_eq!(
            parse("--tape t.tap").unwrap().tape.as_deref(),
            Some("t.tap")
        );
        assert!(parse("--wav").is_err());
        assert_eq!(parse("--crt 1702").unwrap().crt, Some(("1702".into(), 2)));
        assert_eq!(
            parse("--crt pal-tv:3").unwrap().crt,
            Some(("pal-tv".into(), 3))
        );
        assert!(parse("--crt 1702:x").is_err());
        let a = parse("--disk x.d64 --autostart").unwrap();
        assert_eq!(a.disk.as_deref(), Some("x.d64"));
        assert_eq!(a.config.options["autostart"], "on");
    }
}
