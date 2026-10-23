use super::*;

fn vram_write(p: &mut SPpu, word: u16, data: &[u16]) {
    p.write(0x15, 0x80); // increment after the high byte, step 1
    p.write(0x16, word as u8);
    p.write(0x17, (word >> 8) as u8);
    for w in data {
        p.write(0x18, *w as u8);
        p.write(0x19, (*w >> 8) as u8);
    }
}

fn cgram_write(p: &mut SPpu, index: u8, colours: &[u16]) {
    p.write(0x21, index);
    for c in colours {
        p.write(0x22, *c as u8);
        p.write(0x22, (*c >> 8) as u8);
    }
}

fn pixel(p: &SPpu, x: usize, y: usize) -> Rgb8 {
    p.frame()[p.row(y as u16) * WIDTH + x * 2 + 1]
}

const RED: u16 = 0x001f;
const GREEN: u16 = 0x03e0;
const BLUE: u16 = 0x7c00;
const WHITE: u16 = 0x7fff;

fn rgb8(c: u16) -> Rgb8 {
    rgb(c, 15)
}

/// Mode 1, BG1: map at word $0000, tiles at $1000; tile 1 is solid colour
/// 1 (4 bpp); the map has tile 1 at (0,0) and (1,1).
fn bg_scene() -> SPpu {
    let mut p = SPpu::new(false);
    p.write(0x05, 0x01);
    p.write(0x07, 0x00);
    p.write(0x0b, 0x01);
    let mut tile = vec![0u16; 16];
    // Rows 0-7: plane 0 set, plane 1 clear.
    tile[..8].fill(0x00ff);
    vram_write(&mut p, 0x1000 + 16, &tile);
    vram_write(&mut p, 0x0000, &[1]);
    vram_write(&mut p, 0x0021, &[1 | (1 << 10)]); // palette 1
    cgram_write(&mut p, 0, &[BLUE, RED]);
    cgram_write(&mut p, 17, &[GREEN]);
    p.write(0x2c, 0x01);
    p.write(0x00, 0x0f);
    p
}

#[test]
fn vram_cgram_and_oam_ports() {
    let mut p = SPpu::new(false);
    vram_write(&mut p, 0x1234, &[0xbeef, 0xcafe]);
    assert_eq!(p.vram[0x1234], 0xbeef);
    assert_eq!(p.vram[0x1235], 0xcafe);
    // Reading: the prefetch is loaded at the address write and again
    // before each step, so the first word comes twice.
    p.write(0x16, 0x34);
    p.write(0x17, 0x12);
    assert_eq!(p.read(0x39, 0, 0, true), 0xef);
    assert_eq!(p.read(0x3a, 0, 0, true), 0xbe);
    assert_eq!(p.read(0x3a, 0, 0, true), 0xbe, "prefetched before the step");
    assert_eq!(p.read(0x3a, 0, 0, true), 0xca);
    cgram_write(&mut p, 5, &[0x7abc]);
    assert_eq!(p.cgram[5], 0x7abc);
    p.write(0x21, 5);
    assert_eq!(p.read(0x3b, 0, 0, true), 0xbc);
    assert_eq!(p.read(0x3b, 0, 0, true) & 0x7f, 0x7a);
    // OAM: word writes below $200, bytes above.
    p.write(0x02, 0x00);
    p.write(0x03, 0x00);
    for v in [0x11, 0x22, 0x33, 0x44] {
        p.write(0x04, v);
    }
    assert_eq!(&p.oam[..4], &[0x11, 0x22, 0x33, 0x44]);
    p.write(0x02, 0x00);
    p.write(0x03, 0x01);
    p.write(0x04, 0x55);
    assert_eq!(p.oam[0x200], 0x55);
    // VMAIN step 32 and the 8-bit address translation.
    p.write(0x15, 0x81);
    p.write(0x16, 0);
    p.write(0x17, 0);
    p.write(0x19, 1);
    p.write(0x19, 2);
    assert_eq!(p.vram[32] >> 8, 2);
}

#[test]
fn backgrounds_scroll_and_palette() {
    let mut p = bg_scene();
    p.render_line(1);
    assert_eq!(pixel(&p, 0, 1), rgb8(RED), "tile 1, palette 0, colour 1");
    assert_eq!(pixel(&p, 8, 1), rgb8(BLUE), "backdrop");
    p.render_line(9);
    assert_eq!(pixel(&p, 8, 9), rgb8(GREEN), "palette 1");
    // Scroll 4 to the right: the tile's right half at x 0..3.
    p.write(0x0d, 4);
    p.write(0x0d, 0);
    p.render_line(1);
    assert_eq!(pixel(&p, 3, 1), rgb8(RED));
    assert_eq!(pixel(&p, 4, 1), rgb8(BLUE));
    // Master brightness and forced blank.
    p.write(0x00, 0x80);
    p.render_line(1);
    assert_eq!(pixel(&p, 0, 1), Rgb8::default());
}

#[test]
fn colour_math_and_windows() {
    let mut p = bg_scene();
    // Add the fixed colour (green) to BG1.
    p.write(0x32, 0x40 | 31);
    p.write(0x31, 0x01);
    p.render_line(1);
    assert_eq!(pixel(&p, 0, 1), rgb8(RED | GREEN));
    assert_eq!(pixel(&p, 8, 1), rgb8(BLUE), "backdrop: math bit 5 off");
    // Half: (red + green) / 2.
    p.write(0x31, 0x41);
    p.render_line(1);
    assert_eq!(pixel(&p, 0, 1), rgb8((15) | (15 << 5)));
    p.write(0x31, 0x00);
    // Window 1 over x 2..5 hides BG1 there on the main screen.
    p.write(0x26, 2);
    p.write(0x27, 5);
    p.write(0x23, 0x02);
    p.write(0x2e, 0x01);
    p.render_line(1);
    assert_eq!(pixel(&p, 1, 1), rgb8(RED));
    assert_eq!(pixel(&p, 2, 1), rgb8(BLUE));
    assert_eq!(pixel(&p, 6, 1), rgb8(RED));
}

#[test]
fn sprites_and_overflow_flags() {
    let mut p = bg_scene();
    // 8x8 sprites at word $4000 (OBSEL base 2); tile 0 solid colour 1.
    p.write(0x01, 0x02);
    let mut tile = vec![0u16; 16];
    tile[..8].fill(0x00ff);
    vram_write(&mut p, 0x4000, &tile);
    cgram_write(&mut p, 128 + 1, &[WHITE]);
    // OBJ 0 at (16, 10), priority 3; the others off screen.
    let mut oam = vec![0u8; 544];
    for i in 0..128 {
        oam[i * 4 + 1] = 0xf0;
    }
    oam[0..4].copy_from_slice(&[16, 10, 0, 0x30]);
    p.write(0x02, 0);
    p.write(0x03, 0);
    for b in &oam {
        p.write(0x04, *b);
    }
    p.write(0x2c, 0x11);
    p.render_line(11);
    assert_eq!(pixel(&p, 16, 11), rgb8(WHITE));
    assert_eq!(pixel(&p, 24, 11), rgb8(BLUE));
    p.render_line(10);
    assert_eq!(
        pixel(&p, 16, 10),
        rgb8(BLUE),
        "sprites start a line after Y"
    );
    // 33 sprites on one line: range overflow.
    for i in 0..33 {
        oam[i * 4..i * 4 + 4].copy_from_slice(&[(i * 7) as u8, 50, 0, 0]);
    }
    p.write(0x02, 0);
    p.write(0x03, 0);
    for b in &oam {
        p.write(0x04, *b);
    }
    p.render_line(52);
    assert_eq!(p.peek(0x3e) & 0x40, 0x40);
    p.start_frame();
    assert_eq!(p.peek(0x3e) & 0xc0, 0, "cleared at line 0");
}

#[test]
fn mode7_identity_matrix() {
    let mut p = SPpu::new(false);
    p.write(0x05, 0x07);
    // Map entry (0,0) = tile 1; tile 1's pixels all colour 3.
    p.vram[0] = 0x0001;
    for i in 0..64 {
        p.vram[64 + i] = 0x0300 | (p.vram[64 + i] & 0xff);
    }
    cgram_write(&mut p, 0, &[BLUE]);
    cgram_write(&mut p, 3, &[RED]);
    // A = D = 1.0, B = C = 0.
    for (reg, v) in [(0x1b, 0x0100u16), (0x1c, 0), (0x1d, 0), (0x1e, 0x0100)] {
        p.write(reg, v as u8);
        p.write(reg, (v >> 8) as u8);
    }
    p.write(0x1a, 0x80); // outside the map: transparent
    p.write(0x2c, 0x01);
    p.write(0x00, 0x0f);
    p.render_line(1);
    assert_eq!(pixel(&p, 0, 1), rgb8(RED));
    assert_eq!(pixel(&p, 8, 1), rgb8(BLUE), "tile 0 has colour 0");
    // The multiplier: M7A × high byte of M7B.
    p.write(0x1b, 0x00);
    p.write(0x1b, 0x02); // M7A = $0200
    p.write(0x1c, 0x03); // M7B high byte 3
    assert_eq!(p.peek(0x34), 0x00);
    assert_eq!(p.peek(0x35), 0x06);
}

#[test]
fn savestate_round_trip() {
    let mut p = bg_scene();
    p.render_line(5);
    let mut w = StateWriter::new();
    p.save(&mut w);
    let data = w.into_bytes();
    let mut q = SPpu::new(false);
    q.load(&mut StateReader::new(&data)).unwrap();
    q.frame = p.frame.clone();
    assert_eq!(q, p);
    assert!(q.load(&mut StateReader::new(&data[..100])).is_err());
}
