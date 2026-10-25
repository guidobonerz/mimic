use super::*;
use devices_input::Joystick;
use emu_core::input::{Dirs, InputEvent, MachineKey};
use emu_core::media::{Media, MediaBackend, MediaError};

struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        buf.copy_from_slice(&self.0[offset as usize..offset as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn program(bytes: &[u8]) -> Media {
    Media {
        name: "test.prg".into(),
        kind: MediaKind::Program,
        backend: Box::new(Bytes(bytes.to_vec())),
    }
}

fn stub_with(options: &[(&str, &str)], model: &str) -> Vic20 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    config.model = Some(model.into());
    Vic20::new(&config).unwrap()
}

fn stub() -> Vic20 {
    stub_with(&[], "pal")
}

fn run_cycles(m: &mut Vic20, n: u32) {
    for _ in 0..n {
        assert_eq!(m.step_cycle(), StopReason::StepDone);
    }
}

fn key_event(at: u64, k: MachineKey, down: bool) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: k,
            down,
        },
    }
}

fn exec_breakpoint(dbg: &mut DebugHooks, addr: u16) -> emu_core::BpId {
    dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: u32::from(addr),
    })
}

#[test]
fn stub_kernal_boots_and_takes_timer_interrupts() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let idle = exec_breakpoint(&mut dbg, stub::IDLE);
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(idle));
    assert_eq!(m.peek(CPU_SPACE, 0x0314, Size::Word), Some(0xe1c0));
    assert_eq!(m.io.via2.t1_latch, 0x4826);
    assert_eq!(m.peek(CPU_SPACE, 0x9005, Size::Byte), Some(0xf0));
    dbg.remove_breakpoint(idle);
    // VIA2 timer 1 reaches /IRQ and the handler acknowledges it.
    let handler = exec_breakpoint(&mut dbg, 0xe1c0);
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(handler));
    dbg.remove_breakpoint(handler);
    for _ in 0..3 {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

#[test]
fn memory_map_and_expansions() {
    let mut m = stub();
    // Unexpanded: $0400 and $2000 are open, $1000 is RAM, ROMs are read-only.
    for (addr, fitted) in [
        (0x0400, false),
        (0x1000, true),
        (0x2000, false),
        (0xa000, false),
    ] {
        assert!(m.poke(CPU_SPACE, addr, Size::Byte, 0x5a) || !fitted);
        let v = m.peek(CPU_SPACE, addr, Size::Byte).unwrap();
        assert_eq!(v == 0x5a, fitted, "${addr:04X}");
    }
    assert!(!m.poke(CPU_SPACE, 0xe000, Size::Byte, 0));
    assert!(!m.poke(CPU_SPACE, 0x8000, Size::Byte, 0));

    let mut m = stub_with(&[("ram", "all")], "pal");
    for addr in [0x0400, 0x2000, 0x4000, 0x6000, 0xa000] {
        assert!(m.poke(CPU_SPACE, addr, Size::Byte, 0xa5));
        assert_eq!(m.peek(CPU_SPACE, addr, Size::Byte), Some(0xa5));
    }

    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("ram".into(), "64k".into());
    assert!(Vic20::new(&config).is_err());
}

#[test]
fn colour_ram_has_four_bits() {
    let mut m = stub();
    assert!(m.poke(CPU_SPACE, 0x9600, Size::Byte, 0xf7));
    assert_eq!(m.mem.color[0x200], 0x07);
    let v = m.peek(CPU_SPACE, 0x9600, Size::Byte).unwrap();
    assert_eq!(v & 0x0f, 0x07);
}

#[test]
fn vias_are_selected_by_a4_and_a5() {
    let mut m = stub();
    assert!(m.poke(CPU_SPACE, 0x9112, Size::Byte, 0x12)); // VIA1 DDRB
    assert!(m.poke(CPU_SPACE, 0x9122, Size::Byte, 0x34)); // VIA2 DDRB
    assert_eq!(m.io.via1.ddrb, 0x12);
    assert_eq!(m.io.via2.ddrb, 0x34);
    // Mirrored every $40 in $9100–$93FF.
    assert_eq!(m.peek(CPU_SPACE, 0x9352, Size::Byte), Some(0x12));
    // VIC registers repeat every 16 bytes in $9000–$90FF.
    assert!(m.poke(CPU_SPACE, 0x90ff, Size::Byte, 0x2b));
    assert_eq!(m.peek(CPU_SPACE, 0x900f, Size::Byte), Some(0x2b));
}

#[test]
fn keyboard_through_via2() {
    let mut m = stub();
    assert!(m.poke(CPU_SPACE, 0x9122, Size::Byte, 0xff)); // columns out
    assert!(m.poke(CPU_SPACE, 0x9123, Size::Byte, 0x00)); // rows in
    let now = m.now().0;
    m.input(key_event(now, keyboard::keys::RETURN, true));
    run_cycles(&mut m, 1);
    assert!(m.poke(CPU_SPACE, 0x9120, Size::Byte, 0xfd)); // column 1
    assert_eq!(m.peek(CPU_SPACE, 0x9121, Size::Byte), Some(0x7f));
    assert!(m.poke(CPU_SPACE, 0x9120, Size::Byte, 0xfe)); // column 0
    assert_eq!(m.peek(CPU_SPACE, 0x9121, Size::Byte), Some(0xff));
}

#[test]
fn joystick_lines_on_both_vias() {
    let mut m = stub();
    let now = m.now().0;
    m.input(TimedInput {
        at: Cycle(now),
        target: InputTarget::Port(CONTROL_PORT),
        event: InputEvent::Digital {
            dirs: Dirs::UP | Dirs::RIGHT,
            buttons: 1,
        },
    });
    run_cycles(&mut m, 1);
    assert!(m.poke(CPU_SPACE, 0x9113, Size::Byte, 0x00));
    let pa = m.peek(CPU_SPACE, 0x9111, Size::Byte).unwrap();
    assert_eq!(pa & 0x3c, 0x18, "up and fire low, down and left high");
    assert!(m.poke(CPU_SPACE, 0x9122, Size::Byte, 0x00)); // PB all inputs
    assert_eq!(
        m.peek(CPU_SPACE, 0x9120, Size::Byte),
        Some(0x7f),
        "right on PB7"
    );
}

#[test]
fn restore_key_raises_an_nmi() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    let nmi = exec_breakpoint(&mut dbg, 0xe1e8);
    let now = m.now().0;
    m.input(key_event(now + 10, keyboard::RESTORE, true));
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(nmi));
}

#[test]
fn serial_bus_lines_from_the_vias() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    assert_eq!(m.io.iec, drives_cbm::IecLines::default(), "all released");
    // ATN on: VIA1 PA7 high through the inverter.
    assert!(m.poke(CPU_SPACE, 0x911f, Size::Byte, 0x80));
    // CLK and DATA on: CA2 and CB2 manual high.
    assert!(m.poke(CPU_SPACE, 0x912c, Size::Byte, 0xfe));
    run_cycles(&mut m, 1);
    assert!(m.io.iec.atn && m.io.iec.clk && m.io.iec.data);
    // The inputs read the line levels.
    assert_eq!(m.peek(CPU_SPACE, 0x9111, Size::Byte).unwrap() & 0x03, 0);
}

#[test]
fn savestate_resumes_identically() {
    let mut a = stub_with(&[("ram", "8k")], "pal");
    run_cycles(&mut a, 23_456);
    a.mem.color[9] = 3;
    a.mem.ram[0x2345] = 7;
    let state = a.save_state();
    let mut b = stub_with(&[("ram", "8k")], "pal");
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    b.run_frame(&mut dbg);
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.framebuffer().pixels, b.framebuffer().pixels);

    let before = b.save_state();
    assert!(b.load_state(&state[..state.len() - 1]).is_err());
    assert_eq!(
        b.save_state(),
        before,
        "failed load leaves the machine unchanged"
    );
    assert!(stub().load_state(&state).is_err(), "other RAM expansion");
    assert!(stub_with(&[("ram", "8k")], "ntsc")
        .load_state(&state)
        .is_err());
}

#[test]
fn pal_and_ntsc_frames() {
    for (model, width, lines, cycles) in [("pal", 284, 312, 71), ("ntsc", 260, 261, 65)] {
        let mut m = stub_with(&[], model);
        let mut dbg = DebugHooks::new();
        let start = m.now().0;
        m.run_frame(&mut dbg);
        let t = m.now().0;
        m.run_frame(&mut dbg);
        assert_eq!(m.now().0 - t, lines * cycles, "{model}");
        assert!(t - start <= lines * cycles);
        let f = m.framebuffer();
        assert_eq!(f.width, width);
        assert_eq!(f.signal.lines_per_field, lines as u32);
    }
}

#[test]
fn program_slot_relocates_basic_and_loads_code_absolute() {
    let mut m = stub_with(&[("ram", "8k")], "pal");
    // What the KERNAL would leave: BASIC at $1201.
    m.mem.ram[0x2b..0x2d].copy_from_slice(&[0x01, 0x12]);
    // 10 PRINT 1 saved on an unexpanded machine ($1001).
    let basic = [
        0x01, 0x10, 0x0b, 0x10, 0x0a, 0x00, 0x99, 0x20, 0x31, 0x00, 0x00, 0x00,
    ];
    m.media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(program(&basic))
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..=BOOT_FRAMES {
        m.run_frame(&mut dbg);
    }
    assert_eq!(&m.mem.ram[0x1201..0x1203], &[0x09, 0x12], "relinked");
    assert_eq!(&m.mem.ram[0x2d..0x2f], &[0x0b, 0x12], "VARTAB");
    assert!(!m.input.is_empty(), "RUN is being typed");

    let code = [0x00, 0x14, 0xa9, 0x01, 0x60];
    m.media_slot(SLOT_PROGRAM)
        .unwrap()
        .insert(program(&code))
        .unwrap();
    m.run_frame(&mut dbg);
    assert_eq!(&m.mem.ram[0x1400..0x1403], &[0xa9, 0x01, 0x60]);
}

#[test]
fn ports_accept_only_control_port_devices() {
    let mut m = stub();
    assert_eq!(m.ports().len(), 1);
    assert!(matches!(
        m.connect(CONTROL_PORT, Box::new(Joystick::new(PortKind::CbmControl))),
        Err(PortError::Occupied(_))
    ));
    assert!(m.disconnect(CONTROL_PORT).is_some());
    assert!(matches!(
        m.connect(CONTROL_PORT, Box::new(Joystick::new(PortKind::AtariJoy))),
        Err(PortError::Incompatible { .. })
    ));
    assert!(matches!(
        m.connect(PortId(1), Box::new(Joystick::new(PortKind::CbmControl))),
        Err(PortError::NoSuchPort(_))
    ));
}

#[test]
fn debugger_view() {
    let m = stub();
    let spaces = m.memory_spaces();
    assert_eq!(spaces.len(), 1, "no drive without a DOS ROM");
    let chips = m.chips();
    let names: Vec<_> = chips.iter().map(|c| c.name).collect();
    assert_eq!(names, ["vic", "via1", "via2"]);
    assert_eq!(chips[0].regs.len(), 16);
    let instr = m.disassemble(0xe000);
    assert_eq!(instr.mnemonic, "SEI");
}

#[test]
fn paddles_turn_the_vic_pots() {
    use emu_core::input::{InputTarget, TimedInput};
    let mut m = stub_with(&[("port1", "paddles")], "pal");
    // Mid travel from the start.
    assert_eq!(m.peek(CPU_SPACE, 0x9008, Size::Byte), Some(0x80));
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(CONTROL_PORT),
        event: InputEvent::Analog {
            axis: 1,
            value: 0x2000,
        },
    });
    m.run_frame(&mut DebugHooks::new());
    assert_eq!(m.peek(CPU_SPACE, 0x9009, Size::Byte), Some(0x20));
    assert!(Vic20::new(&{
        let mut c = MachineConfig::default();
        c.options.insert("roms".into(), "stub".into());
        c.options.insert("port1".into(), "mouse".into());
        c
    })
    .is_err());
}

#[test]
fn graphics_views_follow_the_vic() {
    let mut m = stub();
    let ids: Vec<_> = m.gfx_views().iter().map(|v| v.id).collect();
    assert_eq!(ids, ["charset", "screen", "palette"]);
    // The KERNAL's unexpanded layout: 22 × 23 at $1E00, characters $8000.
    for (reg, v) in [(2u16, 0x96u8), (3, 0x2e), (5, 0xf0), (15, 0x1b)] {
        assert!(m.poke(CPU_SPACE, 0x9000 + u32::from(reg), Size::Byte, u32::from(v)));
    }
    assert!(m.poke(CPU_SPACE, 0x1e00, Size::Byte, 0x41));
    assert!(m.poke(CPU_SPACE, 0x9600, Size::Byte, 2));
    m.mem.roms.chargen[0x41 * 8] = 0x80;
    let sc = m.gfx_view("screen", &GfxOptions::new()).unwrap();
    assert_eq!(sc.addr, 0x1e00);
    assert_eq!((sc.width, sc.height), (22 * 8, 23 * 8));
    assert_eq!(sc.param("charbase"), Some(0x8000));
    assert_eq!(sc.raw.len(), 2 * 22 * 23);
    assert_eq!(sc.frame().rgb_at(0, 0), Some(m.palette[2]));
    assert_eq!(sc.frame().rgb_at(1, 0), Some(m.palette[1]));
    let cs = m.gfx_view("charset", &GfxOptions::new()).unwrap();
    assert_eq!(cs.addr, 0x8000);
    assert_eq!(cs.raw[0x41 * 8], 0x80);
    let ram = m
        .gfx_view("charset", &GfxOptions::new().with("addr", 0x1000))
        .unwrap();
    assert_eq!((ram.addr, ram.param("charbase")), (0x1000, Some(0x1000)));
    assert_eq!(
        m.gfx_view("palette", &GfxOptions::new()).unwrap().raw.len(),
        16
    );
    assert!(m.gfx_view("sprites", &GfxOptions::new()).is_none());
}

#[test]
fn frame_rows_follow_the_vic() {
    let m = stub();
    let t = m.io.vic.timing();
    assert_eq!(m.frame_row(t.first_line), Some(0));
    assert_eq!(m.frame_row(t.first_line - 1), None);
    assert_eq!(m.frame_row(t.lines), None);
    assert!(m.overlays().is_empty());
}
