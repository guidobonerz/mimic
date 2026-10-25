use super::*;
use emu_core::frame::Rgb8;

fn stub() -> Amiga {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    Amiga::new(&config).unwrap()
}

fn frames(m: &mut Amiga, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn pixel(m: &Amiga, x: u32, y: u32) -> Rgb8 {
    m.framebuffer().rgb_at(x, y).unwrap()
}

#[test]
fn stub_boots_runs_the_copper_and_counts_vertical_blanks() {
    let mut m = stub();
    assert_eq!(m.cpu.pc(), 0xf8_0008, "reset vector through the overlay");
    frames(&mut m, 5);
    assert!(!m.board.mem.overlay, "OVL cleared through CIA-A");
    let red = Rgb8 { r: 255, g: 0, b: 0 };
    let green = Rgb8 { r: 0, g: 255, b: 0 };
    assert_eq!(pixel(&m, 360, 0x70 - 26), red);
    assert_eq!(pixel(&m, 360, 0x90 - 26), green);
    let count = peek_cpu(&m, 0x100, Size::Long);
    assert!((3..=5).contains(&count), "VERTB interrupts: {count}");
}

#[test]
fn a_field_takes_313_lines_of_227_colour_clocks() {
    let mut m = stub();
    frames(&mut m, 1);
    let t0 = m.now().0;
    frames(&mut m, 1);
    let t = m.now().0 - t0;
    let field = 2 * 313 * 227;
    assert!(t.abs_diff(field) < 100, "{t} clocks per field");
}

#[test]
fn memory_map_and_mirrors() {
    let mut m = stub();
    frames(&mut m, 1);
    let b = &mut m.board;
    b.write(0x1234, Size::Word, 0xbeef);
    assert_eq!(
        b.read(0x8_1234, Size::Word, true),
        0xbeef,
        "chip RAM mirror"
    );
    b.write(0xc0_0010, Size::Long, 0x1234_5678);
    assert_eq!(b.read(0xc0_0012, Size::Word, true), 0x5678, "slow RAM");
    assert_eq!(b.read(0xf8_0000, Size::Long, true), 0x0008_0000, "ROM");
    assert_eq!(
        b.read(0xfc_0000, Size::Long, true),
        0x0008_0000,
        "256 KB ROM repeats"
    );
    // Custom registers: a byte write reaches the whole word.
    b.write(0xdf_f180, Size::Byte, 0x0f);
    assert_eq!(b.custom.denise.color12(0), 0x0f0f & 0xfff);
    // CIA-B timer A low byte at $BFD400, CIA-A's at $BFE401.
    b.write(0xbf_d400, Size::Byte, 0x12);
    b.write(0xbf_e401, Size::Byte, 0x34);
    assert_eq!(b.cia_b.ta.latch & 0xff, 0x12);
    assert_eq!(b.cia_a.ta.latch & 0xff, 0x34);
    assert_eq!(
        b.read(0xe8_0000, Size::Word, true),
        0,
        "no autoconfig board"
    );
}

#[test]
fn chip_bus_accesses_wait_for_dma() {
    let mut m = stub();
    frames(&mut m, 1);
    let b = &mut m.board;
    // A free bus: the CPU's slot is the next colour clock.
    let start = 2 * b.cck;
    let mut bus = AmigaBus {
        b,
        start,
        bus_start: start,
        dbg: None,
        beam: BeamPos::default(),
    };
    // Find a refresh slot ($01): an access aiming at it waits two clocks.
    while bus.b.custom.h != 0 {
        bus.b.tick(false);
    }
    bus.start = 2 * bus.b.cck;
    assert_eq!(bus.bus_cycle(Access::Read, 5, 0x1000, Size::Word, 0), 2);
    // ROM never waits.
    let t = (2 * bus.b.cck - bus.start) as u32;
    assert_eq!(bus.bus_cycle(Access::Read, 6, 0xf8_0000, Size::Word, t), 0);
}

#[test]
fn e_clock_waits_align_to_ten_clocks() {
    for at in (0..40u64).step_by(2) {
        let end = at + 4 + u64::from(e_clock_waits(at, 2, 4));
        assert_eq!(end % 10, 0);
        assert!((at + 10..=at + 20).contains(&end), "{at} → {end}");
    }
    // The A1200: the E clock keeps its 7 MHz tenth, twenty 14 MHz clocks.
    for at in (0..80u64).step_by(3) {
        let end = at + 3 + u64::from(e_clock_waits(at, 4, 3));
        assert_eq!(end % 20, 0);
        assert!((at + 20..=at + 40).contains(&end), "{at} → {end}");
    }
}

#[test]
fn floppy_lines_reach_the_drive() {
    let mut m = stub();
    let b = &mut m.board;
    // Port B first: DDRB = $FF would drive the latch's zeros (a step).
    b.write(0xbf_d100, Size::Byte, 0x7f & !0x08); // motor on, DF0 selected
    b.write(0xbf_d300, Size::Byte, 0xff); // DDRB all outputs
    assert!(b.drives[0].motor && b.drives[0].selected);
    // /RDY, /TK0, /CHNG low at CIA-A port A.
    assert_eq!(b.read(0xbf_e001, Size::Byte, true) as u8 & 0x3c, 0x08);
}

#[test]
fn joystick_in_port_1() {
    use emu_core::input::Dirs;
    let mut m = stub();
    m.input(TimedInput {
        at: Cycle(0),
        target: InputTarget::Port(PORT1),
        event: InputEvent::Digital {
            dirs: Dirs(Dirs::RIGHT.0 | Dirs::DOWN.0),
            buttons: 1,
        },
    });
    frames(&mut m, 1);
    assert_eq!(m.board.custom.peek(chips_amiga::reg::JOY1DAT), 0x0002);
    assert_eq!(m.board.read(0xbf_e001, Size::Byte, true) as u8 & 0x80, 0);
}

#[test]
fn savestate_round_trip() {
    let mut m = stub();
    frames(&mut m, 2);
    for _ in 0..500 {
        m.step_cycle();
    }
    let s = m.save_state();
    let mut n = stub();
    n.load_state(&s).unwrap();
    frames(&mut m, 2);
    frames(&mut n, 2);
    assert_eq!(m.now(), n.now());
    assert_eq!(m.cpu.regs(), n.cpu.regs());
    assert_eq!(m.board.custom.frame(), n.board.custom.frame());
    assert!(n.load_state(&s[..s.len() - 3]).is_err());
    assert_eq!(n.now(), m.now(), "a failed load leaves the machine");
}

#[test]
fn debugger_sees_registers_memory_and_disassembly() {
    let mut m = stub();
    let t = m.debug_target(CPU).unwrap();
    assert_eq!(t.regs().get("PC"), Some(0xf8_0008));
    assert_eq!(t.disassemble(0xf8_0008).to_string(), "MOVE.B #$3,$BFE201.L");
    frames(&mut m, 1); // overlay off
    let t = m.debug_target(CPU).unwrap();
    assert!(t.poke(CHIP_SPACE, 0x200, Size::Word, 0x1234));
    assert_eq!(t.peek(CPU_SPACE, 0x200, Size::Word), Some(0x1234));
    assert!(!t.poke(CPU_SPACE, 0xf8_0000, Size::Word, 0));
    let names: Vec<&str> = m.chips().iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        ["agnus", "copper", "blitter", "denise", "paula", "cia_a", "cia_b", "keyboard", "floppy"]
    );
}

mod disk {
    use super::*;
    use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
    use media::adf;
    use std::sync::{Arc, Mutex};

    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl MediaBackend for Shared {
        fn len(&self) -> u64 {
            self.0.lock().unwrap().len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let d = self.0.lock().unwrap();
            buf.copy_from_slice(&d[offset as usize..offset as usize + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let mut d = self.0.lock().unwrap();
            d[offset as usize..offset as usize + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn image() -> Vec<u8> {
        (0..adf::ADF_SIZE)
            .map(|i| (i / 512 + i % 251) as u8)
            .collect()
    }

    /// The stub machine with `bytes` in DF0:, its motor on and selected,
    /// DSKSYNC $4489, WORDSYNC and FAST, disk DMA enabled.
    fn machine(bytes: Arc<Mutex<Vec<u8>>>) -> Amiga {
        let mut m = stub();
        frames(&mut m, 1);
        let media = Media {
            name: "test.adf".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Shared(bytes)),
        };
        m.media_slot(SlotId(0)).unwrap().insert(media).unwrap();
        let b = &mut m.board;
        b.write(0xbf_d100, Size::Byte, 0x7f & !0x08);
        b.write(0xbf_d300, Size::Byte, 0xff);
        for (r, v) in [
            (0x07e, 0x4489),
            (0x09e, 0x8000 | 0x1000 | 0x0400 | 0x0100),
            (0x096, 0x8210),
        ] {
            b.write(0xdf_f000 + r, Size::Word, v);
        }
        m
    }

    /// Start a transfer and run until DSKBLK.
    fn transfer(m: &mut Amiga, addr: u32, dsklen: u16) {
        let b = &mut m.board;
        b.write(0xdf_f09c, Size::Word, 0x0002);
        b.write(0xdf_f020, Size::Long, addr);
        b.write(0xdf_f024, Size::Word, u32::from(dsklen));
        b.write(0xdf_f024, Size::Word, u32::from(dsklen));
        for _ in 0..100 {
            frames(m, 1);
            if m.board.custom.paula.requests() & 0x0002 != 0 {
                return;
            }
        }
        panic!("no DSKBLK");
    }

    #[test]
    fn dma_reads_a_whole_track_after_the_sync() {
        let img = image();
        let mut m = machine(Arc::new(Mutex::new(img.clone())));
        transfer(&mut m, 0x2_0000, 0x8000 | 0x1900);
        let chip = &m.board.mem.chip;
        let mut buf = vec![0x44, 0x89];
        buf.extend((0..0x1900).flat_map(|i| chip.word(0x2_0000 + 2 * i).to_be_bytes()));
        assert_eq!(
            &buf[..4],
            &[0x44, 0x89, 0x44, 0x89],
            "transfer starts after the first sync"
        );
        let sectors = adf::decode_track(&buf);
        assert_eq!(sectors.len(), adf::SECTORS);
        let mut out = vec![0; adf::ADF_SIZE];
        adf::store_track(&mut out, 0, &sectors);
        assert_eq!(out[..adf::SECTORS * 512], img[..adf::SECTORS * 512]);
        assert_ne!(m.board.custom.paula.requests() & 0x1000, 0, "DSKSYN");
    }

    #[test]
    fn dma_writes_reach_the_image() {
        let bytes = Arc::new(Mutex::new(vec![0; adf::ADF_SIZE]));
        let mut m = machine(bytes.clone());
        let mut img = vec![0u8; adf::ADF_SIZE];
        img[512..1024].fill(0x3c);
        let track = adf::encode_track(&img, 0);
        let words = track.len() / 2;
        for (i, w) in track.chunks(2).enumerate() {
            m.board
                .mem
                .chip
                .set_word(0x2_0000 + 2 * i as u32, u16::from_be_bytes([w[0], w[1]]));
        }
        transfer(&mut m, 0x2_0000, 0xc000 | words as u16);
        m.media_slot(SlotId(0)).unwrap().flush().unwrap();
        let b = bytes.lock().unwrap();
        assert!(b[512..1024].iter().all(|&v| v == 0x3c));
        assert!(b[..512].iter().all(|&v| v == 0));
    }

    #[test]
    fn index_pulses_reach_cia_b_flag() {
        let mut m = machine(Arc::new(Mutex::new(image())));
        m.board.cia_b.read(0x0d, 0xff, 0xff);
        frames(&mut m, 12); // more than one revolution (200 ms)
        assert_ne!(m.board.cia_b.peek(0x0d, 0xff, 0xff) & 0x10, 0);
    }

    #[test]
    fn disks_survive_savestates_and_resets() {
        let mut m = machine(Arc::new(Mutex::new(image())));
        let s = m.save_state();
        m.load_state(&s).unwrap();
        assert!(m.media_slots()[0].inserted);
        m.reset(ResetKind::Hard);
        assert!(m.media_slots()[0].inserted);
        assert!(m.media_slot(SlotId(0)).unwrap().eject().is_some());
        assert!(!m.media_slots()[0].inserted);
    }
}

#[test]
fn mouse_motion_counts_into_joy0dat() {
    let mut m = stub();
    frames(&mut m, 1);
    m.input(TimedInput {
        at: m.now(),
        target: InputTarget::Port(PORT0),
        event: InputEvent::MouseRel {
            dx: 10,
            dy: -3,
            buttons: 3,
        },
    });
    frames(&mut m, 2);
    let joy0 = m.board.custom.peek(chips_amiga::reg::JOY0DAT);
    assert_eq!((joy0 & 0xff, joy0 >> 8), (10, 0xfd));
    // Left button on CIA-A /FIR0, right button on POTGOR DATLY.
    assert_eq!(m.board.read(0xbf_e001, Size::Byte, true) as u8 & 0x40, 0);
    assert_eq!(m.board.custom.peek(chips_amiga::reg::POTGOR) & 0x0400, 0);
}

#[test]
fn keys_arrive_at_cia_a() {
    use emu_core::input::MachineKey;
    let mut m = stub();
    frames(&mut m, 12); // past the power-up stream ($FD, $FE): no handshake
                        // Handshake the power-up codes away by hand.
    for _ in 0..2 {
        m.board.write(0xbf_ee01, Size::Byte, 0x40);
        frames(&mut m, 1);
        m.board.write(0xbf_ee01, Size::Byte, 0x00);
        frames(&mut m, 1);
    }
    m.board.cia_a.read(0x0d, 0xff, 0xff);
    m.input(TimedInput {
        at: m.now(),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(0x20),
            down: true,
        },
    });
    frames(&mut m, 2);
    assert_ne!(
        m.board.cia_a.peek(0x0d, 0xff, 0xff) & 0x08,
        0,
        "SP interrupt"
    );
    let sdr = !m.board.cia_a.peek(0x0c, 0xff, 0xff);
    assert_eq!(sdr.rotate_right(1), 0x20, "A pressed");
}

#[test]
fn interlace_alternates_long_and_short_fields() {
    let mut m = stub();
    frames(&mut m, 2);
    m.board.write(0xdf_f100, Size::Word, 0x0204); // BPLCON0: COLOR, LACE
    let mut seen = Vec::new();
    for _ in 0..4 {
        let t0 = m.now().0;
        frames(&mut m, 1);
        // A frame ends at an instruction boundary, within half a line of
        // the field end: round to whole lines.
        let lines = (m.now().0 - t0 + 227) / (2 * 227);
        seen.push((lines, m.framebuffer().signal.field));
    }
    use emu_core::frame::FieldParity::{Even, Odd};
    let pattern: Vec<_> = seen.iter().map(|&(_, f)| f).collect();
    assert!(
        pattern == [Odd, Even, Odd, Even] || pattern == [Even, Odd, Even, Odd],
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|&(l, _)| l == 312) && seen.iter().any(|&(l, _)| l == 313),
        "{seen:?}"
    );
}

mod debugger {
    use super::*;
    use emu_core::debug::BreakpointKind;
    use emu_core::freeze::{Freeze, FreezeMode};

    /// Hooks configured the way the debug session does it.
    fn hooks(m: &Amiga) -> DebugHooks {
        let mut dbg = DebugHooks::new();
        for s in m.memory_spaces() {
            dbg.set_big_endian(s.id, s.big_endian);
        }
        dbg
    }

    #[test]
    fn copper_and_blitter_are_debug_targets() {
        let mut m = stub();
        frames(&mut m, 2);
        assert_eq!(m.devices(), [CPU, COPPER, BLITTER]);
        let t = m.debug_target(COPPER).unwrap();
        assert_eq!(t.regs().get("COP1LC"), Some(0x1000));
        assert_eq!(t.disassemble(0x1000).to_string(), "MOVE #$0F00,COLOR00");
        assert_eq!(t.disassemble(0x1004).to_string(), "WAIT $80,$00");
        assert_eq!(t.peek(COPPER_SPACE, 0x1002, Size::Word), Some(0x0f00));
        t.set_reg("COP2LC", 0x2000);
        assert_eq!(m.board.custom.copper.cop2lc, 0x2000);
        let t = m.debug_target(BLITTER).unwrap();
        t.set_reg("BLTDPT", 0x1_2346);
        t.set_reg("BLTCON0", 0x01f0);
        assert_eq!(m.board.custom.blitter.pt[3], 0x1_2346);
        assert_eq!(m.board.custom.blitter.con0, 0x01f0);
        let t = m.debug_target(BLITTER).unwrap();
        assert_eq!(t.regs().get("BLTDPT"), Some(0x1_2346));
        assert_eq!(
            m.debug_target(CPU)
                .unwrap()
                .regs()
                .get("PC")
                .map(|pc| pc >> 16),
            Some(0xf8)
        );
    }

    #[test]
    fn copper_exec_breakpoint_stops_at_the_instruction() {
        let mut m = stub();
        frames(&mut m, 2);
        let mut dbg = hooks(&m);
        let id = dbg.add_breakpoint(BreakpointKind::Exec {
            space: COPPER_SPACE,
            addr: 0x1008,
        });
        let mut stop = StopReason::FrameDone;
        for _ in 0..3 {
            stop = m.run_frame(&mut dbg);
            if stop != StopReason::FrameDone {
                break;
            }
        }
        assert_eq!(stop, StopReason::Breakpoint(id));
        // The Copper is past the WAIT at line $80, at the second MOVE.
        assert!(
            m.board.custom.v >= 0x80 && m.board.custom.v < 0x82,
            "line {}",
            m.board.custom.v
        );
    }

    #[test]
    fn long_freezes_in_chip_ram_are_big_endian() {
        let mut m = stub();
        frames(&mut m, 2);
        // The stub's interrupt handler counts fields with ADDQ.L #1,$100.
        let mut dbg = hooks(&m);
        let mut f = Freeze::new(CHIP_SPACE, 0x100, Size::Long, 0x0001_0002);
        f.mode = FreezeMode::ReapplyPerFrame;
        let id = dbg.add_freeze(f);
        for _ in 0..3 {
            m.run_frame(&mut dbg);
        }
        let v = peek_cpu(&m, 0x100, Size::Long);
        assert!((0x0001_0002..=0x0001_0003).contains(&v), "{v:08x}");
        dbg.remove_freeze(id);
        // Blocked writes: the counter stops; CPU reads see the frozen value.
        let before = peek_cpu(&m, 0x100, Size::Long);
        let mut f = Freeze::new(CHIP_SPACE, 0x100, Size::Long, 0x0000_0042);
        f.mode = FreezeMode::BlockWrites;
        dbg.add_freeze(f);
        for _ in 0..3 {
            m.run_frame(&mut dbg);
        }
        assert_eq!(peek_cpu(&m, 0x100, Size::Long), before, "writes blocked");
        let mut bus = AmigaBus {
            b: &mut m.board,
            start: 0,
            bus_start: 0,
            dbg: Some(&mut dbg),
            beam: BeamPos::default(),
        };
        assert_eq!(bus.read(0x100, Size::Word), 0x0000, "high word");
        assert_eq!(bus.read(0x102, Size::Word), 0x0042, "low word");
        assert_eq!(bus.read(0x8_0102, Size::Word), 0x0042, "through the mirror");
    }
}

fn stub_with(model: Option<&str>, options: &[(&str, &str)]) -> Amiga {
    let mut config = MachineConfig {
        model: model.map(str::to_string),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    Amiga::new(&config).unwrap()
}

#[test]
fn models_fit_their_memory() {
    let m = stub_with(None, &[]);
    assert_eq!((m.model(), m.standard()), (Model::A500, Standard::Pal));
    assert_eq!(m.board.mem.chip.len_bytes(), 0x8_0000);
    assert_eq!(m.board.mem.slow.len() * 2, 0x8_0000);
    let m = stub_with(Some("1000-ntsc"), &[]);
    assert_eq!((m.model(), m.standard()), (Model::A1000, Standard::Ntsc));
    assert_eq!(m.board.mem.chip.len_bytes(), 0x4_0000);
    assert!(m.board.mem.slow.is_empty());
    assert!(m.board.mem.a1000.as_ref().is_some_and(|w| w.protected));
    let m = stub_with(Some("2000"), &[]);
    assert_eq!(m.model(), Model::A2000);
    assert_eq!(m.board.mem.chip.len_bytes(), 0x10_0000);
    assert_eq!(m.memory_spaces()[1].addr_bits, 20);
    let m = stub_with(
        Some("500"),
        &[("chip", "1m"), ("slow", "0"), ("drivetype", "hd")],
    );
    assert_eq!(m.board.mem.chip.len_bytes(), 0x10_0000);
    assert!(m.board.drives[0].hd);
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("fast".into(), "3m".into());
    assert!(Amiga::new(&config).is_err(), "bad size");
    config.options.insert("fast".into(), "2m".into());
    config.model = Some("3000".into());
    assert!(Amiga::new(&config).is_err(), "unknown model");
}

#[test]
fn the_500plus_has_the_ecs_chips_and_two_megabytes_of_chip_ram() {
    let m = stub_with(Some("500plus"), &[]);
    assert_eq!(m.model(), Model::A500Plus);
    assert_eq!(m.board.custom.chipset, Chipset::ECS);
    assert_eq!(m.board.mem.chip.len_bytes(), 0x10_0000);
    assert!(m.board.mem.slow.is_empty());
    assert_eq!(m.board.custom.peek(0x004) & 0x7f00, 0x2000, "8372A/8375 id");
    assert_eq!(m.board.custom.peek(0x07c), 0xfffc, "8373");
    let mut m = stub_with(Some("500plus"), &[("chip", "2m")]);
    assert_eq!(m.board.mem.chip.len_bytes(), 0x20_0000);
    assert_eq!(m.memory_spaces()[1].addr_bits, 21);
    frames(&mut m, 2);
    let b = &mut m.board;
    b.write(0x1f_fffc, Size::Word, 0xbeef);
    assert_eq!(b.read(0x1f_fffc, Size::Word, true), 0xbeef, "2 MB");
    assert_eq!(b.read(0x0f_fffc, Size::Word, true), 0, "no mirror");
    assert_eq!(b.custom.ptr_mask, 0x1f_fffe);
    // An OCS A500 with the ECS Agnus only, and an OCS Denise.
    let m = stub_with(None, &[("agnus", "ecs")]);
    assert_eq!(
        m.board.custom.chipset,
        Chipset {
            ecs_agnus: true,
            ecs_denise: false,
            aga: false,
        }
    );
    assert_eq!(m.board.custom.peek(0x07c), 0xffff);
    let m = stub_with(Some("2000"), &[("chipset", "ecs"), ("denise", "ocs")]);
    assert!(m.board.custom.chipset.ecs_agnus && !m.board.custom.chipset.ecs_denise);
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("chip".into(), "2m".into());
    assert!(Amiga::new(&config).is_err(), "2 MB needs the ECS Agnus");
    config.options.insert("chipset".into(), "aaa".into());
    assert!(Amiga::new(&config).is_err(), "unknown chipset");
}

#[test]
fn the_1200_has_a_68020_the_aga_chips_and_gayle() {
    let mut m = stub_with(Some("1200"), &[]);
    assert_eq!(m.model(), Model::A1200);
    assert_eq!(m.cpu.model, cpu_m68k::Model::Mc68020);
    assert_eq!(m.board.custom.chipset, Chipset::AGA);
    assert_eq!(m.board.mem.chip.len_bytes(), 0x20_0000);
    assert!(m.board.mem.slow.is_empty());
    assert_eq!(m.board.custom.peek(0x004) & 0x7f00, 0x2200, "Alice");
    assert_eq!(m.board.custom.peek(0x07c), 0xfff8, "Lisa");
    assert_eq!(m.board.clk, 4);
    frames(&mut m, 3);
    assert!(
        peek_cpu(&m, 0x100, Size::Long) >= 2,
        "the stub runs on the 68020"
    );
    // Gayle's id, bit by bit (D7), most significant bit first: $D0.
    let b = &mut m.board;
    b.write(0xde_1000, Size::Byte, 0);
    let mut id = 0u8;
    for _ in 0..8 {
        id = id << 1 | (b.read(0xde_1000, Size::Byte, false) >> 7) as u8;
    }
    assert_eq!(id, 0xd0);
    assert_eq!(b.read(0xda_8000, Size::Byte, false), 0, "no IDE drive");
    // Another model has no Gayle: open bus.
    let mut m = stub_with(None, &[]);
    assert_eq!(m.board.read(0xde_1000, Size::Byte, false), 0);
    assert!(m.board.mem.gayle.is_none());
    // The AGA chip set on another board, and its option checks.
    let m = stub_with(Some("2000"), &[("chipset", "aga"), ("chip", "2m")]);
    assert_eq!(m.board.custom.chipset, Chipset::AGA);
    assert!(m.cpu.model == cpu_m68k::Model::Mc68000);
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("chipset".into(), "aga".into());
    config.options.insert("denise".into(), "ocs".into());
    assert!(Amiga::new(&config).is_err(), "no single chips with AGA");
}

#[test]
fn one_megabyte_chip_ram_mirrors_once_and_runs() {
    let mut m = stub_with(None, &[("chip", "1m")]);
    frames(&mut m, 3);
    let b = &mut m.board;
    b.write(0x8_1234, Size::Word, 0xbeef);
    assert_eq!(
        b.read(0x18_1234, Size::Word, true),
        0xbeef,
        "mirror at 1 MB"
    );
    assert_eq!(b.read(0x1234, Size::Word, true), 0, "not at 512 KB");
    assert!(peek_cpu(&m, 0x100, Size::Long) >= 2, "the stub runs");
}

#[test]
fn fast_ram_board_goes_through_autoconfig() {
    let mut m = stub_with(None, &[("fast", "1m")]);
    frames(&mut m, 1);
    let b = &mut m.board;
    // er_Type: Zorro II, memory list, 1 MB = $E5.
    assert_eq!(b.read(0xe8_0000, Size::Byte, false) & 0xf0, 0xe0);
    assert_eq!(b.read(0xe8_0002, Size::Byte, false) & 0xf0, 0x50);
    assert_eq!(
        b.read(0x20_0000, Size::Word, true),
        0,
        "nothing at $200000 yet"
    );
    b.write(0x20_0000, Size::Word, 0x1234);
    assert_eq!(b.read(0x20_0000, Size::Word, true), 0, "writes go nowhere");
    // Kickstart's way: low nibble to $4A, high nibble to $48.
    b.write(0xe8_004a, Size::Byte, 0x00);
    b.write(0xe8_0048, Size::Byte, 0x20);
    assert_eq!(
        b.read(0xe8_0000, Size::Byte, false),
        0,
        "configured: open bus"
    );
    b.write(0x20_0000, Size::Long, 0xcafe_babe);
    assert_eq!(b.read(0x20_0000, Size::Long, true), 0xcafe_babe);
    b.write(0x20_0001, Size::Byte, 0x11);
    assert_eq!(b.read(0x20_0000, Size::Word, true), 0xca11);
    assert_eq!(b.read(0x30_0000, Size::Word, true), 0, "ends after 1 MB");
    // A savestate keeps the configuration and the contents.
    let s = m.save_state();
    let mut n = stub_with(None, &[("fast", "1m")]);
    n.load_state(&s).unwrap();
    assert_eq!(n.board.read(0x20_0000, Size::Long, true), 0xca11_babe);
    assert_eq!(
        n.memory_spaces().len(),
        3,
        "fast RAM is in the CPU space, no space of its own"
    );
    // A soft reset puts the board back into autoconfig; the RAM stays.
    Machine::reset(&mut n, ResetKind::Soft);
    assert_eq!(n.board.read(0xe8_0000, Size::Byte, false) & 0xf0, 0xe0);
    let mut other = stub_with(None, &[]);
    assert!(
        other.load_state(&s).is_err(),
        "fast RAM size is configuration"
    );
}

#[test]
fn a1000_boots_from_the_bootstrap_rom_and_protects_the_wcs() {
    let mut config = MachineConfig {
        model: Some("1000".into()),
        ..MachineConfig::default()
    };
    config.options.insert("wcs".into(), "disk".into());
    assert!(Amiga::new(&config).is_err(), "needs the bootstrap ROM");
    // The stub as a bootstrap ROM: it runs from $F80008 either way.
    config
        .roms
        .insert("a1000boot".into(), stub::rom()[..0x2000].to_vec());
    let mut m = Amiga::new(&config).unwrap();
    assert_eq!(m.board.mem.region(0, false), Region::Boot);
    assert_eq!(m.board.read(0xfc_0000, Size::Long, true), 0, "WCS empty");
    frames(&mut m, 3);
    assert!(
        peek_cpu(&m, 0x100, Size::Long) >= 2,
        "runs from the bootstrap ROM"
    );
    let b = &mut m.board;
    b.write(0xfc_0100, Size::Long, 0x1234_5678);
    assert_eq!(
        b.read(0xfc_0100, Size::Long, true),
        0x1234_5678,
        "the WCS is RAM"
    );
    assert_eq!(
        b.read(0xf8_0100, Size::Long, true),
        0x52b8_0100,
        "bootstrap ROM still in front (the stub's handler)"
    );
    b.write(0xf8_0000, Size::Word, 0);
    assert!(
        b.mem.a1000.as_ref().unwrap().protected,
        "a write to the bootstrap range latches"
    );
    assert_eq!(
        b.read(0xf8_0100, Size::Long, true),
        0x1234_5678,
        "the WCS shows twice"
    );
    b.write(0xfc_0100, Size::Long, 0);
    assert_eq!(
        b.read(0xfc_0100, Size::Long, true),
        0x1234_5678,
        "write-protected"
    );
    // Soft reset keeps the WCS, a power cycle clears it.
    Machine::reset(&mut m, ResetKind::Soft);
    assert_eq!(m.board.read(0xfc_0100, Size::Long, true), 0x1234_5678);
    Machine::reset(&mut m, ResetKind::Hard);
    assert_eq!(m.board.read(0xfc_0100, Size::Long, true), 0);
    assert_eq!(m.board.mem.region(0, false), Region::Boot);
    // Preloaded Kickstart: the stub in the WCS, protected.
    let mut m = stub_with(Some("1000"), &[]);
    frames(&mut m, 3);
    assert!(peek_cpu(&m, 0x100, Size::Long) >= 2);
    assert_eq!(m.board.mem.region(0xf8_0000, false), Region::Rom);
}

#[test]
fn serial_loopback_returns_what_was_sent() {
    let mut m = stub_with(None, &[("serial", "loopback")]);
    frames(&mut m, 1);
    let b = &mut m.board;
    b.write(0xdf_f032, Size::Word, 0x0173); // 9600 baud: 3 546 895 / 372
    b.write(0xdf_f09c, Size::Word, 0x0801); // clear RBF, TBE
    b.write(0xdf_f030, Size::Word, 0x0100 | 0x41);
    assert_eq!(
        b.read(0xdf_f01e, Size::Word, true) & 0x0001,
        0x0001,
        "TBE: buffer taken"
    );
    assert_eq!(
        b.read(0xdf_f018, Size::Word, true) & 0x1000,
        0,
        "TSRE clear: sending"
    );
    frames(&mut m, 1);
    let b = &mut m.board;
    let serdatr = b.read(0xdf_f018, Size::Word, true);
    assert_eq!(
        serdatr & 0x43ff,
        0x4141,
        "RBF with the data and stop bit: {serdatr:04x}"
    );
    assert_eq!(
        b.read(0xdf_f01e, Size::Word, true) & 0x0800,
        0x0800,
        "INTREQ RBF"
    );
    b.write(0xdf_f09c, Size::Word, 0x0800);
    assert_eq!(
        b.read(0xdf_f018, Size::Word, true) & 0xc000,
        0,
        "RBF and OVRUN cleared"
    );
    // Without loopback nothing comes back.
    let mut m = stub_with(None, &[]);
    frames(&mut m, 1);
    m.board.write(0xdf_f030, Size::Word, 0x0141);
    frames(&mut m, 1);
    assert_eq!(m.board.read(0xdf_f018, Size::Word, true) & 0x4000, 0);
}

#[test]
fn attached_audio_channel_modulates_the_next() {
    let mut m = stub_with(None, &[]);
    frames(&mut m, 1);
    let b = &mut m.board;
    // Channel 0 attached to channel 1's volume (ADKCON USE0V); CPU-written
    // words: the word $0020 sets AUD1VOL to 32.
    b.write(0xdf_f09e, Size::Word, 0x8001);
    b.write(0xdf_f0a6, Size::Word, 200); // AUD0PER
    b.write(0xdf_f0b8, Size::Word, 64); // AUD1VOL
    b.write(0xdf_f0aa, Size::Word, 0x0020); // AUD0DAT
    for _ in 0..4 {
        b.tick(false);
    }
    assert_eq!(b.custom.paula.audio[1].vol, 32);
    assert_eq!(
        b.custom.paula.audio[0].output, 0,
        "an attached channel is silent"
    );
}

#[test]
fn graphics_views_decode_chip_ram_without_side_effects() {
    let mut m = stub();
    frames(&mut m, 2);
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["sprites", "bitplanes", "palette"]);
    // One low-resolution plane at $20000, pixel 0 of line 0 set.
    m.board.mem.chip.set_word(0x2_0000, 0x8000);
    let c = &mut m.board.custom;
    for (r, v) in [
        (chips_amiga::reg::BPLCON0, 0x1200),
        (chips_amiga::reg::DIWSTRT, 0x2c81),
        (chips_amiga::reg::DIWSTOP, 0x2cc1),
        (chips_amiga::reg::DDFSTRT, 0x38),
        (chips_amiga::reg::DDFSTOP, 0xd0),
        (chips_amiga::reg::BPL1PTH, 0x0002),
        (chips_amiga::reg::BPL1PTH + 2, 0x0000),
        (chips_amiga::reg::COLOR00 + 2, 0x0f0),
    ] {
        c.write(r, v);
    }
    let before = m.save_state();
    let v = m
        .gfx_view("bitplanes", &emu_core::GfxOptions::new())
        .unwrap();
    assert_eq!(v.space, CHIP_SPACE);
    assert_eq!((v.width, v.height, v.addr), (320, 256, 0x2_0000));
    assert_eq!(v.raw.len(), 40 * 256);
    assert_eq!(v.frame().rgb_at(0, 0), Some(Rgb8 { r: 0, g: 255, b: 0 }));
    let p = m.gfx_view("palette", &emu_core::GfxOptions::new()).unwrap();
    assert_eq!(p.param("color01"), Some(0x0f0));
    let s = m.gfx_view("sprites", &emu_core::GfxOptions::new()).unwrap();
    assert_eq!(s.width, 8 * 17 - 1);
    assert!(m.gfx_view("nope", &emu_core::GfxOptions::new()).is_none());
    assert_eq!(m.save_state(), before, "views change nothing");
}

#[test]
fn overlays_record_only_when_switched_on_and_stay_out_of_savestates() {
    let mut m = stub();
    let ids: Vec<_> = m.overlays().iter().map(|o| o.id).collect();
    assert_eq!(ids, ["dma", "sprites", "copper"]);
    assert_eq!(m.frame_row(26), Some(0));
    assert_eq!(m.frame_row(10), None);
    frames(&mut m, 1);
    assert!(m.overlay("dma").is_none(), "off");
    let mut twin = stub();
    frames(&mut twin, 1);
    m.set_overlays(&["dma", "copper"]);
    frames(&mut m, 2);
    frames(&mut twin, 2);
    assert_eq!(
        m.save_state(),
        twin.save_state(),
        "recording changes nothing"
    );
    // The stub's Copper list changes COLOR00 at two lines.
    let c = m.overlay("copper").unwrap();
    assert!(!c.shapes.is_empty());
    let d = m.overlay("dma").unwrap();
    let emu_core::overlay::Shape::Cells { ref cells, .. } = d.shapes[0] else {
        panic!()
    };
    assert!(cells.contains(&8), "free slots");
    assert!(m.overlay("sprites").is_none(), "not switched on");
    let s = m.save_state();
    m.load_state(&s).unwrap();
    assert!(m.overlay("dma").is_some(), "still recording after a load");
}

#[test]
fn light_pen_in_game_port_0_freezes_the_beam_counters() {
    let mut m = stub_with(None, &[("port0", "lightpen")]);
    frames(&mut m, 2);
    // LPEN on; the stub's picture is green (bright) below line $80.
    m.board.custom.write(chips_amiga::reg::BPLCON0, 0x0208);
    let now = m.now().0;
    m.input(TimedInput {
        at: Cycle(now),
        target: InputTarget::Port(PortId(0)),
        event: emu_core::input::InputEvent::LightPen {
            x: 200,
            y: 150,
            trigger: true,
        },
    });
    frames(&mut m, 2);
    // Row 150 is line 26 + 150 = $B0; column 200 colour clock 108 of Agnus.
    let vhposr = |m: &Amiga| peek_cpu(m, 0xdf_f006, Size::Word);
    assert_eq!(vhposr(&m), 0xb000 | 108);
    // The button (LPENPR, pin 5) reads on POTGOR bit 8 (DATLX) low.
    assert_eq!(peek_cpu(&m, 0xdf_f016, Size::Word) & 0x0100, 0);
    // Savestate round trip keeps the latch and the pen's aim.
    let s = m.save_state();
    let mut n = stub_with(None, &[("port0", "lightpen")]);
    n.load_state(&s).unwrap();
    assert_eq!(vhposr(&n), 0xb000 | 108);
    frames(&mut n, 1);
    assert_eq!(vhposr(&n), 0xb000 | 108);
    // In the red part (luma 76) the pen sees no light: the counters
    // freeze at the end of the field instead.
    let now = n.now().0;
    n.input(TimedInput {
        at: Cycle(now),
        target: InputTarget::Port(PortId(0)),
        event: emu_core::input::InputEvent::LightPen {
            x: 200,
            y: 50,
            trigger: false,
        },
    });
    frames(&mut n, 2);
    assert_eq!(vhposr(&n), (312 & 0xff) << 8 | 226);
}
