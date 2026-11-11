//! The Model B with the stub MOS: fields, the picture, savestates, the
//! screen address, the keyboard and the addressable latch.

use super::*;

fn stub() -> Bbc {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    Bbc::new(&config).unwrap()
}

fn frames(m: &mut Bbc, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn fields_are_50_hz() {
    let mut m = stub();
    frames(&mut m, 5);
    let start = m.now().0;
    frames(&mut m, 10);
    let per = (m.now().0 - start) / 10;
    assert!((39_500..40_500).contains(&per), "{per} cycles a field");
}

#[test]
fn the_stub_paints_mode_4_top_half_white() {
    let mut m = stub();
    frames(&mut m, 5);
    let fb = &m.board.video.fb;
    let column: Vec<u8> = (0..HEIGHT as usize)
        .map(|y| fb[y * WIDTH as usize + WIDTH as usize / 2])
        .collect();
    let white = column.iter().filter(|&&c| c == 7).count();
    assert_eq!(white, 128, "16 character rows of 8 lines");
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub();
    frames(&mut a, 3);
    let state = a.save_state();
    frames(&mut a, 3);
    let mut b = stub();
    b.load_state(&state).unwrap();
    frames(&mut b, 3);
    assert_eq!(a.board.video.fb, b.board.video.fb);
    assert_eq!(a.now(), b.now());
    assert!(b.load_state(&state[..50]).is_err());
}

#[test]
fn screen_addresses_wrap_and_teletext() {
    let mut m = stub();
    // MODE 0 at &3000: MA = &600.
    m.board.latch = 0x20; // 20 KB
    assert_eq!(m.board.screen_addr(0x0600, 3), 0x3003);
    // Past the end: MA bit 12 wraps back by 20 KB.
    assert_eq!(
        m.board.screen_addr(0x1000, 0),
        0x8000u16.wrapping_sub(0x5000) & 0x7fff
    );
    assert_eq!(m.board.screen_addr(0x2000 | 0x123, 0), 0x7c00 + 0x123);
}

#[test]
fn keyboard_reads_through_port_a_and_ca2() {
    let mut m = stub();
    m.board.keyboard.key(0x41, true); // A: row 4, column 1
                                      // Keyboard enabled (latch bit 3 low): PA7 answers for PA0–6.
    m.board.latch = 0;
    m.board.sysvia.write(3, 0x7f); // DDRA: PA0–6 out
    m.board.sysvia.write(1, 0x41);
    assert_eq!(m.board.sysvia_pa() & 0x80, 0x80);
    m.board.sysvia.write(1, 0x42);
    assert_eq!(m.board.sysvia_pa() & 0x80, 0);
    assert!(m.board.keyboard.any(None));
    assert!(m.board.keyboard.any(Some(1)) && !m.board.keyboard.any(Some(2)));
}

#[test]
fn the_latch_takes_pb0_to_pb3_and_strobes_sound() {
    let mut m = stub();
    let b = &mut m.board;
    b.io_write(0xfe42, 0x0f); // DDRB: PB0–3 out
    b.io_write(0xfe40, 0x08 | 4); // latch bit 4 high
    assert_eq!(b.latch & 0x10, 0x10);
    b.io_write(0xfe40, 0x08); // bit 0 high
    b.io_write(0xfe43, 0xff); // DDRA
    b.io_write(0xfe41, 0x9f); // tone 0 attenuation 15
    b.io_write(0xfe40, 0x00); // bit 0 low: the SN76489 takes port A
    assert_eq!(b.latch & 1, 0);
    assert_eq!(b.psg.atten[0], 15);
}

fn stub_model(model: &str) -> Bbc {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.model = Some(model.into());
    Bbc::new(&config).unwrap()
}

#[test]
fn bplus_shadow_ram_serves_only_the_vdu_driver() {
    let mut m = stub_model("bplus");
    let b = &mut m.board;
    b.write(0x3000, 0x11);
    b.io_write(0xfe34, 0x80);
    // Code outside &C000–&DFFF: main RAM.
    b.vdu = false;
    assert_eq!(b.peek(0x3000), 0x11);
    // The VDU driver: the shadow RAM.
    b.vdu = true;
    b.write(0x3000, 0x22);
    assert_eq!(b.peek(0x3000), 0x22);
    assert_eq!(b.ram[0x3000], 0x11);
    // The display reads the shadow RAM while it is on.
    assert_eq!(b.screen_byte(0x3000), 0x22);
    b.io_write(0xfe34, 0x00);
    assert_eq!(b.peek(0x3000), 0x11, "shadow off");
    assert_eq!(b.screen_byte(0x3000), 0x11);
}

#[test]
fn bplus_private_ram_pages_in_with_romsel_bit_7() {
    let mut m = stub_model("bplus");
    let b = &mut m.board;
    b.io_write(0xfe30, 0x8f);
    b.write(0x8000, 0x5a);
    b.write(0xafff, 0xa5);
    assert_eq!((b.peek(0x8000), b.peek(0xafff)), (0x5a, 0xa5));
    b.io_write(0xfe30, 0x0f);
    assert_ne!(b.peek(0x8000), 0x5a, "the ROM again");
    // The Model B has neither.
    let mut m = stub();
    m.board.io_write(0xfe30, 0x8f);
    m.board.write(0x8000, 0x5a);
    assert_ne!(m.board.peek(0x8000), 0x5a);
}

#[test]
fn bplus128_sideways_ram_is_writable_and_saved() {
    let mut m = stub_model("bplus128");
    m.board.io_write(0xfe30, 12);
    m.board.write(0x8123, 0x77);
    assert_eq!(m.board.peek(0x8123), 0x77);
    let state = m.save_state();
    m.board.write(0x8123, 0);
    m.load_state(&state).unwrap();
    assert_eq!(m.board.peek(0x8123), 0x77);
    // A ROM slot stays read-only.
    m.board.io_write(0xfe30, 2);
    m.board.write(0x8123, 0x77);
    assert_ne!(m.board.peek(0x8123), 0x77);
}

#[test]
fn master_paging_lynne_hazel_andy_and_sideways_ram() {
    let mut m = stub_model("master");
    let b = &mut m.board;
    b.write(0x3000, 0x11);
    // X: every access to &3000–&7FFF goes to LYNNE.
    b.io_write(0xfe34, 0x04);
    b.write(0x3000, 0x22);
    assert_eq!((b.peek(0x3000), b.ram[0x3000]), (0x22, 0x11));
    // E: only the VDU driver's.
    b.io_write(0xfe34, 0x02);
    b.vdu = false;
    assert_eq!(b.peek(0x3000), 0x11);
    b.vdu = true;
    assert_eq!(b.peek(0x3000), 0x22);
    // D: the display.
    assert_eq!(b.screen_byte(0x3000), 0x11);
    b.io_write(0xfe34, 0x01);
    assert_eq!(b.screen_byte(0x3000), 0x22);
    // ACCCON reads back.
    assert_eq!(b.io_peek(0xfe34), 0x01);
    // Y: HAZEL over the VDU driver.
    let rom = b.peek(0xc000);
    b.io_write(0xfe34, 0x08);
    b.write(0xc000, rom ^ 0xff);
    assert_eq!(b.peek(0xc000), rom ^ 0xff);
    b.io_write(0xfe34, 0x00);
    assert_eq!(b.peek(0xc000), rom);
    // ROMSEL bit 7: ANDY at &8000–&8FFF only.
    b.io_write(0xfe30, 0x80 | 9);
    b.write(0x8000, 0x33);
    assert_eq!(b.peek(0x8000), 0x33);
    b.write(0x9000, 0x44);
    assert_ne!(b.peek(0x9000), 0x44, "&9000 is the ROM's");
    // Banks 4–7 are sideways RAM.
    b.io_write(0xfe30, 5);
    b.write(0x9000, 0x44);
    assert_eq!(b.peek(0x9000), 0x44);
    // IRR pulls the IRQ line.
    b.io_write(0xfe34, 0x80);
    assert!(b.irr());
}

#[test]
fn master_savestates_keep_the_cmos_ram_and_the_clock() {
    let mut m = stub_model("master");
    frames(&mut m, 3);
    m.board.rtc.as_mut().unwrap().regs[20] = 0x5a;
    m.board.io_write(0xfe34, 0x0a);
    let state = m.save_state();
    frames(&mut m, 60);
    let later = m.board.rtc.as_ref().unwrap().regs[0];
    m.board.rtc.as_mut().unwrap().regs[20] = 0;
    m.load_state(&state).unwrap();
    assert_eq!(m.board.rtc.as_ref().unwrap().regs[20], 0x5a);
    assert_eq!(m.board.acccon, 0x0a);
    frames(&mut m, 60);
    assert_eq!(
        m.board.rtc.as_ref().unwrap().regs[0],
        later,
        "deterministic"
    );
    // A new Master comes configured: LANG 12, FILE 9.
    assert_eq!(stub_model("master").board.rtc.unwrap().regs[14 + 5], 0xc9);
}

#[test]
fn keypad_keys_fall_back_to_the_main_keys_without_a_keypad() {
    let mut b = keyboard::Keyboard::default();
    b.key(0x6b, true); // keypad 1 → 1 (row 3, column 0)
    assert_eq!(b.cols[0], 1 << 3);
    b.key(0x6b, false);
    b.key(0x5b, true); // keypad * needs SHIFT on the main keyboard: nothing
    assert_eq!(b.cols, [0; 16]);
    let mut m = keyboard::Keyboard {
        keypad: true,
        ..keyboard::Keyboard::default()
    };
    m.key(0x5b, true);
    assert_eq!(m.cols[11], 1 << 5);
}

#[test]
fn gfx_views_decode_the_screen_palette_and_charset() {
    let mut m = stub_model("b");
    frames(&mut m, 20);
    let views = m.gfx_views();
    assert_eq!(views.len(), 3);
    let opts = emu_core::GfxOptions::new();
    for v in &views {
        let g = m.gfx_view(v.id, &opts).unwrap();
        assert!(g.width > 0 && g.height > 0, "{}", v.id);
    }
    let charset = m.gfx_view("charset", &opts).unwrap();
    assert_eq!((charset.width, charset.height), (16 * 13 - 1, 12 * 21 - 1));
    let before = m.save_state();
    let _ = m.gfx_view("screen", &opts);
    assert_eq!(m.save_state(), before, "views do not change the machine");
}

#[test]
fn the_break_key_resets_the_cpu_and_leaves_the_matrix_alone() {
    let mut m = stub();
    frames(&mut m, 3);
    let reset = u16::from(m.board.peek(0xfffc)) | u16::from(m.board.peek(0xfffd)) << 8;
    m.board.ram[0x1234] = 0x5a;
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: emu_core::input::MachineKey(keyboard::keys::BREAK),
            down: true,
        },
    });
    let mut seen = false;
    for _ in 0..20 {
        m.step_cycle();
        seen |= m.cpu.pc() == reset;
    }
    assert!(seen, "the CPU went through its reset vector");
    assert_eq!(m.board.keyboard.cols, [0; 16], "BREAK is not SHIFT");
    assert_eq!(m.board.ram[0x1234], 0x5a, "BREAK keeps the RAM");
}
