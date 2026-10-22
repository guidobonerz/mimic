//! Runs test programs of the vAmiga test suite (vAmigaTS, GPL-3.0 test
//! programs with photographs of real Amigas; not in the repository) and
//! writes a snapshot per test for comparison with the photographs:
//!
//!     EMU_ROM_DIR=~/roms EMU_VAMIGATS_DIR=/path/to/Denise/Sprites cargo test -p emu-tests --release --test vamigats -- --ignored --nocapture
//!
//! `EMU_VAMIGATS_DIR` is searched for `*.adf` files (any depth). Each ADF
//! boots on an A500 with Kickstart 1.3, 512 KB chip and 512 KB slow RAM
//! (the suite's "A500_OCS_1MB") for `EMU_FRAMES` frames (default 450 ≈
//! 9 s, the suite's wait); the last frame goes to
//! `target/tmp/vamigats/<name>.png`. `EMU_FILTER=substring` limits the run.
//! With `EMU_CLXDAT=1` the test also prints CLXDAT as read by the program
//! (sampled every frame through the chip register view), which is what
//! the collision tests turn into colour bars.

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, StopReason};

fn adfs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            adfs(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("adf")) {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs EMU_ROM_DIR (Kickstart 1.3) and EMU_VAMIGATS_DIR"]
fn snapshots_of_the_test_programs() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&rom_dir, "amiga").expect("ROMs");
    let dir = std::env::var("EMU_VAMIGATS_DIR").expect("EMU_VAMIGATS_DIR");
    let frames: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(450);
    let filter = std::env::var("EMU_FILTER").unwrap_or_default();
    let clxdat = std::env::var_os("EMU_CLXDAT").is_some();
    let mut list = Vec::new();
    adfs(std::path::Path::new(&dir), &mut list);
    list.sort();
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vamigats");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut ran = 0;
    for adf in list {
        let name = adf.file_stem().unwrap().to_string_lossy().into_owned();
        if !name.contains(&filter) {
            continue;
        }
        let mut config = MachineConfig {
            roms: roms.clone(),
            ..MachineConfig::default()
        };
        config.options.insert("kickstart".into(), "1.3".into());
        let mut m = emu_machines::registry().create("amiga", &config).unwrap();
        let media = media_backends::image::open(&adf).expect("ADF");
        m.media_slot(emu_core::media::SlotId(0))
            .unwrap()
            .insert(media)
            .expect("insert");
        let mut dbg = DebugHooks::new();
        let mut seen = std::collections::BTreeMap::new();
        for _ in 0..frames {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
            if clxdat {
                let v = m
                    .chips()
                    .into_iter()
                    .find(|c| c.name == "denise")
                    .and_then(|c| c.regs.into_iter().find(|r| r.name == "CLXDAT"))
                    .map(|r| r.value);
                if let Some(v) = v {
                    *seen.entry(v).or_insert(0) += 1;
                }
            }
        }
        let png = emu_preview::encode_png(&m.framebuffer());
        let path = out_dir.join(format!("{name}.png"));
        std::fs::write(&path, png).unwrap();
        let summary: Vec<String> = seen.iter().map(|(v, n)| format!("{v:04x}x{n}")).collect();
        println!("{name}: {} {}", path.display(), summary.join(" "));
        ran += 1;
    }
    assert!(ran > 0, "no ADF under {dir}");
}
