use super::*;

fn reg(v: &mut MdVdp, r: u8, value: u8) {
    v.write_control(0x8000 | u16::from(r) << 8 | u16::from(value));
}

/// A command word for `code` (0–$3F) at `addr`.
fn command(v: &mut MdVdp, code: u8, addr: u16) -> Option<DmaRequest> {
    let first = u16::from(code & 3) << 14 | (addr & 0x3fff);
    let second = u16::from(code & 0x3c) << 2 | addr >> 14;
    v.write_control(first);
    v.write_control(second)
}

fn field(v: &mut MdVdp) {
    while !v.advance(LINE_MCLK) {}
}

fn run_to_line(v: &mut MdVdp, line: u16) {
    while v.line != line {
        v.advance(LINE_MCLK - v.pos);
    }
}

fn pixel(v: &MdVdp, x: u32, line: u16) -> Rgb8 {
    let (w, _, px) = v.frame();
    let left = if v.h40() { H40_LEFT } else { H32_LEFT } as u32;
    let row = v.line_row(line).unwrap();
    px[(row * w + left + x) as usize]
}

const RED: Rgb8 = Rgb8 { r: 255, g: 0, b: 0 };
const GREEN: Rgb8 = Rgb8 { r: 0, g: 255, b: 0 };
const BLUE: Rgb8 = Rgb8 { r: 0, g: 0, b: 255 };
const BLACK: Rgb8 = Rgb8 { r: 0, g: 0, b: 0 };

/// H40, display on, plane A $C000, B $E000, window $B000, sprites $A800,
/// hscroll $AC00, 64 × 32 planes, increment 2; pattern 1 solid colour 1;
/// CRAM 1 red, 17 green, 33 blue.
fn setup() -> MdVdp {
    let mut v = MdVdp::new(false);
    for (r, val) in [
        (0, 0x04),
        (1, 0x54),
        (2, 0x30),
        (3, 0x2c),
        (4, 0x07),
        (5, 0x54),
        (12, 0x81),
        (13, 0x2b),
        (15, 0x02),
        (16, 0x01),
    ] {
        reg(&mut v, r, val);
    }
    command(&mut v, 1, 32);
    for _ in 0..16 {
        v.write_data(0x1111);
    }
    for (i, c) in [(1u8, 0x000e), (17, 0x00e0), (33, 0x0e00)] {
        command(&mut v, 3, u16::from(i) * 2);
        v.write_data(c);
    }
    v
}

#[test]
fn command_words_reach_vram_cram_and_vsram() {
    let mut v = MdVdp::new(false);
    reg(&mut v, 15, 2);
    command(&mut v, 1, 0x1234);
    v.write_data(0xabcd);
    assert_eq!(&v.vram[0x1234..0x1236], &[0xab, 0xcd]);
    command(&mut v, 1, 0x2001);
    v.write_data(0xabcd);
    assert_eq!(&v.vram[0x2000..0x2002], &[0xcd, 0xab], "odd address swaps");
    command(&mut v, 3, 0x7e);
    v.write_data(0xffff);
    assert_eq!(v.cram[63], 0x0eee);
    command(&mut v, 5, 2);
    v.write_data(0x07ff);
    assert_eq!(v.vsram[1], 0x3ff);
    command(&mut v, 0, 0x1234);
    assert_eq!(v.read_data(), 0xabcd);
    // A register write clears the code.
    reg(&mut v, 7, 0x12);
    assert_eq!((v.regs[7], v.code), (0x12, 0));
}

#[test]
fn planes_scroll_and_priority() {
    let mut v = setup();
    // Plane B, cell (0, 0): pattern 1 palette 1 (green); plane A, cell
    // (1, 0): pattern 1 palette 0 (red).
    command(&mut v, 1, 0xe000);
    v.write_data(0x2001);
    command(&mut v, 1, 0xc002);
    v.write_data(0x0001);
    field(&mut v);
    field(&mut v);
    assert_eq!(pixel(&v, 0, 0), GREEN);
    assert_eq!(pixel(&v, 8, 0), RED);
    assert_eq!(pixel(&v, 16, 0), BLACK, "backdrop colour 0");
    // Plane A scrolled 4 pixels right covers half of B's cell.
    command(&mut v, 1, 0xac00);
    v.write_data(4);
    field(&mut v);
    assert_eq!(pixel(&v, 12, 0), RED);
    assert_eq!(pixel(&v, 19, 0), RED);
    assert_eq!(pixel(&v, 20, 0), BLACK);
    // Vertical scroll of plane B by 8 lines moves its cell up.
    command(&mut v, 5, 2);
    v.write_data(8);
    field(&mut v);
    assert_eq!(pixel(&v, 0, 0), BLACK);
}

#[test]
fn the_window_replaces_plane_a() {
    let mut v = setup();
    command(&mut v, 1, 0xc000);
    for _ in 0..64 {
        v.write_data(0x0001);
    }
    // Window cells: palette 2 (blue), first 2 columns (WHP = 1).
    command(&mut v, 1, 0xb000);
    v.write_data(0x4001);
    v.write_data(0x4001);
    reg(&mut v, 17, 0x01);
    field(&mut v);
    field(&mut v);
    assert_eq!(pixel(&v, 0, 0), BLUE);
    assert_eq!(pixel(&v, 15, 0), BLUE);
    assert_eq!(pixel(&v, 16, 0), RED);
}

fn sprite(v: &mut MdVdp, n: u16, x: u16, y: u16, size: u8, link: u8, attr: u16) {
    command(v, 1, 0xa800 + n * 8);
    v.write_data(y + 128);
    v.write_data(u16::from(size) << 8 | u16::from(link));
    v.write_data(attr);
    v.write_data(x + 128);
}

#[test]
fn sprites_link_overflow_and_collide() {
    let mut v = setup();
    // 21 sprites on line 10, linked 0 → 1 → … → 20; palette 2 (blue).
    for n in 0..21u16 {
        let link = if n == 20 { 0 } else { n as u8 + 1 };
        sprite(&mut v, n, 4 * n, 10, 0, link, 0x4001);
    }
    field(&mut v);
    run_to_line(&mut v, 20);
    let status = v.read_status();
    assert_eq!(status & 0x60, 0x60, "overflow and collision");
    field(&mut v);
    assert_eq!(pixel(&v, 0, 10), BLUE);
    assert_eq!(pixel(&v, 4 * 19 + 7, 10), BLUE);
    assert_eq!(pixel(&v, 4 * 20 + 7, 10), BLACK, "the 21st is not drawn");
    assert_eq!(pixel(&v, 0, 9), BLACK);
}

#[test]
fn interrupts_and_counters() {
    let mut v = setup();
    reg(&mut v, 1, 0x74);
    field(&mut v);
    v.acknowledge(6);
    run_to_line(&mut v, 224);
    assert_eq!(v.ipl(), 0, "not before H counter $08");
    v.advance(VINT_H40 + 1);
    assert_eq!(v.ipl(), 6);
    assert!(v.z80_int());
    v.acknowledge(6);
    assert_eq!(v.ipl(), 0);
    // Line interrupts every 8 lines.
    reg(&mut v, 10, 7);
    reg(&mut v, 0, 0x14);
    field(&mut v);
    v.acknowledge(4);
    let mut at = Vec::new();
    for line in 1..=40 {
        run_to_line(&mut v, line);
        if v.ipl() == 4 {
            at.push(line);
            v.acknowledge(4);
        }
    }
    assert_eq!(at, [8, 16, 24, 32, 40]);
    // The V counter jumps from $EA to $E5 (NTSC 224 lines).
    run_to_line(&mut v, 0xea);
    assert_eq!(v.hv_counter() >> 8, 0xea);
    run_to_line(&mut v, 0xeb);
    assert_eq!(v.hv_counter() >> 8, 0xe5);
    // The H counter at the line start: $A5 (H40).
    assert_eq!(v.hv_counter() & 0xff, 0xa5);
    v.advance(ACTIVE_H40);
    assert_eq!(v.hv_counter() & 0xff, 0x00);
}

#[test]
fn dma_fill_copy_and_transfer() {
    let mut v = setup();
    // Fill $1000.. with $AB, 16 bytes, increment 1.
    reg(&mut v, 15, 1);
    reg(&mut v, 19, 16);
    reg(&mut v, 20, 0);
    reg(&mut v, 23, 0x80);
    assert!(command(&mut v, 0x21, 0x1000).is_none());
    v.write_data(0xab00);
    assert!(
        v.vram[0x1000..0x1010].iter().all(|&b| b == 0xab),
        "{:x?}",
        &v.vram[0x1000..0x1012]
    );
    assert_ne!(v.status() & 2, 0, "DMA busy");
    // Copy those 16 bytes to $2000.
    reg(&mut v, 19, 16);
    reg(&mut v, 21, 0x00);
    reg(&mut v, 22, 0x10);
    reg(&mut v, 23, 0xc0);
    command(&mut v, 0x30, 0x2000);
    assert_eq!(v.vram[0x2000..0x2010], v.vram[0x1000..0x1010]);
    // 68000 → CRAM: 4 words from $FF0000.
    reg(&mut v, 15, 2);
    reg(&mut v, 19, 4);
    reg(&mut v, 20, 0);
    reg(&mut v, 21, 0x00);
    reg(&mut v, 22, 0x80);
    reg(&mut v, 23, 0x7f);
    let req = command(&mut v, 0x23, 0).expect("a 68000 transfer");
    assert_eq!(
        req,
        DmaRequest {
            source: 0xff0000,
            words: 4
        }
    );
    for w in [0x0002, 0x0004, 0x0006, 0x0008] {
        assert!(v.dma_word(w));
    }
    assert_eq!(&v.cram[..4], &[2, 4, 6, 8]);
    // Without register 1 bit 4 nothing starts.
    reg(&mut v, 1, 0x44);
    assert!(command(&mut v, 0x23, 0).is_none());
}

#[test]
fn shadow_and_highlight() {
    let mut v = setup();
    reg(&mut v, 12, 0x89);
    command(&mut v, 1, 0xc000);
    v.write_data(0x0001); // low priority: shadowed
    v.write_data(0x8001); // high priority: normal
    field(&mut v);
    field(&mut v);
    assert_eq!(pixel(&v, 0, 0), Rgb8 { r: 127, g: 0, b: 0 });
    assert_eq!(pixel(&v, 8, 0), RED);
}

#[test]
fn h32_and_pal_240_lines() {
    let mut v = setup();
    reg(&mut v, 12, 0x00);
    field(&mut v);
    field(&mut v);
    assert_eq!(v.frame().0, H32_WIDTH);
    let mut p = MdVdp::new(true);
    reg(&mut p, 1, 0x4c);
    assert_eq!(p.height(), 240);
    assert_eq!(p.frame().1, 294);
    assert_eq!(p.lines(), 313);
}

#[test]
fn savestates_round_trip() {
    let mut v = setup();
    run_to_line(&mut v, 100);
    let mut w = StateWriter::new();
    v.save(&mut w);
    let bytes = w.into_bytes();
    let mut n = MdVdp::new(false);
    n.load(&mut StateReader::new(&bytes)).unwrap();
    field(&mut v);
    field(&mut n);
    assert_eq!(v.frame().2, n.frame().2);
    assert!(n.load(&mut StateReader::new(&bytes[..30])).is_err());
}
