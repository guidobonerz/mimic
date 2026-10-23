use super::*;

/// 8 KB of pattern memory and 2 KB of nametables, vertically mirrored.
struct Vram {
    chr: Vec<u8>,
    nt: Vec<u8>,
    reads: Vec<u16>,
}

impl Vram {
    fn new() -> Self {
        Self {
            chr: vec![0; 0x2000],
            nt: vec![0; 0x800],
            reads: Vec::new(),
        }
    }
}

impl PpuBus for Vram {
    fn read(&mut self, addr: u16) -> u8 {
        self.reads.push(addr);
        self.peek(addr)
    }
    fn write(&mut self, addr: u16, v: u8) {
        if addr < 0x2000 {
            self.chr[usize::from(addr)] = v;
        } else {
            self.nt[usize::from(addr & 0x7ff)] = v;
        }
    }
    fn peek(&self, addr: u16) -> u8 {
        if addr < 0x2000 {
            self.chr[usize::from(addr)]
        } else {
            self.nt[usize::from(addr & 0x7ff)]
        }
    }
}

/// A PPU past its warm-up, at line 0 dot 0.
fn ready(model: PpuModel) -> (Ppu, Vram) {
    let mut ppu = Ppu::new(model);
    let mut bus = Vram::new();
    while ppu.warmup || ppu.line != 0 || ppu.dot != 0 {
        ppu.tick(&mut bus);
    }
    (ppu, bus)
}

fn run_to(ppu: &mut Ppu, bus: &mut Vram, line: u16, dot: u16) {
    while ppu.line != line || ppu.dot != dot {
        ppu.tick(bus);
    }
}

fn set_addr(ppu: &mut Ppu, bus: &mut Vram, addr: u16) {
    ppu.write(6, (addr >> 8) as u8, bus);
    ppu.write(6, addr as u8, bus);
}

#[test]
fn vertical_blank_flag_and_nmi() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    ppu.write(0, 0x80, &mut bus);
    run_to(&mut ppu, &mut bus, 241, 1);
    assert!(!ppu.nmi());
    assert!(ppu.tick(&mut bus), "picture complete");
    assert!(ppu.nmi());
    assert_eq!(ppu.read(2, &mut bus) & 0x80, 0x80);
    assert!(!ppu.nmi(), "the read clears the flag");
    assert_eq!(ppu.read(2, &mut bus) & 0x80, 0);
    // Cleared at the pre-render line even without a read.
    run_to(&mut ppu, &mut bus, 241, 2);
    ppu.write(0, 0x00, &mut bus);
    run_to(&mut ppu, &mut bus, 261, 2);
    assert_eq!(ppu.status & 0x80, 0);
}

#[test]
fn status_read_just_before_vblank_suppresses_it() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    ppu.write(0, 0x80, &mut bus);
    run_to(&mut ppu, &mut bus, 241, 1);
    assert_eq!(ppu.read(2, &mut bus) & 0x80, 0);
    ppu.tick(&mut bus);
    assert!(!ppu.nmi());
    assert_eq!(ppu.read(2, &mut bus) & 0x80, 0);
}

#[test]
fn frame_lengths() {
    let dots = |model, mask| {
        let (mut ppu, mut bus) = ready(model);
        ppu.write(1, mask, &mut bus);
        let mut lens = Vec::new();
        for _ in 0..2 {
            let start = ppu.fields;
            let mut n = 0u32;
            while ppu.fields == start {
                ppu.tick(&mut bus);
                n += 1;
            }
            lens.push(n);
        }
        lens
    };
    let ntsc = dots(PpuModel::Ntsc, 0x08);
    assert!(
        ntsc.contains(&(262 * 341)) && ntsc.contains(&(262 * 341 - 1)),
        "{ntsc:?}"
    );
    assert_eq!(
        dots(PpuModel::Ntsc, 0),
        vec![262 * 341; 2],
        "no skip without rendering"
    );
    assert_eq!(dots(PpuModel::Pal, 0x08), vec![312 * 341; 2]);
}

#[test]
fn scroll_and_address_registers() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    ppu.write(0, 0x03, &mut bus);
    assert_eq!(ppu.t & 0x0c00, 0x0c00);
    ppu.read(2, &mut bus);
    ppu.write(5, 0x7d, &mut bus); // X = 125: coarse 15, fine 5
    assert_eq!((ppu.t & 0x1f, ppu.x), (15, 5));
    ppu.write(5, 0x5e, &mut bus); // Y = 94: coarse 11, fine 6
    assert_eq!((ppu.t >> 5) & 0x1f, 11);
    assert_eq!(ppu.t >> 12, 6);
    set_addr(&mut ppu, &mut bus, 0x2345);
    assert_eq!(ppu.v, 0x2345);
}

#[test]
fn ppudata_buffer_and_palette() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    set_addr(&mut ppu, &mut bus, 0x2400);
    for v in [0x11, 0x22, 0x33] {
        ppu.write(7, v, &mut bus);
    }
    assert_eq!(bus.nt[0x400], 0x11, "vertical mirroring: $2400 is page 1");
    set_addr(&mut ppu, &mut bus, 0x2400);
    assert_eq!(ppu.read(7, &mut bus), 0, "stale buffer");
    assert_eq!(ppu.read(7, &mut bus), 0x11);
    assert_eq!(ppu.read(7, &mut bus), 0x22);
    // Palette: no buffer, $3F10 mirrors $3F00.
    set_addr(&mut ppu, &mut bus, 0x3f10);
    ppu.write(7, 0x2a, &mut bus);
    set_addr(&mut ppu, &mut bus, 0x3f00);
    assert_eq!(ppu.read(7, &mut bus) & 0x3f, 0x2a);
    // Increment 32.
    ppu.write(0, 0x04, &mut bus);
    set_addr(&mut ppu, &mut bus, 0x2000);
    ppu.write(7, 1, &mut bus);
    assert_eq!(ppu.v, 0x2020);
}

/// A solid tile 1 (colour 3) at the top left; palette 0 = $0F $00 $10 $30,
/// sprite palette 0 red.
fn tile_scene(ppu: &mut Ppu, bus: &mut Vram) {
    for i in 0..8 {
        bus.chr[0x10 + i] = 0xff;
        bus.chr[0x18 + i] = 0xff;
    }
    bus.nt[0] = 1;
    set_addr(ppu, bus, 0x3f00);
    for c in [0x0f, 0x00, 0x10, 0x30] {
        ppu.write(7, c, bus);
    }
    set_addr(ppu, bus, 0x3f11);
    for c in [0x16, 0x16, 0x16] {
        ppu.write(7, c, bus);
    }
    set_addr(ppu, bus, 0x2000);
    ppu.write(5, 0, bus);
    ppu.write(5, 0, bus);
}

fn pixel(ppu: &Ppu, x: usize, y: usize) -> Rgb8 {
    ppu.frame()[y * WIDTH + BORDER + x]
}

#[test]
fn background_tiles_render() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    tile_scene(&mut ppu, &mut bus);
    ppu.write(1, 0x0a, &mut bus);
    // Rendering runs from the pre-render line on, which fetches tile 0.
    run_to(&mut ppu, &mut bus, 261, 0);
    run_to(&mut ppu, &mut bus, 241, 0);
    let white = palette::table()[0x30];
    let black = palette::table()[0x0f];
    assert_eq!(pixel(&ppu, 0, 0), white);
    assert_eq!(pixel(&ppu, 7, 7), white);
    assert_eq!(pixel(&ppu, 8, 0), black);
    assert_eq!(pixel(&ppu, 0, 8), black);
    assert_eq!(ppu.frame()[0], black, "border in the backdrop colour");
    // Fine X scroll by 3 moves the tile left.
    ppu.write(5, 3, &mut bus);
    ppu.write(5, 0, &mut bus);
    run_to(&mut ppu, &mut bus, 241, 0);
    run_to(&mut ppu, &mut bus, 241, 1);
    run_to(&mut ppu, &mut bus, 241, 0);
    assert_eq!(pixel(&ppu, 4, 0), white);
    assert_eq!(pixel(&ppu, 5, 0), black);
}

#[test]
fn sprite_zero_hit_and_overflow() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    tile_scene(&mut ppu, &mut bus);
    // Sprite 0 at (4, 3) over the tile (Y = line - 1).
    ppu.write(3, 0, &mut bus);
    for v in [2u8, 1, 0, 4] {
        ppu.write(4, v, &mut bus);
    }
    for _ in 1..64 {
        for v in [0xf0, 0, 0, 0] {
            ppu.write(4, v, &mut bus);
        }
    }
    ppu.write(1, 0x1e, &mut bus);
    run_to(&mut ppu, &mut bus, 261, 0);
    run_to(&mut ppu, &mut bus, 3, 0);
    assert_eq!(ppu.status & 0x40, 0);
    run_to(&mut ppu, &mut bus, 3, 6);
    assert_eq!(ppu.status & 0x40, 0x40, "hit at x = 4");
    assert_eq!(pixel(&ppu, 4, 3), palette::table()[0x16], "sprite in front");
    assert_eq!(ppu.status & 0x20, 0);
    // Nine sprites on line 100: overflow.
    run_to(&mut ppu, &mut bus, 241, 0);
    ppu.write(1, 0x00, &mut bus);
    ppu.write(3, 0, &mut bus);
    for _ in 0..9 {
        for v in [99, 1, 0, 100] {
            ppu.write(4, v, &mut bus);
        }
    }
    ppu.write(1, 0x1e, &mut bus);
    run_to(&mut ppu, &mut bus, 101, 0);
    assert_eq!(ppu.status & 0x20, 0x20);
    run_to(&mut ppu, &mut bus, 261, 2);
    assert_eq!(ppu.status & 0x60, 0, "cleared at the pre-render line");
}

#[test]
fn sprite_patterns_are_fetched_from_the_sprite_table() {
    let (mut ppu, mut bus) = ready(PpuModel::Ntsc);
    ppu.write(0, 0x08, &mut bus); // sprites at $1000
    ppu.write(1, 0x18, &mut bus);
    run_to(&mut ppu, &mut bus, 10, 257);
    bus.reads.clear();
    run_to(&mut ppu, &mut bus, 10, 321);
    let high: Vec<_> = bus.reads.iter().filter(|a| **a & 0x1000 != 0).collect();
    assert_eq!(high.len(), 16, "two pattern bytes for each of 8 sprites");
    assert!(
        high.iter().all(|a| **a & 0xff0 == 0xff0),
        "unused slots fetch tile $FF"
    );
}

#[test]
fn savestate_round_trip() {
    let (mut ppu, mut bus) = ready(PpuModel::Pal);
    tile_scene(&mut ppu, &mut bus);
    ppu.write(1, 0x1e, &mut bus);
    run_to(&mut ppu, &mut bus, 100, 123);
    let mut w = StateWriter::new();
    ppu.save(&mut w);
    let data = w.into_bytes();
    let mut copy = Ppu::new(PpuModel::Pal);
    copy.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(copy, ppu);
    let mut bus2 = Vram {
        chr: bus.chr.clone(),
        nt: bus.nt.clone(),
        reads: Vec::new(),
    };
    for _ in 0..100_000 {
        ppu.tick(&mut bus);
        copy.tick(&mut bus2);
    }
    assert_eq!(copy, ppu);
    let mut bad = data.clone();
    let n = bad.len();
    bad.truncate(n - 10);
    assert!(copy.load(&mut StateReader::new(&bad)).is_err());
}
