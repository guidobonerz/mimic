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

#[test]
fn joystick_reads_as_keyboard_line_9() {
    use emu_core::input::{Dirs, InputEvent, InputTarget, TimedInput};
    let mut m = stub("6128");
    m.input(TimedInput {
        at: m.now(),
        target: InputTarget::Port(JOY0),
        event: InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    let mut mem = m.mem.clone();
    m.io.write(&mut mem, 0xf792, 0x92); // A in
    m.io.ay.addr = 14;
    m.io.write(&mut mem, 0xf649, 0x49); // read, line 9
                                        // Up (0), right (3), fire 1 (4) closed.
    assert_eq!(m.io.read(0xf400), !0b1_1001);
    m.io.write(&mut mem, 0xf646, 0x46); // line 6: joystick 1 absent
    assert_eq!(m.io.read(0xf400), 0xff);
    assert_eq!(m.ports().len(), 2);
    assert!(m.disconnect(JOY0).is_some());
    assert!(m
        .connect(
            JOY0,
            Box::new(devices_input::Joystick::new(PortKind::AtariJoy))
        )
        .is_err());
}

#[test]
fn ay_tone_reaches_the_audio_output() {
    let mut m = stub("464");
    let mut mem = m.mem.clone();
    let mut ay = |reg: u8, v: u8| {
        m.io.write(&mut mem, 0xf782, 0x82);
        m.io.write(&mut mem, 0xf400, reg);
        m.io.write(&mut mem, 0xf6c0, 0xc0); // latch the register number
        m.io.write(&mut mem, 0xf600, 0x00); // inactive, as the firmware does
        m.io.write(&mut mem, 0xf400, v);
        m.io.write(&mut mem, 0xf680, 0x80);
        m.io.write(&mut mem, 0xf600, 0x00);
    };
    ay(0, 142); // ~440 Hz
    ay(7, 0x3e);
    ay(8, 15);
    assert_eq!(&m.io.ay.regs[..9], &[142, 0, 0, 0, 0, 0, 0, 0x3e, 15]);
    frames(&mut m, 5);
    let mut out = Vec::new();
    m.audio(&mut out);
    // About 960 samples per field; the first field after power-on is short.
    assert!(out.len() > 3 * 960, "{}", out.len());
    let (lo, hi) = (out.iter().min().unwrap(), out.iter().max().unwrap());
    assert!(hi - lo > 5000, "{lo}..{hi}");
}
