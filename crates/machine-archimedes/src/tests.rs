use super::*;

fn stub(model: &str) -> Archimedes {
    let mut c = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    c.options.insert("roms".into(), "stub".into());
    Archimedes::new(&c).unwrap()
}

/// A machine running `code` from ROM.
fn with_rom(code: &[u32]) -> Archimedes {
    let mut rom = vec![0u8; 0x8_0000];
    for (i, w) in code.iter().enumerate() {
        rom[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    let mut c = MachineConfig::default();
    c.roms.insert("os200".into(), rom);
    Archimedes::new(&c).unwrap()
}

#[test]
fn models_take_their_ram() {
    assert_eq!(stub("a305").board.ram.len(), 0x8_0000);
    assert_eq!(stub("a310").board.ram.len(), 0x10_0000);
    assert_eq!(stub("a440").board.ram.len(), 0x40_0000);
    let mut c = MachineConfig::default();
    assert!(matches!(
        Archimedes::new(&c).err(),
        Some(ConfigError::MissingRom(_))
    ));
    c.options.insert("os".into(), "9.9".into());
    assert!(Archimedes::new(&c).is_err());
}

#[test]
fn a_frame_is_312_lines_of_64_us_before_vidc_is_set_up() {
    let mut m = stub("a310");
    let t = m.clocks();
    assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
    let frame = m.clocks() - t;
    assert!(frame.abs_diff(312 * 512) < 8, "{frame}");
}

#[test]
fn rom_writes_ram_through_the_physical_map() {
    let m = {
        let mut m = with_rom(&[
            0xe3a0_0402, // MOV R0,#&2000000
            0xe3a0_10ab, // MOV R1,#&AB
            0xe580_1010, // STR R1,[R0,#&10]
            0xeaff_fffe, // B .
        ]);
        for _ in 0..8 {
            m.step_cycle();
        }
        m
    };
    assert_eq!(m.board.ram[0x10], 0xab);
}

#[test]
fn vidc_and_ioc_are_reached_from_the_arm() {
    let mut m = with_rom(&[0xeaff_fffe, 0xeaff_fffe]);
    // By the bus directly, as the ARM would.
    let mut bus = ArcBus {
        b: &mut m.board,
        disks: &[],
        dbg: None,
    };
    assert!(bus.write_word(0x340_0000, 0x4000_0123, false, true));
    assert_eq!(bus.b.vidc.border, 0x123);
    assert!(bus.write_byte(0x320_0018, 0x20, false, true));
    assert_eq!(bus.b.ioc.mask_a, 0x20);
    assert!(
        bus.write_word(0x320_0028, 0x0055_0000, false, true),
        "word: D23-16"
    );
    assert_eq!(bus.b.ioc.mask_b, 0x55);
    assert_eq!(bus.read_byte(0x320_0028, false, true), Some(0x55));
    assert!(
        !bus.write_word(0x320_0000, 0, false, false),
        "user mode aborts"
    );
    assert_eq!(bus.read_word(0x320_0000, false, false), None);
}

#[test]
fn latch_a_selects_drive_and_side() {
    let mut m = stub("a310");
    m.board.io_write(0x335_0040, 0b0010_1110, &[]);
    assert_eq!(m.board.fdc.drive, Some(0));
    assert_eq!(m.board.fdc.side, 1);
    m.board.io_write(0x335_0040, 0b0011_1101, &[]);
    assert_eq!((m.board.fdc.drive, m.board.fdc.side), (Some(1), 0));
}

#[test]
fn keyboard_reset_reaches_the_ioc() {
    let mut m = stub("a310");
    frames(&mut m, 1);
    assert_eq!(m.board.ioc.read(0x04, 0), keyboard::HRST);
    m.board.ioc.write(0x04, keyboard::HRST);
    frames(&mut m, 1);
    assert_eq!(m.board.ioc.read(0x04, 0), keyboard::HRST);
}

fn frames(m: &mut Archimedes, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        m.run_frame(&mut dbg);
    }
}

#[test]
fn rtc_answers_on_control_lines() {
    let mut m = stub("a310");
    // Start, address byte, the device pulls SDA low on the ninth clock.
    let lines = |m: &mut Archimedes, sda: u8, scl: u8| {
        m.board.io_write(0x320_0000, 0xfc | sda | scl << 1, &[]);
    };
    lines(&mut m, 1, 1);
    lines(&mut m, 0, 1);
    lines(&mut m, 0, 0);
    for i in (0..8).rev() {
        let b = 0xa0u8 >> i & 1;
        lines(&mut m, b, 0);
        lines(&mut m, b, 1);
        lines(&mut m, b, 0);
    }
    lines(&mut m, 1, 0);
    assert_eq!(m.board.pins(&[]) & 1, 0, "acknowledged");
}

#[test]
fn flyback_interrupts_and_shows_in_the_control_register() {
    let mut m = stub("a310");
    frames(&mut m, 1);
    assert_ne!(m.board.ioc.read(0x10, 0) & ioc::IRQ_A_FLYBACK, 0);
    assert_eq!(
        m.board.pins(&[]) & 0x80,
        0,
        "the frame ended in flyback's end"
    );
}

#[test]
fn sound_dma_plays_through_the_dac() {
    let mut m = stub("a310");
    m.board.ram[0x100..0x110].fill(0xfe);
    m.board.vidc.sfr = 15;
    m.board.memc.write(0x360_0000 | 4 << 17 | (0x100 / 16) << 2);
    m.board.memc.write(0x360_0000 | 5 << 17 | (0x200 / 16) << 2);
    m.board.memc.write(0x360_0000 | 6 << 17);
    m.board.memc.write(0x360_0000 | 7 << 17 | 1 << 11);
    frames(&mut m, 1);
    let mut out = vec![];
    let f = m.audio(&mut out);
    assert_eq!(f.channels, 2);
    assert!(out.iter().any(|&s| s > 1000), "{}", out.len());
    assert!(out.len() > 1500);
}

#[test]
fn disks_go_in_by_size_only() {
    let mut m = stub("a310");
    struct Mem(Vec<u8>);
    impl emu_core::media::MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), MediaError> {
            let o = o as usize;
            b.copy_from_slice(&self.0[o..o + b.len()]);
            Ok(())
        }
        fn write(&mut self, _o: u64, _d: &[u8]) -> Result<(), MediaError> {
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }
    let disk = |n| Media {
        name: "d".into(),
        kind: MediaKind::Disk,
        backend: Box::new(Mem(vec![0; n])),
    };
    let slot = m.media_slot(SLOT_DISK0).unwrap();
    assert!(slot.insert(disk(1000)).is_err());
    slot.insert(disk(819_200)).unwrap();
    assert!(m.media_slots()[0].inserted);
    assert!(m.media_slot(SlotId(9)).is_none());
}

#[test]
fn savestate_round_trip_runs_on_identically() {
    let mut a = stub("a310");
    frames(&mut a, 3);
    let state = a.save_state();
    let mut b = stub("a310");
    b.load_state(&state).unwrap();
    frames(&mut a, 3);
    frames(&mut b, 3);
    assert_eq!(a.save_state(), b.save_state());
    assert!(b.load_state(&state[..50]).is_err());
}
