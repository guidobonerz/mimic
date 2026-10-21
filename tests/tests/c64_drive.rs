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
    c64_with(autostart, "1541")
}

/// A C64 with drive 8 as "1541", "1571" (true drives) or "virtual".
fn c64_with(autostart: bool, drive: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    if drive != "virtual" {
        let role = format!("dos{drive}");
        assert!(config.roms.contains_key(&role), "{role} ROM missing");
    }
    config.options.insert("drive8".into(), drive.into());
    if autostart {
        config.options.insert("autostart".into(), "on".into());
    }
    emu_machines::registry().create("c64", &config).unwrap()
}

fn disk() -> Media {
    let mut d = D64::blank(b"MIMIC", *b"PT");
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
    load_and_run_with("1541");
}

/// The 1571 in its 1541 mode (a C64 has no fast serial port).
#[test]
#[ignore = "needs C64 ROMs and the 1571 DOS ROM in EMU_ROM_DIR"]
fn load_and_run_from_a_1571() {
    load_and_run_with("1571");
}

fn load_and_run_with(drive: &str) {
    let mut m = c64_with(true, drive);
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
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("c64_{drive}.png"));
    std::fs::write(&out, png).unwrap();
    assert!(found, "program did not run; snapshot at {}", out.display());
}

/// What the C64 shows at $0400 (screen codes).
fn screen_has(m: &mut dyn Machine, text: &str) -> bool {
    let codes: Vec<u8> = text
        .bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect();
    screen(m).windows(codes.len()).any(|w| w == codes)
}

fn type_and_wait(m: &mut dyn Machine, text: &str, until: &str, frames: u32) -> bool {
    let (events, end) = emu_tests::typing::type_text(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
    (0..frames).any(|_| {
        m.run_frame(&mut dbg);
        screen_has(m, until)
    })
}

#[test]
#[ignore = "needs C64 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn save_writes_the_file_to_the_disk_image() {
    let mut m = c64(false);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id;
    m.media_slot(slot).unwrap().insert(disk()).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..150 {
        m.run_frame(&mut dbg);
    }
    assert!(type_and_wait(
        m.as_mut(),
        "10 PRINT \"SAVED\"\n",
        "READY.",
        10
    ));
    // SAVING ... READY. once the drive is done.
    assert!(
        type_and_wait(m.as_mut(), "SAVE\"NEWFILE\",8\n", "SAVING NEWFILE", 50),
        "SAVE not started"
    );
    let mut done = false;
    for _ in 0..2000 {
        m.run_frame(&mut dbg);
        let s = screen(m.as_mut());
        let ready: Vec<u8> = [0x12, 0x05, 0x01, 0x04, 0x19, 0x2e].to_vec();
        if s.windows(6).filter(|w| *w == ready.as_slice()).count() >= 2 {
            done = true;
            break;
        }
    }
    assert!(done, "SAVE did not finish");
    // Let the drive finish and stop its motor, then eject.
    for _ in 0..300 {
        m.run_frame(&mut dbg);
    }
    let mut media = m.media_slot(slot).unwrap().eject().expect("disk");
    let mut bytes = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut bytes).unwrap();
    let image = D64::parse(&bytes).unwrap();
    let names: Vec<String> = image
        .directory()
        .unwrap()
        .iter()
        .map(|e| String::from_utf8_lossy(&e.name).into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "NEWFILE"), "{names:?}");
    let (_, prg) = image.load(b"NEWFILE").unwrap();
    assert_eq!(prg.load, 0x0801);
    assert!(prg.data.windows(5).any(|w| w == b"SAVED"));
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn virtual_drive_autostart_directory_and_save() {
    let mut m = c64_with(true, "virtual");
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id;
    m.media_slot(slot).unwrap().insert(disk()).unwrap();
    assert_eq!(m.devices().len(), 1, "no drive CPU to debug");
    let mut dbg = DebugHooks::new();
    let mut ran = None;
    for frame in 0..1000 {
        m.run_frame(&mut dbg);
        if screen(m.as_mut())
            .chunks(40)
            .any(|row| row.starts_with(&[0x08, 0x09, 0x20]))
        {
            ran = Some(frame);
            break;
        }
    }
    let ran = ran.expect("program did not run from the virtual drive");
    eprintln!("virtual drive: program ran after {ran} frames");

    assert!(type_and_wait(
        m.as_mut(),
        "LOAD\"$\",8
",
        "READY.",
        300
    ));
    let listed = type_and_wait(
        m.as_mut(),
        "LIST
",
        "BLOCKS FREE.",
        100,
    );
    assert!(listed, "directory");
    assert!(screen_has(m.as_mut(), "\"HELLO\""));

    assert!(type_and_wait(
        m.as_mut(),
        "NEW
",
        "READY.",
        20
    ));
    assert!(type_and_wait(
        m.as_mut(),
        "10 PRINT \"V\"
",
        "READY.",
        20
    ));
    assert!(type_and_wait(
        m.as_mut(),
        "SAVE\"VFILE\",8
",
        "SAVING VFILE",
        100
    ));
    for _ in 0..100 {
        m.run_frame(&mut dbg);
    }
    let mut media = m.media_slot(slot).unwrap().eject().expect("disk");
    let mut bytes = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut bytes).unwrap();
    let image = D64::parse(&bytes).unwrap();
    let (_, prg) = image.load(b"VFILE").expect("saved file");
    assert_eq!(prg.load, 0x0801);
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn host_folder_as_virtual_disk() {
    let dir = std::env::temp_dir().join(format!("mimic-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.prg"), PROGRAM).unwrap();
    let mut m = c64_with(true, "virtual");
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .unwrap()
        .id;
    let media = media_backends::image::open(&dir).unwrap();
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    let ran = (0..1000).any(|_| {
        m.run_frame(&mut dbg);
        screen(m.as_mut())
            .chunks(40)
            .any(|row| row.starts_with(&[0x08, 0x09, 0x20]))
    });
    assert!(ran, "program from the folder did not run");
    assert!(type_and_wait(
        m.as_mut(),
        "SAVE\"COPY\",8\n",
        "SAVING COPY",
        100
    ));
    for _ in 0..200 {
        m.run_frame(&mut dbg);
    }
    let saved = std::fs::read(dir.join("copy.prg")).expect("saved into the folder");
    assert_eq!(&saved[..2], &[0x01, 0x08]);
    std::fs::remove_dir_all(&dir).unwrap();
}
