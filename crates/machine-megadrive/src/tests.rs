use super::*;
use mimic_core::frame::Rgb8;
use mimic_core::input::Dirs;
use mimic_core::ControlId;

fn machine(options: &[(&str, &str)]) -> MegaDrive {
    let mut config = MachineConfig::default();
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    MegaDrive::new(&config).unwrap()
}

fn frames(m: &mut MegaDrive, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        let stop = m.run_frame(&mut dbg);
        assert_eq!(stop, StopReason::FrameDone);
    }
}

fn word(m: &MegaDrive, addr: u32) -> u16 {
    u16::from(peek_cpu(m, addr)) << 8 | u16::from(peek_cpu(m, addr + 1))
}

fn send(m: &mut MegaDrive, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

fn pixels(m: &MegaDrive) -> Vec<Rgb8> {
    let PixelData::Rgb(px) = m.framebuffer().pixels else {
        panic!("RGB frames")
    };
    px.to_vec()
}

const YELLOW: Rgb8 = Rgb8 {
    r: 255,
    g: 255,
    b: 0,
};
const DARK_BLUE: Rgb8 = Rgb8 { r: 0, g: 0, b: 109 };
const WHITE: Rgb8 = Rgb8 {
    r: 255,
    g: 255,
    b: 255,
};

#[test]
fn the_demo_cartridge_runs_on_the_vertical_interrupt() {
    let mut m = machine(&[("cart", "demo")]);
    frames(&mut m, 30);
    let n = word(&m, demo::FRAMES);
    assert!((25..=30).contains(&n), "frames counted: {n}");
    let f = m.framebuffer();
    assert_eq!((f.width, f.height), (mdvdp::H40_WIDTH, 243));
    let px = pixels(&m);
    assert!(px.contains(&YELLOW), "the boxes");
    assert!(px.contains(&DARK_BLUE), "the background");
    assert!(px.contains(&WHITE), "the sprite");
    assert_eq!(peek_cpu(&m, demo::PADS) & 0x3f, 0x3f, "nothing pressed");
}

#[test]
fn the_z80_runs_its_program_and_keys_the_ym2612() {
    let mut m = machine(&[("cart", "demo")]);
    frames(&mut m, 10);
    let a = peek_z80(&m, demo::Z80_COUNTER);
    frames(&mut m, 1);
    let b = peek_z80(&m, demo::Z80_COUNTER);
    assert_ne!(a, b, "the Z80 counts");
    assert!(!m.board.z80_reset && !m.board.z80_busreq);
    let mut out = Vec::new();
    let format = m.audio(&mut out);
    assert_eq!(format.channels, 2);
    assert!(out.iter().any(|&s| s.abs() > 500), "the FM note sounds");
}

#[test]
fn pad_1_moves_the_sprite_and_a_sounds_the_psg() {
    let mut m = machine(&[("cart", "demo")]);
    frames(&mut m, 5);
    let x = word(&m, demo::SPRITE_X);
    send(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 0x10, // A
        },
    );
    frames(&mut m, 10);
    assert!(word(&m, demo::SPRITE_X) >= x + 8, "moved right");
    assert_eq!(
        peek_cpu(&m, demo::PADS) & 0x7f,
        0x77,
        "TH high: right pressed"
    );
    assert_eq!(
        peek_cpu(&m, demo::PADS_TH0) & 0x7f,
        0x23,
        "TH low: A pressed, left/right low"
    );
    assert_eq!(m.board.psg.atten[0], 0, "the A tone");
}

#[test]
fn the_z80_bus_and_reset_from_the_68000() {
    let mut m = machine(&[]);
    let b = &m.board;
    assert!(b.z80_reset && !b.z80_bus_granted(), "reset at power-on");
    let start = m.sched.now().0;
    let beam = m.beam_pos();
    let mut bus = MdBus {
        b: &mut m.board,
        ports: &mut m.ports,
        start,
        dbg: None,
        beam,
        fetch: false,
    };
    // The open bus fills the other bits: an even next opcode.
    bus.b.prefetch = 0x4e00;
    assert_eq!(
        bus.read_word(0xa1_1100) & 0x100,
        0x100,
        "the Z80 has the bus"
    );
    bus.write_word(0xa1_1200, 0x0100);
    bus.write_word(0xa1_1100, 0x0100);
    assert_eq!(bus.read_word(0xa1_1100) & 0x100, 0, "granted");
    bus.write_word(0xa0_0010, 0x1234);
    assert_eq!(
        bus.b.z80_ram[0x10], 0x12,
        "a word write stores the high byte"
    );
    assert_eq!(bus.read_word(0xa0_0010), 0x1212);
    // The bank register: nine writes of bit 0.
    for bit in [0, 0, 0, 0, 0, 0, 0, 1, 1] {
        bus.write_byte(0xa0_6000, bit);
    }
    assert_eq!(u32::from(bus.b.z80_bank) << 15, 0xc0_0000);
    bus.write_word(0xa1_1100, 0);
    assert_eq!(
        bus.read_word(0xa0_0010),
        0x4e00,
        "without the bus: the open bus"
    );
}

#[test]
fn the_6_button_pad_through_the_io_chip() {
    let mut m = machine(&[("port1", "pad6")]);
    send(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0x40, // X
        },
    );
    m.step_cycle();
    let start = m.sched.now().0;
    let beam = m.beam_pos();
    let mut bus = MdBus {
        b: &mut m.board,
        ports: &mut m.ports,
        start,
        dbg: None,
        beam,
        fetch: false,
    };
    bus.write_byte(0xa1_0009, 0x40);
    let mut seen = Vec::new();
    for th in [0x40, 0x00, 0x40, 0x00, 0x40] {
        bus.write_byte(0xa1_0003, th);
        seen.push(bus.read_byte(0xa1_0003) & 0x7f);
    }
    assert_eq!(seen[3] & 0x0f, 0x00, "2nd low phase");
    assert_eq!(seen[4] & 0x0f, 0x0b, "X on bit 2");
    assert_eq!(
        bus.read_byte(0xa1_0001),
        0xa0,
        "version: export NTSC, no TMSS"
    );
}

/// A cartridge image with `len` bytes, the header's backup RAM.
fn cart_image(len: usize, sram: bool) -> Vec<u8> {
    let mut rom = demo::rom();
    rom.resize(len, 0);
    for bank in 0..len / 0x8_0000 {
        rom[bank * 0x8_0000 + 0x10] = bank as u8;
    }
    if sram {
        rom[0x1b0..0x1bc]
            .copy_from_slice(&[0x52, 0x41, 0xf8, 0x20, 0, 0x20, 0, 1, 0, 0x20, 0x3f, 0xff]);
    }
    rom
}

fn insert(m: &mut MegaDrive, rom: Vec<u8>) {
    let media = Media {
        name: "test".into(),
        kind: MediaKind::Cartridge,
        backend: Box::new(MemoryImage(rom)),
    };
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    m.step_cycle();
}

#[test]
fn backup_ram_on_odd_bytes_and_the_ssf2_mapper() {
    let mut m = machine(&[]);
    insert(&mut m, cart_image(0x10000, true));
    assert_eq!(m.board.save, Save::Sram);
    assert_eq!(m.board.sram.len(), 0x2000);
    m.board.cart_write(0x20_0001, 0x5a);
    m.board.cart_write(0x20_0002, 0x77);
    assert_eq!(m.board.cart_byte(0x20_0001), 0x5a);
    assert_eq!(m.board.sram[0], 0x5a);
    assert_eq!(m.board.sram[1], 0xff, "even bytes are not RAM");
    // At the end of a frame the battery slot gets the contents.
    frames(&mut m, 1);
    assert_eq!(m.cart.battery.contents.as_deref().map(|c| c[0]), Some(0x5a));
    // A 5 MB image: the SSF2 mapper pages 512 KB banks.
    let mut big = machine(&[]);
    insert(&mut big, cart_image(0x50_0000, false));
    assert!(big.board.cart.as_ref().unwrap().ssf2);
    assert_eq!(big.board.cart_byte(0x38_0010), 7);
    let start = big.sched.now().0;
    let beam = big.beam_pos();
    let mut bus = MdBus {
        b: &mut big.board,
        ports: &mut big.ports,
        start,
        dbg: None,
        beam,
        fetch: false,
    };
    bus.write_byte(0xa1_30ff, 9);
    assert_eq!(bus.b.cart_byte(0x38_0010), 9);
}

#[test]
fn tmss_locks_the_vdp_until_sega_is_written() {
    let mut m = machine(&[("tmss", "on")]);
    let start = m.sched.now().0;
    let beam = m.beam_pos();
    let mut bus = MdBus {
        b: &mut m.board,
        ports: &mut m.ports,
        start,
        dbg: None,
        beam,
        fetch: false,
    };
    assert_eq!(bus.read_byte(0xa1_0001) & 0x0f, 1);
    bus.write_word(0xc0_0004, 0x8712);
    assert_eq!(bus.b.vdp.regs[7], 0, "locked");
    bus.write(0xa1_4000, Size::Long, 0x5345_4741);
    bus.write_word(0xc0_0004, 0x8712);
    assert_eq!(bus.b.vdp.regs[7], 0x12);
    // The demo writes "SEGA" itself and runs.
    let mut d = machine(&[("tmss", "on"), ("cart", "demo")]);
    frames(&mut d, 10);
    assert!(word(&d, demo::FRAMES) > 5);
}

#[test]
fn the_eeprom_option_maps_an_x24c01_at_200001() {
    let mut m = machine(&[("save", "eeprom")]);
    insert(&mut m, cart_image(0x10000, false));
    assert_eq!(m.board.save, Save::Eeprom);
    assert_eq!(m.board.cart_byte(0x20_0001), 1, "SDA released");
}

#[test]
fn pal_europe_and_the_reset_button() {
    let mut m = machine(&[("cart", "demo"), ("region", "europe")]);
    assert_eq!(m.standard, VideoStandard::Pal);
    assert_eq!(m.board.version() & 0xc0, 0xc0);
    frames(&mut m, 3);
    assert_eq!(m.framebuffer().height, 294);
    send(
        &mut m,
        InputTarget::Panel,
        InputEvent::Control {
            id: ControlId(CONTROL_RESET),
            value: 1,
        },
    );
    frames(&mut m, 5);
    assert!(
        word(&m, demo::FRAMES) >= 3,
        "the program restarted and runs"
    );
}

#[test]
fn savestates_round_trip_and_reject_garbage() {
    let mut a = machine(&[("cart", "demo")]);
    frames(&mut a, 5);
    let s = a.save_state();
    let mut b = machine(&[("cart", "demo")]);
    b.load_state(&s).unwrap();
    frames(&mut a, 5);
    frames(&mut b, 5);
    assert_eq!(pixels(&a), pixels(&b));
    assert_eq!(a.now(), b.now());
    assert_eq!(
        peek_z80(&a, demo::Z80_COUNTER),
        peek_z80(&b, demo::Z80_COUNTER)
    );
    assert!(b.load_state(&s[..s.len() / 2]).is_err());
}

#[test]
fn overlays_and_debug_targets() {
    let mut m = machine(&[("cart", "demo")]);
    m.set_overlays(&["sprites", "sprite_bounds"]);
    frames(&mut m, 3);
    assert_eq!(m.overlay("sprites").expect("boxes").shapes.len(), 1);
    assert_eq!(m.devices(), [CPU, Z80_DEV]);
    let t = m.debug_target(Z80_DEV).unwrap();
    assert!(t.regs().get("PC").is_some());
    let t = m.debug_target(CPU).unwrap();
    assert!(t.regs().get("D0").is_some());
    let i = m.debug_target(CPU).unwrap().disassemble(0x200);
    assert!(
        i.mnemonic.to_ascii_lowercase().contains("move"),
        "{}",
        i.mnemonic
    );
}

#[test]
fn a_battery_file_inserted_with_the_cartridge_fills_the_backup_ram() {
    let mut m = machine(&[]);
    let media = Media {
        name: "test".into(),
        kind: MediaKind::Cartridge,
        backend: Box::new(MemoryImage(cart_image(0x10000, true))),
    };
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    // As the frontends do: the .srm right after the cartridge.
    let mut file = vec![0u8; 0x2000];
    file[0] = 0x42;
    let srm = Media {
        name: "test.srm".into(),
        kind: MediaKind::BatteryRam,
        backend: Box::new(MemoryImage(file)),
    };
    m.media_slot(SLOT_BATTERY).unwrap().insert(srm).unwrap();
    frames(&mut m, 2);
    assert_eq!(m.board.cart_byte(0x20_0001), 0x42);
    assert!(m.media_slots()[1].inserted);
}

#[test]
fn graphics_views_show_the_demos_vram() {
    let mut m = machine(&[("cart", "demo")]);
    frames(&mut m, 3);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(
        ids,
        ["tiles", "plane_a", "plane_b", "window", "sprites", "palette"]
    );
    let opts = mimic_core::gfx::GfxOptions::new();
    let a = m.gfx_view("plane_a", &opts).unwrap();
    assert_eq!((a.width, a.height, a.space), (512, 256, VRAM_SPACE));
    let s = m.gfx_view("sprites", &opts).unwrap();
    assert_eq!(s.param("count"), Some(1));
    let p = m.gfx_view("palette", &opts).unwrap();
    assert_eq!(p.space, CRAM_SPACE);
    assert!(m.gfx_view("nothing", &opts).is_none());
}

#[test]
fn unmapped_reads_give_the_next_opcode_high_byte() {
    let mut m = machine(&[]);
    m.board.prefetch = 0x4e71;
    assert_eq!(peek_cpu(&m, 0x40_0000), 0x4e);
    assert_eq!(peek_cpu(&m, 0x40_0001), 0x00);
    assert_eq!(
        peek_cpu(&m, 0xa0_0000),
        0x4e,
        "the Z80's space without its bus"
    );
    assert_eq!(peek_cpu(&m, 0xa1_1100), 0x4f, "bit 0: the Z80 has the bus");
    assert_eq!(
        peek_cpu(&m, 0xc0_0018),
        0x4e,
        "unused VDP ports: the whole word"
    );
    assert_eq!(peek_cpu(&m, 0xc0_0019), 0x71);
    assert_eq!(
        peek_cpu(&m, 0xc0_0004) & 0xfc,
        0x4c,
        "the status's top six bits"
    );
}
