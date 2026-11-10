//! The SD2IEC on the C64's serial bus, with the real KERNAL (user-supplied
//! via `EMU_ROM_DIR`): next to a true 1541 as device 9, and as drive 8.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test c64_sd2iec -- --ignored

use emu_core::host::{lock, HostServices, MemFileHost, OpenMode, SharedFiles};
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, Size};
use media::D64;
use media_backends::image::MemoryBackend;

/// `10 PRINT "HI"` as a tokenised BASIC program at $0801.
const PROGRAM: [u8; 15] = [
    0x01, 0x08, 0x0c, 0x08, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, 0x00, 0x00,
];

/// A C64 with the options and a file area holding `/SD2IEC` (the card).
fn c64(options: &[(&str, &str)]) -> (Box<dyn Machine>, SharedFiles) {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let mut card = MemFileHost::new();
    card.insert("/SD2IEC/hello", PROGRAM.to_vec()).unwrap();
    card.insert("/SD2IEC/games/hi2", PROGRAM.to_vec()).unwrap();
    let mut d = D64::blank(b"IMAGE", *b"IM");
    d.add_prg(b"INIMAGE", &PROGRAM).unwrap();
    card.insert("/SD2IEC/disk.d64", d.to_bytes()).unwrap();
    let host = HostServices::new(None, Some(Box::new(card)));
    let files = host.files.clone().unwrap();
    m.attach_host(host);
    (m, files)
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

fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn count(m: &mut dyn Machine, text: &str) -> usize {
    let c = codes(text);
    screen(m)
        .windows(c.len())
        .filter(|w| *w == c.as_slice())
        .count()
}

/// Blank the screen RAM, so a step's output is all that shows.
fn clear(m: &mut dyn Machine) {
    let ram = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .unwrap()
        .id;
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    for a in 0x0400..0x07e8 {
        cpu.poke(ram, a, Size::Byte, 0x20);
    }
}

/// Clear the screen, type `text`, then run until `until` shows up on
/// screen (`times` = 1) or as often as asked.
fn type_until(m: &mut dyn Machine, text: &str, until: &str, times: usize, frames: u32) -> bool {
    clear(m);
    let (events, end) = emu_tests::typing::type_text(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
    let seen = (0..frames).any(|_| {
        m.run_frame(&mut dbg);
        count(m, until) >= times
    });
    // "?FILE NOT FOUND  ERROR" also ends in READY.
    seen && count(m, "ERROR") == 0
}

/// The screen as text (for failure messages).
fn screen_text(m: &mut dyn Machine) -> String {
    screen(m)
        .chunks(40)
        .map(|row| {
            row.iter()
                .map(|&c| match c & 0x7f {
                    c @ 1..=26 => (c + 0x40) as char,
                    c @ 0x20..=0x3f => c as char,
                    _ => '~',
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn boot(m: &mut dyn Machine) {
    let mut dbg = DebugHooks::new();
    for _ in 0..150 {
        m.run_frame(&mut dbg);
    }
}

fn read_file(files: &SharedFiles, path: &str) -> Option<Vec<u8>> {
    let mut fs = lock(files);
    let h = fs
        .open(
            path,
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )
        .ok()?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n @ 1..) = fs.read(h, &mut buf) {
        out.extend_from_slice(&buf[..n]);
    }
    fs.close(h);
    Some(out)
}

#[test]
#[ignore = "needs C64 ROMs and the 1541 DOS ROM in EMU_ROM_DIR"]
fn sd2iec_as_device_9_next_to_a_1541() {
    let (mut m, files) = c64(&[("drive8", "1541"), ("sd2iec", "on")]);
    let m = m.as_mut();
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name.starts_with("drive 8"))
        .unwrap()
        .id;
    let mut d = D64::blank(b"FLOPPY", *b"FL");
    d.add_prg(b"ON1541", &PROGRAM).unwrap();
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            name: "f.d64".into(),
            kind: MediaKind::Disk,
            backend: Box::new(MemoryBackend { data: d.to_bytes() }),
        })
        .unwrap();
    boot(m);
    // From the SD2IEC, then from the 1541 on the same bus.
    assert!(
        type_until(m, "LOAD\"HELLO\",9\n", "READY.", 1, 600),
        "LOAD from 9"
    );
    assert!(type_until(m, "RUN\n", "HI", 1, 50), "program from 9");
    assert!(
        type_until(m, "LOAD\"ON1541\",8\n", "READY.", 1, 3000),
        "LOAD from 8"
    );
    // A subdirectory, then an image mounted by CD.
    assert!(type_until(
        m,
        "OPEN1,9,15,\"CD:GAMES\":CLOSE1\n",
        "READY.",
        1,
        300
    ));
    assert!(
        type_until(m, "LOAD\"HI2\",9\n", "READY.", 1, 600),
        "LOAD in GAMES"
    );
    assert!(type_until(
        m,
        "OPEN1,9,15,\"CD//:DISK.D64\":CLOSE1\n",
        "READY.",
        1,
        300
    ));
    assert!(
        type_until(m, "LOAD\"INIMAGE\",9\n", "READY.", 1, 600),
        "LOAD from image"
    );
    // SAVE into the image, which is written back to the card.
    assert!(type_until(m, "SAVE\"BACK\",9\n", "READY.", 1, 600), "SAVE");
    let image = D64::parse(&read_file(&files, "/SD2IEC/disk.d64").unwrap()).unwrap();
    assert!(image.read_file(b"BACK").is_ok());
}

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn sd2iec_as_drive_8_saves_to_the_card() {
    let (mut m, files) = c64(&[("drive8", "sd2iec")]);
    let m = m.as_mut();
    assert!(m.media_slots().iter().any(|s| s.name == "sd2iec"));
    boot(m);
    assert!(type_until(m, "LOAD\"HELLO\",8\n", "READY.", 1, 600), "LOAD");
    assert!(type_until(m, "SAVE\"COPY\",8\n", "READY.", 1, 600), "SAVE");
    assert_eq!(read_file(&files, "/SD2IEC/copy").unwrap(), PROGRAM);
    // The directory lists the card.
    assert!(type_until(m, "LOAD\"$\",8\n", "READY.", 1, 600));
    assert!(type_until(m, "LIST\n", "BLOCKS FREE", 1, 100));
    assert!(count(m, "GAMES") >= 1);
}

/// The frontends' setup: the card is the host folder mounted as `SD2IEC`
/// (`--sd2iec-dir`, default `./mimic/sd2iec`), and its disk images show
/// up in the directory, upper-case host names as plain letters.
#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn sd2iec_lists_disk_images_of_the_host_folder() {
    let root = std::env::temp_dir().join(format!("mimic-sd2iec-{}", std::process::id()));
    let card = root.join("sd2iec");
    std::fs::create_dir_all(&card).unwrap();
    let mut d = D64::blank(b"IMAGE", *b"IM");
    d.add_prg(b"INIMAGE", &PROGRAM).unwrap();
    std::fs::write(card.join("Game.d64"), d.to_bytes()).unwrap();
    std::fs::write(card.join("TOOL.D64"), d.to_bytes()).unwrap();
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("drive8".into(), "sd2iec".into());
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let mounts = vec![("SD2IEC".to_string(), card.clone())];
    let journal =
        media_backends::host_journal(false, Some(root.join("ultimate")), &mounts).unwrap();
    m.attach_host(journal.services());
    let m = m.as_mut();
    boot(m);
    assert!(type_until(m, "LOAD\"$\",8\n", "READY.", 1, 600), "LOAD $");
    assert!(type_until(m, "LIST\n", "BLOCKS FREE", 1, 100), "LIST");
    let names = (count(m, "GAME.D64"), count(m, "TOOL.D64"));
    // The upper-case name can be typed: mount it and load from it.
    let mounted = type_until(
        m,
        "OPEN1,8,15,\"CD:TOOL.D64\":CLOSE1
",
        "READY.",
        1,
        300,
    ) && type_until(
        m,
        "LOAD\"INIMAGE\",8
",
        "READY.",
        1,
        600,
    );
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(names, (1, 1));
    assert!(mounted, "CD:TOOL.D64 and LOAD from it");
}

/// REL records and a direct access block read from BASIC, with the real
/// KERNAL (OPEN, PRINT#, INPUT#, GET# on the SD2IEC as drive 8).
#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn sd2iec_rel_records_and_block_reads_from_basic() {
    let (mut m, files) = c64(&[("drive8", "sd2iec")]);
    let m = m.as_mut();
    assert!(m.controls().iter().any(|c| c.name == "SD2IEC NEXT"));
    boot(m);
    // INPUT# and GET# need program mode, and RUN closes open files: all of
    // it is one program.
    for line in [
        "10 OPEN15,8,15:OPEN2,8,2,\"DATA,L,\"+CHR$(20)\n",
        "20 PRINT#15,\"P\"CHR$(98)CHR$(3)CHR$(0)CHR$(1):PRINT#2,\"REC THREE\"\n",
        "30 PRINT#15,\"P\"CHR$(98)CHR$(3)CHR$(0)CHR$(1):INPUT#2,A$:PRINTA$\n",
        "40 CLOSE2:CLOSE15\n",
    ] {
        type_until(m, line, line.split(' ').next().unwrap(), 1, 5);
    }
    // Until READY: line 40 closes the file.
    assert!(
        type_until(m, "RUN\n", "READY.", 1, 600) && count(m, "REC THREE") == 1,
        "record read back:\n{}",
        screen_text(m)
    );
    let r00 = read_file(&files, "/SD2IEC/data.r00").unwrap();
    assert_eq!(r00.len(), 26 + 3 * 20);
    assert_eq!(&r00[26 + 40..26 + 50], b"REC THREE\r");
    // U1 on the BAM block of the mounted image: link 18/1, then "A".
    type_until(m, "NEW\n", "READY.", 1, 50);
    for line in [
        "10 OPEN15,8,15,\"CD:DISK.D64\":OPEN2,8,2,\"#\"\n",
        "20 PRINT#15,\"U1:2,0,18,0\":GET#2,A$,B$,C$:PRINTASC(A$)ASC(B$)C$\n",
        "30 CLOSE2:CLOSE15\n",
    ] {
        type_until(m, line, line.split(' ').next().unwrap(), 1, 5);
    }
    assert!(
        type_until(m, "RUN\n", "READY.", 1, 600) && count(m, " 18  1 A") == 1,
        "block read:\n{}",
        screen_text(m)
    );
}
