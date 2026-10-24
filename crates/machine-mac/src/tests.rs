//! Machine tests with the stub ROM.

use super::*;
use emu_core::input::MachineKey;

fn mac(model: &str) -> Mac {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Mac::new(&config).unwrap()
}

fn frames(m: &mut Mac, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// Black pixels in a screen line.
fn black(m: &Mac, line: u32) -> usize {
    let w = video::WIDTH as usize;
    let l = line as usize;
    m.board.screen.pixels[l * w..(l + 1) * w]
        .iter()
        .filter(|&&p| p == 1)
        .count()
}

#[test]
fn overlay_maps_the_rom_at_zero_until_the_via_turns_it_off() {
    let m = mac("plus");
    assert!(m.board.overlay());
    assert_eq!(m.board.peek(4, Size::Long), 0x0040_0100);
    assert_eq!(m.cpu.pc(), 0x40_0100);
    let mut m = m;
    frames(&mut m, 2);
    assert!(!m.board.overlay());
    assert_eq!(m.board.peek(0x40_0004, Size::Long), 0x0040_0100);
}

#[test]
fn stub_rom_draws_its_text_on_every_model() {
    for model in ["128k", "512k", "plus", "se"] {
        let mut m = mac(model);
        frames(&mut m, 3);
        let text: usize = (stub::TEXT_LINE..stub::TEXT_LINE + 7)
            .map(|l| black(&m, l))
            .sum();
        assert!(text > 100, "{model}: {text} black pixels in the text");
        assert_eq!(black(&m, 200), 0, "{model}: the screen is cleared");
        // The text is in RAM at the screen buffer, as the video read it.
        let base = m.screen_base() as usize;
        let row = base + stub::TEXT_LINE as usize * 64 + stub::TEXT_BYTE as usize;
        assert_ne!(m.ram()[row], 0, "{model}");
    }
}

#[test]
fn mouse_button_reaches_the_via() {
    let mut m = mac("plus");
    frames(&mut m, 3);
    assert_eq!(black(&m, stub::BLOCK_LINE), 0);
    let now = m.now();
    m.input(TimedInput {
        at: now,
        target: InputTarget::Port(MOUSE_PORT),
        event: InputEvent::MouseRel {
            dx: 0,
            dy: 0,
            buttons: 1,
        },
    });
    frames(&mut m, 2);
    assert_eq!(black(&m, stub::BLOCK_LINE), 32, "the block is inverted");
}

#[test]
fn mouse_motion_raises_scc_interrupts() {
    let mut m = mac("plus");
    frames(&mut m, 2);
    // Enable DCD interrupts on channel A as the ROM does.
    let scc = m.scc();
    for (reg, v) in [(15u8, 0x08u8), (1, 0x01), (9, 0x08)] {
        scc.write(0, false, reg);
        scc.write(0, false, v);
    }
    let now = m.now();
    m.input(TimedInput {
        at: now,
        target: InputTarget::Port(MOUSE_PORT),
        event: InputEvent::MouseRel {
            dx: 4,
            dy: 0,
            buttons: 0,
        },
    });
    frames(&mut m, 1);
    assert!(m.board.scc.irq(), "X1 changed: external/status interrupt");
    assert_eq!(m.board.ipl() & 2, 2);
}

#[test]
fn vertical_blanking_flags_ca1_once_a_frame() {
    let mut m = mac("plus");
    frames(&mut m, 2);
    m.board.via.write(0xd, 0x7f);
    let start = m.board.clock;
    // Run to just past the start of vertical blanking.
    let (line, h) = m.board.line_pos();
    let to_vbl = (u64::from(video::HEIGHT) * video::LINE_CLOCKS)
        .wrapping_sub(line * video::LINE_CLOCKS + h)
        % video::FRAME_CLOCKS;
    m.board.run_to(start + to_vbl + 1, &mut None);
    assert_ne!(m.board.via.ifr & chips_cbm::via::ifr::CA1, 0);
}

#[test]
fn keyboard_and_adb_take_keys() {
    let mut m = mac("plus");
    m.input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x00),
            down: true,
        },
    });
    frames(&mut m, 1);
    assert_ne!(m.board.kbd, Some(Keyboard::new(true)), "A is queued");
    let mut se = mac("se");
    se.input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x00),
            down: true,
        },
    });
    frames(&mut se, 1);
    assert!(
        !se.board.adb.as_ref().unwrap().int_line(),
        "service request"
    );
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = mac("plus");
    frames(&mut a, 3);
    let state = a.save_state();
    frames(&mut a, 3);
    let mut b = mac("plus");
    b.load_state(&state).unwrap();
    frames(&mut b, 3);
    assert_eq!(a.board.screen.pixels, b.board.screen.pixels);
    assert_eq!(a.board.clock, b.board.clock);
    assert_eq!(a.cpu.pc(), b.cpu.pc());
    assert!(mac("se").load_state(&state).is_err());
    assert!(b.load_state(&state[..state.len() / 2]).is_err());
}

#[test]
fn media_slots_per_model() {
    let names = |m: &Mac| -> Vec<&str> { m.media_slots().iter().map(|s| s.name).collect() };
    assert_eq!(names(&mac("128k")), ["internal", "external"]);
    let plus = mac("plus");
    assert_eq!(names(&plus).len(), 2 + scsi::TARGETS);
    assert_eq!(
        names(&mac("se"))[..3],
        ["internal", "internal 2", "external"]
    );
    let mut m = mac("128k");
    assert!(m.media_slot(SlotId(SLOT_SCSI.0)).is_none());
}

#[test]
fn rom_chip_pairs_interleave() {
    let mut config = MachineConfig {
        model: Some("128k".into()),
        ..MachineConfig::default()
    };
    config.roms.insert("mac128k-h".into(), vec![0x11; 0x8000]);
    config.roms.insert("mac128k-l".into(), vec![0x22; 0x8000]);
    let m = Mac::new(&config).unwrap();
    assert_eq!(m.board.rom[..4], [0x11, 0x22, 0x11, 0x22]);
    config.roms.clear();
    assert!(matches!(
        Mac::new(&config),
        Err(ConfigError::MissingRom(r)) if r == "mac128k"
    ));
}

/// Development aid: boot a real ROM (EMU_ROM_DIR, EMU_MAC_MODEL,
/// EMU_FRAMES) and print where the CPU spends the last frame.
#[test]
#[ignore]
fn trace_rom() {
    let Some(dir) = std::env::var_os("EMU_ROM_DIR") else {
        return;
    };
    let model = std::env::var("EMU_MAC_MODEL").unwrap_or_else(|_| "plus".into());
    let frames_n: usize = std::env::var("EMU_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let role = match model.as_str() {
        "128k" => "mac128k",
        "512k" => "mac512k",
        "se" => "macse",
        _ => "macplus",
    };
    let path = std::path::Path::new(&dir)
        .join("mac")
        .join(format!("{role}.rom"));
    let mut config = MachineConfig {
        model: Some(model.clone()),
        ..MachineConfig::default()
    };
    config
        .roms
        .insert(role.into(), std::fs::read(path).unwrap());
    if let Ok(ram) = std::env::var("EMU_RAM") {
        config.options.insert("ram".into(), ram);
    }
    let mut m = Mac::new(&config).unwrap();
    if let Ok(spec) = std::env::var("EMU_DIS") {
        let (a, n) = spec.split_once(',').unwrap();
        let mut a = u32::from_str_radix(a, 16).unwrap();
        for _ in 0..n.parse::<usize>().unwrap() {
            let i = m.disassemble(a);
            println!("D {a:06x} {} {}", i.mnemonic, i.operand);
            a += i.bytes.len().max(2) as u32;
        }
        return;
    }
    if let Some(disk) = std::env::var_os("EMU_DISK") {
        let data = std::fs::read(disk).unwrap();
        let slots: Vec<SlotId> = m.media_slots().iter().map(|s| s.id).collect();
        let ok = slots.into_iter().any(|slot| {
            m.media_slot(slot)
                .unwrap()
                .insert(emu_core::media::Media {
                    name: "disk".into(),
                    kind: MediaKind::Disk,
                    backend: Box::new(Mem(data.clone())),
                })
                .is_ok()
        });
        assert!(ok, "no drive takes the disk");
    }
    if let Ok(vec) = std::env::var("EMU_RING_VECTOR") {
        // Run until the CPU enters exception vector `vec`'s handler and
        // show the instructions before it.
        let vec: u32 = vec.parse().unwrap();
        let mut ring = std::collections::VecDeque::new();
        let end = m.board.clock + frames_n as u64 * video::FRAME_CLOCKS;
        while m.board.clock < end {
            let handler = m.board.peek(vec * 4, Size::Long) & 0xff_ffff;
            let pc = m.cpu.pc();
            if pc == handler && m.board.clock > 1000 {
                break;
            }
            ring.push_back((m.board.clock, pc, m.regs()));
            if ring.len() > 60 {
                ring.pop_front();
            }
            m.step_cycle();
        }
        for (t, pc, r) in &ring {
            let i = m.disassemble(*pc);
            let d: Vec<String> =
                r.0.iter()
                    .take(16)
                    .map(|x| format!("{:x}", x.value))
                    .collect();
            println!(
                "R {t} {pc:06x} {} {} | {}",
                i.mnemonic,
                i.operand,
                d.join(" ")
            );
        }
        return;
    }
    if std::env::var_os("EMU_MOUSE_TEST").is_some() {
        frames(&mut m, frames_n);
        let w = |m: &Mac, a: usize| i16::from_be_bytes([m.board.ram[a], m.board.ram[a + 1]]);
        let show = |m: &Mac, what: &str| {
            println!(
                "{what}: Mouse v{} h{}  RawMouse v{} h{}  MTemp v{} h{}",
                w(m, 0x830),
                w(m, 0x832),
                w(m, 0x82c),
                w(m, 0x82e),
                w(m, 0x828),
                w(m, 0x82a)
            );
        };
        show(&m, "before");
        for _ in 0..30 {
            let now = m.now();
            m.input(TimedInput {
                at: now,
                target: InputTarget::Port(MOUSE_PORT),
                event: InputEvent::MouseRel {
                    dx: std::env::var("EMU_DX")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(5),
                    dy: std::env::var("EMU_DY")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(3),
                    buttons: 0,
                },
            });
            frames(&mut m, 1);
        }
        show(&m, "after +150/+90");
        println!(
            "scc wr15 a {:02x} b {:02x} wr1 a {:02x} b {:02x} wr9 {:02x} ip {:02x}",
            m.board.scc.ch[0].wr[15],
            m.board.scc.ch[1].wr[15],
            m.board.scc.ch[0].wr[1],
            m.board.scc.ch[1].wr[1],
            m.board.scc.ch[0].wr[9],
            m.board.scc.pending()
        );
        return;
    }
    if std::env::var_os("EMU_WATCH_DRIVE").is_some() {
        let end = m.board.clock + frames_n as u64 * video::FRAME_CLOCKS;
        let mut last = (255u8, false, false);
        while m.board.clock < end {
            let d = &m.board.iwm.drives[0];
            let now = (d.track, m.board.iwm.sel, d.motor);
            if now != last {
                println!(
                    "DRV {} track {} sel {} motor {} pc {:06x}",
                    m.board.clock,
                    now.0,
                    now.1,
                    now.2,
                    m.cpu.pc()
                );
                last = now;
            }
            m.step_cycle();
        }
        return;
    }
    if std::env::var_os("EMU_SOUND_FRAMES").is_some() {
        for f in 0..frames_n {
            frames(&mut m, 1);
            let top = m.board.ram.len() - 0x300;
            let hi: Vec<u8> = (0..370).map(|i| m.board.ram[top + 2 * i]).collect();
            let pa = m.board.via.port_a_out();
            let pb = m.board.via.port_b_out();
            println!(
                "F {f} min {} max {} vol {} on {} first {:?}",
                hi.iter().min().unwrap(),
                hi.iter().max().unwrap(),
                pa & 7,
                pb & 0x80 == 0,
                &hi[..40]
            );
        }
        return;
    }
    if std::env::var_os("EMU_WATCH_SOUND").is_some() {
        // Print the instructions that change the sound buffer's low bytes.
        let top = m.board.ram.len() - 0x300;
        let end = m.board.clock + frames_n as u64 * video::FRAME_CLOCKS;
        let mut seen = std::collections::BTreeSet::new();
        while m.board.clock < end {
            let before: Vec<u8> = (0..370).map(|i| m.board.ram[top + 2 * i + 1]).collect();
            let pc = m.cpu.pc();
            m.step_cycle();
            let changed = (0..370).any(|i| m.board.ram[top + 2 * i + 1] != before[i]);
            if changed && seen.insert(pc) {
                let i = m.disassemble(pc);
                let d: Vec<String> = m
                    .regs()
                    .0
                    .iter()
                    .take(16)
                    .map(|x| format!("{}={:x}", x.name, x.value))
                    .collect();
                println!("W {pc:06x} {} {} | {}", i.mnemonic, i.operand, d.join(" "));
            }
        }
        return;
    }
    if let Ok(bp) = std::env::var("EMU_BREAK") {
        let bp = u32::from_str_radix(&bp, 16).unwrap();
        let end = m.board.clock + frames_n as u64 * video::FRAME_CLOCKS;
        let mut hits = 0;
        while m.board.clock < end && hits < 2000 {
            if m.cpu.pc() == bp {
                hits += 1;
                let d: Vec<String> = m
                    .regs()
                    .0
                    .iter()
                    .take(16)
                    .map(|x| format!("{}={:x}", x.name, x.value))
                    .collect();
                println!("B {} {}", m.board.clock, d.join(" "));
                if std::env::var_os("EMU_DUMP_SOUND").is_some() {
                    let top = m.board.ram.len() - 0x300;
                    let low: Vec<u8> = (0..370)
                        .map(|i| m.board.ram[top + 2 * i + 1] & 0x3f)
                        .collect();
                    println!("S {:?} avg {}", low, m.board.pwm_avg);
                }
            }
            m.step_cycle();
        }
        return;
    }
    frames(&mut m, frames_n);
    let needle = [0x20u8, 0x78, 0x08, 0x24, 0x30, 0x3c];
    for (i, w) in m.board.ram.windows(6).enumerate() {
        if w == needle {
            println!("boot code found in RAM at {i:06x}");
        }
    }
    println!("{}", b_iwm(&m));
    let w = |a: usize| u16::from(m.board.ram[a]) << 8 | u16::from(m.board.ram[a + 1]);
    let l = |a: usize| u32::from(w(a)) << 16 | u32::from(w(a + 2));
    println!(
        "DSErrCode {} ScrnBase {:06x} MemTop {:06x} SysZone {:06x} ApplZone {:06x}",
        w(0xaf0) as i16,
        l(0x824),
        l(0x108),
        l(0x2a6),
        l(0x2aa)
    );
    if let Some(at) = std::env::var("EMU_TRACE_AT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        while m.board.clock < at {
            m.step_cycle();
        }
        let n: usize = std::env::var("EMU_TRACE_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        for k in 0..n {
            if k % 20000 == 0 {
                let v = &m.board.via;
                println!(
                    "V {} t2 {:04x} ifr {:02x} ier {:02x} ipl {} sr {:04x}",
                    m.board.clock,
                    v.t2_counter,
                    v.ifr,
                    v.ier,
                    m.board.ipl(),
                    m.cpu.sr()
                );
            }
            let pc = m.cpu.pc();
            let i = m.disassemble(pc);
            println!("T {} {pc:06x} {} {}", m.board.clock, i.mnemonic, i.operand);
            m.step_cycle();
        }
    }
    if let Ok(spec) = std::env::var("EMU_DIS_AFTER") {
        let (a, n) = spec.split_once(',').unwrap();
        let mut a = u32::from_str_radix(a, 16).unwrap();
        for _ in 0..n.parse::<usize>().unwrap() {
            let i = m.disassemble(a);
            println!("D {a:06x} {} {}", i.mnemonic, i.operand);
            a += i.bytes.len().max(2) as u32;
        }
    }
    let mut hist = std::collections::BTreeMap::<u32, u32>::new();
    let end = m.board.clock + video::FRAME_CLOCKS;
    while m.board.clock < end {
        *hist.entry(m.cpu.pc()).or_default() += 1;
        m.step_cycle();
    }
    println!(
        "iwm {:?} sel {} mode {:02x}",
        b_iwm(&m),
        m.board.iwm.sel,
        m.board.iwm.mode
    );
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (pc, n) in v.iter().take(24) {
        let i = m.disassemble(*pc);
        println!("{pc:06x} {n:6} {} {}", i.mnemonic, i.operand);
    }
    println!("regs {:?}", m.regs());
    let b = &m.board;
    println!(
        "via ifr {:02x} ier {:02x} acr {:02x} pcr {:02x} orb {:02x} ddrb {:02x} ora {:02x} ddra {:02x} sr {:02x}",
        b.via.ifr, b.via.ier, b.via.acr, b.via.pcr, b.via.orb, b.via.ddrb, b.via.ora, b.via.ddra, b.via.sr
    );
}

/// An image in memory.
pub(crate) struct Mem(pub Vec<u8>);

impl emu_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), emu_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), emu_core::media::MediaError> {
        let o = offset as usize;
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), emu_core::media::MediaError> {
        Ok(())
    }
}

fn b_iwm(m: &Mac) -> String {
    let i = &m.board.iwm;
    format!("lines {:02x} drives {:?}", i.lines, i.drives)
}
