//! Lorenz C64 Emulator Test Suite: the Phase 2 gate (≥ 90 % passing).
//!
//!     bash tools/fetch-testdata.sh
//!     # with real ROMs (KERNAL boots, RUN):
//!     EMU_ROM_DIR=~/roms EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test lorenz lorenz_suite -- --ignored --exact --nocapture
//!     # without ROMs (stub KERNAL, CI):
//!     EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test lorenz lorenz_suite_stub -- --ignored --nocapture
//!
//! `EMU_LORENZ_FILTER=cia` runs only matching tests, `EMU_LORENZ_CIA=new`
//! uses a 6526A and the suite's `*new` program variants.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use emu_core::registry::MachineConfig;
use emu_tests::lorenz::{self, Outcome, Verdict};

/// Emulated time limit per test: 10 minutes.
const MAX_FRAMES: u32 = 50 * 600;
const GATE: f64 = 0.90;

fn testdata() -> PathBuf {
    PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR")).join("lorenz")
}

/// Run every test in `data` (minus `skip`) and return (passed, total).
fn run_suite(mut config: MachineConfig, data: &Path, skip: &[&str], report_name: &str) -> f64 {
    let new_cia = std::env::var("EMU_LORENZ_CIA").is_ok_and(|v| v == "new");
    let filter = std::env::var("EMU_LORENZ_FILTER").unwrap_or_default();
    if new_cia {
        config.options.insert("cia".into(), "6526a".into());
    }

    // Plain names; for a 6526A prefer the "new" variant where one exists.
    let mut names: Vec<String> = std::fs::read_dir(data)
        .expect("testdata/lorenz missing: run tools/fetch-testdata.sh")
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".prg").map(str::to_string)
        })
        .filter(|n| !n.ends_with("new"))
        .filter(|n| n.contains(&filter))
        .collect();
    names.sort();
    let skipped: Vec<String> = names
        .iter()
        .filter(|n| skip.contains(&n.as_str()))
        .cloned()
        .collect();
    names.retain(|n| !skip.contains(&n.as_str()));
    let jobs: Vec<(String, String)> = names
        .into_iter()
        .map(|n| {
            let variant = format!("{n}new");
            let file = if new_cia && data.join(format!("{variant}.prg")).exists() {
                variant
            } else {
                n.clone()
            };
            (n, file)
        })
        .collect();

    let registry = emu_machines::registry();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(String, Outcome)>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((name, file)) = jobs.get(i) else {
                    break;
                };
                let prg = std::fs::read(data.join(format!("{file}.prg"))).unwrap();
                let outcome = lorenz::run(&registry, &config, &prg, MAX_FRAMES);
                results.lock().unwrap().push((name.clone(), outcome));
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let passed = results
        .iter()
        .filter(|(_, o)| o.verdict == Verdict::Pass)
        .count();
    let total = results.len();
    let mut report = String::new();
    for (name, o) in &results {
        report.push_str(&format!(
            "{name:<12} {:?} ({} frames)\n",
            o.verdict, o.frames
        ));
        if o.verdict != Verdict::Pass {
            for line in o.output.lines().filter(|l| !l.trim().is_empty()) {
                report.push_str(&format!("    | {line}\n"));
            }
        }
    }
    for name in &skipped {
        report.push_str(&format!("{name:<12} skipped (needs real ROMs)\n"));
    }
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let outcomes: Vec<(String, bool)> = results
        .iter()
        .map(|(name, o)| (name.clone(), o.verdict == Verdict::Pass))
        .collect();
    let trend = emu_tests::trend::record(tmp, report_name.trim_end_matches(".txt"), &outcomes);
    report.push_str(&trend);
    report.push('\n');
    let out = tmp.join(report_name);
    std::fs::write(&out, &report).unwrap();
    for (name, o) in results.iter().filter(|(_, o)| o.verdict != Verdict::Pass) {
        println!("FAIL {name:<12} {:?}", o.verdict);
    }
    let ratio = passed as f64 / total.max(1) as f64;
    println!(
        "Lorenz: {passed}/{total} passed ({:.1} %), {} skipped, report: {}",
        ratio * 100.0,
        skipped.len(),
        out.display()
    );
    println!("{trend}");
    if filter.is_empty() {
        ratio
    } else {
        1.0
    }
}

#[test]
#[ignore = "needs EMU_ROM_DIR and EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn lorenz_suite() {
    let rom_dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&rom_dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    let ratio = run_suite(config, &testdata(), &[], "lorenz.txt");
    assert!(ratio >= GATE, "gate is {:.0} %", GATE * 100.0);
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh)"]
fn lorenz_suite_stub() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    let ratio = run_suite(
        config,
        &testdata(),
        lorenz::NEEDS_REAL_ROMS,
        "lorenz-stub.txt",
    );
    assert!(ratio >= GATE, "gate is {:.0} %", GATE * 100.0);
}
