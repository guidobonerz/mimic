//! C128 with real ROMs (user-supplied via `EMU_ROM_DIR`, e.g. a `c128/`
//! subfolder): boot to BASIC 7.0 on the 40- and 80-column screens, typing,
//! GO64 into C64 mode, and loading from true-emulated 1541/1571 drives.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test c128_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, MemSpaceId, Size};
use media::D64;
use media_backends::image::MemoryBackend;

fn boot(options: &[(&str, &str)]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c128").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create("c128", &config).unwrap()
}

fn read(m: &mut dyn Machine, space: MemSpaceId, from: u32, len: u32) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (from..from + len)
        .map(|a| cpu.peek(space, a, Size::Byte).unwrap() as u8)
        .collect()
}

/// The 40-column screen (`$0400` of RAM bank 0) or the 80-column one
/// (VDC RAM from 0).
fn screen(m: &mut dyn Machine, vdc: bool) -> Vec<u8> {
    if vdc {
        read(m, machine_c128::VDC_SPACE, 0, 2000)
    } else {
        read(m, machine_c128::RAM_SPACE, 0x0400, 1000)
    }
}

/// Screen codes of `text` (upper case, digits, space and punctuation).
fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn find(screen: &[u8], text: &str) -> Option<usize> {
    let pat = codes(text);
    screen.windows(pat.len()).rposition(|w| w == pat)
}

/// Run until `text` is on the screen; returns the frames it took.
fn run_until(m: &mut dyn Machine, vdc: bool, text: &str, max: u32) -> Option<u32> {
    let mut dbg = DebugHooks::new();
    for frame in 0..max {
        m.run_frame(&mut dbg);
        if frame % 10 == 0 && find(&screen(m, vdc), text).is_some() {
            return Some(frame);
        }
    }
    None
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_c128::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end + 100_000 {
        m.run_frame(&mut dbg);
    }
}

fn snapshot(m: &dyn Machine, name: &str, screen: &str) -> std::path::PathBuf {
    let id = m
        .screens()
        .into_iter()
        .find(|s| s.name == screen)
        .unwrap()
        .id;
    let png = emu_preview::encode_png(&m.screen(id).unwrap());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    std::fs::write(&out, png).unwrap();
    out
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn c128_boots_on_both_screens_and_runs_basic() {
    for (display, vdc, cols) in [("40", false, 40), ("80", true, 80)] {
        let mut m = boot(&[("display", display)]);
        let frames = run_until(m.as_mut(), vdc, "READY.", 1500);
        let out = snapshot(
            m.as_ref(),
            &format!("c128_{display}"),
            if vdc { "vdc" } else { "vic" },
        );
        assert!(
            frames.is_some(),
            "{display} columns: no READY.; snapshot at {}",
            out.display()
        );
        eprintln!("{display} columns: READY. after {} frames", frames.unwrap());
        let s = screen(m.as_mut(), vdc);
        assert!(find(&s, "122365 BYTES FREE").is_some(), "{display}: banner");

        type_text(m.as_mut(), "PRINT 6*7\n");
        let s = screen(m.as_mut(), vdc);
        let typed = find(&s, "PRINT 6*7").map(|p| p / cols);
        assert!(typed.is_some(), "{display}: PRINT not echoed");
        assert_eq!(
            find(&s, " 42").map(|p| p / cols),
            typed.map(|r| r + 1),
            "{display}"
        );
    }
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn without_a_drive_the_boot_is_quick() {
    let mut m = boot(&[("drive8", "none")]);
    let frames = run_until(m.as_mut(), false, "READY.", 1500).expect("READY.");
    eprintln!("no drive: READY. after {frames} frames");
    assert!(frames < 200, "{frames} frames");
}

#[test]
#[ignore = "needs C128 ROMs (incl. the C64-mode ROMs) in EMU_ROM_DIR"]
fn go64_switches_to_c64_mode() {
    let mut m = boot(&[]);
    assert!(run_until(m.as_mut(), false, "READY.", 1500).is_some());
    type_text(m.as_mut(), "GO64\n");
    assert!(find(&screen(m.as_mut(), false), "ARE YOU SURE?").is_some());
    type_text(m.as_mut(), "Y\n");
    let frames = run_until(m.as_mut(), false, "38911 BASIC BYTES FREE", 500);
    let out = snapshot(m.as_ref(), "c128_go64", "vic");
    assert!(
        frames.is_some(),
        "no C64 banner; snapshot at {}",
        out.display()
    );
    assert!(run_until(m.as_mut(), false, "READY.", 300).is_some());
    type_text(m.as_mut(), "PRINT 6*7\n");
    let s = screen(m.as_mut(), false);
    let typed = find(&s, "PRINT 6*7").map(|p| p / 40);
    assert_eq!(find(&s, " 42").map(|p| p / 40), typed.map(|r| r + 1));
}

#[test]
#[ignore = "needs C128 ROMs (incl. the C64-mode ROMs) in EMU_ROM_DIR"]
fn commodore_key_at_power_on_starts_c64_mode() {
    let mut m = boot(&[("drive8", "none")]);
    let key = |at: u64, down: bool| emu_core::input::TimedInput {
        at: emu_core::Cycle(at),
        target: emu_core::input::InputTarget::Keyboard,
        event: emu_core::input::InputEvent::Key {
            machine_key: machine_c128::keyboard::keys::COMMODORE,
            down,
        },
    };
    m.input(key(0, true));
    m.input(key(3_000_000, false));
    let frames = run_until(m.as_mut(), false, "38911 BASIC BYTES FREE", 300);
    let out = snapshot(m.as_ref(), "c128_c64key", "vic");
    assert!(
        frames.is_some(),
        "no C64 banner; snapshot at {}",
        out.display()
    );
}

/// `10 PRINT "HI"` as a tokenised BASIC 7.0 program at $1C01.
const PROGRAM: [u8; 15] = [
    0x01, 0x1c, // load address
    0x0c, 0x1c, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, // line 10
    0x00, 0x00, // end of program
];

#[test]
#[ignore = "needs C128 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn autostart_loads_and_runs_from_drive_8() {
    autostart_with("1541");
}

#[test]
#[ignore = "needs C128 ROMs and the 1571 DOS ROM in EMU_ROM_DIR"]
fn autostart_from_a_1571() {
    autostart_with("1571");
}

#[test]
#[ignore = "needs C128 ROMs in EMU_ROM_DIR"]
fn autostart_from_the_virtual_drive() {
    autostart_with("virtual");
}

fn autostart_with(drive: &str) {
    let mut m = boot(&[("autostart", "on"), ("drive8", drive)]);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id;
    let mut d = D64::blank(b"POLYTRACE", *b"PT");
    d.add_prg(b"HELLO", &PROGRAM).unwrap();
    let disk = Media {
        name: "hello.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: d.to_bytes() }),
    };
    m.media_slot(slot).unwrap().insert(disk).unwrap();
    let mut dbg = DebugHooks::new();
    let mut found = false;
    for _ in 0..4000 {
        m.run_frame(&mut dbg);
        if screen(m.as_mut(), false)
            .chunks(40)
            .any(|row| row.starts_with(&[0x08, 0x09, 0x20]))
        {
            found = true;
            break;
        }
    }
    let out = snapshot(m.as_ref(), "c128_drive", "vic");
    assert!(found, "program did not run; snapshot at {}", out.display());
}
