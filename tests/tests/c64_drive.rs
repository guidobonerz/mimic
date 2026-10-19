//! Loading from a true-emulated 1541 over the serial bus, with the real
//! KERNAL and DOS (user-supplied via `EMU_ROM_DIR`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test c64_drive -- --ignored

use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};
use media::D64;
use media_backends::image::MemoryBackend;

/// `10 PRINT "HI"` as a tokenised BASIC program at $0801.
const PROGRAM: [u8; 15] = [
    0x01, 0x08, // load address
    0x0c, 0x08, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, // line 10
    0x00, 0x00, // end of program
];

fn c64(autostart: bool) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    assert!(config.roms.contains_key("dos1541"), "1541 DOS ROM missing");
    if autostart {
        config.options.insert("autostart".into(), "on".into());
    }
    emu_machines::registry().create("c64", &config).unwrap()
}

fn disk() -> Media {
    let mut d = D64::blank(b"POLYTRACE", *b"PT");
    d.add_prg(b"HELLO", &PROGRAM).unwrap();
    // Also as a file, for trying the headless runner by hand.
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("hello.d64");
    std::fs::write(out, d.to_bytes()).unwrap();
    Media {
        name: "hello.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: d.to_bytes() }),
    }
}

fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let ram = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .unwrap()
        .id;
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x0400..0x07e8)
        .map(|a| cpu.peek(ram, a, Size::Byte).unwrap() as u8)
        .collect()
}

#[test]
#[ignore = "needs C64 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn load_and_run_from_drive_8() {
    let mut m = c64(true);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id;
    m.media_slot(slot).unwrap().insert(disk()).unwrap();
    assert_eq!(m.devices().len(), 2, "cpu and drive8");

    let mut dbg = DebugHooks::new();
    let mut found = false;
    for _ in 0..3000 {
        m.run_frame(&mut dbg);
        let s = screen(m.as_mut());
        // "HI" at the start of a screen line: the program's output.
        if s.chunks(40).any(|row| row.starts_with(&[0x08, 0x09, 0x20])) {
            found = true;
            break;
        }
    }
    // Let the beam draw the output before the snapshot.
    for _ in 0..5 {
        m.run_frame(&mut dbg);
    }
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("c64_drive.png");
    std::fs::write(&out, png).unwrap();
    assert!(found, "program did not run; snapshot at {}", out.display());
}
