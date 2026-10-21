use super::*;
use crate::memory::KERNAL_SIZE;
use devices_input::{Joystick, Mouse1351};

/// Config with test ROMs (not Commodore ROMs): `code` at $E000 is the reset
/// entry, `irq` at $E100 is the IRQ handler, `nmi` at $E200 the NMI handler.
fn config_with(code: &[u8], irq: &[u8], nmi: &[u8]) -> MachineConfig {
    let mut kernal = vec![0xea; KERNAL_SIZE];
    kernal[..code.len()].copy_from_slice(code);
    kernal[0x100..0x100 + irq.len()].copy_from_slice(irq);
    kernal[0x200..0x200 + nmi.len()].copy_from_slice(nmi);
    kernal[0x1ffa..].copy_from_slice(&[0x00, 0xe2, 0x00, 0xe0, 0x00, 0xe1]);
    let mut config = MachineConfig::default();
    config.roms.insert("kernal".into(), kernal);
    config.roms.insert("basic".into(), vec![0xb0; BASIC_SIZE]);
    config
        .roms
        .insert("chargen".into(), vec![0xc0; CHARGEN_SIZE]);
    config
}

fn machine(code: &[u8], irq: &[u8], nmi: &[u8]) -> C64 {
    C64::new(&config_with(code, irq, nmi)).unwrap()
}

fn stub() -> C64 {
    stub_model("pal")
}

fn stub_model(model: &str) -> C64 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.model = Some(model.into());
    C64::new(&config).unwrap()
}

fn run_cycles(m: &mut C64, n: u32) {
    for _ in 0..n {
        assert_eq!(m.step_cycle(), StopReason::StepDone);
    }
}

#[test]
fn ddr_before_data_latch_banks_everything_out() {
    // The pitfall the real KERNAL avoids by writing $01 before $00.
    let code = [0xa9, 0x2f, 0x85, 0x00, 0xea, 0xea];
    let mut m = machine(&code, &[0x40], &[0x40]);
    run_cycles(&mut m, 7 + 2 + 3);
    assert_eq!(m.mem.mode(), 24, "outputs low: all RAM");
    assert_eq!(m.peek(CPU_SPACE, 0xe004, Size::Byte), Some(0));
}

#[test]
fn stub_kernal_boots_and_sets_up_the_port() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let idle = dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: u32::from(stub::IDLE),
    });
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(idle));
    assert_eq!(m.peek(CPU_SPACE, 0, Size::Byte), Some(0x2f));
    // IOINIT leaves $01 = $E7: outputs 0–2 and 5 high, bit 4 input pulled up.
    assert_eq!(m.peek(CPU_SPACE, 1, Size::Byte), Some(0x37));
    assert_eq!(m.mem.mode(), 31);
    assert_eq!(
        m.peek(RAM_SPACE, 0x0314, Size::Word),
        Some(0xea31),
        "RESTOR"
    );
    assert_eq!(m.io.cia1.ta.latch, 0x4025, "IOINIT: 1/60 s IRQ timer");
    // The CIA1 timer interrupt runs through the KERNAL entry and returns.
    dbg.remove_breakpoint(idle);
    let entry = dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: 0xea31,
    });
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(entry));
}

#[test]
fn rom_configuration_errors() {
    let empty = MachineConfig::default();
    assert!(matches!(C64::new(&empty), Err(ConfigError::MissingRom(r)) if r == "kernal"));

    let mut config = config_with(&[], &[], &[]);
    config.roms.insert("basic".into(), vec![0; 100]);
    assert!(matches!(C64::new(&config), Err(ConfigError::Invalid(_))));

    let mut config = config_with(&[], &[], &[]);
    config.model = Some("secam".into());
    assert!(matches!(
        C64::new(&config),
        Err(ConfigError::UnknownModel(_))
    ));

    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "fake".into());
    assert!(matches!(C64::new(&config), Err(ConfigError::Invalid(_))));
}

#[test]
fn banking_from_the_cpu() {
    let code = [
        0xa9, 0x37, 0x85, 0x01, // LDA #$37; STA $01 (latch before DDR)
        0xa9, 0x2f, 0x85, 0x00, // LDA #$2F; STA $00
        0xa9, 0x99, 0x8d, 0x00, 0xa0, // LDA #$99; STA $A000 (under BASIC)
        0xad, 0x00, 0xa0, 0x85, 0x10, // LDA $A000; STA $10  (BASIC byte)
        0xa9, 0x36, 0x85, 0x01, // LDA #$36; STA $01      (BASIC out)
        0xad, 0x00, 0xa0, 0x85, 0x11, // LDA $A000; STA $11  (RAM byte)
        0xa9, 0x33, 0x85, 0x01, // LDA #$33; STA $01      (char ROM in)
        0xad, 0x00, 0xd0, 0x85, 0x12, // LDA $D000; STA $12
        0x4c, 0x24, 0xe0, // JMP *
    ];
    let mut m = machine(&code, &[0x40], &[0x40]);
    m.run_frame(&mut DebugHooks::new());
    assert_eq!(m.regs().get("PC"), Some(0xe024));
    let zp = |a| m.peek(RAM_SPACE, a, Size::Byte).unwrap();
    assert_eq!([zp(0x10), zp(0x11), zp(0x12)], [0xb0, 0x99, 0xc0]);
    assert_eq!(m.mem.mode(), 27);
}

#[test]
fn colour_ram_has_four_bits() {
    let code = [
        0xa9, 0xff, 0x8d, 0x00, 0xd8, // LDA #$FF; STA $D800
        0xad, 0x00, 0xd8, 0x85, 0x10, // LDA $D800; STA $10
        0x4c, 0x0a, 0xe0, // JMP *
    ];
    let mut m = machine(&code, &[0x40], &[0x40]);
    m.run_frame(&mut DebugHooks::new());
    assert_eq!(m.peek(COLOR_SPACE, 0, Size::Byte), Some(0x0f));
    // Upper nibble: open bus (the VIC-II's last fetch at the time of the read).
    assert_eq!(
        m.peek(RAM_SPACE, 0x10, Size::Byte).map(|v| v & 0x0f),
        Some(0x0f)
    );
    assert_eq!(
        m.peek(RAM_SPACE, 0xd800, Size::Byte),
        Some(0),
        "RAM under I/O untouched"
    );
}

#[test]
fn interrupt_lines_are_wired_or() {
    let mut i = Interrupts::default();
    i.set_irq(irq::VIC, true);
    i.set_irq(irq::CARTRIDGE, true);
    i.set_irq(irq::VIC, false);
    assert_ne!(i.irq, 0, "cartridge still pulls the line");
    i.set_irq(irq::CARTRIDGE, false);
    assert_eq!(i.irq, 0);
}

#[test]
fn irq_is_level_triggered() {
    // Main: CLI, loop. IRQ handler: INC $10, RTI (source never cleared, so
    // the CPU re-enters as soon as RTI restores I = 0).
    let code = [0x58, 0x4c, 0x01, 0xe0];
    let irq = [0xe6, 0x10, 0x40];
    // A source nothing else drives (the cartridge sets its own each cycle).
    const TEST_SOURCE: u8 = 1 << 7;
    let mut m = machine(&code, &irq, &[0x40]);
    run_cycles(&mut m, 20);
    assert_eq!(m.mem.ram[0x10], 0);
    m.interrupts_mut().set_irq(TEST_SOURCE, true);
    run_cycles(&mut m, 40);
    let count = m.mem.ram[0x10];
    assert!(count >= 2, "IRQ taken repeatedly while held: {count}");
    m.interrupts_mut().set_irq(TEST_SOURCE, false);
    run_cycles(&mut m, 20);
    let settled = m.mem.ram[0x10];
    run_cycles(&mut m, 40);
    assert_eq!(m.mem.ram[0x10], settled, "line released");
}
#[test]
fn nmi_is_edge_triggered() {
    let code = [0x4c, 0x00, 0xe0];
    let nmi = [0xe6, 0x11, 0x40];
    // A source nothing else drives (the cartridge sets its own each cycle).
    const TEST_SOURCE: u8 = 1 << 7;
    let mut m = machine(&code, &[0x40], &nmi);
    run_cycles(&mut m, 20);
    m.io.keyboard.restore = true;
    run_cycles(&mut m, 60);
    assert_eq!(m.mem.ram[0x11], 1, "held low: one NMI only");
    m.interrupts_mut().set_nmi(TEST_SOURCE, true);
    run_cycles(&mut m, 60);
    assert_eq!(
        m.mem.ram[0x11], 1,
        "second source on an active line: no edge"
    );
    m.io.keyboard.restore = false;
    m.interrupts_mut().set_nmi(TEST_SOURCE, false);
    run_cycles(&mut m, 2);
    m.interrupts_mut().set_nmi(TEST_SOURCE, true);
    run_cycles(&mut m, 60);
    assert_eq!(m.mem.ram[0x11], 2);
}

#[test]
fn savestate_resumes_identically() {
    let mut a = stub();
    run_cycles(&mut a, 12_345);
    a.mem.color[7] = 5;
    let state = a.save_state();
    let mut b = stub();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    b.run_frame(&mut dbg);
    assert_eq!(a.save_state(), b.save_state());

    let before = b.save_state();
    assert!(b.load_state(&state[..state.len() - 1]).is_err());
    assert_eq!(
        b.save_state(),
        before,
        "failed load leaves the machine unchanged"
    );
}

#[test]
fn debugger_memory_spaces() {
    let mut m = stub();
    m.run_frame(&mut DebugHooks::new());
    let names: Vec<_> = m.memory_spaces().iter().map(|s| s.name).collect();
    assert_eq!(names, ["cpu", "ram", "colorram"]);

    assert!(m.poke(RAM_SPACE, 0xe000, Size::Byte, 0x42));
    assert_eq!(m.peek(RAM_SPACE, 0xe000, Size::Byte), Some(0x42));
    assert_eq!(
        m.peek(CPU_SPACE, 0xe000, Size::Byte),
        Some(0x78),
        "KERNAL visible"
    );
    assert!(
        !m.poke(CPU_SPACE, 0xe000, Size::Byte, 0),
        "ROM is not writable"
    );
    assert!(m.poke(CPU_SPACE, 0xd801, Size::Byte, 0x3c));
    assert_eq!(m.peek(COLOR_SPACE, 1, Size::Byte), Some(0x0c));
    assert_eq!(m.peek(MemSpaceId(9), 0, Size::Byte), None);
    assert_eq!(m.disassemble(0xe000).to_string(), "SEI");
    assert_eq!(m.chips()[0].regs[3].value, 31, "PLA mode");
}

#[test]
fn write_breakpoint_on_the_cpu_space() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let bp = dbg.add_breakpoint(emu_core::BreakpointKind::Write {
        space: CPU_SPACE,
        addr: 1,
        len: 1,
    });
    assert_eq!(
        m.run_frame(&mut dbg),
        StopReason::Watchpoint {
            id: bp,
            space: CPU_SPACE,
            addr: 1,
            write: true
        }
    );
    assert_eq!(m.mem.port.data, 0x37);
}

#[test]
fn cia1_timer_interrupt_reaches_the_cpu_and_is_acknowledged() {
    // TA latch $0100, IRQ on TA, continuous; CLI; idle.
    let code = [
        0xa9, 0x00, 0x8d, 0x04, 0xdc, // LDA #0;  STA $DC04
        0xa9, 0x01, 0x8d, 0x05, 0xdc, // LDA #1;  STA $DC05
        0xa9, 0x81, 0x8d, 0x0d, 0xdc, // LDA #$81; STA $DC0D
        0xa9, 0x01, 0x8d, 0x0e, 0xdc, // LDA #1;  STA $DC0E
        0x58, // CLI
        0x4c, 0x15, 0xe0, // JMP *
    ];
    // Handler: INC $10; LDA $DC0D (ack); RTI
    let irq = [0xe6, 0x10, 0xad, 0x0d, 0xdc, 0x40];
    let mut m = machine(&code, &irq, &[0x40]);
    run_cycles(&mut m, 30 + 257 * 10);
    let n = m.mem.ram[0x10];
    assert!((9..=11).contains(&n), "one IRQ per 257 cycles: {n}");
    assert!(!m.io.cia1.irq() || m.ints.irq != 0);
}

#[test]
fn cia2_interrupt_is_an_nmi() {
    // CIA2 TB one-shot with NMI enabled; SEI must not block it.
    let code = [
        0x78, // SEI
        0xa9, 0x40, 0x8d, 0x06, 0xdd, // LDA #$40; STA $DD06
        0xa9, 0x00, 0x8d, 0x07, 0xdd, // LDA #0;   STA $DD07
        0xa9, 0x82, 0x8d, 0x0d, 0xdd, // LDA #$82; STA $DD0D
        0xa9, 0x09, 0x8d, 0x0f, 0xdd, // LDA #9;   STA $DD0F (one-shot, start)
        0x4c, 0x15, 0xe0, // JMP *
    ];
    let nmi = [0xe6, 0x11, 0xad, 0x0d, 0xdd, 0x40]; // INC $11; LDA $DD0D; RTI
    let mut m = machine(&code, &[0x40], &nmi);
    run_cycles(&mut m, 500);
    assert_eq!(m.mem.ram[0x11], 1);
    assert_eq!(
        m.peek(CPU_SPACE, 0xdd0d, Size::Byte),
        Some(0),
        "acknowledged"
    );
}

#[test]
fn cia_registers_are_mirrored_and_visible_to_the_debugger() {
    let code = [
        0xa9, 0x5a, 0x8d, 0xf2, 0xdc, // LDA #$5A; STA $DCF2 (mirror of DDRA)
        0x4c, 0x05, 0xe0,
    ];
    let mut m = machine(&code, &[0x40], &[0x40]);
    run_cycles(&mut m, 30);
    assert_eq!(m.peek(CPU_SPACE, 0xdc02, Size::Byte), Some(0x5a));
    let chips = m.chips();
    let cia1 = chips.iter().find(|c| c.name == "cia1").unwrap();
    assert_eq!(cia1.regs[2].value, 0x5a);
}

#[test]
fn tod_runs_at_fifty_hertz() {
    // 50 frames ≈ 0.998 s at 985 248 Hz / 19 656 cycles per frame.
    tod_runs_at_mains_rate("pal", 50);
}

#[test]
fn tod_runs_at_sixty_hertz_on_ntsc() {
    // 60 frames ≈ 1.003 s at 1 022 727 Hz / 17 095 cycles per frame.
    tod_runs_at_mains_rate("ntsc", 60);
}

fn tod_runs_at_mains_rate(model: &str, frames_per_second: u32) {
    let mut m = stub_model(model);
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg); // boot (IOINIT rewrites CRA)
    let cra = m.io.cia1.ta.cr;
    // CRA bit 7 selects the 50 Hz TOD divider.
    let cra = if frames_per_second == 50 {
        cra | 0x80
    } else {
        cra & 0x7f
    };
    m.io.cia1.write(0x0e, cra);
    m.io.cia1.write(0x0b, 0x01);
    m.io.cia1.write(0x0a, 0);
    m.io.cia1.write(0x09, 0);
    m.io.cia1.write(0x08, 0);
    for _ in 0..frames_per_second {
        m.run_frame(&mut dbg);
    }
    // The power-line phase is arbitrary, so allow one tenth either way.
    let tenths = |m: &C64| {
        let t = m.io.cia1.tod.time;
        u32::from(t[1] >> 4) * 100 + u32::from(t[1] & 15) * 10 + u32::from(t[0])
    };
    assert!((9..=10).contains(&tenths(&m)), "{}", tenths(&m));
    for _ in 0..frames_per_second {
        m.run_frame(&mut dbg);
    }
    assert!((19..=20).contains(&tenths(&m)), "{}", tenths(&m));
}

fn key_event(at: u64, k: emu_core::input::MachineKey, down: bool) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: emu_core::input::InputEvent::Key {
            machine_key: k,
            down,
        },
    }
}

fn joy_event(at: u64, port: PortId, dirs: emu_core::input::Dirs, buttons: u8) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Port(port),
        event: emu_core::input::InputEvent::Digital { dirs, buttons },
    }
}

#[test]
fn keyboard_matrix_through_cia1() {
    let mut m = stub();
    m.run_frame(&mut DebugHooks::new()); // IOINIT: DDRA = $FF, DDRB = $00
    assert!(m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0xfe)); // scan column 0
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xff));
    let now = m.now().0;
    m.input(key_event(now, keyboard::keys::RETURN, true));
    m.step_cycle();
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xfd), "row 1");
    assert!(m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0xfd)); // other column
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xff));
    m.input(key_event(now, keyboard::keys::RETURN, false));
    m.step_cycle();
    assert!(m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0xfe));
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xff));
}

#[test]
fn joystick_in_port_2_reads_on_port_a() {
    use emu_core::input::Dirs;
    let mut m = stub();
    m.run_frame(&mut DebugHooks::new());
    let now = m.now().0;
    m.input(joy_event(now, CONTROL_PORT_2, Dirs::UP | Dirs::RIGHT, 1));
    m.step_cycle();
    // IOINIT leaves $DC00 = $7F; up (bit 0), right (bit 3), fire (bit 4) low.
    assert_eq!(m.peek(CPU_SPACE, 0xdc00, Size::Byte), Some(0x66));
    m.input(joy_event(now, CONTROL_PORT_2, Dirs::NONE, 0));
    m.step_cycle();
    assert_eq!(m.peek(CPU_SPACE, 0xdc00, Size::Byte), Some(0x7f));
}

#[test]
fn input_is_applied_exactly_at_its_cycle() {
    let mut m = stub();
    m.run_frame(&mut DebugHooks::new());
    m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0xfe);
    let at = m.now().0 + 100;
    m.input(key_event(at, keyboard::keys::RETURN, true));
    while m.now().0 < at {
        assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xff));
        m.step_cycle();
    }
    m.step_cycle();
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xfd));
}

#[test]
fn restore_key_raises_nmi() {
    let mut m = stub();
    m.run_frame(&mut DebugHooks::new());
    let now = m.now().0;
    m.input(key_event(now, keyboard::RESTORE, true));
    m.step_cycle();
    assert_ne!(m.ints.nmi & nmi::RESTORE, 0);
}

#[test]
fn ports_accept_only_control_port_devices() {
    let mut m = stub();
    let names: Vec<_> = m.ports().iter().map(|p| (p.name, p.connected)).collect();
    assert_eq!(names, [("control port 1", false), ("control port 2", true)]);
    let atari = Box::new(Joystick::new(PortKind::AtariJoy));
    assert!(matches!(
        m.connect(CONTROL_PORT_1, atari),
        Err(PortError::Incompatible { .. })
    ));
    let joy = || Box::new(Joystick::new(PortKind::CbmControl));
    assert!(matches!(
        m.connect(CONTROL_PORT_2, joy()),
        Err(PortError::Occupied(_))
    ));
    assert!(matches!(
        m.connect(PortId(5), joy()),
        Err(PortError::NoSuchPort(_))
    ));
    assert!(m.connect(CONTROL_PORT_1, joy()).is_ok());
    assert!(m.disconnect(CONTROL_PORT_2).is_some());
    assert!(m.disconnect(CONTROL_PORT_2).is_none());
}

#[test]
fn savestate_carries_queued_input_and_device_state() {
    use emu_core::input::Dirs;
    let mut a = stub();
    a.run_frame(&mut DebugHooks::new());
    let now = a.now().0;
    a.input(joy_event(now, CONTROL_PORT_2, Dirs::LEFT, 0));
    a.input(key_event(now + 5000, keyboard::keys::SPACE, true));
    a.step_cycle();
    let state = a.save_state();
    let mut b = stub();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    b.run_frame(&mut dbg);
    assert_eq!(a.save_state(), b.save_state());
    assert!(
        b.io.keyboard.pressed(keyboard::keys::SPACE),
        "queued event replayed"
    );
    assert_eq!(b.io.port_lines[1] & 0x04, 0x04, "joystick state restored");
}

#[test]
fn sid_output_reaches_the_audio_stream_at_the_right_pitch() {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    config.options.insert("sid".into(), "8580".into());
    let mut m = C64::new(&config).unwrap();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    let mut audio = Vec::new();
    let format = m.audio(&mut audio);
    assert_eq!((format.sample_rate, format.channels), (48_000, 1));
    assert!(
        audio.iter().all(|&s| s == audio[0]),
        "silent before the tone"
    );

    // Sawtooth, frequency $1000 (≈240 Hz at 985 248 Hz), full sustain.
    for (reg, v) in [
        (0x18, 0x0f),
        (0x05, 0x00),
        (0x06, 0xf0),
        (0x00, 0x00),
        (0x01, 0x10),
        (0x04, 0x21),
    ] {
        assert!(m.poke(CPU_SPACE, 0xd400 + reg, Size::Byte, v));
    }
    audio.clear();
    for _ in 0..50 {
        m.run_frame(&mut dbg);
    }
    m.audio(&mut audio);
    let seconds = audio.len() as f64 / 48_000.0;
    assert!((0.99..=1.01).contains(&seconds), "{seconds}");
    let tail = &audio[audio.len() / 2..];
    let mean = tail.iter().map(|&s| f64::from(s)).sum::<f64>() / tail.len() as f64;
    let crossings = tail
        .windows(2)
        .filter(|w| f64::from(w[0]) < mean && f64::from(w[1]) >= mean)
        .count();
    let hz = crossings as f64 / (tail.len() as f64 / 48_000.0);
    assert!((230.0..=250.0).contains(&hz), "{hz} Hz");
}

#[test]
fn ntsc_model_frame_timing_and_signal() {
    let mut m = stub_model("ntsc");
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    let start = m.now().0;
    assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    assert_eq!(m.now().0 - start, 263 * 65);
    let f = m.framebuffer();
    assert_eq!((f.width, f.height), (384, 235));
    assert_eq!(f.signal.standard, emu_core::frame::VideoStandard::Ntsc);
    assert_eq!(f.signal.pixel_clock_hz, 8 * 1_022_727);

    let mut pal = stub();
    pal.run_frame(&mut dbg);
    let start = pal.now().0;
    pal.run_frame(&mut dbg);
    assert_eq!(pal.now().0 - start, 312 * 63);
    assert_eq!(
        pal.framebuffer().signal.standard,
        emu_core::frame::VideoStandard::Pal
    );
}

#[test]
fn savestates_do_not_cross_video_standards() {
    let mut dbg = DebugHooks::new();
    let mut pal = stub();
    pal.run_frame(&mut dbg);
    let state = pal.save_state();
    let mut ntsc = stub_model("ntsc");
    ntsc.run_frame(&mut dbg);
    let before = ntsc.save_state();
    assert!(ntsc.load_state(&state).is_err());
    assert_eq!(
        ntsc.save_state(),
        before,
        "failed load leaves the machine untouched"
    );
    let own = ntsc.save_state();
    ntsc.run_frame(&mut dbg);
    ntsc.load_state(&own).unwrap();
    assert_eq!(ntsc.save_state(), own);
}

#[test]
fn irq_breakpoint_stops_at_the_first_handler_instruction() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let bp = dbg.add_breakpoint(emu_core::BreakpointKind::Interrupt(
        emu_core::InterruptKind::Irq,
    ));
    let mut stop = StopReason::FrameDone;
    for _ in 0..10 {
        stop = m.run_frame(&mut dbg);
        if stop != StopReason::FrameDone {
            break;
        }
    }
    assert_eq!(stop, StopReason::Breakpoint(bp), "CIA1 timer IRQ");
    assert_eq!(m.regs().get("PC"), Some(0xff48));
}

#[test]
fn soft_reset_keeps_ram_hard_reset_clears_it() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    m.mem.ram[0x4000] = 0x55;
    m.mem.color[0] = 7;
    m.io.cia1.write(0x0e, 0x00);
    m.reset(ResetKind::Soft);
    assert_eq!(m.mem.ram[0x4000], 0x55);
    assert_eq!(m.mem.color[0], 7);
    assert_eq!(m.mem.port.ddr, 0, "6510 port back to inputs");
    run_cycles(&mut m, 7);
    assert_eq!(m.regs().get("PC"), Some(0xe000), "reset vector");
    m.run_frame(&mut dbg);
    assert_eq!(
        m.peek(CPU_SPACE, 1, Size::Byte),
        Some(0x37),
        "KERNAL ran again"
    );

    m.reset(ResetKind::Hard);
    assert_eq!(m.mem.ram[0x4000], 0);
    assert_eq!(m.mem.color[0], 0);
    assert_eq!(m.io.vic.beam().frame, 0, "VIC-II restarts on power-up only");
}

/// Run a test program from $E000 for two frames with `freezes` armed.
fn run_frozen(code: &[u8], freezes: Vec<emu_core::Freeze>) -> (C64, DebugHooks) {
    let mut m = machine(code, &[0x40], &[0x40]);
    let mut dbg = DebugHooks::new();
    for f in freezes {
        dbg.add_freeze(f);
    }
    m.run_frame(&mut dbg);
    m.run_frame(&mut dbg);
    (m, dbg)
}

fn freeze(
    space: MemSpaceId,
    addr: u32,
    value: u32,
    mode: emu_core::FreezeMode,
) -> emu_core::Freeze {
    emu_core::Freeze {
        mode,
        ..emu_core::Freeze::new(space, addr, Size::Byte, value)
    }
}

#[test]
fn freeze_block_writes_on_the_cpu_view() {
    // LDA #$11; STA $0400; LDA $0400; STA $0401; JMP *
    let code = [
        0xa9, 0x11, 0x8d, 0x00, 0x04, 0xad, 0x00, 0x04, 0x8d, 0x01, 0x04, 0x4c, 0x0b, 0xe0,
    ];
    let (m, mut dbg) = run_frozen(
        &code,
        vec![freeze(
            CPU_SPACE,
            0x0400,
            0x42,
            emu_core::FreezeMode::BlockWrites,
        )],
    );
    assert_eq!(m.mem.ram[0x0400], 0, "write suppressed");
    assert_eq!(m.mem.ram[0x0401], 0x42, "read returns the frozen value");
    let blocked = dbg.take_blocked_write().expect("blocked write recorded");
    assert_eq!(
        (blocked.space, blocked.addr, blocked.value),
        (CPU_SPACE, 0x0400, 0x11)
    );
}

#[test]
fn freeze_reapply_per_frame_on_physical_ram() {
    // LDA #$11; STA $0400; JMP * (the guest changes the value once)
    let code = [0xa9, 0x11, 0x8d, 0x00, 0x04, 0x4c, 0x05, 0xe0];
    let (m, _) = run_frozen(
        &code,
        vec![freeze(
            RAM_SPACE,
            0x0400,
            0x77,
            emu_core::FreezeMode::ReapplyPerFrame,
        )],
    );
    assert_eq!(
        m.mem.ram[0x0400], 0x77,
        "rewritten at the start of the frame"
    );
}

#[test]
fn freeze_reapply_on_read_leaves_memory_alone() {
    // LDA $0400; STA $0401; JMP *
    let code = [0xad, 0x00, 0x04, 0x8d, 0x01, 0x04, 0x4c, 0x06, 0xe0];
    let (m, _) = run_frozen(
        &code,
        vec![freeze(
            CPU_SPACE,
            0x0400,
            0x99,
            emu_core::FreezeMode::ReapplyOnRead,
        )],
    );
    assert_eq!(m.mem.ram[0x0400], 0, "memory unchanged");
    assert_eq!(m.mem.ram[0x0401], 0x99);
}

#[test]
fn freeze_on_ram_under_rom_respects_banking() {
    let code = [
        0xa9, 0x37, 0x85, 0x01, // LDA #$37; STA $01 (latch before DDR)
        0xa9, 0x2f, 0x85, 0x00, // LDA #$2F; STA $00: BASIC ROM at $A000
        0xad, 0x00, 0xa0, 0x8d, 0x02, 0x04, // LDA $A000; STA $0402 (sees ROM)
        0xa9, 0x11, 0x8d, 0x00, 0xa0, // LDA #$11; STA $A000 (goes to RAM below)
        0xa9, 0x36, 0x85, 0x01, // LDA #$36; STA $01: RAM at $A000
        0xad, 0x00, 0xa0, 0x8d, 0x00, 0x04, // LDA $A000; STA $0400
        0x4c, 0x1d, 0xe0, // JMP *
    ];
    let (m, _) = run_frozen(
        &code,
        vec![freeze(
            RAM_SPACE,
            0xa000,
            0x55,
            emu_core::FreezeMode::BlockWrites,
        )],
    );
    assert_eq!(
        m.mem.ram[0x0402], 0xb0,
        "with ROM banked in, the CPU reads ROM"
    );
    assert_eq!(m.mem.ram[0xa000], 0, "the write under the ROM was blocked");
    assert_eq!(
        m.mem.ram[0x0400], 0x55,
        "with RAM banked in, the frozen value"
    );
}

#[test]
fn cia1_selects_whose_pots_the_sid_reads() {
    let ports = [[0x1000, 0x2000], [0x3000, u16::MAX]];
    assert_eq!(io::pot_lines(0x00, ports), [u16::MAX; 2], "none");
    assert_eq!(io::pot_lines(0x40, ports), [0x1000, 0x2000], "port 1");
    assert_eq!(io::pot_lines(0x80, ports), [0x3000, u16::MAX], "port 2");
    assert_eq!(io::pot_lines(0xc0, ports), [0x1000, 0x2000], "parallel");
}

#[test]
fn a_1351_in_port_1_shows_in_potx_poty() {
    use emu_core::input::{InputEvent, InputTarget, TimedInput};
    let mut m = stub();
    assert!(m
        .connect(CONTROL_PORT_1, Box::new(Mouse1351::new()))
        .is_ok());
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(CONTROL_PORT_1),
        event: InputEvent::MouseRel {
            dx: 5,
            dy: -2,
            buttons: 1,
        },
    });
    // CIA1 port A: PA6/PA7 outputs, PA6 high selects port 1.
    assert!(m.poke(CPU_SPACE, 0xdc02, Size::Byte, 0xc0));
    assert!(m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0x40));
    m.run_frame(&mut DebugHooks::new());
    assert_eq!(m.peek(CPU_SPACE, 0xd419, Size::Byte), Some(10), "POTX");
    assert_eq!(m.peek(CPU_SPACE, 0xd41a, Size::Byte), Some(4), "POTY");
    // The left button is fire on control port 1 (CIA1 port B bit 4).
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte).unwrap() & 0x10, 0);
}
