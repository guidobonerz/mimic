//! Every monitor preset on the CPU path against a reference image of the
//! test card (`common::pattern`), tolerance-based except `raw`, which must
//! match byte for byte. Persistence presets also get a trail picture (the
//! box moved after a frame, through the stateful `Crt`).
//!
//! Regenerate the references after an intended change with
//! `EMU_UPDATE_REFERENCE=1 cargo test -p video-crt --test reference`
//! and explain the change in the commit. They live in
//! `tests/reference/crt/pattern-<preset>.png`.

mod common;

use common::{diff, frame, pattern, read_png, reference_dir, write_png};
use video_crt::{Crt, Profile};

fn check(name: &str, img: &video_crt::Image, exact: bool, failures: &mut Vec<String>) {
    let path = reference_dir().join(format!("{name}.png"));
    if std::env::var_os("EMU_UPDATE_REFERENCE").is_some() {
        write_png(&path, img);
        return;
    }
    let Some((w, h, want)) = read_png(&path) else {
        failures.push(format!(
            "{name}: missing {} (EMU_UPDATE_REFERENCE=1)",
            path.display()
        ));
        return;
    };
    if (w, h) != (img.width, img.height) {
        failures.push(format!(
            "{name}: size {}x{} vs {w}x{h}",
            img.width, img.height
        ));
        return;
    }
    let (max, mean, _) = diff(&img.rgb, &want, 0);
    // Float rounding may differ between targets; structure may not.
    let bad = if exact {
        max > 0
    } else {
        max > 3 || mean > 0.05
    };
    if bad {
        failures.push(format!("{name}: max diff {max}, mean {mean:.4}"));
    }
}

#[test]
fn presets_match_reference_images() {
    let px = pattern(0);
    let moved = pattern(12);
    let mut failures = Vec::new();
    for preset in Profile::PRESETS {
        let profile = Profile::preset(preset).unwrap();
        let img = video_crt::render(&frame(&px), &profile, 2);
        check(
            &format!("pattern-{preset}"),
            &img,
            preset == "raw",
            &mut failures,
        );
        if profile.persistence > 0.0 {
            let mut crt = Crt::new();
            crt.render(&frame(&px), &profile, 2);
            let img = crt.render(&frame(&moved), &profile, 2);
            check(
                &format!("pattern-{preset}-trail"),
                img,
                false,
                &mut failures,
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn raw_reference_is_the_frame_itself() {
    // At scale 1 without aspect correction, raw is the palette lookup.
    let px = pattern(0);
    let img = video_crt::render(&frame(&px), &Profile::preset("raw").unwrap(), 1);
    let want: Vec<u8> = px
        .iter()
        .flat_map(|&i| {
            let c = common::PALETTE[usize::from(i)];
            [c.r, c.g, c.b]
        })
        .collect();
    assert_eq!(img.rgb, want);
}
