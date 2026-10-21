use super::*;

fn stub(model: &str) -> Cpc {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Cpc::new(&config).unwrap()
}

fn frames(m: &mut Cpc, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
    }
}

fn pixel(m: &Cpc, x: usize, y: usize) -> u8 {
    m.io.ga.pixels()[y * gate_array::WIDTH as usize + x]
}

#[test]
fn stub_program_draws_and_counts_interrupts() {
    let mut m = stub("6128");
    frames(&mut m, 20);
    // Screen: 36 lines and 4 µs (64 pixels) of border, then pen 1.
    assert_eq!(pixel(&m, 10, 10), 20, "border black");
    assert_eq!(pixel(&m, 64, 36), 11, "screen white");
    assert_eq!(pixel(&m, 63, 36), 20, "left of the screen");
    assert_eq!(pixel(&m, 64 + 40 * 16, 36), 20, "right of the screen");
    // Six interrupts per field (300 Hz).
    let ints = peek_cpu(&m, 0x8000);
    let before = ints;
    frames(&mut m, 10);
    let after = peek_cpu(&m, 0x8000);
    assert_eq!(after.wrapping_sub(before), 60, "{before} -> {after}");
}

#[test]
fn field_is_20_ms() {
    let mut m = stub("464");
    frames(&mut m, 3);
    let t0 = m.now().0;
    frames(&mut m, 1);
    // 312 lines × 64 µs × 4 T-states, give or take the last instruction.
    let t = m.now().0 - t0;
    assert!((79_860..=79_884).contains(&t), "{t}");
}

#[test]
fn keyboard_reaches_the_ay_port() {
    let mut m = stub("6128");
    m.key(keyboard::keys::SPACE.0, true);
    // Select AY register 14, then read with keyboard line 5.
    let mut mem = m.mem.clone();
    m.io.write(&mut mem, 0xf782, 0x82); // A out
    m.io.write(&mut mem, 0xf40e, 14);
    m.io.write(&mut mem, 0xf6c0, 0xc0); // latch address
    m.io.write(&mut mem, 0xf600, 0x00);
    m.io.write(&mut mem, 0xf792, 0x92); // A in
    m.io.write(&mut mem, 0xf645, 0x45); // read, line 5
    assert_eq!(m.io.read(0xf400), 0x7f, "SPACE is bit 7 of line 5");
    assert_eq!(m.io.read(0xf500) & 0x0e, 0x0e, "Amstrad links");
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub("6128");
    frames(&mut a, 5);
    let state = a.save_state();
    frames(&mut a, 3);
    let mut b = stub("6128");
    b.load_state(&state).unwrap();
    frames(&mut b, 3);
    assert_eq!(a.io.ga.pixels(), b.io.ga.pixels());
    assert_eq!(a.now(), b.now());
    assert_eq!(a.cpu, b.cpu);
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
    let mut c = stub("464");
    assert!(c.load_state(&state).is_err(), "other model");
}
