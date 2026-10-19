use super::*;
use emu_core::beam::{TraceConfig, TraceEvent, TraceRange};
use emu_core::{BreakpointKind, Freeze};
use video::{CYCLES_PER_LINE, LINES, PALETTE};

fn demo() -> Sim6502 {
    Sim6502::new(&MachineConfig::default()).unwrap()
}

fn with_program(prg: &[u8]) -> Sim6502 {
    let mut config = MachineConfig::default();
    config.roms.insert("program".into(), prg.to_vec());
    Sim6502::new(&config).unwrap()
}

fn pc(m: &Sim6502) -> u32 {
    m.regs().get("PC").unwrap()
}

#[test]
fn a_field_takes_312_by_63_cycles() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    assert_eq!(
        m.sched.now().0,
        u64::from(LINES) * u64::from(CYCLES_PER_LINE)
    );
    assert_eq!(
        m.beam(),
        Some(BeamPos {
            frame: 1,
            line: 0,
            cycle: 0,
            hpos: 0
        })
    );
}

#[test]
fn demo_draws_raster_bars_in_the_border() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    m.run_frame(&mut dbg);
    let f = m.framebuffer();
    // Right border of each line shows that line's number as colour, give or
    // take one line for the loop's latency.
    for line in [40u32, 100, 200] {
        let c = f.rgb_at(380, line).unwrap();
        let expected = [line, line - 1].map(|l| PALETTE[(l & 15) as usize]);
        assert!(expected.contains(&c), "line {line}: {c:?}");
    }
}

#[test]
fn runs_are_deterministic_and_savestates_resume_identically() {
    let mut a = demo();
    let mut dbg = DebugHooks::new();
    a.run_frame(&mut dbg);
    for _ in 0..5000 {
        a.step_cycle();
    }
    let state = a.save_state();
    let mut b = demo();
    b.load_state(&state).unwrap();
    assert_eq!(b.save_state(), state);

    a.run_frame(&mut dbg);
    b.run_frame(&mut DebugHooks::new());
    assert_eq!(a.save_state(), b.save_state());
    assert_eq!(a.framebuffer(), b.framebuffer());
}

#[test]
fn corrupt_savestate_is_rejected_without_changes() {
    let mut m = demo();
    m.run_frame(&mut DebugHooks::new());
    let state = m.save_state();
    let before = m.save_state();
    assert!(m.load_state(&state[..state.len() - 3]).is_err());
    assert!(m.load_state(b"garbage").is_err());
    assert_eq!(m.save_state(), before);
}

#[test]
fn exec_breakpoint_stops_before_the_instruction_and_resumes() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    let bp = dbg.add_breakpoint(BreakpointKind::Exec {
        space: CPU_SPACE,
        addr: 0x0803,
    });
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(bp));
    assert_eq!(pc(&m), 0x0803);
    assert_eq!(m.disassemble(0x0803).to_string(), "STA $D000");
    // Resuming executes the STA first instead of stopping again at once.
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(bp));
    assert_eq!(pc(&m), 0x0803);
    assert_eq!(dbg.breakpoint(bp).unwrap().hits, 2);
}

#[test]
fn instruction_step() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    dbg.set_instr_step(true);
    assert_eq!(
        m.run_frame(&mut dbg),
        StopReason::StepDone,
        "reset sequence"
    );
    assert_eq!(pc(&m), 0x0800);
    for expected in [0x0803, 0x0806, 0x0800] {
        dbg.set_instr_step(true);
        assert_eq!(m.run_frame(&mut dbg), StopReason::StepDone);
        assert_eq!(pc(&m), expected);
    }
}

#[test]
fn line_step_shows_the_partial_frame() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    // Paint the "previous field" black so new lines are recognisable.
    m.video.clear_pixels();
    for line in 1..=50u16 {
        dbg.set_line_step(true);
        assert_eq!(m.run_frame(&mut dbg), StopReason::StepDone);
        let beam = m.beam().unwrap();
        assert_eq!((beam.line, beam.cycle), (line, 0));
    }
    let f = m.framebuffer();
    assert_ne!(f.rgb_at(0, 45), Some(PALETTE[0]), "drawn above the beam");
    assert_eq!(
        f.rgb_at(0, 51),
        Some(PALETTE[0]),
        "old field below the beam"
    );
}

#[test]
fn raster_trace_stamps_border_writes_with_the_beam() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    dbg.set_trace(TraceConfig {
        writes: vec![TraceRange {
            space: CPU_SPACE,
            addr: 0xd000,
            len: 1,
        }],
        ..TraceConfig::default()
    });
    m.run_frame(&mut dbg);
    let trace = dbg.drain_trace();
    // 11-cycle loop: about 312 * 63 / 11 writes per field.
    assert!(trace.len() > 1700, "{} entries", trace.len());
    assert!(trace.windows(2).all(|w| w[0].beam < w[1].beam));
    for e in &trace {
        let TraceEvent::Write { addr, value, .. } = e.event else {
            panic!("unexpected {e:?}");
        };
        assert_eq!(addr, 0xd000);
        // The value written is the raster line the LDA saw a few cycles earlier.
        assert!(
            value == u32::from(e.beam.line as u8)
                || value == u32::from(e.beam.line.wrapping_sub(1) as u8),
            "{e:?}"
        );
    }
}

#[test]
fn blocking_freeze_keeps_the_border_colour() {
    let mut m = demo();
    let mut dbg = DebugHooks::new();
    dbg.add_freeze(Freeze::new(CPU_SPACE, 0xd000, Size::Byte, 2));
    m.run_frame(&mut dbg);
    m.run_frame(&mut dbg);
    assert_eq!(m.video.border, 14, "writes blocked, initial colour stays");
    assert!(dbg.take_blocked_write().is_some());
}

#[test]
fn raster_irq_reaches_the_cpu() {
    // $0800: SEI-free setup, enable raster IRQ at line 100, wait.
    // IRQ handler at $0900 stores the line it ran on into $10 and acks.
    let mut prg = vec![0x00, 0x08];
    let mut code = [0u8; 0x200];
    let main = [
        0xa9, 0x64, // LDA #100
        0x8d, 0x12, 0xd0, // STA $D012
        0xa9, 0x01, // LDA #1
        0x8d, 0x1a, 0xd0, // STA $D01A
        0x58, // CLI
        0x4c, 0x0b, 0x08, // JMP *
    ];
    code[..main.len()].copy_from_slice(&main);
    let handler = [
        0xad, 0x10, 0xd0, // LDA $D010
        0x85, 0x10, // STA $10
        0xa9, 0x01, // LDA #1
        0x8d, 0x19, 0xd0, // STA $D019
        0x40, // RTI
    ];
    code[0x100..0x100 + handler.len()].copy_from_slice(&handler);
    prg.extend_from_slice(&code);
    let mut m = with_program(&prg);
    m.poke(CPU_SPACE, 0xfffe, Size::Word, 0x0900);
    m.run_frame(&mut DebugHooks::new());
    assert_eq!(m.peek(CPU_SPACE, 0x10, Size::Byte), Some(100));
}

#[test]
fn jam_stops_with_cpu_jam() {
    let mut m = with_program(&[0x00, 0x08, 0x02]);
    assert_eq!(
        m.run_frame(&mut DebugHooks::new()),
        StopReason::CpuJam {
            device: CPU,
            pc: 0x0801
        }
    );
}

#[test]
fn config_validation() {
    let mut config = MachineConfig {
        model: Some("c64".into()),
        ..MachineConfig::default()
    };
    assert!(matches!(
        Sim6502::new(&config),
        Err(ConfigError::UnknownModel(_))
    ));
    config.model = None;
    config.roms.insert("program".into(), vec![0x00]);
    assert!(matches!(
        Sim6502::new(&config),
        Err(ConfigError::Invalid(_))
    ));
    config.roms.insert("program".into(), vec![0xff, 0xff, 1, 2]);
    assert!(matches!(
        Sim6502::new(&config),
        Err(ConfigError::Invalid(_))
    ));
    config.roms.insert("program".into(), vec![0x00, 0x10, 0xea]);
    config.options.insert("start".into(), "zz".into());
    assert!(matches!(
        Sim6502::new(&config),
        Err(ConfigError::Invalid(_))
    ));
    config.options.insert("start".into(), "$1000".into());
    assert!(Sim6502::new(&config).is_ok());
}

#[test]
fn debugger_view() {
    let mut m = demo();
    assert_eq!(m.devices(), [CPU]);
    assert!(m.debug_target(DeviceId("drive8")).is_none());
    let t = m.debug_target(CPU).unwrap();
    assert_eq!(t.peek(CPU_SPACE, 0x0800, Size::Word), Some(0x10ad));
    assert_eq!(t.peek(MemSpaceId(9), 0, Size::Byte), None);
    assert!(t.poke(CPU_SPACE, 0xd000, Size::Byte, 3));
    assert_eq!(m.chips()[0].regs[0].value, 3);
}
