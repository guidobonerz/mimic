use super::*;

/// 64 KB of RAM plus a 16 KB "ROM" at `$C000`.
struct Mem {
    ram: Vec<u8>,
    rom: Vec<u8>,
}

impl Mem {
    fn new() -> Self {
        Self {
            ram: vec![0; 0x10000],
            rom: vec![0; 0x4000],
        }
    }
}

impl TedMemory for Mem {
    fn fetch(&self, addr: u16, rom: bool) -> u8 {
        if rom && addr >= 0xc000 {
            self.rom[usize::from(addr - 0xc000)]
        } else {
            self.ram[usize::from(addr)]
        }
    }
}

/// The KERNAL's text screen: matrix at `$0800`, characters from ROM at
/// `$D000`, 25 rows, 40 columns, YSCROLL 3.
fn kernal_text(t: &mut Ted) {
    for (reg, v) in [
        (0x06, 0x1b),
        (0x07, 0x08),
        (0x12, 0x04),
        (0x13, 0xd0),
        (0x14, 0x08),
        (0x15, 0x71), // white background
        (0x19, 0x6e), // border
        (0x0c, 0x03), // cursor out of the way
        (0x0d, 0xff),
    ] {
        t.write(reg, v);
    }
}

thread_local! {
    /// A single-clock cycle running on into the next frame.
    static SKIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// CPU cycles of a frame, single clock taking two double clocks as the
/// machine runs it.
fn run_frame(t: &mut Ted, mem: &Mem) -> u32 {
    let mut cpu = 0;
    loop {
        let single = t.single();
        let go = t.slot().0 && !(SKIP.get() && single);
        SKIP.set(go && single);
        let o = t.tick(mem);
        cpu += u32::from(go);
        if o.frame_end {
            return cpu;
        }
    }
}

/// Colour at line `line`, pixel `x` of the line (0 = first pixel of tick 0).
fn pixel(t: &Ted, line: u16, x: u16) -> u8 {
    let tm = t.timing();
    let y =
        (u32::from(line) + u32::from(tm.lines) - u32::from(tm.first_line)) % u32::from(tm.lines);
    t.pixels()[(y * WIDTH + u32::from(x - FRAME_LEFT)) as usize]
}

#[test]
fn text_mode_from_rom_characters() {
    let mut t = Ted::new(Model::Mos8360);
    kernal_text(&mut t);
    let mut mem = Mem::new();
    mem.ram[0x0c00] = 1; // character 1 in the first cell
    mem.ram[0x0800] = 0x32; // red, luminance 3
    mem.ram[0x0c01] = 0x81; // reversed in the second cell
    mem.ram[0x0801] = 0x06;
    mem.rom[0x1000 + 8] = 0xf0; // character 1, line 0
    run_frame(&mut t, &mem);
    run_frame(&mut t, &mem);
    // Row 0 starts at line 1 + YSCROLL = 4.
    assert_eq!(pixel(&t, 4, DISPLAY_X - 1), 0x6e, "border");
    assert_eq!(pixel(&t, 4, DISPLAY_X), 0x32);
    assert_eq!(pixel(&t, 4, DISPLAY_X + 4), 0x71, "background");
    assert_eq!(
        pixel(&t, 4, DISPLAY_X + 8),
        0x71,
        "reversed: set pixels clear"
    );
    assert_eq!(pixel(&t, 4, DISPLAY_X + 12), 0x06);
    assert_eq!(pixel(&t, 5, DISPLAY_X), 0x71, "line 1 of the character");
    assert_eq!(pixel(&t, 3, DISPLAY_X), 0x6e, "above the window");
    assert_eq!(pixel(&t, 203, DISPLAY_X + 319), 0x71);
    assert_eq!(pixel(&t, 204, DISPLAY_X), 0x6e, "below the window");
    assert_eq!(pixel(&t, 100, DISPLAY_X + 320), 0x6e, "right border");
}

#[test]
fn cpu_gets_double_clock_in_the_border_and_less_in_the_display() {
    let mut t = Ted::new(Model::Mos8360);
    let mem = Mem::new();
    run_frame(&mut t, &mem);
    let blank = run_frame(&mut t, &mem);
    // Display off: every double clock but the five refresh cycles, which
    // take two each.
    assert_eq!(blank, 312 * (104 + 5), "display off");
    kernal_text(&mut t);
    // (the first frame starts with the switch, whose single-clock phase
    // depends on where it fell)
    run_frame(&mut t, &mem);
    let display = run_frame(&mut t, &mem);
    // Fetch lines (counter 0–203): 49 single-clock cycles and 16 double
    // clocks; the 50 DMA lines (before and at the start of each of the 25
    // rows) lose the 40 cycles of the DMA; the other 108 lines as blank.
    let expected = 108 * 109 + (204 - 50) * 65 + 50 * (65 - 40);
    assert_eq!(display, expected);
    t.write(0x13, 0xd2); // force single clock
    assert_eq!(run_frame(&mut t, &mem), (312 - 50) * 57 + 50 * (57 - 40));
}

#[test]
fn raster_interrupt_and_registers() {
    let mut t = Ted::new(Model::Mos7360);
    let mem = Mem::new();
    t.write(0x0b, 0x2c);
    t.write(0x0a, 0x03); // raster IRQ, compare bit 8 set: line $12C does not exist on NTSC
    run_frame(&mut t, &mem);
    assert!(!t.irq());
    t.write(0x0a, 0x02); // line $2C
    while t.beam().line != 0x2d {
        t.tick(&mem);
    }
    assert!(t.irq());
    assert_eq!(t.peek(0x09) & 0x82, 0x82);
    t.write(0x09, 0x02);
    assert!(!t.irq());
    assert_eq!(t.peek(0x1d), 0x2d);
    assert_eq!(t.peek(0x1c) & 1, 0);
}

#[test]
fn timers_count_single_clocks() {
    let mut t = Ted::new(Model::Mos8360);
    let mem = Mem::new();
    t.write(0x0a, 0x08);
    t.write(0x00, 99);
    t.write(0x01, 0); // timer 1 = 99, running
    for _ in 0..198 {
        t.tick(&mem);
    }
    assert!(!t.irq());
    for _ in 0..2 {
        t.tick(&mem);
    }
    assert!(t.irq(), "after 100 single clocks");
    assert_eq!(t.peek(0x00), 99, "reloaded");
    t.write(0x09, 0x08);
    t.write(0x00, 5); // writing the low byte stops it
    for _ in 0..50 {
        t.tick(&mem);
    }
    assert!(!t.irq());
}

#[test]
fn voice_pitch() {
    let mut t = Ted::new(Model::Mos8360);
    let mem = Mem::new();
    // 886 723 / 8 / (1024 − 0x3a0) ≈ 1155 Hz.
    t.write(0x0e, 0xa0);
    t.write(0x12, 0x03);
    t.write(0x11, 0x18);
    for _ in 0..50 {
        run_frame(&mut t, &mem);
    }
    let s = std::mem::take(&mut t.samples);
    let high = *s.iter().max().unwrap();
    let edges = s
        .windows(2)
        .filter(|w| w[0] != high && w[1] == high)
        .count();
    let seconds = s.len() as f64 / 48_000.0;
    let hz = edges as f64 / seconds;
    assert!((1120.0..1190.0).contains(&hz), "{hz} Hz");
}

#[test]
fn savestate_round_trip() {
    let mut t = Ted::new(Model::Mos8360);
    kernal_text(&mut t);
    t.write(0x0e, 0x10);
    t.write(0x11, 0x38);
    t.write(0x02, 0x34);
    t.write(0x03, 0x12);
    let mut mem = Mem::new();
    mem.ram[0x0c00] = 1;
    mem.rom[0x1008] = 0x81;
    for _ in 0..40_000 {
        t.tick(&mem);
    }
    let mut w = StateWriter::new();
    t.save(&mut w);
    let data = w.into_bytes();
    let mut u = Ted::new(Model::Mos8360);
    u.load(&mut StateReader::new(&data)).unwrap();
    t.samples.clear();
    assert_eq!(u, t);
    for _ in 0..50_000 {
        u.tick(&mem);
        t.tick(&mem);
    }
    assert_eq!(u, t);
    assert!(Ted::new(Model::Mos7360)
        .load(&mut StateReader::new(&data))
        .is_err());
}

#[test]
fn data_bus_carries_the_fetches_of_display_lines() {
    let mut t = Ted::new(Model::Mos8360);
    let mut mem = Mem::new();
    kernal_text(&mut t);
    // Character 1 everywhere, its first line $5A, attributes $33.
    mem.ram[0x0800..0x0c00].fill(0x33);
    mem.ram[0x0c00..0x1000].fill(0x01);
    for line in 0..8 {
        mem.rom[0x1008 + line] = 0x5a + line as u8;
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut border = 0;
    loop {
        let single = t.single();
        let o = t.tick(&mem);
        let (line, tick) = (t.line, t.tick);
        match t.bus() {
            Some(b) => {
                seen.insert(b);
            }
            None => border += 1,
        }
        // The CPU's double-clock cycles carry no TED byte (in single clock
        // the TED takes the second half),
        // except on a row's first line (attributes).
        if o.cpu && t.fetched() && !single {
            panic!("TED byte in a CPU slot at line {line}, tick {tick}");
        }
        if o.frame_end {
            break;
        }
    }
    assert!(border > 0);
    // Pattern bytes of every character line and the attribute byte.
    for b in 0x5a..0x62 {
        assert!(seen.contains(&b), "pattern {b:02X} missing");
    }
    assert!(seen.contains(&0x33), "attribute missing");
}

#[test]
fn multicolour_bitmap_pairs() {
    // Pair 01 takes the colour of a hires 1 dot (matrix bits 4–7,
    // attribute bits 0–2), pair 10 that of a 0 dot (matrix bits 0–3,
    // attribute bits 4–6): measured against xplus4 on Promised Land's
    // "Gods of the Universe" picture.
    let mut t = Ted::new(Model::Mos8360);
    kernal_text(&mut t);
    t.write(0x06, 0x3b); // bitmap
    t.write(0x07, 0x18); // multicolour
    t.write(0x12, 0x08); // bitmap at $2000
    t.write(0x15, 0x00);
    t.write(0x16, 0x47);
    let mut mem = Mem::new();
    mem.ram[0x2000] = 0b0001_1011; // pairs 00, 01, 10, 11
    mem.ram[0x0c00] = 0x25;
    mem.ram[0x0800] = 0x63;
    run_frame(&mut t, &mem);
    run_frame(&mut t, &mem);
    let at = |pair: u16| pixel(&t, 4, DISPLAY_X + 2 * pair);
    assert_eq!(at(0), 0x00, "pair 00: $FF15");
    assert_eq!(at(1), 0x32, "pair 01");
    assert_eq!(at(2), 0x65, "pair 10");
    assert_eq!(at(3), 0x47, "pair 11: $FF16");
}

/// Run until the line counter turns to `line` (picture line `line - 1`'s
/// tick `TURN`), or to the end of the field.
fn run_to_counter_line(t: &mut Ted, mem: &Mem, line: u16) {
    while t.counter_line() != line || t.tick != TURN {
        if t.tick(mem).frame_end {
            return;
        }
    }
}

/// Matrix A at $0800 (attribute $32, character 1), matrix B at $1000
/// (attribute $65, character 2); characters 1 and 2 solid, 0 empty.
fn two_matrices() -> Mem {
    let mut mem = Mem::new();
    mem.ram[0x0800..0x0c00].fill(0x32);
    mem.ram[0x0c00..0x1000].fill(0x01);
    mem.ram[0x1000..0x1400].fill(0x65);
    mem.ram[0x1400..0x1800].fill(0x02);
    mem.rom[0x1008..0x1018].fill(0xff);
    mem
}

#[test]
fn a_row_keeps_the_attributes_and_codes_its_dma_fetched() {
    // $FF14 switched to B in the middle of row 14 (lines 116–123): the
    // row's last lines keep A's colour, row 15 takes B's (xplus4,
    // `ted_timing` matrix/matrixc).
    let mut t = Ted::new(Model::Mos8360);
    kernal_text(&mut t);
    let mem = two_matrices();
    run_frame(&mut t, &mem);
    run_to_counter_line(&mut t, &mem, 120);
    t.write(0x14, 0x10);
    run_frame(&mut t, &mem);
    let x = DISPLAY_X + 100;
    assert_eq!(pixel(&t, 119, x), 0x32);
    assert_eq!(pixel(&t, 123, x), 0x32, "row 14 keeps its buffers");
    assert_eq!(pixel(&t, 124, x), 0x65, "row 15 from matrix B");
}

#[test]
fn fli_on_every_line_freezes_the_rows_buffers() {
    // YSCROLL = (L + 1) & 7 and $FF14 alternating on every line L: every
    // line is an attribute DMA line, none a pointer DMA line, so neither
    // attributes nor codes reach the picture after the field's start
    // (xplus4, `ted_timing` fli2-fli4).
    let mut t = Ted::new(Model::Mos8360);
    kernal_text(&mut t);
    let mut mem = two_matrices();
    mem.rom[0x1000..0x1008].fill(0xff); // character 0 solid too
    for _ in 0..3 {
        for line in 2..200u16 {
            run_to_counter_line(&mut t, &mem, line);
            // late in the line, after its fetches, as the CPU manages it
            while t.tx() != 104 {
                t.tick(&mem);
            }
            t.write(0x06, 0x18 | ((line + 1) & 7) as u8);
            t.write(0x14, if line % 2 == 0 { 0x10 } else { 0x08 });
        }
        t.write(0x06, 0x1b);
        t.write(0x14, 0x08);
        run_to_counter_line(&mut t, &mem, 0);
    }
    let x = DISPLAY_X + 100;
    let colours: std::collections::BTreeSet<u8> = (10..190).map(|l| pixel(&t, l, x)).collect();
    // The attributes of the last DMA before the field's start (line 199,
    // matrix B) stand all field long.
    assert_eq!(
        colours,
        std::collections::BTreeSet::from([0x65]),
        "one colour under FLI"
    );
}
