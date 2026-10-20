//! PET models boot to BASIC with real ROMs (user-supplied via
//! `EMU_ROM_DIR`, e.g. a `pet/` subfolder) and take keyboard input.
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test pet_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size};
use machine_pet::keyboard::Layout;

fn boot(model: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "pet").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("pet", &config).unwrap();
    let mut dbg = DebugHooks::new();
    while m.now().0 < 3_000_000 {
        m.run_frame(&mut dbg);
    }
    m
}

/// Video RAM as screen codes, `columns` per row.
fn screen(m: &mut dyn Machine, columns: u32) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (0x8000..0x8000 + columns * 25)
        .map(|a| cpu.peek(machine_pet::CPU_SPACE, a, Size::Byte).unwrap() as u8)
        .collect()
}

/// Screen codes of `text` (upper case, digits, space and punctuation).
fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn row_of(screen: &[u8], columns: usize, text: &str) -> Option<usize> {
    let pat = codes(text);
    screen
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| p / columns)
}

#[test]
#[ignore = "needs PET ROMs in EMU_ROM_DIR"]
fn every_model_boots_and_runs_basic() {
    for (model, columns, banner, free, layout, line, answer) in [
        (
            "2001",
            40,
            "*** COMMODORE BASIC ***",
            "7167 BYTES FREE",
            Layout::Graphics,
            "PRINT 6*7",
            " 42",
        ),
        (
            "3032",
            40,
            "### COMMODORE BASIC ###",
            "31743 BYTES FREE",
            Layout::Graphics,
            "PRINT 6*7",
            " 42",
        ),
        (
            "4032",
            40,
            "*** COMMODORE BASIC 4.0 ***",
            "31743 BYTES FREE",
            Layout::Graphics,
            "PRINT 6*7",
            " 42",
        ),
        // The 8032 starts in text mode; letters typed without SHIFT show
        // as lower case, which has the same screen codes.
        (
            "8032",
            80,
            "*** COMMODORE BASIC 4.0 ***",
            "31743 BYTES FREE",
            Layout::Business,
            "PRINT 6;7",
            " 6  7",
        ),
    ] {
        let mut m = boot(model);
        let s = screen(m.as_mut(), columns);
        let png = emu_preview::encode_png(&m.framebuffer());
        let out =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pet_{model}.png"));
        std::fs::write(&out, png).unwrap();
        let c = columns as usize;
        assert_eq!(
            row_of(&s, c, banner),
            Some(0),
            "{model}: banner; snapshot at {}",
            out.display()
        );
        let free_row = row_of(&s, c, free);
        let ready_row = row_of(&s, c, "READY.");
        assert!(free_row.is_some_and(|r| r > 0), "{model}: {free}");
        assert!(ready_row > free_row, "{model}: READY.");

        let (events, end) =
            machine_pet::keyboard::typed_events(layout, &format!("{line}\n"), m.now().0);
        for e in events {
            m.input(e);
        }
        let mut dbg = DebugHooks::new();
        while m.now().0 < end + 100_000 {
            m.run_frame(&mut dbg);
        }
        let s = screen(m.as_mut(), columns);
        let typed = row_of(&s, c, line);
        assert!(typed > ready_row, "{model}: typed line not echoed");
        assert_eq!(
            row_of(&s, c, answer),
            typed.map(|r| r + 1),
            "{model}: no answer"
        );
    }
}
