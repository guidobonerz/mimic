use super::*;

fn stub(model: &str) -> Kc85 {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Kc85::new(&config).unwrap()
}

fn run(m: &mut Kc85, fields: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn stub_draws_its_cell_and_counts_interrupts() {
    for model in ["2", "3", "4", "5"] {
        let mut m = stub(model);
        run(&mut m, 3);
        let fg = stub::COLOUR >> 3;
        let bg = 16 + (stub::COLOUR & 7);
        let want: Vec<u8> = (0..8)
            .map(|i| {
                if stub::PIXELS & (0x80 >> i) != 0 {
                    fg
                } else {
                    bg
                }
            })
            .collect();
        assert_eq!(m.pixels[..8], want[..], "model {model}");
        // Timer: 64 · 256 clocks an interrupt, about 2.1 a field.
        let n = m.peek_cpu(stub::COUNTER);
        assert!((5..=7).contains(&n), "model {model}: {n} interrupts");
    }
}

#[test]
fn field_length() {
    for (model, clocks) in [("3", 112 * 312), ("4", 227 * 312 / 2)] {
        let mut m = stub(model);
        run(&mut m, 2);
        let a = m.now().0;
        run(&mut m, 50);
        let per_field = (m.now().0 - a) as f64 / 50.0;
        assert!(
            (per_field - f64::from(clocks)).abs() < 1.0,
            "model {model}: {per_field}"
        );
    }
}

#[test]
fn reset_keeps_ram_and_power_clears_it() {
    let mut m = stub("3");
    run(&mut m, 3);
    assert!(m.mem.poke(0x1234, 0x55));
    m.reset(ResetKind::Soft);
    assert_eq!(m.peek_cpu(0x1234), 0x55);
    assert_eq!(m.cpu.pc, 0xe000);
    m.reset(ResetKind::Hard);
    assert_eq!(m.peek_cpu(0x1234), 0);
    assert_eq!(m.cpu.pc, 0xf000);
    run(&mut m, 3);
    assert!(m.peek_cpu(stub::COUNTER) > 0);
}

#[test]
fn savestate_round_trip_and_determinism() {
    let mut a = stub("4");
    run(&mut a, 3);
    let state = a.save_state();
    let mut b = stub("4");
    b.load_state(&state).unwrap();
    run(&mut a, 5);
    run(&mut b, 5);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.pixels, b.pixels);
    // Another model's state is refused, corrupt data never panics.
    assert!(stub("3").load_state(&state).is_err());
    for cut in (0..state.len()).step_by(97) {
        let _ = b.load_state(&state[..cut]);
    }
}

#[test]
fn a_module_shows_its_structure_byte_and_its_memory() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("slot8".into(), "m022".into());
    let mut m = Kc85::new(&config).unwrap();
    run(&mut m, 1);
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0x0880, Size::Byte), Some(0xf4));
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0x0c80, Size::Byte), Some(0xff));
    config.options.insert("slotc".into(), "m026".into());
    assert!(matches!(
        Kc85::new(&config),
        Err(ConfigError::MissingRom(r)) if r == "m026"
    ));
    config.roms.insert("m026".into(), vec![0x7f; 0x2000]);
    let mut m = Kc85::new(&config).unwrap();
    m.mem.modules.control(0x0c, 0x41);
    assert_eq!(m.peek_cpu(0x4000), 0x7f, "RAM4 is off, the module answers");
}

#[test]
fn sound_follows_the_ctc() {
    let mut m = stub("3");
    run(&mut m, 2);
    let mut out = Vec::new();
    m.audio(&mut out);
    out.clear();
    // Channel 0: timer, prescaler 16, constant 100: a 547 Hz square wave.
    m.ctc.write(0, 0x07);
    m.ctc.write(0, 100);
    run(&mut m, 50);
    let format = m.audio(&mut out);
    assert_eq!(format.channels, 1);
    assert!((47_500..48_500).contains(&out.len()), "{}", out.len());
    let crossings = out
        .windows(2)
        .filter(|w| (w[0] < w[1]) && w[0] < 0 && w[1] >= 0)
        .count();
    assert!(
        (530..560).contains(&crossings),
        "{crossings} periods a second"
    );
}
