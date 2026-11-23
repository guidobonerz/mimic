use super::*;
use mimic_core::input::MachineKey;
use mimic_core::media::MediaBackend;

/// A medium held in memory, shared with the test.
#[derive(Clone)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl MediaBackend for Shared {
    fn len(&self) -> u64 {
        self.0.lock().unwrap().len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let data = self.0.lock().unwrap();
        let at = offset as usize;
        buf.copy_from_slice(&data[at..at + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        let mut own = self.0.lock().unwrap();
        let at = offset as usize;
        own[at..at + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn set_len(&mut self, len: u64) -> Result<(), MediaError> {
        self.0.lock().unwrap().resize(len as usize, 0);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn medium(kind: MediaKind, bytes: Vec<u8>) -> (Media, Shared) {
    let shared = Shared(std::sync::Arc::new(std::sync::Mutex::new(bytes)));
    let media = Media {
        name: "test".into(),
        kind,
        backend: Box::new(shared.clone()),
    };
    (media, shared)
}

fn machine(model: Option<&str>) -> GameBoy {
    let config = MachineConfig {
        model: model.map(str::to_string),
        ..MachineConfig::default()
    };
    GameBoy::new(&config).unwrap()
}

fn with_demo() -> GameBoy {
    let mut m = machine(None);
    let (media, _) = medium(MediaKind::Cartridge, demo::rom());
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    m
}

fn frames(m: &mut GameBoy, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn send(m: &mut GameBoy, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

#[test]
fn the_demo_draws_sends_and_counts() {
    let mut m = with_demo();
    frames(&mut m, 12);
    assert_eq!(m.take_serial(), b"OK");
    // Tile 1 at the top left: shade 3; white beside it.
    let f = m.framebuffer();
    assert_eq!((f.width, f.height), (160, 144));
    let PixelData::Indexed { pixels, palette } = f.pixels else {
        panic!("indexed");
    };
    assert_eq!(palette.len(), 4);
    assert_eq!(pixels[..9], [3, 3, 3, 3, 3, 3, 3, 3, 0]);
    assert_eq!((pixels[7 * 160], pixels[8 * 160]), (3, 0));
    // One interrupt a frame.
    let before = peek_cpu(&m, demo::BLANKS);
    frames(&mut m, 10);
    assert_eq!(peek_cpu(&m, demo::BLANKS).wrapping_sub(before), 10);
    assert_eq!(peek_cpu(&m, 0xa000), demo::SAVED);
    assert_eq!(m.board.cart.as_ref().unwrap().title(), "MIMIC DEMO");
}

#[test]
fn a_frame_is_70224_clocks() {
    let mut m = with_demo();
    frames(&mut m, 5);
    let start = m.now().0;
    frames(&mut m, 60);
    let clocks = m.now().0 - start;
    // The frame ends with the instruction running: a few clocks either way.
    assert!((clocks as i64 - 60 * 70_224).abs() < 40, "{clocks}");
    let s = m.framebuffer().signal;
    assert_eq!(
        (s.pixel_clock_hz, s.total_pixels, s.lines_per_field),
        (CLOCK_HZ, 456, 154)
    );
    assert_eq!(m.beam().unwrap().line, 144);
}

#[test]
fn buttons_come_from_the_pad_and_the_keyboard() {
    let mut m = with_demo();
    frames(&mut m, 12);
    assert_eq!(
        (peek_cpu(&m, demo::BUTTONS), peek_cpu(&m, demo::DIRECTIONS)),
        (0, 0)
    );
    send(
        &mut m,
        InputTarget::Port(PORT_BUTTONS),
        InputEvent::Digital {
            dirs: Dirs(Dirs::LEFT.0 | Dirs::UP.0),
            buttons: 0b1001,
        },
    );
    frames(&mut m, 2);
    // A and Start; Left and Up.
    assert_eq!(peek_cpu(&m, demo::BUTTONS), 0b1001);
    assert_eq!(peek_cpu(&m, demo::DIRECTIONS), 0b0110);
    send(
        &mut m,
        InputTarget::Port(PORT_BUTTONS),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        },
    );
    for (key, bit) in [
        (keys::A, 1),
        (keys::B, 2),
        (keys::SELECT, 4),
        (keys::START, 8),
    ] {
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(key),
                down: true,
            },
        );
        frames(&mut m, 2);
        assert_eq!(peek_cpu(&m, demo::BUTTONS), bit);
        send(
            &mut m,
            InputTarget::Keyboard,
            InputEvent::Key {
                machine_key: MachineKey(key),
                down: false,
            },
        );
        frames(&mut m, 2);
    }
    assert_eq!(peek_cpu(&m, demo::BUTTONS), 0);
}

#[test]
fn a_button_requests_the_joypad_interrupt() {
    let mut m = with_demo();
    frames(&mut m, 12);
    m.board.requested = 0;
    // The program leaves the directions selected.
    m.board.select = 0x20;
    send(
        &mut m,
        InputTarget::Port(PORT_BUTTONS),
        InputEvent::Digital {
            dirs: Dirs::DOWN,
            buttons: 0,
        },
    );
    m.step_cycle();
    assert_eq!(m.board.requested & INT_JOYPAD, INT_JOYPAD);
    assert_eq!(m.board.io_read(0xff00), 0xc0 | 0x20 | 0x07);
    // A button of the half that is not selected does not.
    m.board.requested = 0;
    send(
        &mut m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::A),
            down: true,
        },
    );
    m.step_cycle();
    assert_eq!(m.board.requested & INT_JOYPAD, 0);
}

#[test]
fn dma_copies_a_page_into_the_object_table() {
    let mut m = with_demo();
    frames(&mut m, 12);
    for i in 0..160u16 {
        m.board.write(0xc100 + i, i as u8 ^ 0x5a, false);
    }
    m.board.write(0xff46, 0xc1, false);
    assert_eq!(m.board.dma_delay, 2);
    // Two cycles until it starts, then a byte a cycle.
    for _ in 0..2 {
        m.board.tick();
    }
    assert_eq!(m.board.dma_at, Some(0));
    assert_eq!(m.board.read(0xfe00, true), 0xff, "the table is busy");
    for _ in 0..160 {
        m.board.tick();
    }
    assert_eq!(m.board.dma_at, None);
    for i in 0..160usize {
        assert_eq!(m.board.ppu.oam[i], i as u8 ^ 0x5a);
    }
    assert_eq!(peek_cpu(&m, 0xff46), 0xc1);
}

#[test]
fn the_battery_ram_goes_to_its_file_and_comes_back() {
    let mut m = with_demo();
    let (file, shared) = medium(MediaKind::BatteryRam, Vec::new());
    m.media_slot(SLOT_BATTERY).unwrap().insert(file).unwrap();
    frames(&mut m, 12);
    {
        let saved = shared.0.lock().unwrap();
        assert_eq!((saved.len(), saved[0]), (0x2000, demo::SAVED));
    }
    // Another machine with that file: the RAM is there before the
    // program writes it.
    let mut other = with_demo();
    let mut bytes = vec![0u8; 0x2000];
    bytes[0x1fff] = 0x77;
    let (file, _) = medium(MediaKind::BatteryRam, bytes);
    other
        .media_slot(SLOT_BATTERY)
        .unwrap()
        .insert(file)
        .unwrap();
    frames(&mut other, 12);
    assert_eq!(peek_cpu(&other, 0xbfff), 0x77);
    // A file of the wrong size is refused; without a cartridge too.
    let (file, _) = medium(MediaKind::BatteryRam, vec![0; 100]);
    assert!(m.media_slot(SLOT_BATTERY).unwrap().insert(file).is_err());
    let mut empty = machine(None);
    let (file, _) = medium(MediaKind::BatteryRam, Vec::new());
    assert!(empty
        .media_slot(SLOT_BATTERY)
        .unwrap()
        .insert(file)
        .is_err());
    // A power cycle keeps the battery's contents.
    m.reset(ResetKind::Hard);
    assert_eq!(peek_cpu(&m, 0xa000), 0xff, "the RAM is switched off");
    assert_eq!(m.board.cart.as_ref().unwrap().ram()[0], demo::SAVED);
}

#[test]
fn savestate_round_trip_and_determinism() {
    let mut a = with_demo();
    frames(&mut a, 7);
    let mut dbg = DebugHooks::new();
    dbg.set_instr_step(true);
    for _ in 0..100 {
        a.run_frame(&mut dbg);
    }
    let state = a.save_state();
    let mut b = with_demo();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    frames(&mut a, 20);
    frames(&mut b, 20);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.board.ppu.frame(), b.board.ppu.frame());
    // Corrupt data never panics; another model or no cartridge refuse.
    for cut in [0, 10, state.len() / 2, state.len() - 1] {
        assert!(b.load_state(&state[..cut]).is_err());
    }
    assert!(machine(None).load_state(&state).is_err());
    let mut pocket = machine(Some("pocket"));
    let (media, _) = medium(MediaKind::Cartridge, demo::rom());
    pocket
        .media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(media)
        .unwrap();
    assert!(pocket.load_state(&state).is_err());
}

#[test]
fn models_options_and_an_empty_slot() {
    assert_eq!(machine(Some("pocket")).model(), Model::Pocket);
    assert_eq!(machine(Some("dmg")).model(), Model::Dmg);
    let mut config = MachineConfig {
        model: Some("color".into()),
        ..MachineConfig::default()
    };
    assert!(GameBoy::new(&config).is_err());
    config.model = None;
    config.options.insert("x".into(), "y".into());
    assert!(GameBoy::new(&config).is_err());
    config.options.clear();
    config.roms.insert("gameboy-boot".into(), vec![0; 100]);
    assert!(GameBoy::new(&config).is_err());
    // Without a cartridge the bus reads $FF: RST $38 for ever, and the
    // machine still makes its frames.
    let mut m = machine(None);
    assert_eq!(m.chips()[0].regs[0].value, 0x91);
    frames(&mut m, 3);
    assert_eq!(m.cpu.pc & 0xff00, 0);
    assert_eq!(m.media_slots().len(), 2);
    assert!(m.ports().is_empty());
    assert_eq!(m.memory_spaces().len(), 5);
    // The debugger reads and writes memory without the unit's locks.
    assert!(DebugTarget::poke(
        &mut m,
        VRAM_SPACE,
        0x10,
        Size::Byte,
        0xaa
    ));
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, 0x8010, Size::Byte),
        Some(0xaa)
    );
    assert!(!DebugTarget::poke(&mut m, CPU_SPACE, 0x100, Size::Byte, 0));
    assert_eq!(DebugTarget::peek(&m, ROM_SPACE, 0, Size::Byte), None);
    assert_eq!(m.disassemble(0x8010).to_string(), "XOR D");
}

#[test]
fn a_boot_rom_runs_first_and_switches_itself_off() {
    // LD A,$91 : LDH ($40),A : LD A,1 : LDH ($50),A at $00FA would be the
    // real ROM's end; here: jump there from 0.
    let mut boot = vec![0u8; 256];
    boot[..3].copy_from_slice(&[0xc3, 0xf8, 0x00]);
    boot[0xf8..0x100].copy_from_slice(&[0x3e, 0x91, 0xe0, 0x40, 0x3e, 0x01, 0xe0, 0x50]);
    let mut config = MachineConfig::default();
    config.roms.insert("gameboy-boot".into(), boot);
    let mut m = GameBoy::new(&config).unwrap();
    let (media, _) = medium(MediaKind::Cartridge, demo::rom());
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    m.step_cycle();
    assert!(m.board.boot_on);
    assert_eq!(m.cpu.pc, 0x00f8);
    assert_eq!(peek_cpu(&m, 0x0000), 0xc3);
    for _ in 0..4 {
        m.step_cycle();
    }
    assert!(!m.board.boot_on);
    assert_eq!(m.cpu.pc, 0x0100);
    assert_eq!(peek_cpu(&m, 0x0000), 0xff, "the cartridge's byte");
    frames(&mut m, 12);
    assert_eq!(m.take_serial(), b"OK");
}

#[test]
fn the_sound_unit_plays_in_stereo() {
    let mut m = with_demo();
    frames(&mut m, 12);
    let mut out = Vec::new();
    let format = m.audio(&mut out);
    assert_eq!((format.sample_rate, format.channels), (48_000, 2));
    // 12 frames of 70224 clocks at 48 kHz, two samples each (the first
    // frame begins in the middle of one).
    let pairs = out.len() / 2;
    assert!((9500..=9650).contains(&pairs), "{pairs}");
    out.clear();
    // Channel 2 at full volume to the left only.
    for (addr, v) in [
        (0xff24u16, 0x77u8),
        (0xff25, 0x20),
        (0xff17, 0xf0),
        (0xff18, 0x00),
        (0xff19, 0x87),
    ] {
        m.board.write(addr, v, false);
    }
    // The step of the change dies away in the high pass first.
    frames(&mut m, 12);
    m.audio(&mut out);
    out.clear();
    frames(&mut m, 6);
    m.audio(&mut out);
    let left = out
        .iter()
        .step_by(2)
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap();
    let right = out
        .iter()
        .skip(1)
        .step_by(2)
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap();
    assert!(left > 3000 && right < 200, "{left} {right}");
    assert_eq!(peek_cpu(&m, 0xff26) & 0x82, 0x82);
}
