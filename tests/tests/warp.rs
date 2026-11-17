//! Warp (`Machine::set_warp`) with the machines' real ROMs (`#[ignore]`;
//! via `EMU_ROM_DIR`, else `~/.mimic/roms`): a machine that ran in warp is
//! afterwards in the state of one that drew every frame, and its next
//! frame is the same picture. Prints how much time warp saves.
//!
//! ```text
//! cargo test -p mimic-tests --release --test warp -- --ignored --nocapture
//! ```

use std::time::Instant;

use mimic_core::registry::MachineConfig;
use mimic_core::{DebugHooks, Machine};

fn machine(name: &str, model: Option<&str>) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("no ROM folder");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, name).expect("ROMs"),
        model: model.map(str::to_string),
        ..MachineConfig::default()
    };
    mimic_machines::registry().create(name, &config).unwrap()
}

/// The picture as bytes.
fn picture(m: &dyn Machine) -> Vec<u8> {
    let f = m.framebuffer();
    match &f.pixels {
        mimic_core::frame::PixelData::Indexed { pixels, .. } => pixels.to_vec(),
        mimic_core::frame::PixelData::Rgb(px) => px.iter().flat_map(|p| [p.r, p.g, p.b]).collect(),
    }
}

/// Boot `boot` frames, then run `frames` with and without warp.
fn check(name: &str, model: Option<&str>, boot: u32, frames: u32) {
    let label = format!("{name} {}", model.unwrap_or(""));
    let mut plain = machine(name, model);
    let mut warped = machine(name, model);
    let mut dbg = DebugHooks::default();
    for _ in 0..boot {
        plain.run_frame(&mut dbg);
        warped.run_frame(&mut dbg);
    }
    assert!(
        plain.save_state() == warped.save_state(),
        "{label}: two machines differ before warp"
    );
    let shown = picture(warped.as_ref());

    let start = Instant::now();
    for _ in 0..frames {
        plain.run_frame(&mut dbg);
    }
    let plain_time = start.elapsed();
    warped.set_warp(true);
    let start = Instant::now();
    for _ in 0..frames {
        warped.run_frame(&mut dbg);
    }
    let warp_time = start.elapsed();
    assert_eq!(plain.now(), warped.now(), "{label}: the same time");
    assert!(
        picture(warped.as_ref()) == shown,
        "{label}: a frame was drawn in warp"
    );

    // Warp off: a whole frame later the two cannot be told apart.
    warped.set_warp(false);
    plain.run_frame(&mut dbg);
    warped.run_frame(&mut dbg);
    assert!(
        picture(plain.as_ref()) == picture(warped.as_ref()),
        "{label}: the picture after warp"
    );
    assert!(
        plain.save_state() == warped.save_state(),
        "{label}: the state after warp"
    );
    let per_frame = |t: std::time::Duration| t.as_secs_f64() * 1e3 / f64::from(frames);
    eprintln!(
        "{label}: {:.2} ms a frame, in warp {:.2} ms ({:+.0} %)",
        per_frame(plain_time),
        per_frame(warp_time),
        (warp_time.as_secs_f64() / plain_time.as_secs_f64() - 1.0) * 100.0
    );
}

#[test]
#[ignore = "needs the user's ROMs"]
fn c64() {
    check("c64", None, 150, 300);
}

#[test]
#[ignore = "needs the user's ROMs"]
fn c128() {
    check("c128", None, 200, 300);
}

#[test]
#[ignore = "needs the user's ROMs"]
fn amiga() {
    check("amiga", None, 400, 300);
}

#[test]
#[ignore = "needs the user's ROMs"]
fn atari8() {
    check("atari8", None, 150, 300);
}
