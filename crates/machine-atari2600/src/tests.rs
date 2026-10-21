use super::*;
use emu_core::input::Dirs;
use emu_core::input::InputEvent;

fn stub(model: &str) -> Atari2600 {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Atari2600::new(&config).unwrap()
}

fn frame(m: &mut Atari2600) {
    assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
}

fn pixel(m: &Atari2600, x: u32, y: u32) -> u8 {
    m.board.tia.pixels()[(y * tia::WIDTH + x) as usize]
}

#[test]
fn stub_frames_are_262_lines_and_lock_to_vsync() {
    let mut m = stub("ntsc");
    frame(&mut m);
    let start = m.now().0;
    frame(&mut m);
    assert_eq!(
        m.now().0 - start,
        262 * 76,
        "one field = 262 lines of 76 cycles"
    );
    assert_eq!(m.beam().unwrap().line, 0, "field starts after VSYNC");
}

#[test]
fn stub_paints_one_background_colour_per_line() {
    let mut m = stub("ntsc");
    frame(&mut m);
    frame(&mut m);
    let first = stub::FIRST_PICTURE_LINE as u32;
    assert_eq!(pixel(&m, 0, first - 1), 0, "VBLANK is black");
    for k in 0..stub::PICTURE_LINES as u32 {
        let want = ((192 - k) >> 1) as u8;
        assert_eq!(pixel(&m, 0, first + k), want, "line {k}");
        assert_eq!(pixel(&m, 159, first + k), want, "line {k}");
    }
    let last = first + stub::PICTURE_LINES as u32;
    assert_eq!(pixel(&m, 80, last), 0, "overscan is blanked");
    assert_eq!(pixel(&m, 80, 261), 0, "VSYNC lines are blanked");
}

#[test]
fn wsync_holds_the_cpu_until_the_next_line() {
    let mut m = stub("ntsc");
    frame(&mut m);
    // Run to the first picture line, then a few cycles into it.
    while m.beam().unwrap().line != stub::FIRST_PICTURE_LINE + 1 {
        m.step_cycle();
    }
    assert!(m.beam().unwrap().cycle < 10, "the program waits on WSYNC");
}

#[test]
fn empty_console_idles_and_free_runs() {
    let mut m = Atari2600::new(&MachineConfig::default()).unwrap();
    frame(&mut m);
    let start = m.now().0;
    frame(&mut m);
    let lines = u64::from(tia::free_run_lines(VideoStandard::Ntsc));
    assert_eq!(m.now().0 - start, lines * 76);
    assert!(m.board.tia.pixels().iter().all(|&p| p == 0));
}

#[test]
fn pal_model_and_bad_options() {
    let m = stub("pal");
    assert_eq!(m.framebuffer().height, 312);
    assert_eq!(m.signal().pixel_clock_hz, 3_546_894);
    let mut config = MachineConfig {
        model: Some("secam".into()),
        ..MachineConfig::default()
    };
    assert!(Atari2600::new(&config).is_err());
    config.model = None;
    config.options.insert("port1".into(), "lightgun".into());
    assert!(Atari2600::new(&config).is_err());
}

#[test]
fn joystick_reaches_swcha_and_inpt4() {
    let mut m = stub("ntsc");
    let now = m.now();
    m.input(TimedInput {
        at: now,
        target: InputTarget::Port(PORT1),
        event: InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        },
    });
    m.step_cycle();
    let swcha = m.peek(CPU_SPACE, 0x280, Size::Byte).unwrap();
    assert_eq!(swcha, 0b0110_1111, "up (bit 4) and right (bit 7) low");
    assert_eq!(
        m.peek(CPU_SPACE, 0x0c, Size::Byte).unwrap() & 0x80,
        0,
        "INPT4 low"
    );
    assert_eq!(m.peek(CPU_SPACE, 0x0d, Size::Byte).unwrap() & 0x80, 0x80);
}

#[test]
fn address_decoding_and_mirrors() {
    let mut m = stub("ntsc");
    assert!(m.poke(CPU_SPACE, 0x80, Size::Byte, 0x5a));
    assert_eq!(
        m.peek(CPU_SPACE, 0x180, Size::Byte),
        Some(0x5a),
        "RAM mirror"
    );
    assert_eq!(
        m.peek(CPU_SPACE, 0xe080, Size::Byte),
        Some(0x5a),
        "A13–A15 ignored"
    );
    assert_eq!(
        m.peek(CPU_SPACE, 0xf000, Size::Byte),
        Some(0x78),
        "cartridge"
    );
    assert_eq!(m.peek(CPU_SPACE, 0x1000, Size::Byte), Some(0x78));
    assert!(!m.poke(CPU_SPACE, 0xf000, Size::Byte, 0));
    assert_eq!(m.disassemble(0xf000).mnemonic.to_ascii_uppercase(), "SEI");
}

#[test]
fn savestate_round_trip_continues_identically() {
    let mut a = stub("ntsc");
    frame(&mut a);
    for _ in 0..1234 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = stub("ntsc");
    b.load_state(&state).unwrap();
    frame(&mut a);
    frame(&mut b);
    assert_eq!(a.now(), b.now());
    assert_eq!(a.board, b.board);
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
    assert!(b.load_state(b"garbage").is_err());
}
