use super::*;

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
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
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
    assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
    // The frame may end inside the idle JMP.
    assert!((0xe00d..=0xe00f).contains(&m.regs().get("PC").unwrap()));
    assert_eq!(m.peek(CPU_SPACE, 0, Size::Byte), Some(0x2f));
    assert_eq!(m.peek(CPU_SPACE, 1, Size::Byte), Some(0x37));
    assert_eq!(m.mem.mode(), 31);
    assert_eq!(m.sched.now().0, u64::from(CYCLES_PER_FRAME));
}

#[test]
fn rom_configuration_errors() {
    let empty = MachineConfig::default();
    assert!(matches!(C64::new(&empty), Err(ConfigError::MissingRom(r)) if r == "kernal"));

    let mut config = config_with(&[], &[], &[]);
    config.roms.insert("basic".into(), vec![0; 100]);
    assert!(matches!(C64::new(&config), Err(ConfigError::Invalid(_))));

    let mut config = config_with(&[], &[], &[]);
    config.model = Some("ntsc".into());
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
    assert_eq!(m.peek(RAM_SPACE, 0x10, Size::Byte), Some(0xff));
    assert_eq!(
        m.peek(RAM_SPACE, 0xd800, Size::Byte),
        Some(0),
        "RAM under I/O untouched"
    );
}

#[test]
fn irq_line_is_wired_or() {
    // Main: CLI, loop. IRQ handler: INC $10, RTI (source never cleared, so
    // the CPU re-enters as soon as RTI restores I = 0).
    let code = [0x58, 0x4c, 0x01, 0xe0];
    let irq = [0xe6, 0x10, 0x40];
    let mut m = machine(&code, &irq, &[0x40]);
    run_cycles(&mut m, 20);
    assert_eq!(m.mem.ram[0x10], 0);
    m.interrupts_mut().set_irq(irq::VIC, true);
    m.interrupts_mut().set_irq(irq::CIA1, true);
    run_cycles(&mut m, 40);
    let count = m.mem.ram[0x10];
    assert!(count >= 2, "IRQ taken repeatedly while held: {count}");
    m.interrupts_mut().set_irq(irq::VIC, false);
    run_cycles(&mut m, 40);
    assert!(m.mem.ram[0x10] > count, "CIA1 still holds the line");
    m.interrupts_mut().set_irq(irq::CIA1, false);
    run_cycles(&mut m, 20);
    let settled = m.mem.ram[0x10];
    run_cycles(&mut m, 40);
    assert_eq!(m.mem.ram[0x10], settled, "line released");
}

#[test]
fn nmi_is_edge_triggered() {
    let code = [0x4c, 0x00, 0xe0];
    let nmi = [0xe6, 0x11, 0x40];
    let mut m = machine(&code, &[0x40], &nmi);
    run_cycles(&mut m, 20);
    m.interrupts_mut().set_nmi(nmi::RESTORE, true);
    run_cycles(&mut m, 60);
    assert_eq!(m.mem.ram[0x11], 1, "held low: one NMI only");
    m.interrupts_mut().set_nmi(nmi::CARTRIDGE, true);
    run_cycles(&mut m, 60);
    assert_eq!(
        m.mem.ram[0x11], 1,
        "second source on an active line: no edge"
    );
    m.interrupts_mut().set_nmi(nmi::RESTORE, false);
    m.interrupts_mut().set_nmi(nmi::CARTRIDGE, false);
    run_cycles(&mut m, 2);
    m.interrupts_mut().set_nmi(nmi::CARTRIDGE, true);
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
    let mut m = stub();
    m.io.cia1.write(0x0e, 0x80);
    m.io.cia1.write(0x08, 0);
    let mut dbg = DebugHooks::new();
    for _ in 0..50 {
        m.run_frame(&mut dbg);
    }
    // 50 frames ≈ 0.998 s at 985 248 Hz / 19 656 cycles per frame.
    assert_eq!(m.io.cia1.tod.time[1], 0x00);
    assert_eq!(m.io.cia1.tod.time[0], 0x09);
    m.run_frame(&mut dbg);
    assert_eq!(m.io.cia1.tod.time[1], 0x01);
}
