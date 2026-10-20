//! A true-emulated 1581 with D81 images (ROMs user-supplied via
//! `EMU_ROM_DIR`): on the C128 (fast serial) it identifies itself, lists,
//! loads and formats; on the C64 (slow serial) it autostarts and saves.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test c128_1581 -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, MemSpaceId, Size};
use media::{Format, D64};
use media_backends::image::MemoryBackend;

fn machine(id: &str, options: &[(&str, &str)]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, id).expect("ROMs"),
        ..MachineConfig::default()
    };
    assert!(config.roms.contains_key("dos1581"), "dos1581 ROM missing");
    config.options.insert("drive8".into(), "1581".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create(id, &config).unwrap()
}

/// The 40-column screen at `$0400` (screen codes).
fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let space = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .map_or(MemSpaceId(0), |s| s.id);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x0400..0x07e8)
        .map(|a| cpu.peek(space, a, Size::Byte).unwrap() as u8)
        .collect()
}

fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn text(m: &mut dyn Machine) -> String {
    screen(m)
        .chunks(40)
        .map(|row| {
            row.iter()
                .map(|&c| match c & 0x7f {
                    c @ 1..=26 => (c + 0x40) as char,
                    c @ 0x20..=0x3f => c as char,
                    _ => ' ',
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How often `text` is on the screen (reverse video counts too).
fn count(m: &mut dyn Machine, text: &str) -> usize {
    let pat = codes(text);
    let s: Vec<u8> = screen(m).iter().map(|c| c & 0x7f).collect();
    s.windows(pat.len()).filter(|w| *w == pat).count()
}

fn run_until(m: &mut dyn Machine, text: &str, n: usize, max: u32) -> Option<u32> {
    let mut dbg = DebugHooks::new();
    (1..=max).find(|_| {
        m.run_frame(&mut dbg);
        count(m, text) >= n
    })
}

fn type_text(m: &mut dyn Machine, c128: bool, text: &str) {
    let (events, end) = if c128 {
        machine_c128::keyboard::typed_events(text, m.now().0)
    } else {
        emu_tests::typing::type_text(text, m.now().0)
    };
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end {
        m.run_frame(&mut dbg);
    }
}

/// Type a command and wait for one more READY.; returns the frames.
fn command(m: &mut dyn Machine, c128: bool, text: &str, max: u32) -> u32 {
    let readies = count(m, "READY.");
    let start = m.now().0;
    type_text(m, c128, text);
    run_until(m, "READY.", readies + 1, max)
        .unwrap_or_else(|| panic!("{text:?} did not finish:\n{}", self::text(m)));
    // Frames of 20 000 cycles or so; report time instead.
    ((m.now().0 - start) / 20_000) as u32
}

fn drive8(m: &dyn Machine) -> SlotId {
    m.media_slots()
        .into_iter()
        .find(|s| s.name == "drive 8")
        .expect("drive 8 present")
        .id
}

fn insert(m: &mut dyn Machine, image: Vec<u8>) {
    let slot = drive8(m);
    let disk = Media {
        name: "test.d81".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend { data: image }),
    };
    m.media_slot(slot).unwrap().insert(disk).unwrap();
}

fn eject(m: &mut dyn Machine) -> D64 {
    let mut dbg = DebugHooks::new();
    for _ in 0..300 {
        m.run_frame(&mut dbg);
    }
    let slot = drive8(m);
    let mut media = m.media_slot(slot).unwrap().eject().expect("disk");
    let mut bytes = vec![0; media.backend.len() as usize];
    media.backend.read(0, &mut bytes).unwrap();
    D64::parse(&bytes).unwrap()
}

/// `10 PRINT "HI"` for the C64 ($0801) or the C128 ($1C01).
fn hello(base: u16) -> Vec<u8> {
    let [lo, hi] = base.to_le_bytes();
    let next = base + 11;
    let [nlo, nhi] = next.to_le_bytes();
    vec![
        lo, hi, nlo, nhi, 0x0a, 0x00, 0x99, 0x20, 0x22, 0x48, 0x49, 0x22, 0x00, 0x00, 0x00,
    ]
}

fn disk(base: u16) -> Vec<u8> {
    let mut d = D64::blank_d81(b"THREE AND A HALF", *b"81");
    d.write_file(b"HELLO", 2, &hello(base)).unwrap();
    let mut big = vec![0x00, 0x20];
    big.extend((0..200 * 254 - 2).map(|i| i as u8));
    d.write_file(b"BIG", 2, &big).unwrap();
    d.to_bytes()
}

#[test]
#[ignore = "needs C128 ROMs and the 1581 DOS ROM in EMU_ROM_DIR"]
fn c128_status_directory_and_run() {
    let mut m = machine("c128", &[]);
    run_until(m.as_mut(), "READY.", 1, 2000).expect("READY.");
    command(m.as_mut(), true, "PRINT DS$\n", 300);
    assert_eq!(
        count(m.as_mut(), "73,COPYRIGHT CBM DOS V10 1581,00,00"),
        1,
        "\n{}",
        text(m.as_mut())
    );
    insert(m.as_mut(), disk(0x1c01));
    command(m.as_mut(), true, "DIRECTORY\n", 2000);
    let shown = text(m.as_mut());
    assert_eq!(
        count(m.as_mut(), "\"THREE AND A HALF\" 81 3D"),
        1,
        "\n{shown}"
    );
    assert_eq!(count(m.as_mut(), "2959 BLOCKS FREE"), 1, "\n{shown}");
    command(m.as_mut(), true, "RUN\"HELLO\"\n", 2000);
    let s = screen(m.as_mut());
    assert!(
        s.chunks(40).any(|row| row.starts_with(&codes("HI "))),
        "\n{}",
        text(m.as_mut())
    );
}

#[test]
#[ignore = "needs C128 ROMs and the 1581 DOS ROM in EMU_ROM_DIR"]
fn c128_fast_load_of_200_blocks() {
    let mut m = machine("c128", &[]);
    run_until(m.as_mut(), "READY.", 1, 2000).expect("READY.");
    insert(m.as_mut(), disk(0x1c01));
    let frames = command(m.as_mut(), true, "BLOAD\"BIG\",B0,P8192\n", 8000);
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let loaded: Vec<u8> = (0x2000..0x2010)
        .map(|a| cpu.peek(machine_c128::RAM_SPACE, a, Size::Byte).unwrap() as u8)
        .collect();
    assert_eq!(loaded, (0..16).collect::<Vec<u8>>());
    eprintln!("1581 on the C128: 200 blocks in about {frames} frames");
    assert!(frames < 1500, "{frames} frames: no fast serial?");
}

#[test]
#[ignore = "needs C128 ROMs and the 1581 DOS ROM in EMU_ROM_DIR"]
fn c128_formats_a_d81() {
    let mut m = machine("c128", &[]);
    run_until(m.as_mut(), "READY.", 1, 2000).expect("READY.");
    insert(m.as_mut(), vec![0; 819_200]);
    command(
        m.as_mut(),
        true,
        "OPEN 15,8,15,\"N0:FRESH,F8\":CLOSE 15\n",
        30_000,
    );
    command(m.as_mut(), true, "PRINT DS$\n", 300);
    assert_eq!(
        count(m.as_mut(), "00, OK,00,00"),
        1,
        "\n{}",
        text(m.as_mut())
    );
    let d = eject(m.as_mut());
    assert_eq!(d.format, Format::D81);
    assert_eq!(d.id(), *b"F8");
    assert_eq!(&d.sector(40, 0).unwrap()[4..9], b"FRESH");
    assert_eq!(d.blocks_free(), 3160);
}

#[test]
#[ignore = "needs C64 ROMs and the 1581 DOS ROM in EMU_ROM_DIR"]
fn c64_autostarts_and_saves() {
    let mut m = machine("c64", &[("autostart", "on")]);
    insert(m.as_mut(), disk(0x0801));
    // The program's output: "HI" at the start of a line, then READY.
    let mut dbg = DebugHooks::new();
    let ran = (0..4000).any(|_| {
        m.run_frame(&mut dbg);
        screen(m.as_mut())
            .chunks(40)
            .any(|row| row.starts_with(&codes("HI ")))
    });
    assert!(ran, "autostart:\n{}", text(m.as_mut()));
    for _ in 0..50 {
        m.run_frame(&mut dbg);
    }
    type_text(m.as_mut(), false, "10 PRINT \"SAVED\"\n");
    command(m.as_mut(), false, "SAVE\"NEWFILE\",8\n", 3000);
    let d = eject(m.as_mut());
    let (_, prg) = d.load(b"NEWFILE").unwrap();
    assert_eq!(prg.load, 0x0801);
    assert!(
        prg.data.windows(5).any(|w| w == b"SAVED"),
        "{:02x?}
{}",
        prg.data,
        text(m.as_mut())
    );
}

#[test]
#[ignore = "needs C64 ROMs and the 1581 DOS ROM in EMU_ROM_DIR"]
fn d64_does_not_fit_a_1581() {
    let mut m = machine("c64", &[]);
    let slot = drive8(m.as_ref());
    let disk = Media {
        name: "x.d64".into(),
        kind: MediaKind::Disk,
        backend: Box::new(MemoryBackend {
            data: D64::blank(b"X", *b"XX").to_bytes(),
        }),
    };
    assert!(m.media_slot(slot).unwrap().insert(disk).is_err());
}
