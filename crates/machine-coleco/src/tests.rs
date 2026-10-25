use super::*;
use emu_core::input::Dirs;
use emu_core::media::{Media, MediaBackend, MediaError};
use emu_core::overlay::{colors, Shape};

fn stub_with(model: Option<&str>, options: &[(&str, &str)]) -> Coleco {
    let mut config = MachineConfig {
        model: model.map(str::to_string),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Coleco::new(&config).unwrap()
}

fn frames(m: &mut Coleco, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn word(m: &Coleco, addr: u16) -> u16 {
    u16::from(peek_cpu(m, addr)) | u16::from(peek_cpu(m, addr + 1)) << 8
}

/// Palette index at active-area pixel (x, y).
fn pixel(m: &Coleco, x: u32, y: u32) -> u8 {
    let v = vdp(m);
    let row = y + u32::from(v.model.top());
    v.frame()[(row * vdp::WIDTH + 13 + x) as usize]
}

fn push(m: &mut Coleco, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

#[test]
fn stub_bios_shows_its_screen_and_counts_frames_by_nmi() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 10);
    let n = word(&m, stub::FRAMES);
    assert!((7..=10).contains(&n), "{n} NMIs in 10 fields");
    // Stripes of the box pattern (yellow on dark blue) and the sprite.
    assert_eq!(pixel(&m, 8, 0), 10, "box top row: yellow");
    assert_eq!(pixel(&m, 9, 1), 4, "inside the box: dark blue");
    assert_eq!(pixel(&m, 0, 0), 4, "pattern 0 shows the background");
    assert_eq!(pixel(&m, 0x78, 0x61), 15, "the white sprite at (120, 97)");
}

#[test]
fn a_field_takes_262_or_313_lines() {
    for (model, lines) in [(None, 262u64), (Some("pal"), 313)] {
        let mut m = stub_with(model, &[]);
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
fn the_joystick_moves_the_sprite_and_fire_sounds() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 3);
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
    // Port 2 is not read by the stub: its joystick changes nothing.
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0,
        },
    );
    frames(&mut m, 2);
    let x2 = peek_cpu(&m, stub::SPRITE_X);
    frames(&mut m, 5);
    assert_eq!(peek_cpu(&m, stub::SPRITE_X), x2, "stopped");
}

#[test]
fn controller_bytes_follow_the_segments() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    push(
        &mut m,
        InputTarget::Port(PORT2),
        InputEvent::Digital {
            dirs: Dirs::UP,
            buttons: 2,
        },
    );
    push(
        &mut m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::PORT2_KEYPAD + 10),
            down: true,
        },
    );
    frames(&mut m, 1);
    let read = |m: &mut Coleco, port: u8| {
        let mut bus = CvBus {
            b: &mut m.board,
            ports: &mut m.ports,
            start: 0,
            dbg: None,
            beam: BeamPos::default(),
        };
        bus.read(IO_BASE + u32::from(port), Size::Byte) as u8
    };
    let write = |m: &mut Coleco, port: u8| {
        let mut bus = CvBus {
            b: &mut m.board,
            ports: &mut m.ports,
            start: 0,
            dbg: None,
            beam: BeamPos::default(),
        };
        bus.write(IO_BASE + u32::from(port), Size::Byte, 0);
    };
    write(&mut m, 0xc0);
    assert_eq!(read(&mut m, 0xff), 0xfe, "port 2 joystick: up");
    write(&mut m, 0x80);
    // Keypad "0" ($A) and right fire (bit 6 low).
    assert_eq!(read(&mut m, 0xff), 0x80 | 0x30 | 0x0a);
    assert_eq!(read(&mut m, 0xfc), 0xff, "port 1: nothing");
}

#[test]
fn spinner_steps_raise_the_maskable_interrupt() {
    let mut m = stub_with(None, &[("port1", "superaction")]);
    frames(&mut m, 2);
    for _ in 0..10 {
        push(
            &mut m,
            InputTarget::Port(PORT1),
            InputEvent::MouseRel {
                dx: 0,
                dy: 8,
                buttons: 0,
            },
        );
        frames(&mut m, 1);
    }
    let spins = peek_cpu(&m, stub::SPINS);
    assert!(spins >= 10, "{spins} spinner interrupts for 40 steps");
    // The Roller Controller's Y axis reaches port 2.
    let mut m = stub_with(None, &[("port1", "roller"), ("port2", "none")]);
    frames(&mut m, 2);
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::MouseRel {
            dx: 0,
            dy: 4,
            buttons: 0,
        },
    );
    frames(&mut m, 1);
    assert!(peek_cpu(&m, stub::SPINS) >= 1, "the Y axis interrupts too");
}

struct MemBackend(Vec<u8>);

impl MediaBackend for MemBackend {
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

/// A cartridge with the "skip the title screen" header whose code fills
/// the screen's backdrop with colour 9 and counts at $7100.
fn cart() -> Vec<u8> {
    let mut rom = vec![0u8; 0x2000];
    rom[0] = 0x55;
    rom[1] = 0xaa;
    rom[0x0a] = 0x30;
    rom[0x0b] = 0x80;
    let code = [
        0x3e, 0x40, 0xd3, 0xbf, 0x3e, 0x81, 0xd3, 0xbf, // R1 = $40: display on
        0x3e, 0x09, 0xd3, 0xbf, 0x3e, 0x87, 0xd3, 0xbf, // R7 = 9
        0x21, 0x00, 0x71, // LD HL,$7100
        0x34, // loop: INC (HL)
        0x18, 0xfd, // JR loop
    ];
    rom[0x30..0x30 + code.len()].copy_from_slice(&code);
    rom
}

#[test]
fn a_cartridge_starts_through_its_header_and_power_cycles_on_insert() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    let slot = m.media_slot(SLOT_CARTRIDGE).unwrap();
    slot.insert(Media {
        kind: MediaKind::Cartridge,
        name: "test.col".into(),
        backend: Box::new(MemBackend(cart())),
    })
    .unwrap();
    frames(&mut m, 3);
    assert_eq!(pixel(&m, 100, 100), 9, "the backdrop the cartridge set");
    assert!(peek_cpu(&m, 0x7100) > 0, "the cartridge's loop runs");
    assert_eq!(peek_cpu(&m, 0xa000), 0x55, "8 KB repeats");
    assert!(m.media_slots()[0].inserted);
}

#[test]
fn savestate_round_trip_and_ports() {
    let mut m = stub_with(Some("pal"), &[("port2", "driving")]);
    frames(&mut m, 3);
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::LEFT,
            buttons: 0,
        },
    );
    frames(&mut m, 2);
    let state = m.save_state();
    let mut other = stub_with(Some("pal"), &[("port2", "driving")]);
    other.load_state(&state).unwrap();
    frames(&mut m, 5);
    frames(&mut other, 5);
    assert_eq!(m.framebuffer().pixels, other.framebuffer().pixels);
    assert_eq!(m.now(), other.now());
    assert_eq!(word(&m, stub::FRAMES), word(&other, stub::FRAMES));
    let mut ntsc = stub_with(None, &[("port2", "driving")]);
    assert!(ntsc.load_state(&state).is_err(), "model differs");
    let mut no_port = stub_with(Some("pal"), &[("port2", "none")]);
    assert!(no_port.load_state(&state).is_err(), "controllers differ");
    assert!(other.load_state(&state[..state.len() - 3]).is_err());
    // Ports: kinds, connect and disconnect.
    let ports = m.ports();
    assert_eq!(ports.len(), 2);
    assert!(ports.iter().all(|p| p.kind == PortKind::ColecoControl));
    let dev = m.disconnect(PORT2).unwrap();
    assert!(!m.ports()[1].connected);
    m.connect(PORT2, dev).unwrap();
    assert!(m
        .connect(
            PORT2,
            Box::new(ColecoController::new(ColecoKind::Standard, 1))
        )
        .is_err());
    let bad = Box::new(devices_input::Joystick::new(PortKind::AtariJoy));
    assert!(matches!(
        m.connect(PORT1, bad),
        Err(PortError::Incompatible { .. } | PortError::Occupied(_))
    ));
}

#[test]
fn debugger_sees_memory_vram_registers_and_disassembly() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    let regs = {
        let t = m.debug_target(CPU).unwrap();
        assert_eq!(t.peek(CPU_SPACE, 0x0000, Size::Byte), Some(0xf3), "DI");
        assert_eq!(
            t.peek(VRAM_SPACE, 0x0008, Size::Byte),
            Some(0xff),
            "box top"
        );
        assert!(t.poke(CPU_SPACE, 0x7200, Size::Byte, 0x42));
        assert!(!t.poke(CPU_SPACE, 0x0000, Size::Byte, 0x42), "ROM");
        assert_eq!(t.disassemble(0x0000).to_string(), "DI");
        t.regs()
    };
    assert!(regs.get("PC").is_some());
    assert_eq!(peek_cpu(&m, 0x6200), 0x42, "RAM repeats");
    let chips = m.chips();
    let vdp_chip = chips.iter().find(|c| c.name == "vdp").unwrap();
    assert_eq!(vdp_chip.regs[1].value, 0xe0, "R1 as the stub set it");
    assert_eq!(m.controls().len(), 2);
    assert_eq!(m.media_slots().len(), 1);
}

#[test]
fn options_and_models_are_checked() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("port1".into(), "lightgun".into());
    assert!(Coleco::new(&config).is_err());
    config.options.remove("port1");
    config.model = Some("secam".into());
    assert!(Coleco::new(&config).is_err());
    let config = MachineConfig::default();
    assert!(
        matches!(Coleco::new(&config), Err(ConfigError::MissingRom(r)) if r == "coleco"),
        "the BIOS is needed without roms=stub"
    );
}

#[test]
fn gfx_views_show_the_vdp_tables() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["tiles", "sprites", "screen", "palette"]);
    let state = m.save_state();
    for id in ids {
        let v = m.gfx_view(id, &GfxOptions::new()).unwrap();
        assert_eq!(v.space, VRAM_SPACE);
        assert!(v.width > 0 && v.height > 0);
        assert!(!v.raw.is_empty());
        if id == "screen" {
            let names = v.param("names").unwrap();
            assert_eq!(v.addr, names);
            assert_eq!(v.raw[0], m.board.vdp.vram[names as usize]);
        }
    }
    assert_eq!(m.save_state(), state, "views leave the machine as it was");
    assert!(m.gfx_view("bitmap", &GfxOptions::new()).is_none());
}

#[test]
fn overlays_record_sprites_only_while_switched_on() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["sprites", "limit"]);
    // Sprites 1–5 at Y = 49 (lines 50–57) from X = 100: five on the line
    // (the stub BIOS moves sprite 0).
    let v = &mut m.board.vdp;
    let attrs = usize::from(v.regs[5] & 0x7f) << 7;
    for n in 1..6 {
        v.vram[attrs + n * 4..attrs + n * 4 + 4].copy_from_slice(&[49, 91 + n as u8 * 9, 0, 15]);
    }
    v.vram[attrs + 24] = 0xd0;
    let state = m.save_state();
    frames(&mut m, 1);
    assert!(m.overlay("sprites").is_none(), "switched off");
    m.set_overlays(&["sprites", "limit"]);
    frames(&mut m, 2);
    let o = m.overlay("sprites").unwrap();
    let big = m.board.vdp.regs[1] & 2 != 0;
    let mag = m.board.vdp.regs[1] & 1 != 0;
    let size = if big { 16 } else { 8 } * if mag { 2 } else { 1 };
    let top = m.frame_row(50).unwrap() as i32;
    assert_eq!(top, 50 + 27);
    assert!(o.shapes.contains(&Shape::Rect {
        x: 13 + 100,
        y: top,
        w: size,
        h: size,
        color: colors::SERIES[1],
        label: Some("1".into()),
    }));
    assert_eq!(m.overlay("limit").unwrap().shapes.len(), 1);
    assert_eq!(m.frame_row(192 + 24), None);
    // The record is not machine state.
    let mut fresh = stub_with(None, &[]);
    fresh.load_state(&state).unwrap();
    assert_eq!(m.save_state().len(), fresh.save_state().len());
    m.set_overlays(&[]);
    assert!(m.overlay("limit").is_none());
}

#[test]
fn beam_hpos_is_the_frame_column() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 2);
    // Sprite 1 (solid pattern 1, white) at X = 100, lines 50–57.
    let v = &mut m.board.vdp;
    let attrs = usize::from(v.regs[5] & 0x7f) << 7;
    let pats = usize::from(v.regs[6] & 7) << 11;
    v.vram[attrs + 4..attrs + 8].copy_from_slice(&[49, 100, 1, 15]);
    v.vram[attrs + 8] = 0xd0;
    v.vram[pats + 8..pats + 40].fill(0xff);
    frames(&mut m, 1);
    let row = m.frame_row(50).unwrap();
    let f = m.framebuffer();
    assert_eq!(f.rgb_at(113, row), Some(vdp::PALETTE[15]));
    assert_ne!(f.rgb_at(112, row), Some(vdp::PALETTE[15]));
    // The pixel clock that shows sprite pixel X = 100 reports column 113.
    m.board.vdp.dot = 13 + 100;
    assert_eq!(m.beam().unwrap().hpos, 113);
    m.board.vdp.dot = 330;
    assert_eq!(
        u32::from(m.beam().unwrap().hpos),
        vdp::WIDTH - 1,
        "blanking clamps"
    );
}
