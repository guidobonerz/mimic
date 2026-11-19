use super::*;

fn reg(v: &mut SmsVdp, r: u8, value: u8) {
    v.write_control(value);
    v.write_control(0x80 | r);
}

fn vram_at(v: &mut SmsVdp, addr: u16) {
    v.write_control(addr as u8);
    v.write_control(0x40 | (addr >> 8) as u8);
}

fn cram_at(v: &mut SmsVdp, addr: u8) {
    v.write_control(addr);
    v.write_control(0xc0);
}

fn field(v: &mut SmsVdp) {
    while !v.tick() {}
}

fn run_to_line(v: &mut SmsVdp, line: u16) {
    while v.line != line {
        v.tick();
    }
}

fn pixel(v: &SmsVdp, x: u32, line: u16) -> Rgb8 {
    let (w, _, px) = v.frame();
    let row = v.line_row(line).unwrap();
    px[(row * w + x) as usize]
}

const RED: Rgb8 = Rgb8 { r: 255, g: 0, b: 0 };
const GREEN: Rgb8 = Rgb8 { r: 0, g: 255, b: 0 };

/// Mode 4, display on, names at $3800, sprites at $3F00, backdrop colour
/// 16+0; pattern 1 solid colour 1, CRAM 1 red, CRAM 17 green.
fn mode4(kind: VdpKind) -> SmsVdp {
    let mut v = SmsVdp::new(kind, false);
    reg(&mut v, 0, 0x04);
    reg(&mut v, 1, 0x40);
    reg(&mut v, 2, 0xff);
    reg(&mut v, 5, 0xff);
    reg(&mut v, 6, 0xfb);
    vram_at(&mut v, 32);
    for _ in 0..8 {
        for plane in [0xff, 0, 0, 0] {
            v.write_data(plane);
        }
    }
    if kind == VdpKind::GameGear {
        cram_at(&mut v, 2);
        v.write_data(0x0f);
        v.write_data(0x00);
        cram_at(&mut v, 34);
        v.write_data(0xf0);
        v.write_data(0x00);
    } else {
        cram_at(&mut v, 1);
        v.write_data(0x03);
        cram_at(&mut v, 17);
        v.write_data(0x0c);
    }
    // No sprites: the end mark.
    vram_at(&mut v, 0x3f00);
    v.write_data(0xd0);
    v
}

#[test]
fn the_command_word_reaches_registers_vram_and_cram() {
    let mut v = SmsVdp::new(VdpKind::Sms2, false);
    reg(&mut v, 7, 0x05);
    assert_eq!(v.regs[7], 5);
    vram_at(&mut v, 0x1234);
    v.write_data(0xab);
    assert_eq!(v.vram[0x1234], 0xab);
    // A read address loads the buffer at once.
    v.write_control(0x34);
    v.write_control(0x12);
    assert_eq!(v.read_data(), 0xab);
    cram_at(&mut v, 3);
    v.write_data(0xff);
    assert_eq!(v.cram[3], 0x3f, "SMS CRAM is 6 bits");
    // The Game Gear: an even address only latches.
    let mut g = SmsVdp::new(VdpKind::GameGear, false);
    cram_at(&mut g, 0);
    g.write_data(0xff);
    assert_eq!(g.cram[0], 0);
    cram_at(&mut g, 0x21);
    g.write_data(0x0f);
    assert_eq!(g.cram[0x10], 0x0fff);
}

#[test]
fn mode4_background_scroll_and_column_mask() {
    let mut v = mode4(VdpKind::Sms2);
    // Name (0, 0) and (1, 0): pattern 1.
    vram_at(&mut v, 0x3800);
    for _ in 0..2 {
        v.write_data(1);
        v.write_data(0);
    }
    field(&mut v);
    field(&mut v);
    assert_eq!(pixel(&v, LEFT, 0), RED);
    assert_eq!(pixel(&v, LEFT + 15, 7), RED);
    assert_eq!(
        pixel(&v, LEFT + 16, 0),
        Rgb8::default(),
        "colour 0: CRAM 0, black"
    );
    // Fine scroll 3: the left 3 pixels show the backdrop, the tiles move.
    reg(&mut v, 8, 3);
    field(&mut v);
    assert_eq!(pixel(&v, LEFT + 2, 0), Rgb8::default(), "backdrop");
    assert_eq!(pixel(&v, LEFT + 18, 0), RED);
    assert_eq!(pixel(&v, LEFT + 19, 0), Rgb8::default());
    // Column 0 masked with the backdrop (register 7 = 1: CRAM 17, green).
    reg(&mut v, 8, 0);
    reg(&mut v, 7, 1);
    reg(&mut v, 0, 0x24);
    field(&mut v);
    assert_eq!(pixel(&v, LEFT, 0), GREEN);
    assert_eq!(pixel(&v, LEFT + 8, 0), RED);
}

#[test]
fn mode4_sprites_overflow_collision_and_priority() {
    let mut v = mode4(VdpKind::Sms2);
    // Nine sprites on line 50 (Y = 49), pattern 1, X = 100 + 4i: they
    // overlap (collision) and the ninth overflows.
    vram_at(&mut v, 0x3f00);
    for _ in 0..9 {
        v.write_data(49);
    }
    v.write_data(0xd0);
    vram_at(&mut v, 0x3f80);
    for i in 0..9u8 {
        v.write_data(100 + 4 * i);
        v.write_data(1);
    }
    run_to_line(&mut v, 60);
    let status = v.read_status();
    assert_eq!(status & 0x60, 0x60, "overflow and collision");
    field(&mut v);
    assert_eq!(pixel(&v, LEFT + 100, 50), GREEN, "sprites use CRAM 16-31");
    assert_eq!(pixel(&v, LEFT + 100, 49), Rgb8::default());
    assert_eq!(
        pixel(&v, LEFT + 100 + 4 * 8 + 7, 50),
        Rgb8::default(),
        "the ninth is not drawn"
    );
    // A background tile with the priority bit covers the sprite.
    vram_at(&mut v, 0x3800 + (50 / 8) * 64 + (100 / 8) * 2);
    v.write_data(1);
    v.write_data(0x10);
    field(&mut v);
    assert_eq!(pixel(&v, LEFT + 100, 50), RED, "tile column 12: X 96-103");
    assert_eq!(
        pixel(&v, LEFT + 104, 50),
        GREEN,
        "column 13 has no priority"
    );
}

#[test]
fn frame_and_line_interrupts() {
    let mut v = mode4(VdpKind::Sms2);
    reg(&mut v, 1, 0x60);
    field(&mut v);
    run_to_line(&mut v, 193);
    assert!(v.int(), "the frame interrupt on line $C1");
    assert_eq!(v.read_status() & 0x80, 0x80);
    assert!(!v.int(), "acknowledged");
    // Line interrupts every 10 lines (register 10 = 9).
    reg(&mut v, 1, 0x40);
    reg(&mut v, 10, 9);
    reg(&mut v, 0, 0x14);
    field(&mut v);
    let mut at = Vec::new();
    for line in 1..=60 {
        run_to_line(&mut v, line);
        if v.int() {
            at.push(line);
            v.read_status();
        }
    }
    assert_eq!(at, [9, 19, 29, 39, 49, 59]);
}

#[test]
fn v_counter_jumps_as_the_tables_give() {
    let mut v = mode4(VdpKind::Sms2);
    run_to_line(&mut v, 218);
    assert_eq!(v.v_counter(), 0xda);
    run_to_line(&mut v, 219);
    assert_eq!(v.v_counter(), 0xd5);
    run_to_line(&mut v, 261);
    assert_eq!(v.v_counter(), 0xff);
    let mut p = SmsVdp::new(VdpKind::Sms2, true);
    while p.line != 243 {
        p.tick();
    }
    assert_eq!(p.v_counter(), 0xba);
}

#[test]
fn the_224_line_screen_is_the_sms_2_s_only() {
    let mut v = mode4(VdpKind::Sms2);
    reg(&mut v, 0, 0x06);
    reg(&mut v, 1, 0x50);
    assert_eq!(v.height(), 224);
    assert_eq!(v.line_row(0), Some(11));
    let mut s1 = mode4(VdpKind::Sms1);
    reg(&mut s1, 0, 0x06);
    reg(&mut s1, 1, 0x50);
    assert_eq!(s1.height(), 192);
}

#[test]
fn graphics1_on_the_sms_2_uses_the_tms_colours() {
    let mut v = SmsVdp::new(VdpKind::Sms2, false);
    // Graphics I: names $1800, colours $2000, patterns $0000.
    for (r, val) in [
        (0, 0x00),
        (1, 0x40),
        (2, 0x06),
        (3, 0x80),
        (4, 0x00),
        (5, 0x36),
        (7, 0x04),
    ] {
        reg(&mut v, r, val);
    }
    vram_at(&mut v, 8);
    for _ in 0..8 {
        v.write_data(0xff);
    }
    vram_at(&mut v, 0x2000);
    v.write_data(0xf4);
    vram_at(&mut v, 0x1800);
    v.write_data(1);
    vram_at(&mut v, 0x1b00);
    v.write_data(0xd0);
    field(&mut v);
    field(&mut v);
    assert_eq!(pixel(&v, LEFT, 0), rgb(0xffffff), "colour 15: white");
    assert_eq!(
        pixel(&v, LEFT + 8, 0),
        rgb(0x5455ed),
        "colour 0: the backdrop 4"
    );
}

#[test]
fn the_game_gear_shows_its_window_in_12_bit_colour() {
    let mut v = mode4(VdpKind::GameGear);
    vram_at(&mut v, 0x3800 + 3 * 64 + 6 * 2);
    v.write_data(1);
    v.write_data(0);
    field(&mut v);
    field(&mut v);
    let (w, h, px) = v.frame();
    assert_eq!((w, h), (GG_WIDTH, GG_HEIGHT));
    // Tile (6, 3) is pixel (48, 24): the window's corner.
    assert_eq!(px[0], RED);
    assert_eq!(px[8], Rgb8::default());
}

#[test]
fn savestates_round_trip() {
    let mut v = mode4(VdpKind::Sms1);
    run_to_line(&mut v, 100);
    let mut w = StateWriter::new();
    v.save(&mut w);
    let bytes = w.into_bytes();
    let mut n = SmsVdp::new(VdpKind::Sms1, false);
    n.load(&mut StateReader::new(&bytes)).unwrap();
    for _ in 0..1000 {
        v.tick();
        n.tick();
    }
    assert_eq!((v.line, v.dot, v.regs), (n.line, n.dot, n.regs));
    assert!(n.load(&mut StateReader::new(&bytes[..10])).is_err());
}
