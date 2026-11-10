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
