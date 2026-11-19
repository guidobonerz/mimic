use super::*;
use mimic_core::input::Dirs;

fn machine(model: &str, options: &[(&str, &str)]) -> Sega8 {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Sega8::new(&config).unwrap()
}

fn frames(m: &mut Sega8, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

fn word(m: &Sega8, addr: u16) -> u16 {
    u16::from(peek_cpu(m, addr)) | u16::from(peek_cpu(m, addr + 1)) << 8
}

fn send(m: &mut Sega8, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

#[test]
fn the_demo_cartridge_runs_on_the_frame_interrupt() {
    let mut m = machine("sg1000", &[("cart", "demo")]);
    frames(&mut m, 30);
    let n = word(&m, demo::FRAMES);
    assert!(
        (25..=30).contains(&n),
        "fields counted by the /INT handler: {n}"
    );
    // The picture: the box pattern in yellow on dark blue.
    let f = m.framebuffer();
    let PixelData::Indexed { pixels, .. } = f.pixels else {
        panic!()
    };
    assert!(pixels.contains(&10), "yellow box lines");
    assert!(pixels.contains(&4), "dark blue background");
    // The pads, read through port $DC: nothing pressed.
    assert_eq!(peek_cpu(&m, demo::PADS), 0xff);
}

#[test]
fn pad_1_moves_the_sprite_and_tl_sounds() {
    let mut m = machine("sg1000", &[("cart", "demo")]);
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
    // Pad 2's right button (TR) is port B bit 3.
    send(
        &mut m,
        InputTarget::Port(PORT2),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 2,
        },
    );
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, demo::PADS_B) & 0x0f, 0x07);
}

#[test]
fn pause_is_the_nmi() {
    let mut m = machine("sg1000ii", &[("cart", "demo")]);
    frames(&mut m, 3);
    assert_eq!(peek_cpu(&m, demo::PAUSES), 0);
    for v in [1, 0] {
        send(
            &mut m,
            InputTarget::Panel,
            InputEvent::Control {
                id: mimic_core::ControlId(CONTROL_NMI),
                value: v,
            },
        );
        frames(&mut m, 1);
    }
    assert_eq!(peek_cpu(&m, demo::PAUSES), 1, "one edge, one NMI");
    assert_eq!(m.controls()[1].name, "PAUSE");
}

#[test]
fn memory_map_mirrors_and_open_bus() {
    let mut m = machine("sg1000", &[("cart", "demo")]);
    frames(&mut m, 1);
    // The 8 KB demo repeats up to $7FFF; $8000 has nothing: the address's
    // high byte. Work RAM: 1 KB repeated.
    assert_eq!(peek_cpu(&m, 0x2000), peek_cpu(&m, 0x0000));
    assert_eq!(peek_cpu(&m, 0x9abc), 0x9a);
    m.board.mem_write(0xc010, 0x55);
    assert_eq!(peek_cpu(&m, 0xc410), 0x55, "1 KB mirror");
    let mut sc = machine("sc3000", &[("cart", "demo")]);
    frames(&mut sc, 1);
    sc.board.mem_write(0xc010, 0x66);
    assert_eq!(peek_cpu(&sc, 0xc810), 0x66, "2 KB mirror");
    assert_eq!(peek_cpu(&sc, 0xc410), 0, "not 1 KB");
    // Cartridge RAM: 16 KB at $8000 on the SC-3000 (auto), none with
    // cartram=none, 32 KB switches the work RAM off.
    sc.board.mem_write(0x8123, 0x77);
    assert_eq!(peek_cpu(&sc, 0x8123), 0x77);
    let mut none = machine("sc3000", &[("cart", "demo"), ("cartram", "none")]);
    frames(&mut none, 1);
    none.board.mem_write(0x8123, 0x77);
    assert_eq!(peek_cpu(&none, 0x8123), 0x81);
    let mut big = machine("sc3000", &[("cart", "demo"), ("cartram", "32k")]);
    frames(&mut big, 1);
    big.board.mem_write(0xe000, 0x12);
    assert_eq!(peek_cpu(&big, 0xe000), 0x12);
    assert_eq!(big.board.cart_ram[0x6000], 0x12, "in the cartridge's RAM");
    assert!(Sega8::new(&MachineConfig {
        model: Some("sms".into()),
        ..MachineConfig::default()
    })
    .is_err());
}

#[test]
fn the_sc3000_keyboard_rows_and_reset_key() {
    let mut m = machine("sc3000h", &[("cart", "demo")]);
    frames(&mut m, 3);
    assert_eq!(peek_cpu(&m, demo::ROW0), 0xff);
    // Row 0: '1' (column 0) and '8' (column 8, port B bit 0).
    for (k, down) in [(keys::at(0, 0), true), (keys::at(0, 8), true)] {
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(k),
                down,
            },
        );
    }
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, demo::ROW0), 0xfe);
    assert_eq!(peek_cpu(&m, demo::ROW0_B) & 0x0f, 0x0e);
    // Port B's upper bits: no cassette signal, BUSY/FAULT/CONT high.
    assert_eq!(peek_cpu(&m, demo::ROW0_B) & 0xf0, 0x70);
    // The pads still read in row 7.
    assert_eq!(peek_cpu(&m, demo::PADS), 0xff);
    // The RESET key: an NMI.
    for down in [true, false] {
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(keys::RESET),
                down,
            },
        );
        frames(&mut m, 1);
    }
    assert_eq!(peek_cpu(&m, demo::PAUSES), 1);
    assert_eq!(m.controls()[1].name, "RESET");
    // The SG-1000 has no keyboard: row 0 reads the pads.
    let mut sg = machine("sg1000", &[("cart", "demo")]);
    frames(&mut sg, 3);
    send(
        &mut sg,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::at(0, 0)),
            down: true,
        },
    );
    frames(&mut sg, 2);
    assert_eq!(peek_cpu(&sg, demo::ROW0), 0xff);
}

#[test]
fn the_sc3000_types_text_and_the_consoles_cannot() {
    assert_eq!(keys::for_char('a'), Some(vec![keys::at(0, 2)]));
    assert_eq!(keys::for_char('8'), Some(vec![keys::at(0, 8)]));
    assert_eq!(keys::for_char('\n'), Some(vec![keys::CR]));
    assert_eq!(keys::for_char('"'), Some(vec![keys::SHIFT, keys::at(1, 0)]));
    assert_eq!(keys::for_char('é'), None);
    let mut m = machine("sc3000", &[("cart", "demo")]);
    frames(&mut m, 2);
    let (events, end) = m.typed_keys("1\n", m.now()).unwrap();
    assert_eq!(events.len(), 4, "1 and CR down and up");
    // Typed: row 0's port A shows '1' while it is down.
    for e in events {
        m.input(e);
    }
    let mut seen = false;
    while m.now() < end {
        frames(&mut m, 1);
        seen |= peek_cpu(&m, demo::ROW0) == 0xfe;
    }
    assert!(seen, "the demo read '1' in row 0");
    assert!(machine("sg1000", &[]).typed_keys("1", Cycle(0)).is_none());
}

/// A writable in-memory image (a blank tape).
struct Bytes(Vec<u8>);

impl mimic_core::media::MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), mimic_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), mimic_core::media::MediaError> {
        let end = offset as usize + data.len();
        if self.0.len() < end {
            self.0.resize(end, 0);
        }
        self.0[offset as usize..end].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), mimic_core::media::MediaError> {
        Ok(())
    }
}

/// Load `code` into the cartridge RAM at `at` and run it from there.
fn run_from_ram(m: &mut Sega8, at: u16, code: &[u8]) {
    for (i, &b) in code.iter().enumerate() {
        m.board.mem_write(at + i as u16, b);
    }
    m.cpu.pc = at;
}

#[test]
fn the_sc3000_records_and_plays_its_cassette() {
    let mut m = machine("sc3000", &[("cart", "demo")]);
    frames(&mut m, 2);
    assert!(m.media_slots().iter().any(|s| s.id == SLOT_CASSETTE));
    assert!(machine("sg1000", &[]).media_slot(SLOT_CASSETTE).is_none());
    let tape = m.media_slot(SLOT_CASSETTE).unwrap();
    tape.insert(Media {
        name: "blank.wav".into(),
        kind: MediaKind::Tape,
        backend: Box::new(Bytes(Vec::new())),
    })
    .unwrap();
    tape.control("record").unwrap();
    // PC3 high and low through the control register's bit set/reset,
    // about 2 kHz.
    run_from_ram(
        &mut m,
        0x8000,
        &[
            0xf3, // DI
            0x3e, 0x07, 0xd3, 0xdf, // LD A,7 ; OUT ($DF),A: PC3 = 1
            0x06, 0x40, 0x10, 0xfe, // LD B,64 ; DJNZ $
            0x3e, 0x06, 0xd3, 0xdf, // PC3 = 0
            0x06, 0x40, 0x10, 0xfe, // LD B,64 ; DJNZ $
            0x18, 0xee, // JR $8001
        ],
    );
    frames(&mut m, 20);
    let tape = m.media_slot(SLOT_CASSETTE).unwrap();
    tape.control("stop").unwrap();
    tape.control("rewind").unwrap();
    tape.control("play").unwrap();
    // Count the changes of port B bit 7 into $8100.
    run_from_ram(
        &mut m,
        0x8020,
        &[
            0xf3, // DI
            0x21, 0x00, 0x00, // LD HL,0
            0x0e, 0x00, // LD C,0
            0xdb, 0xdd, // IN A,($DD)
            0xe6, 0x80, // AND $80
            0xb9, // CP C
            0x28, 0xf9, // JR Z,$8026
            0x4f, // LD C,A
            0x23, // INC HL
            0x22, 0x00, 0x81, // LD ($8100),HL
            0x18, 0xf2, // JR $8026
        ],
    );
    frames(&mut m, 15);
    let changes = word(&m, 0x8100);
    // 15 fields of a 2 kHz square wave: about 1000 edges.
    assert!(changes > 500, "the recording plays back: {changes} edges");
}

#[test]
fn ppi_control_and_port_c() {
    let mut p = Ppi::default();
    p.write_control(0x92);
    p.c = 0x05;
    assert_eq!(p.row(), 5);
    p.write_control(0x07); // set bit 3
    assert_eq!(p.c, 0x0d);
    p.write_control(0x06); // reset bit 3
    assert_eq!(p.c, 0x05);
    p.write_control(0x93); // port C lower half an input
    assert_eq!(p.row(), 7);
}

#[test]
fn savestates_round_trip_and_reject_garbage() {
    let mut m = machine("sc3000", &[("cart", "demo")]);
    frames(&mut m, 4);
    let state = m.save_state();
    let mut n = machine("sc3000", &[("cart", "demo")]);
    frames(&mut n, 1);
    n.load_state(&state).unwrap();
    frames(&mut m, 5);
    frames(&mut n, 5);
    assert_eq!(m.save_state(), n.save_state());
    for cut in (0..state.len()).step_by(97) {
        let _ = n.load_state(&state[..cut]);
    }
    let mut other = machine("sg1000", &[("cart", "demo")]);
    assert!(other.load_state(&state).is_err(), "another model");
}

#[test]
fn overlays_and_graphics_views_come_from_the_vdp() {
    let mut m = machine("sg1000", &[("cart", "demo")]);
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["sprites", "sprite_bounds", "limit"]);
    m.set_overlays(&["sprites", "sprite_bounds"]);
    frames(&mut m, 4);
    assert_eq!(m.overlay("sprites").unwrap().shapes.len(), 1, "the block");
    assert_eq!(m.overlay("sprite_bounds").unwrap().shapes.len(), 1);
    assert!(m.gfx_views().iter().any(|v| v.id == "sprites"));
}
