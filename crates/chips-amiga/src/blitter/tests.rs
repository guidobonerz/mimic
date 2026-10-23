use super::*;

fn run(b: &mut Blitter, chip: &mut ChipRam) {
    for _ in 0..100_000 {
        b.tick();
        if b.wants_slot() && b.cycle(chip) {
            return;
        }
    }
    panic!("blit did not finish");
}

fn words(chip: &ChipRam, addr: u32, n: u32) -> Vec<u16> {
    (0..n).map(|i| chip.word(addr + 2 * i)).collect()
}

#[test]
fn minterms_select_combinations() {
    assert_eq!(minterm(0xf0, 0x1234, 0, 0), 0x1234, "D = A");
    assert_eq!(minterm(0xcc, 0, 0x00ff, 0), 0x00ff, "D = B");
    assert_eq!(minterm(0xaa, 0, 0, 0xf00f), 0xf00f, "D = C");
    // Cookie cut: A·B + ¬A·C.
    assert_eq!(minterm(0xca, 0xff00, 0x1234, 0xabcd), 0x12cd);
}

#[test]
fn fills_inclusive_and_exclusive() {
    assert_eq!(fill(0b0100_0100, false, false), (0b0111_1100, false));
    assert_eq!(fill(0b0100_0100, false, true), (0b0011_1100, false));
    assert!(fill(0b0000_0100, false, false).1, "carry out");
}

#[test]
fn copies_with_shift_masks_and_modulo() {
    let mut chip = ChipRam::new(0x1_0000);
    // Source: 2 rows of 2 words with a 2-byte modulo.
    for (i, w) in [0xffff, 0x0000, 0xdead, 0x8001, 0xffff, 0xbeef]
        .iter()
        .enumerate()
    {
        chip.set_word(0x1000 + 2 * i as u32, *w);
    }
    let mut b = Blitter::new();
    b.con0 = 0x49f0; // ASH 4, USEA|USED, D = A
    b.afwm = 0xffff;
    b.alwm = 0xff00;
    b.pt[A] = 0x1000;
    b.pt[D] = 0x2000;
    b.modulo[A] = 2;
    b.start(2 << 6 | 2);
    assert!(b.busy);
    run(&mut b, &mut chip);
    assert!(!b.busy && !b.zero);
    // Row 1: $FFFF, $0000&$FF00 shifted right by 4.
    // Row 2: the old bits of row 1's last word ($0000) shift in.
    assert_eq!(
        words(&chip, 0x2000, 4),
        vec![0x0fff, 0xf000, 0x0800, 0x1ff0]
    );
    assert_eq!(b.pt[D], 0x2008);
}

#[test]
fn descending_fill_closes_shapes() {
    let mut chip = ChipRam::new(0x1_0000);
    chip.set_word(0x1000, 0x8000);
    chip.set_word(0x1002, 0x0001);
    let mut b = Blitter::new();
    b.con0 = 0x09f0;
    b.con1 = con1::DESC | con1::IFE;
    b.afwm = 0xffff;
    b.alwm = 0xffff;
    b.pt[A] = 0x1002;
    b.pt[D] = 0x2002;
    b.start(1 << 6 | 2);
    run(&mut b, &mut chip);
    assert_eq!(words(&chip, 0x2000, 2), vec![0xffff, 0xffff]);
}

#[test]
fn zero_flag_without_destination() {
    let mut chip = ChipRam::new(0x1_0000);
    let mut b = Blitter::new();
    b.con0 = 0x0af0; // USEA|USEC, D = A
    b.afwm = 0xffff;
    b.alwm = 0xffff;
    b.pt[A] = 0x1000;
    b.start(4 << 6 | 4);
    run(&mut b, &mut chip);
    assert!(b.zero);
}

/// Draw a line the way the graphics library sets it up (HRM chapter 6,
/// "Line mode"): returns the bitplane rows (40 bytes wide) touched.
fn line(x0: i32, y0: i32, x1: i32, y1: i32) -> ChipRam {
    let mut chip = ChipRam::new(0x1_0000);
    let (mut dx, mut dy) = (x1 - x0, y1 - y0);
    let mut code = 0u16;
    if dx < 0 {
        dx = -dx;
        code |= 1;
    }
    if dy < 0 {
        dy = -dy;
        code |= 2;
    }
    // (dmajor, dminor, x major)
    let (dmaj, dmin, xmaj) = if dx >= dy {
        (dx, dy, true)
    } else {
        (dy, dx, false)
    };
    let sud = xmaj;
    let (major_neg, minor_neg) = if xmaj {
        (code & 1 != 0, code & 2 != 0)
    } else {
        (code & 2 != 0, code & 1 != 0)
    };
    let mut b = Blitter::new();
    let width = 40;
    let addr = 0x1000 + (y0 * width + (x0 >> 3 & !1)) as u32;
    let err = 4 * dmin - 2 * dmaj;
    b.con0 = ((x0 & 15) as u16) << 12 | 0x0bca;
    b.con1 = ((x0 & 15) as u16) << 12
        | con1::LINE
        | if sud { con1::SUD } else { 0 }
        | if minor_neg { con1::SUL } else { 0 }
        | if major_neg { con1::AUL } else { 0 }
        | if err < 0 { con1::SIGN } else { 0 };
    b.adat = 0x8000;
    b.bdat = 0xffff;
    b.afwm = 0xffff;
    b.pt[A] = err as u16 as u32;
    b.modulo[A] = (4 * (dmin - dmaj)) as u16;
    b.modulo[B] = (4 * dmin) as u16;
    b.modulo[C] = width as u16;
    b.modulo[D] = width as u16;
    b.pt[C] = addr;
    b.pt[D] = addr;
    b.start(((dmaj + 1) as u16) << 6 | 2);
    run(&mut b, &mut chip);
    chip
}

fn pixel(chip: &ChipRam, x: u32, y: u32) -> bool {
    chip.word(0x1000 + y * 40 + (x >> 3 & !1)) & (0x8000 >> (x & 15)) != 0
}

#[test]
fn draws_lines_in_all_octants() {
    for &(x0, y0, x1, y1) in &[
        (10, 10, 30, 15),
        (30, 15, 10, 10),
        (10, 10, 15, 30),
        (15, 30, 10, 10),
        (20, 20, 40, 5),
        (40, 5, 20, 20),
        (20, 20, 17, 40),
        (17, 40, 20, 20),
    ] {
        let chip = line(x0, y0, x1, y1);
        assert!(
            pixel(&chip, x0 as u32, y0 as u32),
            "start of {x0},{y0}-{x1},{y1}"
        );
        assert!(
            pixel(&chip, x1 as u32, y1 as u32),
            "end of {x0},{y0}-{x1},{y1}"
        );
        let count: usize = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&chip, x, y))
            .count();
        let len = (x1 - x0).abs().max((y1 - y0).abs()) as usize + 1;
        assert_eq!(count, len, "pixels of {x0},{y0}-{x1},{y1}");
    }
}

#[test]
fn savestate_round_trip_mid_blit() {
    let mut chip = ChipRam::new(0x1_0000);
    let mut b = Blitter::new();
    b.con0 = 0x09f0;
    b.afwm = 0xffff;
    b.alwm = 0xffff;
    b.adat = 0x1111;
    b.start(3 << 6 | 3);
    for _ in 0..7 {
        b.tick();
        if b.wants_slot() {
            b.cycle(&mut chip);
        }
    }
    let mut w = StateWriter::new();
    b.save(&mut w);
    let data = w.into_bytes();
    let c = Blitter::load(&mut StateReader::new(&data), 7).unwrap();
    assert_eq!(c, b);
}

#[test]
fn ecs_sizes_allow_wide_and_tall_blits() {
    let mut chip = ChipRam::new(0x8_0000);
    let mut b = Blitter::new();
    // D only, 100 words wide, 3 rows: more than BLTSIZE's 64 words.
    b.pt[3] = 0x1000;
    b.con0 = 0x0100 | 0xff;
    b.sizv = 3;
    b.start_ecs(100);
    assert_eq!(b.job_size(), Some((100, 3)));
    run(&mut b, &mut chip);
    assert!(!b.busy);
    assert_eq!(b.pt[3], 0x1000 + 2 * 300);
    assert_eq!(chip.word(0x1000 + 2 * 299), 0xffff);
    assert_eq!(chip.word(0x1000 + 2 * 300), 0);
    b.sizv = 0;
    b.start_ecs(0);
    assert_eq!(b.job_size(), Some((2048, 32768)), "0 means the maximum");
}

#[test]
fn cycle_counts_follow_the_channels() {
    // (BLTCON0 channel bits, colour clocks per word): 4 clocks, +2 for B,
    // +2 for C and D together (HRM "Blitter speed").
    for (use_bits, per_word) in [
        (0x0900, 2),
        (0x0500, 3),
        (0x0700, 4),
        (0x0f00, 4),
        (0x0100, 2),
    ] {
        let mut chip = ChipRam::new(0x1_0000);
        let mut b = Blitter::new();
        b.con0 = use_bits | 0xf0;
        b.afwm = 0xffff;
        b.alwm = 0xffff;
        b.start(2 << 6 | 3);
        let mut slots = 0;
        loop {
            b.tick();
            if b.wants_slot() {
                slots += 1;
                if b.cycle(&mut chip) {
                    break;
                }
            }
        }
        assert_eq!(slots, 6 * per_word, "channels {use_bits:04x}");
    }
}
