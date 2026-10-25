use super::*;
use emu_core::input::Dirs;
use emu_core::media::{Media, MediaBackend, MediaError};

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// An NROM image: 16 KB PRG at $C000 with `code` at $C000, `nmi` at
/// $C100, the vectors, and 8 KB CHR RAM.
fn nrom(code: &[u8], nmi: &[u8]) -> Vec<u8> {
    let mut prg = vec![0xea; 0x4000];
    prg[..code.len()].copy_from_slice(code);
    prg[0x100..0x100 + nmi.len()].copy_from_slice(nmi);
    prg[0x3ffa..].copy_from_slice(&[0x00, 0xc1, 0x00, 0xc0, 0x00, 0xc1]);
    let mut img = vec![b'N', b'E', b'S', 0x1a, 1, 0, 0, 0];
    img.resize(16, 0);
    img.extend(prg);
    img
}

fn machine(options: &[(&str, &str)], model: &str) -> Nes {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Nes::new(&config).unwrap()
}

fn with_cart(m: &mut Nes, image: Vec<u8>) {
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "test.nes".into(),
            backend: Box::new(Bytes(image)),
        })
        .unwrap();
}

fn frames(m: &mut Nes, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// Wait for two vertical blanks (the PPU ignores $2000 before the
/// first one ends), then enable the NMI; the handler counts in $10.
const NMI_PROGRAM: [u8; 15] = [
    0x2c, 0x02, 0x20, // BIT $2002
    0x10, 0xfb, // BPL -5
    0x2c, 0x02, 0x20, // BIT $2002
    0x10, 0xfb, // BPL -5
    0xa9, 0x80, // LDA #$80
    0x8d, 0x00, 0x20, // STA $2000
];

fn nmi_rom() -> Vec<u8> {
    let mut code = NMI_PROGRAM.to_vec();
    code.extend([0x4c, 0x0f, 0xc0]); // JMP $C00F
    nrom(&code, &[0xe6, 0x10, 0x40]) // INC $10; RTI
}

#[test]
fn nmi_comes_every_frame() {
    for model in ["ntsc", "pal"] {
        let mut m = machine(&[], model);
        with_cart(&mut m, nmi_rom());
        frames(&mut m, 12);
        let n = peek_cpu(&m, 0x10);
        assert!((9..=11).contains(&n), "{model}: {n} NMIs in 12 frames");
    }
}

#[test]
fn frame_rate_follows_the_model() {
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 2);
    let start = cpu_cycles(&m);
    frames(&mut m, 60);
    let per = (cpu_cycles(&m) - start) as f64 / 60.0;
    assert!((per - 29780.5).abs() < 1.0, "NTSC {per} cycles a frame");
    let mut m = machine(&[], "pal");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 2);
    let start = cpu_cycles(&m);
    frames(&mut m, 50);
    let per = (cpu_cycles(&m) - start) as f64 / 50.0;
    assert!((per - 33247.5).abs() < 1.0, "PAL {per} cycles a frame");
}

/// Run cycles until RAM $12 is 1; returns the cycles.
fn cycles_until_marker(m: &mut Nes) -> u64 {
    for n in 0..10_000 {
        if peek_cpu(m, 0x12) == 1 {
            return n;
        }
        m.cycle(None);
    }
    panic!("marker never set");
}

#[test]
fn oam_dma_stops_the_cpu_for_513_or_514_cycles() {
    let program = |reg: u8| {
        nrom(
            &[
                0xa9, 0x02, // LDA #2
                0x8d, reg, 0x40, // STA $4014 (or $4018)
                0xa9, 0x01, // LDA #1
                0x85, 0x12, // STA $12
                0x4c, 0x09, 0xc0, // JMP *
            ],
            &[0x40],
        )
    };
    let mut plain = machine(&[], "ntsc");
    with_cart(&mut plain, program(0x18));
    plain.cycle(None); // powers up with the cartridge
    let base = cycles_until_marker(&mut plain);
    let mut dma = machine(&[], "ntsc");
    with_cart(&mut dma, program(0x14));
    dma.cycle(None);
    for i in 0..256 {
        dma.board.ram[0x200 + i] = i as u8 ^ 0x5a;
    }
    let with = cycles_until_marker(&mut dma);
    assert!(
        (513..=514).contains(&(with - base)),
        "DMA took {}",
        with - base
    );
    assert_eq!(dma.board.ppu.oam[3], 0x5a ^ 3);
    assert_eq!(
        dma.board.ppu.oam[2],
        (0x5a ^ 2) & 0xe3,
        "attribute bits 2-4 not stored"
    );
    assert_eq!(dma.board.ppu.oam[255], 0x5a ^ 255);
}

/// Strobe, read eight bits of $4016 to $20–$27 and of $4017 to $28–$2F
/// (24 of each with `n` = 24 into $20/$40), then set $12.
fn controller_rom(n: u8) -> Vec<u8> {
    nrom(
        &[
            0xa9, 0x01, // LDA #1
            0x8d, 0x16, 0x40, // STA $4016
            0xa9, 0x00, // LDA #0
            0x8d, 0x16, 0x40, // STA $4016
            0xa2, 0x00, // LDX #0
            0xad, 0x16, 0x40, // LDA $4016
            0x95, 0x20, // STA $20,X
            0xad, 0x17, 0x40, // LDA $4017
            0x95, 0x40, // STA $40,X
            0xe8, // INX
            0xe0, n, // CPX #n
            0xd0, 0xf1, // BNE -15
            0xa9, 0x01, // LDA #1
            0x85, 0x12, // STA $12
            0x4c, 0x1f, 0xc0, // JMP *
        ],
        &[0x40],
    )
}

#[test]
fn controllers_shift_out_their_buttons() {
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, controller_rom(9));
    m.apply_input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 1,
        },
    });
    m.apply_input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(keys::PAD2 + 3),
            down: true,
        },
    });
    cycles_until_marker(&mut m);
    let pad1: Vec<u8> = (0..9).map(|i| peek_cpu(&m, 0x20 + i)).collect();
    // Bits 5-7 are open bus: the $40 of the address's high byte.
    assert_eq!(pad1, [0x41, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x41, 0x41]);
    let pad2: Vec<u8> = (0..8).map(|i| peek_cpu(&m, 0x40 + i) & 1).collect();
    assert_eq!(pad2, [0, 0, 0, 1, 0, 0, 0, 0], "Start from the keyboard");
}

#[test]
fn four_score_adds_two_controllers_and_a_signature() {
    let mut m = machine(&[("fourscore", "on")], "ntsc");
    assert_eq!(m.ports().len(), 4);
    with_cart(&mut m, controller_rom(24));
    m.apply_input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Port(PortId(2)),
        event: InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 2,
        },
    });
    cycles_until_marker(&mut m);
    let bits = |base: u16| -> u32 {
        (0..24).fold(0, |v, i| v | u32::from(peek_cpu(&m, base + i) & 1) << i)
    };
    assert_eq!(bits(0x20), 1 << 9 | 1 << 19, "pad 3's B, signature bit 20");
    assert_eq!(bits(0x40), 1 << 18, "signature bit 19");
    let mut config = MachineConfig::default();
    config.options.insert("fourscore".into(), "on".into());
    config.options.insert("port2".into(), "zapper".into());
    assert!(Nes::new(&config).is_err(), "Four Score with a Zapper");
}

#[test]
fn zapper_sees_a_bright_picture_below_its_aim() {
    let mut m = machine(&[("port2", "zapper")], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 2);
    m.board.ppu.palette[0] = 0x30; // white backdrop, rendering off
    frames(&mut m, 1);
    let aim = [ppu::BORDER as u16 + 100, 100];
    while m.board.ppu.line != 110 {
        m.cycle(None);
    }
    assert!(m.board.light(aim));
    assert!(!m.board.light([NO_AIM, NO_AIM]));
    while m.board.ppu.line != 50 {
        m.cycle(None);
    }
    assert!(!m.board.light(aim), "the beam is above the aim");
    m.board.ppu.palette[0] = 0x0f;
    frames(&mut m, 1);
    while m.board.ppu.line != 110 {
        m.cycle(None);
    }
    assert!(!m.board.light(aim), "a black picture");
}

#[test]
fn savestate_round_trip_and_determinism() {
    let mut m = machine(&[("port2", "arkanoid")], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 5);
    for _ in 0..1234 {
        m.cycle(None);
    }
    let state = m.save_state();
    let mut copy = machine(&[("port2", "arkanoid")], "ntsc");
    with_cart(&mut copy, nmi_rom());
    frames(&mut copy, 1);
    copy.load_state(&state).unwrap();
    frames(&mut m, 3);
    frames(&mut copy, 3);
    assert_eq!(m.save_state(), copy.save_state());
    assert_eq!(
        m.framebuffer().pixels,
        copy.framebuffer().pixels,
        "same picture"
    );
    let mut other = machine(&[], "pal");
    with_cart(&mut other, nmi_rom());
    assert!(other.load_state(&state).is_err(), "model differs");
    let mut bad = state.clone();
    bad.truncate(state.len() / 2);
    assert!(copy.load_state(&bad).is_err());
}

#[test]
fn without_a_cartridge_the_cpu_does_not_run() {
    let mut m = machine(&[], "ntsc");
    frames(&mut m, 2);
    assert_eq!(m.cpu.pc(), 0);
    assert!(Nes::new(&MachineConfig {
        model: Some("secam".into()),
        ..MachineConfig::default()
    })
    .is_err());
    let mut config = MachineConfig::default();
    config.options.insert("port1".into(), "lightgun".into());
    assert!(Nes::new(&config).is_err());
}

#[test]
fn debugger_views() {
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 3);
    assert!(m.poke(PPU_SPACE, 0x2005, Size::Byte, 0x77));
    assert_eq!(m.peek(PPU_SPACE, 0x2005, Size::Byte), Some(0x77));
    assert!(m.poke(PPU_SPACE, 0x3f01, Size::Byte, 0x16));
    assert_eq!(m.peek(PPU_SPACE, 0x3f01, Size::Byte), Some(0x16));
    assert!(m.poke(OAM_SPACE, 4, Size::Byte, 9));
    assert_eq!(m.peek(OAM_SPACE, 4, Size::Byte), Some(9));
    assert_eq!(m.peek(CPU_SPACE, 0xfffc, Size::Word), Some(0xc000));
    assert!(m.chips().iter().any(|c| c.name == "mapper"));
    assert_eq!(m.disassemble(0xc000).mnemonic, "BIT");
}

#[test]
fn gfx_views_read_the_ppu_space_through_the_cartridge() {
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 3);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["tiles", "screen", "sprites", "palette"]);
    // Tile 1 of table 0, row 0, colour 1; name 1 in the top-left cell.
    assert!(m.poke(PPU_SPACE, 0x0010, Size::Byte, 0x80));
    assert!(m.poke(PPU_SPACE, 0x2000, Size::Byte, 1));
    assert!(m.poke(PPU_SPACE, 0x3f01, Size::Byte, 0x16));
    m.board.ppu.ctrl &= !0x10;
    let state = m.save_state();
    let t = m
        .gfx_view("tiles", &GfxOptions::new().with("palette", 0))
        .unwrap();
    assert_eq!(t.space, PPU_SPACE);
    assert_eq!(t.raw[0x10], 0x80);
    let red = m
        .board
        .ppu
        .gfx_view(
            "palette",
            &ppu::gfx::GfxContext {
                peek: &|_| 0,
                space: PPU_SPACE,
                oam_space: OAM_SPACE,
            },
            &GfxOptions::new(),
        )
        .unwrap()
        .frame()
        .rgb_at(17, 0);
    assert_eq!(t.frame().rgb_at(8, 0), red);
    let s = m.gfx_view("screen", &GfxOptions::new()).unwrap();
    assert_eq!(s.frame().rgb_at(0, 0), red);
    assert_eq!(s.raw[0x400], 1, "horizontal mirroring: $2400 = $2000");
    assert_eq!(s.frame().rgb_at(256, 0), red);
    let o = m.gfx_view("sprites", &GfxOptions::new()).unwrap();
    assert_eq!((o.space, o.raw.len()), (OAM_SPACE, 256));
    assert_eq!(m.save_state(), state, "views leave the machine as it was");
}

#[test]
fn overlays_show_sprites_the_hit_and_interrupts() {
    use emu_core::overlay::{colors, Shape};
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 3);
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["sprites", "sprite0", "irq"]);
    // Tile 0 solid colour 1 (background everywhere), sprite 0 at
    // Y = 49, X = 100 (lines 50–57), the others off screen.
    for a in 0..8 {
        assert!(m.poke(PPU_SPACE, a, Size::Byte, 0xff));
    }
    for a in 0..256 {
        let v = if a < 4 {
            [49, 0, 0, 100][a as usize]
        } else {
            0xff
        };
        assert!(m.poke(OAM_SPACE, a, Size::Byte, v));
    }
    m.board.ppu.mask = 0x1e;
    let state = m.save_state();
    frames(&mut m, 2);
    assert!(m.overlay("sprites").is_none(), "switched off");
    m.set_overlays(&["sprites", "sprite0", "irq"]);
    assert_eq!(
        m.overlay("sprites").unwrap().shapes,
        [],
        "nothing recorded yet"
    );
    frames(&mut m, 2);
    let rect = Shape::Rect {
        x: ppu::BORDER as i32 + 100,
        y: 50,
        w: 8,
        h: 8,
        color: colors::SERIES[0],
        label: Some("0".into()),
    };
    assert_eq!(m.overlay("sprites").unwrap().shapes, [rect]);
    let hit = m.overlay("sprite0").unwrap().shapes;
    assert!(
        matches!(&hit[..], [Shape::Mark { x, y: 50, .. }] if *x == ppu::BORDER as i32 + 100),
        "{hit:?}"
    );
    let irq = m.overlay("irq").unwrap().shapes;
    assert!(
        matches!(&irq[..], [Shape::Mark { y: 239, label: Some(l), .. }] if l.starts_with("NMI 241:")),
        "{irq:?}"
    );
    // Halted mid-frame: the frame so far.
    while m.board.ppu.line != 100 {
        m.step_cycle();
    }
    assert_eq!(m.overlay("sprites").unwrap().shapes.len(), 1);
    assert!(m.overlay("irq").unwrap().shapes.is_empty(), "no NMI yet");
    assert_eq!(m.frame_row(239), Some(239));
    assert_eq!(m.frame_row(240), None);
    // Recording changes nothing in the emulation or the savestate.
    let mut run = |ids: &[&str]| {
        m.load_state(&state).unwrap();
        m.set_overlays(ids);
        frames(&mut m, 4);
        (m.save_state(), ppu(&m).frame().to_vec())
    };
    let on = run(&["sprites", "sprite0", "irq"]);
    assert_eq!(on, run(&[]));
}

#[test]
fn beam_hpos_is_the_column_of_the_next_pixel() {
    let mut m = machine(&[], "ntsc");
    with_cart(&mut m, nmi_rom());
    frames(&mut m, 3);
    // Sprite 0 (tile 0 solid) at X = 100 on lines 50–57.
    for a in 0..8 {
        assert!(m.poke(PPU_SPACE, a, Size::Byte, 0xff));
    }
    for a in 0..256 {
        let v = if a < 4 {
            [49, 0, 0, 100][a as usize]
        } else {
            0xff
        };
        assert!(m.poke(OAM_SPACE, a, Size::Byte, v));
    }
    m.board.ppu.mask = 0x1e;
    frames(&mut m, 1);
    // A new sprite colour: watch the pixel at (BORDER + 100, 50) change.
    m.board.ppu.palette[0x11] = 0x2a;
    let col = ppu::BORDER as u32 + 100;
    let old = m.framebuffer().rgb_at(col, 50);
    loop {
        let before = m.beam().unwrap();
        m.step_cycle();
        let after = m.beam().unwrap();
        if m.framebuffer().rgb_at(col, 50) != old {
            assert_eq!((before.line, after.line), (50, 50));
            assert!(
                u32::from(before.hpos) <= col && col < u32::from(after.hpos),
                "{before:?} {after:?}"
            );
            break;
        }
    }
    m.board.ppu.dot = 0;
    assert_eq!(m.beam().unwrap().hpos, ppu::BORDER as u16 - 1);
    m.board.ppu.dot = 300;
    assert_eq!(usize::from(m.beam().unwrap().hpos), ppu::WIDTH - 1);
}
