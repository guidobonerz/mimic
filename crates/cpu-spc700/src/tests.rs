use super::*;

struct Ram(Vec<u8>);

impl SpcBus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.0[usize::from(addr)]
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.0[usize::from(addr)] = v;
    }
    fn idle(&mut self) {}
    fn peek(&self, addr: u16) -> u8 {
        self.0[usize::from(addr)]
    }
}

fn at(program: &[u8]) -> (Spc700, Ram) {
    let mut ram = Ram(vec![0; 0x10000]);
    ram.0[0x200..0x200 + program.len()].copy_from_slice(program);
    ram.0[0xfffe] = 0x00;
    ram.0[0xffff] = 0x02;
    let mut cpu = Spc700::new();
    cpu.reset(&mut ram);
    assert_eq!(cpu.pc, 0x200);
    (cpu, ram)
}

#[test]
fn moves_arithmetic_and_cycles() {
    // MOV A,#$12; MOV X,#$34; ADC A,#$F0; MOV $10,A; MUL YA (Y = 0)
    let (mut cpu, mut ram) = at(&[0xe8, 0x12, 0xcd, 0x34, 0x88, 0xf0, 0xc4, 0x10, 0xcf]);
    let cycles: Vec<u32> = (0..5).map(|_| cpu.step(&mut ram)).collect();
    assert_eq!(cycles, [2, 2, 2, 4, 9]);
    assert_eq!(ram.0[0x10], 0x02);
    assert_eq!((cpu.a, cpu.y), (0, 0), "MUL: 0 × 2");
    assert_eq!(cpu.psw & Z, Z);
}

#[test]
fn call_and_return() {
    // CALL $0300; at $0300: INC A; RET
    let (mut cpu, mut ram) = at(&[0x3f, 0x00, 0x03]);
    ram.0[0x300] = 0xbc;
    ram.0[0x301] = 0x6f;
    assert_eq!(cpu.step(&mut ram), 8);
    assert_eq!(cpu.pc, 0x300);
    cpu.step(&mut ram);
    assert_eq!(cpu.step(&mut ram), 5);
    assert_eq!((cpu.pc, cpu.a, cpu.sp), (0x203, 1, 0xef));
}

#[test]
fn div_and_decimal_adjust() {
    // MOV A,#$64; MOV Y,#$00; MOV X,#$07; DIV YA,X -> A 14, Y 2
    let (mut cpu, mut ram) = at(&[0xe8, 0x64, 0x8d, 0x00, 0xcd, 0x07, 0x9e]);
    for _ in 0..3 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.step(&mut ram), 12);
    assert_eq!((cpu.a, cpu.y), (14, 2));
    // SETC; MOV A,#$09; ADC A,#$01 (binary $0B); DAA -> $11 (C was in)
    let (mut cpu, mut ram) = at(&[0x60, 0xe8, 0x09, 0x88, 0x01, 0xdf]);
    for _ in 0..4 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.a, 0x10);
}

#[test]
fn disassembly() {
    let mut ram = Ram(vec![0; 0x10000]);
    ram.0[0x400..0x405].copy_from_slice(&[0xfa, 0x12, 0x34, 0x2f, 0xfe]);
    let i = disassemble(&ram, 0x400);
    assert_eq!(
        (i.mnemonic.as_str(), i.operand.as_str()),
        ("MOV", "$34,$12")
    );
    let i = disassemble(&ram, 0x403);
    assert_eq!((i.operand.as_str(), i.target), ("$0403", Some(0x403)));
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut ram) = at(&[0xe8, 0x12]);
    cpu.step(&mut ram);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let data = w.into_bytes();
    let mut c = Spc700::new();
    c.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(c.regs().0, cpu.regs().0);
}
