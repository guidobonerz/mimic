//! VIC-20 boot to BASIC with real ROMs (user-supplied via `EMU_ROM_DIR`,
//! e.g. a `vic20/` subfolder).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test vic20_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};

/// "READY." in screen codes.
const READY: [u8; 6] = [0x12, 0x05, 0x01, 0x04, 0x19, 0x2e];

fn boot(model: &str, ram: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "vic20").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("ram".into(), ram.into());
    let mut m = emu_machines::registry().create("vic20", &config).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..150 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
    m
}

/// The 22 × 23 text screen at `base`.
fn screen(m: &mut dyn Machine, base: u32) -> Vec<u8> {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    (base..base + 22 * 23)
        .map(|a| cpu.peek(machine_vic20::CPU_SPACE, a, Size::Byte).unwrap() as u8)
        .collect()
}

/// Screen codes of `text` (upper case, digits, space and punctuation).
fn codes(text: &str) -> Vec<u8> {
    text.bytes()
        .map(|b| if b.is_ascii_uppercase() { b - 0x40 } else { b })
        .collect()
}

fn find(screen: &[u8], text: &str) -> Option<usize> {
    let pat = codes(text);
    screen.windows(pat.len()).position(|w| w == pat)
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_boots_and_runs_basic() {
    let mut m = boot("pal", "none");
    let s = screen(m.as_mut(), 0x1e00);
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vic20_boot.png");
    std::fs::write(&out, png).unwrap();
    assert!(
        find(&s, "3583 BYTES FREE").is_some(),
        "banner missing; snapshot at {}",
        out.display()
    );
    let ready = s.windows(READY.len()).position(|w| w == READY);
    // The 22-character banner and "BYTES FREE" each wrap into a blank line.
    assert_eq!(ready.map(|p| p / 22), Some(4), "READY. below the banner");

    // Type a BASIC line; the KERNAL scans the matrix through VIA2.
    let (events, end) = machine_vic20::keyboard::typed_events("PRINT 6*7\n", m.now().0);
    for e in events {
        m.input(e);
    }
    let mut dbg = DebugHooks::new();
    while m.now().0 < end + 100_000 {
        m.run_frame(&mut dbg);
    }
    let s = screen(m.as_mut(), 0x1e00);
    let typed = find(&s, "PRINT 6*7");
    let result = find(&s, " 42");
    assert!(typed.is_some(), "PRINT not echoed");
    assert_eq!(result.map(|p| p / 22), typed.map(|p| p / 22 + 1));
}

#[test]
#[ignore = "needs VIC-20 ROMs in EMU_ROM_DIR"]
fn vic20_memory_expansions_and_ntsc() {
    // With block 1 the KERNAL moves the screen to $1000.
    for (model, ram, base, free) in [
        ("pal", "3k", 0x1e00, "6655 BYTES FREE"),
        ("pal", "24k", 0x1000, "28159 BYTES FREE"),
        ("ntsc", "8k", 0x1000, "11775 BYTES FREE"),
    ] {
        let mut m = boot(model, ram);
        let s = screen(m.as_mut(), base);
        assert!(find(&s, free).is_some(), "{model} {ram}: {free} missing");
    }
}
