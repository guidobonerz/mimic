use super::*;
use emu_core::ControlId;

fn machine() -> Imsai {
    Imsai::new(&MachineConfig::default()).unwrap()
}

fn frames(m: &mut Imsai, n: u32) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

/// Operate a control, as the panel strip does: applied in the next frame.
fn control(m: &mut Imsai, id: u16, value: u8) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Panel,
        event: InputEvent::Control {
            id: ControlId(id),
            value,
        },
    });
    frames(m, 1);
}

fn press(m: &mut Imsai, id: u16) {
    control(m, id, 1);
    control(m, id, 0);
}

fn set_switches(m: &mut Imsai, v: u16) {
    for bit in 0..16 {
        control(m, ctl::SWITCH + bit, (v >> bit & 1) as u8);
    }
}

/// Toggle a program in as the manual says: DEPOSIT, then DEPOSIT NEXT.
fn toggle_in(m: &mut Imsai, program: &[u8]) {
    press(m, ctl::RESET);
    for (i, &b) in program.iter().enumerate() {
        set_switches(m, u16::from(b));
        press(
            m,
            if i == 0 {
                ctl::DEPOSIT
            } else {
                ctl::DEPOSIT_NEXT
            },
        );
    }
    set_switches(m, 0);
    press(m, ctl::EXAMINE);
}

/// A click on the picture.
fn click(m: &mut Imsai, x: u16, y: u16, down: bool) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(PortId(0)),
        event: InputEvent::LightPen {
            x,
            y,
            trigger: down,
        },
    });
    frames(m, 1);
}

#[test]
fn stopped_at_power_on_with_the_manuals_status_lamps() {
    let mut m = machine();
    frames(&mut m, 2);
    assert!(!m.running());
    let l = m.shown_lamps();
    // MEMR, M1, /WO; WAIT on, RUN off.
    assert_eq!(l.status, [15, 0, 15, 0, 0, 0, 15, 0]);
    assert_eq!(l.run, [0, 0, 15, 0]);
    assert_eq!(l.address, [0; 16]);
    assert_eq!(l.output, [15; 8], "OUT $FF 0 lights all eight");
}

/// The manual's Test Program 1: IN FF, OUT FF, JMP 0. The lamps show the
/// inverse of the left switches, Test Program 2 adds CMA.
#[test]
fn manuals_test_program_echoes_the_switches() {
    let mut m = machine();
    let program = [0xdb, 0xff, 0xd3, 0xff, 0xc3, 0x00, 0x00];
    toggle_in(&mut m, &program);
    for (i, &b) in program.iter().enumerate() {
        assert_eq!(m.peek_mem(i as u16), b);
    }
    assert_eq!(m.pc(), 0);
    let high = 0xa5u16;
    set_switches(&mut m, high << 8);
    press(&mut m, ctl::RUN);
    frames(&mut m, 2);
    assert_eq!(m.board.output, 0xa5);
    let l = m.shown_lamps();
    assert_eq!(l.output, [0, 15, 0, 15, 15, 0, 15, 0]);
    assert_eq!(l.run[1], 15, "RUN");
    assert_eq!(l.run[2], 0, "WAIT");
    assert!(l.status[2] > 0 && l.status[1] > 0, "M1 and INP glow");
    press(&mut m, ctl::STOP);
    assert!(!m.running());
}

#[test]
fn examine_and_deposit_next_walk_memory() {
    let mut m = machine();
    toggle_in(&mut m, &[0x11, 0x22, 0x33]);
    set_switches(&mut m, 1);
    press(&mut m, ctl::EXAMINE);
    frames(&mut m, 1);
    assert_eq!(m.board.latch.1, 1);
    assert_eq!(m.board.latch.2, 0x22);
    press(&mut m, ctl::EXAMINE_NEXT);
    assert_eq!(m.board.latch.1, 2);
    assert_eq!(m.board.latch.2, 0x33);
    // Not while running.
    press(&mut m, ctl::RUN);
    set_switches(&mut m, 0x12);
    press(&mut m, ctl::DEPOSIT);
    assert_eq!(m.peek_mem(0x12), 0);
}

#[test]
fn single_step_runs_one_instruction() {
    let mut m = machine();
    toggle_in(&mut m, &[0x3e, 0x42, 0x00]); // MVI A,$42
    press(&mut m, ctl::STEP);
    assert_eq!(m.pc(), 2);
    assert_eq!(m.cpu.a, 0x42);
    assert!(!m.running());
}

/// Kill the Bit (Dean McDaniel, 1975): the bit runs over the address lamps
/// A15–A8 through LDAX D; the switches kill it.
#[test]
fn kill_the_bit_runs_over_the_address_lamps() {
    let program = [
        0x21, 0x00, 0x00, 0x16, 0x80, 0x01, 0x0e, 0x00, 0x1a, 0x1a, 0x1a, 0x1a, 0x09, 0xd2, 0x08,
        0x00, 0xdb, 0xff, 0xaa, 0x0f, 0x57, 0xc3, 0x08, 0x00,
    ];
    let mut m = machine();
    toggle_in(&mut m, &program);
    press(&mut m, ctl::RUN);
    let mut seen = [false; 8];
    for _ in 0..240 {
        frames(&mut m, 1);
        seen[m.cpu.d.trailing_zeros() as usize % 8] = true;
    }
    assert_eq!(seen, [true; 8], "the bit went round");
    let l = m.shown_lamps();
    assert!(l.address[..8].iter().any(|&v| v > 5), "A15–A8: {l:?}");
}

#[test]
fn clicks_on_the_picture_work_the_paddles() {
    let mut m = machine();
    // Left block, first paddle: A15; the middle block's last: A0.
    click(&mut m, 74, 290, true);
    click(&mut m, 74, 290, false);
    assert_eq!(m.board.switches, 0x8000);
    click(&mut m, 404 + 7 * 37, 290, true);
    click(&mut m, 404 + 7 * 37, 290, false);
    assert_eq!(m.board.switches, 0x8001);
    click(&mut m, 74, 290, true);
    click(&mut m, 74, 290, false);
    assert_eq!(m.board.switches, 0x0001);
    // EXAMINE up, then RUN up and held until release.
    click(&mut m, 722, 275, true);
    assert_eq!(m.controls_shown()[0], Paddle::Up);
    click(&mut m, 722, 275, false);
    assert_eq!(m.controls_shown()[0], Paddle::Middle);
    assert_eq!(m.pc(), 1);
    click(&mut m, 722 + 3 * 44, 275, true);
    click(&mut m, 722 + 3 * 44, 275, false);
    assert!(m.running());
    click(&mut m, 722 + 3 * 44, 320, true);
    click(&mut m, 722 + 3 * 44, 320, false);
    assert!(!m.running());
    // PWR off and on.
    click(&mut m, 722 + 5 * 44, 290, true);
    click(&mut m, 722 + 5 * 44, 290, false);
    assert_eq!(m.shown_lamps(), Lamps::default());
    click(&mut m, 722 + 5 * 44, 290, true);
    click(&mut m, 722 + 5 * 44, 290, false);
    assert!(m.power);
    // Off the paddles nothing happens.
    click(&mut m, 500, 30, true);
    click(&mut m, 500, 30, false);
    assert_eq!(m.board.switches, 1);
}

/// Echo on the SIO-2's channel A: wait for RxRDY, read, wait for TxRDY, send.
#[test]
fn terminal_echoes_through_the_sio2() {
    let program = [
        0xdb, 0x03, 0xe6, 0x02, 0xca, 0x00, 0x00, // IN 3, ANI 2, JZ 0
        0xdb, 0x02, 0x47, // IN 2, MOV B,A
        0xdb, 0x03, 0xe6, 0x01, 0xca, 0x0a, 0x00, // IN 3, ANI 1, JZ $0A
        0x78, 0xd3, 0x02, 0xc3, 0x00, 0x00, // MOV A,B, OUT 2, JMP 0
    ];
    let mut m = machine();
    toggle_in(&mut m, &program);
    press(&mut m, ctl::RUN);
    for b in b"HELLO\r\nOK" {
        m.type_byte(*b);
    }
    frames(&mut m, 4);
    assert_eq!(m.terminal().line(0), "HELLO");
    assert_eq!(m.terminal().line(1), "OK");
}

#[test]
fn keys_on_the_terminal_turn_into_ascii() {
    let mut m = machine();
    m.key(u16::from(b'a'), true);
    m.key(keys::CAPS, true);
    m.key(u16::from(b'a'), true);
    m.key(keys::SHIFT, true);
    m.key(u16::from(b'1'), true);
    m.key(keys::SHIFT, false);
    m.key(keys::CTRL, true);
    m.key(u16::from(b'c'), true);
    let typed: Vec<u8> = m.board.typed.iter().copied().collect();
    assert_eq!(typed, [b'A', b'a', b'!', 3]);
}

/// All five columns of the font reach the screen: H has both its stems.
#[test]
fn terminal_draws_whole_letters() {
    let mut t = Terminal::new();
    t.put(b'H');
    t.put(b' ');
    t.render(false);
    let px = t.pixels();
    let row: Vec<u8> = (0..8)
        .map(|x| px[3 * terminal::WIDTH as usize + x])
        .collect();
    assert_eq!(row, [0, 1, 0, 0, 0, 1, 0, 0]);
}

#[test]
fn terminal_control_codes_and_scrolling() {
    let mut t = Terminal::new();
    for b in b"AB\x08C\r\nD\x1b=\x22\x25E" {
        t.put(*b);
    }
    assert_eq!(t.line(0), "AC");
    assert_eq!(t.line(1), "D");
    assert_eq!(t.line(2), "     E");
    for _ in 0..30 {
        t.put(b'\n');
    }
    assert_eq!(t.line(0), "");
    t.put(0x1a);
    assert_eq!((t.col, t.row), (0, 0));
}

struct Mem(Vec<u8>);

impl emu_core::media::MediaBackend for Mem {
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

fn media(kind: MediaKind, bytes: &[u8]) -> Media {
    Media {
        name: "t".into(),
        kind,
        backend: Box::new(Mem(bytes.to_vec())),
    }
}

#[test]
fn program_slot_loads_binary_and_hex_and_returns_after_power() {
    let mut m = machine();
    m.media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(media(MediaKind::Program, &[1, 2, 3]))
        .unwrap();
    frames(&mut m, 1);
    assert_eq!(m.peek_mem(2), 3);
    control(&mut m, ctl::POWER, 0);
    control(&mut m, ctl::POWER, 1);
    frames(&mut m, 1);
    assert_eq!(m.peek_mem(2), 3, "loaded again");

    let hex = b":03010000AABBCCCB\r\n:00000001FF\r\n";
    let image = parse_hex(hex).unwrap();
    assert_eq!(image.start, 0x0100);
    assert_eq!(image.blocks, [(0x0100, vec![0xaa, 0xbb, 0xcc])]);
    assert!(parse_hex(b":03010000AABBCC00\r\n").is_none(), "checksum");

    let mut c = MachineConfig::default();
    c.options.insert("autostart".into(), "on".into());
    let mut m = Imsai::new(&c).unwrap();
    m.media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(media(MediaKind::Program, hex))
        .unwrap();
    frames(&mut m, 1);
    assert!(m.running());
}

#[test]
fn tape_reads_on_the_reader_ports_or_the_console() {
    let mut m = machine();
    m.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(media(MediaKind::Tape, b"XY"))
        .unwrap();
    frames(&mut m, 2);
    assert_eq!(m.board.io_peek(0x06) & 1, 0, "88-ACR: byte waiting");
    assert_eq!(m.board.io_read(0x07), b'X');
    frames(&mut m, 2);
    assert_eq!(m.board.io_read(0x05) & 2, 2, "SIO-2 B: RxRDY");
    assert_eq!(m.board.io_read(0x04), b'Y');

    let mut c = MachineConfig::default();
    c.options.insert("reader".into(), "console".into());
    let mut m = Imsai::new(&c).unwrap();
    m.media_slot(SLOT_TAPE)
        .unwrap()
        .insert(media(MediaKind::Tape, b"Z"))
        .unwrap();
    frames(&mut m, 2);
    assert_eq!(m.board.io_read(0x02), b'Z');
}

#[test]
fn mpu_rom_maps_at_zero_and_d800_until_port_f3_removes_it() {
    let mut rom = vec![0u8; 0x800];
    // OUT $F3 with $40, then JMP $D000 (RAM) with the copy at $D800 mapped.
    rom[..7].copy_from_slice(&[0x3e, 0x40, 0xd3, 0xf3, 0xc3, 0x00, 0xd8]);
    let mut c = MachineConfig::default();
    c.options.insert("mpu".into(), "b".into());
    assert!(Imsai::new(&c).is_err(), "ROM missing");
    c.roms.insert("mpu-b".into(), rom);
    let mut m = Imsai::new(&c).unwrap();
    assert_eq!(m.peek_mem(0), 0x3e);
    assert_eq!(m.peek_mem(0xd800), 0x3e);
    m.board.mem_write(0xd800, 0x12);
    assert_eq!(m.peek_mem(0xd800), 0x3e, "ROM");
    press(&mut m, ctl::STEP);
    press(&mut m, ctl::STEP);
    assert_eq!(m.peek_mem(0), 0, "low copy gone");
    assert_eq!(m.peek_mem(0xd800), 0x3e, "high copy stays");
    m.board.io_write(0xf3, 0xc0);
    assert_eq!(m.peek_mem(0xd800), 0);
    press(&mut m, ctl::RESET);
    assert_eq!(m.peek_mem(0), 0x3e, "RESET maps it in again");
    assert_eq!(m.board.io_peek(0x15), 0, "no parallel port");
}

#[test]
fn ram_size_and_protect() {
    let mut c = MachineConfig::default();
    c.options.insert("ram".into(), "4k".into());
    let mut m = Imsai::new(&c).unwrap();
    m.board.mem_write(0x0fff, 1);
    m.board.mem_write(0x1000, 1);
    assert_eq!(m.peek_mem(0x0fff), 1);
    assert_eq!(m.peek_mem(0x1000), 0xff);
    m.board.protect = 1;
    m.board.mem_write(0x0010, 9);
    assert_eq!(m.peek_mem(0x0010), 0);
    c.options.insert("ram".into(), "128k".into());
    assert!(Imsai::new(&c).is_err());
}

#[test]
fn savestate_round_trip_runs_on_identically() {
    let mut a = machine();
    toggle_in(&mut a, &[0xdb, 0xff, 0xd3, 0xff, 0x3c, 0xc3, 0x00, 0x00]);
    press(&mut a, ctl::RUN);
    a.type_byte(b'Q');
    frames(&mut a, 3);
    let state = a.save_state();
    let mut b = machine();
    b.load_state(&state).unwrap();
    frames(&mut a, 5);
    frames(&mut b, 5);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.panel.pixels(), b.panel.pixels());
    assert!(b.load_state(&state[..20]).is_err());
}

#[test]
fn debugger_steps_while_stopped_and_sees_the_screens() {
    let mut m = machine();
    toggle_in(&mut m, &[0x00, 0x00]);
    assert_eq!(m.step_cycle(), StopReason::StepDone);
    assert_eq!(m.pc(), 1);
    assert_eq!(m.screens().len(), 2);
    let t = m.screen(TERMINAL_SCREEN).unwrap();
    assert_eq!((t.width, t.height), (640, 384));
    assert!(m.screen(ScreenId(7)).is_none());
    assert_eq!(m.controls().len(), 26);
    assert!(m.controls().iter().all(|c| c.on_picture));
    assert_eq!(m.disassemble(0).mnemonic, "NOP");
    assert_eq!(
        DebugTarget::peek(&m, CPU_SPACE, IO_BASE + 0xff, Size::Byte),
        Some(0)
    );
}
