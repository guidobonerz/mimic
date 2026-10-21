//! Acid800 (Avery Lee's Atari 8-bit hardware test suite): the Phase 5
//! gate, "Acid800 mostly green".
//!
//!     bash tools/fetch-testdata.sh
//!     EMU_ROM_DIR=~/roms EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test acid800 -- --ignored --nocapture
//!
//! Runs every standalone test of the suite on an 800XL with the real OS.
//! `EMU_ACID_FILTER=antic` runs only matching tests, `EMU_ACID_MODEL=ntsc`
//! the NTSC machine. The report goes to `target/tmp/acid800-<model>.txt`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use emu_core::registry::MachineConfig;
use emu_tests::acid800::{self, Outcome, Verdict};

/// Emulated time limit per test: 60 s (the whole suite takes "a couple of
/// minutes" on a real machine).
const MAX_FRAMES: u32 = 50 * 60;
/// Share of tests that must pass for the gate. PAL and NTSC pass 52 of 57
/// (2026-10-21); the five failures need models not built yet: the CPU's
/// bus value seen by ANTIC/GTIA (`antic_virtdma`, `gtia_phantomdma`),
/// playfield DMA running on through horizontal blank (`antic_hscrolbug`),
/// `antic_hiresbug` and GTIA's pseudo mode E (`gtia_psuedomodee`).
const GATE: f64 = 0.90;

fn testdata() -> PathBuf {
    PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR"))
        .join("acid800/standalone")
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh) and Atari ROMs in EMU_ROM_DIR"]
fn acid800_suite() {
    let model = std::env::var("EMU_ACID_MODEL").unwrap_or_else(|_| "pal".into());
    let filter = std::env::var("EMU_ACID_FILTER").unwrap_or_default();
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "atari800xl").expect("ROMs"),
        model: Some(model.clone()),
        ..MachineConfig::default()
    };
    // The standalone tests boot from the program slot; the serial tests
    // talk to D1:, which holds the suite's own disk.
    config.options.insert("drives".into(), "1".into());
    let disk = std::fs::read(testdata().join("../acid800.atr")).expect("acid800.atr");
    let data = testdata();
    let mut names: Vec<String> = std::fs::read_dir(&data)
        .expect("testdata/acid800 missing: run tools/fetch-testdata.sh")
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".xex").map(str::to_string)
        })
        // mod_* are the full suite's loadable modules, not tests.
        .filter(|n| !n.starts_with("mod_"))
        .filter(|n| n.contains(&filter))
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no tests match '{filter}'");

    let registry = emu_machines::registry();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(String, Result<Outcome, String>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(name) = names.get(i) else { break };
                let xex = std::fs::read(data.join(format!("{name}.xex"))).expect("xex");
                let lab =
                    std::fs::read_to_string(data.join(format!("{name}.lab"))).expect("label file");
                let r = acid800::run(&registry, &config, &xex, &lab, Some(&disk), MAX_FRAMES);
                results.lock().unwrap().push((name.clone(), r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let mut report = String::new();
    let (mut pass, mut skip) = (0, 0);
    for (name, r) in &results {
        let line = match r {
            Ok(o) => {
                match o.verdict {
                    Verdict::Pass => pass += 1,
                    Verdict::Skipped => skip += 1,
                    _ => {}
                }
                format!(
                    "{:<8} {name:<28} {:>5} fields  {}",
                    format!("{:?}", o.verdict),
                    o.frames,
                    o.message
                )
            }
            Err(e) => format!("ERROR    {name:<28} {e}"),
        };
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }
    // Skipped tests (e.g. 65C816 on a 6502) count as not applicable.
    let total = results.len() - skip;
    let ratio = pass as f64 / total.max(1) as f64;
    let summary = format!(
        "Acid800 ({model}): {pass}/{total} passed ({:.1} %), {skip} skipped",
        ratio * 100.0
    );
    println!("{summary}");
    report.push_str(&summary);
    report.push('\n');
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("acid800-{model}.txt"));
    std::fs::write(&out, report).unwrap();
    if filter.is_empty() {
        assert!(ratio >= GATE, "{summary}; report: {}", out.display());
    }
}
