use super::*;

fn setup() -> (Custom, ChipRam) {
    (Custom::new(Standard::Pal, 0x7fffe), ChipRam::new(0x8_0000))
}

/// Run until the start of the next field.
fn field(c: &mut Custom, chip: &mut ChipRam) {
    for _ in 0..200_000 {
        if c.tick(chip, false).field_done {
            return;
        }
    }
    panic!("no field end");
}

fn pixel(c: &Custom, x: u32, y: u32) -> Rgb8 {
    c.frame()[(y * WIDTH + x) as usize]
}

/// A 320×256 lowres screen with one plane at $10000: column 0 of every
/// row set.
fn screen(c: &mut Custom, chip: &mut ChipRam) {
    for row in 0..256u32 {
        chip.set_word(0x1_0000 + row * 40, 0x8000);
    }
    for (r, v) in [
        (reg::BPLCON0, 0x1200),
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::DDFSTRT, 0x38),
        (reg::DDFSTOP, 0xd0),
        (reg::BPL1PTH, 0x0001),
        (reg::BPL1PTH + 2, 0x0000),
        (reg::COLOR00, 0x00f),
        (reg::COLOR00 + 2, 0xfff),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
    ] {
        c.write(r, v);
    }
}

const BLUE: Rgb8 = Rgb8 { r: 0, g: 0, b: 255 };
const WHITE: Rgb8 = Rgb8 {
    r: 255,
    g: 255,
    b: 255,
};

#[test]
fn beam_counts_pal_lines_and_fields() {
    let (mut c, mut chip) = setup();
    let mut n = 0u32;
    let mut hsyncs = 0;
    loop {
        let out = c.tick(&mut chip, false);
        n += 1;
        hsyncs += u32::from(out.hsync);
        if out.field_done {
            break;
        }
    }
    assert_eq!(n, 313 * 227, "long field");
    assert_eq!(hsyncs, 313);
    assert_eq!(c.peek(reg::VPOSR), 0x8000);
    assert_ne!(c.paula.requests() & int::VERTB, 0);
}

#[test]
fn lowres_plane_shows_inside_the_display_window() {
    let (mut c, mut chip) = setup();
    screen(&mut c, &mut chip);
    field(&mut c, &mut chip);
    // DIWSTRT $2C81: line 44 (row 18), pixel $81 (column 2·$81 − 4·$38).
    let (x, y) = (2 * 0x81 - 4 * FIRST_CCK, 44 - 26);
    assert_eq!(pixel(&c, x, y), WHITE);
    assert_eq!(pixel(&c, x + 1, y), WHITE);
    assert_eq!(pixel(&c, x + 2, y), BLUE);
    assert_eq!(pixel(&c, x - 1, y), BLUE);
    assert_eq!(pixel(&c, x, y - 1), BLUE, "above the window");
    assert_eq!(pixel(&c, x, y + 255), WHITE, "last line");
    assert_eq!(pixel(&c, x, y + 256), BLUE, "below the window");
    // 256 rows of 40 bytes fetched; software reloads the pointer for the
    // next field.
    assert_eq!(c.bplpt[0], 0x1_0000 + 256 * 40);
}

#[test]
fn modulo_comes_with_the_last_fetch_not_after_a_later_pointer_write() {
    let (mut c, mut chip) = setup();
    screen(&mut c, &mut chip);
    c.write(reg::BPL1MOD, 40);
    // Run into line $40, to the first slot after the fetch window
    // ($38 + 19·8 + 8 = $D8).
    while !(c.v == 0x40 && c.h == 0xd8) {
        c.tick(&mut chip, false);
    }
    let fetched = 0x1_0000 + (0x40 - 0x2c) * 80 + 40;
    assert_eq!(c.bplpt[0], fetched + 40, "40 bytes and the modulo");
    // A pointer write after the window (a Copper MOVE at the line end)
    // is what the next line fetches from.
    c.write(reg::BPL1PTH + 2, 0x2000);
    while c.h != 0x38 {
        c.tick(&mut chip, false);
    }
    assert_eq!(c.bplpt[0], 0x1_2000);
}

#[test]
fn copper_waits_past_line_255_after_wait_ff_de() {
    // WAIT $FF,$DE; WAIT $20,$DE; MOVE COLOR00: the second WAIT must be
    // fetched in line 256, so the colour changes at line $120 (288), not
    // right after line 255 (Rule 30's kaleidoscope uses this).
    let (mut c, mut chip) = setup();
    for (i, w) in [
        0x0180u16, 0x0f00, 0xffdf, 0xfffe, 0x20df, 0xfffe, 0x0180, 0x00f0, 0xffff, 0xfffe,
    ]
    .iter()
    .enumerate()
    {
        chip.set_word(0x2000 + 2 * i as u32, *w);
    }
    c.write(reg::COP1LCH, 0);
    c.write(reg::COP1LCL, 0x2000);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::COPEN);
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    let red = Rgb8 { r: 255, g: 0, b: 0 };
    let green = Rgb8 { r: 0, g: 255, b: 0 };
    assert_eq!(pixel(&c, 300, 270 - 26), red, "line 270: still red");
    assert_eq!(pixel(&c, 300, 290 - 26), green, "line 290: green");
}

#[test]
fn copper_changes_colour_at_a_line() {
    let (mut c, mut chip) = setup();
    for (i, w) in [
        0x0180u16, 0x0f00, 0x6001, 0xfffe, 0x0180, 0x00f0, 0xffff, 0xfffe,
    ]
    .iter()
    .enumerate()
    {
        chip.set_word(0x2000 + 2 * i as u32, *w);
    }
    c.write(reg::COP1LCH, 0);
    c.write(reg::COP1LCL, 0x2000);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::COPEN);
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    let red = Rgb8 { r: 255, g: 0, b: 0 };
    let green = Rgb8 { r: 0, g: 255, b: 0 };
    assert_eq!(pixel(&c, 300, 0x5f - 26), red);
    assert_eq!(pixel(&c, 300, 0x61 - 26), green);
}

#[test]
fn copper_blitter_and_cpu_share_slots() {
    let (mut c, mut chip) = setup();
    for i in 0..64 {
        chip.set_word(0x3000 + 2 * i, 0xffff);
    }
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::BLTEN);
    // A to D: both slots of every blitter cycle access chip RAM.
    c.write(reg::BLTCON0, 0x09f0);
    c.write(reg::BLTAFWM, 0xffff);
    c.write(reg::BLTALWM, 0xffff);
    c.write(reg::BLTAPTL, 0x3000);
    c.write(reg::BLTDPTL, 0x4000);
    c.write(reg::BLTSIZE, 8 << 6 | 8);
    assert_ne!(c.peek(reg::DMACONR) & dma::BBUSY, 0);
    let (mut cpu, mut total) = (0, 0);
    while c.blitter.busy {
        total += 1;
        cpu += u32::from(c.tick(&mut chip, true).cpu_slot);
    }
    // About every fourth slot goes to the waiting CPU.
    assert!(cpu * 5 > total && cpu * 3 < total, "cpu {cpu} of {total}");
    assert_eq!(chip.word(0x4000), 0xffff);
    assert_eq!(chip.word(0x4000 + 63 * 2), 0xffff);
    assert_ne!(c.paula.requests() & int::BLIT, 0);
    c.write(reg::BLTAPTL, 0x3000);
    c.write(reg::BLTDPTL, 0x4000);
    // With BLTPRI the CPU waits for the whole blit.
    c.write(reg::DMACON, 0x8000 | dma::BLTPRI);
    c.write(reg::BLTSIZE, 8 << 6 | 8);
    for _ in 0..4 {
        c.tick(&mut chip, true); // start delay
    }
    while c.blitter.busy {
        let out = c.tick(&mut chip, true);
        if c.blitter.busy {
            assert!(!out.cpu_slot, "CPU slot during a nasty blit");
        }
    }
}

/// A CPU bus cycle against a busy blitter: the request clock, then the
/// waiting clocks until the data slot. Returns the colour clocks the cycle
/// took from the request on.
fn cpu_bus_cycle(c: &mut Custom, chip: &mut ChipRam) -> u32 {
    c.cpu_request();
    c.tick(chip, false);
    let mut n = 1;
    loop {
        n += 1;
        if c.tick(chip, true).cpu_slot {
            return n;
        }
        assert!(n < 20, "CPU never got the bus");
    }
}

#[test]
fn a_waiting_cpu_gets_every_fifth_colour_clock_of_a_busy_blitter() {
    let (mut c, mut chip) = setup();
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::BLTEN);
    // A, B, C and D: no idle cycles, the blit lasts a whole field.
    c.write(reg::BLTCON0, 0x0fca);
    c.write(reg::BLTAFWM, 0xffff);
    c.write(reg::BLTALWM, 0xffff);
    c.write(reg::BLTSIZE, 0);
    for _ in 0..8 {
        c.tick(&mut chip, false);
    }
    // The request half-cycle does not count; three Blitter cycles in which
    // the CPU waits for its data slot do, the next goes to the CPU
    // (measured: tools/amiga_bustest2.py on WinUAE).
    while c.h & 1 == 0 {
        c.tick(&mut chip, false);
    }
    assert_eq!(cpu_bus_cycle(&mut c, &mut chip), 5);
    // Back-to-back bus cycles keep that rhythm.
    assert_eq!(cpu_bus_cycle(&mut c, &mut chip), 5);
    assert_eq!(cpu_bus_cycle(&mut c, &mut chip), 5);
    // An internal CPU cycle in between: the count starts over.
    c.tick(&mut chip, false);
    c.tick(&mut chip, false);
    assert_eq!(cpu_bus_cycle(&mut c, &mut chip), 5);
    assert!(c.blitter.busy);
}

#[test]
fn idle_blitter_cycles_leave_the_bus_to_the_cpu() {
    let (mut c, mut chip) = setup();
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::BLTEN | dma::BLTPRI);
    // A line: per pixel a C read, two idle cycles and the D write.
    c.write(reg::BLTCON0, 0x0bca);
    c.write(reg::BLTCON1, 0x0001);
    c.write(reg::BLTADAT, 0x8000);
    c.write(reg::BLTBDAT, 0xffff);
    c.write(reg::BLTCMOD, 40);
    c.write(reg::BLTCPTH, 0);
    c.write(reg::BLTCPTL, 0x4000);
    c.write(reg::BLTDPTL, 0x4000);
    c.write(reg::BLTSIZE, 64 << 6 | 2);
    let (mut cpu, mut total) = (0, 0);
    while c.blitter.busy {
        total += 1;
        cpu += u32::from(c.tick(&mut chip, true).cpu_slot);
    }
    // Even with BLTPRI, the CPU gets the idle cycles on its slots.
    assert!(cpu * 5 > total, "cpu {cpu} of {total}");
}

#[test]
fn savestate_round_trip() {
    let (mut c, mut chip) = setup();
    screen(&mut c, &mut chip);
    for _ in 0..50_000 {
        c.tick(&mut chip, false);
    }
    let mut w = StateWriter::new();
    c.save(&mut w);
    let data = w.into_bytes();
    let mut d = Custom::new(Standard::Pal, 0x7fffe);
    d.load(&mut StateReader::new(&data)).unwrap();
    let mut chip2 = chip.clone();
    for _ in 0..100_000 {
        c.tick(&mut chip, false);
        d.tick(&mut chip2, false);
    }
    assert_eq!((c.v, c.h, c.bplpt), (d.v, d.h, d.bplpt));
    assert_eq!(c.frame(), d.frame());
    assert!(d
        .load(&mut StateReader::new(&data[..data.len() - 1]))
        .is_err());
}

#[test]
fn sprite_dma_fetches_control_and_data_words() {
    let (mut c, mut chip) = setup();
    // Sprite 0 at line $2C, low-resolution pixel $A0, two lines high.
    for (i, w) in [0x2c50u16, 0x2e00, 0x8000, 0x0000, 0xc000, 0x8000, 0, 0]
        .iter()
        .enumerate()
    {
        chip.set_word(0x3000 + 2 * i as u32, *w);
    }
    for (r, v) in [
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::SPR0PTH, 0),
        (reg::SPR0PTH + 2, 0x3000),
        (reg::COLOR00, 0x000),
        (reg::COLOR00 + 34, 0xf00),
        (reg::COLOR00 + 36, 0x0f0),
        (reg::COLOR00 + 38, 0x00f),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::SPREN),
    ] {
        c.write(r, v);
    }
    // One field: software (the Copper) reloads SPR0PT for the next.
    field(&mut c, &mut chip);
    let (x, y) = (2 * 0xa0 - 4 * FIRST_CCK, 0x2c - 26);
    let red = Rgb8 { r: 255, g: 0, b: 0 };
    let blue = Rgb8 { r: 0, g: 0, b: 255 };
    let black = Rgb8::default();
    assert_eq!(pixel(&c, x, y), red);
    assert_eq!(pixel(&c, x + 2, y), black);
    assert_eq!(pixel(&c, x, y + 1), blue, "colour 3 on the second line");
    assert_eq!(pixel(&c, x + 2, y + 1), red);
    assert_eq!(pixel(&c, x, y + 2), black, "VSTOP");
    assert_eq!(pixel(&c, x, y - 1), black);
}

#[test]
fn a_sprite_with_vstart_equal_to_vstop_fetches_control_words_and_no_data() {
    // Sanity's "Interference" parks its eight sprites with $2900/$2900 and
    // the Copper list right behind: on hardware the sprite stays off; a
    // sprite that read data instead would take 2 slots every line.
    let (mut c, mut chip) = setup();
    for (i, w) in [0x2900u16, 0x2900, 0x0000, 0x0000, 0xffff, 0xffff]
        .iter()
        .enumerate()
    {
        chip.set_word(0x3000 + 2 * i as u32, *w);
    }
    for (r, v) in [
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::SPR0PTH, 0),
        (reg::SPR0PTH + 2, 0x3000),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::SPREN),
    ] {
        c.write(r, v);
    }
    field(&mut c, &mut chip);
    // Control words at the first sprite line, new ones at line $29 (the
    // VSTOP compare wins over VSTART), nothing after: the pointer stops
    // behind the second pair.
    assert_eq!(c.sprpt[0], 0x3008);
    assert_eq!(c.spr_dma[0], SprDma::Wait);
}

/// Six lowres planes at $10000 + plane · $2800, first word of line $2C
/// only; returns the first two lowres pixels of the line as RGB.
fn six_planes(bplcon0: u16, words: [u16; 6], colors: &[(usize, u16)]) -> [Rgb8; 3] {
    let (mut c, mut chip) = setup();
    for (p, w) in words.iter().enumerate() {
        chip.set_word(0x1_0000 + 0x2800 * p as u32, *w);
    }
    let mut regs = vec![
        (reg::BPLCON0, bplcon0),
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2dc1),
        (reg::DDFSTRT, 0x38),
        (reg::DDFSTOP, 0xd0),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
    ];
    for p in 0..6u16 {
        let addr = 0x1_0000 + 0x2800 * u32::from(p);
        regs.push((reg::BPL1PTH + 4 * p, (addr >> 16) as u16));
        regs.push((reg::BPL1PTH + 4 * p + 2, addr as u16));
    }
    for &(i, col) in colors {
        regs.push((reg::COLOR00 + 2 * i as u16, col));
    }
    for (r, v) in regs {
        c.write(r, v);
    }
    field(&mut c, &mut chip);
    let (x, y) = (2 * 0x81 - 4 * FIRST_CCK, 0x2c - 26);
    [pixel(&c, x, y), pixel(&c, x + 2, y), pixel(&c, x + 4, y)]
}

#[test]
fn ham_holds_and_modifies_through_dma() {
    // Pixel 0: palette 3; pixel 1: modify red to $F; pixel 2: modify blue.
    let px = six_planes(
        0x6a00,
        [0xe000, 0xc000, 0x6000, 0x6000, 0x2000, 0x4000],
        &[(3, 0x123), (12, 0)],
    );
    let rgb = |c: u16| crate::denise::rgb12(c);
    assert_eq!(px, [rgb(0x123), rgb(0xf23), rgb(0xf2d)]);
}

#[test]
fn dual_playfield_through_dma() {
    // Pixel 0: PF1 colour 1 and PF2 colour 1 (PF1 in front); pixel 1: PF2
    // only (colour 9); pixel 2: neither.
    let px = six_planes(
        0x4600,
        [0x8000, 0xc000, 0, 0, 0, 0],
        &[(1, 0xf00), (9, 0x0f0), (0, 0x00f)],
    );
    let rgb = |c: u16| crate::denise::rgb12(c);
    assert_eq!(px, [rgb(0xf00), rgb(0x0f0), rgb(0x00f)]);
}

#[test]
fn hires_fetch_window_takes_40_words_with_ddfstop_d0_or_d4() {
    for stop in [0xd0, 0xd4] {
        let (mut c, mut chip) = setup();
        for (r, v) in [
            (reg::BPLCON0, 0x9200),
            (reg::DIWSTRT, 0xac81),
            (reg::DIWSTOP, 0xadc1), // one line ($AC; VSTOP bit 8 = !bit 7)
            (reg::DDFSTRT, 0x3c),
            (reg::DDFSTOP, stop),
            (reg::BPL1PTH, 0x0001),
            (reg::BPL1PTH + 2, 0x0000),
            (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
        ] {
            c.write(r, v);
        }
        field(&mut c, &mut chip);
        assert_eq!(c.bplpt[0], 0x1_0000 + 80, "one line, DDFSTOP {stop:x}");
    }
}
