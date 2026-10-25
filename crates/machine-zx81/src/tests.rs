use super::*;
use emu_core::input::MachineKey;
use emu_core::media::MediaBackend;
use std::sync::{Arc, Mutex};

fn stub(ram: &str) -> Zx81 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("ram".into(), ram.into());
    Zx81::new(&config).unwrap()
}

fn frames(m: &mut Zx81, n: usize) {
    let mut dbg = DebugHooks::default();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn pixel(m: &Zx81, x: u32, y: u32) -> u8 {
    m.ula.pixels()[(y * ula::WIDTH + x) as usize]
}

#[test]
fn stub_shows_24_rows_of_boxes_with_white_borders() {
    let mut m = stub("16k");
    frames(&mut m, 4);
    assert!(
        peek_cpu(&m, stub::FRAMES) >= 3,
        "fields counted by the stub"
    );
    // The display file: HALT, then rows of 24 codes and a HALT.
    assert_eq!(peek_cpu(&m, 0x4000), 0x76);
    assert_eq!(peek_cpu(&m, 0x4001 + 24), 0x76);
    // Seven blank rows, then 24 rows of boxes: lines 56-247, each row 8
    // lines of 24 characters at 8 pixels. Find the first black pixel.
    let (x0, y0) = (0..ula::HEIGHT)
        .flat_map(|y| (0..ula::WIDTH).map(move |x| (x, y)))
        .find(|&(x, y)| pixel(&m, x, y) == 1)
        .expect("black pixels");
    assert_eq!(y0, 56 - ula::V_OFFSET, "first row at line 56");
    let width = stub::COLUMNS * 8;
    assert!(x0 + width <= ula::WIDTH, "row inside the window: {x0}");
    // A box: the top line is 8 black pixels per character.
    assert!(
        (x0..x0 + width).all(|x| pixel(&m, x, y0) == 1),
        "row 0 of every character is solid"
    );
    assert_eq!(pixel(&m, x0 + width, y0), 0, "then the border");
    assert_eq!(pixel(&m, x0 - 1, y0), 0);
    // Row 1 of the pattern is $81: only the outer pixels of each character.
    assert_eq!(pixel(&m, x0, y0 + 1), 1);
    assert_eq!(pixel(&m, x0 + 1, y0 + 1), 0);
    assert_eq!(pixel(&m, x0 + 7, y0 + 1), 1);
    assert_eq!(pixel(&m, x0 + 8, y0 + 1), 1, "next character's left edge");
    // 24 rows of 8 lines, then the blank rows.
    assert_eq!(pixel(&m, x0, y0 + stub::ROWS * 8 - 1), 1);
    assert_eq!(pixel(&m, x0, y0 + stub::ROWS * 8), 0);
    // Every line of the block starts at the same column: no jitter.
    for y in y0..y0 + stub::ROWS * 8 {
        assert_eq!(pixel(&m, x0, y), 1, "line {y}");
        assert_eq!(pixel(&m, x0 - 1, y), 0, "line {y}");
    }
    // Two frames in a row are identical.
    let a = m.ula.pixels().to_vec();
    frames(&mut m, 1);
    assert_eq!(m.ula.pixels(), &a[..]);
}

#[test]
fn a_field_is_320_lines_of_207_t_states_for_the_stub() {
    let mut m = stub("16k");
    frames(&mut m, 2);
    let t0 = m.now().0;
    frames(&mut m, 1);
    let t = m.now().0 - t0;
    // 38 rows of 8 lines plus the VSYNC pause (about 16 lines).
    assert!(
        (318..=324).contains(&(t / u64::from(ula::T_PER_LINE))),
        "{t} T-states"
    );
}

#[test]
fn keyboard_rows_reach_port_fe() {
    let mut m = stub("16k");
    let at = m.now();
    for (key, down) in [
        (keyboard::keys::SHIFT, true),
        (MachineKey(keyboard::key(2, 3)), true),
    ] {
        m.input(TimedInput {
            at,
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: key,
                down,
            },
        });
    }
    frames(&mut m, 1);
    let t = &m as &dyn DebugTarget;
    let read = |port: u16| {
        t.peek(CPU_SPACE, IO_BASE + u32::from(port), Size::Byte)
            .unwrap() as u8
    };
    assert_eq!(read(0xfefe) & 0x1f, 0x1e, "SHIFT on row 0");
    assert_eq!(read(0xfbfe) & 0x1f, 0x17, "R on row 2");
    assert_eq!(read(0xfafe) & 0x1f, 0x16, "both rows selected");
    assert_eq!(read(0xfffe) & 0x1f, 0x1f, "no row");
    assert_eq!(read(0xfefe) & 0x40, 0x40, "50 Hz");
}

#[test]
fn options_and_models() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    assert_eq!(Zx81::new(&config).unwrap().mem.ram.len(), 0x4000);
    config.model = Some("ts1000".into());
    assert_eq!(Zx81::new(&config).unwrap().model(), Model::Ts1000);
    config.model = Some("spectrum".into());
    assert!(Zx81::new(&config).is_err());
    config.model = None;
    config.options.insert("ram".into(), "48k".into());
    assert!(Zx81::new(&config).is_err());
    let config = MachineConfig::default();
    assert!(matches!(
        Zx81::new(&config),
        Err(ConfigError::MissingRom(_))
    ));
}

#[test]
fn savestate_round_trip() {
    let mut m = stub("16k");
    frames(&mut m, 2);
    for _ in 0..300 {
        m.step_cycle();
    }
    let s = m.save_state();
    let mut n = stub("16k");
    n.load_state(&s).unwrap();
    frames(&mut m, 2);
    frames(&mut n, 2);
    assert_eq!(m.now(), n.now());
    assert_eq!(m.cpu.regs(), n.cpu.regs());
    assert_eq!(m.ula.pixels(), n.ula.pixels());
    assert!(n.load_state(&s[..s.len() - 5]).is_err());
    let mut other = stub("1k");
    assert!(other.load_state(&s).is_err(), "RAM size is configuration");
}

/// A backend over shared bytes.
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
        d.resize(offset as usize + data.len(), 0);
        d[offset as usize..].copy_from_slice(data);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

#[test]
fn load_and_save_traps_move_the_program_area() {
    let mut m = stub("16k");
    let bytes = Arc::new(Mutex::new((0..100u8).collect::<Vec<_>>()));
    m.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(Media {
            name: "test.p".into(),
            kind: MediaKind::Tape,
            backend: Box::new(Shared(bytes.clone())),
        })
        .unwrap();
    // Reach the LOAD entry: the trap fills $4009… and continues at SLOW/FAST.
    m.cpu.pc = ROM_LOAD;
    m.step_cycle();
    assert_eq!(peek_cpu(&m, PROGRAM_START), 0);
    assert_eq!(peek_cpu(&m, PROGRAM_START + 99), 99);
    // SAVE writes $4009 up to E_LINE back.
    m.mem.write(E_LINE, 0x20);
    m.mem.write(E_LINE + 1, 0x40); // E_LINE = $4020: 23 bytes
    m.mem.write(PROGRAM_START, 0xaa);
    m.cpu.pc = ROM_SAVE;
    m.step_cycle();
    m.media_slot(SLOT_TAPE).unwrap().flush().unwrap();
    let b = bytes.lock().unwrap();
    assert_eq!(b.len(), 0x4020 - 0x4009);
    assert_eq!(b[0], 0xaa);
    drop(b);
    // Without a tape the ROM code runs instead.
    assert!(m.media_slot(SLOT_TAPE).unwrap().eject().is_some());
    m.cpu.pc = ROM_LOAD;
    m.step_cycle();
    assert_ne!(m.cpu.pc, ROM_SLOW_FAST);
}

#[test]
fn frame_rows_and_beam_columns_match_the_picture_window() {
    let mut m = stub("16k");
    assert_eq!(m.frame_row(ula::V_OFFSET as u16), Some(0));
    assert_eq!(m.frame_row(ula::V_OFFSET as u16 - 1), None);
    assert_eq!(
        m.frame_row((ula::V_OFFSET + ula::HEIGHT - 1) as u16),
        Some(ula::HEIGHT - 1)
    );
    assert_eq!(m.frame_row((ula::V_OFFSET + ula::HEIGHT) as u16), None);
    m.ula.hcount = ula::H_OFFSET + 3;
    m.ula.line = ula::V_OFFSET + 7;
    let b = m.beam().unwrap();
    assert_eq!((b.hpos, m.frame_row(b.line)), (6, Some(7)));
}
