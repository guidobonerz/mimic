use super::*;

/// 16 MB of RAM, recording the cycles.
struct Ram {
    mem: Vec<u8>,
    log: Vec<(u32, Option<u8>, char)>,
    nmi: bool,
    irq: bool,
}

impl Ram {
    fn new() -> Self {
        Self {
            mem: vec![0; 1 << 24],
            log: Vec::new(),
            nmi: false,
            irq: false,
        }
    }

    fn load(&mut self, addr: u32, bytes: &[u8]) {
        let a = addr as usize;
        self.mem[a..a + bytes.len()].copy_from_slice(bytes);
    }
}

impl Bus816 for Ram {
    fn read(&mut self, addr: u32) -> u8 {
        let v = self.mem[addr as usize];
        self.log.push((addr, Some(v), 'r'));
        v
    }
    fn write(&mut self, addr: u32, v: u8) {
        self.mem[addr as usize] = v;
        self.log.push((addr, Some(v), 'w'));
    }
    fn idle(&mut self, addr: u32) {
        self.log.push((addr, None, 'i'));
    }
    fn peek(&self, addr: u32) -> u8 {
        self.mem[addr as usize]
    }
    fn nmi(&mut self) -> bool {
        self.nmi
    }
    fn irq(&mut self) -> bool {
        self.irq
    }
}

fn booted(program: &[u8]) -> (Cpu65816, Ram) {
    let mut ram = Ram::new();
    ram.load(0xfffc, &[0x00, 0x80]);
    ram.load(0x8000, program);
    let mut cpu = Cpu65816::new();
    cpu.reset();
    let (cycles, _) = cpu.step(&mut ram);
    assert_eq!(cycles, 7, "reset sequence");
    assert_eq!(cpu.pc, 0x8000);
    (cpu, ram)
}

#[test]
fn native_mode_16_bit_arithmetic() {
    // CLC; XCE; REP #$30; LDA #$1234; CLC; ADC #$0FFF; STA $10; LDX #$ABCD
    let (mut cpu, mut ram) = booted(&[
        0x18, 0xfb, 0xc2, 0x30, 0xa9, 0x34, 0x12, 0x18, 0x69, 0xff, 0x0f, 0x85, 0x10, 0xa2, 0xcd,
        0xab,
    ]);
    let mut cycles = Vec::new();
    for _ in 0..8 {
        cycles.push(cpu.step(&mut ram).0);
    }
    assert!(!cpu.e);
    assert_eq!(cpu.a, 0x2233);
    assert_eq!(&ram.mem[0x10..0x12], &[0x33, 0x22]);
    assert_eq!(cpu.x, 0xabcd);
    assert_eq!(
        cycles,
        [2, 2, 3, 3, 2, 3, 4, 3],
        "16-bit immediates and dp add a cycle"
    );
}

#[test]
fn decimal_mode() {
    // SED; CLC; LDA #$58; ADC #$46 -> $04, C (58 + 46 = 104)
    let (mut cpu, mut ram) = booted(&[0xf8, 0x18, 0xa9, 0x58, 0x69, 0x46]);
    for _ in 0..4 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.a & 0xff, 0x04);
    assert_eq!(cpu.p & C, C);
}

#[test]
fn nmi_is_taken_after_the_instruction_and_uses_the_native_vector() {
    // CLC; XCE; NOP; NOP
    let (mut cpu, mut ram) = booted(&[0x18, 0xfb, 0xea, 0xea]);
    ram.load(0xffea, &[0x00, 0x90]);
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    ram.nmi = true;
    cpu.step(&mut ram); // NOP sees the edge before its last cycle
    let (cycles, entered) = cpu.step(&mut ram);
    assert_eq!(entered, Some(InterruptKind::Nmi));
    assert_eq!(cycles, 8, "native interrupt: 8 cycles");
    assert_eq!(cpu.pc24(), 0x9000);
    assert_eq!(cpu.p & I, I);
    // Level-triggered IRQ masked by I.
    ram.irq = true;
    ram.load(0x9000, &[0xea, 0x58, 0xea, 0xea]);
    cpu.step(&mut ram);
    assert_eq!(cpu.pc24(), 0x9001, "masked");
}

#[test]
fn wai_waits_for_an_interrupt() {
    // SEI; WAI; NOP
    let (mut cpu, mut ram) = booted(&[0x78, 0xcb, 0xea]);
    cpu.step(&mut ram);
    cpu.step(&mut ram);
    assert!(cpu.waiting);
    for _ in 0..5 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.pc24(), 0x8002);
    ram.irq = true;
    cpu.step(&mut ram);
    assert!(!cpu.waiting, "IRQ wakes it even with I set");
    cpu.step(&mut ram);
    assert_eq!(cpu.pc24(), 0x8003, "and it goes on without the handler");
}

#[test]
fn block_move_repeats_itself() {
    // CLC; XCE; REP #$30; LDA #2; LDX #$1000; LDY #$2000; MVN $7E,$7F
    let (mut cpu, mut ram) = booted(&[
        0x18, 0xfb, 0xc2, 0x30, 0xa9, 0x02, 0x00, 0xa2, 0x00, 0x10, 0xa0, 0x00, 0x20, 0x54, 0x7e,
        0x7f,
    ]);
    ram.load(0x7f_1000, &[1, 2, 3, 4]);
    for _ in 0..6 {
        cpu.step(&mut ram);
    }
    let mut n = 0;
    while cpu.pc == 0x800d {
        assert_eq!(cpu.step(&mut ram).0, 7);
        n += 1;
    }
    assert_eq!(n, 3);
    assert_eq!(&ram.mem[0x7e_2000..0x7e_2004], &[1, 2, 3, 0]);
    assert_eq!(
        (cpu.a, cpu.x, cpu.y, cpu.dbr),
        (0xffff, 0x1003, 0x2003, 0x7e)
    );
}

#[test]
fn disassembly_follows_the_widths() {
    let mut ram = Ram::new();
    ram.load(0x12_3456, &[0xa9, 0x34, 0x12, 0x22, 0x56, 0x34, 0x12]);
    let i = disassemble(&ram, 0x12_3456, false, true);
    assert_eq!((i.mnemonic.as_str(), i.operand.as_str()), ("LDA", "#$1234"));
    let i = disassemble(&ram, 0x12_3456, true, true);
    assert_eq!(i.bytes.len(), 2);
    let i = disassemble(&ram, 0x12_3459, true, true);
    assert_eq!((i.mnemonic.as_str(), i.target), ("JSL", Some(0x12_3456)));
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut ram) = booted(&[0x18, 0xfb, 0xc2, 0x30, 0xa9, 0x34, 0x12]);
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let data = w.into_bytes();
    let mut c = Cpu65816::new();
    c.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(c.regs().0, cpu.regs().0);
    assert!(c
        .load(&mut StateReader::new(&data[..data.len() - 1]))
        .is_err());
}
