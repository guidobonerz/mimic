//! CPC cassette: SAVE a BASIC program onto a blank CDT, LOAD it back and
//! run it, with the real firmware (ROMs via `EMU_ROM_DIR`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test cpc_tape -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use media_backends::image::MemoryBackend;

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

fn type_text(m: &mut dyn Machine, text: &str) {
    let (events, end) = machine_cpc::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let fields = (end - m.now().0) / 79_872 + 25;
    run(m, fields as usize);
}

fn tape(m: &mut dyn Machine, command: &str) {
    m.media_slot(machine_cpc::SLOT_TAPE)
        .unwrap()
        .control(command)
        .unwrap();
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

#[test]
#[ignore = "needs CPC ROMs in EMU_ROM_DIR"]
fn save_and_load_a_program_on_tape() {
    for model in ["464", "6128"] {
        let mut m = boot(model);
        if model != "464" {
            // The disc models start on disc.
            type_text(m.as_mut(), "|tape\r");
        }
        m.media_slot(machine_cpc::SLOT_TAPE)
            .unwrap()
            .insert(Media {
                name: "blank".into(),
                kind: MediaKind::Tape,
                backend: Box::new(MemoryBackend {
                    data: drives_other::cpc_tape::blank_tape(2),
                }),
            })
            .unwrap();
        type_text(m.as_mut(), "10 poke &4000,77\r");
        // "Press REC and PLAY then any key:"
        tape(m.as_mut(), "record");
        type_text(m.as_mut(), "save \"test\"\r");
        type_text(m.as_mut(), " ");
        run(m.as_mut(), 600);
        tape(m.as_mut(), "stop");
        tape(m.as_mut(), "rewind");
        type_text(m.as_mut(), "new\r");
        // "Press PLAY then any key:"
        tape(m.as_mut(), "play");
        type_text(m.as_mut(), "load \"test\"\r");
        type_text(m.as_mut(), " ");
        run(m.as_mut(), 600);
        type_text(m.as_mut(), "run\r");
        let shot = snapshot(m.as_ref(), &format!("cpc_tape_{model}.png"));
        assert_eq!(byte(m.as_mut(), 0x4000), 77, "{model}: snapshot at {shot}");
    }
}
