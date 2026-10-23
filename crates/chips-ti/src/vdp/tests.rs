use super::*;

fn vdp() -> Tms9918 {
    Tms9918::new(VdpModel::Ntsc)
}

fn reg(v: &mut Tms9918, r: u8, value: u8) {
    v.write_control(value);
    v.write_control(0x80 | r);
}

fn set_write_addr(v: &mut Tms9918, a: u16) {
    v.write_control(a as u8);
    v.write_control(0x40 | (a >> 8) as u8);
}

fn poke(v: &mut Tms9918, a: u16, bytes: &[u8]) {
    set_write_addr(v, a);
    for b in bytes {
        v.write_data(*b);
    }
}

/// Run until the start of the next field.
fn field(v: &mut Tms9918) {
    for _ in 0..400_000 {
        if v.tick() {
            return;
        }
    }
    panic!("no field end");
}

/// Palette index at active-area pixel (x, y).
fn px(v: &Tms9918, x: u32, y: u32) -> u8 {
    let row = y + u32::from(v.model.top());
    v.frame()[(row * WIDTH + LEFT + x) as usize]
}

#[test]
fn registers_addresses_and_the_read_ahead_buffer() {
    let mut v = vdp();
    reg(&mut v, 7, 0xf4);
    assert_eq!(v.regs[7], 0xf4);
    poke(&mut v, 0x1234, &[0xaa, 0xbb, 0xcc]);
    assert_eq!(&v.vram[0x1234..0x1237], &[0xaa, 0xbb, 0xcc]);
    assert_eq!(v.addr, 0x1237);
    // A read setup prefetches the first byte.
    v.write_control(0x34);
    v.write_control(0x12);
    assert_eq!(v.read_data(), 0xaa);
    assert_eq!(v.read_data(), 0xbb);
    assert_eq!(v.addr, 0x1237, "one byte ahead");
    // A status read resets the control latch.
    v.write_control(0x55);
    v.read_status();
    reg(&mut v, 2, 0x06);
    assert_eq!(v.regs[2], 0x06);
    // The address wraps at 16 KB.
    poke(&mut v, 0x3fff, &[1, 2]);
    assert_eq!((v.vram[0x3fff], v.vram[0]), (1, 2));
}

#[test]
fn the_frame_flag_comes_after_line_191_and_interrupts_with_ie() {
    let mut v = vdp();
    reg(&mut v, 1, 0x20);
    let mut ticks = 0u32;
    while v.status & status::F == 0 {
        v.tick();
        ticks += 1;
    }
    assert_eq!(ticks, 192 * u32::from(LINE));
    assert!(v.int());
    assert_eq!(v.read_status() & status::F, status::F);
    assert!(!v.int(), "reading the status clears F");
    reg(&mut v, 1, 0x00);
    field(&mut v);
    while v.status & status::F == 0 {
        v.tick();
    }
    assert!(!v.int(), "IE off");
    // NTSC field: 262 lines of 342 clocks; PAL 313.
    let mut v = vdp();
    field(&mut v);
    let mut n = 0u32;
    while !v.tick() {
        n += 1;
    }
    assert_eq!(n + 1, 262 * 342);
    let mut p = Tms9918::new(VdpModel::Pal);
    field(&mut p);
    let mut n = 0u32;
    while !p.tick() {
        n += 1;
    }
    assert_eq!(n + 1, 313 * 342);
    assert_eq!(p.frame().len(), (WIDTH * 294) as usize);
}

/// Graphics I with a name table at $1800, patterns at $0000, colours at
/// $2000: name 1 is a left half bar in colours $F1 (white on black).
fn graphics1(v: &mut Tms9918) {
    reg(v, 0, 0x00);
    reg(v, 1, 0x40);
    reg(v, 2, 0x06);
    reg(v, 3, 0x80);
    reg(v, 4, 0x00);
    reg(v, 5, 0x36);
    reg(v, 6, 0x07);
    reg(v, 7, 0x04);
    poke(v, 0x0008, &[0xf0; 8]);
    poke(v, 0x2000, &[0xf1]);
    poke(v, 0x1800, &[1]);
    poke(v, 0x1b00, &[0xd0]);
}

#[test]
fn graphics_one_shows_patterns_in_their_group_colours_and_the_backdrop() {
    let mut v = vdp();
    graphics1(&mut v);
    field(&mut v);
    assert_eq!(px(&v, 0, 0), 15);
    assert_eq!(px(&v, 3, 7), 15);
    assert_eq!(px(&v, 4, 0), 1, "background colour 1");
    assert_eq!(
        px(&v, 8, 0),
        1,
        "name 0 shares group 0's colours: background 1"
    );
    // Borders show the backdrop.
    assert_eq!(v.frame()[0], 4);
    assert_eq!(v.frame()[(WIDTH * 100 + WIDTH - 1) as usize], 4);
    // BLANK off: the whole picture is the backdrop.
    reg(&mut v, 1, 0x00);
    field(&mut v);
    assert_eq!(px(&v, 0, 0), 4);
}

#[test]
fn graphics_two_uses_a_pattern_and_colour_per_line_and_third() {
    let mut v = vdp();
    reg(&mut v, 0, 0x02);
    reg(&mut v, 1, 0x40);
    reg(&mut v, 2, 0x06);
    reg(&mut v, 3, 0xff); // colours at $2000, no masking
    reg(&mut v, 4, 0x03); // patterns at $0000, no masking
    reg(&mut v, 5, 0x36);
    reg(&mut v, 7, 0x00);
    // Name 0 in the second third (rows 8–15) is pattern 256.
    poke(&mut v, 0x0800, &[0x80, 0x00, 0, 0, 0, 0, 0, 0]);
    poke(&mut v, 0x2800, &[0x21, 0x31]);
    poke(&mut v, 0x1800 + 8 * 32, &[0]);
    poke(&mut v, 0x1b00, &[0xd0]);
    field(&mut v);
    assert_eq!(px(&v, 0, 64), 2, "pattern line 0, colour $21");
    assert_eq!(px(&v, 1, 64), 1);
    assert_eq!(px(&v, 0, 65), 1, "pattern line 1 empty, colour $31");
    // With R4 = 0 the pattern index is masked to the first 256: the
    // second third uses pattern 0's bytes.
    reg(&mut v, 4, 0x00);
    poke(&mut v, 0x0000, &[0x00]);
    field(&mut v);
    assert_eq!(px(&v, 0, 64), 1);
}

#[test]
fn multicolor_and_text_modes() {
    let mut v = vdp();
    // Multicolour: name 2 at the top left; its bytes for rows 0 and 1.
    reg(&mut v, 1, 0x48);
    reg(&mut v, 2, 0x06);
    reg(&mut v, 4, 0x00);
    reg(&mut v, 5, 0x36);
    poke(&mut v, 0x1b00, &[0xd0]);
    poke(&mut v, 0x1800, &[2]);
    poke(&mut v, 0x0010, &[0x9a, 0xbc]);
    field(&mut v);
    assert_eq!((px(&v, 0, 0), px(&v, 4, 3)), (9, 10));
    assert_eq!((px(&v, 0, 4), px(&v, 7, 7)), (11, 12));
    // Text: 40 columns of 6 pixels from active pixel 8.
    let mut v = vdp();
    reg(&mut v, 1, 0x50);
    reg(&mut v, 2, 0x00);
    reg(&mut v, 4, 0x01);
    reg(&mut v, 7, 0xf4);
    poke(&mut v, 0x0000, &[0, 1]);
    poke(&mut v, 0x0808, &[0xfc; 8]);
    field(&mut v);
    assert_eq!(px(&v, 7, 0), 4, "border");
    assert_eq!(px(&v, 8, 0), 4, "character 0: blank");
    assert_eq!(px(&v, 14, 0), 15, "character 1 starts at 8 + 6");
    assert_eq!(px(&v, 19, 0), 15);
    assert_eq!(px(&v, 20, 0), 4);
}

#[test]
fn sprites_four_per_line_fifth_flag_and_coincidence() {
    let mut v = vdp();
    graphics1(&mut v);
    // Sprite pattern 0: a full 8×8 block.
    poke(&mut v, 0x3800, &[0xff; 8]);
    // Five sprites on line 10 (Y = 9), colours 2..6, 20 pixels apart.
    let mut attrs = Vec::new();
    for n in 0..5u8 {
        attrs.extend([9, 20 * n, 0, 2 + n]);
    }
    attrs.push(0xd0);
    poke(&mut v, 0x1b00, &attrs);
    field(&mut v);
    assert_eq!(px(&v, 0, 10), 2);
    assert_eq!(px(&v, 60, 17), 5);
    assert_eq!(px(&v, 80, 10), 1, "the fifth sprite is not shown");
    assert_eq!(px(&v, 0, 9), 1, "Y + 1 is the first line");
    let s = v.read_status();
    assert_eq!(s & (status::FIFTH | 0x1f), status::FIFTH | 4);
    assert_eq!(s & status::COINCIDENCE, 0);
    // Overlap two sprites: coincidence, the lower number in front.
    poke(&mut v, 0x1b00, &[9, 0, 0, 2, 9, 4, 0, 3, 0xd0]);
    field(&mut v);
    assert_eq!(px(&v, 4, 10), 2);
    assert_eq!(px(&v, 9, 10), 3);
    let s = v.read_status();
    assert_eq!(s & status::COINCIDENCE, status::COINCIDENCE);
    assert_eq!(s & 0x1f, 1, "no fifth sprite: the last one looked at");
    // Magnified 16×16, early clock: 32×32 pixels, 32 to the left.
    reg(&mut v, 1, 0x43);
    poke(&mut v, 0x3800, &[0xff; 32]);
    poke(&mut v, 0x1b00, &[9, 40, 0, 0x87, 0xd0]);
    field(&mut v);
    assert_eq!(px(&v, 8, 10), 7);
    assert_eq!(px(&v, 39, 41), 7);
    assert_eq!(px(&v, 40, 10), 1);
    assert_eq!(px(&v, 8, 42), 1);
}

#[test]
fn savestate_round_trip() {
    let mut v = vdp();
    graphics1(&mut v);
    for _ in 0..100_000 {
        v.tick();
    }
    v.write_control(0x12);
    let mut w = StateWriter::new();
    v.save(&mut w);
    let data = w.into_bytes();
    let mut back = vdp();
    back.load(&mut StateReader::new(&data)).unwrap();
    for _ in 0..200_000 {
        v.tick();
        back.tick();
    }
    assert_eq!(back.frame(), v.frame());
    assert_eq!((back.regs, back.addr, back.line), (v.regs, v.addr, v.line));
    assert!(back
        .load(&mut StateReader::new(&data[..data.len() - 1]))
        .is_err());
}
