//! Atari 800XL boot with real ROMs (user-supplied via `EMU_ROM_DIR`, e.g.
//! an `atari/` subfolder with the XL OS and BASIC).
//!
//!     EMU_ROM_DIR=~/roms cargo test -p emu-tests --test atari_boot -- --ignored

use emu_core::machine::Machine;
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, DeviceId, Size, StopReason};

fn boot(model: &str, basic: &str) -> Box<dyn Machine> {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "atari8").expect("ROMs"),
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("basic".into(), basic.into());
    let mut m = emu_machines::registry().create("atari8", &config).unwrap();
    // Without a disk drive the OS retries its boot command (STATUS to
    // D1:) until it gives up: about 250 fields.
    run(m.as_mut(), 350);
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
    cpu.peek(machine_atari8::CPU_SPACE, addr, Size::Byte)
        .unwrap() as u8
}

/// The 40 × 24 text screen the OS shows (SAVMSC at `$58`).
fn screen(m: &mut dyn Machine) -> Vec<u8> {
    let base = u32::from(byte(m, 0x58)) | (u32::from(byte(m, 0x59)) << 8);
    (base..base + 40 * 24).map(|a| byte(m, a)).collect()
}

/// Internal screen codes of `text` (ATASCII $20–$5F minus $20).
fn codes(text: &str) -> Vec<u8> {
    text.bytes().map(|b| b - 0x20).collect()
}

fn find(screen: &[u8], text: &str) -> Option<usize> {
    let pat = codes(text);
    screen.windows(pat.len()).position(|w| w == pat)
}

fn snapshot(m: &dyn Machine, name: &str) -> String {
    let png = emu_preview::encode_png(&m.framebuffer());
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&out, png).unwrap();
    out.display().to_string()
}

#[test]
#[ignore = "needs Atari XL OS and BASIC ROMs in EMU_ROM_DIR"]
fn atari800xl_boots_to_basic_and_runs_a_line() {
    for model in ["800xl", "800xl-ntsc"] {
        let mut m = boot(model, "on");
        let s = screen(m.as_mut());
        let shot = snapshot(m.as_ref(), &format!("atari_boot_{model}.png"));
        let ready = find(&s, "READY");
        assert!(
            ready.is_some(),
            "{model}: READY missing; snapshot at {shot}"
        );

        // The OS reads the keyboard through POKEY's scan and IRQ.
        let (events, end) = machine_atari8::keyboard::typed_events("PRINT 6*7\r", m.now().0);
        for e in events {
            m.input(e);
        }
        while m.now().0 < end + 200_000 {
            run(m.as_mut(), 1);
        }
        let s = screen(m.as_mut());
        let shot = snapshot(m.as_ref(), &format!("atari_print_{model}.png"));
        let typed = find(&s, "PRINT 6*7").map(|p| p / 40);
        let result = find(&s, "42").map(|p| p / 40);
        assert!(
            typed.is_some(),
            "{model}: PRINT not echoed; snapshot at {shot}"
        );
        assert_eq!(result, typed.map(|l| l + 1), "{model}: 42 below; {shot}");
    }
}

#[test]
#[ignore = "needs Atari XL OS ROM in EMU_ROM_DIR"]
fn without_basic_the_xl_starts_the_self_test() {
    // OPTION held at power-up: no BASIC, no cartridge, no disk — the XL OS
    // runs the self-test from the ROM banked in at $5000.
    let mut m = boot("800xl", "off");
    let shot = snapshot(m.as_ref(), "atari_selftest.png");
    let portb = byte(m.as_mut(), 0xd301);
    assert_eq!(
        portb & 0x82,
        0x02,
        "self-test in, BASIC out ({portb:02X}); {shot}"
    );
    let pc = m
        .debug_target(DeviceId("cpu"))
        .unwrap()
        .regs()
        .get("PC")
        .unwrap();
    assert!(
        (0x5000..0x5800).contains(&pc) || pc >= 0xc000,
        "PC {pc:04X}; {shot}"
    );
    assert!(find(&screen(m.as_mut()), "READY").is_none(), "{shot}");
}

#[test]
#[ignore = "needs Atari XL OS and BASIC ROMs in EMU_ROM_DIR"]
fn basic_reads_stick_and_paddle_and_plays_sound() {
    use emu_core::input::{Dirs, InputEvent, InputTarget, TimedInput};
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let mut config = MachineConfig {
        roms: media_backends::roms::load(&dir, "atari8").expect("ROMs"),
        ..MachineConfig::default()
    };
    config.options.insert("port2".into(), "paddles".into());
    let mut m = emu_machines::registry().create("atari8", &config).unwrap();
    run(m.as_mut(), 350);
    let at = m.now();
    let ev = |target, event| TimedInput { at, target, event };
    m.input(ev(
        InputTarget::Port(machine_atari8::PORT1),
        InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 0,
        },
    ));
    m.input(ev(
        InputTarget::Port(machine_atari8::PORT2),
        InputEvent::Analog {
            axis: 0,
            value: 0x8000,
        },
    ));
    let (events, end) = machine_atari8::keyboard::typed_events(
        "PRINT STICK(0);\" \";PADDLE(2)\r",
        m.now().0 + 1000,
    );
    for e in events {
        m.input(e);
    }
    while m.now().0 < end + 200_000 {
        run(m.as_mut(), 1);
    }
    let s = screen(m.as_mut());
    let shot = snapshot(m.as_ref(), "atari_stick.png");
    let typed = find(&s, "PRINT STICK").map(|p| p / 40);
    assert!(typed.is_some(), "not echoed; {shot}");
    assert_eq!(
        find(&s, "7 114").map(|p| p / 40),
        typed.map(|l| l + 1),
        "{shot}"
    );

    // SOUND 0,121,10,8: pure tone on channel 1, heard on the audio output.
    let (events, end) = machine_atari8::keyboard::typed_events("SOUND 0,121,10,8\r", m.now().0);
    for e in events {
        m.input(e);
    }
    let mut out = Vec::new();
    while m.now().0 < end + 100_000 {
        run(m.as_mut(), 1);
        out.clear();
        m.audio(&mut out);
    }
    let (min, max) = (out.iter().min().unwrap(), out.iter().max().unwrap());
    assert!(max - min > 1000, "a tone: {min}..{max}");
}

/// Phase 9: the other models boot with real ROMs — the 130XE to BASIC's
/// READY (its OS is the XL's), the 800 with OS-B and the BASIC cartridge
/// to READY, without the cartridge to the OS's memo pad.
#[test]
#[ignore = "needs the Atari XL OS, OS-B and BASIC ROMs in EMU_ROM_DIR"]
fn other_models_boot() {
    let dir = media_backends::roms::env_dir().expect("EMU_ROM_DIR not set");
    let roms = media_backends::roms::load(&dir, "atari8").expect("ROMs");
    for (model, basic, expect) in [
        ("130xe", "on", "READY"),
        ("800", "on", "READY"),
        ("800", "off", "MEMO PAD"),
        ("400", "on", "READY"),
    ] {
        if model != "130xe" && !roms.contains_key("osb") {
            eprintln!("no OS-B ROM: skipping the {model}");
            continue;
        }
        let mut config = MachineConfig {
            roms: roms.clone(),
            model: Some(model.into()),
            ..MachineConfig::default()
        };
        config.options.insert("basic".into(), basic.into());
        let mut m = emu_machines::registry().create("atari8", &config).unwrap();
        run(m.as_mut(), 350);
        let s = screen(m.as_mut());
        let path = snapshot(m.as_ref(), &format!("atari-{model}-{basic}.png"));
        assert!(
            find(&s, expect).is_some(),
            "{model} basic={basic}: no '{expect}' on the screen, see {path}"
        );
        eprintln!("{model} basic={basic}: {expect} — {path}");
    }
}
