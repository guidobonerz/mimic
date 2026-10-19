use super::*;

struct Mem {
    ram: Vec<u8>,
    color: Vec<u8>,
}

impl Mem {
    fn new() -> Self {
        Self {
            ram: vec![0; 0x4000],
            color: vec![0; 0x400],
        }
    }
}

impl VicMemory for Mem {
    fn read(&self, addr: u16) -> u8 {
        self.ram[usize::from(addr & 0x3fff)]
    }

    fn color(&self, index: u16) -> u8 {
        self.color[usize::from(index & 0x3ff)]
    }
}

fn cycle(v: &mut Vic2, m: &Mem) -> bool {
    v.phi1(m);
    v.phi2(m)
}

fn frame(v: &mut Vic2, m: &Mem) {
    while !cycle(v, m) {}
}

/// Run until the next phi1 would be (line, cycle).
fn run_to(v: &mut Vic2, m: &Mem, line: u16, c: u8) {
    while !(v.raster() == line && v.cycle() == c) {
        cycle(v, m);
    }
}

/// Screen at $0400, charset at $1000, text mode, 25 rows, 40 columns.
fn text_setup() -> (Vic2, Mem) {
    let mut v = Vic2::new();
    v.write(0x11, 0x1b);
    v.write(0x16, 0x08);
    v.write(0x18, 0x14);
    v.write(0x20, 14);
    v.write(0x21, 6);
    (v, Mem::new())
}

/// Pixel at VIC X coordinate and raster line.
fn px(v: &Vic2, x: u16, line: u16) -> u8 {
    let col = usize::from((x + X_WRAP - VISIBLE_X0) % X_WRAP);
    let row = usize::from(line - FIRST_LINE);
    v.pixels()[row * WIDTH as usize + col]
}

#[test]
fn a_field_is_312_lines_of_63_cycles() {
    let mut v = Vic2::new();
    let m = Mem::new();
    let mut n = 1;
    while !cycle(&mut v, &m) {
        n += 1;
    }
    assert_eq!(n, 312 * 63);
    assert_eq!((v.raster(), v.cycle()), (0, 1));
    assert_eq!(v.beam().frame, 1);
}

#[test]
fn register_read_masks_and_mirrors() {
    let mut v = Vic2::new();
    for reg in 0..0x40 {
        v.write(reg, 0);
    }
    // Compare 0 equals raster line 0: that write raised the raster flag.
    assert_eq!(v.peek(0x19) & 0x01, 0x01);
    v.write(0x19, 0x0f);
    assert_eq!(v.peek(0x16), 0xc0);
    assert_eq!(v.peek(0x18), 0x01);
    assert_eq!(v.peek(0x19), 0x70);
    assert_eq!(v.peek(0x1a), 0xf0);
    assert_eq!(v.peek(0x20), 0xf0);
    assert_eq!(v.peek(0x2f), 0xff);
    assert_eq!(v.peek(0x3f), 0xff);
    v.write(0x60, 0x0a); // mirror of $20
    assert_eq!(v.peek(0x20), 0xfa);
    v.write(0x01, 0x80);
    v.write(0x10, 0x01);
    assert_eq!(v.sprites[0].x, 0x100);
}

#[test]
fn raster_irq_at_cycle_1_and_line_0_at_cycle_2() {
    let mut v = Vic2::new();
    let m = Mem::new();
    v.write(0x1a, 0x01);
    v.write(0x12, 100);
    run_to(&mut v, &m, 100, 1);
    assert!(!v.irq());
    v.phi1(&m);
    assert!(v.irq());
    assert_eq!(v.peek(0x19), 0xf1);
    v.write(0x19, 0x01);
    assert!(!v.irq());

    v.write(0x12, 0);
    run_to(&mut v, &m, 0, 1);
    v.phi1(&m);
    assert!(!v.irq(), "line 0 compares one cycle late");
    v.phi2(&m);
    v.phi1(&m);
    assert!(v.irq());
}

#[test]
fn writing_the_current_line_to_the_compare_register_triggers() {
    let mut v = Vic2::new();
    let m = Mem::new();
    v.write(0x1a, 0x01);
    run_to(&mut v, &m, 77, 30);
    v.write(0x12, 77);
    assert!(v.irq());
}

#[test]
fn bad_line_ba_low_from_cycle_12_to_54() {
    let (mut v, m) = text_setup();
    run_to(&mut v, &m, 0x33, 1);
    let mut low = Vec::new();
    for _ in 0..63 {
        v.phi1(&m);
        if !v.ba() {
            low.push(v.cycle());
        }
        v.phi2(&m);
    }
    assert_eq!(low, (12..=54).collect::<Vec<u8>>());
    // Next line is not a bad line.
    for _ in 0..63 {
        v.phi1(&m);
        assert!(v.ba());
        v.phi2(&m);
    }
}

#[test]
fn no_bad_lines_without_den_in_line_0x30() {
    let (mut v, m) = text_setup();
    v.write(0x11, 0x0b);
    run_to(&mut v, &m, 0x31, 1);
    v.write(0x11, 0x1b);
    run_to(&mut v, &m, 0x33, 20);
    v.phi1(&m);
    assert!(v.ba(), "DEN was off during line $30");
}

#[test]
fn sprite_dma_steals_cycles_55_to_59_for_sprite_0() {
    let mut v = Vic2::new();
    let m = Mem::new();
    v.write(0x15, 0x01);
    v.write(0x01, 100);
    run_to(&mut v, &m, 100, 50);
    let mut low = Vec::new();
    for _ in 0..13 {
        v.phi1(&m);
        if !v.ba() {
            low.push(v.cycle());
        }
        v.phi2(&m);
    }
    assert_eq!(low, [55, 56, 57, 58, 59]);
}

#[test]
fn text_mode_renders_characters_inside_the_border() {
    let (mut v, mut m) = text_setup();
    m.ram[0x0400] = 1; // screen code 1 in the top-left cell
    m.color[0] = 1; // white
    for row in 0..8 {
        m.ram[0x1000 + 8 + row] = 0xff; // char 1 is a filled block
    }
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 23, 60), 14, "left border");
    assert_eq!(px(&v, 24, 51), 1, "first cell, first pixel");
    assert_eq!(px(&v, 31, 58), 1, "first cell, last pixel");
    assert_eq!(px(&v, 32, 51), 6, "second cell: background");
    assert_eq!(px(&v, 24, 50), 14, "top border");
    assert_eq!(px(&v, 100, 250), 6);
    assert_eq!(px(&v, 100, 251), 14, "bottom border");
    assert_eq!(px(&v, 343, 100), 6);
    assert_eq!(px(&v, 344, 100), 14, "right border");
}

#[test]
fn xscroll_delays_the_graphics() {
    let (mut v, mut m) = text_setup();
    v.write(0x16, 0x0b); // XSCROLL 3
    m.ram[0x0400] = 1;
    m.color[0] = 1;
    m.ram[0x1008] = 0x80;
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 26, 51), 6);
    assert_eq!(px(&v, 27, 51), 1);
}

#[test]
fn den_off_shows_only_border() {
    let (mut v, m) = text_setup();
    v.write(0x11, 0x0b);
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 100, 100), 14);
}

fn sprite_setup(v: &mut Vic2, m: &mut Mem, n: usize, x: u16, y: u8, color: u8) {
    v.write(0x15, v.peek(0x15) | 1 << n);
    v.write(2 * n as u8, x as u8);
    v.write(2 * n as u8 + 1, y);
    v.write(0x27 + n as u8, color);
    m.ram[0x07f8 + n] = 0x80; // data at $2000
    for i in 0..63 {
        m.ram[0x2000 + i] = 0xff;
    }
}

#[test]
fn sprite_appears_at_its_coordinates_one_line_after_y() {
    let (mut v, mut m) = text_setup();
    sprite_setup(&mut v, &mut m, 0, 100, 100, 2);
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 100, 101), 2);
    assert_eq!(px(&v, 123, 121), 2, "24 × 21 pixels");
    assert_eq!(px(&v, 124, 101), 6);
    assert_eq!(px(&v, 99, 101), 6);
    assert_eq!(px(&v, 100, 100), 6);
    assert_eq!(px(&v, 100, 122), 6);
}

#[test]
fn sprite_expansion_doubles_size() {
    let (mut v, mut m) = text_setup();
    sprite_setup(&mut v, &mut m, 0, 100, 100, 2);
    v.write(0x17, 0x01);
    v.write(0x1d, 0x01);
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 147, 142), 2, "48 × 42 pixels");
    assert_eq!(px(&v, 148, 101), 6);
    assert_eq!(px(&v, 100, 143), 6);
}

#[test]
fn sprite_collisions_latch_and_clear_on_read() {
    let (mut v, mut m) = text_setup();
    sprite_setup(&mut v, &mut m, 0, 100, 100, 2);
    sprite_setup(&mut v, &mut m, 1, 110, 105, 3);
    v.write(0x1a, 0x04);
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 112, 110), 2, "sprite 0 wins over sprite 1");
    assert!(v.irq());
    assert_eq!(v.read(0x1e), 0x03);
    assert_eq!(v.read(0x1e), 0x00);
}

#[test]
fn sprite_behind_foreground_with_priority_bit() {
    let (mut v, mut m) = text_setup();
    for i in 0..40 {
        m.ram[0x0400 + i] = 1;
        m.color[i] = 1;
    }
    for row in 0..8 {
        m.ram[0x1008 + row] = 0xaa;
    }
    sprite_setup(&mut v, &mut m, 0, 40, 47, 2);
    v.write(0x1b, 0x01);
    frame(&mut v, &m);
    frame(&mut v, &m);
    assert_eq!(px(&v, 40, 52), 1, "foreground pixel covers the sprite");
    assert_eq!(px(&v, 41, 52), 2, "sprite over background");
    assert_eq!(v.read(0x1f), 0x01, "sprite-data collision");
}

#[test]
fn savestate_mid_frame_continues_identically() {
    let (mut a, mut m) = text_setup();
    sprite_setup(&mut a, &mut m, 0, 100, 100, 2);
    m.ram[0x0400] = 1;
    run_to(&mut a, &m, 120, 40);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = Vic2::new();
    b.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(a, b);
    frame(&mut a, &m);
    frame(&mut b, &m);
    assert_eq!(a, b);

    let before = b.clone();
    assert!(b.load(&mut StateReader::new(&data[..100])).is_err());
    assert_eq!(b, before);
}
