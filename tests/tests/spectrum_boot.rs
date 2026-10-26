//! ZX Spectrum 48K, 128K and +2 with the Sinclair/Amstrad ROMs
//! (user-supplied via `EMU_ROM_DIR`, e.g. a `spectrum/` subfolder with
//! `Spec48.rom`, `spec128.rom`, `specp2.rom`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test spectrum_boot -- --ignored --nocapture
//!
//! Phase 15 gate: every model boots to BASIC and prints, and a TAP file
//! (built here: a BASIC program that runs itself) loads through the ROM's
//! LOAD — with the loading trap and by playing the tape's pulses.

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use std::collections::BTreeMap;

fn machine(model: &str, options: &[(&str, &str)]) -> (Box<dyn Machine>, Vec<u8>) {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&dir, "spectrum").expect("ROMs");
    let role = match model {
        "48k" => "spectrum48",
        "128k" => "spectrum128",
        _ => "spectrumplus2",
    };
    // The 48 BASIC ROM (the last 16 KB) holds the character set at $3D00.
    let rom = roms.get(role).unwrap_or_else(|| panic!("no {role} ROM"));
    let font = rom[rom.len() - 0x4000 + 0x3d00..rom.len() - 0x4000 + 0x4000].to_vec();
    let config = MachineConfig {
        model: Some(model.into()),
        roms,
        options: options
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    };
    (
        emu_machines::registry()
            .create("spectrum", &config)
            .unwrap(),
        font,
    )
}

fn run(m: &mut dyn Machine, frames: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &mut dyn Machine, addr: u32) -> u8 {
    m.debug_target(DeviceId("cpu"))
        .unwrap()
        .peek(machine_spectrum::CPU_SPACE, addr, Size::Byte)
        .unwrap() as u8
}

/// The 24 × 32 text on the screen at $4000, read back through the ROM's
/// character set (inverse cells count too; unknown cells are `?`).
fn screen(m: &mut dyn Machine, font: &[u8]) -> Vec<String> {
    let mut rows = Vec::new();
    for row in 0..24u32 {
        let mut s = String::new();
        for col in 0..32u32 {
            let cell: Vec<u8> = (0..8u32)
                .map(|line| {
                    let y = row * 8 + line;
                    let off = ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2);
                    byte(m, 0x4000 + off + col)
                })
                .collect();
            let c = (0..96).find(|&c| {
                let glyph = &font[c * 8..c * 8 + 8];
                glyph == &cell[..] || glyph.iter().zip(&cell).all(|(g, v)| *g == !*v)
            });
            s.push(c.map_or('?', |c| (32 + c as u8) as char));
        }
        rows.push(s);
    }
    rows
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, _) = machine_spectrum::keyboard::typed_events(text, m.now().0, 3_500_000);
    for e in events {
        m.input(e);
    }
}

/// Type `text` and run until it is typed (eight frames a key).
fn type_all(m: &mut dyn Machine, text: &str) {
    type_text(m, text);
    run(m, text.chars().count() * 8 + 10);
}

/// Press keys (matrix codes) together for a few frames.
fn press(m: &mut dyn Machine, keys: &[u16]) {
    use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
    let at = m.now();
    for (down, delay) in [(true, 0u64), (false, 3_500_000 / 50 * 4)] {
        for &k in keys {
            m.input(TimedInput {
                at: emu_core::Cycle(at.0 + delay),
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: MachineKey(k),
                    down,
                },
            });
        }
    }
    run(m, 10);
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

const ENTER: u16 = 30;
const CAPS: u16 = 0;
/// Key 6 (row 4, column 4): CAPS SHIFT + 6 is cursor down.
const KEY6: u16 = 24;

/// 128K and +2: from the menu (on "Tape Loader") down to 128 BASIC.
fn enter_128_basic(m: &mut dyn Machine) {
    press(m, &[CAPS, KEY6]);
    press(m, &[ENTER]);
    run(m, 50);
}

#[test]
#[ignore = "needs the Spectrum ROMs in EMU_ROM_DIR"]
fn every_model_boots_to_basic_and_prints() {
    for model in ["48k", "128k", "plus2"] {
        let (mut m, font) = machine(model, &[]);
        run(m.as_mut(), 150);
        let s = screen(m.as_mut(), &font);
        let copyright = s
            .iter()
            .any(|r| r.contains("Sinclair") || r.contains("Amstrad"));
        let path = snapshot(m.as_ref(), &format!("spectrum-{model}-boot.png"));
        assert!(
            copyright,
            "{model}: no copyright line, see {path}:\n{}",
            s.join("\n")
        );
        if model == "48k" {
            // K mode: P is PRINT; + is SYMBOL SHIFT + K.
            type_text(m.as_mut(), "p2+2\n");
        } else {
            enter_128_basic(m.as_mut());
            type_text(m.as_mut(), "print 2+2\n");
        }
        run(m.as_mut(), 100);
        let s = screen(m.as_mut(), &font);
        let path = snapshot(m.as_ref(), &format!("spectrum-{model}-print.png"));
        eprintln!("{model}:\n{}", s.join("\n"));
        assert!(
            s.iter().any(|r| r.starts_with("4 ")),
            "{model}: no 4 printed, see {path}"
        );
        assert!(
            s.iter().any(|r| r.contains("0 OK")),
            "{model}: no OK report, see {path}"
        );
    }
}

/// A backend over bytes.
struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// A TAP block: length, flag, data, checksum.
fn tap_block(flag: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vec![flag];
    b.extend_from_slice(data);
    b.push(b.iter().fold(0, |a, x| a ^ x));
    let mut out = (b.len() as u16).to_le_bytes().to_vec();
    out.extend(b);
    out
}

/// `10 PRINT "LOADED"` as a program that runs itself from line 10.
fn program_tap() -> Vec<u8> {
    let mut line = vec![0xf5, b'"'];
    line.extend_from_slice(b"LOADED");
    line.extend_from_slice(&[b'"', 0x0d]);
    let mut prog = vec![0x00, 0x0a];
    prog.extend_from_slice(&(line.len() as u16).to_le_bytes());
    prog.extend(line);
    let mut header = vec![0u8];
    header.extend_from_slice(b"test      ");
    header.extend_from_slice(&(prog.len() as u16).to_le_bytes());
    header.extend_from_slice(&10u16.to_le_bytes());
    header.extend_from_slice(&(prog.len() as u16).to_le_bytes());
    let mut tap = tap_block(0x00, &header);
    tap.extend(tap_block(0xff, &prog));
    tap
}

fn insert_tape(m: &mut dyn Machine) {
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: "test.tap".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Bytes(program_tap())),
        })
        .unwrap();
}

#[test]
#[ignore = "needs the Spectrum ROMs in EMU_ROM_DIR"]
fn a_tap_program_loads_with_and_without_the_trap() {
    for (model, traps, frames) in [
        ("48k", "on", 100),
        ("48k", "off", 600),
        ("128k", "off", 600),
    ] {
        let (mut m, font) = machine(model, &[("traps", traps)]);
        run(m.as_mut(), 150);
        insert_tape(m.as_mut());
        if model == "48k" {
            // K mode: J is LOAD; " is SYMBOL SHIFT + P.
            type_text(m.as_mut(), "j\"\"\n");
        } else {
            // The menu's Tape Loader.
            press(m.as_mut(), &[ENTER]);
        }
        run(m.as_mut(), frames);
        let s = screen(m.as_mut(), &font);
        let path = snapshot(m.as_ref(), &format!("spectrum-{model}-load-{traps}.png"));
        eprintln!("{model} traps {traps}:\n{}", s.join("\n"));
        assert!(
            s.iter().any(|r| r.starts_with("LOADED")),
            "{model}, traps {traps}: the program did not run, see {path}"
        );
    }
}

/// Load `tap` with LOAD "" (the 128K's Tape Loader), run `frames` and
/// return the screen text.
fn load_and_run(model: &str, tap: Vec<u8>, frames: usize, name: &str) -> Vec<String> {
    let (mut m, font) = machine(model, &[]);
    run(m.as_mut(), 150);
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Tape,
            backend: Box::new(Bytes(tap)),
        })
        .unwrap();
    if model == "48k" {
        type_text(
            m.as_mut(),
            "j\"\"
",
        );
    } else {
        press(m.as_mut(), &[ENTER]);
    }
    run(m.as_mut(), frames);
    let s = screen(m.as_mut(), &font);
    let out = snapshot(m.as_ref(), &format!("spectrum-{name}-{model}.png"));
    eprintln!(
        "{model}:
{}
{out}",
        s.join(
            "
"
        )
    );
    s
}

/// Phase 15 gate, timing: Philip Kendall's Fuse Test (fetched into
/// `EMU_TESTDATA_DIR/spectrum` by `tools/fetch-testdata.sh`) checks the
/// frame length, contended memory and I/O, the floating bus, LDIR at the
/// contended boundary and $7FFD reads; every test must pass (the 48K
/// skips the three 128K ones).
#[test]
#[ignore = "needs the Spectrum ROMs in EMU_ROM_DIR and the Fuse Test in EMU_TESTDATA_DIR"]
fn fuse_test_passes() {
    let dir = std::env::var("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR");
    let tap = std::fs::read(std::path::Path::new(&dir).join("spectrum/fusetest.tap"))
        .expect("spectrum/fusetest.tap (tools/fetch-testdata.sh)");
    for (model, frame) in [("48k", "0x9100"), ("128k", "0x94fc"), ("plus2", "0x94fc")] {
        let s = load_and_run(model, tap.clone(), 2500, "fusetest");
        assert!(
            s.iter()
                .any(|r| r.contains(&format!("Frame length 0x8000 + {frame}"))),
            "{model}: frame length"
        );
        let results: Vec<&String> = s.iter().filter(|r| r.contains("...")).collect();
        assert_eq!(results.len(), 10, "{model}: ten tests reported");
        let passed = results.iter().filter(|r| r.contains("passed")).count();
        let want = if model == "48k" { 7 } else { 10 };
        assert_eq!(passed, want, "{model}: passed");
        assert!(
            !s.iter().any(|r| r.contains("failed")),
            "{model}: a test failed"
        );
    }
}

/// A user's TAP/TZX: `EMU_SPECTRUM_TAP=file` (`EMU_SPECTRUM_MODEL`
/// 48k|128k|plus2, `EMU_FRAMES`, `EMU_OPTION=name=value`): loads it with
/// LOAD "" (the 128K's Tape Loader), prints the screen text and saves the
/// picture.
#[test]
#[ignore = "needs the Spectrum ROMs in EMU_ROM_DIR and EMU_SPECTRUM_TAP"]
fn user_tap() {
    let path = std::env::var("EMU_SPECTRUM_TAP").expect("EMU_SPECTRUM_TAP");
    let model = std::env::var("EMU_SPECTRUM_MODEL").unwrap_or_else(|_| "48k".into());
    let frames = std::env::var("EMU_FRAMES").map_or(1500, |v| v.parse().unwrap());
    let option = std::env::var("EMU_OPTION").unwrap_or_default();
    let options: Vec<(&str, &str)> = option.split_once('=').into_iter().collect();
    let (mut m, font) = machine(&model, &options);
    run(m.as_mut(), 150);
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: path.clone(),
            kind: MediaKind::Tape,
            backend: Box::new(Bytes(std::fs::read(&path).unwrap())),
        })
        .unwrap();
    if model == "48k" {
        type_text(m.as_mut(), "j\"\"\n");
    } else {
        press(m.as_mut(), &[ENTER]);
    }
    run(m.as_mut(), frames);
    let s = screen(m.as_mut(), &font);
    let out = snapshot(m.as_ref(), &format!("spectrum-user-{model}.png"));
    eprintln!("{}\n{out}", s.join("\n"));
}

fn panel(m: &mut dyn Machine, id: u16) {
    use emu_core::input::{InputEvent, InputTarget, TimedInput};
    let at = m.now();
    for (value, delay) in [(1, 0u64), (0, 3_500_000 / 50)] {
        m.input(TimedInput {
            at: emu_core::Cycle(at.0 + delay),
            target: InputTarget::Panel,
            event: InputEvent::Control {
                id: emu_core::ControlId(id),
                value,
            },
        });
    }
}

/// The Multiface 128 (ROM role "mf128"): its button stops the program
/// with the menu, `r` returns to it unharmed.
#[test]
#[ignore = "needs the Spectrum and Multiface 128 ROMs in EMU_ROM_DIR"]
fn multiface_menu_and_return() {
    for model in ["48k", "128k"] {
        let (mut m, font) = machine(model, &[("multiface", "on")]);
        assert!(m.controls().iter().any(|c| c.name == "MULTIFACE"));
        run(m.as_mut(), 150);
        if model != "48k" {
            enter_128_basic(m.as_mut());
        }
        let before = screen(m.as_mut(), &font);
        panel(m.as_mut(), 1);
        run(m.as_mut(), 100);
        let path = snapshot(m.as_ref(), &format!("spectrum-{model}-multiface.png"));
        let menu = screen(m.as_mut(), &font);
        eprintln!("{model} menu:\n{}\n{path}", menu.join("\n"));
        assert_ne!(menu, before, "{model}: no menu, see {path}");
        type_text(m.as_mut(), "r");
        run(m.as_mut(), 100);
        let after = screen(m.as_mut(), &font);
        let path = snapshot(
            m.as_ref(),
            &format!("spectrum-{model}-multiface-return.png"),
        );
        assert_eq!(after, before, "{model}: screen not restored, see {path}");
        // BASIC still works.
        if model == "48k" {
            type_text(m.as_mut(), "p2+2\n");
        } else {
            type_text(m.as_mut(), "print 2+2\n");
        }
        run(m.as_mut(), 100);
        let s = screen(m.as_mut(), &font);
        assert!(
            s.iter().any(|r| r.starts_with("4 ")),
            "{model}: BASIC after return"
        );
    }
}

/// A backend over shared bytes, so a test can look at a cartridge.
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

const SYM: u16 = 36;
/// Key 0 and 9 (row 4, columns 0 and 1): FORMAT and CAT, the red
/// keywords below them, in E mode with SYMBOL SHIFT.
const KEY0: u16 = 20;
const KEY9: u16 = 21;

/// E mode (CAPS + SYMBOL SHIFT), then SYMBOL SHIFT + `key`.
fn extended(m: &mut dyn Machine, key: u16) {
    press(m, &[CAPS, SYM]);
    press(m, &[SYM, key]);
}

/// The Interface 1 (ROM role "if1"): FORMAT a blank cartridge, SAVE a
/// program to it, CAT, NEW and LOAD it back.
#[test]
#[ignore = "needs the Spectrum and Interface 1 ROMs in EMU_ROM_DIR"]
fn interface1_microdrive_format_save_load() {
    let (mut m, font) = machine("48k", &[("if1", "on")]);
    let cart = std::sync::Arc::new(std::sync::Mutex::new(
        media::mdr::Cartridge::blank().to_bytes(),
    ));
    m.media_slot(SlotId(2))
        .unwrap()
        .insert(Media {
            name: "test.mdr".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Shared(cart.clone())),
        })
        .unwrap();
    run(m.as_mut(), 150);
    let show = |m: &mut dyn Machine, what: &str| {
        let s = screen(m, &font);
        let path = snapshot(m, &format!("spectrum-if1-{what}.png"));
        eprintln!("{what}:\n{}\n{path}", s.join("\n"));
        s
    };
    // FORMAT "m";1;"mimic"
    extended(m.as_mut(), KEY0);
    type_all(m.as_mut(), "\"m\";1;\"mimic\"\n");
    run(m.as_mut(), 1500);
    let s = show(m.as_mut(), "format");
    assert!(s.iter().any(|r| r.contains("0 OK")), "FORMAT failed");
    // 10 REM hello, SAVE *"m";1;"prog"
    type_all(m.as_mut(), "10ehello\n");
    run(m.as_mut(), 60);
    type_all(m.as_mut(), "s*\"m\";1;\"prog\"\n");
    run(m.as_mut(), 600);
    let s = show(m.as_mut(), "save");
    assert!(s.iter().any(|r| r.contains("0 OK")), "SAVE failed");
    // CAT 1
    extended(m.as_mut(), KEY9);
    type_all(m.as_mut(), "1\n");
    run(m.as_mut(), 600);
    let s = show(m.as_mut(), "cat");
    assert!(
        s.iter().any(|r| r.trim() == "mimic"),
        "cartridge name in CAT"
    );
    assert!(s.iter().any(|r| r.trim() == "prog"), "file in CAT");
    // NEW, LOAD *"m";1;"prog", LIST
    type_all(m.as_mut(), "a\n");
    run(m.as_mut(), 150);
    type_all(m.as_mut(), "j*\"m\";1;\"prog\"\n");
    run(m.as_mut(), 600);
    type_all(m.as_mut(), "k\n");
    run(m.as_mut(), 60);
    let s = show(m.as_mut(), "load");
    assert!(
        s.iter().any(|r| r.contains("10 REM hello")),
        "program loaded back"
    );
    // Written back to the cartridge: some header carries the name.
    m.media_slot(SlotId(2)).unwrap().flush().unwrap();
    let bytes = cart.lock().unwrap();
    assert!(bytes.windows(5).any(|w| w == b"mimic"));
}
