//! Lobo MAX-80 (Phase 41) with Lobo's boot ROM (`EMU_ROM_DIR`, a `max80/`
//! subfolder with `max80.e12`, 512 bytes) and disks from
//! `$EMU_TESTDATA_DIR/max80`. `tools/fetch-testdata.sh` fetches the free
//! ones: the lobo_max80 project's diagnostic disk (MIT) for both drive
//! sizes and LDOS 5.1.3 for the MAX-80 from Tim Mann's pages (free to use
//! by its owners' permission). Lobo's CP/M 2.2 (`cpm22_8in.imd`, the
//! 8-inch master from the Internet Archive's `lobo-max80-software`) is the
//! user's to supply; its test passes over a missing disk.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p mimic-tests --release --test max80_boot -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use machine_max80::Max80;
use mimic_core::machine::Machine;
use mimic_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, StopReason};

/// Fields a second (59.97).
const SECOND: usize = 60;

fn machine(options: &[(&str, &str)]) -> Option<Max80> {
    let dir = media_backends::roms::env_dir()?;
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "max80").expect("ROMs"),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Some(Max80::new(&config).expect("machine"))
}

fn run(m: &mut Max80, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// Run until the screen (read by `text`) shows `want`, at most `fields`
/// fields; panics with the screen if it never does.
fn wait_for(m: &mut Max80, text: fn(&Max80) -> String, want: &str, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        m.run_frame(&mut dbg);
        if text(m).contains(want) {
            return;
        }
    }
    panic!("no '{want}' on the screen:\n{}", text(m));
}

fn plain(m: &Max80) -> String {
    m.screen_text()
}

/// The screen with the second style of the diagnostic's font read as the
/// characters it stands for: `$80-$9F` are `$20-$3F` again, `$A0-$BF` the
/// capitals and `$00-$1F` the small letters.
fn styled(m: &Max80) -> String {
    let mut s = String::new();
    for row in m.screen_codes() {
        for c in row {
            let c = match c {
                0x00..=0x1f => c + 0x60,
                0x80..=0xbf => c - 0x60,
                _ => c,
            };
            s.push(if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '.'
            });
        }
        s.push('\n');
    }
    s
}

struct Mem(Arc<Mutex<Vec<u8>>>);

impl MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, off: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        buf.copy_from_slice(&d[off as usize..off as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, off: u64, buf: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        let end = off as usize + buf.len();
        if d.len() < end {
            d.resize(end, 0);
        }
        d[off as usize..end].copy_from_slice(buf);
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.0.lock().unwrap().resize(len as usize, 0);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn testdata(name: &str) -> Option<Vec<u8>> {
    let dir = PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR")?).join("max80");
    std::fs::read(dir.join(name)).ok()
}

fn insert(m: &mut Max80, drive: u8, name: &str, data: Arc<Mutex<Vec<u8>>>) {
    m.media_slot(SlotId(machine_max80::SLOT_DRIVE0.0 + drive))
        .unwrap()
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(data)),
        })
        .unwrap();
}

/// A machine with the image `name` in drive 0; the bytes stay shared for
/// a look at what was written. `None` without the ROM or the disk.
fn booted(name: &str) -> Option<(Max80, Arc<Mutex<Vec<u8>>>)> {
    let mut m = machine(&[])?;
    let Some(bytes) = testdata(name) else {
        println!("{name}: not in EMU_TESTDATA_DIR/max80, passed over");
        return None;
    };
    let data = Arc::new(Mutex::new(bytes));
    insert(&mut m, 0, name, data.clone());
    Some((m, data))
}

/// The diagnostic disk's menu item `key` on the disk `name`, run until
/// its report shows `done`.
fn diagnostic(name: &str, key: &str, done: &str, seconds: usize) -> Option<String> {
    let (mut m, _) = booted(name)?;
    wait_for(&mut m, styled, "Select item to run:", 10 * SECOND);
    m.type_text(key);
    wait_for(&mut m, styled, done, seconds * SECOND);
    run(&mut m, SECOND / 2);
    Some(styled(&m))
}

/// The boot ROM loads the diagnostic from a 5.25-inch and from an 8-inch
/// disk (MFM and FM, the DIP switches following the disk), and its tests
/// of what the emulation must get right pass: the four RAM chunks and
/// their banking, the picture's timing from the CRTC's display enable
/// against the tick, the clock chip.
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR"]
fn the_diagnostic_disk_passes() {
    for (name, device) in [
        ("diag_525.imd", "5.25\" floppy"),
        ("diag.imd", "8\" floppy"),
    ] {
        let Some((mut m, _)) = booted(name) else {
            return;
        };
        wait_for(&mut m, styled, "Select item to run:", 10 * SECOND);
        let s = styled(&m);
        assert!(s.contains("MAX-80 DIAGNOSTIC"), "{s}");
        assert!(s.contains("Detected RAM: 128K"), "{s}");
        assert!(s.contains(&format!("Boot device: {device}")), "{s}");
    }
    let s = diagnostic("diag_525.imd", "6", "Retention, 1 s twice:", 40).unwrap();
    assert!(s.contains("Banking: PASS"), "{s}");
    assert!(s.contains("A1 PASS A2 PASS B1 PASS B2 PASS"), "{s}");
    assert!(s.contains("A2 PASS B1 PASS B2 PASS"), "{s}");
    println!("{s}");

    let s = diagnostic("diag_525.imd", "g", "Line period:", 10).unwrap();
    assert!(s.contains("Lines a frame: 263.9"), "{s}");
    assert!(s.contains("Frame rate: 59.97 Hz"), "{s}");
    assert!(s.contains("Line rate: 15.83 kHz"), "{s}");

    let s = diagnostic("diag_525.imd", "d", "Ticks a second:               6", 10).unwrap();
    assert!(s.contains("Digits in range:              YES"), "{s}");
    assert!(s.contains("Seconds counting:             YES"), "{s}");
    assert!(s.contains("Ticks a second:               61"), "{s}");
    assert!(s.contains("Date, month/day/year:         01/01/84"), "{s}");

    // The wait states as measured on a machine: 1.02 and 2.00 T.
    let s = diagnostic("diag_525.imd", "j", "Plain RAM:", 15).unwrap();
    assert!(
        s.contains("Font RAM, reads:     1.00 to 1.00 T an access"),
        "{s}"
    );
    assert!(
        s.contains("SLORAM, writes:      2.00 to 2.00 T an access"),
        "{s}"
    );
}

/// 64 KB: the diagnostic finds bank B missing.
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR"]
fn the_diagnostic_sees_64k() {
    let Some(mut m) = machine(&[("ram", "64k")]) else {
        return;
    };
    let Some(bytes) = testdata("diag_525.imd") else {
        return;
    };
    insert(&mut m, 0, "diag_525.imd", Arc::new(Mutex::new(bytes)));
    wait_for(&mut m, styled, "Detected RAM: 64K", 10 * SECOND);
}

/// LDOS 5.1.3 for the MAX-80 boots from its 5.25-inch disk (80 tracks,
/// two sides, double density, a DMK image) to `LDOS Ready` in the 64 × 16
/// picture with the block at `$3000`, takes typed commands and loads
/// their programs from the disk.
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR"]
fn ldos_boots_and_runs_commands() {
    let Some((mut m, _)) = booted("ldm-513.dsk") else {
        return;
    };
    wait_for(&mut m, plain, "LDOS Ready", 60 * SECOND);
    let s = plain(&m);
    assert!(s.contains("Ver 5.1.3M"), "{s}");
    assert_eq!(m.screen_codes().len(), 16);
    assert_eq!(m.screen_codes()[0].len(), 64);
    assert_eq!(m.io().vlatch >> 4, 3, "the block as a TRS-80's memory map");
    m.type_text("dir :0\n");
    wait_for(&mut m, plain, "Free space=", 30 * SECOND);
    wait_for(&mut m, plain, "SETTIME/CMD", 30 * SECOND);
    // The prompt again, below the listing.
    let mut dbg = DebugHooks::new();
    for _ in 0..30 * SECOND {
        m.run_frame(&mut dbg);
        if plain(&m).matches("LDOS Ready").count() == 2 {
            break;
        }
    }
    run(&mut m, SECOND);
    m.type_text("free\n");
    wait_for(&mut m, plain, "Files=", 30 * SECOND);
    println!("{}", plain(&m));
}

/// Lobo's CP/M 2.2 boots from its 8-inch master (the block at `$F000`),
/// runs STAT, and PIP copies a file: the copy is in the directory, in the
/// image written back, and a second machine boots that image and finds
/// it.
#[test]
#[ignore = "needs EMU_ROM_DIR and the user's CP/M disk in EMU_TESTDATA_DIR"]
fn cpm_boots_runs_programs_and_writes_the_disk() {
    let Some((mut m, data)) = booted("cpm22_8in.imd") else {
        return;
    };
    let before = data.lock().unwrap().clone();
    wait_for(&mut m, plain, "A>", 30 * SECOND);
    let s = plain(&m);
    assert!(s.contains("MAX-80 60K CP/M VERS 2.20"), "{s}");
    assert_eq!(m.screen_codes().len(), 24);
    assert_eq!(m.screen_codes()[0].len(), 80);
    assert_eq!(m.io().vlatch >> 4, 15);
    run(&mut m, SECOND);
    m.type_text("stat\n");
    wait_for(&mut m, plain, "A: R/W, Space:", 20 * SECOND);
    run(&mut m, 2 * SECOND);
    m.type_text("pip x.com=stat.com\n");
    run(&mut m, 25 * SECOND);
    m.type_text("dir\n");
    wait_for(&mut m, plain, "X        COM", 20 * SECOND);
    println!("{}", plain(&m));

    m.media_slot(machine_max80::SLOT_DRIVE0)
        .unwrap()
        .flush()
        .unwrap();
    let after = data.lock().unwrap().clone();
    assert_ne!(after, before, "the image was written back");
    let image = media::dmk::Image::parse(&after).expect("an IMD image again");
    assert!(image.eight_inch);

    let mut n = machine(&[]).unwrap();
    insert(&mut n, 0, "copy.imd", Arc::new(Mutex::new(after)));
    wait_for(&mut n, plain, "A>", 30 * SECOND);
    run(&mut n, SECOND);
    n.type_text("dir\n");
    wait_for(&mut n, plain, "X        COM", 20 * SECOND);
}

/// A savestate taken while the boot loads from disk continues to the same
/// screen at the same clock as the machine it was taken from.
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR"]
fn a_savestate_in_the_middle_of_the_boot() {
    let Some((mut m, _)) = booted("diag_525.imd") else {
        return;
    };
    run(&mut m, SECOND);
    let state = m.save_state();
    run(&mut m, 6 * SECOND);
    assert!(styled(&m).contains("Select item to run:"));
    let (mut n, _) = booted("diag_525.imd").unwrap();
    n.load_state(&state).unwrap();
    run(&mut n, 6 * SECOND);
    assert_eq!(n.now(), m.now());
    assert_eq!(n.screen_codes(), m.screen_codes());
    assert_eq!(n.save_state(), m.save_state());
}

/// No disk in the drive: the ROM keeps trying and nothing shows. With
/// the boot device switched off it beeps.
#[test]
#[ignore = "needs EMU_ROM_DIR"]
fn without_a_disk_the_rom_waits_and_without_a_boot_device_it_beeps() {
    let Some(mut m) = machine(&[]) else {
        return;
    };
    run(&mut m, 3 * SECOND);
    assert!(m.screen_codes().is_empty(), "no picture yet");
    let mut sound = Vec::new();
    m.audio(&mut sound);
    assert!(sound.iter().all(|&s| s == 0));

    let mut m = machine(&[("boot", "none")]).unwrap();
    let mut sound = Vec::new();
    for _ in 0..3 * SECOND {
        run(&mut m, 1);
        m.audio(&mut sound);
    }
    assert!(sound.iter().any(|&s| s != 0), "the ROM's beep");
}

/// A look at what a disk does (not a check):
/// `MAX80_DISK=name [MAX80_KEYS=text] [MAX80_SECONDS=n]`.
#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR"]
fn explore() {
    let Ok(name) = std::env::var("MAX80_DISK") else {
        return;
    };
    let Some((mut m, _)) = booted(&name) else {
        return;
    };
    let seconds: usize = std::env::var("MAX80_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    run(&mut m, seconds * SECOND);
    println!("{}", plain(&m));
    if let Ok(keys) = std::env::var("MAX80_KEYS") {
        m.type_text(&keys.replace("\\n", "\n"));
        run(&mut m, keys.len() * 24 + seconds * SECOND);
        println!("{}\n{}", plain(&m), styled(&m));
    }
    let io = m.io();
    println!(
        "vlatch {:02x} mlatch {:02x} pio a {:02x} crtc {:02x?} fdc track {} sector {}",
        io.vlatch, io.mlatch, io.pio_a, io.crtc, io.fdc.track, io.fdc.sector
    );
    let cpu = m.devices()[0];
    let regs = m.debug_target(cpu).unwrap().regs();
    println!("pc {:04x}", regs.get("PC").unwrap_or(0));
}
