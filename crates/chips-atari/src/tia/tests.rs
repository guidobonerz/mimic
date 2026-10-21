use super::*;

const BK: u8 = 0x02; // index 1
const PF: u8 = 0x44; // index 0x22
const C0: u8 = 0x1e; // index 0x0f
const C1: u8 = 0x86; // index 0x43

fn tia() -> Tia {
    let mut t = Tia::new(VideoStandard::Ntsc);
    t.write(COLUBK, BK);
    t.write(COLUPF, PF);
    t.write(COLUP0, C0);
    t.write(COLUP1, C1);
    t
}

/// Run until the beam is at `cc` of `line` (the next write lands there).
fn run_to(t: &mut Tia, line: u16, cc: u8) {
    while !(t.line == line && t.cc == cc) {
        t.tick();
    }
}

/// Pixels of `line` showing `colour`.
fn lit(t: &Tia, line: u16, colour: u8) -> Vec<u32> {
    let row = &t.pixels()[line as usize * WIDTH as usize..][..WIDTH as usize];
    (0..WIDTH)
        .filter(|&x| row[x as usize] == colour >> 1)
        .collect()
}

fn span(from: u32, to: u32) -> Vec<u32> {
    (from..to).collect()
}

#[test]
fn playfield_repeats_and_mirrors() {
    let mut t = tia();
    t.write(PF1, 0x80); // bit 4 of the line: pixels 16–19
    run_to(&mut t, 2, 0);
    assert_eq!(lit(&t, 1, PF), [span(16, 20), span(96, 100)].concat());
    t.write(CTRLPF, 1);
    run_to(&mut t, 3, 0);
    assert_eq!(lit(&t, 2, PF), [span(16, 20), span(140, 144)].concat());
    t.write(PF0, 0x10);
    t.write(PF1, 0);
    t.write(PF2, 0x80);
    t.write(CTRLPF, 0);
    run_to(&mut t, 4, 0);
    assert_eq!(
        lit(&t, 3, PF),
        [span(0, 4), span(76, 80), span(80, 84), span(156, 160)].concat(),
        "PF0 D4 first, PF2 D7 last"
    );
}

#[test]
fn playfield_bits_are_taken_four_clocks_ahead() {
    let mut t = tia();
    t.write(PF1, 0x80);
    // Bit 4 (pixels 16–19, shown from clock 84) is taken at clock 80.
    run_to(&mut t, 1, 80);
    t.write(PF1, 0);
    run_to(&mut t, 2, 0);
    assert_eq!(
        lit(&t, 1, PF),
        Vec::<u32>::new(),
        "written before it was taken"
    );
    t.write(PF1, 0x80);
    run_to(&mut t, 2, 81);
    t.write(PF1, 0);
    run_to(&mut t, 3, 0);
    assert_eq!(lit(&t, 2, PF), span(16, 20), "written after: still shown");
}

#[test]
fn player_reset_positions_and_copies() {
    let mut t = tia();
    t.write(GRP0, 0x80);
    t.write(NUSIZ0, 1); // two copies, close
                        // CPU cycle 23 starts at clock 69 (pixel 1): main copy at pixel 6.
    run_to(&mut t, 0, 69);
    t.write(RESP0, 0);
    run_to(&mut t, 2, 0);
    assert_eq!(
        lit(&t, 0, C0),
        [22],
        "the reset line shows the close copy only"
    );
    assert_eq!(lit(&t, 1, C0), [6, 22]);
    t.write(NUSIZ0, 6); // three copies, medium
    run_to(&mut t, 3, 0);
    assert_eq!(lit(&t, 2, C0), [6, 38, 70]);
    t.write(NUSIZ0, 5); // double width: one pixel later, 2 wide
    t.write(GRP0, 0xc0);
    run_to(&mut t, 4, 0);
    assert_eq!(lit(&t, 3, C0), span(7, 11));
    t.write(NUSIZ0, 0);
    t.write(REFP0, 8);
    run_to(&mut t, 5, 0);
    assert_eq!(lit(&t, 4, C0), [12, 13], "reflected: D7 drawn last");
}

#[test]
fn resets_during_blanking() {
    let mut t = tia();
    t.write(GRP1, 0x80);
    t.write(ENAM0, 2);
    t.write(ENABL, 2);
    run_to(&mut t, 0, 10);
    t.write(RESP1, 0);
    t.write(RESM0, 0);
    run_to(&mut t, 2, 0);
    assert_eq!(lit(&t, 1, C1), [3]);
    assert_eq!(lit(&t, 1, C0), [2], "missile 0 in player 0's colour");
    // The ball starts at once and keeps the position.
    run_to(&mut t, 2, HBLANK + 32);
    t.write(RESBL, 0);
    run_to(&mut t, 4, 0);
    assert_eq!(lit(&t, 2, PF), [36]);
    assert_eq!(lit(&t, 3, PF), [36]);
}

#[test]
fn missile_and_ball_sizes() {
    let mut t = tia();
    t.write(ENAM1, 2);
    t.write(NUSIZ1, 0x30); // 8 wide
    t.write(ENABL, 2);
    t.write(CTRLPF, 0x20); // 4 wide
    run_to(&mut t, 0, HBLANK + 50);
    t.write(RESM1, 0);
    run_to(&mut t, 0, HBLANK + 100);
    t.write(RESBL, 0);
    run_to(&mut t, 2, 0);
    assert_eq!(lit(&t, 1, C1), span(54, 62));
    assert_eq!(lit(&t, 1, PF), span(104, 108));
}

#[test]
fn hmove_moves_and_blanks_the_left_edge() {
    let mut t = tia();
    t.write(GRP0, 0x80);
    run_to(&mut t, 0, HBLANK + 34);
    t.write(RESP0, 0);
    run_to(&mut t, 2, 0);
    assert_eq!(lit(&t, 1, C0), [39]);
    t.write(HMP0 + P0 as u8, 0x10); // one pixel left
    run_to(&mut t, 2, 6); // STA HMOVE right after WSYNC
    t.write(HMOVE, 0);
    run_to(&mut t, 4, 0);
    assert_eq!(lit(&t, 2, C0), [38]);
    assert_eq!(lit(&t, 3, C0), [38], "no HMOVE: stays");
    let row2 = &t.pixels()[2 * WIDTH as usize..][..8];
    assert!(row2.iter().all(|&p| p == 0), "HMOVE comb: 8 black pixels");
    assert_eq!(t.pixels()[3 * WIDTH as usize], BK >> 1);
    // HMxx = -8: the blanking extension alone, 8 pixels right.
    t.write(HMCLR, 0);
    t.write(HMP0, 0x80);
    run_to(&mut t, 4, 6);
    t.write(HMOVE, 0);
    run_to(&mut t, 6, 0);
    assert_eq!(lit(&t, 5, C0), [46]);
}

#[test]
fn hmove_late_in_the_line_moves_left_without_comb() {
    let mut t = tia();
    t.write(GRP0, 0x80);
    run_to(&mut t, 0, HBLANK + 64);
    t.write(RESP0, 0); // pixel 69
    t.write(HMP0, 0); // 0: 8 left without the blanking extension
    run_to(&mut t, 1, 219); // CPU cycle 73
    t.write(HMOVE, 0);
    run_to(&mut t, 3, 0);
    assert_eq!(lit(&t, 1, C0), [69]);
    assert_eq!(lit(&t, 2, C0), [61]);
    assert_eq!(t.pixels()[2 * WIDTH as usize], BK >> 1, "no comb");
}

#[test]
fn changed_motion_value_keeps_the_object_moving() {
    let mut t = tia();
    t.write(GRP0, 0x80);
    run_to(&mut t, 0, HBLANK + 34);
    t.write(RESP0, 0);
    t.write(HMP0, 0x70);
    run_to(&mut t, 2, 6);
    t.write(HMOVE, 0);
    // Step 2 has passed: a value of 2 is never met again.
    run_to(&mut t, 2, 6 + 30);
    t.write(HMP0, 0xa0);
    run_to(&mut t, 5, 0);
    let (a, b) = (lit(&t, 3, C0), lit(&t, 4, C0));
    assert!(!a.is_empty() && !b.is_empty());
    assert_ne!(a, b, "extra motion clocks every 4 colour clocks");
}

#[test]
fn vertical_delay_shows_the_old_graphics() {
    let mut t = tia();
    run_to(&mut t, 0, HBLANK + 34);
    t.write(RESP0, 0);
    t.write(VDELP0, 1);
    t.write(GRP0, 0x80);
    run_to(&mut t, 2, 0);
    assert!(lit(&t, 1, C0).is_empty(), "old GRP0 still 0");
    t.write(GRP1, 0);
    run_to(&mut t, 3, 0);
    assert_eq!(lit(&t, 2, C0), [39], "GRP1 copied new GRP0 into old");
    t.write(ENABL, 2);
    t.write(VDELBL, 1);
    assert!(!t.enabl_old);
    t.write(GRP1, 0);
    assert!(t.enabl_old);
}

#[test]
fn priority_and_score_colours() {
    let mut t = tia();
    t.write(PF1, 0xff); // pixels 16–47 and 96–127
    t.write(GRP0, 0x80);
    run_to(&mut t, 0, HBLANK + 15);
    t.write(RESP0, 0); // pixel 20
    run_to(&mut t, 2, 0);
    assert_eq!(t.pixels()[WIDTH as usize + 20], C0 >> 1, "players in front");
    t.write(CTRLPF, 4);
    run_to(&mut t, 3, 0);
    assert_eq!(
        t.pixels()[2 * WIDTH as usize + 20],
        PF >> 1,
        "playfield in front"
    );
    t.write(CTRLPF, 2);
    t.write(GRP0, 0);
    run_to(&mut t, 4, 0);
    assert_eq!(
        t.pixels()[3 * WIDTH as usize + 16],
        C0 >> 1,
        "score: left half P0"
    );
    assert_eq!(
        t.pixels()[3 * WIDTH as usize + 96],
        C1 >> 1,
        "right half P1"
    );
}

#[test]
fn collisions_latch_until_cleared() {
    let mut t = tia();
    t.write(PF1, 0x02); // bit 10: pixels 40–43
    t.write(GRP0, 0xff);
    run_to(&mut t, 0, HBLANK + 34);
    t.write(RESP0, 0); // pixels 39–46
    run_to(&mut t, 2, 0);
    assert_eq!(t.peek(0x02), 0x80, "CXP0FB: P0-PF");
    assert_eq!(t.peek(0x07), 0);
    t.write(CXCLR, 0);
    assert_eq!(t.peek(0x02), 0);
}

#[test]
fn fire_button_latch() {
    let mut t = tia();
    assert_eq!(t.peek(0x0c), 0x80);
    t.set_input(4, false);
    assert_eq!(t.peek(0x0c), 0);
    t.set_input(4, true);
    t.write(VBLANK, 0x40);
    t.set_input(4, false);
    t.set_input(4, true);
    assert_eq!(t.peek(0x0c), 0, "latched low");
    t.write(VBLANK, 0);
    assert_eq!(t.peek(0x0c), 0x80);
    t.write(VBLANK, 0x40);
    assert_eq!(t.peek(0x0c), 0x80, "latch starts high");
}

#[test]
fn vblank_blanks_but_objects_still_collide() {
    let mut t = tia();
    t.write(VBLANK, 2);
    t.write(PF0, 0xf0);
    t.write(ENABL, 2);
    run_to(&mut t, 0, 20);
    t.write(RESBL, 0);
    run_to(&mut t, 2, 0);
    assert!(t.pixels()[WIDTH as usize..2 * WIDTH as usize]
        .iter()
        .all(|&p| p == 0));
    assert_eq!(t.peek(0x06), 0x80, "CXBLPF");
}

#[test]
fn savestate_round_trip() {
    let mut t = tia();
    t.write(GRP0, 0x5a);
    t.write(HMP0, 0x30);
    run_to(&mut t, 1, 100);
    t.write(RESP0, 0);
    t.write(HMOVE, 0);
    t.tick();
    let mut w = StateWriter::new();
    t.save(&mut w);
    let bytes = w.into_bytes();
    let mut back = Tia::new(VideoStandard::Ntsc);
    back.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(back, t);
    assert!(back.load(&mut StateReader::new(&bytes[..40])).is_err());
}

/// Output transitions of channel 0 over `lines` lines with `audc`, `audf`,
/// and the output sampled after each audio clock.
fn toggles(audc: u8, audf: u8, lines: u32) -> (u32, Vec<bool>) {
    let mut t = tia();
    t.write(AUDC0, audc);
    t.write(AUDF0, audf);
    t.write(AUDV0, 15);
    let mut seq = Vec::new();
    let mut last = t.audio_level() > 0;
    let mut n = 0;
    for _ in 0..lines * u32::from(CLOCKS_PER_LINE) {
        t.tick();
        let now = t.audio_level() > 0;
        if t.cc == 1 || t.cc == 115 {
            seq.push(now);
        }
        n += u32::from(now != last);
        last = now;
    }
    (n, seq)
}

/// Shortest period of `seq` after a settling prefix.
fn period(seq: &[bool]) -> usize {
    let s = &seq[64..];
    (1..s.len() / 2)
        .find(|&p| s.iter().zip(&s[p..]).all(|(a, b)| a == b))
        .unwrap()
}

#[test]
fn pure_tones_divide_the_audio_clock() {
    // Two audio clocks per line: 200 pulses in 100 lines.
    assert_eq!(toggles(0x04, 0, 100).0, 200, "AUDC 4: ÷2");
    assert_eq!(toggles(0x04, 1, 100).0, 100, "AUDF 1 halves the rate");
    assert_eq!(period(&toggles(0x0c, 0, 400).1), 6, "AUDC 12: ÷6");
    assert_eq!(period(&toggles(0x06, 0, 400).1), 31, "AUDC 6: ÷31");
    assert_eq!(period(&toggles(0x0e, 0, 800).1), 93, "AUDC 14: ÷93");
}

#[test]
fn noise_periods() {
    assert_eq!(period(&toggles(0x01, 0, 400).1), 15, "4-bit polynomial");
    assert_eq!(period(&toggles(0x09, 0, 400).1), 31, "5-bit polynomial");
    assert_eq!(period(&toggles(0x08, 0, 2000).1), 511, "9-bit polynomial");
}

#[test]
fn constant_output_and_volume() {
    let mut t = tia();
    t.write(AUDV0, 7);
    t.write(AUDV0 + 1, 9);
    for _ in 0..1000 {
        t.tick();
    }
    assert_eq!(t.audio_level(), 16, "AUDC 0: both channels high");
    t.write(AUDC0, 0x0b);
    t.write(AUDV0 + 1, 0);
    for _ in 0..1000 {
        t.tick();
    }
    assert_eq!(t.audio_level(), 7);
}

#[test]
fn paddles_charge_after_the_dump() {
    let mut t = tia();
    t.set_pot(0, 0x7fff); // centre: 190 lines
    t.set_pot(1, 0); // no resistance
                     // INPT2 has nothing connected; INPT3 is pulled low by a device.
    t.set_pot(3, 0);
    t.set_input(3, false);
    t.write(VBLANK, 0x80);
    run_to(&mut t, 2, 0);
    assert_eq!(t.peek(0x08) | t.peek(0x09), 0, "grounded while dumped");
    t.write(VBLANK, 0);
    run_to(&mut t, 2, 10);
    assert_eq!(t.peek(0x09), 0x80, "no resistance: high at once");
    assert_eq!(t.peek(0x08), 0);
    run_to(&mut t, 2 + 189, 0);
    assert_eq!(t.peek(0x08), 0, "line 189: still charging");
    run_to(&mut t, 2 + 191, 0);
    assert_eq!(t.peek(0x08), 0x80, "line 191: charged");
    assert_eq!(t.peek(0x0a), 0, "open line never charges");
    assert_eq!(t.peek(0x0b), 0, "pulled low");
}
