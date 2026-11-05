//! C16 and Plus/4 boot to BASIC with real ROMs (user-supplied via
//! `EMU_ROM_DIR`, e.g. a `plus4/` subfolder), take keyboard input and load
//! from a true-emulated 1541 and from a 1551 on the TCBM bus.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test ted_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};
use media::D64;
use media_backends::image::MemoryBackend;

fn boot(machine: &str, model: &str, autostart: bool) -> Box<dyn Machine> {
    boot_with(machine, model, autostart, "1541")
}

fn boot_with(machine: &str, model: &str, autostart: bool, drive: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, machine).expect("ROMs"),
        video_format: emu_core::frame::VideoStandard::parse(model),
        ..MachineConfig::default()
    };
    if autostart {
        config.options.insert("autostart".into(), "on".into());
    }
    if config.roms.contains_key(&format!("dos{drive}")) || drive == "virtual" {
        config.options.insert("drive8".into(), drive.into());
    }
    emu_machines::registry().create(machine, &config).unwrap()
}

fn run_frames(m: &mut dyn Machine, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

/// The 40 × 25 text screen at `$0C00`.
fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x0c00..0x0c00 + 1000)
        .map(|a| cpu.peek(machine_ted::CPU_SPACE, a, Size::Byte).unwrap() as u8)
        .collect()
}

/// Screen codes of `text` (upper case, digits, space and punctuation).
fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn row_of(screen: &[u8], text: &str) -> Option<usize> {
    let pat = codes(text);
    screen
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| p / 40)
}

fn snapshot(m: &dyn Machine, name: &str) -> std::path::PathBuf {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    std::fs::write(&out, emu_preview::encode_png(&m.framebuffer())).unwrap();
    out
}

#[test]
#[ignore = "needs C16/Plus/4 ROMs in EMU_ROM_DIR"]
fn c16_and_plus4_boot_and_run_basic() {
    for (machine, model, free) in [
        ("plus4", "pal", "60671 BYTES FREE"),
        ("c16", "pal", "12277 BYTES FREE"),
        ("plus4", "ntsc", "60671 BYTES FREE"),
    ] {
        let mut m = boot(machine, model, false);
        run_frames(m.as_mut(), 150);
        let s = screen(m.as_mut());
        let out = snapshot(m.as_ref(), &format!("{machine}_{model}"));
        let banner = row_of(&s, free);
        assert!(
            banner.is_some(),
            "{machine} {model}: {free}; snapshot at {}",
            out.display()
        );
        let ready = row_of(&s, "READY.");
        assert!(ready > banner, "{machine} {model}: READY.");

        let (events, end) = machine_ted::keyboard::typed_events("PRINT 6*7\n", m.now().0);
        for e in events {
            m.input(e);
        }
        let mut dbg = DebugHooks::new();
        while m.now().0 < end + 200_000 {
            m.run_frame(&mut dbg);
        }
        let s = screen(m.as_mut());
        let typed = row_of(&s, "PRINT 6*7");
        assert!(typed > ready, "{machine} {model}: PRINT not echoed");
        assert_eq!(row_of(&s, " 42"), typed.map(|r| r + 1), "{machine} {model}");
    }
}

/// `10 PRINT "HI"` as a tokenised BASIC program at $1001.
const PROGRAM: [u8; 15] = [
    0x01, 0x10, // load address
    0x0c, 0x10, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, // line 10
    0x00, 0x00, // end of program
];

#[test]
#[ignore = "needs Plus/4 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn plus4_autostart_loads_from_drive_8() {
    autostart_with("1541");
}

#[test]
#[ignore = "needs Plus/4 ROMs and the 1551 DOS ROM in EMU_ROM_DIR"]
fn plus4_autostart_loads_from_a_1551() {
    autostart_with("1551");
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn plus4_autostart_from_the_virtual_drive() {
    autostart_with("virtual");
}

fn autostart_with(drive: &str) {
    let mut m = boot_with("plus4", "pal", true, drive);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name.starts_with("drive 8"))
        .expect("drive 8 present (1541 DOS ROM in roms/drives/)")
        .id;
    let mut d = D64::blank(b"MIMIC", *b"PT");
    d.add_prg(b"HELLO", &PROGRAM).unwrap();
    let disk = Media {
        name: "hello.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: d.to_bytes() }),
    };
    m.media_slot(slot).unwrap().insert(disk).unwrap();
    let mut found = false;
    for _ in 0..3000 {
        run_frames(m.as_mut(), 1);
        let s = screen(m.as_mut());
        if s.chunks(40).any(|row| row.starts_with(&[0x08, 0x09, 0x20])) {
            found = true;
            break;
        }
    }
    let out = snapshot(m.as_ref(), &format!("plus4_drive_{drive}"));
    assert!(found, "program did not run; snapshot at {}", out.display());
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_ted::keyboard::typed_events(text, m.now().0);
    for ev in events {
        m.input(ev);
    }
    // Let the keys go through before anything else is typed.
    while m.now().0 < end {
        run_frames(m, 1);
    }
}

/// Run until `text` shows on the screen (or `frames` pass).
fn wait_for(m: &mut dyn Machine, text: &str, frames: u32) -> bool {
    for _ in 0..frames {
        run_frames(m, 1);
        if row_of(&screen(m), text).is_some() {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "needs Plus/4 ROMs and the 1551 DOS ROM in EMU_ROM_DIR"]
fn plus4_saves_to_a_1551_and_lists_the_directory() {
    let mut m = boot_with("plus4", "pal", false, "1551");
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8 (1551)")
        .expect("1551 as drive 8 (dos1551 ROM in roms/drives/)")
        .id;
    let disk = Media {
        name: "blank.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend {
            data: D64::blank(b"TCBM", *b"T1").to_bytes(),
        }),
    };
    m.media_slot(slot).unwrap().insert(disk).unwrap();
    run_frames(m.as_mut(), 150);
    type_text(m.as_mut(), "10 PRINT\"SAVED\"\r");
    run_frames(m.as_mut(), 20);
    type_text(m.as_mut(), "SAVE\"PROG\",8\r");
    let saved = wait_for(m.as_mut(), "SAVING PROG", 300);
    // READY. after the save (the second READY. on the screen).
    let mut done = false;
    for _ in 0..1500 {
        run_frames(m.as_mut(), 1);
        let s = screen(m.as_mut());
        if s.windows(6).filter(|w| *w == codes("READY.")).count() >= 2 {
            done = true;
            break;
        }
    }
    // SCNCLR first: the SAVE line itself shows "PROG".
    type_text(m.as_mut(), "SCNCLR\r");
    run_frames(m.as_mut(), 30);
    type_text(m.as_mut(), "DIRECTORY\r");
    let listed = wait_for(m.as_mut(), "\"PROG\"", 1500);
    // The picture follows the screen RAM a frame later.
    run_frames(m.as_mut(), 60);
    let out = snapshot(m.as_ref(), "plus4_1551_directory");
    assert!(
        saved && done,
        "SAVE did not finish; snapshot at {}",
        out.display()
    );
    assert!(
        listed,
        "PROG not in the directory; snapshot at {}",
        out.display()
    );
    // The header line is shown reversed, the name padded to 16 characters.
    let header: Vec<u8> = codes("\"TCBM").iter().map(|c| c | 0x80).collect();
    assert!(
        screen(m.as_mut())
            .windows(header.len())
            .any(|w| w == header),
        "disk name; snapshot at {}",
        out.display()
    );
    assert!(
        row_of(&screen(m.as_mut()), "663 BLOCKS FREE").is_some(),
        "one block used; snapshot at {}",
        out.display()
    );
    assert!(m.media_slot(slot).unwrap().dirty(), "the disk was written");
}
