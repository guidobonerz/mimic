use super::*;
use emu_core::gfx::GfxOptions;
use emu_core::MemSpaceId;

fn reg(v: &mut V9938, n: u8, value: u8) {
    v.write_control(value);
    v.write_control(0x80 | n);
}

/// Set the VRAM address for writing.
fn seek(v: &mut V9938, a: usize) {
    reg(v, 14, (a >> 14) as u8);
    v.write_control(a as u8);
    v.write_control(0x40 | (a >> 8) as u8 & 0x3f);
}

fn field(v: &mut V9938) {
    while !v.tick() {}
}

/// The colour of half pixel `x` of active line `y`.
fn at(v: &V9938, x: usize, y: usize) -> Rgb8 {
    let row = usize::from(v.display_start()) + y;
    v.frame()[row * WIDTH as usize + LEFT + x]
}

fn color(n: usize) -> Rgb8 {
    let (r, g, b) = DEFAULT_PALETTE[n];
    rgb777(r, g, b)
}

/// A chip showing mode `m` (M5…M1), display on, 212 lines.
fn chip(m: u8) -> V9938 {
    let mut v = V9938::new(false);
    reg(&mut v, 0, m >> 1 & 0x0e);
    reg(&mut v, 1, 0x40 | (m & 1) << 4 | (m & 2) << 2);
    reg(&mut v, 9, 0x80);
    v
}

/// Run a command to its end; the clocks it took.
fn finish(v: &mut V9938) -> u32 {
    let mut n = 0;
    while v.status_peek(2) & 1 != 0 {
        v.tick();
        n += 1;
        assert!(n < 2_000_000, "the command never ends");
    }
    n
}

fn command(v: &mut V9938, regs: &[(u8, u16)], arg: u8, cmd: u8) {
    for &(n, value) in regs {
        reg(v, n, value as u8);
        reg(v, n + 1, (value >> 8) as u8);
    }
    reg(v, 45, arg);
    reg(v, 46, cmd);
}

#[test]
fn registers_ports_and_the_17_bit_address() {
    let mut v = chip(mode::G4);
    assert_eq!(v.mode(), mode::G4);
    // The indirect port: R17 advances, or not with bit 7.
    reg(&mut v, 17, 2);
    v.write_indirect(0x1f);
    v.write_indirect(0xaa);
    assert_eq!((v.regs[2], v.regs[3], v.regs[17]), (0x1f, 0xaa, 4));
    reg(&mut v, 17, 0x80 | 7);
    v.write_indirect(1);
    v.write_indirect(2);
    assert_eq!(v.regs[7], 2);
    // In a V9938 mode the address carries into R14.
    seek(&mut v, 0x7fff);
    v.write_data(0x11);
    v.write_data(0x22);
    assert_eq!((v.vram[0x7fff], v.vram[0x8000]), (0x11, 0x22));
    assert_eq!(v.regs[14], 2);
    // Reading: the pair without bit 6 reads ahead.
    reg(&mut v, 14, 1);
    v.write_control(0xff);
    v.write_control(0x3f);
    assert_eq!((v.read_data(), v.read_data()), (0x11, 0x22));
    // In a TMS9918A mode it wraps within 16 KB.
    let mut v = chip(mode::G2);
    seek(&mut v, 0x3fff);
    v.write_data(1);
    v.write_data(2);
    assert_eq!((v.vram[0x3fff], v.vram[0], v.regs[14]), (1, 2, 0));
}

#[test]
fn the_palette_port_takes_pairs() {
    let mut v = chip(mode::G4);
    assert_eq!(v.colors()[2], color(2));
    reg(&mut v, 16, 5);
    v.write_palette(0x71);
    v.write_palette(0x03);
    v.write_palette(0x07);
    v.write_palette(0x00);
    assert_eq!(v.colors()[5], rgb777(7, 3, 1));
    assert_eq!(v.colors()[6], rgb777(0, 0, 7));
    assert_eq!(v.regs[16], 7);
    v.reset();
    assert_eq!(v.colors()[5], color(5));
}

#[test]
fn g4_shows_its_page_scrolled_and_colour_0_as_the_backdrop() {
    let mut v = chip(mode::G4);
    reg(&mut v, 7, 4);
    v.vram[0] = 0x2f; // line 0: pixels 0 and 1
    v.vram[128 * 5 + 1] = 0x90; // line 5: pixel 2
    v.vram[0x8000] = 0x66; // page 1
    field(&mut v);
    assert_eq!(v.height(), HEIGHT_NTSC);
    assert_eq!(at(&v, 0, 0), color(2));
    assert_eq!(at(&v, 1, 0), color(2));
    assert_eq!(at(&v, 2, 0), color(15));
    assert_eq!(at(&v, 4, 0), color(4), "colour 0 is the backdrop");
    assert_eq!(at(&v, 4, 5), color(9));
    assert_eq!(v.frame()[0], color(4), "border");
    // TP: colour 0 is the palette's.
    reg(&mut v, 8, 0x20);
    field(&mut v);
    assert_eq!(at(&v, 4, 0), color(0));
    // R23 moves line 5 up to line 2; page 1 through R2.
    reg(&mut v, 23, 3);
    field(&mut v);
    assert_eq!(at(&v, 4, 2), color(9));
    assert_eq!(at(&v, 0, 253 - 212 + 211 - 41), color(0));
    reg(&mut v, 23, 0);
    reg(&mut v, 2, 0x3f);
    field(&mut v);
    assert_eq!(at(&v, 0, 0), color(6));
}

#[test]
fn g5_g6_and_g7_decode_their_pixels() {
    let mut v = chip(mode::G5);
    reg(&mut v, 8, 0x20);
    reg(&mut v, 7, 0b1110); // backdrop: 3 and 2 by halves
    v.vram[0] = 0b00_01_10_11;
    field(&mut v);
    for (x, c) in [0, 1, 2, 3].into_iter().enumerate() {
        assert_eq!(at(&v, x, 0), color(c), "G5 pixel {x}");
    }
    assert_eq!(v.frame()[0], color(3));
    assert_eq!(v.frame()[1], color(2));

    let mut v = chip(mode::G6);
    v.vram[256 + 1] = 0x8f;
    field(&mut v);
    assert_eq!(at(&v, 2, 1), color(8));
    assert_eq!(at(&v, 3, 1), color(15));

    let mut v = chip(mode::G7);
    reg(&mut v, 7, 0xe0);
    v.vram[2] = 0x1f;
    v.vram[0x1_0000] = 0xff;
    field(&mut v);
    assert_eq!(at(&v, 4, 0), rgb777(7, 0, 7));
    assert_eq!(at(&v, 5, 0), rgb777(7, 0, 7));
    assert_eq!(at(&v, 0, 0), rgb777(0, 0, 0), "no transparency in G7");
    assert_eq!(
        v.frame()[0],
        rgb777(0, 7, 0),
        "the backdrop is a colour byte"
    );
    reg(&mut v, 2, 0x3f);
    field(&mut v);
    assert_eq!(at(&v, 0, 0), rgb777(7, 7, 7), "page 1");
}

#[test]
fn text_2_shows_80_columns_and_blinks() {
    let mut v = chip(mode::T2);
    reg(&mut v, 2, 0x03); // names at 0
    reg(&mut v, 4, 0x02); // patterns at $1000
    reg(&mut v, 3, 0x27); // blink table at $0800
    reg(&mut v, 7, 0xf4);
    reg(&mut v, 12, 0x1a);
    v.vram[79] = 1;
    v.vram[0x1000 + 8] = 0x80;
    v.vram[0x0800 + 9] = 0x01; // column 79 blinks
    field(&mut v);
    let x = 16 + 79 * 6;
    assert_eq!(at(&v, x, 0), color(15));
    assert_eq!(at(&v, x + 1, 0), color(4));
    assert_eq!(at(&v, 0, 0), color(4), "the margin shows the backdrop");
    // Blinking: on for 10 fields, off for 10.
    reg(&mut v, 13, 0x11);
    let mut seen = Vec::new();
    for _ in 0..40 {
        field(&mut v);
        seen.push(at(&v, x, 0) == color(1));
    }
    let on = seen.iter().filter(|b| **b).count();
    assert_eq!(on, 20, "{seen:?}");
    assert!(seen.windows(10).any(|w| w.iter().all(|b| *b)));
}

#[test]
fn the_tms_modes_use_the_palette() {
    // G1: pattern 1 in colours 10 on 2.
    let mut v = chip(mode::G1);
    reg(&mut v, 2, 6); // names $1800
    reg(&mut v, 3, 0x80); // colours $2000
    reg(&mut v, 7, 1);
    v.vram[0x1800] = 1;
    v.vram[8] = 0xff;
    v.vram[0x2000] = 0xa2;
    field(&mut v);
    assert_eq!(at(&v, 0, 0), color(10));
    assert_eq!(at(&v, 1, 0), color(10));
    assert_eq!(at(&v, 0, 1), color(2));
    // T1: 40 columns from pixel 8.
    let mut v = chip(mode::T1);
    reg(&mut v, 7, 0xf4);
    v.vram[0] = 1;
    v.vram[8] = 0x80;
    field(&mut v);
    assert_eq!(at(&v, 16, 0), color(15));
    assert_eq!(at(&v, 18, 0), color(4));
    // G3 is G2 with other sprites.
    let mut v = chip(mode::G3);
    reg(&mut v, 2, 6);
    reg(&mut v, 3, 0xff);
    reg(&mut v, 4, 0x03);
    v.vram[0x1800 + 32 * 8] = 5; // second third
    v.vram[0x0800 + 5 * 8] = 0x80;
    v.vram[0x2800 + 5 * 8] = 0x6f;
    field(&mut v);
    assert_eq!(at(&v, 0, 64), color(6));
    assert_eq!(at(&v, 2, 64), color(15));
}

#[test]
fn sprite_mode_2_has_line_colours_eight_on_a_line_and_cc() {
    let mut v = chip(mode::G4);
    reg(&mut v, 5, 0xec); // attributes $7600, colours $7400
    reg(&mut v, 11, 0);
    reg(&mut v, 6, 0x0f); // patterns $7800
    for row in 0..8 {
        v.vram[0x7800 + row] = 0xff;
    }
    // Sprite 0 at (10, line 20), line colours 3 then 5.
    v.vram[0x7600..0x7604].copy_from_slice(&[19, 10, 0, 0]);
    v.vram[0x7400] = 3;
    v.vram[0x7401] = 5;
    // Sprite 1 over it with CC: colour 8 ORed in.
    v.vram[0x7604..0x7608].copy_from_slice(&[19, 14, 0, 0]);
    v.vram[0x7410] = 0x48;
    v.vram[0x7608] = 216;
    field(&mut v);
    assert_eq!(at(&v, 20, 20), color(3));
    assert_eq!(at(&v, 20, 21), color(5));
    assert_eq!(
        at(&v, 28, 20),
        color(3 | 8),
        "the CC sprite's colour is ORed"
    );
    assert_eq!(at(&v, 40, 20), color(8), "and shows beside it");
    assert_eq!(
        at(&v, 20, 22),
        color(0),
        "a line of colour 0 is transparent"
    );
    assert_eq!(
        v.status & status::COINCIDENCE,
        0,
        "CC sprites do not collide"
    );
    // Two solid sprites collide; S3–S6 name the place.
    v.vram[0x7410] = 0x08;
    v.read_status();
    field(&mut v);
    assert_ne!(v.status & status::COINCIDENCE, 0);
    assert_eq!(v.status_peek(3), 14 + 12);
    assert_eq!(v.status_peek(5), 20 + 8);
    // Nine on a line: the ninth is not drawn and is named in S0.
    for n in 0..9usize {
        let a = 0x7600 + n * 4;
        v.vram[a..a + 4].copy_from_slice(&[99, 20 * n as u8, 0, 0]);
        v.vram[0x7400 + n * 16] = 15;
    }
    v.vram[0x7600 + 9 * 4] = 216;
    v.read_status();
    field(&mut v);
    assert_eq!(v.status & 0x5f, status::FIFTH | 8);
    assert_eq!(at(&v, 2 * 140, 100), color(15));
    assert_eq!(at(&v, 2 * 160, 100), color(0));
    // SPD switches the sprites off.
    reg(&mut v, 8, 2);
    field(&mut v);
    assert_eq!(at(&v, 2 * 140, 100), color(0));
}

#[test]
fn sprite_mode_1_and_the_fixed_colours_of_g7() {
    let mut v = chip(mode::G1);
    reg(&mut v, 5, 0x36); // attributes $1B00
    reg(&mut v, 6, 0x07); // patterns $3800
    v.vram[0x3800] = 0x80;
    for n in 0..5usize {
        let a = 0x1b00 + n * 4;
        v.vram[a..a + 4].copy_from_slice(&[9, 20 + 10 * n as u8, 0, 15]);
    }
    v.vram[0x1b00 + 20] = 208;
    v.set_overlays(&["sprites", "limit"]);
    field(&mut v);
    v.overlay_end_frame();
    assert_eq!(at(&v, 40, 10), color(15));
    assert_eq!(v.status & 0x5f, status::FIFTH | 4);
    let o = v.overlay("sprites", true).unwrap();
    assert_eq!(o.shapes.len(), 4);
    assert_eq!(v.overlay("limit", true).unwrap().shapes.len(), 1);

    let mut v = chip(mode::G7);
    reg(&mut v, 5, 0xec);
    reg(&mut v, 6, 0x0f);
    v.vram[0x7800] = 0x80;
    v.vram[0x7600..0x7604].copy_from_slice(&[255, 0, 0, 0]);
    v.vram[0x7400] = 8;
    v.vram[0x7604] = 216;
    field(&mut v);
    assert_eq!(at(&v, 0, 0), rgb777(7, 4, 2));
}

#[test]
fn frame_and_line_interrupts_and_status_2() {
    let mut v = chip(mode::G4);
    reg(&mut v, 1, 0x60);
    reg(&mut v, 19, 100);
    assert!(!v.int());
    let start = v.display_start();
    assert_eq!(start, 17);
    // Without IE1 the flag shows only to the end of line 100.
    while !(v.line == start + 100 && v.dot == 300) {
        v.tick();
    }
    assert_eq!(v.status_peek(1), 1);
    assert!(!v.int());
    assert_eq!(
        v.status_peek(2) & 0x60,
        0x20,
        "horizontal blank, not vertical"
    );
    while v.dot != 20 {
        v.tick();
    }
    assert_eq!(v.status_peek(1), 0);
    assert_eq!(v.status_peek(2) & 0x60, 0);
    // The frame flag at the end of line 211.
    while v.status & status::F == 0 {
        v.tick();
    }
    assert_eq!((v.line, v.dot), (start + 212, 0));
    assert!(v.int());
    assert_eq!(v.status_peek(2) & 0x40, 0x40);
    assert_eq!(v.read_status() & status::F, status::F);
    assert!(!v.int());
    // With IE1 the line interrupt stays until S1 is read; R23 moves it.
    let r0 = v.regs[0] | 0x10;
    reg(&mut v, 0, r0);
    reg(&mut v, 23, 90);
    field(&mut v);
    while !v.int() {
        v.tick();
    }
    assert_eq!((v.line, v.dot), (start + 10, 13 + 256 + 1));
    for _ in 0..1000 {
        v.tick();
    }
    assert!(v.int());
    reg(&mut v, 15, 1);
    assert_eq!(v.read_status(), 1);
    assert!(!v.int());
    assert_eq!(v.read_status(), 0, "the ID is 0");
}

#[test]
fn nt_switches_to_313_lines_at_the_next_field() {
    let mut v = V9938::new(false);
    field(&mut v);
    assert_eq!((v.lines(), v.height()), (262, HEIGHT_NTSC));
    reg(&mut v, 9, 2);
    field(&mut v);
    let mut clocks = 0u32;
    while !v.tick() {
        clocks += 1;
    }
    assert_eq!(clocks + 1, 313 * 342);
    assert_eq!((v.height(), v.display_start()), (HEIGHT_PAL, 54));
    assert_eq!(v.frame().len(), (WIDTH * HEIGHT_PAL) as usize);
    let pal = V9938::new(true);
    assert_eq!((pal.lines(), pal.regs[9]), (313, 2));
    // S2's field bit alternates.
    let a = v.status_peek(2) & 2;
    field(&mut v);
    assert_ne!(v.status_peek(2) & 2, a);
}

#[test]
fn fills_and_copies() {
    let mut v = chip(mode::G4);
    // HMMV: 16 × 2 pixels of byte $5A at (8, 3).
    reg(&mut v, 44, 0x5a);
    command(&mut v, &[(36, 8), (38, 3), (40, 16), (42, 2)], 0, 0xc0);
    assert_eq!(v.status_peek(2) & 1, 1, "CE while it runs");
    let clocks = finish(&mut v);
    assert!((100..400).contains(&clocks), "{clocks} clocks for 16 bytes");
    assert_eq!(&v.vram[3 * 128 + 4..3 * 128 + 12], &[0x5a; 8]);
    assert_eq!(v.vram[3 * 128 + 12], 0);
    assert_eq!(&v.vram[4 * 128 + 4..4 * 128 + 12], &[0x5a; 8]);
    assert_eq!(v.vram[5 * 128 + 4], 0);
    assert_eq!((v.regs[38], v.regs[42]), (5, 0), "DY and NY follow");
    // LMMM with XOR onto itself one pixel to the right: 5 ^ A at the seam.
    command(
        &mut v,
        &[(32, 8), (34, 3), (36, 9), (38, 10), (40, 4), (42, 1)],
        0,
        0x90,
    );
    finish(&mut v);
    assert_eq!(v.vram[10 * 128 + 4], 0x05);
    assert_eq!(v.vram[10 * 128 + 5], 0xa5);
    assert_eq!(v.vram[10 * 128 + 6], 0xa0);
    // LMMV with TIMP and colour 0 changes nothing; with IMP it clears.
    reg(&mut v, 44, 0);
    command(&mut v, &[(36, 8), (38, 3), (40, 4), (42, 1)], 0, 0x88);
    finish(&mut v);
    assert_eq!(v.vram[3 * 128 + 4], 0x5a);
    command(&mut v, &[(36, 9), (38, 3), (40, 2), (42, 1)], 0, 0x80);
    finish(&mut v);
    assert_eq!(&v.vram[3 * 128 + 4..3 * 128 + 6], &[0x50, 0x0a]);
    // HMMM to the left and up, clipped at the edges.
    v.vram[0] = 0x12;
    v.vram[1] = 0x34;
    command(
        &mut v,
        &[
            (32, 2),
            (34, 0),
            (36, 0x82),
            (38, 0x40),
            (40, 100),
            (42, 100),
        ],
        0x0c,
        0xd0,
    );
    finish(&mut v);
    assert_eq!(&v.vram[0x40 * 128 + 0x40..0x40 * 128 + 0x42], &[0x12, 0x34]);
    // YMMM: line 3 from column 8 to the right edge, to line 20.
    command(&mut v, &[(34, 3), (36, 8), (38, 20), (42, 1)], 0, 0xe0);
    finish(&mut v);
    assert_eq!(v.vram[20 * 128 + 4], 0x50);
    assert_eq!(v.vram[20 * 128 + 11], 0x5a);
    assert_eq!(v.vram[20 * 128 + 3], 0);
}

#[test]
fn points_lines_and_searches() {
    let mut v = chip(mode::G5);
    // PSET and POINT in G5's 2-bit pixels.
    reg(&mut v, 44, 3);
    command(&mut v, &[(36, 5), (38, 1)], 0, 0x50);
    assert_eq!(v.vram[128 + 1], 0b00_11_00_00);
    command(&mut v, &[(32, 5), (34, 1)], 0, 0x40);
    assert_eq!(v.status_peek(7), 3);
    // SRCH to the right for colour 3 from column 0.
    command(&mut v, &[(32, 0), (34, 1)], 0, 0x60);
    finish(&mut v);
    assert_eq!(v.status_peek(2) & 0x10, 0x10, "found");
    assert_eq!((v.status_peek(8), v.status_peek(9)), (5, 0xfe));
    // …and for anything but colour 3 from column 5.
    command(&mut v, &[(32, 5), (34, 1)], 0x02, 0x60);
    finish(&mut v);
    assert_eq!(v.status_peek(8), 6);
    // Nothing on line 2.
    command(&mut v, &[(32, 0), (34, 2)], 0, 0x60);
    finish(&mut v);
    assert_eq!(v.status_peek(2) & 0x10, 0);

    // LINE in G7: 8 along X, 4 along Y, from (10, 10).
    let mut v = chip(mode::G7);
    reg(&mut v, 44, 0xff);
    command(&mut v, &[(36, 10), (38, 10), (40, 8), (42, 4)], 0, 0x70);
    finish(&mut v);
    let set: Vec<(usize, usize)> = (0..512usize)
        .flat_map(|y| (0..256).map(move |x| (x, y)))
        .filter(|&(x, y)| v.vram[y * 256 + x] != 0)
        .collect();
    assert_eq!(set.len(), 9);
    assert_eq!((set[0], set[8]), ((10, 10), (18, 14)));
    // The long side along Y, up and left.
    command(
        &mut v,
        &[(36, 100), (38, 100), (40, 10), (42, 0)],
        0x0d,
        0x70,
    );
    finish(&mut v);
    assert_eq!(v.vram[90 * 256 + 100], 0xff);
    assert_eq!(v.vram[89 * 256 + 100], 0);
}

#[test]
fn transfers_to_and_from_the_cpu() {
    let mut v = chip(mode::G4);
    // HMMC: 4 × 2 pixels = 2 bytes a row; the first byte is in R44
    // before the command.
    reg(&mut v, 44, 0x12);
    command(&mut v, &[(36, 0), (38, 0), (40, 4), (42, 2)], 0, 0xf0);
    assert_eq!(v.status_peek(2) & 0x81, 0x81, "TR and CE");
    assert_eq!(v.vram[0], 0x12);
    reg(&mut v, 17, 0x80 | 44);
    for b in [0x34, 0x56] {
        v.write_indirect(b);
        assert_eq!(v.status_peek(2) & 1, 1);
    }
    v.write_indirect(0x78);
    assert_eq!(v.status_peek(2) & 0x81, 0, "done");
    assert_eq!(&v.vram[..2], &[0x12, 0x34]);
    assert_eq!(&v.vram[128..130], &[0x56, 0x78]);
    // LMMC: pixels, with OR.
    reg(&mut v, 44, 0x0c);
    command(&mut v, &[(36, 0), (38, 0), (40, 2), (42, 1)], 0, 0xb2);
    reg(&mut v, 17, 0x80 | 44);
    v.write_indirect(0x01);
    assert_eq!(v.vram[0], 0xd3);
    assert_eq!(v.status_peek(2) & 1, 0);
    // LMCM: the pixels come through S7.
    command(&mut v, &[(32, 0), (34, 0), (40, 4), (42, 1)], 0, 0xa0);
    reg(&mut v, 15, 7);
    let mut got = Vec::new();
    for _ in 0..4 {
        assert_eq!(v.status_peek(2) & 0x80, 0x80, "TR");
        got.push(v.read_status());
    }
    assert_eq!(got, [0x0d, 0x03, 0x03, 0x04]);
    assert_eq!(v.status_peek(2) & 0x81, 0);
    // STOP ends a command; outside G4–G7 none starts.
    command(&mut v, &[(36, 0), (38, 0), (40, 0), (42, 0)], 0, 0xc0);
    assert_eq!(v.status_peek(2) & 1, 1);
    reg(&mut v, 46, 0);
    assert_eq!(v.status_peek(2) & 1, 0);
    let mut v = chip(mode::G2);
    command(&mut v, &[(36, 0), (38, 0), (40, 0), (42, 0)], 0, 0xc0);
    assert_eq!(v.status_peek(2) & 1, 0);
}

#[test]
fn display_adjust_moves_the_picture() {
    let mut v = chip(mode::G4);
    v.vram[0] = 0xf0;
    reg(&mut v, 18, 0x12); // two pixels left, one line up
    field(&mut v);
    assert_eq!(v.display_start(), 16);
    let row = 16 * WIDTH as usize;
    assert_eq!(v.frame()[row + LEFT - 4], color(15));
    reg(&mut v, 18, 0xff); // one pixel right, one line down
    field(&mut v);
    let row = 18 * WIDTH as usize;
    assert_eq!(v.frame()[row + LEFT + 2], color(15));
}

#[test]
fn views_are_side_effect_free() {
    let mut v = chip(mode::G4);
    v.vram[0] = 0xf2;
    reg(&mut v, 5, 0xec);
    reg(&mut v, 6, 0x0f);
    let before = v.clone();
    let space = MemSpaceId(1);
    let s = v.gfx_view("screen", space, &GfxOptions::new()).unwrap();
    assert_eq!((s.width, s.height, s.raw.len()), (256, 256, 0x8000));
    assert_eq!(s.frame().rgb_at(0, 0), Some(color(15)));
    assert_eq!(s.frame().rgb_at(1, 0), Some(color(2)));
    let t2 = v
        .gfx_view("screen", space, &GfxOptions::new().with("mode", 9))
        .unwrap();
    assert_eq!((t2.width, t2.height, t2.raw.len()), (480, 212, 2160));
    let sp = v.gfx_view("sprites", space, &GfxOptions::new()).unwrap();
    assert_eq!((sp.width, sp.height), (8 * 9 - 1, 4 * 9 - 1));
    assert_eq!(sp.addr, 0x7600);
    assert_eq!(sp.raw.len(), 32 * 20);
    let p = v.gfx_view("palette", space, &GfxOptions::new()).unwrap();
    assert_eq!(p.raw.len(), 32);
    let t = v.gfx_view("tiles", space, &GfxOptions::new()).unwrap();
    assert_eq!(t.raw.len(), 2048);
    assert!(v.gfx_view("nope", space, &GfxOptions::new()).is_none());
    assert_eq!(v, before);
}

#[test]
fn savestate_round_trip_mid_command() {
    let mut v = chip(mode::G6);
    reg(&mut v, 16, 3);
    v.write_palette(0x25);
    reg(&mut v, 44, 0x77);
    command(&mut v, &[(36, 0), (38, 0), (40, 300), (42, 40)], 0, 0x80);
    for _ in 0..5000 {
        v.tick();
    }
    assert_eq!(v.status_peek(2) & 1, 1);
    let mut w = StateWriter::new();
    v.save(&mut w);
    let bytes = w.into_bytes();
    let mut copy = V9938::new(false);
    copy.load(&mut StateReader::new(&bytes)).unwrap();
    for _ in 0..400_000 {
        v.tick();
        copy.tick();
    }
    assert_eq!(v.status_peek(2) & 1, 0);
    assert_eq!(v, copy);
    // Damaged data is refused, never a panic.
    for cut in [0, 10, bytes.len() / 2, bytes.len() - 1] {
        assert!(V9938::new(false)
            .load(&mut StateReader::new(&bytes[..cut]))
            .is_err());
    }
}
