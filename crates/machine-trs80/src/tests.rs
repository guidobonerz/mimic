use super::*;

fn stub() -> Trs80 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    Trs80::new(&config).unwrap()
}

fn run(m: &mut Trs80, fields: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn stub_writes_its_line_and_sees_enter() {
    let mut m = stub();
    run(&mut m, 5);
    let text = m.screen_text();
    assert!(text.starts_with("TRS-80 STUB ROM "), "{text}");
    assert_eq!(m.peek_cpu(0x3c40), 0x20);
    m.input(TimedInput {
        at: m.now(),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: keyboard::keys::ENTER,
            down: true,
        },
    });
    run(&mut m, 2);
    assert_eq!(m.peek_cpu(0x3c40), 0xbf);
    let lit = m.pixels.iter().filter(|&&p| p != 0).count();
    assert!(lit > 100, "the line and the block are drawn");
}

#[test]
fn field_rate() {
    let mut m = stub();
    run(&mut m, 60);
    // 60 fields of 264 lines of 112 clocks: one second at 1.774 MHz.
    let t = m.now().0;
    assert!((t as i64 - i64::from(CPU_HZ)).abs() < 30, "{t}");
}

#[test]
fn port_ff_selects_32_columns() {
    let mut m = stub();
    run(&mut m, 3);
    let narrow = m.pixels.iter().filter(|&&p| p != 0).count();
    m.port_ff = 0x08;
    run(&mut m, 1);
    let wide = m.pixels.iter().filter(|&&p| p != 0).count();
    assert!(wide > narrow, "{narrow} {wide}");
}

#[test]
fn savestate_round_trip() {
    let mut m = stub();
    run(&mut m, 3);
    let s = m.save_state();
    let mut n = stub();
    n.load_state(&s).unwrap();
    run(&mut m, 2);
    run(&mut n, 2);
    assert_eq!(m.save_state(), n.save_state());
    assert!(n.load_state(&s[..s.len() / 2]).is_err());
}

#[test]
fn missing_rom_is_an_error() {
    let err = Trs80::new(&MachineConfig::default()).err().unwrap();
    assert!(matches!(err, ConfigError::MissingRom(r) if r == "level2"));
}
