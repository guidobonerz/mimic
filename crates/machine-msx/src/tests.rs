use super::*;
use chips_ti::vdp;
use emu_core::input::Dirs;
use emu_core::media::{Media, MediaBackend, MediaError};

fn stub_with(format: Option<&str>, options: &[(&str, &str)]) -> Msx {
    let mut config = MachineConfig {
        video_format: format.and_then(VideoStandard::parse),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Msx::new(&config).unwrap()
}

fn frames(m: &mut Msx, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn word(m: &Msx, addr: u16) -> u16 {
    u16::from(peek_cpu(m, addr)) | u16::from(peek_cpu(m, addr + 1)) << 8
}

/// Palette index at active-area pixel (x, y).
fn pixel(m: &Msx, x: u32, y: u32) -> u8 {
    let v = vdp(m);
    let row = y + u32::from(v.model.top());
    v.frame()[(row * vdp::WIDTH + 13 + x) as usize]
}

fn push(m: &mut Msx, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

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

fn insert(m: &mut Msx, slot: SlotId, rom: Vec<u8>) {
    m.media_slot(slot)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "test.rom".into(),
            backend: Box::new(Bytes(rom)),
        })
        .unwrap();
}

/// A cartridge image of `len` bytes for page 1 whose program is `code`
/// (at $4010), each 8 KB bank marked with its number at offset $1000.
fn cart(len: usize, code: &[u8]) -> Vec<u8> {
    let mut rom = vec![0xffu8; len];
    for (b, chunk) in rom.chunks_mut(0x2000).enumerate() {
        chunk[0x1000] = b as u8;
    }
    rom[..4].copy_from_slice(&[b'A', b'B', 0x10, 0x40]);
    rom[0x10..0x10 + code.len()].copy_from_slice(code);
    rom
}

#[test]
fn stub_bios_shows_its_screen_and_counts_frames_by_interrupt() {
    let mut m = stub_with(None, &[]);
    assert_eq!(m.model(), Model::Ntsc, "the stub's $002B says 60 Hz");
    frames(&mut m, 10);
    let n = word(&m, stub::FRAMES);
    assert!((7..=10).contains(&n), "{n} interrupts in 10 fields");
    // Stripes of the box pattern (yellow on dark blue) and the sprite.
    assert_eq!(pixel(&m, 8, 0), 10, "box top row: yellow");
    assert_eq!(pixel(&m, 9, 1), 4, "inside the box: dark blue");
    assert_eq!(pixel(&m, 0, 0), 4, "pattern 0 shows the background");
    assert_eq!(pixel(&m, 0x78, 0x61), 15, "the white sprite at (120, 97)");
}

#[test]
fn a_field_takes_262_or_313_lines() {
    for (format, lines) in [(None, 262u64), (Some("pal"), 313)] {
        let mut m = stub_with(format, &[]);
        frames(&mut m, 2);
        let a = m.now().0;
        frames(&mut m, 10);
        let per_field = (m.now().0 - a) as f64 / 10.0;
        assert!(
            (per_field - (lines * LINE_T) as f64).abs() < 30.0,
            "{per_field} T-states per field"
        );
        assert_eq!(m.framebuffer().height, vdp(&m).model.height());
    }
}

#[test]
fn every_m1_cycle_takes_a_wait_state() {
    // The stub idles in HALT: each of its NOPs is an M1 cycle of 4 + 1
    // T-states.
    let mut m = stub_with(None, &[]);
    frames(&mut m, 3);
    assert_eq!(peek_cpu(&m, m.cpu.pc.wrapping_sub(1)), 0x76, "halted");
    let a = m.now().0;
    m.step_cycle();
    assert_eq!(m.now().0 - a, 5);
}

#[test]
fn the_joystick_moves_the_sprite_and_the_trigger_sounds() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 3);
    assert_eq!(peek_cpu(&m, stub::JOY) & 0x3f, 0x3f, "nothing held");
    let x0 = peek_cpu(&m, stub::SPRITE_X);
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::RIGHT | Dirs::DOWN,
            buttons: 1,
        },
    );
    frames(&mut m, 10);
    // Bits 0–5: up, down, left, right, trigger A, trigger B; low when held.
    assert_eq!(peek_cpu(&m, stub::JOY) & 0x3f, 0x3f & !0x1a);
    let x1 = peek_cpu(&m, stub::SPRITE_X);
    assert!(x1 > x0 + 5, "moved right: {x0} → {x1}");
    assert!(peek_cpu(&m, stub::SPRITE_Y) > 0x65, "moved down");
    let mut audio = Vec::new();
    m.audio(&mut audio);
    let (min, max) = audio
        .iter()
        .skip(audio.len() / 2)
        .fold((i16::MAX, i16::MIN), |(a, b), &s| (a.min(s), b.max(s)));
    assert!(max - min > 4000, "a tone: {min}..{max}");
    // Port 2 is not selected: its joystick changes nothing.
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        },
    );
    push(
        &mut m,
        InputTarget::Port(PORT2),
        InputEvent::Digital {
            dirs: Dirs::LEFT,
            buttons: 2,
        },
    );
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, stub::JOY) & 0x3f, 0x3f);
    let x2 = peek_cpu(&m, stub::SPRITE_X);
    frames(&mut m, 5);
    assert_eq!(peek_cpu(&m, stub::SPRITE_X), x2, "stopped");
}

#[test]
fn psg_port_b_selects_the_joystick_port_and_holds_the_trigger_pins() {
    let mut ports: [Option<Box<dyn PortDevice>>; 2] = [
        Some(Box::new(devices_input::Joystick::new(PortKind::MsxJoy))),
        Some(Box::new(devices_input::Joystick::new(PortKind::MsxJoy))),
    ];
    ports[1].as_mut().unwrap().input(&InputEvent::Digital {
        dirs: Dirs::UP | Dirs::LEFT,
        buttons: 2,
    });
    let now = Cycle(0);
    assert_eq!(psg_port_a(&ports, 0x0f, now), 0xff, "port 1: idle");
    assert_eq!(
        psg_port_a(&ports, 0x4f, now),
        0xda,
        "port 2: up, left and trigger B"
    );
    assert_eq!(
        psg_port_a(&ports, 0x0e, now),
        0xef,
        "pin 6 of port 1 held low"
    );
    assert_eq!(
        psg_port_a(&ports, 0x47, now) & 0x30,
        0x10,
        "pin 7 of port 2"
    );
    ports[0] = None;
    assert_eq!(psg_port_a(&ports, 0x0f, now), 0xff, "an empty port floats");
}

#[test]
fn keys_show_in_their_matrix_row() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, stub::KEYS), 0xff);
    for (key, down) in [
        (keys::SPACE, true),
        (keys::key(8, 7), true),
        (keys::RETURN, true),
    ] {
        push(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(key),
                down,
            },
        );
    }
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, stub::KEYS), 0x7e, "row 8: SPACE and →");
    // The stub left row 8 selected: port B reads it, port C what was written.
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, IO_BASE + 0xa9, Size::Byte),
        Some(0x7e)
    );
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, IO_BASE + 0xaa, Size::Byte),
        Some(0x58)
    );
    push(
        &mut m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::SPACE),
            down: false,
        },
    );
    // A key outside the matrix is ignored.
    push(
        &mut m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(200),
            down: true,
        },
    );
    frames(&mut m, 2);
    assert_eq!(peek_cpu(&m, stub::KEYS), 0x7f);
}

#[test]
fn pages_follow_the_slot_register() {
    let mut m = stub_with(None, &[]);
    // After reset port A is an input: slot 0 everywhere.
    assert_eq!(m.board.slots(), 0);
    assert_eq!(peek_cpu(&m, 0x0000), 0xf3, "the BIOS");
    assert_eq!(peek_cpu(&m, 0xc000), 0xff, "slot 0 has nothing in page 3");
    frames(&mut m, 2);
    // The stub's demo: pages 3 and 2 RAM, 1 and 0 slot 0.
    assert_eq!(m.board.slots(), 0xf0);
    assert!(m.poke(CPU_SPACE, 0xc123, Size::Word, 0xbeef));
    assert_eq!(word(&m, 0xc123), 0xbeef);
    assert!(
        !m.poke(CPU_SPACE, 0x0000, Size::Byte, 0),
        "ROM is not poked"
    );
    assert_eq!(peek_cpu(&m, 0x4000), 0xff, "slot 0, page 1 of the stub");
}

#[test]
fn less_ram_sits_at_the_top() {
    let mut m = stub_with(None, &[("ram", "16")]);
    frames(&mut m, 2);
    assert_eq!(m.board.ram.len(), 0x4000);
    assert!(m.poke(CPU_SPACE, 0xc000, Size::Byte, 0x5a));
    assert_eq!(peek_cpu(&m, 0xc000), 0x5a);
    assert!(
        !m.poke(CPU_SPACE, 0xbfff, Size::Byte, 0x5a),
        "page 2 is empty"
    );
    assert_eq!(peek_cpu(&m, 0xbfff), 0xff);
    assert!(n_frames_counted(&m), "the stub's stack and variables work");
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("ram".into(), "48".into());
    assert!(Msx::new(&config).is_err());
}

fn n_frames_counted(m: &Msx) -> bool {
    word(m, stub::FRAMES) > 0
}

#[test]
fn the_stub_starts_a_cartridge_in_either_slot() {
    // LD A,$77 ; LD ($E010),A ; DI ; HALT
    let code = [0x3e, 0x77, 0x32, 0x10, 0xe0, 0xf3, 0x76];
    for slot in [SLOT_CART1, SLOT_CART2] {
        let mut m = stub_with(None, &[]);
        insert(&mut m, slot, cart(0x4000, &code));
        m.step_cycle();
        assert!(m.media_slots()[usize::from(slot.0)].inserted);
        for _ in 0..2000 {
            m.step_cycle();
        }
        // Pages 1 and 2 on the cartridge's slot, RAM in page 3.
        let expect = if slot == SLOT_CART1 { 0xd4 } else { 0xe8 };
        assert_eq!(m.board.slots(), expect);
        assert_eq!(peek_cpu(&m, 0xe010), 0x77, "the cartridge ran");
        assert_eq!(peek_cpu(&m, 0x4000), b'A');
        assert_eq!(peek_cpu(&m, 0x8000), 0xff, "16 KB: nothing in page 2");
    }
}

#[test]
fn megarom_banks_switch_and_the_mapper_option_overrides_the_guess() {
    // LD A,5 ; LD ($7000),A ; DI ; HALT — ASCII 8 (the unreached LD ($6800),A says so): the window at $8000.
    let code = [0x3e, 0x05, 0x32, 0x00, 0x70, 0xf3, 0x76, 0x32, 0x00, 0x68];
    let rom = cart(0x2_0000, &code);
    let mut m = stub_with(None, &[]);
    insert(&mut m, SLOT_CART1, rom.clone());
    for _ in 0..2000 {
        m.step_cycle();
    }
    assert_eq!(m.board.carts[0].as_ref().unwrap().mapper(), Mapper::Ascii8);
    assert_eq!(peek_cpu(&m, 0x9000), 5);
    let banks = m
        .chips()
        .into_iter()
        .find(|c| c.name == "slots")
        .unwrap()
        .regs;
    let bank = banks.iter().find(|r| r.name == "CART1_8000").unwrap();
    assert_eq!(bank.value, 5);

    // The same image as a Konami SCC cartridge: $7000 is the $6000 window.
    let mut m = stub_with(None, &[("mapper1", "konamiscc")]);
    insert(&mut m, SLOT_CART1, rom);
    for _ in 0..2000 {
        m.step_cycle();
    }
    assert_eq!(peek_cpu(&m, 0x7000), 5);
    assert_eq!(peek_cpu(&m, 0x9000), 2);
    // RESET puts the banks back.
    push(
        &mut m,
        InputTarget::Panel,
        InputEvent::Control {
            id: emu_core::ControlId(1),
            value: 1,
        },
    );
    m.step_cycle();
    assert_eq!(m.board.carts[0].as_ref().unwrap().banks, [0, 1, 2, 3]);

    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("mapper2".into(), "mbc5".into());
    assert!(Msx::new(&config).is_err());
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub_with(Some("pal"), &[]);
    insert(
        &mut a,
        SLOT_CART2,
        cart(0x2_0000, &[0xfb, 0x76, 0x18, 0xfd]),
    );
    frames(&mut a, 5);
    push(
        &mut a,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::SHIFT),
            down: true,
        },
    );
    frames(&mut a, 2);
    let state = a.save_state();
    let mut b = stub_with(Some("pal"), &[]);
    insert(
        &mut b,
        SLOT_CART2,
        cart(0x2_0000, &[0xfb, 0x76, 0x18, 0xfd]),
    );
    b.step_cycle();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    frames(&mut a, 3);
    frames(&mut b, 3);
    assert_eq!(a.now(), b.now());
    assert_eq!(a.framebuffer().pixels, b.framebuffer().pixels);
    assert_eq!(a.save_state(), b.save_state());

    // Another configuration refuses the state and stays as it was.
    let mut ntsc = stub_with(None, &[]);
    assert!(ntsc.load_state(&state).is_err());
    let mut no_cart = stub_with(Some("pal"), &[]);
    let before = no_cart.save_state();
    assert!(no_cart.load_state(&state).is_err());
    assert_eq!(no_cart.save_state(), before);
    // Corrupt data never panics.
    for cut in [0, 1, state.len() / 2, state.len() - 1] {
        assert!(b.load_state(&state[..cut]).is_err());
    }
}

#[test]
fn configuration_errors() {
    let config = MachineConfig::default();
    assert!(
        matches!(Msx::new(&config), Err(ConfigError::MissingRom(r)) if r == "msx-bios"),
        "the BIOS is needed without roms=stub"
    );
    let mut config = MachineConfig::default();
    config.roms.insert("msx-bios".into(), vec![0; 0x4000]);
    assert!(matches!(Msx::new(&config), Err(ConfigError::Invalid(_))));
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.model = Some("msx3".into());
    assert!(matches!(
        Msx::new(&config),
        Err(ConfigError::UnknownModel(_))
    ));
    config.model = None;
    config.options.insert("port1".into(), "mouse".into());
    assert!(Msx::new(&config).is_err());
}

#[test]
fn the_bios_names_the_field_rate_and_the_logo_rom_needs_c_bios() {
    // A BIOS that only halts; $002B bit 7: 50 Hz.
    let mut bios = vec![0xffu8; 0x8000];
    bios[..2].copy_from_slice(&[0xf3, 0x76]);
    bios[0x2b] = 0xa1;
    let mut config = MachineConfig::default();
    config.roms.insert("msx-bios".into(), bios.clone());
    config.roms.insert("msx-logo".into(), vec![0x12; 0x4000]);
    let m = Msx::new(&config).unwrap();
    assert_eq!(m.model(), Model::Pal);
    assert_eq!(
        peek_cpu(&m, 0x8000),
        0xff,
        "no logo ROM beside another BIOS"
    );
    config.video_format = VideoStandard::parse("ntsc");
    assert_eq!(Msx::new(&config).unwrap().model(), Model::Ntsc);
    bios[0x100..0x106].copy_from_slice(b"C-BIOS");
    config.roms.insert("msx-bios".into(), bios);
    let m = Msx::new(&config).unwrap();
    assert_eq!(peek_cpu(&m, 0x8000), 0x12);
    assert_eq!(peek_cpu(&m, 0xc000), 0xff);
}

#[test]
fn ports_take_msx_joysticks_only() {
    let mut m = stub_with(None, &[("port2", "none")]);
    let ports = m.ports();
    assert_eq!(ports.len(), 2);
    assert!(ports[0].connected && !ports[1].connected);
    assert!(matches!(
        m.connect(
            PORT2,
            Box::new(devices_input::Joystick::new(PortKind::AtariJoy))
        ),
        Err(PortError::Incompatible { .. })
    ));
    m.connect(
        PORT2,
        Box::new(devices_input::Joystick::new(PortKind::MsxJoy)),
    )
    .unwrap();
    assert!(matches!(
        m.connect(
            PORT2,
            Box::new(devices_input::Joystick::new(PortKind::MsxJoy))
        ),
        Err(PortError::Occupied(_))
    ));
    assert!(m.disconnect(PORT1).is_some());
    assert!(m.disconnect(PortId(7)).is_none());
}

/// Peak-to-peak level of the second half of the sound since the last call.
fn swing(m: &mut Msx) -> i32 {
    let mut audio = Vec::new();
    m.audio(&mut audio);
    let (min, max) = audio
        .iter()
        .skip(audio.len() / 2)
        .fold((i16::MAX, i16::MIN), |(a, b), &s| (a.min(s), b.max(s)));
    i32::from(max) - i32::from(min)
}

#[test]
fn a_konami_scc_cartridge_sounds() {
    let code = [
        0x3e, 0x3f, 0x32, 0x00, 0x90, // LD A,$3F ; LD ($9000),A: the SCC
        0x21, 0x00, 0x98, 0x06, 0x10, // LD HL,$9800 ; LD B,16
        0x36, 0x7f, 0x23, 0x10, 0xfb, // LD (HL),$7F ; INC HL ; DJNZ: a square
        0x3e, 0x40, 0x32, 0x80, 0x98, // period 64: 1721 Hz
        0x3e, 0x0f, 0x32, 0x8a, 0x98, // volume 15
        0x3e, 0x01, 0x32, 0x8f, 0x98, // channel 1 on
        0xf3, 0x76, // DI ; HALT
    ];
    let rom = cart(0x2_0000, &code);
    let mut m = stub_with(None, &[]);
    insert(&mut m, SLOT_CART1, rom.clone());
    frames(&mut m, 4);
    let c = m.board.carts[0].as_ref().unwrap();
    assert_eq!(
        c.mapper(),
        Mapper::KonamiScc,
        "guessed from the $9000 write"
    );
    assert_eq!(c.scc.as_ref().unwrap().period[0], 0x40);
    assert!(m.board.cart_sound);
    swing(&mut m);
    frames(&mut m, 4);
    let loud = swing(&mut m);
    assert!(loud > 1000, "a tone: {loud}");
    // The state carries the chip along.
    let state = m.save_state();
    m.load_state(&state).unwrap();
    assert_eq!(m.save_state(), state);
    // The same image in a cartridge without the chip stays silent.
    let mut m = stub_with(None, &[("mapper1", "konami")]);
    insert(&mut m, SLOT_CART1, rom);
    frames(&mut m, 4);
    swing(&mut m);
    frames(&mut m, 4);
    assert!(!m.board.cart_sound);
    let quiet = swing(&mut m);
    assert!(quiet < 50, "silence: {quiet}");
}

/// A file the test can look into after the machine wrote it.
#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let d = self.0.lock().unwrap();
        let o = offset as usize;
        buf.copy_from_slice(&d[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut d = self.0.lock().unwrap();
        let o = offset as usize;
        d[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.0.lock().unwrap().resize(len as usize, 0);
        Ok(())
    }
}

#[test]
fn battery_ram_goes_to_its_file_and_back() {
    let code = [
        0x3e, 0x80, 0x32, 0x00, 0x70, // LD A,$80 ; LD ($7000),A: RAM at $8000
        0x3a, 0x01, 0x80, 0x32, 0x20, 0xe0, // LD A,($8001) ; LD ($E020),A
        0x3e, 0xc3, 0x32, 0x00, 0x80, // LD A,$C3 ; LD ($8000),A
        0xf3, 0x76, // DI ; HALT
    ];
    let rom = cart(0x2_0000, &code);
    let file = Shared::default();
    let srm = |f: &Shared| Media {
        kind: MediaKind::BatteryRam,
        name: "test.srm".into(),
        backend: Box::new(f.clone()),
    };
    let sram = |m: &Msx| m.board.carts[0].as_ref().unwrap().sram().to_vec();

    let mut m = stub_with(None, &[("mapper1", "ascii8sram")]);
    insert(&mut m, SLOT_CART1, rom.clone());
    m.media_slot(SLOT_BATTERY)
        .unwrap()
        .insert(srm(&file))
        .unwrap();
    assert!(m.media_slots()[2].inserted);
    frames(&mut m, 2);
    {
        let d = file.0.lock().unwrap();
        assert_eq!(d.len(), 0x2000);
        assert_eq!(d[0], 0xc3);
    }
    // A power cycle keeps it.
    m.board.carts[0].as_mut().unwrap().sram_mut()[1] = 0x17;
    frames(&mut m, 1);
    m.reset(ResetKind::Hard);
    frames(&mut m, 1);
    assert_eq!(peek_cpu(&m, 0xe020), 0x17);
    assert_eq!(file.0.lock().unwrap()[1], 0x17);

    // Another machine starts from the file.
    file.0.lock().unwrap()[1] = 0x99;
    let mut m = stub_with(None, &[("mapper1", "ascii8sram")]);
    insert(&mut m, SLOT_CART1, rom.clone());
    m.media_slot(SLOT_BATTERY)
        .unwrap()
        .insert(srm(&file))
        .unwrap();
    frames(&mut m, 1);
    assert_eq!(peek_cpu(&m, 0xe020), 0x99);
    assert_eq!(sram(&m)[..2], [0xc3, 0x99]);
    // The state carries the RAM.
    let state = m.save_state();
    m.board.carts[0].as_mut().unwrap().sram_mut()[5] = 1;
    m.load_state(&state).unwrap();
    assert_eq!(sram(&m)[5], 0);

    // Wrong sizes and cartridges without a battery refuse the file.
    file.0.lock().unwrap().truncate(100);
    assert!(matches!(
        m.media_slot(SLOT_BATTERY).unwrap().insert(srm(&file)),
        Err(MediaError::Format(_))
    ));
    let mut m = stub_with(None, &[]);
    insert(&mut m, SLOT_CART1, rom);
    assert!(matches!(
        m.media_slot(SLOT_BATTERY).unwrap().insert(srm(&file)),
        Err(MediaError::Unsupported(_))
    ));
    // Taking the cartridge out takes the battery with it.
    let mut m = stub_with(None, &[("mapper1", "gamemaster2")]);
    insert(&mut m, SLOT_CART1, cart(0x2_0000, &[0xf3, 0x76]));
    file.0.lock().unwrap().clear();
    m.media_slot(SLOT_BATTERY)
        .unwrap()
        .insert(srm(&file))
        .unwrap();
    assert_eq!(file.0.lock().unwrap().len(), 0, "nothing kept yet");
    frames(&mut m, 1);
    assert_eq!(file.0.lock().unwrap().len(), 0x2000);
    assert!(m.media_slot(SLOT_CART1).unwrap().eject().is_some());
    assert!(!m.media_slots()[2].inserted);
}

#[test]
fn the_motor_relay_runs_the_cassette_and_the_psg_reads_it() {
    // Motor on (port C bit 4 low), then read PSG register 14 for ever,
    // keeping the lowest and the highest value seen at $E030/$E031.
    let code = [
        0x3e, 0x00, 0xd3, 0xaa, // LD A,0 ; OUT ($AA),A
        0x3e, 0x0e, 0xd3, 0xa0, // LD A,14 ; OUT ($A0),A
        0x21, 0x30, 0xe0, 0x36, 0xff, 0x23, 0x36, 0x00, // ($E030) = $FF, ($E031) = 0
        0xdb, 0xa2, 0x47, // loop: IN A,($A2) ; LD B,A
        0x21, 0x30, 0xe0, 0xa6, 0x77, // LD HL,$E030 ; AND (HL) ; LD (HL),A
        0x23, 0x78, 0xb6, 0x77, // INC HL ; LD A,B ; OR (HL) ; LD (HL),A
        0x18, 0xf2, // JR loop
    ];
    let tape = media_tape(&[vec![0x55; 4]]);
    let mut m = stub_with(None, &[]);
    insert(&mut m, SLOT_CART1, cart(0x4000, &code));
    m.media_slot(SLOT_TAPE).unwrap().insert(tape).unwrap();
    assert!(m.media_slots()[3].inserted);
    frames(&mut m, 30);
    assert!(m.media_slot(SLOT_TAPE).unwrap().status().active);
    assert_eq!(peek_cpu(&m, 0xe030) & 0x80, 0, "the gap is low");
    assert_eq!(
        peek_cpu(&m, 0xe031) & 0x80,
        0,
        "no tone in the first second"
    );
    frames(&mut m, 60);
    assert_eq!(peek_cpu(&m, 0xe031) & 0x80, 0x80, "the header tone");
    // The state keeps the deck's place.
    let state = m.save_state();
    let mut n = stub_with(None, &[]);
    insert(&mut n, SLOT_CART1, cart(0x4000, &code));
    n.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(media_tape(&[vec![0x55; 4]]))
        .unwrap();
    n.step_cycle();
    n.load_state(&state).unwrap();
    assert_eq!(n.save_state(), state);
    assert_eq!(n.tape.counter(), m.tape.counter());
    // After the 7.4 s of the tape PLAY springs up.
    frames(&mut m, 400);
    assert_eq!(m.tape.counter(), (1, 1));
    assert!(!m.media_slot(SLOT_TAPE).unwrap().status().active);
    // RESET lets the relay go.
    m.media_slot(SLOT_TAPE).unwrap().control("rewind").unwrap();
    m.media_slot(SLOT_TAPE).unwrap().control("play").unwrap();
    assert!(m.tape.running());
    m.reset(ResetKind::Soft);
    assert!(!m.tape.running());
}

fn media_tape(blocks: &[Vec<u8>]) -> Media {
    let mut image = Vec::new();
    for b in blocks {
        image.resize(image.len().next_multiple_of(8), 0);
        image.extend_from_slice(&[0x1f, 0xa6, 0xde, 0xba, 0xcc, 0x13, 0x7d, 0x74]);
        image.extend_from_slice(b);
    }
    Media {
        kind: MediaKind::Tape,
        name: "test.cas".into(),
        backend: Box::new(Bytes(image)),
    }
}

#[test]
fn the_disk_rom_expands_slot_3() {
    let mut rom = vec![0xffu8; 0x4000];
    rom[..2].copy_from_slice(&[0x12, 0x34]);
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.roms.insert("msx-disk-svi738".into(), rom);
    let mut m = Msx::new(&config).unwrap();
    assert_eq!(m.media_slots().len(), 6, "drives A and B");
    assert!(m.media_slot(SLOT_DRIVE_B).is_some());
    frames(&mut m, 2);
    // The stub's demo: pages 3 and 2 on slot 3, there 3-2, the RAM.
    assert_eq!(m.board.slots(), 0xf0);
    assert_eq!(m.board.sub, 0xaa);
    assert_eq!(peek_cpu(&m, 0xffff), 0x55, "the register reads inverted");
    assert!(n_frames_counted(&m), "the RAM is where the stub expects it");
    // Pages 1 and 2 on slot 3-1: the disk ROM.
    m.board.mem_write(0xffff, 0x96);
    assert_eq!(peek_cpu(&m, 0xffff), !0x96);
    assert_eq!(peek_cpu(&m, 0x8000), 0x12, "the ROM repeats in page 2");
    assert!(!m.poke(CPU_SPACE, 0x8000, Size::Byte, 1), "no RAM in 3-1");
    assert!(
        m.poke(CPU_SPACE, 0xc000, Size::Byte, 1),
        "page 3 is still 3-2"
    );
    m.board.ppi.write(0, 0xfc);
    assert_eq!((peek_cpu(&m, 0x4000), peek_cpu(&m, 0x4001)), (0x12, 0x34));
    assert!(m.board.disk_selected(0x7fbc) && !m.board.disk_selected(0x7fb0));
    assert!(m.board.disk_selected(0xbfbc) && !m.board.disk_selected(0xffbc));
    let state = m.save_state();
    m.reset(ResetKind::Soft);
    assert_eq!(m.board.sub, 0);
    m.load_state(&state).unwrap();
    assert_eq!(m.board.sub, 0x96);
    // 3-0 and 3-3 are empty.
    m.board.mem_write(0xffff, 0x00);
    assert_eq!(peek_cpu(&m, 0xc000), 0xff);
    assert!(!m.poke(CPU_SPACE, 0xc000, Size::Byte, 1));
    // Without the interface: no register, no drives; states do not mix.
    config.options.insert("disk".into(), "off".into());
    let mut plain = Msx::new(&config).unwrap();
    assert_eq!(plain.media_slots().len(), 4);
    assert!(plain.media_slot(SLOT_DRIVE_A).is_none());
    assert!(plain.load_state(&state).is_err());
    config.roms.clear();
    config.options.insert("disk".into(), "on".into());
    assert!(matches!(Msx::new(&config), Err(ConfigError::MissingRom(_))));
}

fn msx2_stub(format: Option<&str>, options: &[(&str, &str)]) -> Msx {
    let mut config = MachineConfig {
        model: Some("msx2".into()),
        video_format: format.and_then(VideoStandard::parse),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Msx::new(&config).unwrap()
}

/// The colour at active-area pixel (x, y) of a machine with a V9938.
fn rgb(m: &Msx, x: u32, y: u32) -> emu_core::frame::Rgb8 {
    let Video::V9938(v) = video(m) else {
        panic!("no V9938");
    };
    let row = y + u32::from(v.display_start());
    m.framebuffer()
        .rgb_at(chips_yamaha::v9938::LEFT as u32 + 2 * x, row)
        .unwrap()
}

fn default_color(n: usize) -> emu_core::frame::Rgb8 {
    let (r, g, b) = chips_yamaha::v9938::DEFAULT_PALETTE[n];
    chips_yamaha::v9938::rgb777(r, g, b)
}

#[test]
fn an_msx2_runs_the_stub_on_its_v9938() {
    for (format, lines, height) in [(None, 262u32, 243), (Some("pal"), 313, 294)] {
        let mut m = msx2_stub(format, &[]);
        assert_eq!(m.generation(), Generation::Msx2);
        frames(&mut m, 10);
        assert!(n_frames_counted(&m), "the RAM is in slot 3-2");
        let f = m.framebuffer();
        assert_eq!((f.width, f.height), (568, height));
        assert_eq!(f.signal.lines_per_field, lines);
        assert_eq!(f.signal.total_pixels, 684);
        assert_eq!(f.signal.output, OutputType::Rgb);
        assert!(matches!(f.pixels, emu_core::frame::PixelData::Rgb(_)));
        assert_eq!(rgb(&m, 8, 0), default_color(10), "box top row: yellow");
        assert_eq!(rgb(&m, 9, 1), default_color(4));
        assert_eq!(rgb(&m, 0x78, 0x61), default_color(15), "the sprite");
        let spaces = m.memory_spaces();
        assert_eq!(spaces[1].addr_bits, 17);
        assert!(m.poke(VRAM_SPACE, 0x1_2345, Size::Byte, 0x5a));
        assert_eq!(m.peek(VRAM_SPACE, 0x1_2345, Size::Byte), Some(0x5a));
        assert!(m.gfx_views().iter().all(|v| v.name.starts_with("V9938")));
        assert!(m.gfx_view("screen", &GfxOptions::new()).is_some());
        assert!(m.chips()[0].regs.iter().any(|r| r.name == "S2"));
    }
    // An MSX1 can have the chip too (as the Spectravideo SVI-738 has).
    let mut m = stub_with(Some("pal"), &[("vdp", "v9938")]);
    assert_eq!(m.generation(), Generation::Msx1);
    frames(&mut m, 10);
    assert_eq!(m.framebuffer().signal.lines_per_field, 313);
    assert_eq!(rgb(&m, 8, 0), default_color(10));
    assert!(m.board.mapper.is_none() && m.board.rtc.is_none());
}

#[test]
fn the_memory_mapper_switches_segments() {
    let mut m = msx2_stub(None, &[]);
    assert_eq!(m.board.ram.len(), 256 * 1024);
    // Segment 5 into page 3, a byte there; segment 0 back, another.
    let code = [
        0xf3, 0x3e, 0x05, 0xd3, 0xff, 0x3e, 0x77, 0x32, 0x00, 0xc0, 0x3e, 0x00, 0xd3, 0xff, 0x3e,
        0x11, 0x32, 0x00, 0xc0, 0x18, 0xfe,
    ];
    insert(&mut m, SLOT_CART1, cart(0x4000, &code));
    frames(&mut m, 5);
    assert_eq!(m.board.ram[5 * 0x4000], 0x77);
    assert_eq!(m.board.ram[0], 0x11);
    assert_eq!(m.board.mapper, Some([3, 2, 1, 0]));
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0xfc, Size::Byte), Some(0xf3));
    assert!(m.chips()[3].regs.iter().any(|r| r.name == "SEGMENT3"));
    // The segment number wraps at the RAM's size; a reset restores the order.
    m.board.mapper = Some([3, 2, 1, 16 + 5]);
    assert_eq!(peek_cpu(&m, 0xc000), 0x77);
    m.reset(ResetKind::Soft);
    assert_eq!(m.board.mapper, Some([3, 2, 1, 0]));
    for (ram, len) in [("64", 64), ("4096", 4096)] {
        assert_eq!(msx2_stub(None, &[("ram", ram)]).board.ram.len(), len * 1024);
    }
    let mut config = MachineConfig {
        model: Some("msx2".into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    for (key, value) in [("ram", "32"), ("vdp", "tms9918")] {
        let mut c = config.clone();
        c.options.insert(key.into(), value.into());
        assert!(
            matches!(Msx::new(&c), Err(ConfigError::Invalid(_))),
            "{key}"
        );
    }
}

#[test]
fn the_clock_chip_answers_at_b4_and_runs_with_the_machine() {
    let mut m = msx2_stub(None, &[]);
    // Block 2, register 0 = 10; read it back into $C000.
    let code = [
        0xf3, 0x3e, 0x0d, 0xd3, 0xb4, 0x3e, 0x0a, 0xd3, 0xb5, 0x3e, 0x00, 0xd3, 0xb4, 0x3e, 0x0a,
        0xd3, 0xb5, 0xdb, 0xb5, 0x32, 0x00, 0xc0, 0x18, 0xfe,
    ];
    insert(&mut m, SLOT_CART1, cart(0x4000, &code));
    frames(&mut m, 5);
    assert_eq!(m.board.rtc.as_ref().unwrap().blocks[2][0], 10);
    assert_eq!(peek_cpu(&m, 0xc000), 0xfa);
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0xb5, Size::Byte), Some(0xfa));
    // 150 fields of 1/60 s: two seconds, and half a one.
    frames(&mut m, 145);
    assert_eq!(m.board.rtc.as_ref().unwrap().blocks[0][0], 2);
    // The battery keeps it through a power cycle; an MSX1 has none.
    m.reset(ResetKind::Hard);
    assert_eq!(m.board.rtc.as_ref().unwrap().blocks[2][0], 10);
    let m1 = stub_with(None, &[]);
    assert_eq!(m1.peek(CPU_SPACE, IO_BASE + 0xb5, Size::Byte), Some(0xff));
}

#[test]
fn an_msx2_needs_its_roms_and_has_the_sub_rom_in_slot_3_0() {
    let mut config = MachineConfig {
        model: Some("msx2".into()),
        ..MachineConfig::default()
    };
    // An MSX1 BIOS does not count.
    config.roms.insert("msx-bios".into(), vec![0; 0x8000]);
    assert!(matches!(Msx::new(&config), Err(ConfigError::MissingRom(r)) if r == "msx2-bios"));
    let mut bios = vec![0xffu8; 0x8000];
    bios[..2].copy_from_slice(&[0xf3, 0x76]);
    bios[0x2b] = 0x80;
    config.roms.insert("msx2-bios".into(), bios);
    // C-BIOS's sub ROM does not stand in for a real BIOS's.
    config.roms.insert("msx2-sub-cbios".into(), vec![0; 0x4000]);
    assert!(matches!(Msx::new(&config), Err(ConfigError::MissingRom(r)) if r == "msx2-sub"));
    config.roms.insert("msx2-sub".into(), vec![0; 0x2000]);
    assert!(matches!(Msx::new(&config), Err(ConfigError::Invalid(_))));
    let mut sub = vec![0x5au8; 0x4000];
    sub[..2].copy_from_slice(b"CD");
    config.roms.insert("msx2-sub".into(), sub);
    let mut m = Msx::new(&config).unwrap();
    assert_eq!(m.model(), Model::Pal, "the BIOS's $002B");
    assert_eq!(m.framebuffer().signal.lines_per_field, 313);
    assert_eq!(
        m.media_slots().len(),
        4,
        "no disk interface without its ROM"
    );
    // Page 0 on slot 3, there 3-0.
    m.board.ppi.write(3, 0x82);
    m.board.ppi.write(0, 0xc3);
    assert_eq!(peek_cpu(&m, 0xffff), 0xff, "the register reads inverted");
    assert_eq!((peek_cpu(&m, 0), peek_cpu(&m, 2)), (b'C', 0x5a));
    assert_eq!(peek_cpu(&m, 0xc000), 0xff, "3-0 has only page 0");
    m.board.mem_write(0xffff, 0x80);
    assert_eq!(peek_cpu(&m, 0xc000), 0x00, "the RAM in 3-2");
    // With the disk ROM: 3-1, beside the sub ROM.
    config
        .roms
        .insert("msx-disk-svi738".into(), vec![0x12; 0x4000]);
    let mut m = Msx::new(&config).unwrap();
    assert_eq!(m.media_slots().len(), 6);
    m.board.ppi.write(3, 0x82);
    m.board.ppi.write(0, 0xcf);
    m.board.mem_write(0xffff, 0x84);
    assert_eq!((peek_cpu(&m, 0), peek_cpu(&m, 0x4000)), (b'C', 0x12));
}

#[test]
fn msx2_savestates_are_deterministic_and_do_not_mix() {
    let mut a = msx2_stub(None, &[]);
    frames(&mut a, 7);
    a.board.mapper = Some([3, 2, 1, 0]);
    let state = a.save_state();
    let mut b = msx2_stub(None, &[]);
    b.load_state(&state).unwrap();
    frames(&mut a, 20);
    frames(&mut b, 20);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.framebuffer(), b.framebuffer());
    let mut msx1 = stub_with(None, &[]);
    assert!(msx1.load_state(&state).is_err());
    assert!(a.load_state(&msx1.save_state()).is_err());
    for cut in [0, 40, state.len() / 2, state.len() - 3] {
        assert!(b.load_state(&state[..cut]).is_err());
    }
}
