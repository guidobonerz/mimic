//! The TI-99/4A with the stub ROM: boot, the VDP interrupt through the
//! 9901, the picture, savestates, cartridge banking, the keyboard matrix.

use super::*;

/// Bytes in memory as a medium.
struct Bytes(Vec<u8>);

impl emu_core::media::MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.0[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn pixels(m: &Ti99) -> Vec<u8> {
    match m.framebuffer().pixels {
        PixelData::Indexed { pixels, .. } => pixels.to_vec(),
        _ => panic!("indexed"),
    }
}

fn stub() -> Ti99 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    Ti99::new(&config).unwrap()
}

fn frames(m: &mut Ti99, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn the_stub_counts_fields_in_its_interrupt() {
    let mut m = stub();
    // Clearing 16 KB of VRAM a byte at a time takes the stub ~17 fields.
    frames(&mut m, 30);
    let before = peek_cpu(&m, stub::FRAMES);
    frames(&mut m, 10);
    let n = peek_cpu(&m, stub::FRAMES) - before;
    assert_eq!(n, 10, "one interrupt a field");
    // About 50 000 CPU cycles a field at 60 Hz.
    let per = m.now().0 / 40;
    assert!((49_000..51_000).contains(&per), "{per} cycles a field");
}

#[test]
fn the_stub_paints_the_top_half_white() {
    let mut m = stub();
    frames(&mut m, 30);
    let f = m.framebuffer();
    let PixelData::Indexed { pixels, .. } = f.pixels else {
        panic!("indexed")
    };
    let w = f.width as usize;
    let at = |x: usize, y: usize| pixels[y * w + x];
    // Active area: 256 × 192 inside the borders; find white (15) above,
    // dark blue (4) below.
    let column = w / 2;
    let rows: Vec<u8> = (0..f.height as usize).map(|y| at(column, y)).collect();
    let white = rows.iter().filter(|&&c| c == 15).count();
    let blue = rows.iter().filter(|&&c| c == 4).count();
    assert_eq!(white, 96, "the top half of the 192 lines");
    // The bottom half and the borders (backdrop colour 4).
    assert!(blue > 96, "{blue} dark blue lines");
}

#[test]
fn pal_has_more_lines_per_field() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.video_format = Some(VideoStandard::Pal);
    let mut m = Ti99::new(&config).unwrap();
    frames(&mut m, 5);
    let per = m.now().0 / 5;
    assert!(
        (59_000..61_000).contains(&per),
        "{per} cycles a field at 50 Hz"
    );
}

#[test]
fn savestate_round_trip_is_deterministic() {
    let mut a = stub();
    frames(&mut a, 3);
    let state = a.save_state();
    frames(&mut a, 3);
    let mut b = stub();
    b.load_state(&state).unwrap();
    frames(&mut b, 3);
    assert_eq!(pixels(&a), pixels(&b));
    assert_eq!(a.now(), b.now());
    assert!(b.load_state(&state[..40]).is_err());
}

#[test]
fn the_memory_map_and_wait_states() {
    let mut m = stub();
    // Scratchpad mirrors at >8000, >8100, >8200, >8300.
    m.board.mem_write(0x8304, 0x1234);
    assert_eq!(peek_cpu(&m, 0x8004), 0x1234);
    m.board.mem_write(0xa000, 0xbeef);
    m.board.mem_write(0x2000, 0xcafe);
    assert_eq!(
        (peek_cpu(&m, 0xa000), peek_cpu(&m, 0x2000)),
        (0xbeef, 0xcafe)
    );
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("ram".into(), "none".into());
    let mut bare = Ti99::new(&config).unwrap();
    bare.board.mem_write(0xa000, 0xbeef);
    assert_eq!(peek_cpu(&bare, 0xa000), 0);
    let ports = [None, None];
    let mut tape = AppleTape::with_motor_control(CPU_HZ);
    let mut bus = TiBus {
        b: &mut m.board,
        ports: &ports,
        tape: &mut tape,
        disk: &mut None,
        dbg: None,
        beam: BeamPos::default(),
    };
    assert_eq!(bus.wait_states(0x0000), 0);
    assert_eq!(bus.wait_states(0x8300), 0);
    assert_eq!(bus.wait_states(0x8800), 4);
    assert_eq!(bus.wait_states(0xa000), 4);
}

#[test]
fn cartridge_banks_switch_by_writes() {
    let mut m = stub();
    let cart = TiCart {
        rom: vec![0x11; 0x2000],
        rom2: vec![0x22; 0x2000],
        grom: vec![0x33; 0x1800],
    };
    m.cart
        .insert(Media {
            name: "x.ticart".into(),
            kind: MediaKind::Cartridge,
            backend: Box::new(Bytes(cart.to_bytes())),
        })
        .unwrap();
    frames(&mut m, 1);
    assert_eq!(peek_cpu(&m, 0x6000), 0x1111);
    m.board.mem_write(0x6002, 0);
    assert_eq!(peek_cpu(&m, 0x6000), 0x2222);
    m.board.mem_write(0x6000, 0);
    assert_eq!(peek_cpu(&m, 0x6000), 0x1111);
    assert_eq!(m.board.grom.byte(0x6000), 0x33, "cartridge GROM 3");
}

#[test]
fn keyboard_rows_reach_the_9901() {
    let mut m = stub();
    // Column 5 (P2–P4 = 1, 0, 1), key A = column 5 row 5.
    m.board.keyboard.key(45, true);
    m.board.pio.write_bit(18, true);
    m.board.pio.write_bit(19, false);
    m.board.pio.write_bit(20, true);
    m.board.update_pio();
    assert!(!m.board.pio.read_bit(3 + 5), "row 5 pulled low");
    assert!(m.board.pio.read_bit(3 + 4));
    // Alpha lock (on by default) on row 4 with P5 low.
    m.board.pio.write_bit(21, false);
    m.board.update_pio();
    assert!(!m.board.pio.read_bit(3 + 4));
}
