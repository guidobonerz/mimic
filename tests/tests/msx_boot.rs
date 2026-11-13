//! MSX with a BIOS from the ROM folder (`EMU_ROM_DIR`, else `~/.mimic/roms`,
//! subfolder `msx/`): a user-supplied MSX1 BIOS or C-BIOS, the free one
//! (`python tools/fetch_cbios.py`), and optionally cartridges.
//!
//!     cargo test -p emu-tests --release --test msx_boot -- --ignored --nocapture
//!
//! `EMU_MSX_CARTS=/path` runs every `.rom`/`.mx1`/`.bin` there for a few
//! seconds and saves a picture of each in the test's temporary folder
//! (Phase 34 gate: "user-supplied cartridges run"); `EMU_FRAMES=n` the
//! length (default 900 fields).

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};

fn machine(format: Option<&str>) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "msx").expect("ROMs"),
        video_format: format.and_then(emu_core::frame::VideoStandard::parse),
        ..MachineConfig::default()
    };
    emu_machines::registry().create("msx", &config).unwrap()
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

/// Distinct colours in the picture.
fn colours(m: &dyn Machine) -> usize {
    let f = m.framebuffer();
    let mut set = std::collections::BTreeSet::new();
    for y in 0..f.height {
        for x in 0..f.width {
            let c = f.rgb_at(x, y).unwrap();
            set.insert((c.r, c.g, c.b));
        }
    }
    set.len()
}

#[test]
#[ignore = "needs an MSX BIOS or C-BIOS in the ROM folder"]
fn bios_shows_its_screen_without_a_cartridge() {
    for format in [None, Some("ntsc"), Some("pal")] {
        let name = format.unwrap_or("bios");
        let mut m = machine(format);
        run(m.as_mut(), 300);
        let path = snapshot(m.as_ref(), &format!("msx-{name}.png"));
        let n = colours(m.as_ref());
        eprintln!(
            "{name}: {} lines, {n} colours, {path}",
            m.framebuffer().signal.lines_per_field
        );
        // C-BIOS draws its logo and "No cartridge found", a real BIOS its
        // start screen and BASIC: more than the backdrop.
        assert!(n >= 2, "{name}: only the backdrop, see {path}");
    }
}

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// A 16 KB cartridge for page 1 that calls the BIOS: CHGMOD (screen 1),
/// then CHPUT for each character of "MIMIC", then it waits.
fn bios_call_cart() -> Vec<u8> {
    let mut rom = vec![0xffu8; 0x4000];
    rom[..4].copy_from_slice(&[b'A', b'B', 0x10, 0x40]);
    let code: &[u8] = &[
        0x3e, 0x01, // LD A,1
        0xcd, 0x5f, 0x00, // CALL CHGMOD
        0x21, 0x30, 0x40, // LD HL,text
        0x7e, // loop: LD A,(HL)
        0xb7, // OR A
        0x28, 0x06, // JR Z,done
        0xcd, 0xa2, 0x00, // CALL CHPUT
        0x23, // INC HL
        0x18, 0xf6, // JR loop
        0xfb, 0x76, // done: EI ; HALT
        0x18, 0xfc, // JR done
    ];
    rom[0x10..0x10 + code.len()].copy_from_slice(code);
    rom[0x30..0x36].copy_from_slice(b"MIMIC\0");
    rom
}

#[test]
#[ignore = "needs an MSX BIOS or C-BIOS in the ROM folder"]
fn a_cartridge_is_found_and_calls_the_bios() {
    for slot in [0, 1] {
        let mut m = machine(None);
        m.media_slot(SlotId(slot))
            .unwrap()
            .insert(Media {
                kind: MediaKind::Cartridge,
                name: "bioscall.rom".into(),
                backend: Box::new(Bytes(bios_call_cart())),
            })
            .unwrap();
        run(m.as_mut(), 400);
        let path = snapshot(m.as_ref(), &format!("msx-bioscall-{slot}.png"));
        // Screen 1's name table is at $1800 in VRAM: the text is on it.
        let vram = emu_core::MemSpaceId(1);
        let cpu = m.devices()[0];
        let t = m.debug_target(cpu).unwrap();
        // (the BIOS leaves a margin at the left of the 32 columns).
        let row: Vec<u8> = (0..32)
            .map(|i| t.peek(vram, 0x1800 + i, emu_core::Size::Byte).unwrap() as u8)
            .collect();
        assert!(
            row.windows(5).any(|w| w == b"MIMIC"),
            "slot {}: {row:?}, see {path}",
            slot + 1
        );
    }
}

#[test]
#[ignore = "needs an MSX BIOS or C-BIOS in the ROM folder and EMU_MSX_CARTS"]
fn user_cartridges_run() {
    let dir = std::env::var("EMU_MSX_CARTS").expect("EMU_MSX_CARTS not set");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "rom" | "mx1" | "bin"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no cartridges in {dir}");
    let mut failed = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut m = machine(None);
        let bytes = std::fs::read(&path).unwrap();
        let slot = m.media_slot(SlotId(0)).unwrap();
        if let Err(e) = slot.insert(Media {
            kind: MediaKind::Cartridge,
            name: name.clone(),
            backend: Box::new(Bytes(bytes)),
        }) {
            eprintln!("{name}: not loaded: {e}");
            failed.push(name);
            continue;
        }
        run(m.as_mut(), frames);
        let n = colours(m.as_ref());
        let shot = snapshot(m.as_ref(), &format!("msx-{name}.png"));
        eprintln!("{name}: {n} colours, {shot}");
        if n < 2 {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "no picture: {failed:?}");
}

/// `EMU_MSX_ROM=file`: the cartridge runs for `EMU_FRAMES` fields (default
/// 900), then SPACE and the joystick's trigger A are tapped a few times and it
/// runs as long again.
/// Pictures before and after go to the temporary folder; the test reports
/// whether the picture changed and how loud the sound was.
#[test]
#[ignore = "needs an MSX BIOS or C-BIOS in the ROM folder and EMU_MSX_ROM"]
fn a_user_cartridge_reacts_to_space_and_trigger() {
    use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
    let path = std::env::var("EMU_MSX_ROM").expect("EMU_MSX_ROM not set");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let mut m = machine(None);
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "user.rom".into(),
            backend: Box::new(Bytes(std::fs::read(&path).unwrap())),
        })
        .unwrap();
    run(m.as_mut(), frames);
    let before = m.framebuffer().rgb_at(0, 0).map(|_| {
        let f = m.framebuffer();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .map(|(x, y)| f.rgb_at(x, y).unwrap())
            .map(|c| (c.r, c.g, c.b))
            .collect::<Vec<_>>()
    });
    let a = snapshot(m.as_ref(), "msx-user-before.png");
    let mut audio = Vec::new();
    m.audio(&mut audio);
    audio.clear();
    // SPACE is row 8, bit 0.
    for _ in 0..3 {
        for down in [true, false] {
            let at = m.now();
            m.input(TimedInput {
                at,
                target: InputTarget::Keyboard,
                event: InputEvent::Key {
                    machine_key: MachineKey(64),
                    down,
                },
            });
            run(m.as_mut(), 10);
            // Trigger A of the joystick in port 1 with it.
            let at = m.now();
            m.input(TimedInput {
                at,
                target: InputTarget::Port(emu_core::port::PortId(0)),
                event: InputEvent::Digital {
                    dirs: emu_core::input::Dirs::NONE,
                    buttons: u8::from(down),
                },
            });
        }
    }
    run(m.as_mut(), frames);
    let after = {
        let f = m.framebuffer();
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .map(|(x, y)| f.rgb_at(x, y).unwrap())
            .map(|c| (c.r, c.g, c.b))
            .collect::<Vec<_>>()
    };
    let b = snapshot(m.as_ref(), "msx-user-after.png");
    m.audio(&mut audio);
    let (min, max) = audio
        .iter()
        .fold((i16::MAX, i16::MIN), |(a, b), &s| (a.min(s), b.max(s)));
    let n = colours(m.as_ref());
    eprintln!("{path}: {n} colours, sound {min}..{max}, {a}, {b}");
    assert!(n >= 2, "only the backdrop");
    assert_ne!(
        before,
        Some(after),
        "SPACE and the trigger changed nothing, see {a} and {b}"
    );
}

/// The 40 × 24 text screen (SCREEN 0: the name table at VRAM 0).
fn screen_text(m: &mut dyn Machine) -> String {
    let vram = emu_core::MemSpaceId(1);
    let cpu = m.devices()[0];
    let t = m.debug_target(cpu).unwrap();
    (0..40 * 24)
        .map(|i| t.peek(vram, i, emu_core::Size::Byte).unwrap() as u8)
        .map(|b| if b.is_ascii_graphic() { b as char } else { ' ' })
        .collect()
}

/// The matrix key (row × 8 + bit) of a character on the international
/// keyboard, and whether SHIFT is held with it.
fn key_of(c: char) -> (u16, bool) {
    let k = |row: u16, bit: u16| row * 8 + bit;
    match c {
        '0'..='7' => (k(0, c as u16 - '0' as u16), false),
        '8' => (k(1, 0), false),
        '9' => (k(1, 1), false),
        '-' => (k(1, 2), false),
        '"' => (k(2, 0), true),
        ':' => (k(1, 7), true),
        ',' => (k(2, 2), false),
        'A' | 'B' => (k(2, 6 + c as u16 - 'A' as u16), false),
        'C'..='Z' => {
            let n = c as u16 - 'C' as u16;
            (k(3 + n / 8, n % 8), false)
        }
        ' ' => (k(8, 0), false),
        '\n' => (k(7, 7), false),
        other => panic!("no key for {other:?}"),
    }
}

fn type_text(m: &mut dyn Machine, text: &str) {
    use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
    const SHIFT: u16 = 6 * 8;
    let key = |m: &mut dyn Machine, k: u16, down: bool| {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(k),
                down,
            },
        });
    };
    for c in text.chars() {
        let (k, shift) = key_of(c);
        if shift {
            key(m, SHIFT, true);
            run(m, 2);
        }
        key(m, k, true);
        run(m, 4);
        key(m, k, false);
        if shift {
            key(m, SHIFT, false);
        }
        run(m, 4);
    }
}

/// A machine with MSX BASIC at its prompt; `None` with C-BIOS.
fn basic_machine() -> Option<Box<dyn Machine>> {
    let mut m = machine(None);
    run(m.as_mut(), 600);
    // With a disk ROM the machine asks for the date first.
    if screen_text(m.as_mut()).contains("Enter date") {
        type_text(
            m.as_mut(),
            "
",
        );
        run(m.as_mut(), 100);
    }
    if screen_text(m.as_mut()).contains("BASIC") {
        Some(m)
    } else {
        eprintln!("no MSX BASIC in the ROM folder (C-BIOS): skipped");
        None
    }
}

#[test]
#[ignore = "needs a real MSX BIOS with BASIC in the ROM folder"]
fn basic_computes_what_is_typed() {
    let Some(mut m) = basic_machine() else {
        return;
    };
    let text = screen_text(m.as_mut());
    assert!(text.contains("Ok") && text.contains("Bytes free"), "{text}");
    type_text(m.as_mut(), "PRINT 50-8\n");
    run(m.as_mut(), 20);
    let text = screen_text(m.as_mut());
    let path = snapshot(m.as_ref(), "msx-basic.png");
    assert!(text.contains(" 42 "), "see {path}: {text}");
}

/// A file the test can look into after the machine wrote it.
#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        let o = offset as usize;
        buf.copy_from_slice(&d[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        let o = offset as usize;
        d[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.0.lock().unwrap().resize(len as usize, 0);
        Ok(())
    }
}

#[test]
#[ignore = "needs a real MSX BIOS with BASIC in the ROM folder"]
fn csave_writes_a_cas_image_and_cload_reads_it_back() {
    const TAPE: SlotId = SlotId(3);
    let Some(mut m) = basic_machine() else {
        return;
    };
    let file = Shared::default();
    m.media_slot(TAPE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Tape,
            name: "test.cas".into(),
            backend: Box::new(file.clone()),
        })
        .unwrap();
    m.media_slot(TAPE).unwrap().control("record").unwrap();
    type_text(m.as_mut(), "10 PRINT 50-8\nCSAVE\"T\"\n");
    // Two header tones (6.4 s and 1.6 s) and the two blocks.
    run(m.as_mut(), 600);
    assert!(!m.media_slot(TAPE).unwrap().status().active, "motor off");
    m.media_slot(TAPE).unwrap().control("stop").unwrap();
    let blocks = media::msx_cas::blocks(&file.0.lock().unwrap()).unwrap();
    assert_eq!(blocks.len(), 2, "{blocks:x?}");
    let mut header = vec![0xd3u8; 10];
    // Typed without SHIFT or CAPS: the name is in lower case.
    header.extend_from_slice(b"t     ");
    assert_eq!(blocks[0], header);
    eprintln!("CSAVE: {} bytes of program", blocks[1].len());

    type_text(m.as_mut(), "NEW\n");
    m.media_slot(TAPE).unwrap().control("rewind").unwrap();
    m.media_slot(TAPE).unwrap().control("play").unwrap();
    type_text(m.as_mut(), "CLOAD\n");
    run(m.as_mut(), 700);
    let text = screen_text(m.as_mut());
    let path = snapshot(m.as_ref(), "msx-cload.png");
    assert!(text.contains("Found:t"), "see {path}: {text}");
    type_text(m.as_mut(), "RUN\n");
    run(m.as_mut(), 20);
    let text = screen_text(m.as_mut());
    assert!(text.contains(" 42 "), "see {path}: {text}");
}

/// `EMU_MSX_CAS=file`: the cassette goes in, `EMU_MSX_TYPE` is typed
/// (default `RUN"CAS:"`), and the machine runs for `EMU_FRAMES` fields
/// (default 30 000, ten minutes), with a picture after each quarter.
#[test]
#[ignore = "needs a real MSX BIOS with BASIC in the ROM folder and EMU_MSX_CAS"]
fn a_user_cassette_loads() {
    let path = std::env::var("EMU_MSX_CAS").expect("EMU_MSX_CAS not set");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);
    let command = std::env::var("EMU_MSX_TYPE").unwrap_or_else(|_| "RUN\"CAS:\"".into());
    let Some(mut m) = basic_machine() else {
        return;
    };
    m.media_slot(SlotId(3))
        .unwrap()
        .insert(Media {
            kind: MediaKind::Tape,
            name: "user.cas".into(),
            backend: Box::new(Bytes(std::fs::read(&path).unwrap())),
        })
        .unwrap();
    type_text(
        m.as_mut(),
        &format!(
            "{command}
"
        ),
    );
    for quarter in 1..=4 {
        run(m.as_mut(), frames / 4);
        let shot = snapshot(m.as_ref(), &format!("msx-cas-{quarter}.png"));
        let moving = m.media_slot(SlotId(3)).unwrap().status().active;
        eprintln!(
            "{quarter}/4: {} colours, tape {}, {shot}",
            colours(m.as_ref()),
            if moving { "runs" } else { "stands" }
        );
    }
    assert!(colours(m.as_ref()) >= 2, "only the backdrop");
}

/// A blank 720 KB disk in MS-DOS format: boot sector with its parameter
/// block, two FATs of three sectors, 112 directory entries.
fn blank_disk() -> Vec<u8> {
    let mut d = vec![0u8; 737_280];
    d[..3].copy_from_slice(&[0xeb, 0xfe, 0x90]);
    d[3..11].copy_from_slice(b"MIMIC   ");
    d[0x0b..0x0d].copy_from_slice(&512u16.to_le_bytes());
    d[0x0d] = 2; // sectors per cluster
    d[0x0e] = 1; // reserved sectors
    d[0x10] = 2; // FATs
    d[0x11] = 112; // directory entries
    d[0x13..0x15].copy_from_slice(&1440u16.to_le_bytes());
    d[0x15] = 0xf9; // media
    d[0x16] = 3; // sectors per FAT
    d[0x18] = 9; // sectors per track
    d[0x1a] = 2; // sides
    for fat in [512, 4 * 512] {
        d[fat..fat + 3].copy_from_slice(&[0xf9, 0xff, 0xff]);
    }
    d
}

#[test]
#[ignore = "needs a real MSX BIOS and the SVI-738 disk ROM in the ROM folder"]
fn disk_basic_saves_a_program_and_loads_it_back() {
    const DRIVE_A: SlotId = SlotId(4);
    let Some(mut m) = basic_machine() else {
        return;
    };
    let text = screen_text(m.as_mut());
    if !text.contains("Disk BASIC") {
        eprintln!("no disk ROM in the ROM folder: skipped");
        return;
    }
    let file = Shared(std::sync::Arc::new(std::sync::Mutex::new(blank_disk())));
    m.media_slot(DRIVE_A)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Disk,
            name: "test.dsk".into(),
            backend: Box::new(file.clone()),
        })
        .unwrap();
    type_text(
        m.as_mut(),
        "10 PRINT 50-8
SAVE\"T\"
",
    );
    run(m.as_mut(), 400);
    m.media_slot(DRIVE_A).unwrap().flush().unwrap();
    {
        let d = file.0.lock().unwrap();
        // The root directory follows the boot sector and the FATs.
        let entry = &d[7 * 512..7 * 512 + 32];
        assert_eq!(&entry[..8], b"T       ", "{entry:x?}");
        assert_ne!(u32::from_le_bytes(entry[28..32].try_into().unwrap()), 0);
    }
    type_text(
        m.as_mut(),
        "NEW
FILES
",
    );
    run(m.as_mut(), 200);
    type_text(
        m.as_mut(),
        "LOAD\"T\"
",
    );
    run(m.as_mut(), 300);
    type_text(
        m.as_mut(),
        "RUN
",
    );
    run(m.as_mut(), 30);
    let text = screen_text(m.as_mut());
    let path = snapshot(m.as_ref(), "msx-disk.png");
    assert!(text.contains(" 42 "), "see {path}: {text}");
}

/// `EMU_MSX_DSK=file`: the disk goes into drive A before power-on and the
/// machine runs for `EMU_FRAMES` fields (default 1500); the test prints
/// the text screen and saves a picture. With `EMU_MSX_EXPECT=text` the
/// screen must show that text (e.g. `MSX-DOS`).
#[test]
#[ignore = "needs a real MSX BIOS, the SVI-738 disk ROM and EMU_MSX_DSK"]
fn a_user_disk_boots() {
    let path = std::env::var("EMU_MSX_DSK").expect("EMU_MSX_DSK not set");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut m = machine(None);
    let Some(drive) = m.media_slot(SlotId(4)) else {
        eprintln!("no disk ROM in the ROM folder: skipped");
        return;
    };
    drive
        .insert(Media {
            kind: MediaKind::Disk,
            name: "user.dsk".into(),
            backend: Box::new(Bytes(std::fs::read(&path).unwrap())),
        })
        .unwrap();
    run(m.as_mut(), frames);
    let text = screen_text(m.as_mut());
    let shot = snapshot(m.as_ref(), "msx-dsk.png");
    for line in text.as_bytes().chunks(40) {
        let line = String::from_utf8_lossy(line);
        if !line.trim().is_empty() {
            eprintln!("| {}", line.trim_end());
        }
    }
    eprintln!("{} colours, {shot}", colours(m.as_ref()));
    if let Ok(expect) = std::env::var("EMU_MSX_EXPECT") {
        assert!(text.contains(&expect), "no '{expect}' on the screen");
    }
}

fn msx2_machine() -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let mut config = MachineConfig {
        model: Some("msx2".into()),
        roms: media_backends::roms::load(&dir, "msx").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("disk".into(), "off".into());
    emu_machines::registry().create("msx", &config).unwrap()
}

#[test]
#[ignore = "needs an MSX2 BIOS and sub ROM or C-BIOS's in the ROM folder"]
fn msx2_bios_shows_its_screen() {
    let mut m = msx2_machine();
    run(m.as_mut(), 400);
    let path = snapshot(m.as_ref(), "msx2-bios.png");
    let n = colours(m.as_ref());
    let cpu = m.devices()[0];
    let regs = m.debug_target(cpu).unwrap().regs();
    let vdp: Vec<String> = m.chips()[0]
        .regs
        .iter()
        .map(|r| format!("{}={:x}", r.name, r.value))
        .collect();
    eprintln!("{n} colours, {path}\n{regs:?}\n{}", vdp.join(" "));
    assert!(n >= 2, "only the backdrop, see {path}");
}

#[test]
#[ignore = "needs an MSX2 BIOS and sub ROM or C-BIOS's in the ROM folder"]
fn msx2_finds_a_cartridge_that_calls_the_bios() {
    let mut m = msx2_machine();
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "bioscall.rom".into(),
            backend: Box::new(Bytes(bios_call_cart())),
        })
        .unwrap();
    run(m.as_mut(), 400);
    let path = snapshot(m.as_ref(), "msx2-bioscall.png");
    let vram = emu_core::MemSpaceId(1);
    let cpu = m.devices()[0];
    let t = m.debug_target(cpu).unwrap();
    let row: Vec<u8> = (0..32)
        .map(|i| t.peek(vram, 0x1800 + i, emu_core::Size::Byte).unwrap() as u8)
        .collect();
    assert!(row.windows(5).any(|w| w == b"MIMIC"), "{row:?}, see {path}");
}

#[test]
#[ignore = "needs a real MSX2 BIOS in the ROM folder"]
fn msx2_basic_starts() {
    let mut m = msx2_machine();
    run(m.as_mut(), 600);
    let path = snapshot(m.as_ref(), "msx2-basic.png");
    let text = screen_text(m.as_mut());
    let cpu = m.devices()[0];
    eprintln!("{:?}", m.debug_target(cpu).unwrap().regs());
    for c in m.chips() {
        let r: Vec<String> = c
            .regs
            .iter()
            .map(|r| format!("{}={:x}", r.name, r.value))
            .collect();
        eprintln!("{}: {}", c.name, r.join(" "));
    }
    for line in text.as_bytes().chunks(40) {
        eprintln!("|{}|", String::from_utf8_lossy(line));
    }
    assert!(text.contains("Ok"), "no BASIC prompt, see {path}");
}
