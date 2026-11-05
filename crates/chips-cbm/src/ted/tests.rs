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

fn run_frame(t: &mut Ted, mem: &Mem) -> u32 {
    let mut cpu = 0;
    loop {
        let o = t.tick(mem);
        cpu += u32::from(o.cpu);
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
        let o = t.tick(&mem);
        let (line, tick) = (t.line, t.tick);
        match t.bus(&mem) {
            Some(b) => {
                seen.insert(b);
            }
            None => border += 1,
        }
        // The CPU's double clocks in the fetch window carry no TED byte,
        // except on a row's first line (attributes).
        if o.cpu && t.fetched() {
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
