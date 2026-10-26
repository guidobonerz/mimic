use super::*;
use emu_core::media::MediaBackend;
use emu_core::registry::MachineConfig;

fn stub(model: &str) -> Spectrum {
    let mut config = MachineConfig {
        model: Some(model.into()),
        ..MachineConfig::default()
    };
    config.options.insert("roms".into(), "stub".into());
    Spectrum::new(&config).unwrap()
}

struct Mem(Vec<u8>);

impl MediaBackend for Mem {
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

fn media(kind: MediaKind, bytes: Vec<u8>) -> Media {
    Media {
        name: "test".into(),
        kind,
        backend: Box::new(Mem(bytes)),
    }
}

fn frames(m: &mut Spectrum, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        while m.run_frame(&mut dbg) != StopReason::FrameDone {}
    }
}

#[test]
fn stub_boots_counts_frames_and_sets_the_border() {
    for model in ["48k", "128k"] {
        let mut m = stub(model);
        frames(&mut m, 12);
        let count = peek_cpu(&m, stub::FRAMES);
        // The stub's LDIRs take the first two frames.
        assert!((8..=10).contains(&count), "{model}: {count} interrupts");
        assert_eq!(m.frame_start, 12 * m.frame_len(), "{model}: frame lengths");
        let f = m.framebuffer();
        let PixelData::Indexed { pixels, .. } = f.pixels else {
            panic!("indexed");
        };
        // The border of the last frame: its interrupt set it to the count
        // before the picture's first line.
        assert_eq!(pixels[0], count & 7, "{model}: border");
        // Display: $55 (black ink on white paper): pixel 0 white, 1 black.
        let at = |x: u32, y: u32| pixels[(y * ula::WIDTH + x) as usize];
        assert_eq!((at(48, 48), at(49, 48)), (7, 0), "{model}: display");
    }
}

/// Run one instruction at frame T-state `t` and return its length.
fn timed(m: &mut Spectrum, t: u32) -> u64 {
    let now = m.sched.now().0;
    m.frame_start = now - u64::from(t);
    m.step_cycle();
    m.sched.now().0 - now
}

#[test]
fn contended_memory_delays_by_the_table() {
    let mut m = stub("48k");
    frames(&mut m, 1);
    // LD A,($4000) at $8000: its data read comes 10 T-states in.
    for (i, b) in [0x3a, 0x00, 0x40].iter().enumerate() {
        m.mem.write(0x8000 + i as u16, *b);
    }
    for (start, want) in [(14325, 13 + 6), (14326, 13 + 5), (14331, 13), (14320, 13)] {
        m.cpu.pc = 0x8000;
        assert_eq!(timed(&mut m, start), want, "from T-state {start}");
    }
    // NOP in contended RAM: the fetch waits too.
    m.mem.write(0x4000, 0x00);
    m.cpu.pc = 0x4000;
    assert_eq!(timed(&mut m, 14335), 4 + 6);
    // INC BC at $8000 with I = $40: the two internal T-states put IR
    // ($40xx) on the bus and are contended.
    m.mem.write(0x8000, 0x03);
    m.cpu.pc = 0x8000;
    m.cpu.i = 0x40;
    // The fetch (4) runs 14331–14334, the internal ones at 14335 (6) and
    // 14342 (0).
    assert_eq!(timed(&mut m, 14331), 6 + 6);
}

#[test]
fn io_contention_and_the_floating_bus() {
    let mut m = stub("48k");
    frames(&mut m, 1);
    m.mem.bank_mut(5)[0] = 0x5a; // bitmap byte 0
    m.mem.bank_mut(5)[0x1800] = 0x38; // its attribute
                                      // IN A,($FF) at $8000 with A = 0: port $00FF (odd, uncontended high
                                      // byte): N:4. The read sees the floating bus 3 T-states into the I/O
                                      // cycle, which starts 7 T-states into the instruction.
    for (i, b) in [0xdb, 0xff].iter().enumerate() {
        m.mem.write(0x8000 + i as u16, *b);
    }
    let first = 14338 - 3 - 7;
    for (start, want) in [(first, 0x5a), (first + 1, 0x38), (first + 4, 0xff)] {
        m.cpu.pc = 0x8000;
        m.cpu.a = 0;
        assert_eq!(timed(&mut m, start), 11, "uncontended");
        assert_eq!(m.cpu.a, want, "floating bus from T-state {start}");
    }
    // IN A,($FE) with A = $40: contended high byte, even port: C:1, C:3.
    m.cpu.pc = 0x8000;
    m.cpu.a = 0x40;
    m.mem.write(0x8001, 0xfe);
    assert_eq!(timed(&mut m, 14335 - 7), 11 + 6);
}

#[test]
fn paging_through_port_7ffd() {
    let mut m = stub("128k");
    // LD BC,$7FFD; LD A,$1B; OUT (C),A at $8000.
    let code = [0x01, 0xfd, 0x7f, 0x3e, 0x1b, 0xed, 0x79];
    for (i, b) in code.iter().enumerate() {
        m.mem.write(0x8000 + i as u16, *b);
    }
    m.cpu.pc = 0x8000;
    for _ in 0..3 {
        m.step_cycle();
    }
    assert_eq!(m.mem.paging, 0x1b);
    assert_eq!(m.mem.screen_bank(), 7);
    assert_eq!(m.mem.rom_index(), 1);
    m.mem.write(0xc000, 0x77);
    assert_eq!(m.mem.bank(3)[0], 0x77);
    assert!(m.mem.contended(0xc000));
    // The AY: select register 7, write $38, read it back.
    let code = [
        0x01, 0xfd, 0xff, 0x3e, 0x07, 0xed, 0x79, // LD BC,$FFFD; LD A,7; OUT (C),A
        0x06, 0xbf, 0x3e, 0x38, 0xed, 0x79, // LD B,$BF; LD A,$38; OUT (C),A
        0x06, 0xff, 0xed, 0x78, // LD B,$FF; IN A,(C)
    ];
    for (i, b) in code.iter().enumerate() {
        m.mem.write(0x8000 + i as u16, *b);
    }
    m.cpu.pc = 0x8000;
    for _ in 0..8 {
        m.step_cycle();
    }
    assert_eq!(m.ay.as_ref().unwrap().regs[7], 0x38);
    assert_eq!(m.cpu.a, 0x38);
}

#[test]
fn kempston_and_sinclair_joysticks() {
    let mut m = stub("48k");
    m.connect(
        PORT_SINCLAIR1,
        Box::new(devices_input::Joystick::new(PortKind::SpectrumJoy)),
    )
    .unwrap();
    let at = m.now();
    let digital = |port, dirs| TimedInput {
        at,
        target: InputTarget::Port(port),
        event: InputEvent::Digital { dirs, buttons: 1 },
    };
    use emu_core::input::Dirs;
    m.input(digital(PORT_KEMPSTON, Dirs::LEFT));
    m.input(digital(PORT_SINCLAIR1, Dirs::UP));
    // IN A,($1F); then LD A,$EF; IN A,($FE) (keys 6–0).
    let code = [0x3e, 0x00, 0xdb, 0x1f, 0x47, 0x3e, 0xef, 0xdb, 0xfe];
    for (i, b) in code.iter().enumerate() {
        m.mem.write(0x8000 + i as u16, *b);
    }
    m.cpu.pc = 0x8000;
    for _ in 0..5 {
        m.step_cycle();
    }
    assert_eq!(m.cpu.b, 0x12, "Kempston: left and fire");
    // Up = 9 (bit 1), fire = 0 (bit 0), active low.
    assert_eq!(m.cpu.a & 0x1f, 0x1c, "Sinclair 1: 9 and 0");
    assert!(m
        .connect(
            PORT_MOUSE,
            Box::new(devices_input::Joystick::new(PortKind::SpectrumJoy))
        )
        .is_err());
}

#[test]
fn savestate_round_trip_is_exact() {
    let mut m = stub("128k");
    frames(&mut m, 3);
    let s = m.save_state();
    frames(&mut m, 2);
    let (now, pixels) = (m.now(), m.ula.pixels().to_vec());
    let mut n = stub("128k");
    n.load_state(&s).unwrap();
    frames(&mut n, 2);
    assert_eq!(n.now(), now);
    assert_eq!(n.ula.pixels(), &pixels[..]);
    assert!(n.load_state(&s[..s.len() / 2]).is_err());
    assert!(stub("48k").load_state(&s).is_err(), "other model");
}

#[test]
fn sna_snapshot_loads_at_the_next_instruction() {
    let mut m = stub("48k");
    frames(&mut m, 1);
    let mut f = vec![0u8; 49179];
    f[23..25].copy_from_slice(&0x9000u16.to_le_bytes()); // SP
    f[25] = 1; // IM 1
    f[26] = 4; // border
               // PC $8000 on the stack at $9000; at $8000: JR $.
    f[27 + 0x5000] = 0x00;
    f[27 + 0x5001] = 0x80;
    f[27 + 0x4000] = 0x18;
    f[27 + 0x4001] = 0xfe;
    m.media_slot(SLOT_SNAPSHOT)
        .unwrap()
        .insert(media(MediaKind::Program, f))
        .unwrap();
    frames(&mut m, 1);
    assert_eq!(m.cpu.pc, 0x8000);
    assert_eq!(m.cpu.sp, 0x9002);
    assert_eq!(m.ula.border, 4);
}

#[test]
fn the_tape_trap_copies_a_block() {
    let mut m = stub("48k");
    // The ROM's LD-BYTES entry and a RET at SA/LD-RET.
    let rom = &mut m.mem.roms[0];
    rom[usize::from(ROM_LD_BYTES)..usize::from(ROM_LD_BYTES) + 4].copy_from_slice(&LD_BYTES_CODE);
    rom[usize::from(ROM_SA_LD_RET)] = 0xc9;
    // A data block: flag $FF, three bytes, checksum.
    let data = [0xffu8, 1, 2, 3];
    let sum = data.iter().fold(0, |a, b| a ^ b);
    let mut tap = vec![5, 0];
    tap.extend_from_slice(&data);
    tap.push(sum);
    m.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(media(MediaKind::Tape, tap))
        .unwrap();
    // CALL LD-BYTES from $8000 with A = $FF, IX = $9000, DE = 3, carry.
    m.mem.write(0x8000, 0xcd);
    m.mem.write(0x8001, 0x56);
    m.mem.write(0x8002, 0x05);
    m.cpu.pc = 0x8000;
    m.cpu.sp = 0xa000;
    m.cpu.a = 0xff;
    m.cpu.f = 0x01;
    m.cpu.ix = 0x9000;
    m.cpu.set_de(3);
    m.step_cycle(); // CALL
    m.step_cycle(); // trap, then the RET at $053F
    assert_eq!(m.cpu.pc, 0x8003);
    assert_eq!(m.cpu.f & 1, 1, "loaded");
    assert_eq!(
        (
            peek_cpu(&m, 0x9000),
            peek_cpu(&m, 0x9001),
            peek_cpu(&m, 0x9002)
        ),
        (1, 2, 3)
    );
    assert_eq!(m.cpu.ix, 0x9003);
}

#[test]
fn audio_comes_at_48_khz() {
    let mut m = stub("128k");
    frames(&mut m, 50);
    let mut out = Vec::new();
    m.audio(&mut out);
    // 50 frames of 70908 T-states at 3.5469 MHz = 0.99958 s.
    assert!((47_900..=48_000).contains(&out.len()), "{}", out.len());
}
