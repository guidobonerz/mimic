use super::*;

fn stub(model: &str) -> Kc85 {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Kc85::new(&config).unwrap()
}

fn run(m: &mut Kc85, fields: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..fields {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn stub_draws_its_cell_and_counts_interrupts() {
    for model in ["2", "3", "4", "5"] {
        let mut m = stub(model);
        run(&mut m, 3);
        let fg = stub::COLOUR >> 3;
        let bg = 16 + (stub::COLOUR & 7);
        let want: Vec<u8> = (0..8)
            .map(|i| {
                if stub::PIXELS & (0x80 >> i) != 0 {
                    fg
                } else {
                    bg
                }
            })
            .collect();
        assert_eq!(m.pixels[..8], want[..], "model {model}");
        // Timer: 64 · 256 clocks an interrupt, about 2.1 a field.
        let n = m.peek_cpu(stub::COUNTER);
        assert!((5..=7).contains(&n), "model {model}: {n} interrupts");
    }
}

#[test]
fn field_length() {
    for (model, clocks) in [("3", 112 * 312), ("4", 227 * 312 / 2)] {
        let mut m = stub(model);
        run(&mut m, 2);
        let a = m.now().0;
        run(&mut m, 50);
        let per_field = (m.now().0 - a) as f64 / 50.0;
        assert!(
            (per_field - f64::from(clocks)).abs() < 1.0,
            "model {model}: {per_field}"
        );
    }
}

#[test]
fn reset_keeps_ram_and_power_clears_it() {
    let mut m = stub("3");
    run(&mut m, 3);
    assert!(m.mem.poke(0x1234, 0x55));
    m.reset(ResetKind::Soft);
    assert_eq!(m.peek_cpu(0x1234), 0x55);
    assert_eq!(m.cpu.pc, 0xe000);
    m.reset(ResetKind::Hard);
    assert_eq!(m.peek_cpu(0x1234), 0);
    assert_eq!(m.cpu.pc, 0xf000);
    run(&mut m, 3);
    assert!(m.peek_cpu(stub::COUNTER) > 0);
}

#[test]
fn savestate_round_trip_and_determinism() {
    let mut a = stub("4");
    run(&mut a, 3);
    let state = a.save_state();
    let mut b = stub("4");
    b.load_state(&state).unwrap();
    run(&mut a, 5);
    run(&mut b, 5);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.pixels, b.pixels);
    // Another model's state is refused, corrupt data never panics.
    assert!(stub("3").load_state(&state).is_err());
    for cut in (0..state.len()).step_by(97) {
        let _ = b.load_state(&state[..cut]);
    }
}

#[test]
fn a_module_shows_its_structure_byte_and_its_memory() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("slot8".into(), "m022".into());
    let mut m = Kc85::new(&config).unwrap();
    run(&mut m, 1);
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0x0880, Size::Byte), Some(0xf4));
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0x0c80, Size::Byte), Some(0xff));
    config.options.insert("slotc".into(), "m026".into());
    assert!(matches!(
        Kc85::new(&config),
        Err(ConfigError::MissingRom(r)) if r == "m026"
    ));
    config.roms.insert("m026".into(), vec![0x7f; 0x2000]);
    let mut m = Kc85::new(&config).unwrap();
    m.mem.modules.control(0x0c, 0x41);
    assert_eq!(m.peek_cpu(0x4000), 0x7f, "RAM4 is off, the module answers");
}

#[test]
fn sound_follows_the_ctc() {
    let mut m = stub("3");
    run(&mut m, 2);
    let mut out = Vec::new();
    m.audio(&mut out);
    out.clear();
    // Channel 0: timer, prescaler 16, constant 100: a 547 Hz square wave.
    m.ctc.write(0, 0x07);
    m.ctc.write(0, 100);
    run(&mut m, 50);
    let format = m.audio(&mut out);
    assert_eq!(format.channels, 1);
    assert!((47_500..48_500).contains(&out.len()), "{}", out.len());
    let crossings = out
        .windows(2)
        .filter(|w| (w[0] < w[1]) && w[0] < 0 && w[1] >= 0)
        .count();
    assert!(
        (530..560).contains(&crossings),
        "{crossings} periods a second"
    );
}

/// An in-memory disc image.
struct Mem(Vec<u8>);

impl mimic_core::media::MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), mimic_core::media::MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), mimic_core::media::MediaError> {
        let o = offset as usize;
        self.0[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), mimic_core::media::MediaError> {
        Ok(())
    }
}

/// The D004 side's test program, loaded into the Koppel-RAM at `$FC00`:
/// drive 0's motor on, wait for READY, SPECIFY (DMA mode), READ DATA of
/// track 0 record 1 (1024 bytes), 512 of them by DACK into `$9000`, then
/// TC; the seven result bytes to `$FD80`, the first and last byte read to
/// `$FFF0`/`$FFF1`, 1 to `$FFFF`; HALT.
const D004_PROGRAM: [u8; 0x7e] = [
    0x31, 0x00, 0x80, // FC00 LD SP,$8000
    0x3e, 0x01, 0xd3, 0xf6, // FC03 drive 0, motor on
    0xdb, 0xf4, 0xcb, 0x6f, 0x28, 0xfa, // FC07 wait for READY (bit 5)
    0x21, 0x60, 0xfc, 0x06, 0x03, 0xcd, 0x40, 0xfc, // FC0D SPECIFY
    0x21, 0x63, 0xfc, 0x06, 0x09, 0xcd, 0x40, 0xfc, // FC15 READ DATA
    0x21, 0x00, 0x90, // FC1D LD HL,$9000
    0x01, 0xf2, 0x00, // FC20 LD BC,$00F2
    0x16, 0x02, // FC23 LD D,2
    0xdb, 0xf4, // FC25 rd: IN A,($F4)
    0x87, // FC27 ADD A,A
    0xf2, 0x34, 0xfc, // FC28 JP P,rdend (INT)
    0x38, 0xf8, // FC2B JR C,rd (no DRQ)
    0xed, 0xa2, // FC2D INI
    0x20, 0xf4, // FC2F JR NZ,rd
    0x15, // FC31 DEC D
    0x20, 0xf1, // FC32 JR NZ,rd
    0xd3, 0xf8, // FC34 rdend: OUT ($F8),A (TC)
    0x21, 0x80, 0xfd, // FC36 LD HL,$FD80
    0x06, 0x07, // FC39 LD B,7
    0xc3, 0x50, 0xfc, // FC3B JP res
    0x00, 0x00, // FC3E
    0xdb, 0xf0, 0xe6, 0xc0, 0xfe, 0x80, 0x20, 0xf8, // FC40 send: wait RQM, to FDC
    0x7e, 0xd3, 0xf1, 0x23, 0x10, 0xf2, 0xc9, // FC48
    0x00, // FC4F
    0xdb, 0xf0, 0xe6, 0xc0, 0xfe, 0xc0, 0x20, 0xf8, // FC50 res: wait RQM + DIO
    0xdb, 0xf1, 0x77, 0x23, 0x10, 0xf2, // FC58
    0x18, 0x0c, // FC5E JR fin
    0x03, 0x9f, 0x14, // FC60 SPECIFY: SRT 9, ND = 0
    0x46, 0x00, 0x00, 0x00, 0x01, 0x03, 0x05, 0x2a, 0xff, // FC63 READ DATA
    0x3a, 0x00, 0x90, 0x32, 0xf0, 0xff, // FC6C fin
    0x3a, 0xff, 0x91, 0x32, 0xf1, 0xff, // FC72
    0x3e, 0x01, 0x32, 0xff, 0xff, // FC78
    0x76, // FC7D HALT
];

/// The KC side: clear the D004's RAM and hold it in RESET, copy 256 bytes
/// from `$0300` into the Koppel-RAM's first page, let it run.
const KC_PROGRAM: [u8; 0x17] = [
    0x3e, 0x02, 0xd3, 0xf4, // 0200 clear + RESET
    0x21, 0x00, 0x03, // 0204 LD HL,$0300
    0x01, 0xf0, 0x00, // 0207 LD BC,$00F0
    0x7e, 0xed, 0x79, 0x23, 0x04, 0x20, 0xf9, // 020A copy loop
    0x3e, 0x01, 0xd3, 0xf4, // 0211 run
    0x18, 0xfe, // 0215 JR $
];

fn with_d004() -> Kc85 {
    let mut config = MachineConfig {
        model: Some("4".into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("d004".into(), "on".into());
    assert!(matches!(
        Kc85::new(&config),
        Err(ConfigError::MissingRom(r)) if r == "d004"
    ));
    config.roms.insert("d004".into(), vec![0xa7; 0x2000]);
    Kc85::new(&config).unwrap()
}

/// A byte of the Koppel-RAM as the KC reads it.
fn koppel(m: &Kc85, addr: u16) -> u8 {
    let page = u32::from(addr >> 8 & 3);
    let port = u32::from(addr & 0xff) << 8 | (0xf0 + page);
    m.peek(CPU_SPACE, IO_BASE + port, Size::Byte).unwrap() as u8
}

#[test]
fn the_d004_boots_from_the_koppel_ram_and_reads_a_sector_by_dma() {
    let mut m = with_d004();
    assert_eq!(m.devices(), vec![CPU, d004::D004_DEVICE]);
    run(&mut m, 2);
    // Slot $FC: the ROM's structure byte; the Koppel-RAM answers only
    // while the module is connected.
    assert_eq!(m.peek(CPU_SPACE, IO_BASE + 0xfc80, Size::Byte), Some(0xa7));
    m.d004.as_mut().unwrap().kc_write(0x00f0, 0x11);
    assert_eq!(koppel(&m, 0xfc00), 0xff, "not connected");
    m.mem.modules.control(0xfc, 0x05);
    assert_eq!(koppel(&m, 0xfc00), 0x11);
    assert_eq!(m.peek_cpu(0xc000), 0xa7, "the ROM at $C000");
    m.mem.modules.control(0xfc, 0xff);
    assert_eq!(m.peek_cpu(0xc000), 0xff, "bit 5: at $E000 (under CAOS)");

    let mut disc = vec![0u8; 819_200];
    disc[0] = 0x5a;
    disc[0x1ff] = 0xa5;
    m.media_slot(SLOT_DRIVE0)
        .unwrap()
        .insert(mimic_core::media::Media {
            name: "boot.img".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(disc)),
        })
        .unwrap();
    for (i, &b) in KC_PROGRAM.iter().enumerate() {
        assert!(m.mem.poke(0x200 + i as u16, b));
    }
    for (i, &b) in D004_PROGRAM.iter().enumerate() {
        assert!(m.mem.poke(0x300 + i as u16, b));
    }
    m.cpu.pc = 0x200;
    run(&mut m, 2);
    let d = m.d004.as_ref().unwrap();
    assert!(d.running());
    assert_eq!(d.ram()[0x1000], 0, "the RAM was cleared");
    // The NOPs up to $FC00 take 65 ms; the sector comes within a turn.
    let mut fields = 0;
    while koppel(&m, 0xffff) != 1 {
        run(&mut m, 1);
        fields += 1;
        assert!(fields < 40, "the D004 did not finish");
    }
    assert_eq!((koppel(&m, 0xfff0), koppel(&m, 0xfff1)), (0x5a, 0xa5));
    let result: Vec<u8> = (0..7).map(|i| koppel(&m, 0xfd80 + i)).collect();
    assert_eq!(result, vec![0x00, 0, 0, 0, 0, 2, 3], "normal end after TC");
    let target = m.debug_target(d004::D004_DEVICE).unwrap();
    assert_eq!(
        target.peek(d004::D004_SPACE, 0x9000, Size::Byte),
        Some(0x5a)
    );
    assert_eq!(target.disassemble(0xfc7d).mnemonic, "HALT");

    // A savestate with the D004 restores and runs on alike.
    let state = m.save_state();
    let mut n = with_d004();
    n.load_state(&state).unwrap();
    run(&mut m, 3);
    run(&mut n, 3);
    assert_eq!(m.save_state(), n.save_state());
    for cut in (0..state.len()).step_by(997) {
        let _ = n.load_state(&state[..cut]);
    }
}

/// CAOS' key cell: `(IX+13)` the code, bit 0 of `(IX+8)` "a key is
/// there" (set here by hand: the stub ROM has no CAOS).
#[test]
fn pasted_text_goes_into_caos_key_cell_one_character_at_a_time() {
    let mut m = stub("4");
    run(&mut m, 2);
    // Without CAOS' working cells the text is typed.
    assert_eq!(m.paste_chars("A"), None);
    m.cpu.ix = 0x01f0;
    m.cpu.i = 1;
    m.cpu.im = 2;
    m.mem.pio_a |= memory::A_RAM0 | memory::A_RAM0_WRITE;
    m.mem.poke(0x01f8, 0x80);
    // A character the keyboard lacks is dropped with the one behind it.
    assert_eq!(m.paste_chars("\u{e4}a\nB"), Some(2));
    assert_eq!(m.peek_cpu(0x01fd), b'a');
    assert_eq!(m.peek_cpu(0x01f8), 0x81, "the other bits stay");
    // Nothing more until KBD took the key.
    assert_eq!(m.paste_chars("\nB"), Some(0));
    m.mem.poke(0x01f8, 0x80);
    assert_eq!(m.paste_chars("\nB"), Some(1));
    assert_eq!(m.peek_cpu(0x01fd), 0x0d);
    // While CAOS reads the tape (PIO A's interrupt on) the cell holds
    // the block's checksum.
    m.mem.poke(0x01f8, 0x80);
    m.pio.write_control(pio::A, 0x83);
    assert_eq!(m.paste_chars("B"), Some(0));
    assert_eq!(m.peek_cpu(0x01fd), 0x0d);
    m.pio.write_control(pio::A, 0x03);
    assert_eq!(m.paste_chars("B"), Some(1));
    assert_eq!(m.peek_cpu(0x01fd), b'B');
}
