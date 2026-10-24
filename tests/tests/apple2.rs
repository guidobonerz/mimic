//! Apple II, II+ and IIe: the stub ROM, the real ROMs (user-supplied via
//! `EMU_ROM_DIR`, an `apple2/` or `apple/` subfolder), booting a disk this
//! test builds through the Disk II boot PROM, and the colours a composite
//! monitor makes of the hi-res dot patterns.
//!
//!     cargo test -p emu-tests --release --test apple2
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test apple2 -- --include-ignored --nocapture

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, MemSpaceId, Size, StopReason};

fn stub(model: &str) -> Box<dyn Machine> {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    emu_machines::registry().create("apple2", &config).unwrap()
}

fn with_roms(model: &str, options: &[(&str, &str)], drop: &[&str]) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut roms = media_backends::roms::load(&dir, "apple2").expect("ROMs");
    for r in drop {
        roms.remove(*r);
    }
    let mut config = MachineConfig {
        roms,
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    emu_machines::registry().create("apple2", &config).unwrap()
}

fn run(m: &mut dyn Machine, frames: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

const MAIN: MemSpaceId = MemSpaceId(1);

fn peek(m: &mut dyn Machine, space: MemSpaceId, addr: u32) -> u8 {
    let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
    t.peek(space, addr, Size::Byte).unwrap() as u8
}

/// The text page's row `row` as ASCII (high bits stripped).
fn text_row(m: &mut dyn Machine, row: u32) -> String {
    let base = 0x400 + (row % 8) * 0x80 + (row / 8) * 40;
    (0..40)
        .map(|c| (peek(m, MAIN, base + c) & 0x7f) as char)
        .collect()
}

fn key(m: &mut dyn Machine, code: u16) {
    for down in [true, false] {
        let at = m.now();
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(code),
                down,
            },
        });
        run(m, 2);
    }
}

#[test]
fn stub_rom_prints_and_echoes_keys() {
    for (model, typed) in [("iiplus", 'A'), ("iie", 'a')] {
        let mut m = stub(model);
        run(m.as_mut(), 5);
        assert!(text_row(m.as_mut(), 0).starts_with("MIMIC APPLE II STUB ROM"));
        if model == "iie" {
            key(m.as_mut(), 0x102); // CAPS LOCK off
        }
        key(m.as_mut(), u16::from(b'a'));
        assert_eq!(
            text_row(m.as_mut(), 2).chars().next(),
            Some(typed),
            "{model}"
        );
    }
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub("iie");
    run(a.as_mut(), 3);
    let state = a.save_state();
    run(a.as_mut(), 10);
    let mut b = stub("iie");
    b.load_state(&state).unwrap();
    run(b.as_mut(), 10);
    assert_eq!(a.save_state(), b.save_state());
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
    let mut other = stub("iiplus");
    assert!(other.load_state(&state).is_err(), "another model");
}

/// Hi-res colour bars: green, violet, orange and blue bands, decoded by
/// the composite signal stage of `video-crt` with an NTSC TV profile.
#[test]
fn hires_patterns_decode_to_the_four_colours() {
    let mut m = stub("iiplus");
    run(m.as_mut(), 2);
    // Stop at an instruction boundary (the stub's keyboard loop).
    let mut dbg = DebugHooks::new();
    dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: emu_core::MemSpaceId(0),
        addr: 0xf02c,
    });
    assert!(matches!(
        m.run_frame(&mut dbg),
        StopReason::Breakpoint { .. }
    ));
    // A program in RAM: graphics, full screen, hi-res, then loop.
    let prog: [u8; 12] = [
        0xad, 0x50, 0xc0, 0xad, 0x52, 0xc0, 0xad, 0x57, 0xc0, 0x4c, 0x09, 0x03,
    ];
    let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
    for (i, &b) in prog.iter().enumerate() {
        t.poke(MAIN, 0x300 + i as u32, Size::Byte, u32::from(b));
    }
    // (even column byte, odd column byte) per band of 48 lines.
    let bands = [(0x2a, 0x55), (0x55, 0x2a), (0xaa, 0xd5), (0xd5, 0xaa)];
    for y in 0..192u32 {
        let base = 0x2000 + (y % 8) * 0x400 + ((y / 8) % 8) * 0x80 + (y / 64) * 40;
        let (even, odd) = bands[(y / 48) as usize];
        for c in 0..40 {
            let v = if c % 2 == 0 { even } else { odd };
            t.poke(MAIN, base + c, Size::Byte, v);
        }
    }
    t.set_reg("PC", 0x300);
    run(m.as_mut(), 3);
    let f = m.framebuffer();
    assert_eq!(f.signal.output, emu_core::frame::OutputType::Composite);
    let profile = video_crt::Profile::preset("ntsc-tv").unwrap();
    let rgb = video_crt::signal::process(&f, &profile).expect("composite decoding");
    snapshot(m.as_ref(), "apple2-hires-raw.png");
    let mean = |band: u32| {
        let y = 24 + band * 48 + 24;
        let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
        for x in 400..880 {
            let p = rgb[(y * f.width + x) as usize];
            r += u32::from(p.r);
            g += u32::from(p.g);
            b += u32::from(p.b);
        }
        (r / 480, g / 480, b / 480)
    };
    let (green, violet, orange, blue) = (mean(0), mean(1), mean(2), mean(3));
    eprintln!("green {green:?} violet {violet:?} orange {orange:?} blue {blue:?}");
    assert!(green.1 > green.0 && green.1 > green.2, "green {green:?}");
    assert!(
        violet.0 > violet.1 && violet.2 > violet.1,
        "violet {violet:?}"
    );
    assert!(
        orange.0 > orange.1 && orange.0 > orange.2,
        "orange {orange:?}"
    );
    assert!(blue.2 > blue.0 && blue.2 > blue.1, "blue {blue:?}");
}

#[test]
#[ignore = "needs the Apple II ROMs in EMU_ROM_DIR"]
fn roms_boot_to_basic() {
    for (model, title, prompt) in [
        ("iiplus", "APPLE ][", ']'),
        ("iie-unenhanced", "Apple ][", ']'),
        ("iie", "Apple //e", ']'),
        ("ii", "", '*'),
    ] {
        let mut m = with_roms(model, &[("slot6", "none")], &[]);
        run(m.as_mut(), 200);
        let path = snapshot(m.as_ref(), &format!("apple2-{model}.png"));
        let rows: Vec<String> = (0..24).map(|r| text_row(m.as_mut(), r)).collect();
        if !title.is_empty() {
            assert!(
                rows[0].contains(title),
                "{model}: {:?}, see {path}",
                rows[0]
            );
        }
        assert!(
            rows.iter().any(|r| r.starts_with(prompt)),
            "{model}: no '{prompt}' prompt, see {path}"
        );
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
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        if self.0.len() < o + data.len() {
            self.0.resize(o + data.len(), 0);
        }
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// A disk whose boot sector (track 0, sector 0) prints "BOOT OK".
fn boot_disk() -> Vec<u8> {
    let mut img = vec![0u8; 143_360];
    let code: [u8; 16] = [
        0x01, // sectors for the boot PROM to load
        0xa2, 0x00, //       0801 LDX #0
        0xbd, 0x10, 0x08, // 0803 LDA msg,X
        0xf0, 0x06, //       0806 BEQ 080E
        0x9d, 0x00, 0x04, // 0808 STA $0400,X
        0xe8, //             080B INX
        0xd0, 0xf5, //       080C BNE 0803
        0xf0, 0xfe, //       080E BEQ 080E
    ];
    img[..16].copy_from_slice(&code);
    for (i, c) in b"BOOT OK".iter().enumerate() {
        img[16 + i] = c | 0x80;
    }
    img
}

#[test]
#[ignore = "needs the Apple II ROMs in EMU_ROM_DIR"]
fn boot_prom_loads_a_disk() {
    // With the P6 sequencer PROM and with the functional sequencer.
    for (name, drop) in [("p6", &[][..]), ("functional", &["disk2-p6"][..])] {
        for model in ["iiplus", "iie"] {
            let mut m = with_roms(model, &[], drop);
            m.media_slot(SlotId(0))
                .unwrap()
                .insert(Media {
                    name: "boot.dsk".into(),
                    kind: MediaKind::Disk,
                    backend: Box::new(Bytes(boot_disk())),
                })
                .unwrap();
            run(m.as_mut(), 300);
            let path = snapshot(m.as_ref(), &format!("apple2-boot-{model}-{name}.png"));
            let row = text_row(m.as_mut(), 0);
            assert!(
                row.starts_with("BOOT OK"),
                "{model}/{name}: {row:?}, see {path}"
            );
        }
    }
}

/// `EMU_APPLE2_DISK=path` (and optionally `EMU_APPLE2_MODEL`, default
/// "iie"; `EMU_FRAMES`, default 900): boots a user-supplied disk and saves
/// a picture (raw and through an NTSC TV) in the test's temporary folder.
#[test]
#[ignore = "needs the Apple II ROMs in EMU_ROM_DIR and EMU_APPLE2_DISK"]
fn user_disk_boots() {
    let Ok(path) = std::env::var("EMU_APPLE2_DISK") else {
        eprintln!("EMU_APPLE2_DISK not set: nothing to boot");
        return;
    };
    let model = std::env::var("EMU_APPLE2_MODEL").unwrap_or_else(|_| "iie".into());
    let frames = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let mut m = with_roms(&model, &[], &[]);
    m.media_slot(SlotId(0))
        .unwrap()
        .insert(Media {
            name: path.clone(),
            kind: MediaKind::Disk,
            backend: Box::new(Bytes(std::fs::read(&path).unwrap())),
        })
        .unwrap();
    run(m.as_mut(), frames);
    for r in 0..24 {
        let base = 0x400 + (r % 8) * 0x80 + (r / 8) * 40;
        let bytes: Vec<String> = (0..12)
            .map(|c| format!("{:02x}", peek(m.as_mut(), MAIN, base + c)))
            .collect();
        eprintln!("{r:2} {:40} {}", text_row(m.as_mut(), r), bytes.join(" "));
    }
    let f = m.framebuffer();
    let profile = video_crt::Profile::preset("ntsc-tv").unwrap();
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(
        out.join(format!("apple2-disk-{model}-tv.png")),
        emu_preview::encode_png_crt(&f, &profile, 1),
    )
    .unwrap();
    eprintln!(
        "{}",
        snapshot(m.as_ref(), &format!("apple2-disk-{model}.png"))
    );
}

fn type_text(m: &mut dyn Machine, text: &str) {
    for c in text.bytes() {
        let code = match c {
            b'\n' => 0x0d,
            c => u16::from(c.to_ascii_lowercase()),
        };
        key(m, code);
    }
}

/// The monitor writes memory to tape (`W`) and reads it back (`R`)
/// through the cassette port.
#[test]
#[ignore = "needs the Apple II ROMs in EMU_ROM_DIR"]
fn monitor_saves_and_loads_through_the_cassette() {
    let mut m = with_roms("iiplus", &[("slot6", "none")], &[]);
    run(m.as_mut(), 120);
    type_text(m.as_mut(), "CALL -151\n");
    let pattern = |i: u32| (i * 7 + 3) as u8;
    {
        let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
        for i in 0..256 {
            t.poke(MAIN, 0x800 + i, Size::Byte, u32::from(pattern(i)));
        }
    }
    let tape = SlotId(2);
    m.media_slot(tape)
        .unwrap()
        .insert(Media {
            name: "blank.wav".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Bytes(Vec::new())),
        })
        .unwrap();
    m.media_slot(tape).unwrap().control("record").unwrap();
    type_text(m.as_mut(), "800.8FFW\n");
    // A 10 s header and 256 bytes take about 12 s.
    run(m.as_mut(), 900);
    m.media_slot(tape).unwrap().control("stop").unwrap();
    {
        let t = m.debug_target(emu_core::DeviceId("cpu")).unwrap();
        for i in 0..256 {
            t.poke(MAIN, 0x800 + i, Size::Byte, 0);
        }
    }
    m.media_slot(tape).unwrap().control("rewind").unwrap();
    m.media_slot(tape).unwrap().control("play").unwrap();
    type_text(m.as_mut(), "800.8FFR\n");
    run(m.as_mut(), 1500);
    let path = snapshot(m.as_ref(), "apple2-cassette.png");
    let rows: Vec<String> = (0..24).map(|r| text_row(m.as_mut(), r)).collect();
    assert!(
        !rows.iter().any(|r| r.contains("ERR")),
        "{rows:#?}, see {path}"
    );
    for i in 0..256 {
        assert_eq!(
            peek(m.as_mut(), MAIN, 0x800 + i),
            pattern(i),
            "byte {i}, see {path}"
        );
    }
}
