use emu_core::input::{Dirs, InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::port::PortId;
use emu_core::registry::MachineConfig;
use emu_core::DebugHooks;

#[test]
#[ignore]
fn run() {
    let rom = std::env::var("VCS_ROM").unwrap();
    let out = std::env::var("VCS_OUT").unwrap();
    let mut config = MachineConfig::default();
    if let Ok(m) = std::env::var("VCS_MODEL") {
        config.model = Some(m);
    }
    let bytes = std::fs::read(&rom).unwrap();
    eprintln!(
        "SCHEME {:?}",
        devices_cartridge::atari2600::detect(&bytes).map(|(s, sc)| (s.name(), sc))
    );
    let mut m = emu_machines::registry()
        .create("atari2600", &config)
        .unwrap();
    let media = media_backends::image::open_cart(std::path::Path::new(&rom)).unwrap();
    m.media_slot(emu_core::media::SlotId(0))
        .unwrap()
        .insert(media)
        .unwrap();
    let mut dbg = DebugHooks::new();
    let mut audio = Vec::new();
    let (mut dirs, mut fire) = (Dirs::NONE, 0u8);
    for frame in 0..=900u64 {
        let now = m.now();
        let key = |down| TimedInput {
            at: now,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(0),
                down,
            },
        };
        match frame {
            60 => m.input(key(true)),
            70 => m.input(key(false)),
            _ => {}
        }
        let before = (dirs, fire);
        match frame {
            300 => fire = 1,
            310 => fire = 0,
            400 => dirs = Dirs::RIGHT,
            500 => dirs = Dirs::UP,
            600 => dirs = Dirs::NONE,
            _ => {}
        }
        if (dirs, fire) != before {
            m.input(TimedInput {
                at: now,
                target: InputTarget::Port(PortId(0)),
                event: InputEvent::Digital {
                    dirs,
                    buttons: fire,
                },
            });
        }
        if [250, 450, 650, 900].contains(&frame) {
            std::fs::write(
                format!("{out}-{frame}.png"),
                emu_preview::encode_png(&m.framebuffer()),
            )
            .unwrap();
        }
        let stop = m.run_frame(&mut dbg);
        assert_eq!(stop, emu_core::StopReason::FrameDone, "frame {frame}");
        m.audio(&mut audio);
    }
    let loud = audio.iter().filter(|&&s| s > 2000).count();
    eprintln!("AUDIO samples={} non-silent={}", audio.len(), loud);
}
