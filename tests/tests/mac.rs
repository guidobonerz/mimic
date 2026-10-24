//! Macintosh 128K, 512K, Plus and SE: the stub ROM, the real ROMs
//! (user-supplied via `EMU_ROM_DIR`, a `mac/` subfolder) reaching the
//! "insert a disk" screen, and booting a disk this test builds (an MFS
//! volume whose boot blocks blacken the screen) through the IWM and the
//! Sony drives — 400K on the 128K/512K, 800K on the Plus and SE.
//!
//!     cargo test -p emu-tests --release --test mac
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --release --test mac -- --include-ignored --nocapture

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};
use media_backends::image::MemoryBackend;

fn machine(model: &str, stub: bool) -> Box<dyn Machine> {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    if stub {
        config.options.insert("roms".into(), "stub".into());
    } else {
        let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
        config.roms = media_backends::roms::load(&dir, "mac").expect("ROMs");
    }
    emu_machines::registry().create("mac", &config).unwrap()
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

/// Share of black pixels in a rectangle of the screen.
fn black(m: &dyn Machine, x0: usize, y0: usize, w: usize, h: usize) -> f64 {
    let f = m.framebuffer();
    let emu_core::frame::PixelData::Indexed { pixels, .. } = f.pixels else {
        panic!("indexed frame expected");
    };
    let width = f.width as usize;
    let mut n = 0;
    for y in y0..y0 + h {
        n += pixels[y * width + x0..y * width + x0 + w]
            .iter()
            .filter(|&&p| p == 1)
            .count();
    }
    n as f64 / (w * h) as f64
}

/// Frames until each model's ROM shows its disk icon (the SE scans the
/// SCSI bus first).
fn boot_frames(model: &str) -> usize {
    if model == "se" {
        2400
    } else {
        900
    }
}

#[test]
fn stub_rom_runs_on_every_model() {
    for model in ["128k", "512k", "plus", "se"] {
        let mut m = machine(model, true);
        run(m.as_mut(), 3);
        // The text line at the top, the rest white.
        assert!(black(m.as_ref(), 32, 16, 144, 7) > 0.15, "{model}");
        assert_eq!(black(m.as_ref(), 0, 100, 512, 200), 0.0, "{model}");
    }
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = machine("plus", true);
    run(a.as_mut(), 2);
    let state = a.save_state();
    run(a.as_mut(), 2);
    let mut b = machine("plus", true);
    b.load_state(&state).unwrap();
    run(b.as_mut(), 2);
    assert_eq!(a.now(), b.now());
    assert_eq!(
        emu_preview::encode_png(&a.framebuffer()),
        emu_preview::encode_png(&b.framebuffer())
    );
}

#[test]
#[ignore = "needs the Macintosh ROMs in EMU_ROM_DIR"]
fn roms_reach_the_disk_icon() {
    for model in ["128k", "512k", "plus", "se"] {
        let mut m = machine(model, false);
        run(m.as_mut(), boot_frames(model));
        let path = snapshot(m.as_ref(), &format!("mac-{model}.png"));
        // The grey desktop pattern is half black; the disk icon in the
        // middle is mostly white.
        let desk = black(m.as_ref(), 16, 260, 64, 64);
        let icon = black(m.as_ref(), 244, 148, 24, 24);
        assert!(
            (0.4..0.6).contains(&desk),
            "{model}: desktop {desk}, see {path}"
        );
        assert!(icon < 0.35, "{model}: no disk icon ({icon}), see {path}");
    }
}

/// An MFS volume of `size` bytes whose boot blocks fill the screen
/// (`ScrnBase`, low memory $824) with black and stop. bbVersion $44xx: the
/// Plus and SE ROMs only run boot code with that flag.
fn boot_disk(size: usize) -> Vec<u8> {
    let mut img = vec![0u8; size];
    img[0..2].copy_from_slice(b"LK");
    img[2..6].copy_from_slice(&[0x60, 0x00, 0x00, 0x86]); // BRA.W $8A
    img[6..8].copy_from_slice(&[0x44, 0x17]);
    #[rustfmt::skip]
    let code: [u8; 18] = [
        0x20, 0x78, 0x08, 0x24, // MOVEA.L $824.W,A0
        0x30, 0x3c, 0x15, 0x5f, // MOVE.W #21888/4-1,D0
        0x72, 0xff,             // MOVEQ #-1,D1
        0x20, 0xc1,             // MOVE.L D1,(A0)+
        0x51, 0xc8, 0xff, 0xfc, // DBRA D0,*-2
        0x60, 0xfe,             // BRA *
    ];
    img[0x8a..0x8a + code.len()].copy_from_slice(&code);
    // Master directory block (block 2): MFS signature, directory at block
    // 4 (12 blocks), allocation blocks of 1 KB from block 16, all free.
    let blocks = (size / 512) as u16;
    let alloc = (blocks - 16) / 2;
    let mut mdb = Vec::new();
    mdb.extend_from_slice(&0xd2d7u16.to_be_bytes());
    mdb.extend_from_slice(&[0; 10]); // dates, attributes
    mdb.extend_from_slice(&0u16.to_be_bytes()); // files
    mdb.extend_from_slice(&4u16.to_be_bytes());
    mdb.extend_from_slice(&12u16.to_be_bytes());
    mdb.extend_from_slice(&alloc.to_be_bytes());
    mdb.extend_from_slice(&1024u32.to_be_bytes());
    mdb.extend_from_slice(&8192u32.to_be_bytes());
    mdb.extend_from_slice(&16u16.to_be_bytes());
    mdb.extend_from_slice(&1u32.to_be_bytes());
    mdb.extend_from_slice(&alloc.to_be_bytes());
    mdb.extend_from_slice(b"\x04Test");
    img[1024..1024 + mdb.len()].copy_from_slice(&mdb);
    img
}

#[test]
#[ignore = "needs the Macintosh ROMs in EMU_ROM_DIR"]
fn roms_boot_a_built_disk() {
    for (model, size) in [
        ("128k", 409_600),
        ("512k", 409_600),
        ("plus", 819_200),
        ("se", 819_200),
    ] {
        let mut m = machine(model, false);
        m.media_slot(SlotId(0))
            .unwrap()
            .insert(Media {
                name: "boot.dsk".into(),
                kind: MediaKind::Disk,
                backend: Box::new(MemoryBackend {
                    data: boot_disk(size),
                }),
            })
            .unwrap();
        run(m.as_mut(), boot_frames(model));
        let path = snapshot(m.as_ref(), &format!("mac-boot-{model}.png"));
        let b = black(m.as_ref(), 0, 0, 512, 340);
        assert!(
            b > 0.99,
            "{model}: the boot code did not run ({b}), see {path}"
        );
    }
}

/// `EMU_MAC_DISK=path` (several separated by `;` go into the next drives
/// and SCSI IDs; `EMU_MAC_MODEL`, default "plus"; `EMU_FRAMES`, default
/// 1800): boots user-supplied disks and saves a picture.
#[test]
#[ignore = "needs the Macintosh ROMs in EMU_ROM_DIR and EMU_MAC_DISK"]
fn user_disk_boots() {
    let Ok(paths) = std::env::var("EMU_MAC_DISK") else {
        eprintln!("EMU_MAC_DISK not set: nothing to boot");
        return;
    };
    let model = std::env::var("EMU_MAC_MODEL").unwrap_or_else(|_| "plus".into());
    let frames = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1800);
    let mut m = machine(&model, false);
    let mut free: Vec<SlotId> = m
        .media_slots()
        .iter()
        .filter(|s| s.kind == MediaKind::Disk)
        .map(|s| s.id)
        .collect();
    for path in paths.split(';') {
        let data = std::fs::read(path).unwrap();
        let at = free
            .iter()
            .position(|&slot| {
                m.media_slot(slot)
                    .unwrap()
                    .insert(Media {
                        name: path.into(),
                        kind: MediaKind::Disk,
                        backend: Box::new(MemoryBackend { data: data.clone() }),
                    })
                    .is_ok()
            })
            .unwrap_or_else(|| panic!("{path}: no drive takes it"));
        free.remove(at);
    }
    run(m.as_mut(), frames);
    eprintln!("{}", snapshot(m.as_ref(), &format!("mac-disk-{model}.png")));
}
