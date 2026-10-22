use super::*;
use emu_core::input::{Dirs, MachineKey};

fn stub(model: &str) -> Atari8 {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Atari8::new(&config).unwrap()
}

fn frames(m: &mut Atari8, n: usize) {
    let mut dbg = DebugHooks::default();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn byte(m: &Atari8, addr: u16) -> u8 {
    m.peek(CPU_SPACE, u32::from(addr), Size::Byte).unwrap() as u8
}

fn poke(m: &mut Atari8, addr: u16, v: u8) {
    assert!(m.poke(CPU_SPACE, u32::from(addr), Size::Byte, u32::from(v)));
}

#[test]
fn stub_boots_and_counts_fields() {
    for (model, lines) in [("800xl", 312u64), ("800xl-ntsc", 262)] {
        let mut m = stub(model);
        frames(&mut m, 10);
        // RTCLOK ($14) counts VBIs: one per field.
        let count = byte(&m, 0x14);
        assert!((9..=10).contains(&count), "{model}: {count}");
        assert_eq!(m.now().0, 10 * lines * 114, "{model}: cycles per field");
        assert_eq!(m.cpu.regs().get("PC"), Some(u32::from(stub::IDLE)));
        let f = m.framebuffer();
        assert_eq!((f.width, f.height), (384, 240));
        assert_eq!(f.signal.lines_per_field as u64, lines);
    }
}

#[test]
fn antic_dma_slows_the_cpu() {
    // Mode 2 lines steal cycles: the stub's display costs CPU time. With
    // the display off a busy loop runs more often per field.
    let mut on = stub("800xl");
    frames(&mut on, 2);
    let mut off = stub("800xl");
    frames(&mut off, 2);
    poke(&mut off, 0xd400, 0x00);
    // Count instructions over one field.
    let count = |m: &mut Atari8| {
        // Instruction starts: the CPU reaches a boundary (it stays there
        // while halted, so count transitions).
        let (mut n, mut prev) = (0, true);
        let start = m.io.antic.frame();
        while m.io.antic.frame() == start {
            let at = m.cpu.at_instruction_boundary();
            if at && !prev {
                n += 1;
            }
            prev = at;
            m.step_cycle();
        }
        n
    };
    count(&mut on);
    count(&mut off);
    let (a, b) = (count(&mut on), count(&mut off));
    assert!(a < b, "with DMA {a}, without {b}");
}

#[test]
fn text_reaches_the_screen() {
    let mut m = stub("800xl");
    // A solid character 1 in the stub's (RAM-copied) character set: point
    // CHBASE at RAM and put the block there.
    frames(&mut m, 2);
    for r in 0..8 {
        poke(&mut m, 0x3000 + 8 + r, 0xff);
    }
    poke(&mut m, 0xd409, 0x30);
    poke(&mut m, stub::SCREEN, 0x01);
    poke(&mut m, 0xd018, 0x94); // COLPF2
    poke(&mut m, 0xd017, 0x0e); // COLPF1 luma
    frames(&mut m, 2);
    let f = m.framebuffer();
    // First mode 2 line: scan line 32 → row 24; normal playfield from
    // colour clock 48 → x = (48 - 32) * 2.
    let at = |x: u32, y: u32| match f.pixels {
        PixelData::Indexed { pixels, .. } => pixels[(y * f.width + x) as usize],
        _ => unreachable!(),
    };
    assert_eq!(at(32, 24), 0x9e, "set pixel: PF2 hue, PF1 luma");
    assert_eq!(at(32 + 8, 24), 0x94, "background of the text area");
    assert_eq!(at(0, 24), 0x00, "border");
}

#[test]
fn portb_banks_the_roms() {
    let mut m = stub("800xl");
    frames(&mut m, 1);
    m.mem.ram[0xc000] = 0x5a;
    assert_ne!(byte(&m, 0xc000), 0x5a, "OS ROM on after power-up");
    poke(&mut m, 0xd303, 0x30); // PBCTL: DDR
    poke(&mut m, 0xd301, 0xff); // all outputs
    poke(&mut m, 0xd303, 0x34); // PBCTL: port
    poke(&mut m, 0xd301, 0xfe); // OS off
    assert_eq!(byte(&m, 0xc000), 0x5a);
    poke(&mut m, 0xd301, 0xff);
    assert_ne!(byte(&m, 0xc000), 0x5a);
    // RESET resets the PIA: the OS ROM comes back.
    poke(&mut m, 0xd301, 0xfe);
    m.reset(ResetKind::Soft);
    assert_ne!(byte(&m, 0xc000), 0x5a);
}

#[test]
fn keyboard_reaches_pokey() {
    let mut m = stub("800xl");
    frames(&mut m, 1);
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x3f),
            down: true,
        },
    });
    frames(&mut m, 2);
    assert_eq!(byte(&m, 0xd209), 0x3f, "KBCODE A");
    assert_eq!(byte(&m, 0xd20f) & 0x04, 0, "SKSTAT key down");
    let at = m.now();
    for k in [keys::START, keys::OPTION] {
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: k,
                down: true,
            },
        });
    }
    frames(&mut m, 1);
    assert_eq!(byte(&m, 0xd01f) & 0x07, 0x02, "START and OPTION down");
}

#[test]
fn joystick_reaches_pia_and_gtia() {
    let mut m = stub("800xl");
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PORT1),
        event: InputEvent::Digital {
            dirs: Dirs::UP | Dirs::LEFT,
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    poke(&mut m, 0xd302, 0x34); // PACTL: port
    assert_eq!(byte(&m, 0xd300), 0xfa, "up and left on stick 0");
    assert_eq!(byte(&m, 0xd010), 0, "TRIG0 pressed");
    assert_eq!(byte(&m, 0xd011), 1, "TRIG1 up");
    assert!(m.disconnect(PORT1).is_some());
    assert_eq!(byte(&m, 0xd300), 0xff);
    let joy = Box::new(devices_input::Joystick::new(PortKind::AtariJoy));
    assert!(m.connect(PORT2, joy).is_ok());
    let wrong = Box::new(devices_input::Joystick::new(PortKind::CbmControl));
    assert!(matches!(
        m.connect(PORT1, wrong),
        Err(PortError::Incompatible { .. })
    ));
}

#[test]
fn savestate_round_trip() {
    let mut a = stub("800xl-ntsc");
    frames(&mut a, 3);
    for _ in 0..12_345 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = stub("800xl-ntsc");
    b.load_state(&state).unwrap();
    frames(&mut a, 3);
    frames(&mut b, 3);
    assert_eq!(a.now(), b.now());
    assert_eq!(a.cpu.regs(), b.cpu.regs());
    assert_eq!(a.framebuffer().pixels, b.framebuffer().pixels);
    assert_eq!(a.save_state(), b.save_state());
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
    assert!(b.load_state(b"garbage").is_err());
}

#[test]
fn options() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("port2".into(), "joystick".into());
    config.options.insert("basic".into(), "off".into());
    let m = Atari8::new(&config).unwrap();
    assert!(m.ports().iter().all(|p| p.connected));
    assert_eq!(m.io.gtia.peek(0x1f) & 0x04, 0, "OPTION held for boot");
    config.options.insert("port1".into(), "mouse".into());
    assert!(Atari8::new(&config).is_err());
    let mut config = MachineConfig::default();
    assert!(matches!(
        Atari8::new(&config),
        Err(ConfigError::MissingRom(_))
    ));
    config.model = Some("800xl-secam".into());
    assert!(Atari8::new(&config).is_err());
}

#[test]
fn paddles_turn_pokey_pots() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("port2".into(), "paddles".into());
    let mut m = Atari8::new(&config).unwrap();
    let at = m.now();
    for (axis, value) in [(0u8, 0u16), (1, 0x8000)] {
        m.input(TimedInput {
            at,
            target: InputTarget::Port(PORT2),
            event: InputEvent::Analog { axis, value },
        });
    }
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PORT2),
        event: InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    poke(&mut m, 0xd20b, 0); // POTGO
    frames(&mut m, 1);
    assert_eq!(byte(&m, 0xd202), 1, "POT2: turned fully");
    assert_eq!(byte(&m, 0xd203), 114, "POT3: mid travel");
    assert_eq!(byte(&m, 0xd200), 228, "POT0: nothing in port 1's pots");
    poke(&mut m, 0xd302, 0x34);
    assert_eq!(byte(&m, 0xd300), 0xbf, "paddle 2's button: stick 1 left");
}

#[test]
fn drive_slots_and_options() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("drives".into(), "2".into());
    config.options.insert("drive".into(), "810".into());
    let mut m = Atari8::new(&config).unwrap();
    let names: Vec<_> = m.media_slots().iter().map(|s| s.name).collect();
    assert_eq!(names, ["D1:", "D2:", "program", "tape", "cartridge"]);
    assert!(m.media_slot(SlotId(2)).is_none(), "no D3:");
    let disk_as_program = emu_core::media::Media {
        name: "x.atr".into(),
        kind: MediaKind::Disk,
        backend: Box::new(media_backends_free::Bytes(media::atr::blank(720))),
    };
    assert!(m
        .media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(disk_as_program)
        .is_err());
    for (k, v) in [("drives", "5"), ("drive", "1541")] {
        let mut c = config.clone();
        c.options.insert(k.into(), v.into());
        assert!(Atari8::new(&c).is_err(), "{k}={v}");
    }
    // Savestates carry the drive configuration.
    let state = m.save_state();
    config.options.insert("drives".into(), "1".into());
    let mut other = Atari8::new(&config).unwrap();
    assert!(other.load_state(&state).is_err());
}

/// A minimal in-memory backend for tests (media-backends is a frontend
/// crate).
mod media_backends_free {
    use emu_core::media::{MediaBackend, MediaError};

    pub struct Bytes(pub Vec<u8>);

    impl MediaBackend for Bytes {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(MediaError::OutOfRange)?);
            Ok(())
        }
        fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
            Err(MediaError::WriteProtected)
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }
}

#[test]
fn cartridge_maps_banks_and_drives_trig3() {
    let mut m = stub("800xl");
    frames(&mut m, 1);
    assert_eq!(byte(&m, 0xd013), 0, "TRIG3: no cartridge");
    m.mem.ram[0x8000] = 0x11;
    // XEGS 32K: four banks whose bytes name the bank.
    let rom: Vec<u8> = (0..0x8000).map(|i| (i / 0x2000) as u8 + 0xc0).collect();
    let car = media::car::Car { kind: 12, rom };
    let media = emu_core::media::Media {
        name: "x.car".into(),
        kind: MediaKind::Cartridge,
        backend: Box::new(media_backends_free::Bytes(car.to_bytes())),
    };
    m.media_slot(SLOT_CARTRIDGE).unwrap().insert(media).unwrap();
    frames(&mut m, 1); // the change powers the machine up
    assert_eq!(byte(&m, 0xd013), 1, "TRIG3: RD5");
    assert_eq!(byte(&m, 0x8000), 0xc0, "bank 0 at $8000");
    assert_eq!(byte(&m, 0xa000), 0xc3, "last bank at $A000");
    poke(&mut m, 0xd500, 2);
    assert_eq!(byte(&m, 0x8000), 0xc2);
    // State round trip keeps the bank.
    let state = m.save_state();
    poke(&mut m, 0xd500, 1);
    m.load_state(&state).unwrap();
    assert_eq!(byte(&m, 0x8000), 0xc2);
    assert!(m.media_slot(SLOT_CARTRIDGE).unwrap().eject().is_some());
    frames(&mut m, 1);
    assert_eq!(byte(&m, 0xd013), 0);
    assert_eq!(byte(&m, 0xa000), 0x00, "RAM again (cleared by power-up)");
}

#[test]
fn chips_show_both_register_sides() {
    let mut m = stub("800xl");
    frames(&mut m, 1);
    poke(&mut m, 0xd012, 0x46);
    let chips = m.chips();
    let find = |chip: &str, reg: &str| {
        chips
            .iter()
            .find(|c| c.name == chip)
            .and_then(|c| c.regs.iter().find(|r| r.name == reg))
            .map(|r| (r.addr, r.value))
    };
    assert_eq!(find("gtia", "COLPM0"), Some((Some(0xd012), 0x46)));
    assert_eq!(find("gtia", "PAL"), Some((Some(0xd014), 0x01)));
    assert_eq!(find("gtia", "TRIG3"), Some((Some(0xd013), 0)));
    assert_eq!(find("pokey", "POT0"), Some((Some(0xd200), 228)));
    assert_eq!(find("antic", "DMACTL"), Some((Some(0xd400), 0x22)));
    assert!(find("pia", "PORTB").is_some());
}

#[test]
fn models_choose_memory_ports_and_reset_behaviour() {
    let m = stub("65xe");
    assert_eq!(m.model(), Model::A65Xe);
    assert_eq!(m.ports().len(), 2);
    assert!(m.mem.banks_with_portb());
    let m = stub("130xe-ntsc");
    assert_eq!(
        (m.model(), m.standard),
        (Model::A130Xe, VideoStandard::Ntsc)
    );
    assert_eq!(m.mem.extended.len(), 0x1_0000);
    let m = stub("800");
    assert_eq!(m.ports().len(), 4, "four joystick ports");
    assert_eq!(m.mem.kind, Kind::Old { ram: 0xc000 });
    assert_eq!(m.mem.roms.os.len(), OS_AB_SIZE);
    let m = stub("400");
    assert_eq!(m.mem.kind, Kind::Old { ram: 0x4000 });
    let mut config = MachineConfig {
        model: Some("400".into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("ram".into(), "48k".into());
    assert_eq!(
        Atari8::new(&config).unwrap().mem.kind,
        Kind::Old { ram: 0xc000 }
    );
    config.options.insert("ram".into(), "64k".into());
    assert!(Atari8::new(&config).is_err());
    config.options.remove("ram");
    config.options.insert("port4".into(), "joystick".into());
    assert!(Atari8::new(&config).unwrap().ports()[3].connected);
    config.model = Some("800xl".into());
    assert!(
        Atari8::new(&config).is_ok(),
        "port4 is ignored on two-port models"
    );
}

#[test]
fn the_800_runs_the_stub_without_banking_and_resets_through_an_nmi() {
    let mut m = stub("800");
    frames(&mut m, 3);
    assert!(
        byte(&m, 0xd800) != 0 || byte(&m, 0xf000) != 0,
        "OS-B area from the stub"
    );
    // PORTB is joystick ports 3/4: writing it banks nothing.
    poke(&mut m, 0xd303, 0x34); // PBCTL: port
    poke(&mut m, 0xd301, 0x00);
    assert_eq!(
        byte(&m, 0xd301),
        0xff,
        "an input port reads the pulled-up lines"
    );
    assert_eq!(byte(&m, 0xc000), 0xff, "nothing at $C000");
    // Joystick in port 3 pulls PORTB bits 0-3 and TRIG2.
    let at = m.now();
    let joy = Box::new(devices_input::Joystick::new(PortKind::AtariJoy));
    m.connect(PORT3, joy).unwrap();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PORT3),
        event: InputEvent::Digital {
            dirs: Dirs::DOWN,
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    assert_eq!(byte(&m, 0xd301), 0xfd, "down on stick 3");
    assert_eq!(byte(&m, 0xd012), 0, "TRIG2 pressed");
    assert_eq!(byte(&m, 0xd013), 1, "TRIG3 is port 4, not the cartridge");
    // The RESET key is an NMI: NMIST bit 5 appears within the line (the
    // stub's handler clears NMIST again at the next vertical blank).
    let fields = m.io.antic.frame();
    m.key(keys::RESET.0, true);
    let mut seen = false;
    for _ in 0..2000 {
        m.step_cycle();
        if byte(&m, 0xd40f) & 0x20 != 0 {
            seen = true;
            break;
        }
    }
    assert!(seen, "NMIST RNMI");
    frames(&mut m, 2);
    assert!(m.io.antic.frame() > fields, "the machine kept running");
    assert_eq!(
        m.mem.portb & 0x0f,
        0xfd & 0x0f,
        "no system reset: PORTB inputs still read the stick"
    );
}

#[test]
fn the_130xe_switches_banks_for_cpu_and_antic() {
    let mut m = stub("130xe");
    frames(&mut m, 1);
    poke(&mut m, 0xd303, 0x30); // PBCTL: DDR
    poke(&mut m, 0xd301, 0xff); // all outputs
    poke(&mut m, 0xd303, 0x34); // PBCTL: port
    poke(&mut m, 0xd301, 0xff);
    poke(&mut m, 0x4000, 0x11);
    // CPU bank 1 ($E7 = bits 2-3: 01, bit 4 clear), ANTIC main RAM.
    poke(&mut m, 0xd301, 0xe7);
    poke(&mut m, 0x4000, 0x22);
    assert_eq!(byte(&m, 0x4000), 0x22);
    assert_eq!(m.mem.read_dma(0x4000), 0x11);
    assert_eq!(m.mem.extended[0x4000], 0x22, "bank 1");
    // ANTIC bank 1, CPU main RAM.
    poke(&mut m, 0xd301, 0xd7);
    assert_eq!(byte(&m, 0x4000), 0x11);
    assert_eq!(m.mem.read_dma(0x4000), 0x22);
    // A savestate round trip keeps the extended RAM and refuses the XL.
    let s = m.save_state();
    let mut n = stub("130xe");
    n.load_state(&s).unwrap();
    assert_eq!(n.mem.extended[0x4000], 0x22);
    let mut xl = stub("800xl");
    assert!(xl.load_state(&s).is_err());
}
