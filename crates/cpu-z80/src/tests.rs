use super::*;
use emu_core::testing::FlatBus;

/// 64 KB RAM plus a port log; port reads return the low port byte.
struct TestBus {
    mem: FlatBus,
    ports: Vec<(u16, u8)>,
}

impl TestBus {
    fn with(code: &[(u16, &[u8])]) -> Self {
        let mut mem = FlatBus::new(16);
        for (addr, bytes) in code {
            for (i, &b) in bytes.iter().enumerate() {
                mem.write(u32::from(*addr) + i as u32, Size::Byte, u32::from(b));
            }
        }
        Self {
            mem,
            ports: Vec::new(),
        }
    }

    fn byte(&self, addr: u16) -> u8 {
        self.mem.peek(u32::from(addr), Size::Byte) as u8
    }
}

impl Bus for TestBus {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        if addr >= IO_BASE {
            return addr & 0xff;
        }
        self.mem.read(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.mem.peek(addr & 0xffff, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        if addr >= IO_BASE {
            self.ports.push(((addr - IO_BASE) as u16, v as u8));
        } else {
            self.mem.write(addr, size, v);
        }
    }

    fn addr_bits(&self) -> u8 {
        16
    }
}

/// Ticks until the next instruction boundary; returns the T-states.
fn instr(cpu: &mut Z80, bus: &mut TestBus) -> u32 {
    let mut t = 0;
    loop {
        t += 1;
        if let CycleResult::InstrDone { .. } = cpu.tick(bus) {
            return t;
        }
    }
}

#[test]
fn reset_state() {
    let cpu = Z80::new();
    assert_eq!((cpu.pc, cpu.sp, cpu.af()), (0, 0xffff, 0xffff));
    assert!(!cpu.iff1 && !cpu.iff2);
    assert_eq!(cpu.im, 0);
}

#[test]
fn ticks_spread_an_instruction_over_its_t_states() {
    // LD A,$12; LD ($8000),A; JP $0000
    let mut bus = TestBus::with(&[(0, &[0x3e, 0x12, 0x32, 0x00, 0x80, 0xc3, 0x00, 0x00])]);
    let mut cpu = Z80::new();
    assert_eq!(instr(&mut cpu, &mut bus), 7);
    assert_eq!(instr(&mut cpu, &mut bus), 13);
    assert_eq!(bus.byte(0x8000), 0x12);
    assert_eq!(instr(&mut cpu, &mut bus), 10);
    assert_eq!(cpu.pc, 0);
}

#[test]
fn io_ports_live_above_memory() {
    // LD BC,$1234; OUT (C),A; IN A,($56)
    let mut bus = TestBus::with(&[(0, &[0x01, 0x34, 0x12, 0xed, 0x79, 0xdb, 0x56])]);
    let mut cpu = Z80::new();
    cpu.a = 0x9a;
    for _ in 0..3 {
        instr(&mut cpu, &mut bus);
    }
    assert_eq!(bus.ports, [(0x1234, 0x9a)]);
    assert_eq!(cpu.a, 0x56, "port $9A56 read returns its low byte");
}

#[test]
fn interrupt_modes() {
    // EI; HALT at $0000; handlers at $0038 and $1234 (via the IM 2 table).
    let mut bus = TestBus::with(&[
        (0x0000, &[0xfb, 0x76]),
        (0x0038, &[0xc9]),
        (0x80fe, &[0x34, 0x12]),
        (0x1234, &[0xc9]),
    ]);
    let mut cpu = Z80::new();
    cpu.sp = 0x9000;
    cpu.im = 1;
    cpu.set_int(true);
    assert_eq!(instr(&mut cpu, &mut bus), 4, "EI");
    assert!(cpu.ei);
    assert_eq!(
        instr(&mut cpu, &mut bus),
        4,
        "HALT runs before the interrupt"
    );
    assert!(cpu.halted);
    assert_eq!(instr(&mut cpu, &mut bus), 13, "IM 1 response");
    assert_eq!(cpu.take_interrupt(), Some(InterruptKind::Irq));
    assert_eq!((cpu.pc, cpu.iff1, cpu.halted), (0x38, false, false));
    assert_eq!(bus.byte(0x8ffe), 0x02, "returns after the HALT");

    cpu.im = 2;
    cpu.i = 0x80;
    cpu.set_int_vector(0xfe);
    cpu.iff1 = true;
    cpu.pc = 0x0002;
    assert_eq!(instr(&mut cpu, &mut bus), 19, "IM 2 response");
    assert_eq!(cpu.pc, 0x1234);

    cpu.im = 0;
    cpu.set_int_vector(0xd7); // RST 10h
    cpu.iff1 = true;
    instr(&mut cpu, &mut bus);
    assert_eq!(cpu.pc, 0x10);
}

#[test]
fn nmi_is_edge_triggered_and_keeps_iff2() {
    let mut bus = TestBus::with(&[(0, &[0x00; 16])]);
    let mut cpu = Z80::new();
    cpu.sp = 0x9000;
    cpu.iff1 = true;
    cpu.iff2 = true;
    cpu.set_nmi(true);
    assert_eq!(instr(&mut cpu, &mut bus), 11);
    assert_eq!(cpu.pc, 0x66);
    assert_eq!(cpu.take_interrupt(), Some(InterruptKind::Nmi));
    assert!(!cpu.iff1 && cpu.iff2);
    assert_eq!(instr(&mut cpu, &mut bus), 4, "held line: no second NMI");
    cpu.set_nmi(false);
    cpu.set_nmi(true);
    instr(&mut cpu, &mut bus);
    assert_eq!(cpu.pc, 0x66);
}

#[test]
fn no_interrupt_between_prefix_and_instruction() {
    // DD DD 21 34 12: the first DD acts alone, the second loads IX.
    let mut bus = TestBus::with(&[(0, &[0xdd, 0xdd, 0x21, 0x34, 0x12])]);
    let mut cpu = Z80::new();
    cpu.sp = 0x9000;
    cpu.im = 1;
    cpu.iff1 = true;
    assert_eq!(instr(&mut cpu, &mut bus), 4);
    cpu.set_int(true);
    assert_eq!(
        instr(&mut cpu, &mut bus),
        14,
        "LD IX,nn before the interrupt"
    );
    assert_eq!(cpu.ix, 0x1234);
    assert_eq!(instr(&mut cpu, &mut bus), 13);
}

#[test]
fn registers_through_the_debugger_interface() {
    let mut cpu = Z80::new();
    cpu.set_reg("HL", 0xbeef);
    cpu.set_reg("ix", 0x1111);
    cpu.set_reg("AF'", 0x2222);
    let regs = cpu.regs();
    assert_eq!(regs.get("H"), Some(0xbe));
    assert_eq!(regs.get("IX"), Some(0x1111));
    assert_eq!(regs.get("AF'"), Some(0x2222));
    assert_eq!(regs.get("PC"), Some(0));
}

#[test]
fn savestate_round_trip_mid_instruction() {
    // LD HL,$0000; LDIR with BC = 3.
    let mut bus = TestBus::with(&[(0, &[0x21, 0x00, 0x00, 0xed, 0xb0])]);
    let mut cpu = Z80::new();
    cpu.set_bc(3);
    cpu.set_de(0x4000);
    for _ in 0..15 {
        cpu.tick(&mut bus);
    }
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let data = w.into_bytes();
    let mut other = Z80::new();
    other.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(other, cpu);
    let mut bus2 = TestBus::with(&[(0, &[0x21, 0x00, 0x00, 0xed, 0xb0])]);
    for _ in 0..15 {
        cpu.tick(&mut bus);
    }
    // The second machine's bus starts without the first 15 T-states'
    // writes; compare the CPUs only.
    for _ in 0..15 {
        other.tick(&mut bus2);
    }
    assert_eq!(other.regs(), cpu.regs());
    assert!(other
        .load(&mut StateReader::new(&data[..data.len() - 1]))
        .is_err());
}
