//! A disk change in drive 8 is noticed by the 1541's DOS: `LOAD"$",8`
//! after an eject and an insert lists the new disk.
//!
//!     EMU_ROM_DIR=$PWD/roms EMU_DISK_A=a.d64 EMU_DISK_B=b.d64 cargo test -p emu-tests --release --test c64_disk_change -- --ignored --nocapture

use emu_core::media::{MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Size, StopReason};

fn frames(m: &mut dyn emu_core::Machine, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn type_text(m: &mut dyn emu_core::Machine, text: &str) {
    let (events, _) = machine_c64::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
}

/// The screen's first 10 lines as text (screen codes A-Z, 0-9, space).
fn screen(m: &mut dyn emu_core::Machine) -> String {
    let t = m.debug_target(machine_c64::CPU).unwrap();
    let mut out = String::new();
    for row in 0..12 {
        for col in 0..40 {
            let c = t
                .peek(machine_c64::CPU_SPACE, 0x400 + row * 40 + col, Size::Byte)
                .unwrap() as u8;
            out.push(match c & 0x7f {
                1..=26 => (b'A' + c - 1) as char,
                0x20..=0x3f => (c & 0x7f) as char,
                _ => '.',
            });
        }
        out.push('\n');
    }
    out
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_DISK_A/EMU_DISK_B"]
fn a_changed_disk_shows_its_own_directory() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let slot = m
        .media_slots()
        .iter()
        .find(|s| s.kind == MediaKind::Disk)
        .unwrap()
        .id;
    let open = |v: &str| {
        let p = std::env::var(v).expect(v);
        media_backends::image::open(std::path::Path::new(&p)).unwrap()
    };
    let gap: usize = std::env::var("EMU_GAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    m.media_slot(slot)
        .unwrap()
        .insert(open("EMU_DISK_A"))
        .unwrap();
    frames(m.as_mut(), 150);
    type_text(m.as_mut(), "load\"$\",8\n");
    frames(m.as_mut(), 600);
    type_text(m.as_mut(), "list\n");
    frames(m.as_mut(), 100);
    let first = screen(m.as_mut());
    println!("disk A:\n{first}");
    type_text(m.as_mut(), "\u{93}");
    frames(m.as_mut(), 20);
    m.media_slot(SlotId(slot.0)).unwrap().eject();
    frames(m.as_mut(), gap);
    m.media_slot(slot)
        .unwrap()
        .insert(open("EMU_DISK_B"))
        .unwrap();
    frames(m.as_mut(), 50);
    type_text(m.as_mut(), "load\"$\",8\n");
    frames(m.as_mut(), 600);
    type_text(m.as_mut(), "list\n");
    frames(m.as_mut(), 100);
    let second = screen(m.as_mut());
    println!("disk B:\n{second}");
}

/// A hash of the picture, to see when it stands still.
fn picture_hash(m: &dyn emu_core::Machine) -> u64 {
    use std::hash::{Hash, Hasher};
    let f = m.framebuffer();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let emu_core::frame::PixelData::Indexed { pixels, .. } = f.pixels {
        pixels.hash(&mut h);
    }
    h.finish()
}

/// A demo spanning disks (Edge of Disgrace): boot EMU_DISK_A with
/// autostart until the picture stands still (the "insert disk" screen),
/// then eject, wait EMU_GAP frames, insert EMU_DISK_B and see whether the
/// picture moves on. Snapshots in target/tmp/disk-change-*.png.
///
///     EMU_ROM_DIR=$PWD/roms EMU_DISK_A=EdgeOfDisgrace_0.d64 EMU_DISK_B=EdgeOfDisgrace_1a.d64 cargo test -p emu-tests --release --test c64_disk_change demo_continues -- --ignored --nocapture
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_DISK_A/EMU_DISK_B"]
fn demo_continues_after_the_disk_change() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR");
    let roms = media_backends::roms::load(&rom_dir, "c64").expect("ROMs");
    let mut config = MachineConfig {
        roms,
        ..MachineConfig::default()
    };
    config.options.insert("autostart".into(), "on".into());
    if let Ok(opts) = std::env::var("EMU_OPTION") {
        for kv in opts.split(',') {
            if let Some((k, v)) = kv.split_once('=') {
                config.options.insert(k.into(), v.into());
            }
        }
    }
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let slot = m
        .media_slots()
        .iter()
        .find(|s| s.kind == MediaKind::Disk)
        .unwrap()
        .id;
    let open = |v: &str| {
        let p = std::env::var(v).expect(v);
        media_backends::image::open(std::path::Path::new(&p)).unwrap()
    };
    let gap: usize = std::env::var("EMU_GAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let max: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20000);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tmp");
    std::fs::create_dir_all(&out).unwrap();
    let snap = |m: &dyn emu_core::Machine, name: &str| {
        std::fs::write(
            out.join(format!("disk-change-{name}.png")),
            emu_preview::encode_png(&m.framebuffer()),
        )
        .unwrap();
    };
    m.media_slot(slot)
        .unwrap()
        .insert(open("EMU_DISK_A"))
        .unwrap();
    // Run until the picture has stood still for 300 frames.
    let (mut last, mut still, mut frame) = (0u64, 0usize, 0usize);
    while frame < max {
        frames(m.as_mut(), 1);
        frame += 1;
        let h = picture_hash(m.as_ref());
        if h == last {
            still += 1;
            if still == 300 && frame > 1000 {
                break;
            }
        } else {
            still = 0;
            last = h;
        }
    }
    println!("picture still since frame {}", frame - still);
    snap(m.as_ref(), "wait");
    assert!(frame < max, "the picture never stood still");
    m.media_slot(slot).unwrap().eject();
    frames(m.as_mut(), gap);
    m.media_slot(slot)
        .unwrap()
        .insert(open("EMU_DISK_B"))
        .unwrap();
    let mut moved = None;
    for i in 0..2000 {
        frames(m.as_mut(), 1);
        if picture_hash(m.as_ref()) != last {
            moved = Some(i);
            break;
        }
    }
    frames(m.as_mut(), 200);
    snap(m.as_ref(), "after");
    println!("picture moved after {moved:?} frames");
    assert!(moved.is_some(), "the demo did not notice the new disk");
}
