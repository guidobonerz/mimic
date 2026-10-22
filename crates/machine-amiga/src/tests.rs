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
    assert_eq!(b.custom.denise.colors[0], 0x0f0f & 0xfff);
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
        let end = at + 4 + u64::from(e_clock_waits(at));
        assert_eq!(end % 10, 0);
        assert!((at + 10..=at + 20).contains(&end), "{at} → {end}");
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
        let lines = (m.now().0 - t0) / (2 * 227);
        seen.push((lines, m.framebuffer().signal.field));
    }
    use emu_core::frame::FieldParity::{Even, Odd};
    let pattern: Vec<_> = seen.iter().map(|&(_, f)| f).collect();
    assert!(
        pattern == [Odd, Even, Odd, Even] || pattern == [Even, Odd, Even, Odd],
        "{seen:?}"
    );
    assert!(seen.iter().any(|&(l, _)| l == 312) && seen.iter().any(|&(l, _)| l == 313));
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
            dbg: Some(&mut dbg),
            beam: BeamPos::default(),
        };
        assert_eq!(bus.read(0x100, Size::Word), 0x0000, "high word");
        assert_eq!(bus.read(0x102, Size::Word), 0x0042, "low word");
        assert_eq!(bus.read(0x8_0102, Size::Word), 0x0042, "through the mirror");
    }
}
