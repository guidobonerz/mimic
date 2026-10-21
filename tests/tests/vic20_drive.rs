//! Loading from a true-emulated 1541 over the VIC-20's serial bus, with
//! the real KERNAL and DOS (user-supplied via `EMU_ROM_DIR`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test vic20_drive -- --ignored

use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};
use media::D64;
use media_backends::image::MemoryBackend;

/// `10 PRINT "HI"` as a tokenised BASIC program saved on an unexpanded
/// VIC-20 ($1001).
const PROGRAM: [u8; 15] = [
    0x01, 0x10, // load address
    0x0c, 0x10, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, // line 10
    0x00, 0x00, // end of program
];

fn disk() -> Media {
    let mut d = D64::blank(b"MIMIC", *b"PT");
    d.add_prg(b"HELLO", &PROGRAM).unwrap();
    Media {
        name: "hello.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: d.to_bytes() }),
    }
}

/// The text screen at `$1E00` (unexpanded) or `$1000` (8 KB and more).
fn screen(m: &mut dyn Machine, base: u32) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (base..base + 22 * 23)
        .map(|a| cpu.peek(machine_vic20::CPU_SPACE, a, Size::Byte).unwrap() as u8)
        .collect()
}

#[test]
#[ignore = "needs VIC-20 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn autostart_loads_and_runs_from_drive_8() {
    // With 8 KB, BASIC starts at $1201: LOAD"*",8 relocates the program.
    for (ram, base, drive) in [
        ("none", 0x1e00, "1541"),
        ("8k", 0x1000, "1541"),
        ("none", 0x1e00, "virtual"),
        ("8k", 0x1000, "virtual"),
    ] {
        let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
        let mut config = MachineConfig {
            roms: media_backends::roms::load(&dir, "vic20").expect("ROMs"),
            ..MachineConfig::default()
        };
        assert!(config.roms.contains_key("dos1541"), "1541 DOS ROM missing");
        config.options.insert("drive8".into(), drive.into());
        config.options.insert("autostart".into(), "on".into());
        config.options.insert("ram".into(), ram.into());
        let mut m = emu_machines::registry().create("vic20", &config).unwrap();
        let slot = m
            .media_slots()
            .into_iter()
            .find(|s| s.name == "drive 8")
            .expect("drive 8 present")
            .id;
        m.media_slot(slot).unwrap().insert(disk()).unwrap();

        let mut dbg = DebugHooks::new();
        let mut found = false;
        for _ in 0..3000 {
            m.run_frame(&mut dbg);
            let s = screen(m.as_mut(), base);
            // "HI" at the start of a screen line: the program's output.
            if s.chunks(22).any(|row| row.starts_with(&[0x08, 0x09, 0x20])) {
                found = true;
                break;
            }
        }
        let png = emu_preview::encode_png(&m.framebuffer());
        let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vic20_drive.png");
        std::fs::write(&out, png).unwrap();
        assert!(
            found,
            "{ram}: program did not run; snapshot at {}",
            out.display()
        );
    }
}
