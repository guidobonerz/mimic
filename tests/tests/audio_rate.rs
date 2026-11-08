//! Every machine's sound against its frame rate: a frontend paces frames by
//! the duration the picture's signal metadata gives (pixel clock, lines,
//! pixels a line) and plays the samples at 48 kHz, so the samples a frame
//! produces must be 48 000 × that duration. A mismatch fills or drains the
//! desktop's sound buffer: the rate control (±0.5 %) runs at its limit and
//! the buffer grows into seconds of latency, or the sound breaks up.
//!
//! `EMU_ROM_DIR` supplies the ROMs; machines that cannot start without
//! theirs are skipped and listed. Run with `--nocapture` for the table.

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Machine, SAMPLE_RATE};

/// Frames to measure over, after a few to settle.
const FRAMES: u64 = 300;

fn measure(m: &mut dyn Machine) -> Option<(f64, f64)> {
    let mut dbg = DebugHooks::new();
    let mut out = Vec::new();
    // Long enough for a system to boot into its own picture mode.
    for _ in 0..600 {
        m.run_frame(&mut dbg);
        out.clear();
        m.audio(&mut out);
    }
    let mut samples = 0u64;
    let mut channels = 1;
    for _ in 0..FRAMES {
        m.run_frame(&mut dbg);
        out.clear();
        let f = m.audio(&mut out);
        channels = u64::from(f.channels.max(1));
        samples += out.len() as u64;
    }
    let s = m.framebuffer().signal;
    let pixels = f64::from(s.lines_per_field) * f64::from(s.total_pixels);
    if s.pixel_clock_hz == 0 || pixels == 0.0 {
        return None;
    }
    let frame_s = pixels / f64::from(s.pixel_clock_hz);
    let per_frame = (samples / channels) as f64 / FRAMES as f64;
    Some((per_frame, f64::from(SAMPLE_RATE) * frame_s))
}

#[test]
#[ignore = "needs EMU_ROM_DIR; prints a table"]
fn every_machine_makes_48_khz_at_its_frame_rate() {
    let Some(dir) = media_backends::roms::env_dir() else {
        eprintln!("EMU_ROM_DIR not set");
        return;
    };
    let registry = emu_machines::registry();
    let mut bad = Vec::new();
    let mut skipped = Vec::new();
    // Every model, and the Logophor also with its system card (Panorama
    // switches to the XGA field timing).
    let runs = registry.entries().iter().flat_map(|e| {
        let extra = (e.id == "logophor16").then_some((e, None::<&str>, true));
        // A machine without a model list runs once with its default.
        let models: Vec<Option<&str>> = if e.models.is_empty() {
            vec![None]
        } else {
            e.models.iter().map(|m| Some(*m)).collect()
        };
        models.into_iter().map(move |m| (e, m, false)).chain(extra)
    });
    for (entry, model, system_card) in runs {
        {
            let mut config = MachineConfig {
                model: model.map(str::to_string),
                ..MachineConfig::default()
            };
            if system_card {
                config.options.insert("card".into(), "system".into());
            }
            config.roms = media_backends::roms::load(&dir, entry.id).unwrap_or_default();
            let mut m = match registry.create(entry.id, &config) {
                Ok(m) => m,
                Err(e) => {
                    skipped.push(format!("{} {}: {e:?}", entry.id, model.unwrap_or("")));
                    continue;
                }
            };
            let Some((got, want)) = measure(m.as_mut()) else {
                skipped.push(format!(
                    "{} {}: no signal timing",
                    entry.id,
                    model.unwrap_or("")
                ));
                continue;
            };
            let ppm = (got / want - 1.0) * 1e6;
            println!(
                "{:12} {:10} {:9.2} samples/frame, {:9.2} by the signal: {:+8.0} ppm",
                entry.id,
                if system_card {
                    "card=system"
                } else {
                    model.unwrap_or("")
                },
                got,
                want,
                ppm
            );
            // No samples at all: a machine without sound (ZX81, IMSAI,
            // the test machine); the desktop then gets silence by itself.
            if got > 0.0 && ppm.abs() > 1000.0 {
                bad.push(format!(
                    "{} {} {ppm:+.0} ppm",
                    entry.id,
                    model.unwrap_or("")
                ));
            }
        }
    }
    for s in &skipped {
        println!("skipped: {s}");
    }
    assert!(bad.is_empty(), "sound and frame rate apart: {bad:?}");
}
