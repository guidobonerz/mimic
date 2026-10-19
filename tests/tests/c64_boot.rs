//! C64 boot to BASIC with real ROMs (user-supplied via `EMU_ROM_DIR`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test c64_boot -- --ignored

use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};

/// "READY." in screen codes.
const READY: [u8; 6] = [0x12, 0x05, 0x01, 0x04, 0x19, 0x2e];

#[test]
#[ignore = "needs C64 ROMs in EMU_ROM_DIR"]
fn c64_boots_to_ready() {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "c64").expect("ROMs"),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("c64", &config).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..150 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }

    let ram = m
        .memory_spaces()
        .into_iter()
        .find(|s| s.name == "ram")
        .unwrap()
        .id;
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let screen: Vec<u8> = (0x0400..0x07e8)
        .map(|a| cpu.peek(ram, a, Size::Byte).unwrap() as u8)
        .collect();
    let found = screen.windows(READY.len()).position(|w| w == READY);

    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("c64_boot.png");
    std::fs::write(&out, png).unwrap();

    let row = found.map(|p| p / 40);
    assert!(
        found.is_some(),
        "READY. not on screen; snapshot at {}",
        out.display()
    );
    assert_eq!(row, Some(5), "READY. below the banner");

    // Type a BASIC line; the KERNAL scans the matrix through CIA1.
    let (events, end) = emu_tests::typing::type_text("PRINT 6*7\n", m.now().0);
    for e in events {
        m.input(e);
    }
    while m.now().0 < end + 100_000 {
        m.run_frame(&mut dbg);
    }
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    let screen: Vec<u8> = (0x0400..0x07e8)
        .map(|a| cpu.peek(ram, a, Size::Byte).unwrap() as u8)
        .collect();
    // " 42" in screen codes below the typed line.
    let answer = [0x20, 0x34, 0x32];
    let typed = screen
        .windows(5)
        .position(|w| w == [0x10, 0x12, 0x09, 0x0e, 0x14]);
    let result = screen.windows(3).position(|w| w == answer);
    assert!(typed.is_some(), "PRINT not echoed");
    assert_eq!(result.map(|p| p / 40), typed.map(|p| p / 40 + 1));

    // Light blue border around a blue screen.
    let f = m.framebuffer();
    let border = f.rgb_at(2, 2).unwrap();
    let paper = f.rgb_at(200, 200).unwrap();
    assert_ne!(border, paper);
    assert!(paper.b > paper.r && border.b > border.r);
}
