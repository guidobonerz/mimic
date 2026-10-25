use super::*;
use emu_core::input::Dirs;
use emu_core::media::{Media, MediaBackend, MediaError};

fn stub_with(options: &[(&str, &str)]) -> Vectrex {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Vectrex::new(&config).unwrap()
}

fn frames(m: &mut Vectrex, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn push(m: &mut Vectrex, target: InputTarget, event: InputEvent) {
    let at = m.now();
    m.input(TimedInput { at, target, event });
}

#[test]
fn a_frame_is_30000_cycles() {
    let mut m = stub_with(&[]);
    frames(&mut m, 1);
    let a = m.now().0;
    frames(&mut m, 5);
    let per = (m.now().0 - a) as f64 / 5.0;
    assert!((per - 30_000.0).abs() < 20.0, "{per}");
    // Frontends pace by the picture's signal metadata: 50 Hz.
    let s = m.framebuffer().signal;
    let hz = f64::from(s.pixel_clock_hz) / f64::from(s.total_pixels * s.lines_per_field);
    assert_eq!(hz, 50.0);
}

#[test]
fn stub_draws_a_dot_and_a_square() {
    let mut m = stub_with(&[]);
    frames(&mut m, 3);
    let v = m.vectors().unwrap();
    assert_eq!(v.width, render::WIDTH * UNIT as u32);
    let (cx, cy) = (HALF_W, HALF_H);
    let dot = v
        .segments
        .iter()
        .find(|s| s.x0 == s.x1 && s.y0 == s.y1)
        .expect("a dot");
    assert_eq!((dot.x0, dot.y0), (cx, cy));
    assert!(dot.cycles >= 20, "{dot:?}");
    assert_eq!(dot.intensity, 0xfe);
    let lines: Vec<_> = v
        .segments
        .iter()
        .filter(|s| s.cycles > 30 && s != &dot)
        .collect();
    assert_eq!(lines.len(), 4, "{:?}", v.segments);
    // About $30 units a cycle for $40 cycles; up, right, down, left.
    let len = 0x30 * 0x41;
    let d: Vec<(i32, i32)> = lines.iter().map(|s| (s.x1 - s.x0, s.y1 - s.y0)).collect();
    for ((dx, dy), (ex, ey)) in d.iter().zip([(0, -1), (1, 0), (0, 1), (-1, 0)]) {
        assert!((dx - ex * len).abs() <= 0x60, "{d:?}");
        assert!((dy - ey * len).abs() <= 0x60, "{d:?}");
    }
    assert_eq!((lines[0].x0, lines[0].y0), (cx, cy), "starts at the centre");
    // The raster picture shows them.
    let f = m.framebuffer();
    let px = |x: i32, y: i32| f.rgb_at((x / UNIT) as u32, (y / UNIT) as u32).unwrap();
    assert!(px(cx, cy).r > 150, "dot");
    assert!(px(cx, cy - len / 2).r > 100, "left edge of the square");
    assert_eq!(px(cx + len / 2, cy - len / 2).r, 0, "inside the square");
}

#[test]
fn comparator_finds_the_stick() {
    let mut m = stub_with(&[]);
    frames(&mut m, 1);
    let cmp = |m: &Vectrex, dac: u8, sel: u8| {
        let mut b = m.board.clone();
        b.via.write(1, dac);
        b.via.write(0, 0x80 | (sel << 1));
        b.via_pins(&m.ports).1 & 0x20 != 0
    };
    assert!(!cmp(&m, 0x10, 0), "DAC above the centred stick: low");
    assert!(cmp(&m, 0xf0, 0), "DAC below it: high");
    push(
        &mut m,
        InputTarget::Port(PORT1),
        InputEvent::Digital {
            dirs: Dirs::RIGHT | Dirs::DOWN,
            buttons: 0,
        },
    );
    frames(&mut m, 1);
    assert!(cmp(&m, 0x7e, 0), "stick right: the X pot is at the top");
    assert!(!cmp(&m, 0x81, 1), "stick down: the Y pot is at the bottom");
    assert!(cmp(&m, 0xf0, 2), "controller 2 centred");
}

#[test]
fn buttons_reach_the_ay_port_and_ca1() {
    let mut m = stub_with(&[]);
    frames(&mut m, 1);
    push(
        &mut m,
        InputTarget::Port(PORT2),
        InputEvent::Digital {
            dirs: Dirs::NONE,
            buttons: 0b1000,
        },
    );
    push(
        &mut m,
        InputTarget::Keyboard,
        InputEvent::Key {
            machine_key: MachineKey(keys::PORT1 + 1),
            down: true,
        },
    );
    frames(&mut m, 1);
    // Button 4 of controller 2 pulls CA1 low (falling edge, PCR bit 0 = 0).
    let mut b = m.board.clone();
    b.via.write(0xc, 0x00);
    b.via.set_ca1(true, 0xff);
    b.ca1 = true;
    b.tick(&mut m.ports);
    assert_ne!(b.via.ifr & chips_cbm::via::ifr::CA1, 0, "button 4 on CA1");
    assert!(!b.ca1);
    // Latch AY register 14 (BDIR + BC1), then read it (BC1).
    b.via.write(3, 0xff);
    b.via.write(1, 14);
    b.via.write(0, 0x99);
    b.tick(&mut m.ports);
    b.via.write(0, 0x89);
    b.via.write(3, 0x00);
    b.tick(&mut m.ports);
    let (pa, _) = b.via_pins(&m.ports);
    assert_eq!(pa, !0x82, "controller 1 button 2, controller 2 button 4");
}

#[test]
fn light_pen_sees_the_lit_beam() {
    let mut m = stub_with(&[("port2", "lightpen")]);
    frames(&mut m, 1);
    let mut b = m.board.clone();
    b.analog.lit = true;
    b.analog.x = 0;
    b.analog.y = 0;
    let (cx, cy) = (HALF_W / UNIT, HALF_H / UNIT);
    assert!(b.pen_sees_beam([cx as u16 + 1, cy as u16]));
    assert!(!b.pen_sees_beam([cx as u16 + 10, cy as u16]));
    b.analog.lit = false;
    assert!(!b.pen_sees_beam([cx as u16, cy as u16]));
    assert!(Vectrex::new(&MachineConfig {
        options: [
            ("roms".into(), "stub".into()),
            ("port1".into(), "lightpen".into())
        ]
        .into(),
        ..MachineConfig::default()
    })
    .is_err());
}

#[test]
fn savestate_round_trip() {
    let mut m = stub_with(&[]);
    frames(&mut m, 2);
    // Stop mid-frame.
    for _ in 0..500 {
        m.step_cycle();
    }
    let s = m.save_state();
    frames(&mut m, 2);
    let a: Vec<Segment> = m.vectors().unwrap().segments.to_vec();
    let pa = m.framebuffer().pixels;
    let pa = match pa {
        PixelData::Rgb(pixels) => pixels.to_vec(),
        _ => unreachable!(),
    };
    let mut n = stub_with(&[]);
    n.load_state(&s).unwrap();
    frames(&mut n, 2);
    assert_eq!(n.vectors().unwrap().segments, a.as_slice());
    match n.framebuffer().pixels {
        PixelData::Rgb(pixels) => assert_eq!(pixels, pa.as_slice()),
        _ => unreachable!(),
    }
    assert!(n.load_state(&s[..s.len() / 2]).is_err());
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

#[test]
fn cartridge_appears_at_0_and_power_cycles_on_insert() {
    let mut m = stub_with(&[]);
    frames(&mut m, 1);
    assert_eq!(m.peek_cpu(0), 0xff);
    let mut rom = vec![0u8; 0x2000];
    rom[0] = b'g';
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "test.vec".into(),
            backend: Box::new(MemBackend(rom)),
        })
        .unwrap();
    frames(&mut m, 1);
    assert_eq!(m.peek_cpu(0), b'g');
    assert_eq!(m.peek_cpu(0x4000), b'g', "8 KB repeats");
    assert!(m.media_slots()[0].inserted);
}

#[test]
fn a_4k_system_rom_fills_both_halves() {
    let mut rom = vec![0x12u8; 0x1000];
    rom[0xffe] = 0xf0;
    rom[0xfff] = 0x00;
    let mut config = MachineConfig::default();
    config.roms.insert("vectrex".into(), rom);
    let m = Vectrex::new(&config).unwrap();
    assert_eq!(m.cpu().pc, 0xf000);
    assert_eq!(m.peek_cpu(0xe000), 0x12);
    let mut config = MachineConfig::default();
    config.roms.insert("vectrex".into(), vec![0; 100]);
    assert!(Vectrex::new(&config).is_err());
    assert!(Vectrex::new(&MachineConfig::default()).is_err());
}

#[test]
fn disassembly_and_debug_peek() {
    let mut m = stub_with(&[]);
    let i = m.disassemble(0xf000);
    assert_eq!(i.to_string(), "LDS #$CBEA");
    assert!(m.poke(CPU_SPACE, 0xc880, Size::Word, 0x1234));
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, 0xc880, Size::Word),
        Some(0x1234)
    );
    assert_eq!(m.peek_cpu(0xcc80), 0x12, "RAM repeats");
}

#[test]
fn imager_spins_signals_ca1_and_colours_by_eye() {
    // Port A of the sound chip stays an input: pin 2 floats high, the
    // motor runs; the index pulse's rising edge sets the CA1 flag.
    let mut m = stub_with(&[("port2", "imager"), ("eye", "both")]);
    frames(&mut m, 100);
    let mut b = m.board.clone();
    b.via.write(0xc, 0x01); // CA1 on the rising edge
    b.via.write(0xd, 0x7f);
    let mut ports = std::mem::take(&mut m.ports);
    for _ in 0..60_000 {
        b.tick(&mut ports);
    }
    assert_ne!(b.via.ifr & chips_cbm::via::ifr::CA1, 0, "index pulse");
    m.ports = ports;
    // Both eyes: the figure in the filters' colours.
    let colours: Vec<Rgb8> = m
        .vectors()
        .unwrap()
        .segments
        .iter()
        .map(|s| s.color)
        .collect();
    assert!(!colours.is_empty());
    assert!(colours
        .iter()
        .all(|c| [FILTER_RED, FILTER_GREEN, FILTER_BLUE].contains(c)));
    // One eye: about half the beam time is hidden.
    let mut left = stub_with(&[("port2", "imager")]);
    frames(&mut left, 100);
    let time =
        |m: &Vectrex| -> u32 { m.vectors().unwrap().segments.iter().map(|s| s.cycles).sum() };
    let (one, both) = (time(&left), time(&m));
    assert!(one < both * 3 / 4, "{one} of {both}");
    assert!(Vectrex::new(&MachineConfig {
        options: [
            ("roms".into(), "stub".into()),
            ("eye".into(), "middle".into())
        ]
        .into(),
        ..MachineConfig::default()
    })
    .is_err());
}

/// Development aid: run the system ROM (EMU_ROM_DIR/vectrex/*.vec or .bin)
/// with a cartridge (EMU_CART) for EMU_FRAMES frames and print where the
/// CPU spends the next frame.
#[test]
#[ignore]
fn trace_cart() {
    let (Some(dir), Some(cart)) = (
        std::env::var_os("EMU_ROM_DIR"),
        std::env::var_os("EMU_CART"),
    ) else {
        return;
    };
    let rom_path = std::fs::read_dir(std::path::Path::new(&dir).join("vectrex"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            std::fs::metadata(p)
                .map(|m| m.len())
                .is_ok_and(|l| l == 0x2000 || l == 0x1000)
        })
        .unwrap();
    let mut config = MachineConfig::default();
    config
        .roms
        .insert("vectrex".into(), std::fs::read(rom_path).unwrap());
    let mut m = Vectrex::new(&config).unwrap();
    m.media_slot(SLOT_CARTRIDGE)
        .unwrap()
        .insert(Media {
            kind: MediaKind::Cartridge,
            name: "cart.vec".into(),
            backend: Box::new(MemBackend(std::fs::read(cart).unwrap())),
        })
        .unwrap();
    let n: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    if std::env::var_os("EMU_DDRB").is_some() {
        let mut last = m.board.via.ddrb;
        let end = m.now().0 + n as u64 * FRAME_CYCLES;
        while m.now().0 < end {
            let pc = m.cpu().pc;
            m.step_cycle();
            if m.board.via.ddrb != last {
                last = m.board.via.ddrb;
                println!(
                    "DDRB {last:02x} at {} pc {pc:04x} {}",
                    m.now().0,
                    m.disassemble(u32::from(pc))
                );
            }
        }
        return;
    }
    frames(&mut m, n);
    if let Ok(dir) = std::env::var("EMU_JOY") {
        let d = match dir.as_str() {
            "up" => Dirs::UP,
            "down" => Dirs::DOWN,
            "left" => Dirs::LEFT,
            _ => Dirs::RIGHT,
        };
        let ram = |m: &Vectrex| (0xc885..0xc88c).map(|a| m.peek_cpu(a)).collect::<Vec<u8>>();
        println!("before {:02x?}", ram(&m));
        push(
            &mut m,
            InputTarget::Port(PORT1),
            InputEvent::Digital {
                dirs: d,
                buttons: 0,
            },
        );
        for _ in 0..30 {
            frames(&mut m, 1);
            if std::env::var_os("EMU_JOY_EACH").is_some() {
                println!(
                    "  joy {:02x} {:02x} c885 {:02x} c887 {:02x} {:02x} ddrb {:02x}",
                    m.peek_cpu(0xc81b),
                    m.peek_cpu(0xc81c),
                    m.peek_cpu(0xc885),
                    m.peek_cpu(0xc887),
                    m.peek_cpu(0xc888),
                    m.board.via.ddrb
                );
            }
        }
        println!(
            "after  {:02x?} (joy {:02x?})",
            ram(&m),
            [m.peek_cpu(0xc81b), m.peek_cpu(0xc81c)]
        );
        push(
            &mut m,
            InputTarget::Port(PORT1),
            InputEvent::Digital {
                dirs: Dirs(0),
                buttons: 0,
            },
        );
        return;
    }
    if std::env::var_os("EMU_ANALOG").is_some() {
        let mut last = String::new();
        for _ in 0..FRAME_CYCLES {
            let pc = m.cpu().pc;
            m.step_cycle();
            let a = m.analog();
            let pb = m.board.via.port_b_out();
            let line = format!(
                "pa {:4} pb {:02x} ysh {:4} zref {:4} z {:4} x {:6} y {:6} ca2 {} acr {:02x}",
                m.board.via.port_a_out() as i8,
                pb,
                a.ysh,
                a.zero_ref,
                a.z,
                a.x,
                a.y,
                m.board.via.ca2_out(),
                m.board.via.acr
            );
            if line != last {
                println!("A {:6} {pc:04x} {line}", m.now().0);
                last = line;
            }
        }
        return;
    }
    let mut hist = std::collections::BTreeMap::<u16, u32>::new();
    let end = m.now().0 + FRAME_CYCLES;
    while m.now().0 < end {
        *hist.entry(m.cpu().pc).or_default() += 1;
        m.step_cycle();
    }
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (pc, n) in v.iter().take(20) {
        println!("{pc:04x} {n:6} {}", m.disassemble(u32::from(*pc)));
    }
    if let Ok(spec) = std::env::var("EMU_DIS") {
        let (a, k) = spec.split_once(',').unwrap();
        let mut a = u32::from_str_radix(a, 16).unwrap();
        for _ in 0..k.parse::<usize>().unwrap() {
            let i = m.disassemble(a);
            println!("D {a:04x} {i}");
            a += i.bytes.len().max(1) as u32;
        }
    }
    println!("regs {:?}", m.regs());
}
