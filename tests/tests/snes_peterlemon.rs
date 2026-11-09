//! PeterLemon's SNES test and demo ROMs (https://github.com/PeterLemon/SNES,
//! no licence given: fetched into `$EMU_TESTDATA_DIR/peterlemon-snes` by
//! `tools/fetch-testdata.sh`, never committed). Each ROM has a PNG of the
//! picture it should show; this runner runs the ROM and compares.
//!
//!     EMU_TESTDATA_DIR=$PWD/testdata cargo test -p emu-tests --release --test snes_peterlemon -- --ignored --nocapture
//!
//! Compared: the CPU and SPC700 instruction tests (`CPUTest/`), the PPU
//! tests (`PPU/`) and the bank tests (`BANK/`); the Super FX (`CHIP/GSU`),
//! MSU-1 and input ROMs are left out. The reference pictures are 256 × 224
//! (or 512 × 448): our 512-dot frame is sampled at every other dot for a
//! 256-wide picture, the rows from the top; a colour counts as equal within
//! 24 per channel (the 15-bit to 8-bit expansion differs between tools).
//! The report goes to `target/tmp/snes-peterlemon.txt` with the trend,
//! our pictures of failing ROMs next to it. `EMU_SNES_FILTER=text` runs
//! only paths containing it, `EMU_SNES_FRAMES=n` runs n frames (300).

use std::path::{Path, PathBuf};

use emu_core::frame::PixelData;
use emu_core::machine::Machine;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::DebugHooks;

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

const FRAMES: u32 = 300;
const SPC_FRAMES: u32 = 3000;
const TOLERANCE: i32 = 24;
/// Our frame row of the reference pictures' top row.
const FIRST_ROW: u32 = 0;

fn root() -> PathBuf {
    let dir = std::env::var_os("EMU_TESTDATA_DIR")
        .expect("EMU_TESTDATA_DIR not set; run tools/fetch-testdata.sh first");
    PathBuf::from(dir).join("peterlemon-snes")
}

fn roms(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            roms(&p, out);
        } else if p.extension().is_some_and(|e| e == "sfc") && p.with_extension("png").exists() {
            out.push(p);
        }
    }
}

fn machine(path: &Path) -> Result<Box<dyn Machine>, String> {
    let mut config = MachineConfig::default();
    if let Some(dir) = media_backends::roms::env_dir() {
        if let Ok(r) = media_backends::roms::load(&dir, "snes") {
            config.roms = r;
        }
    }
    let mut m = emu_machines::registry()
        .create("snes", &config)
        .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    m.media_slot(machine_snes::SLOT_CARTRIDGE)
        .ok_or("no cartridge slot")?
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: path.display().to_string(),
            backend: Box::new(Bytes(bytes)),
        })
        .map_err(|e| e.to_string())?;
    Ok(m)
}

/// A PNG as RGB: width, height, bytes.
fn read_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let bpp = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(format!("PNG colour type {other:?}")),
    };
    let rgb = buf.chunks(bpp).flat_map(|c| [c[0], c[1], c[2]]).collect();
    Ok((info.width, info.height, rgb))
}

/// Pixels that differ, or an error.
fn compare(m: &dyn Machine, png: &Path) -> Result<usize, String> {
    let (rw, rh, reference) = read_png(png)?;
    let f = m.framebuffer();
    let PixelData::Rgb(px) = f.pixels else {
        return Err("not an RGB frame".into());
    };
    let step = if rw * 2 <= f.width { 2 } else { 1 };
    // A 448-row picture of a non-interlaced frame has every line twice.
    let line_halved = rh > 300 && f.height < 300;
    let mut differ = 0;
    for y in 0..rh {
        // Both start with the first shown line.
        let fy = if line_halved { y / 2 } else { y } + FIRST_ROW;
        if fy >= f.height {
            differ += rw as usize;
            continue;
        }
        for x in 0..rw {
            let fx = x * step;
            let ours = px[(fy * f.width + fx) as usize];
            let i = ((y * rw + x) * 3) as usize;
            let r = &reference[i..i + 3];
            let close = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= TOLERANCE;
            if !(close(ours.r, r[0]) && close(ours.g, r[1]) && close(ours.b, r[2])) {
                differ += 1;
            }
        }
    }
    Ok(differ)
}

fn dump(m: &dyn Machine, name: &str) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("snes-peterlemon");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{}.png", name.replace(['/', '\\'], "_")));
    std::fs::write(file, emu_preview::encode_png(&m.framebuffer())).unwrap();
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR with peterlemon-snes"]
fn peterlemon_snes() {
    let base = root();
    let filter = std::env::var("EMU_SNES_FILTER").unwrap_or_default();
    let mut list = Vec::new();
    for d in ["CPUTest", "PPU", "BANK"] {
        roms(&base.join(d), &mut list);
    }
    let mut report = String::new();
    let mut results = Vec::new();
    let (mut pass, mut fail) = (0, 0);
    for path in &list {
        let name = path
            .strip_prefix(&base)
            .unwrap()
            .display()
            .to_string()
            .replace('\\', "/");
        if !name.contains(&filter) {
            continue;
        }
        let verdict = match machine(path) {
            Err(e) => format!("error: {e}"),
            Ok(mut m) => {
                let mut dbg = DebugHooks::new();
                let frames = std::env::var("EMU_SNES_FRAMES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(if name.starts_with("CPUTest/SPC700") {
                        // Their upload-and-run rounds take up to ~50 s
                        // here (with the machine's own IPL loader; whether
                        // the real IPL is quicker is open).
                        SPC_FRAMES
                    } else {
                        FRAMES
                    });
                for _ in 0..frames {
                    m.run_frame(&mut dbg);
                }
                match compare(m.as_ref(), &path.with_extension("png")) {
                    Ok(0) => "pass".into(),
                    Ok(n) => {
                        dump(m.as_ref(), &name);
                        format!("FAIL {n} pixels differ")
                    }
                    Err(e) => format!("error: {e}"),
                }
            }
        };
        if verdict == "pass" {
            pass += 1;
        } else {
            fail += 1;
        }
        report.push_str(&format!("{name:70} {verdict}\n"));
        results.push((name, verdict == "pass"));
    }
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    report.push_str(&format!("\n{pass} passed, {fail} failed\n"));
    let trend = emu_tests::trend::record(tmp, "snes-peterlemon", &results);
    report.push_str(&trend);
    std::fs::write(tmp.join("snes-peterlemon.txt"), &report).unwrap();
    println!("{report}");
}
