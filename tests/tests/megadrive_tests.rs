//! Mega Drive test ROMs from Genesis Plus GX's `md_test.zip` (no licence
//! given; fetched into `$EMU_TESTDATA_DIR/md-tests` by
//! `tools/fetch-testdata.sh`, never committed), Phase 37.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p mimic-tests --release --test megadrive_tests -- --ignored --nocapture
//!
//! `memtest_68k.bin` reads the 68000's memory map and prints each result
//! beside the value a real console gives, in brackets: the test reads the
//! text back from plane A and compares the two. The other ROMs have no
//! verdict of their own; they must run (no halted 68000, a picture that
//! is not blank) and their pictures go to `target/tmp/md-tests/` for a
//! look.

use std::path::PathBuf;

use machine_megadrive::{MegaDrive, SLOT_CARTRIDGE, VRAM_SPACE};
use mimic_core::machine::Machine;
use mimic_core::media::{Media, MediaBackend, MediaError, MediaKind};
use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, DeviceId, Size, StopReason};

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

fn testdata() -> PathBuf {
    PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR")).join("md-tests")
}

fn run(name: &str, frames: u32) -> MegaDrive {
    let rom = std::fs::read(testdata().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut m = MegaDrive::new(&MachineConfig::default()).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            name: name.into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(Bytes(rom)),
        })
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        let stop = m.run_frame(&mut dbg);
        assert_eq!(stop, StopReason::FrameDone, "{name}");
    }
    m
}

fn vdp_reg(m: &MegaDrive, r: usize) -> u32 {
    let chips = m.chips();
    let vdp = chips.iter().find(|c| c.name == "vdp").unwrap();
    vdp.regs[r].value
}

/// Plane A's text: the pattern number of each cell minus `font` as ASCII
/// (cells outside 32–126 as blanks), one string per row.
fn plane_text(m: &mut MegaDrive, font: u32) -> Vec<String> {
    let base = (vdp_reg(m, 2) & 0x38) << 10;
    let cols = match vdp_reg(m, 16) & 3 {
        1 => 64,
        3 => 128,
        _ => 32,
    };
    let t = m.debug_target(DeviceId("cpu")).unwrap();
    (0..28)
        .map(|row| {
            (0..40)
                .map(|col| {
                    let a = base + (row * cols + col) * 2;
                    let w = t.peek(VRAM_SPACE, a, Size::Word).unwrap() & 0x7ff;
                    let c = w.wrapping_sub(font);
                    if (32..127).contains(&c) {
                        char::from(c as u8)
                    } else {
                        ' '
                    }
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The font offset: the one that makes a row read `needle`.
fn find_font(m: &mut MegaDrive, needle: &str) -> Option<u32> {
    (0..0x7ff).find(|&f| plane_text(m, f).iter().any(|l| l.contains(needle)))
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with md-tests (tools/fetch-testdata.sh)"]
fn memtest_68k_matches_the_hardware_values() {
    let mut m = run("memtest_68k.bin", 120);
    let font = find_font(&mut m, "memory test").expect("the title");
    let text = plane_text(&mut m, font);
    let mut bad = Vec::new();
    let mut rows = 0;
    for line in &text {
        // "A10000-A1001F : A0A0 A0A0 (A0A0 A0A0)"
        let (Some(open), Some(close)) = (line.find('('), line.find(')')) else {
            continue;
        };
        let Some(colon) = line.find(':') else {
            continue;
        };
        rows += 1;
        let ours: Vec<&str> = line[colon + 1..open].split_whitespace().collect();
        let real: Vec<&str> = line[open + 1..close].split_whitespace().collect();
        let differs = ours
            .iter()
            .zip(&real)
            .any(|(o, r)| o.chars().zip(r.chars()).any(|(a, b)| b != '?' && a != b));
        println!("{} {line}", if differs { "BAD" } else { "ok " });
        if differs {
            bad.push(line.clone());
        }
    }
    assert!(rows >= 10, "the result rows: {text:#?}");
    assert!(
        bad.is_empty(),
        "{} of {rows} rows differ from the hardware:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with md-tests (tools/fetch-testdata.sh)"]
fn the_other_test_roms_run_and_draw() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/tmp/md-tests");
    std::fs::create_dir_all(&out).unwrap();
    for name in [
        "VDPTEST.bin",
        "SHLTEST.BIN",
        "SHLTEST2.BIN",
        "stetest.bin",
        "STETEST2.bin",
        "512PAL.BIN",
        "960PAL.bin",
        "PAL512.bin",
        "MD1536.bin",
        "mdtrans.bin",
        "vctest.bin",
        "wbug.bin",
        "wtest.bin",
        "wtest_i1.bin",
        "wtest_i2.bin",
    ] {
        let m = run(name, 300);
        let f = m.framebuffer();
        let mimic_core::frame::PixelData::Rgb(px) = f.pixels else {
            panic!("RGB")
        };
        let distinct = px
            .iter()
            .map(|p| (p.r, p.g, p.b))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let png = out.join(format!("{}.png", name.split('.').next().unwrap()));
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, f.width, f.height);
            enc.set_color(png::ColorType::Rgb);
            let mut w = enc.write_header().unwrap();
            let data: Vec<u8> = px.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
            w.write_image_data(&data).unwrap();
        }
        std::fs::write(&png, bytes).unwrap();
        println!(
            "{name}: {} × {}, {distinct} colours -> {}",
            f.width,
            f.height,
            png.display()
        );
        assert!(distinct > 1, "{name}: a blank picture");
    }
}
