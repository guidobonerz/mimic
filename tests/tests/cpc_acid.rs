//! Kevin Thacker's Arnold "Acid tests" on the CPC 6128: the Phase 6
//! gate.
//!
//!     bash tools/fetch-testdata.sh          # needs a C++ compiler (pasmo)
//!     EMU_ROM_DIR=~/roms EMU_TESTDATA_DIR=$PWD/testdata \
//!         cargo test -p emu-tests --release --test cpc_acid -- --ignored --nocapture
//!
//! Runs every assembled test program (`testdata/cpc-acid/<suite>/*.bin`)
//! and counts the result lines of those that finish ("Done."). Visual and
//! interactive tests never finish by themselves and only show up in the
//! report. `EMU_CPC_FILTER=ppi` runs matching programs, `EMU_CPC_CRTC=0`
//! picks the CRTC type (default 1, the author's reference machine),
//! `EMU_CPC_FRAMES` the time limit in fields, `EMU_CPC_TAIL=<n>` adds the
//! last n output characters. The report goes to `target/tmp/cpc-acid.txt`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use emu_core::registry::MachineConfig;
use emu_tests::cpc_acid::{self, Outcome};

/// Emulated time limit per program: 15 minutes (some decode tests wait
/// for a VSYNC on each of the 65536 ports; the emulation runs about nine
/// times faster than a CPC).
const MAX_FRAMES: u32 = 50 * 60 * 15;
/// Programs for other CRTC types than the configured one (the CPC Plus
/// ASIC's types 3 and 4 are not emulated).
const ONLY_FOR_CRTC: [(&str, &str); 1] = [("crtc/crtc34st", "34")];
/// Share of result lines that must pass, over the programs that finish.
const GATE: f64 = 0.90;

fn testdata() -> PathBuf {
    PathBuf::from(std::env::var_os("EMU_TESTDATA_DIR").expect("EMU_TESTDATA_DIR")).join("cpc-acid")
}

#[test]
#[ignore = "needs EMU_TESTDATA_DIR (tools/fetch-testdata.sh) and CPC ROMs in EMU_ROM_DIR"]
fn cpc_acid_suite() {
    let filter = std::env::var("EMU_CPC_FILTER").unwrap_or_default();
    let crtc = std::env::var("EMU_CPC_CRTC").unwrap_or_else(|_| "1".into());
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "cpc").expect("ROMs"),
        model: Some("6128".into()),
        ..MachineConfig::default()
    };
    config.options.insert("crtc".into(), crtc.clone());
    let root = testdata();
    let mut programs: Vec<(String, PathBuf)> = Vec::new();
    for suite in
        std::fs::read_dir(&root).expect("testdata/cpc-acid missing: run tools/fetch-testdata.sh")
    {
        let suite = suite.unwrap().path();
        if !suite.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&suite).unwrap() {
            let p = f.unwrap().path();
            if p.extension().is_some_and(|e| e == "bin") {
                let name = format!(
                    "{}/{}",
                    suite.file_name().unwrap().to_string_lossy(),
                    p.file_stem().unwrap().to_string_lossy()
                );
                let other_crtc = ONLY_FOR_CRTC
                    .iter()
                    .any(|(p, types)| *p == name && !types.contains(crtc.as_str()));
                if name.contains(&filter) && !other_crtc {
                    programs.push((name, p));
                }
            }
        }
    }
    programs.sort();
    assert!(!programs.is_empty(), "no programs match '{filter}'");

    let crtc_digit = crtc.bytes().next().unwrap_or(b'1');
    // EMU_CPC_FRAMES=<n>: a shorter limit while diagnosing.
    let max_frames = std::env::var("EMU_CPC_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_FRAMES);
    let registry = emu_machines::registry();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(String, Result<Outcome, String>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((name, path)) = programs.get(i) else {
                    break;
                };
                let bin = std::fs::read(path).expect("bin");
                let sym = std::fs::read_to_string(path.with_extension("sym")).unwrap_or_default();
                let r = cpc_acid::run(&registry, &config, &bin, &sym, crtc_digit, max_frames);
                results.lock().unwrap().push((name.clone(), r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let mut report = String::new();
    let (mut passed, mut failed, mut finished) = (0, 0, 0);
    for (name, r) in &results {
        let line = match r {
            Ok(o) => {
                if o.done {
                    finished += 1;
                    passed += o.passed;
                    failed += o.failed;
                }
                let state = match (o.manual, o.done) {
                    (true, _) => "manual",
                    (_, true) => "done",
                    _ => "no end",
                };
                let mut l = format!(
                    "{name:<24} {state:<7} {:>4} pass {:>4} fail {:>5} fields",
                    o.passed, o.failed, o.frames
                );
                // EMU_CPC_TAIL=<n>: the last n characters of the
                // program's output (diagnosis).
                let tail_len = std::env::var("EMU_CPC_TAIL")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok());
                if let Some(n) = tail_len {
                    let tail: String = o
                        .text
                        .chars()
                        .rev()
                        .take(n)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    l.push_str(&format!(
                        "
  tail: {tail:?}"
                    ));
                }
                for f in &o.failures {
                    l.push_str(&format!("\n    {f}"));
                }
                l
            }
            Err(e) => format!("{name:<24} ERROR {e}"),
        };
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }
    let total = passed + failed;
    let ratio = f64::from(passed) / f64::from(total.max(1));
    let summary = format!(
        "CPC Acid (CRTC {crtc}): {passed}/{total} results passed ({:.1} %) in {finished} of {} programs that finish",
        ratio * 100.0,
        results.len()
    );
    println!("{summary}");
    report.push_str(&summary);
    report.push('\n');
    // Trend per program: it finishes without a failed result.
    let outcomes: Vec<(String, bool)> = results
        .iter()
        .map(|(name, r)| {
            let ok = matches!(r, Ok(o) if o.done && o.failed == 0);
            (name.clone(), ok)
        })
        .collect();
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let trend = emu_tests::trend::record(&tmp, &format!("cpc-acid-crtc{crtc}"), &outcomes);
    println!("{trend}");
    report.push_str(&trend);
    report.push('\n');
    let out = tmp.join("cpc-acid.txt");
    std::fs::write(&out, report).unwrap();
    if filter.is_empty() {
        assert!(ratio >= GATE, "{summary}; report: {}", out.display());
    }
}
