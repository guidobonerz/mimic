use super::*;

fn setup() -> (Custom, ChipRam) {
    (
        Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS),
        ChipRam::new(0x8_0000),
    )
}

fn setup_ecs(standard: Standard) -> (Custom, ChipRam) {
    (
        Custom::new(standard, 0x1f_fffe, Chipset::ECS),
        ChipRam::new(0x20_0000),
    )
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
    let mut d = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
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
fn ecs_state_keeps_the_beam_registers_and_the_switched_standard() {
    let (mut c, mut chip) = setup_ecs(Standard::Pal);
    screen(&mut c, &mut chip);
    c.write(reg::BEAMCON0, 0);
    c.write(reg::DIWHIGH, 0x2100);
    c.write(reg::BPLCON3, 0x0020);
    c.write(reg::BLTSIZV, 9);
    c.write(reg::HTOTAL, 0xe3);
    for _ in 0..50_000 {
        c.tick(&mut chip, false);
    }
    let mut w = StateWriter::new();
    c.save(&mut w);
    let data = w.into_bytes();
    let mut d = Custom::new(Standard::Pal, 0x1f_fffe, Chipset::ECS);
    d.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(d.standard, Standard::Ntsc, "BEAMCON0 PAL off");
    assert_eq!(d.frame().len(), (WIDTH * Standard::Ntsc.height()) as usize);
    assert_eq!((d.beam_regs, d.diwhigh), (c.beam_regs, c.diwhigh));
    assert_eq!(d.denise.bplcon3, 0x0020);
    assert_eq!(d.blitter.sizv, 9);
    assert_eq!(
        (d.v, d.h, d.lof, d.lol, d.dmacon),
        (c.v, c.h, c.lof, c.lol, c.dmacon)
    );
    assert_eq!(
        (&d.denise, &d.blitter, &d.copper),
        (&c.denise, &c.blitter, &c.copper)
    );
    d.reset();
    assert_eq!(d.standard, Standard::Pal, "reset: back to the strap");
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
    // One (empty) plane: sprites show only on lines with plane data.
    for (r, v) in [
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::DDFSTRT, 0x38),
        (reg::DDFSTOP, 0xd0),
        (reg::BPLCON0, 0x1200),
        (reg::BPL1PTH, 0),
        (reg::BPL1PTH + 2, 0x4000),
        (reg::SPR0PTH, 0),
        (reg::SPR0PTH + 2, 0x3000),
        (reg::COLOR00, 0x000),
        (reg::COLOR00 + 34, 0xf00),
        (reg::COLOR00 + 36, 0x0f0),
        (reg::COLOR00 + 38, 0x00f),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN | dma::SPREN),
    ] {
        c.write(r, v);
    }
    // One field: software (the Copper) reloads SPR0PT for the next.
    field(&mut c, &mut chip);
    // The sprite's first pixel is one low-resolution pixel right of HSTART.
    let (x, y) = (2 * 0xa1 - 4 * FIRST_CCK, 0x2c - 26);
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

#[test]
fn ecs_ids_and_lol_in_vposr() {
    let (c, _) = setup();
    assert_eq!(c.peek(reg::VPOSR) & 0x7fff, 0, "OCS: no id");
    assert_eq!(c.peek(reg::DENISEID), 0xffff);
    let (c, _) = setup_ecs(Standard::Pal);
    assert_eq!(c.peek(reg::VPOSR) & 0x7f80, 0x2000, "8372A PAL");
    assert_eq!(c.peek(reg::DENISEID), 0xfffc, "8373");
    assert_eq!(c.beamcon0(), beamcon0::PAL);
    let (mut c, mut chip) = setup_ecs(Standard::Ntsc);
    assert_eq!(c.peek(reg::VPOSR) & 0x7f80, 0x3000, "8372A NTSC");
    assert_eq!(c.beamcon0(), 0);
    for _ in 0..228 {
        c.tick(&mut chip, false);
    }
    assert_eq!(c.peek(reg::VPOSR) & 0x80, 0x80, "LOL after the first line");
}

#[test]
fn beamcon0_switches_the_beam_between_pal_and_ntsc() {
    let (mut c, mut chip) = setup_ecs(Standard::Ntsc);
    let lines = |c: &mut Custom, chip: &mut ChipRam| {
        field(c, chip);
        let mut n = 0;
        loop {
            let out = c.tick(chip, false);
            n += u32::from(out.hsync);
            if out.field_done {
                return n;
            }
        }
    };
    assert_eq!(lines(&mut c, &mut chip), 263);
    assert_eq!(c.frame().len(), (WIDTH * Standard::Ntsc.height()) as usize);
    c.write(reg::BEAMCON0, beamcon0::PAL);
    assert_eq!(lines(&mut c, &mut chip), 313);
    assert_eq!(c.frame().len(), (WIDTH * Standard::Pal.height()) as usize);
    assert_eq!(c.line_len(), 227);
    // The programmable beam: HTOTAL + 1 clocks, VTOTAL + 1 lines.
    c.write(reg::HTOTAL, 0x70);
    c.write(reg::VTOTAL, 0x100);
    c.write(reg::BEAMCON0, beamcon0::PAL | beamcon0::VARBEAMEN);
    assert_eq!(c.line_len(), 0x71);
    assert_eq!(lines(&mut c, &mut chip), 0x101);
    // An OCS Agnus ignores all of it.
    let (mut c, mut chip) = setup();
    c.write(reg::BEAMCON0, 0);
    assert_eq!(lines(&mut c, &mut chip), 313);
}

#[test]
fn diwhigh_extends_the_vertical_window_until_diwstrt_is_written() {
    let (mut c, mut chip) = setup_ecs(Standard::Pal);
    screen(&mut c, &mut chip);
    // Stop at line $12C = 300 instead of $2C | $100.
    c.write(reg::DIWHIGH, 0x2100);
    c.write(reg::DIWSTOP, 0x2cc1);
    assert_eq!(c.diwhigh, 0x2100, "DIWSTOP restored the default");
    // DIWSTOP $2C with V8 from DIWHIGH: the window stops at line $12C,
    // 256 rows (the plane data has 256 rows, the pointer is reloaded).
    let show = |c: &mut Custom, chip: &mut ChipRam| {
        c.write(reg::BPL1PTH, 1);
        c.write(reg::BPL1PTH + 2, 0);
        field(c, chip);
    };
    show(&mut c, &mut chip);
    let x = 0x102 - 4 * FIRST_CCK;
    assert_eq!(pixel(&c, x, 0x2c - 26), WHITE);
    assert_eq!(pixel(&c, x, 0x12b - 26), WHITE, "last row");
    assert_eq!(pixel(&c, x, 0x12c - 26), BLUE, "stops at $12C");
    // Stop V9 instead: the window runs to the end of the field.
    c.write(reg::DIWHIGH, 0x2200);
    show(&mut c, &mut chip);
    assert_eq!(
        pixel(&c, x, 0x12c - 26),
        BLUE,
        "row 256 of the data is empty"
    );
    assert!(c.bplpt[0] > 0x1_0000 + 256 * 40, "fetching past row 256");
    // Start V8 = 1: nothing in the upper part of the field.
    c.write(reg::DIWHIGH, 0x2201);
    show(&mut c, &mut chip);
    assert_eq!(pixel(&c, x, 0x2c - 26), BLUE, "start at $12C");
}

#[test]
fn ecs_bltcon0l_sets_only_the_minterm() {
    // Kickstart 2+/3.x text: the channels and A shift stay, the minterm
    // changes between the template blits (Workbench 3.1 drew no text
    // without it).
    let (mut c, _) = setup_ecs(Standard::Pal);
    c.write(reg::BLTCON0, 0x970a);
    c.write(reg::BLTCON0L, 0x12ca);
    assert_eq!(c.blitter.con0, 0x97ca);
    // The OCS Agnus has no such register.
    let (mut c, _) = setup();
    c.write(reg::BLTCON0, 0x970a);
    c.write(reg::BLTCON0L, 0x00ca);
    assert_eq!(c.blitter.con0, 0x970a);
}

#[test]
fn ecs_blitter_sizes_and_copper_reach_every_register() {
    let (mut c, mut chip) = setup_ecs(Standard::Pal);
    c.write(reg::BLTCON0, 0x01ff);
    c.write(reg::BLTCON1, 0);
    c.write(reg::BLTDPTL, 0x4000);
    c.write(reg::BLTDMOD, 0);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::BLTEN);
    c.write(reg::BLTSIZV, 2);
    c.write(reg::BLTSIZH, 100);
    assert_eq!(c.blitter.job_size(), Some((100, 2)));
    field(&mut c, &mut chip);
    assert_eq!(chip.word(0x4000 + 2 * 199), 0xffff);
    assert_eq!(chip.word(0x4000 + 2 * 200), 0);
    // The Copper with CDANG writes BLTSIZV ($5C) and even DSKPTH ($20).
    chip.set_word(0x1000, reg::BLTSIZV);
    chip.set_word(0x1002, 5);
    chip.set_word(0x1004, reg::DSKPTH);
    chip.set_word(0x1006, 0x0007);
    chip.set_word(0x1008, 0xffff);
    chip.set_word(0x100a, 0xfffe);
    c.write(reg::COPCON, 2);
    c.write(reg::COP1LCH, 0);
    c.write(reg::COP1LCL, 0x1000);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::COPEN);
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    assert_eq!(c.blitter.sizv, 5);
    assert_eq!(c.paula.disk.pt >> 16, 7);
    // An OCS Agnus does not take BLTSIZV or let the Copper below $40.
    let (mut c, mut chip) = setup();
    c.write(reg::BLTSIZV, 5);
    assert_eq!(c.blitter.sizv, 0);
    chip.set_word(0x1000, reg::DSKPTH);
    chip.set_word(0x1002, 0x0007);
    c.write(reg::COPCON, 2);
    c.write(reg::COP1LCL, 0x1000);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::COPEN);
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    assert_eq!(c.paula.disk.pt, 0);
}

fn setup_aga() -> (Custom, ChipRam) {
    (
        Custom::new(Standard::Pal, 0x1f_fffe, Chipset::AGA),
        ChipRam::new(0x20_0000),
    )
}

#[test]
fn aga_ids_and_eight_planes_with_wide_fetches() {
    let (mut c, mut chip) = setup_aga();
    assert_eq!(c.peek(reg::VPOSR) & 0x7f00, 0x2200, "Alice");
    assert_eq!(c.peek(reg::DENISEID), 0xfff8, "Lisa");
    // Eight planes, 64-bit fetches: plane n at $10000 + n·$1000, the first
    // row's first 64 pixels set in planes 1 and 8 only; the colour of
    // index $81 is a 24-bit value written through LOCT.
    for p in 0..8u32 {
        if p == 0 || p == 7 {
            for i in 0..4 {
                chip.set_word(0x1_0000 + p * 0x1000 + 2 * i, 0xffff);
            }
        }
        c.write(reg::BPL1PTH + 4 * p as u16, 0x0001);
        c.write(reg::BPL1PTH + 4 * p as u16 + 2, (p * 0x1000) as u16);
    }
    for (r, v) in [
        (reg::BPLCON0, 0x0210),
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::DDFSTRT, 0x38),
        (reg::DDFSTOP, 0xd0),
        (reg::FMODE, 0x0003),
        (reg::BPLCON3, 0x0c00 | 0x8000),
        (reg::COLOR00 + 2, 0x123),
        (reg::BPLCON3, 0x0c00 | 0x8000 | 0x0200),
        (reg::COLOR00 + 2, 0x456),
        (reg::BPLCON3, 0x0c00),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
    ] {
        c.write(r, v);
    }
    field(&mut c, &mut chip);
    let x = 2 * 0x81 - 4 * FIRST_CCK;
    let y = 44 - 26;
    assert_eq!(
        pixel(&c, x, y),
        Rgb8 {
            r: 0x14,
            g: 0x25,
            b: 0x36
        },
        "colour $81"
    );
    assert_eq!(
        pixel(&c, x + 127, y),
        Rgb8 {
            r: 0x14,
            g: 0x25,
            b: 0x36
        },
        "64 pixels wide"
    );
    assert_eq!(pixel(&c, x + 128, y), Rgb8::default());
    // Units of 32 clocks from $38: the one in progress at the OCS DDFSTOP
    // $D0 ($B8–$D7) completes and one more follows, 6 units = 48 bytes
    // per plane per line (an FMODE 3 screen of 320 pixels uses $B8).
    assert_eq!(c.bplpt[0], 0x1_0000 + 256 * 48);
    assert_eq!(c.bplpt[7], 0x1_7000 + 256 * 48);
    // An OCS Agnus ignores FMODE and the extra planes.
    let (mut c, _) = setup();
    c.write(reg::FMODE, 3);
    c.write(reg::BPL7PTH + 2, 0x1234);
    assert_eq!((c.fmode, c.bplpt[6]), (0, 0));
}

#[test]
fn aga_sprite_dma_fetches_wide_words() {
    let (mut c, mut chip) = setup_aga();
    // Sprite 0 at $20000: control words (VSTART 60, HSTART $80; VSTOP 62)
    // then 64-bit data lines, as 64-bit fetches: 8 bytes per word.
    chip.set_word(0x2_0000, 0x3c40);
    chip.set_word(0x2_0008, 0x3e00);
    for i in 0..4 {
        chip.set_word(0x2_0010 + 2 * i, 0x8000 >> (4 * i));
        chip.set_word(0x2_0018 + 2 * i, 0);
    }
    c.write(reg::SPR0PTH, 0x0002);
    c.write(reg::SPR0PTH + 2, 0x0000);
    c.write(reg::FMODE, 0x000c);
    c.write(reg::DMACON, 0x8000 | dma::DMAEN | dma::SPREN);
    // From the field start past line 60's sprite slots: 2 control fetches
    // (line 25) and the data fetches of line 60, 8 bytes each.
    for _ in 0..60 * 227 + 50 {
        c.tick(&mut chip, false);
    }
    assert_eq!(c.sprpt[0], 0x2_0000 + 4 * 8);
    assert_eq!(
        c.denise.sprites[0].data, 0x8000_0800_0080_0008,
        "64 bits of sprite data"
    );
}

/// Colour 0 white, no planes, LPEN on: the whole picture lights a pen.
fn pen_setup() -> (Custom, ChipRam) {
    let (mut c, chip) = setup();
    c.write(reg::BPLCON0, 0x0208);
    c.write(reg::COLOR00, 0xfff);
    c.set_light_pen(Some((200, 100)), 80);
    (c, chip)
}

fn run_to_line(c: &mut Custom, chip: &mut ChipRam, v: u16) {
    while c.v != v {
        c.tick(chip, false);
    }
}

#[test]
fn light_pen_freezes_vposr_and_vhposr_until_line_20() {
    let (mut c, mut chip) = pen_setup();
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    // Frame pixel (200, 100): Denise column 50 + $38 = 106 colour clocks,
    // two behind Agnus (h = 108); row 100 is line 26 + 100.
    assert_eq!(c.peek(reg::VHPOSR), 126 << 8 | 108);
    assert_eq!(c.peek(reg::VPOSR) & 7, 0);
    run_to_line(&mut c, &mut chip, 19);
    assert_eq!(
        c.peek(reg::VHPOSR),
        126 << 8 | 108,
        "frozen through blanking"
    );
    run_to_line(&mut c, &mut chip, 21);
    assert_eq!(c.peek(reg::VHPOSR) >> 8, 21, "released at line 20");
    run_to_line(&mut c, &mut chip, 127);
    assert_eq!(c.peek(reg::VHPOSR), 126 << 8 | 108);

    // State: the latch round-trips.
    let mut w = StateWriter::new();
    c.save(&mut w);
    let data = w.into_bytes();
    let mut d = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
    d.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(d.peek(reg::VHPOSR), 126 << 8 | 108);
}

#[test]
fn light_pen_without_a_trigger_freezes_at_the_end_of_the_field() {
    let (mut c, mut chip) = pen_setup();
    c.write(reg::COLOR00, 0x000);
    field(&mut c, &mut chip);
    // Last colour clock of the long PAL field: line 312, h 226.
    assert_eq!(c.peek(reg::VPOSR) & 7, 1);
    assert_eq!(c.peek(reg::VHPOSR), (312 & 0xff) << 8 | 226);
    // LPEN off: the running counters.
    c.write(reg::BPLCON0, 0x0200);
    assert_eq!(c.peek(reg::VHPOSR), 0);
}

#[test]
fn ecs_lpendis_shows_the_running_counters() {
    let (mut c, mut chip) = setup_ecs(Standard::Pal);
    c.write(reg::BPLCON0, 0x0208);
    c.write(reg::COLOR00, 0xfff);
    c.set_light_pen(Some((200, 100)), 80);
    field(&mut c, &mut chip);
    assert_eq!(c.peek(reg::VHPOSR), 126 << 8 | 108);
    c.write(0x1dc, beamcon0::PAL | beamcon0::LPENDIS);
    assert_eq!(c.peek(reg::VHPOSR), 0);
}

#[test]
fn super_hires_widens_the_field_and_it_narrows_again() {
    let (mut c, mut chip) = setup_ecs(Standard::Pal);
    screen(&mut c, &mut chip);
    // Every word $8000, however many words a line fetches.
    for i in 0..0x4_0000 {
        chip.set_word(0x1_0000 + 2 * i, 0x8000);
    }
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    assert_eq!(c.width(), WIDTH);
    let row = |c: &Custom| {
        let w = c.width() as usize;
        let y = 100;
        c.frame()[y * w..(y + 1) * w].to_vec()
    };
    let narrow = row(&c);
    let at = narrow.iter().position(|&p| p == WHITE).unwrap();
    assert_eq!(narrow[at + 1], WHITE, "a low-resolution pixel: two columns");
    c.write(reg::BPLCON0, 0x1240); // super-hires, one plane
    field(&mut c, &mut chip);
    field(&mut c, &mut chip);
    assert_eq!((c.width(), c.pixels_per_cck()), (WIDE, 8));
    assert_eq!(c.frame().len(), (WIDE * Standard::Pal.height()) as usize);
    let wide = row(&c);
    let at = wide.iter().position(|&p| p == WHITE).unwrap();
    assert_ne!(wide[at + 1], WHITE, "a super-hires pixel: one 35 ns column");
    // A savestate keeps the width.
    let mut w = StateWriter::new();
    c.save(&mut w);
    let data = w.into_bytes();
    let mut d = Custom::new(Standard::Pal, 0x1f_fffe, Chipset::ECS);
    d.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(d.width(), WIDE);
    assert_eq!(&d.denise, &c.denise);
    // Without 35 ns content the picture narrows after FINE_HOLD fields.
    c.write(reg::BPLCON0, 0x1200);
    for _ in 0..FINE_HOLD - 1 {
        field(&mut c, &mut chip);
    }
    assert_eq!(c.width(), WIDE, "held");
    field(&mut c, &mut chip);
    assert_eq!(c.width(), WIDTH);
    assert_eq!(c.frame().len(), (WIDTH * Standard::Pal.height()) as usize);
}

#[test]
fn the_frame_reports_the_field_it_holds() {
    // LOF turns at the end of an interlaced field: the picture still holds
    // the field before, which a frontend weaves by its LOF.
    let (mut c, mut chip) = setup();
    c.write(reg::BPLCON0, 0x0204);
    field(&mut c, &mut chip);
    for _ in 0..4 {
        let drawn = c.lof;
        field(&mut c, &mut chip);
        assert_ne!(c.lof, drawn);
        assert_eq!(c.frame_long(), drawn);
    }
}

#[test]
fn aga_wide_fetch_runs_in_aligned_blocks_and_sprites_end_at_ddfstrt() {
    // WinUAE A1200 with `tools/amiga_agatest.py` (2026-10-26): hires
    // FMODE 3 fetches in 16-clock blocks, the planes in the last eight
    // clocks; sprites whose first slot is at or after DDFSTRT get no DMA.
    let (mut c, _chip) = setup_aga();
    for (r, v) in [
        (reg::BPLCON0, 0x8211),
        (reg::DIWSTRT, 0x2c81),
        (reg::DIWSTOP, 0x2cc1),
        (reg::DDFSTOP, 0xd0),
        (reg::FMODE, 0x000f),
        (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN | dma::SPREN),
    ] {
        c.write(r, v);
    }
    for ddf in [0x40, 0x44, 0x48] {
        c.write(reg::DDFSTRT, ddf);
        assert_eq!(c.fetch_window().0, 0x40, "DDFSTRT {ddf:#x}");
    }
    c.write(reg::DDFSTRT, 0x38);
    assert_eq!(c.fetch_window(), (0x30, 0x30 + 11 * 16));
    c.v = 0x40;
    c.bpl_vertical = true;
    assert!((0x30..0x38).all(|h| c.plane_at(h).is_none()));
    assert_eq!(c.plane_at(0x38), Some(7), "plane 8 first");
    assert_eq!(c.plane_at(0x3f), Some(0), "plane 1 last");
    let lost = |c: &Custom| (0..8).filter(|&n| c.sprite_blocked(n)).collect::<Vec<_>>();
    assert_eq!(lost(&c), Vec::<usize>::new());
    c.write(reg::DDFSTRT, 0x30);
    assert_eq!(lost(&c), vec![7]);
    c.write(reg::DDFSTRT, 0x24);
    assert_eq!(lost(&c), vec![4, 5, 6, 7]);
    c.write(reg::DDFSTRT, 0x18);
    assert_eq!(lost(&c), (1..8).collect::<Vec<_>>());
}
