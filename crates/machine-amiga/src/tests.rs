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
        ["agnus", "copper", "blitter", "denise", "paula", "cia_a", "cia_b"]
    );
}
