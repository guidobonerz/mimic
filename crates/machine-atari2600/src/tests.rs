use super::*;
use emu_core::input::Dirs;
use emu_core::input::{InputEvent, MachineKey};
use emu_core::media::MediaKind;

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

/// A machine running `program` from `$F000` (4 KB cartridge).
fn with_program(program: &[u8]) -> Atari2600 {
    let mut rom = vec![0xea; 0x1000];
    rom[..program.len()].copy_from_slice(program);
    rom[0xffc..].copy_from_slice(&[0x00, 0xf0, 0x00, 0xf0]);
    let mut m = Atari2600::new(&MachineConfig::default()).unwrap();
    m.board.cart = Some(Cart2600::parse(&rom).unwrap());
    m.reset(ResetKind::Soft);
    m
}

#[test]
fn resp0_in_cpu_cycle_23_puts_the_player_at_pixel_6() {
    let mut m = with_program(&[
        0x78, 0xd8, // SEI, CLD
        0xa9, 0x80, 0x85, 0x1b, // LDA #$80, STA GRP0
        0xa9, 0x1e, 0x85, 0x06, // LDA #$1E, STA COLUP0
        0x85, 0x02, // STA WSYNC: the next instruction starts in cycle 0
        0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, 0xea, // 9 × NOP: 18 cycles
        0xa5, 0x80, // LDA $80: 3 cycles
        0x85, 0x10, // STA RESP0: written in cycle 23
        0x4c, 0x19, 0xf0, // JMP *
    ]);
    while m.beam().unwrap().line < 4 {
        m.step_cycle();
    }
    let row = |y: u32| -> Vec<u32> {
        (0..tia::WIDTH)
            .filter(|&x| pixel(&m, x, y) == 0x1e >> 1)
            .collect()
    };
    // Line 1 still shows the copy started from the power-up count.
    assert_eq!(row(2), [6]);
    assert_eq!(row(3), [6], "kept on the following lines");
}

#[test]
fn tia_sound_reaches_the_audio_output() {
    let mut m = with_program(&[
        0xa9, 0x04, 0x85, 0x15, // LDA #4 (÷2 tone), STA AUDC0
        0xa9, 0x0f, 0x85, 0x19, // LDA #15, STA AUDV0
        0xa9, 0x1f, 0x85, 0x17, // LDA #31, STA AUDF0: 31.4 kHz / 32 / 2 ≈ 490 Hz
        0x4c, 0x0c, 0xf0, // JMP *
    ]);
    let mut samples = Vec::new();
    for _ in 0..30 {
        m.run_frame(&mut DebugHooks::new());
        m.audio(&mut samples);
    }
    // 30 free-running fields of 302 lines at 76 cycles: ≈ 0.58 s of sound.
    let rate = f64::from(emu_core::SAMPLE_RATE);
    assert!((samples.len() as f64 / rate - 0.577).abs() < 0.01);
    let high = samples.iter().filter(|&&s| s > 10_000).count();
    let low = samples.iter().filter(|&&s| s < 5_000).count();
    assert!(
        high > samples.len() / 3 && low > samples.len() / 3,
        "a square wave"
    );
    // Count rising edges: ≈ 490 Hz.
    let rises = samples
        .windows(2)
        .filter(|w| w[0] < 7_500 && w[1] >= 7_500)
        .count();
    let hz = rises as f64 / (samples.len() as f64 / rate);
    assert!((hz - 490.6).abs() < 10.0, "{hz} Hz");
}

fn with_options(options: &[(&str, &str)]) -> Atari2600 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Atari2600::new(&config).unwrap()
}

fn send(m: &mut Atari2600, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
    m.step_cycle();
}

fn key(m: &mut Atari2600, key: MachineKey, down: bool) {
    send(
        m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: key,
            down,
        },
    );
}

fn peek(m: &Atari2600, addr: u32) -> u8 {
    m.peek(CPU_SPACE, addr, Size::Byte).unwrap() as u8
}

#[test]
fn console_switches_on_swchb() {
    let mut m = with_options(&[("left", "a"), ("tv", "bw")]);
    assert_eq!(peek(&m, 0x282), 0x37 | 0x40, "left A, black and white");
    key(&mut m, keys::RESET, true);
    assert_eq!(peek(&m, 0x282) & 1, 0, "RESET pressed");
    key(&mut m, keys::RESET, false);
    key(&mut m, keys::SELECT, true);
    assert_eq!(peek(&m, 0x282) & 3, 1);
    key(&mut m, keys::SELECT, false);
    key(&mut m, keys::TV_TYPE, true);
    key(&mut m, keys::TV_TYPE, false);
    key(&mut m, keys::RIGHT_DIFFICULTY, true);
    assert_eq!(peek(&m, 0x282), 0x37 | 0x08 | 0x40 | 0x80, "toggled");
    let state = m.save_state();
    let mut back = with_options(&[]);
    back.load_state(&state).unwrap();
    assert_eq!(peek(&back, 0x282), peek(&m, 0x282));
    let mut config = MachineConfig::default();
    config.options.insert("tv".into(), "secam".into());
    assert!(Atari2600::new(&config).is_err());
}

#[test]
fn paddles_charge_inpt0_and_fire_on_swcha() {
    let mut m = with_options(&[("port1", "paddles"), ("port2", "none")]);
    let port = InputTarget::Port(PORT1);
    // Paddle 0 is the pot on pin 5 (device axis 1), its button pin 4.
    send(&mut m, port, InputEvent::Analog { axis: 1, value: 0 });
    send(
        &mut m,
        port,
        InputEvent::Analog {
            axis: 0,
            value: 0xfffe,
        },
    );
    send(
        &mut m,
        port,
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 2,
        },
    );
    assert_eq!(peek(&m, 0x280) & 0xc0, 0x40, "paddle 0 button: SWCHA D7");
    m.poke(CPU_SPACE, 0x01, Size::Byte, 0x80); // dump
    m.poke(CPU_SPACE, 0x01, Size::Byte, 0x00); // release
    for _ in 0..76 * 10 {
        m.step_cycle();
    }
    assert_eq!(peek(&m, 0x08) & 0x80, 0x80, "INPT0: no resistance");
    assert_eq!(peek(&m, 0x09) & 0x80, 0, "INPT1: full turn, still charging");
    assert_eq!(peek(&m, 0x0a) & 0x80, 0, "nothing in the right port");
}

#[test]
fn keypad_rows_and_columns() {
    let mut m = with_options(&[("port1", "keypad")]);
    m.poke(CPU_SPACE, 0x281, Size::Byte, 0xf0); // SWACNT: D4–D7 outputs
    m.poke(CPU_SPACE, 0x280, Size::Byte, 0xe0); // row 1 (1 2 3) low
    key(&mut m, MachineKey(keys::LEFT_KEYPAD + 1), true); // "2"
    let col = |m: &Atari2600| [0x08, 0x09, 0x0c].map(|a| peek(m, a) >> 7);
    assert_eq!(col(&m), [1, 0, 1], "column 2 on INPT1");
    m.poke(CPU_SPACE, 0x280, Size::Byte, 0xd0); // row 2 (4 5 6)
    m.step_cycle();
    assert_eq!(col(&m), [1, 1, 1], "key not in the selected row");
    key(&mut m, MachineKey(keys::LEFT_KEYPAD + 5), true); // "6"
    assert_eq!(col(&m), [1, 1, 0], "column 3 on INPT4");
}

#[test]
fn driving_controller_gray_code() {
    let mut m = with_options(&[("port2", "driving")]);
    let port = InputTarget::Port(PORT2);
    let code = |m: &Atari2600| peek(m, 0x280) & 0x03;
    let mut seen = vec![code(&m)];
    for _ in 0..4 {
        send(
            &mut m,
            port,
            InputEvent::MouseRel {
                dx: 8,
                dy: 0,
                buttons: 0,
            },
        );
        seen.push(code(&m));
    }
    assert_eq!(seen, [3, 1, 0, 2, 3], "Gray code on SWCHA D0/D1");
    send(
        &mut m,
        port,
        InputEvent::MouseRel {
            dx: 0,
            dy: 0,
            buttons: 1,
        },
    );
    assert_eq!(peek(&m, 0x0d) & 0x80, 0, "fire on INPT5");
}

/// A machine with `rom` in the cartridge slot (as a user inserts it).
fn with_cart(rom: Vec<u8>, options: &[(&str, &str)]) -> Atari2600 {
    use emu_core::media::{Media, MediaBackend, MediaError};
    struct Mem(Vec<u8>);
    impl MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }
        fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }
    let mut config = MachineConfig::default();
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    let mut m = Atari2600::new(&config).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "test.a26".into(),
            backend: Box::new(Mem(rom)),
        })
        .unwrap();
    m.step_cycle();
    m
}

#[test]
fn f8_program_switches_banks_and_uses_the_superchip() {
    let mut rom = vec![0u8; 0x2000];
    // Bank 1 (power-up), at $F100 (the first 256 bytes are the RAM ports):
    // LDA $1FF8 switches to bank 0, which continues at the next address:
    // store $42 into the Superchip and read it back.
    rom[0x1100..0x1103].copy_from_slice(&[0xad, 0xf8, 0x1f]);
    rom[0x1ffc..0x1ffe].copy_from_slice(&[0x00, 0xf1]);
    rom[0x0103..0x0111].copy_from_slice(&[
        0xa9, 0x42, // LDA #$42
        0x8d, 0x05, 0xf0, // STA $F005 (Superchip write port)
        0xad, 0x85, 0xf0, // LDA $F085 (read port)
        0x85, 0x80, // STA $80
        0x4c, 0x0d, 0xf1, // JMP *
        0x00,
    ]);
    let mut m = with_cart(rom, &[("bank", "f8sc")]);
    for _ in 0..100 {
        m.step_cycle();
    }
    assert_eq!(peek(&m, 0x80), 0x42);
    let state = m.save_state();
    m.load_state(&state).unwrap();
}

#[test]
fn bank_option_is_checked() {
    let mut config = MachineConfig::default();
    config.options.insert("bank".into(), "x9".into());
    assert!(Atari2600::new(&config).is_err());
}

#[test]
fn fe_switches_on_jsr_and_rts() {
    let mut rom = vec![0u8; 0x2000];
    // Bank 1 (power-up) runs at $Dxxx, bank 0 at $Fxxx.
    rom[0x1000..0x100d].copy_from_slice(&[
        0xa2, 0xff, // D000 LDX #$FF
        0x9a, // D002 TXS
        0x20, 0x10, 0xf0, // D003 JSR $F010: bank 0
        0xa9, 0x66, // D006 LDA #$66 (back in bank 1)
        0x85, 0x82, // D008 STA $82
        0x4c, 0x0a, 0xd0, // D00A JMP *
    ]);
    rom[0x1ffc..0x1ffe].copy_from_slice(&[0x00, 0xd0]);
    rom[0x0010..0x0015].copy_from_slice(&[0xa9, 0x55, 0x85, 0x81, 0x60]); // LDA #$55, STA $81, RTS
    let mut m = with_cart(rom, &[("bank", "fe")]);
    for _ in 0..100 {
        m.step_cycle();
    }
    assert_eq!((peek(&m, 0x81), peek(&m, 0x82)), (0x55, 0x66));
}

#[test]
fn gfx_views_show_the_tia_registers() {
    let mut m = stub("ntsc");
    frame(&mut m);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["objects", "palette"]);
    m.board.tia.write(tia::COLUPF, 0x86);
    m.board.tia.write(tia::PF1, 0x80); // playfield bit 4
    let v = m.gfx_view("objects", &GfxOptions::new()).unwrap();
    assert_eq!((v.width, v.height), (160, 9 * 9 - 1));
    assert_eq!(v.space, CPU_SPACE);
    assert_eq!(v.param("pf1"), Some(0x80));
    assert_eq!(v.raw.len(), 0x2d + 3);
    assert_eq!(v.raw[usize::from(tia::PF1)], 0x80);
    let pf_strip = 7 * 9;
    match &v.pixels {
        emu_core::gfx::GfxPixels::Indexed { pixels, .. } => {
            assert_eq!(pixels[(pf_strip * 160 + 16) as usize], 0x43);
        }
        _ => panic!("indexed"),
    }
    let p = m.gfx_view("palette", &GfxOptions::new()).unwrap();
    assert_eq!(p.param("count"), Some(128));
    assert!(m.gfx_view("nope", &GfxOptions::new()).is_none());
}

#[test]
fn overlay_marks_position_strobes_and_hmove() {
    let mut m = stub("ntsc");
    assert_eq!(m.overlays().len(), 1);
    assert!(m.overlay("objects").is_none(), "off by default");
    m.set_overlays(&["objects"]);
    frame(&mut m);
    assert!(m.overlay("objects").unwrap().shapes.is_empty());
    while m.board.tia.line() != 50 || m.board.tia.clock() < 120 {
        m.step_cycle();
    }
    let clock = m.board.tia.clock();
    m.board.write(u16::from(tia::RESP0), 0);
    m.board.write(u16::from(tia::HMOVE), 0);
    m.board.write(u16::from(tia::COLUBK), 0); // not a strobe
    let so_far = m.overlay("objects").unwrap();
    assert_eq!(so_far.shapes.len(), 2, "mid-field: the field so far");
    frame(&mut m);
    let o = m.overlay("objects").unwrap();
    assert_eq!(
        o.shapes[0],
        emu_core::overlay::Shape::Mark {
            x: i32::from(clock - tia::HBLANK),
            y: 50,
            color: emu_core::overlay::colors::RED,
            label: Some("RESP0".into()),
        }
    );
    assert!(matches!(&o.shapes[1],
        emu_core::overlay::Shape::Mark { label: Some(l), .. } if l == "HMOVE"));
    frame(&mut m);
    assert!(m.overlay("objects").unwrap().shapes.is_empty());
    m.set_overlays(&[]);
    assert!(m.overlay("objects").is_none());
    assert!(m.board.overlay.is_none());
}

#[test]
fn frame_rows_and_savestates_ignore_the_overlay() {
    let ntsc = stub("ntsc");
    assert_eq!(ntsc.frame_row(0), Some(0));
    assert_eq!(ntsc.frame_row(261), Some(261));
    assert_eq!(ntsc.frame_row(262), None);
    assert_eq!(stub("pal").frame_row(311), Some(311));
    let mut a = stub("ntsc");
    let mut b = stub("ntsc");
    b.set_overlays(&["objects"]);
    frame(&mut a);
    frame(&mut b);
    b.board.write(u16::from(tia::RESBL), 0);
    a.board.write(u16::from(tia::RESBL), 0);
    assert_eq!(a.save_state(), b.save_state());
    b.load_state(&a.save_state()).unwrap();
    assert!(
        b.overlay("objects").unwrap().shapes.is_empty(),
        "cleared on load"
    );
}
