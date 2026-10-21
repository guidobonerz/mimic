//! CPC discs: SAVE a BASIC program onto a blank DATA disc with AMSDOS,
//! LOAD it back and run it; the file is in the image afterwards (ROMs via
//! `EMU_ROM_DIR`).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test cpc_disc -- --ignored

use emu_core::machine::Machine;
use emu_core::media::{Media, MediaKind};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};
use media::dsk::Dsk;
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

fn type_text(m: &mut dyn Machine, text: &str, settle: u64) {
    let (events, end) = machine_cpc::keyboard::typed_events(text, m.now().0);
    for e in events {
        m.input(e);
    }
    let fields = (end - m.now().0) / 79_872 + settle;
    run(m, fields as usize);
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

#[test]
#[ignore = "needs CPC ROMs with AMSDOS in EMU_ROM_DIR"]
fn save_and_load_a_program_on_disc() {
    for model in ["6128", "664"] {
        let mut m = boot(model);
        m.media_slot(machine_cpc::SLOT_A)
            .unwrap()
            .insert(Media {
                name: "blank".into(),
                kind: MediaKind::Disk,
                backend: Box::new(MemoryBackend {
                    data: media::dsk::blank_data(40),
                }),
            })
            .unwrap();
        type_text(m.as_mut(), "10 poke &4000,77\r", 25);
        type_text(m.as_mut(), "save \"test\"\r", 150);
        type_text(m.as_mut(), "new\r", 25);
        type_text(m.as_mut(), "load \"test\"\r", 150);
        type_text(m.as_mut(), "run\r", 25);
        let shot = snapshot(m.as_ref(), &format!("cpc_disc_{model}.png"));
        assert_eq!(byte(m.as_mut(), 0x4000), 77, "{model}: snapshot at {shot}");
        type_text(m.as_mut(), "cat\r", 150);
        snapshot(m.as_ref(), &format!("cpc_cat_{model}.png"));

        // The directory (track 0, sector &C1) names TEST.BAS.
        let media = m.media_slot(machine_cpc::SLOT_A).unwrap().eject().unwrap();
        let mut backend = media.backend;
        let mut bytes = vec![0; backend.len() as usize];
        backend.read(0, &mut bytes).unwrap();
        let dsk = Dsk::parse(&bytes).unwrap();
        let dir = dsk
            .track(0, 0)
            .unwrap()
            .sectors
            .iter()
            .find(|s| s.id[2] == 0xc1)
            .copied()
            .unwrap();
        let entry = &dsk.data(&dir)[..32];
        assert_eq!(&entry[1..12], b"TEST    BAS", "{model}: {entry:02x?}");
    }
}
