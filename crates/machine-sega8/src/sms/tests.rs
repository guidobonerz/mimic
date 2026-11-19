use super::*;
use mimic_core::input::{Dirs, MachineKey};
use mimic_core::ControlId;

fn machine(model: &str, options: &[(&str, &str)]) -> Sms {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Sms::new(SmsModel::from_name(model).unwrap(), &config).unwrap()
}

fn frames(m: &mut Sms, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

fn word(m: &Sms, addr: u16) -> u16 {
    u16::from(peek_cpu(m, addr)) | u16::from(peek_cpu(m, addr + 1)) << 8
}

fn send(m: &mut Sms, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

fn rgb_pixels(m: &Sms) -> Vec<mimic_core::frame::Rgb8> {
    let f = m.framebuffer();
    let PixelData::Rgb(px) = f.pixels else {
        panic!("RGB frames")
    };
    px.to_vec()
}

#[test]
fn the_mode4_demo_runs_on_the_frame_interrupt() {
    for model in ["mark3", "sms", "sms2"] {
        let mut m = machine(model, &[("cart", "demo")]);
        frames(&mut m, 30);
        let n = word(&m, demo::FRAMES);
        assert!((25..=30).contains(&n), "{model}: frames counted {n}");
        let px = rgb_pixels(&m);
        // Yellow box lines (CRAM 1 = $0F: red 3, green 3) on dark blue.
        let yellow = mimic_core::frame::Rgb8 {
            r: 255,
            g: 255,
            b: 0,
        };
        let blue = mimic_core::frame::Rgb8 { r: 0, g: 0, b: 170 };
        assert!(px.contains(&yellow), "{model}: the boxes");
        assert!(px.contains(&blue), "{model}: the background");
        // The white sprite.
        assert!(px.contains(&mimic_core::frame::Rgb8 {
            r: 255,
            g: 255,
            b: 255
        }));
        assert_eq!(peek_cpu(&m, demo::PADS), 0xff);
    }
}

#[test]
fn pad_1_moves_the_sprite_and_pad_2_reads_on_port_b() {
    let mut m = machine("sms", &[("cart", "demo")]);
    frames(&mut m, 5);
    let x = peek_cpu(&m, demo::SPRITE_X);
    send(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::RIGHT,
            buttons: 1,
        },
    );
    frames(&mut m, 10);
    assert!(peek_cpu(&m, demo::SPRITE_X) >= x + 8, "moved right");
    assert_eq!(peek_cpu(&m, demo::PADS), !0x18, "right and TL, active low");
    let mut out = Vec::new();
    m.audio(&mut out);
    assert!(out.iter().any(|&s| s.abs() > 1000), "the tone sounds");
    send(
        &mut m,
        InputTarget::Port(PORT2),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 2,
        },
    );
    frames(&mut m, 2);
    // Port B: TR of pad 2 is bit 3, RESET (bit 4) up, bit 5 high, TH high.
    assert_eq!(peek_cpu(&m, demo::PADS_B), 0xf7);
    // The RESET button.
    send(
        &mut m,
        InputTarget::Panel,
        InputEvent::Control {
            id: ControlId(CONTROL_RESET),
            value: 1,
        },
    );
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, demo::PADS_B), 0xe7);
}

#[test]
fn pause_is_the_nmi() {
    let mut m = machine("sms2", &[("cart", "demo")]);
    frames(&mut m, 3);
    for down in [true, false] {
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(crate::keys::PAUSE),
                down,
            },
        );
        frames(&mut m, 1);
    }
    assert_eq!(peek_cpu(&m, demo::PAUSES), 1);
    assert_eq!(m.controls()[1].name, "PAUSE");
}

/// A 64 KB image whose 16 KB banks start with their number.
fn banked_rom() -> Vec<u8> {
    let mut rom = demo::sms_rom(false);
    rom.resize(0x10000, 0xff);
    for bank in 1..4 {
        rom[bank * 0x4000] = bank as u8;
    }
    rom
}

fn insert(m: &mut Sms, rom: Vec<u8>) {
    let media = Media {
        name: "banks".into(),
        kind: MediaKind::Cartridge,
        backend: Box::new(MemoryImage(rom)),
    };
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    m.step_cycle();
}

#[test]
fn the_sega_mapper_pages_banks_and_cartridge_ram() {
    let mut m = machine("sms", &[]);
    insert(&mut m, banked_rom());
    let b = &mut m.board;
    assert_eq!(b.mem_peek(0x4000), 1);
    assert_eq!(b.mem_peek(0x8000), 2);
    b.mem_write(0xffff, 3);
    assert_eq!(b.mem_peek(0x8000), 3);
    assert_eq!(b.ram[0x1fff], 3, "the register is written into RAM too");
    b.mem_write(0xfffd, 2);
    assert_eq!(
        b.mem_peek(0x0000),
        b.cart.as_ref().unwrap().rom()[0],
        "the first 1 KB is fixed"
    );
    assert_eq!(b.mem_peek(0x0400), b.cart.as_ref().unwrap().rom()[0x8400]);
    // Cartridge RAM at $8000.
    b.mem_write(0xfffc, 0x08);
    b.mem_write(0x8000, 0x5a);
    assert_eq!(b.mem_peek(0x8000), 0x5a);
    b.mem_write(0xfffc, 0x00);
    assert_eq!(b.mem_peek(0x8000), 3);
    // Work RAM repeats at $E000.
    b.mem_write(0xc123, 0x77);
    assert_eq!(b.mem_peek(0xe123), 0x77);
}

#[test]
fn the_codemasters_mapper_and_the_copier_header() {
    let mut m = machine("sms", &[("mapper", "codemasters")]);
    let mut rom = vec![0u8; 512];
    rom.extend(banked_rom());
    insert(&mut m, rom);
    let b = &mut m.board;
    assert_eq!(
        b.cart.as_ref().unwrap().rom().len(),
        0x10000,
        "the header dropped"
    );
    assert_eq!(b.mem_peek(0x4000), 1);
    assert_eq!(b.mem_peek(0x8000), 0xf3, "bank 0 at $8000");
    b.mem_write(0x8000, 3);
    assert_eq!(b.mem_peek(0x8000), 3);
}

#[test]
fn the_bios_runs_first_and_hands_over() {
    // A BIOS that enables the cartridge (port $3E = $AB) and jumps to 0:
    // copied to RAM first, as the real one does.
    let stub: [u8; 9] = [0x3e, 0xab, 0xd3, 0x3e, 0xc3, 0x00, 0x00, 0x00, 0x00];
    let mut bios = vec![0u8; 0x2000];
    // $0000: copy the stub to $C000 and jump there.
    let copy = [
        0x21, 0x10, 0x00, // LD HL,$0010
        0x11, 0x00, 0xc0, // LD DE,$C000
        0x01, 0x09, 0x00, // LD BC,9
        0xed, 0xb0, // LDIR
        0xc3, 0x00, 0xc0, // JP $C000
    ];
    bios[..copy.len()].copy_from_slice(&copy);
    bios[0x10..0x19].copy_from_slice(&stub);
    let mut config = MachineConfig {
        model: Some("sms".into()),
        ..MachineConfig::default()
    };
    config.roms.insert("sms-bios".into(), bios);
    config.options.insert("cart".into(), "demo".into());
    let mut m = Sms::new(SmsModel::Sms, &config).unwrap();
    assert_eq!(m.board.memctl, 0xe3, "BIOS on, cartridge off");
    assert_eq!(peek_cpu(&m, 0), 0x21);
    frames(&mut m, 30);
    assert_eq!(m.board.memctl, 0xab);
    assert!(word(&m, demo::FRAMES) > 20, "the cartridge runs");
}

#[test]
fn the_v_counter_and_region_read_through_the_ports() {
    let mut m = machine("sms", &[]);
    let b = &mut m.board;
    // TH A as an output, low: export machines read it back low.
    b.io_write(0x3f, 0xf5 & !0x20);
    assert_eq!(b.io_peek(0xdd) & 0x40, 0);
    let mut j = machine("mark3", &[]);
    j.board.io_write(0x3f, 0xf5);
    assert_eq!(j.board.io_peek(0xdd) & 0x40, 0, "Japan: output TH reads 0");
    m.board.io_write(0x3f, 0xf5);
    assert_eq!(m.board.io_peek(0xdd) & 0x40, 0x40, "export: the level");
    while m.board.vdp.line != 100 {
        m.board.tick();
    }
    assert_eq!(m.board.io_peek(0x7e), 100);
}

#[test]
fn the_game_gear_has_start_stereo_and_its_window() {
    let mut m = machine("gg", &[("cart", "demo")]);
    frames(&mut m, 10);
    let f = m.framebuffer();
    assert_eq!((f.width, f.height), (160, 144));
    let px = rgb_pixels(&m);
    let yellow = mimic_core::frame::Rgb8 {
        r: 255,
        g: 255,
        b: 0,
    };
    assert!(px.contains(&yellow), "12-bit CRAM through the GG writes");
    assert_eq!(m.board.io_peek(0x00) & 0x80, 0x80, "START up");
    send(
        &mut m,
        InputTarget::Panel,
        InputEvent::Control {
            id: ControlId(CONTROL_PAUSE),
            value: 1,
        },
    );
    frames(&mut m, 1);
    assert_eq!(m.board.io_peek(0x00) & 0x80, 0);
    assert_eq!(m.controls()[1].name, "START");
    let mut out = Vec::new();
    let format = m.audio(&mut out);
    assert_eq!(format.channels, 2);
    assert_eq!(out.len() % 2, 0);
}

#[test]
fn savestates_round_trip_and_reject_garbage() {
    let mut a = machine("sms", &[("cart", "demo")]);
    frames(&mut a, 5);
    let s = a.save_state();
    let mut b = machine("sms", &[("cart", "demo")]);
    b.load_state(&s).unwrap();
    frames(&mut a, 5);
    frames(&mut b, 5);
    assert_eq!(rgb_pixels(&a), rgb_pixels(&b));
    assert_eq!(a.now(), b.now());
    assert!(b.load_state(&s[..s.len() / 2]).is_err());
    let mut gg = machine("gg", &[("cart", "demo")]);
    assert!(gg.load_state(&s).is_err(), "another model");
}

#[test]
fn overlays_box_the_sprite() {
    let mut m = machine("sms", &[("cart", "demo")]);
    m.set_overlays(&["sprites", "sprite_bounds"]);
    frames(&mut m, 3);
    let o = m.overlay("sprites").expect("boxes");
    assert_eq!(o.shapes.len(), 1, "one sprite");
    assert!(m.overlay("sprite_bounds").is_some());
}

#[test]
fn the_japanese_sms_detects_and_plays_its_ym2413() {
    let mut m = machine("sms", &[("region", "japan")]);
    let b = &mut m.board;
    b.io_write(0x3e, b.memctl | 0x04);
    assert_eq!(b.io_peek(0xf2), 0);
    b.io_write(0xf2, 0x01);
    assert_eq!(b.io_peek(0xf2), 1, "the detection reads the bit back");
    // Flute at A440 on channel 0.
    for (r, v) in [(0x10, 0x20), (0x30, 0x40), (0x20, 0x19)] {
        b.io_write(0xf0, r);
        b.io_write(0xf1, v);
    }
    for _ in 0..200_000 {
        b.tick();
    }
    assert!(b.samples.iter().any(|&s| s.abs() > 1000), "FM sounds");
    assert!(m.chips().iter().any(|c| c.name == "ym2413"));
    // An export SMS has none: $F2 is the (switched off) I/O chip.
    let mut e = machine("sms", &[]);
    e.board.io_write(0x3e, 0xaf);
    e.board.io_write(0xf2, 0x01);
    assert_eq!(e.board.io_peek(0xf2), 0xff);
    assert!(Sms::new(
        SmsModel::GameGear,
        &MachineConfig {
            options: [("fm".to_string(), "on".to_string())].into(),
            ..MachineConfig::default()
        }
    )
    .is_err());
}
