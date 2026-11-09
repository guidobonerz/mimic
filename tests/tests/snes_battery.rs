//! The SNES cartridge's battery RAM through a real `.srm` file beside the
//! image (`media_backends::image::attach_battery_ram`), as the frontends
//! attach it. Needs no ROMs.

use std::path::Path;

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::DebugHooks;

/// A 32 KB LoROM image with 2 KB battery RAM: `code` at $8000.
fn image(code: &[u8]) -> Vec<u8> {
    let mut rom = vec![0xea; 0x8000];
    rom[..code.len()].copy_from_slice(code);
    rom[0x7fc0..0x7fd5].copy_from_slice(b"MIMIC BATTERY        ");
    rom[0x7fd5] = 0x20;
    rom[0x7fd6..0x7fdc].copy_from_slice(&[0x02, 0x05, 0x01, 0x01, 0x00, 0x00]);
    rom[0x7fdc..0x7fe0].copy_from_slice(&[0xff, 0xff, 0x00, 0x00]);
    rom[0x7ffc..0x7ffe].copy_from_slice(&[0x00, 0x80]);
    rom
}

fn run(cart: &Path, frames: u32) -> Box<dyn Machine> {
    let mut m = emu_machines::registry()
        .create("snes", &MachineConfig::default())
        .unwrap();
    let media = media_backends::image::open_cart(cart).unwrap();
    m.media_slot(machine_snes::SLOT_CARTRIDGE)
        .unwrap()
        .insert(media)
        .unwrap();
    media_backends::image::attach_battery_ram(m.as_mut(), cart).unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..frames {
        m.run_frame(&mut dbg);
    }
    m
}

#[test]
fn the_game_saves_into_the_srm_file() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("snes-battery");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SEI; CLC; XCE; LDA $700000; INC A; STA $700000; BRA *
    let code = [
        0x78, 0x18, 0xfb, 0xaf, 0x00, 0x00, 0x70, 0x1a, 0x8f, 0x00, 0x00, 0x70, 0x80, 0xfe,
    ];
    let cart = dir.join("game.sfc");
    std::fs::write(&cart, image(&code)).unwrap();
    let srm = dir.join("game.srm");

    drop(run(&cart, 2));
    let saved = std::fs::read(&srm).unwrap();
    assert_eq!(saved.len(), 2048);
    assert_eq!(saved[0], 1);
    // The next start counts on from the file.
    drop(run(&cart, 2));
    assert_eq!(std::fs::read(&srm).unwrap()[0], 2);

    // A cartridge without a battery gets no file.
    let mut plain = image(&code);
    plain[0x7fd6] = 0x00;
    plain[0x7fd8] = 0x00;
    let other = dir.join("plain.sfc");
    std::fs::write(&other, plain).unwrap();
    drop(run(&other, 1));
    assert!(!dir.join("plain.srm").exists());
}
