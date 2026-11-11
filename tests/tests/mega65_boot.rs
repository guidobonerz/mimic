//! The MEGA65 with the user's ROM (`~/.mimic/roms/mega65/mega65.rom` or
//! `EMU_ROM_DIR`): the Open ROMs (LGPL-3), later the official 920xxx ROM.
//!
//!     cargo test -p emu-tests --release --test mega65_boot -- --ignored

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Machine, MemSpaceId, Size};

fn mega65() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "mega65").expect("ROMs"),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("mega65", &config).unwrap()
}

fn run(m: &mut dyn Machine, frames: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
}

const PHYS: MemSpaceId = MemSpaceId(1);

fn phys(m: &mut dyn Machine, a: u32, n: u32) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0..n)
        .map(|i| cpu.peek(PHYS, a + i, Size::Byte).unwrap() as u8)
        .collect()
}

/// The screen's text (screen codes) at the VIC-IV's screen pointer
/// (`$D060`–`$D062`), `$D058` bytes a row.
fn screen(m: &mut dyn Machine) -> String {
    let regs = phys(m, 0x0ffd_3058, 0x0b);
    let step = u32::from(regs[0]) | u32::from(regs[1]) << 8;
    let base = u32::from(regs[8]) | u32::from(regs[9]) << 8 | u32::from(regs[10]) << 16;
    let mut out = String::new();
    for row in 0..50 {
        for &c in &phys(m, base + row * step, step.min(80)) {
            let c = c & 0x7f;
            out.push(match c {
                0 => '@',
                1..=26 => (b'A' + c - 1) as char,
                0x20..=0x3f => c as char,
                _ => ' ',
            });
        }
        out.push('\n');
    }
    out
}

/// The Open ROMs (release 2021-08-24) start in direct start: their banner,
/// the system summary and READY. on the 80 x 50 screen.
#[test]
#[ignore = "needs the MEGA65 ROM (mega65/mega65.rom)"]
fn open_roms_boot_to_ready() {
    let mut m = mega65();
    let m = m.as_mut();
    run(m, 200);
    let t = screen(m);
    assert!(t.contains("OPEN ROMS BASIC & KERNAL"), "{t}");
    assert!(t.contains("BASIC BYTES FREE"), "{t}");
    assert!(t.contains("VIDEO    : 80X50 PAL"), "{t}");
    assert!(t.contains("READY."), "{t}");
    assert!(
        t.contains("BOARD    : MEGA65 R3"),
        "the model ID at $D629: {t}"
    );
}

/// FNV-1a over the picture's RGB bytes.
fn picture_hash(m: &dyn Machine) -> u64 {
    let f = m.framebuffer();
    let emu_core::frame::PixelData::Rgb(pixels) = f.pixels else {
        panic!("RGB frame")
    };
    pixels
        .iter()
        .flat_map(|p| [p.r, p.g, p.b])
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
}

/// The Open ROMs' start screen against its reference picture (checked by
/// eye: the MEGA logo with its colour bars, the banner, the system
/// summary and READY. with the cursor, white on blue, in the 640 × 400
/// area of the 720 × 576 frame; then fixed by its hash).
/// `EMU_SNAPSHOT_DIR` saves it as `mega65-open-roms.png`.
#[test]
#[ignore = "needs the MEGA65 ROM (mega65/mega65.rom)"]
fn open_roms_start_screen_matches_its_reference() {
    let mut m = mega65();
    let m = m.as_mut();
    run(m, 200);
    let hash = picture_hash(m);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let png = emu_preview::encode_png(&m.framebuffer());
        std::fs::write(std::path::Path::new(&d).join("mega65-open-roms.png"), png).unwrap();
        eprintln!("picture hash {hash:016x}");
    }
    assert_eq!(hash, OPEN_ROMS_REFERENCE, "picture hash {hash:016x}");
}

/// The reference picture's hash (2026-11-11).
const OPEN_ROMS_REFERENCE: u64 = 0x3b44_55ea_502b_a2f5;

/// The C65 matrix keys (column × 8 + row) that type `c`, with SHIFT first
/// where needed.
fn keys(c: char) -> Vec<u16> {
    const LSHIFT: u16 = 15;
    let k = |col: u16, row: u16| col * 8 + row;
    let letters = [
        (1, 2),
        (3, 4),
        (2, 4),
        (2, 2),
        (1, 6),
        (2, 5),
        (3, 2),
        (3, 5),
        (4, 1),
        (4, 2),
        (4, 5),
        (5, 2),
        (4, 4),
        (4, 7),
        (4, 6),
        (5, 1),
        (7, 6),
        (2, 1),
        (1, 5),
        (2, 6),
        (3, 6),
        (3, 7),
        (1, 1),
        (2, 7),
        (3, 1),
        (1, 4),
    ];
    let digits = [
        (4, 3),
        (7, 0),
        (7, 3),
        (1, 0),
        (1, 3),
        (2, 0),
        (2, 3),
        (3, 0),
        (3, 3),
        (4, 0),
    ];
    match c {
        'A'..='Z' => {
            let (col, row) = letters[(c as u8 - b'A') as usize];
            vec![k(col, row)]
        }
        '0'..='9' => {
            let (col, row) = digits[(c as u8 - b'0') as usize];
            vec![k(col, row)]
        }
        ' ' => vec![k(7, 4)],
        '\n' => vec![k(0, 1)],
        '+' => vec![k(5, 0)],
        '-' => vec![k(5, 3)],
        '*' => vec![k(6, 1)],
        '=' => vec![k(6, 5)],
        ':' => vec![k(5, 5)],
        ';' => vec![k(6, 2)],
        ',' => vec![k(5, 7)],
        '.' => vec![k(5, 4)],
        '/' => vec![k(6, 7)],
        '"' => vec![LSHIFT, k(7, 3)],
        '$' => vec![LSHIFT, k(1, 3)],
        '(' => vec![LSHIFT, k(3, 3)],
        ')' => vec![LSHIFT, k(4, 0)],
        _ => panic!("no key for {c:?}"),
    }
}

fn type_text(m: &mut dyn Machine, text: &str) {
    for c in text.chars() {
        let ks = keys(c);
        for (down, list) in [
            (true, ks.clone()),
            (false, ks.iter().rev().copied().collect()),
        ] {
            let now = m.now().0;
            for k in list {
                m.input(TimedInput {
                    at: emu_core::Cycle(now),
                    target: InputTarget::Keyboard,
                    event: InputEvent::Key {
                        machine_key: MachineKey(k),
                        down,
                    },
                });
            }
            run(m, 3);
        }
    }
}

/// Typing through the keyboard matrix ($D614/$D613, the Open ROMs' own
/// keyboard handler): a program line, LIST and RUN. (The Open ROMs of
/// 2021-08-24 answer numbers, `PRINT 3` or `A=1`, with their own
/// "?NOT IMPLEMENTED ERROR"; strings work.)
#[test]
#[ignore = "needs the MEGA65 ROM (mega65/mega65.rom)"]
fn open_roms_type_list_and_run() {
    let mut m = mega65();
    let m = m.as_mut();
    run(m, 200);
    type_text(m, "PRINT \"HI\"\n");
    run(m, 10);
    type_text(m, "10 PRINT \"MIMIC\"\n");
    type_text(m, "LIST\n");
    run(m, 10);
    type_text(m, "RUN\n");
    run(m, 20);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let png = emu_preview::encode_png(&m.framebuffer());
        std::fs::write(std::path::Path::new(&d).join("mega65-typed.png"), png).unwrap();
    }
    let t = screen(m);
    let lines: Vec<&str> = t.lines().map(str::trim_end).collect();
    assert!(lines.contains(&"HI"), "{t}");
    assert_eq!(
        lines.iter().filter(|l| **l == "10 PRINT \"MIMIC\"").count(),
        2,
        "typed and listed: {t}"
    );
    assert!(lines.contains(&"MIMIC"), "{t}");
}

/// A machine with a disk in drive 8.
fn mega65_with_disk(image: Vec<u8>, name: &str) -> Box<dyn Machine> {
    use emu_core::media::{Media, MediaKind};
    let mut m = mega65();
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.name.starts_with("drive 8"))
        .expect("drive 8");
    m.media_slot(slot.id)
        .unwrap()
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Disk,
            backend: Box::new(media_backends::image::MemoryBackend { data: image }),
        })
        .unwrap();
    m
}

/// A D64 with a BASIC program "HELLO": 10 PRINT "IEC OK".
fn hello_d64() -> Vec<u8> {
    let mut d = media::d64::D64::blank(b"MIMIC", *b"65");
    // Load address $2001 (the Open ROMs' BASIC start), one line, the end.
    let line: &[u8] = &[
        0x99, 0x20, 0x22, b'I', b'E', b'C', b' ', b'O', b'K', 0x22, 0x00,
    ];
    let mut prg = vec![0x01, 0x20];
    let next = 0x2001 + 4 + line.len() as u16;
    prg.extend_from_slice(&next.to_le_bytes());
    prg.extend_from_slice(&10u16.to_le_bytes());
    prg.extend_from_slice(line);
    prg.extend_from_slice(&[0, 0]);
    d.write_file(b"HELLO", 2, &prg).unwrap();
    d.to_bytes()
}

/// The serial bus with a 1541 (true drive, its DOS ROM from the ROM
/// folder's `drives`): the directory and a program load and run.
#[test]
#[ignore = "needs the MEGA65 ROM and the 1541 DOS ROM"]
fn open_roms_load_from_a_1541() {
    let mut m = mega65_with_disk(hello_d64(), "hello.d64");
    let m = m.as_mut();
    assert!(m.devices().iter().any(|d| d.0 == "drive8"), "a true 1541");
    run(m, 200);
    type_text(m, "LOAD \"$\",8\n");
    run(m, 600);
    type_text(m, "LIST\n");
    run(m, 30);
    let t = screen(m);
    assert!(t.contains("\"MIMIC "), "{t}");
    assert!(t.contains("\"HELLO\""), "{t}");
    type_text(m, "LOAD \"HELLO\",8\n");
    run(m, 600);
    type_text(m, "RUN\n");
    run(m, 30);
    if let Ok(d) = std::env::var("EMU_SNAPSHOT_DIR") {
        let png = emu_preview::encode_png(&m.framebuffer());
        std::fs::write(std::path::Path::new(&d).join("mega65-1541.png"), png).unwrap();
    }
    let t = screen(m);
    assert!(t.lines().any(|l| l.trim_end() == "IEC OK"), "{t}");
}
