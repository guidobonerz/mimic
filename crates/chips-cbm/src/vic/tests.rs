use super::*;

/// 16 KB of VIC memory plus 1 KB of colour nibbles.
struct Mem {
    bytes: Vec<u8>,
    color: Vec<u8>,
}

impl Mem {
    fn new() -> Self {
        Self {
            bytes: vec![0; 0x4000],
            color: vec![0; 0x400],
        }
    }
}

impl VicMemory for Mem {
    fn read(&self, addr: u16) -> u8 {
        self.bytes[usize::from(addr & 0x3fff)]
    }

    fn color(&self, index: u16) -> u8 {
        self.color[usize::from(index & 0x3ff)]
    }
}

/// The PAL KERNAL's set-up for an unexpanded VIC-20: 22 × 23 characters
/// at screen `$1E00` (VIC `$3E00`), characters from VIC `$0000`, white
/// background, cyan border, normal mode.
fn kernal_pal(v: &mut Vic) {
    for (reg, value) in [
        (0, 12),
        (1, 38),
        (2, 0x96),
        (3, 0x2e),
        (5, 0xf0),
        (14, 0),
        (15, 0x1b),
    ] {
        v.write(reg, value);
    }
}

fn run_frame(v: &mut Vic, mem: &Mem) {
    while !v.tick(mem) {}
}

/// The pixel at `x` counted from the start of cycle 0.
fn pixel(v: &Vic, line: u16, x: u32) -> u8 {
    let t = v.timing();
    let y = u32::from(line - t.first_line);
    v.pixels()[(y * t.width() + (x + t.x_offset) % t.width()) as usize]
}

#[test]
fn draws_characters_inside_the_border() {
    let mut v = Vic::new(Model::Mos6561);
    kernal_pal(&mut v);
    let mut mem = Mem::new();
    mem.bytes[0x3e00] = 1; // first cell: character 1
    mem.bytes[0x3e01] = 1; // second cell too
    mem.bytes[8] = 0xf0; // character 1, line 0: left half set
    mem.color[0x200] = 6; // blue
    mem.color[0x201] = 2; // red
    run_frame(&mut v, &mem);
    run_frame(&mut v, &mem);

    // Text starts at line 38 × 2 = 76 and cycle 12 (pixel 48).
    assert_eq!(pixel(&v, 76, 47), 3, "border");
    assert_eq!(pixel(&v, 76, 48), 6, "set pixel in character colour");
    assert_eq!(pixel(&v, 76, 51), 6);
    assert_eq!(pixel(&v, 76, 52), 1, "clear pixel in background colour");
    assert_eq!(pixel(&v, 76, 56), 2, "second cell");
    assert_eq!(pixel(&v, 77, 48), 1, "line 1 of the character is empty");
    assert_eq!(pixel(&v, 75, 48), 3, "above the text window");
    // 22 columns of 8 pixels end at pixel 48 + 176.
    assert_eq!(pixel(&v, 77, 48 + 175), 1);
    assert_eq!(pixel(&v, 77, 48 + 176), 3);
    // 23 rows of 8 lines end at line 76 + 184.
    assert_eq!(pixel(&v, 76 + 183, 48), 1);
    assert_eq!(pixel(&v, 76 + 184, 48), 3);
}

#[test]
fn reverse_mode_and_multicolour() {
    let mut v = Vic::new(Model::Mos6561);
    kernal_pal(&mut v);
    v.write(15, 0x13); // white background, reverse mode, cyan border
    v.write(14, 0x70); // auxiliary colour yellow
    let mut mem = Mem::new();
    mem.bytes[0x3e00] = 1;
    mem.bytes[0x3e01] = 1;
    mem.bytes[8] = 0b0001_1011; // multicolour pairs 00 01 10 11
    mem.color[0x200] = 6; // hires blue
    mem.color[0x201] = 8 | 2; // multicolour red
    run_frame(&mut v, &mem);
    run_frame(&mut v, &mem);
    // Reverse: clear pixels take the character colour, set ones the background.
    assert_eq!(pixel(&v, 76, 48), 6);
    assert_eq!(pixel(&v, 76, 48 + 3), 1);
    let mc: Vec<u8> = (0..8).map(|i| pixel(&v, 76, 56 + i)).collect();
    assert_eq!(mc, [1, 1, 3, 3, 2, 2, 7, 7]);
}

#[test]
fn raster_register_counts_lines() {
    let mut v = Vic::new(Model::Mos6560);
    let mem = Mem::new();
    let t = v.timing();
    for _ in 0..u32::from(t.cycles) * 131 {
        v.tick(&mem);
    }
    assert_eq!(v.peek(4), 131 >> 1);
    assert_eq!(v.peek(3) >> 7, 1);
    run_frame(&mut v, &mem);
    assert_eq!(v.beam().line, 0);
    assert_eq!(v.beam().frame, 1);
}

#[test]
fn ntsc_and_pal_geometry() {
    assert_eq!(PAL.cpu_hz(), 1_108_404);
    assert_eq!(NTSC.cpu_hz(), 1_022_727);
    assert_eq!(PAL.width(), 284);
    assert_eq!(NTSC.width(), 260);
    assert!(u32::from(PAL.first_line) + PAL.height <= u32::from(PAL.lines));
    assert!(u32::from(NTSC.first_line) + NTSC.height <= u32::from(NTSC.lines));
}

#[test]
fn a_voice_makes_a_square_wave() {
    let mut v = Vic::new(Model::Mos6561);
    let mem = Mem::new();
    v.write(14, 15);
    run_frame(&mut v, &mem);
    let silent = std::mem::take(&mut v.samples);
    assert!(silent.windows(2).all(|w| w[0] == w[1]), "all voices off");

    v.write(0x0c, 0x80 | 100); // soprano: 1 108 404 / (64 × 28) ≈ 618 Hz
    run_frame(&mut v, &mem);
    let tone = std::mem::take(&mut v.samples);
    let high = *tone.iter().max().unwrap();
    let low = *tone.iter().min().unwrap();
    assert!(high > low);
    let flips = tone
        .windows(2)
        .filter(|w| (w[0] == high) != (w[1] == high))
        .count();
    // One PAL frame is 20 ms: about 12 periods, 24 edges.
    assert!((20..=28).contains(&flips), "{flips} edges");
}

#[test]
fn savestate_round_trip() {
    let mut v = Vic::new(Model::Mos6561);
    kernal_pal(&mut v);
    v.write(0x0a, 0x80 | 7);
    v.write(0x0d, 0x80 | 90);
    v.write(14, 9);
    let mut mem = Mem::new();
    mem.bytes[0x3e00] = 1;
    mem.bytes[8] = 0x81;
    for _ in 0..50_000 {
        v.tick(&mem);
    }
    let mut w = StateWriter::new();
    v.save(&mut w);
    let data = w.into_bytes();
    let mut u = Vic::new(Model::Mos6561);
    u.load(&mut StateReader::new(&data)).unwrap();
    v.samples.clear();
    assert_eq!(u, v);
    for _ in 0..30_000 {
        u.tick(&mem);
        v.tick(&mem);
    }
    assert_eq!(u, v);

    let mut ntsc = Vic::new(Model::Mos6560);
    assert!(ntsc.load(&mut StateReader::new(&data)).is_err());
    assert!(u
        .load(&mut StateReader::new(&data[..data.len() - 1]))
        .is_err());
}

#[test]
fn display_offsets_stay_inside_the_line() {
    for t in [&PAL, &NTSC] {
        assert!(t.x_offset < t.width());
    }
}

#[test]
fn frame_rows_follow_the_visible_lines() {
    let mem = Mem::new();
    for model in [Model::Mos6561, Model::Mos6560] {
        let mut v = Vic::new(model);
        let t = v.timing();
        assert_eq!(v.frame_row(t.first_line - 1), None);
        assert_eq!(v.frame_row(t.first_line), Some(0));
        assert_eq!(
            v.frame_row(t.first_line + t.height as u16 - 1),
            Some(t.height - 1)
        );
        assert_eq!(v.frame_row(t.first_line + t.height as u16), None);
        // Border colour 5 on line 100 only: that row of the frame.
        while v.line != 100 {
            v.tick(&mem);
        }
        v.write(15, 5);
        while v.line != 101 {
            v.tick(&mem);
        }
        v.write(15, 0);
        while !v.tick(&mem) {}
        let row = v.frame_row(100).unwrap();
        let px = |r: u32| v.pixels()[(r * t.width()) as usize];
        assert_eq!((px(row - 1), px(row), px(row + 1)), (0, 5, 0));
    }
}

#[test]
fn hpos_is_the_frame_column_of_the_cycle() {
    let mem = Mem::new();
    for model in [Model::Mos6561, Model::Mos6560] {
        let mut v = Vic::new(model);
        let t = v.timing();
        while !(v.line == 100 && v.cycle == 20) {
            v.tick(&mem);
        }
        let x = u32::from(v.beam().hpos);
        v.write(15, 5);
        while v.line != 101 {
            v.tick(&mem);
        }
        v.write(15, 0);
        while !v.tick(&mem) {}
        let at = v.frame_row(100).unwrap() * t.width();
        let px = |c: u32| v.pixels()[(at + c) as usize];
        assert_eq!((px(x - 1), px(x)), (0, 5));
    }
}
