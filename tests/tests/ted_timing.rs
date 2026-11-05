//! TED timing measured by the programs of `tools/ted_timing.py`, compared
//! with the bytes the same programs measured in VICE's xplus4
//! (`tests/data/ted-<name>.ref`, read back from its screenshot; see the
//! tool's documentation). The programs keep their results at `$2000`.
//!
//!     EMU_ROM_DIR=$PWD/roms cargo test -p emu-tests --release --test ted_timing -- --ignored --nocapture
//!
//! `EMU_TED_SHOW=1` prints both side by side.

use emu_core::media::MediaKind;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};

fn data(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run `ted-<name>.prg` on a PAL Plus/4 and return `len` bytes from `$2000`.
fn measure(name: &str, len: usize) -> Vec<u8> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "plus4").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("plus4", &config).unwrap();
    let prg = data(&format!("ted-{name}.prg"));
    let media =
        media_backends::image::from_bytes(&format!("ted-{name}.prg"), prg, MediaKind::Program);
    let slot = m
        .media_slots()
        .into_iter()
        .find(|s| s.kind == MediaKind::Program)
        .unwrap()
        .id;
    m.media_slot(slot).unwrap().insert(media).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..500 {
        m.run_frame(&mut dbg);
    }
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0..len as u32)
        .map(|i| {
            cpu.peek(machine_ted::CPU_SPACE, 0x2000 + i, Size::Byte)
                .unwrap() as u8
        })
        .collect()
}

/// $FF1E counts 0–227 (two units a double clock); the step between two
/// samples, assuming less than a line passed.
fn step(a: u8, b: u8) -> i32 {
    let d = i32::from(b) - i32::from(a);
    if d < 0 {
        d + 228
    } else {
        d
    }
}

fn show() -> bool {
    std::env::var_os("EMU_TED_SHOW").is_some()
}

/// Compare and count differing samples; print side by side if asked.
fn compare_trace(reference: &[u8], ours: &[u8]) -> usize {
    let n = reference.len() / 2;
    let (rh, rl) = reference.split_at(n);
    let (oh, ol) = ours.split_at(n);
    let mut differ = 0;
    if show() {
        println!("trace: xplus4 line hpos step | mimic line hpos step");
    }
    for i in 0..n {
        let same = rh[i] == oh[i] && rl[i] == ol[i];
        differ += usize::from(!same);
        if show() {
            let s = |h: &[u8]| {
                if i == 0 {
                    String::new()
                } else {
                    step(h[i - 1], h[i]).to_string()
                }
            };
            println!(
                "  {i:4}  {:3} ${:02X} {:>4} | {:3} ${:02X} {:>4} {}",
                rl[i],
                rh[i],
                s(rh),
                ol[i],
                oh[i],
                s(oh),
                if same { "" } else { "*" }
            );
        }
    }
    differ
}

fn compare_fine(reference: &[u8], ours: &[u8]) -> usize {
    let mut differ = 0;
    for (run, (r, o)) in reference.chunks(256).zip(ours.chunks(256)).enumerate() {
        let d = r.iter().zip(o).filter(|(a, b)| a != b).count();
        differ += d;
        if show() {
            println!("fine run {run}: {d} differ");
            for i in 0..256 {
                if r[i] != o[i] || i < 4 {
                    let rs = if i == 0 { 0 } else { step(r[i - 1], r[i]) };
                    let os = if i == 0 { 0 } else { step(o[i - 1], o[i]) };
                    println!(
                        "  {i:3} xplus4 ${:02X} {rs:3} | mimic ${:02X} {os:3}",
                        r[i], o[i]
                    );
                }
            }
        }
    }
    differ
}

fn check(name: &str, compare: fn(&[u8], &[u8]) -> usize) {
    let reference = data(&format!("ted-{name}.ref"));
    let ours = measure(name, reference.len());
    let differ = compare(&reference, &ours);
    println!("{name}: {differ} of {} bytes differ", reference.len());
    assert_eq!(differ, 0, "TED {name} differs from xplus4 (EMU_TED_SHOW=1)");
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn trace_matches_xplus4() {
    check("trace", compare_trace);
}

#[test]
#[ignore = "needs Plus/4 ROMs in EMU_ROM_DIR"]
fn fine_matches_xplus4() {
    check("fine", compare_fine);
}
