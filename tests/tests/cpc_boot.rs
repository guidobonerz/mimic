//! Amstrad CPC 464/664/6128 boot with real ROMs (user-supplied via
//! `EMU_ROM_DIR`, e.g. a `cpc/` subfolder with OS, BASIC and AMSDOS).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test cpc_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};

fn boot(model: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let config = MachineConfig {
        roms: media_backends::roms::load(&dir, "cpc").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    let mut m = emu_machines::registry().create("cpc", &config).unwrap();
    run(m.as_mut(), 150);
    m
}

fn run(m: &mut dyn Machine, fields: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &mut dyn Machine, addr: u32) -> u8 {
    let cpu = m.debug_target(DeviceId("cpu")).unwrap();
    cpu.peek(machine_cpc::CPU_SPACE, addr, Size::Byte).unwrap() as u8
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_cpc::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
    // 50 fields per second, 80 000 T-states each.
    let fields = (end - m.now().0) / 79_872 + 25;
    run(m, fields as usize);
}

#[test]
#[ignore = "needs CPC OS/BASIC (and AMSDOS) ROMs in EMU_ROM_DIR"]
fn cpc_models_boot_to_basic_and_run_a_line() {
    for model in ["6128", "664", "464"] {
        let mut m = boot(model);
        type_text(m.as_mut(), "poke &4000,6*7\r");
        let shot = snapshot(m.as_ref(), &format!("cpc_boot_{model}.png"));
        assert_eq!(byte(m.as_mut(), 0x4000), 42, "{model}: snapshot at {shot}");
        // The 6128's second 64 KB: write bank 4 through configuration 4,
        // which shows it at &4000, and back to the normal map.
        if model == "6128" {
            type_text(m.as_mut(), "out &7f00,&c4:poke &4000,99:out &7f00,&c0\r");
            assert_eq!(byte(m.as_mut(), 0x4000), 42, "bank 1 untouched");
            // &8000 is bank 2 in both maps: copy bank 4's byte there.
            type_text(
                m.as_mut(),
                "out &7f00,&c4:poke &8000,peek(&4000):out &7f00,&c0\r",
            );
            let shot = snapshot(m.as_ref(), "cpc_banks.png");
            assert_eq!(
                byte(m.as_mut(), 0x8000),
                99,
                "bank 4 kept its byte; snapshot at {shot}"
            );
        }
    }
}
