use super::*;
use emu_core::input::{InputEvent, MachineKey};

fn stub_with(model: &str, options: &[(&str, &str)]) -> C128 {
    let mut config = MachineConfig::default();
    config.options.insert("roms".into(), "stub".into());
    for (k, v) in options {
        config.options.insert((*k).into(), (*v).into());
    }
    config.model = Some(model.into());
    C128::new(&config).unwrap()
}

fn stub() -> C128 {
    stub_with("pal", &[])
}

fn run_cycles(m: &mut C128, n: u32) {
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

fn exec_bp(dbg: &mut DebugHooks, space: MemSpaceId, addr: u16) -> emu_core::BpId {
    dbg.add_breakpoint(emu_core::BreakpointKind::Exec {
        space,
        addr: u32::from(addr),
    })
}

#[test]
fn z80_boots_and_hands_over_to_the_8502() {
    let mut m = stub();
    assert!(!m.mem.mmu.m8502(), "the Z80 starts");
    let mut dbg = DebugHooks::new();
    let idle = exec_bp(&mut dbg, CPU_SPACE, stub::IDLE);
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(idle));
    assert!(m.mem.mmu.m8502());
    assert_eq!(m.z80.pc, 0x0007, "Z80 stopped after its OUT");
    dbg.remove_breakpoint(idle);
    // CIA1 timer interrupts reach the 8502.
    let irq = exec_bp(&mut dbg, CPU_SPACE, stub::IRQ);
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(irq));
}

#[test]
fn z80_breakpoints_and_debug_target() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    let bp = exec_bp(&mut dbg, Z80_SPACE, 0x0005);
    assert_eq!(m.run_frame(&mut dbg), StopReason::Breakpoint(bp));
    let z80 = m.debug_target(Z80_DEV).unwrap();
    assert_eq!(z80.regs().get("PC"), Some(5));
    assert_eq!(z80.regs().get("BC"), None, "pairs are set_reg only");
    assert_eq!(z80.regs().get("B"), Some(0xd5));
    assert_eq!(z80.disassemble(0).mnemonic, "LD");
    let cpu = m.debug_target(CPU).unwrap();
    assert_eq!(cpu.disassemble(0xe000).mnemonic, "SEI");
}

#[test]
fn mmu_banking_through_ff00() {
    let mut m = stub();
    assert_eq!(m.peek(CPU_SPACE, 0xe000, Size::Byte), Some(0x78), "KERNAL");
    // Bank 1, all RAM: $E000 now RAM bank 1.
    assert!(m.poke(CPU_SPACE, 0xff00, Size::Byte, 0x7f));
    assert!(m.poke(CPU_SPACE, 0xe000, Size::Byte, 0x42));
    assert_eq!(m.mem.ram[0x1e000], 0x42);
    assert_eq!(m.peek(RAM_SPACE, 0x1e000, Size::Byte), Some(0x42));
    // $FF00 stays the MMU in any configuration.
    assert_eq!(m.peek(CPU_SPACE, 0xff00, Size::Byte), Some(0x7f));
    // Back to bank 0 with ROMs and I/O; the MMU at $D500 reads the same CR.
    assert!(m.poke(CPU_SPACE, 0xff00, Size::Byte, 0x00));
    assert_eq!(m.peek(CPU_SPACE, 0xd500, Size::Byte), Some(0x00));
    assert_eq!(m.peek(CPU_SPACE, 0xd50b, Size::Byte), Some(0x20));
}

#[test]
fn vdc_through_d600() {
    let mut m = stub();
    // Update address $1234, write $AB.
    for (r, v) in [(18, 0x12), (19, 0x34), (31, 0xab)] {
        assert!(m.poke(CPU_SPACE, 0xd600, Size::Byte, r));
        assert!(m.poke(CPU_SPACE, 0xd601, Size::Byte, v));
    }
    assert_eq!(m.peek(VDC_SPACE, 0x1234, Size::Byte), Some(0xab));
    assert_eq!(m.peek(CPU_SPACE, 0xd600, Size::Byte).unwrap() & 0x80, 0x80);
    assert!(m.poke(VDC_SPACE, 0x0010, Size::Byte, 0x55));
    assert_eq!(m.io.vdc.ram()[0x10], 0x55);
}

#[test]
fn two_screens_follow_the_4080_key() {
    let mut m = stub();
    let s = m.screens();
    assert_eq!(s.iter().map(|s| s.name).collect::<Vec<_>>(), ["vic", "vdc"]);
    assert!(s[0].active && !s[1].active);
    assert_eq!(m.screen(VDC_SCREEN).unwrap().width, vdc::WIDTH);
    assert_eq!(m.screen(VIC_SCREEN).unwrap().width, m.framebuffer().width);
    assert!(m.screen(ScreenId(2)).is_none());
    let now = m.now().0;
    m.input(key_event(now, keyboard::DISPLAY_4080, true));
    m.input(key_event(now + 1, keyboard::DISPLAY_4080, false));
    run_cycles(&mut m, 2);
    assert!(m.screens()[1].active);
    assert_eq!(
        m.peek(CPU_SPACE, 0xd505, Size::Byte).unwrap() & 0x80,
        0,
        "MCR bit 7"
    );
    let m = stub_with("pal", &[("display", "80")]);
    assert!(m.screens()[1].active);
}

#[test]
fn keyboard_extra_columns_through_d02f() {
    let mut m = stub();
    let now = m.now().0;
    m.input(key_event(now, keyboard::keys::HELP, true));
    run_cycles(&mut m, 1);
    // CIA1: port A all outputs high, port B inputs.
    assert!(m.poke(CPU_SPACE, 0xdc02, Size::Byte, 0xff));
    assert!(m.poke(CPU_SPACE, 0xdc03, Size::Byte, 0x00));
    assert!(m.poke(CPU_SPACE, 0xdc00, Size::Byte, 0xff));
    assert!(m.poke(CPU_SPACE, 0xd02f, Size::Byte, 0xfe)); // K0 low
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xfe));
    assert_eq!(m.peek(CPU_SPACE, 0xd02f, Size::Byte), Some(0xfe));
    assert!(m.poke(CPU_SPACE, 0xd02f, Size::Byte, 0xff));
    assert_eq!(m.peek(CPU_SPACE, 0xdc01, Size::Byte), Some(0xff));
}

/// 8502 instructions completed in the next `n` system cycles.
fn instructions_in(m: &mut C128, n: u64) -> u32 {
    let end = m.now().0 + n;
    let mut dbg = DebugHooks::new();
    dbg.set_instr_step(true);
    let mut count = 0;
    while m.now().0 < end {
        if m.cycle(Some(&mut dbg)) == Some(StopReason::StepDone) {
            count += 1;
            dbg.set_instr_step(true);
        }
    }
    count
}

#[test]
fn fast_mode_runs_two_cpu_cycles_per_vic_cycle() {
    let mut m = stub();
    let mut dbg = DebugHooks::new();
    m.run_frame(&mut dbg);
    // SEI; LDA #0 / JMP $1001 at $1000.
    for (i, &b) in [0x78, 0xa9, 0x00, 0x4c, 0x01, 0x10].iter().enumerate() {
        m.mem.ram[0x1000 + i] = b;
    }
    m.cpu.set_reg("PC", 0x1000);
    let slow = instructions_in(&mut m, 2000);
    assert!(m.poke(CPU_SPACE, 0xd030, Size::Byte, 0x01));
    let fast = instructions_in(&mut m, 2000);
    assert!(fast > slow * 3 / 2, "fast {fast}, slow {slow}");
}

#[test]
fn c64_mode_maps_like_a_c64() {
    let mut m = stub();
    m.mem.roms.c64[0x2000] = 0xc6; // C64 KERNAL $E000
    m.mem.roms.c64[0] = 0xba; // C64 BASIC $A000
    assert!(m.poke(CPU_SPACE, 0xd505, Size::Byte, 0x41)); // C64 mode, 8502
    assert!(m.mem.mmu.c64_mode());
    assert!(m.poke(CPU_SPACE, 0x0000, Size::Byte, 0x2f));
    assert!(m.poke(CPU_SPACE, 0x0001, Size::Byte, 0x37));
    assert_eq!(m.peek(CPU_SPACE, 0xe000, Size::Byte), Some(0xc6));
    assert_eq!(m.peek(CPU_SPACE, 0xa000, Size::Byte), Some(0xba));
    assert!(m.poke(CPU_SPACE, 0xd020, Size::Byte, 0x02));
    assert_eq!(m.io.vic.peek(0x20) & 0x0f, 2, "I/O at $D000");
    // No MMU at $D500 or $FF00 in C64 mode.
    assert_eq!(
        m.peek(CPU_SPACE, 0xff00, Size::Byte),
        Some(0xff),
        "C64 KERNAL there"
    );
    assert!(m.poke(CPU_SPACE, 0x0001, Size::Byte, 0x30)); // all RAM
    assert!(m.poke(CPU_SPACE, 0xa000, Size::Byte, 0x11));
    assert_eq!(m.peek(CPU_SPACE, 0xa000, Size::Byte), Some(0x11));
}

#[test]
fn savestate_resumes_identically() {
    let mut a = stub_with("pal", &[("vdc_ram", "64k")]);
    run_cycles(&mut a, 40_000);
    a.mem.ram[0x1_2345] = 7;
    let state = a.save_state();
    let mut b = stub_with("pal", &[("vdc_ram", "64k")]);
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
    assert!(stub().load_state(&state).is_err(), "VDC RAM size differs");
}

#[test]
fn debugger_view() {
    let mut m = stub();
    let names: Vec<_> = m.memory_spaces().iter().map(|s| s.name).collect();
    assert_eq!(names, ["cpu", "ram", "z80", "vdcram"]);
    assert_eq!(m.devices(), [CPU, Z80_DEV]);
    let chips: Vec<_> = m.chips().iter().map(|c| c.name).collect();
    assert_eq!(chips, ["vic2", "cia1", "cia2", "mmu", "vdc"]);
    assert!(m.debug_target(DeviceId("drive8")).is_none());
}
