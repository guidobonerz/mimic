//! Own per-instruction tests with hand-encoded instructions, from the
//! RISC-V manuals' descriptions and the rvlite notes.

use super::*;
use crate::exec::{EBREAK, ECALL, MRET, WFI};
use mimic_core::Flow;

/// 64 KB of RAM, mirrored through the whole address space.
struct Ram {
    m: Vec<u8>,
    fetches: u32,
}

impl Ram {
    fn with(code: &[u32]) -> Self {
        let mut r = Self {
            m: vec![0; 0x10000],
            fetches: 0,
        };
        for (i, w) in code.iter().enumerate() {
            r.poke32(i as u32 * 4, *w);
        }
        r
    }
    fn byte(&self, a: u32) -> u8 {
        self.m[a as usize & 0xffff]
    }
    fn poke(&mut self, a: u32, v: u8) {
        self.m[a as usize & 0xffff] = v;
    }
    fn peek32(&self, a: u32) -> u32 {
        u32::from_le_bytes([0, 1, 2, 3].map(|n| self.byte(a.wrapping_add(n))))
    }
    fn poke32(&mut self, a: u32, v: u32) {
        for (n, b) in v.to_le_bytes().into_iter().enumerate() {
            self.poke(a.wrapping_add(n as u32), b);
        }
    }
}

impl Rv32Bus for Ram {
    fn fetch(&mut self, addr: u32) -> u32 {
        self.fetches += 1;
        self.peek32(addr)
    }
    fn read8(&mut self, addr: u32) -> u8 {
        self.byte(addr)
    }
    fn read16(&mut self, addr: u32) -> u16 {
        self.peek32(addr) as u16
    }
    fn read32(&mut self, addr: u32) -> u32 {
        self.peek32(addr)
    }
    fn write8(&mut self, addr: u32, v: u8) {
        self.poke(addr, v);
    }
    fn write16(&mut self, addr: u32, v: u16) {
        self.poke(addr, v as u8);
        self.poke(addr.wrapping_add(1), (v >> 8) as u8);
    }
    fn write32(&mut self, addr: u32, v: u32) {
        self.poke32(addr, v);
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.peek32(addr) & size.mask()
    }
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        for n in 0..size.bytes() {
            self.poke(addr.wrapping_add(n), (v >> (8 * n)) as u8);
        }
    }
    fn addr_bits(&self) -> u8 {
        32
    }
}

// --- encoder ---------------------------------------------------------

const ZERO: u32 = 0;
const RA: u32 = 1;
const SP: u32 = 2;
const T0: u32 = 5;
const S0: u32 = 8;
const A0: u32 = 10;
const A1: u32 = 11;
const A2: u32 = 12;

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    funct7 << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode
}
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (imm as u32 & 0xfff) << 20 | rs1 << 15 | funct3 << 12 | rd << 7 | opcode
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32) -> u32 {
    let imm = imm as u32;
    (imm >> 5 & 0x7f) << 25 | rs2 << 20 | rs1 << 15 | funct3 << 12 | (imm & 0x1f) << 7 | 0x23
}
fn b_type(offset: i32, rs2: u32, rs1: u32, funct3: u32) -> u32 {
    let imm = offset as u32;
    (imm >> 12 & 1) << 31
        | (imm >> 5 & 0x3f) << 25
        | rs2 << 20
        | rs1 << 15
        | funct3 << 12
        | (imm >> 1 & 0xf) << 8
        | (imm >> 11 & 1) << 7
        | 0x63
}
fn u_type(imm20: u32, rd: u32, opcode: u32) -> u32 {
    (imm20 & 0xf_ffff) << 12 | rd << 7 | opcode
}
fn jal(rd: u32, offset: i32) -> u32 {
    let imm = offset as u32;
    (imm >> 20 & 1) << 31
        | (imm >> 1 & 0x3ff) << 21
        | (imm >> 11 & 1) << 20
        | (imm >> 12 & 0xff) << 12
        | rd << 7
        | 0x6f
}
fn jalr(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x67)
}
fn lui(rd: u32, imm20: u32) -> u32 {
    u_type(imm20, rd, 0x37)
}
fn auipc(rd: u32, imm20: u32) -> u32 {
    u_type(imm20, rd, 0x17)
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, 0, rd, 0x13)
}
/// OP-IMM with `funct3`.
fn opi(funct3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, funct3, rd, 0x13)
}
/// OP with `funct7` and `funct3`.
fn op(funct7: u32, funct3: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    r_type(funct7, rs2, rs1, funct3, rd, 0x33)
}
fn load(funct3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
    i_type(imm, rs1, funct3, rd, 0x03)
}
/// CSR instruction: `funct3` 1 CSRRW, 2 CSRRS, 3 CSRRC, 5–7 the immediate
/// forms (`src` is then the 5-bit immediate).
fn csr(funct3: u32, rd: u32, addr: u16, src: u32) -> u32 {
    u32::from(addr) << 20 | src << 15 | funct3 << 12 | rd << 7 | 0x73
}
const NOP: u32 = 0x13;

fn start(code: &[u32]) -> (Rv32, Ram) {
    let mut cpu = Rv32::new(Model::Rvlite);
    cpu.reset(0);
    (cpu, Ram::with(code))
}

/// Run `code` straight through, one step per word.
fn run(code: &[u32]) -> (Rv32, Ram) {
    let (mut cpu, mut bus) = start(code);
    for _ in code {
        assert_eq!(cpu.step(&mut bus), StepResult::Done);
    }
    (cpu, bus)
}

/// `a` OP `b` with the given funct7/funct3.
fn alu(funct7: u32, funct3: u32, a: u32, b: u32) -> u32 {
    let (mut cpu, mut bus) = start(&[op(funct7, funct3, 3, 1, 2)]);
    cpu.set_x(1, a);
    cpu.set_x(2, b);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    cpu.x(3)
}

/// `a` OP-IMM `imm` with the given funct3.
fn alu_imm(funct3: u32, a: u32, imm: i32) -> u32 {
    let (mut cpu, mut bus) = start(&[opi(funct3, 3, 1, imm)]);
    cpu.set_x(1, a);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    cpu.x(3)
}

// --- tests -----------------------------------------------------------

#[test]
fn encoder_matches_known_words() {
    assert_eq!(addi(SP, SP, -120), 0xf881_0113);
    assert_eq!(jalr(ZERO, RA, 0), 0x0000_8067);
    assert_eq!(csr(7, ZERO, CSR_MSTATUS, 8), 0x3004_7073);
    assert_eq!(jal(RA, 8), 0x0080_00ef);
    assert_eq!(b_type(8, A1, A0, 0), 0x00b5_0463);
    assert_eq!(s_type(4, A0, SP, 2), 0x00a1_2223);
    assert_eq!(load(2, A0, S0, -4), 0xffc4_2503);
    assert_eq!(lui(A0, 0x12345), 0x1234_5537);
    assert_eq!(op(1, 0, A0, A1, A2), 0x02c5_8533);
}

#[test]
fn arithmetic_and_logic() {
    assert_eq!(alu(0, 0, 7, 0xffff_ffff), 6);
    assert_eq!(alu(0x20, 0, 3, 5), 0xffff_fffe);
    assert_eq!(alu(0, 4, 0xff00, 0x0ff0), 0xf0f0);
    assert_eq!(alu(0, 6, 0xff00, 0x0ff0), 0xfff0);
    assert_eq!(alu(0, 7, 0xff00, 0x0ff0), 0x0f00);
    assert_eq!(alu_imm(0, 5, -7), 0xffff_fffe);
    assert_eq!(alu_imm(0, 0x7fff_ffff, 1), 0x8000_0000);
    assert_eq!(alu_imm(4, 0x0f0f, -1), 0xffff_f0f0);
    assert_eq!(alu_imm(6, 0x1000, 0x7ff), 0x17ff);
    assert_eq!(alu_imm(7, 0x1234_5678, -16), 0x1234_5670);

    let (cpu, _) = run(&[NOP, lui(3, 0xfffff), auipc(4, 0x12345), auipc(5, 0xfffff)]);
    assert_eq!(cpu.x(3), 0xffff_f000);
    assert_eq!(cpu.x(4), 0x1234_5008);
    assert_eq!(cpu.x(5), 0xffff_f00c);
    assert_eq!(cpu.pc(), 16);
}

#[test]
fn shifts() {
    assert_eq!(alu(0, 1, 0x8000_0001, 4), 0x10);
    assert_eq!(alu(0, 5, 0x8000_0010, 4), 0x0800_0001);
    assert_eq!(alu(0x20, 5, 0x8000_0010, 4), 0xf800_0001);
    // Only the low five bits of rs2 count.
    assert_eq!(alu(0, 1, 1, 33), 2);
    assert_eq!(alu(0, 5, 0x8000_0000, 63), 1);
    assert_eq!(alu(0x20, 5, 0x8000_0000, 63), 0xffff_ffff);
    assert_eq!(alu_imm(1, 1, 31), 0x8000_0000);
    assert_eq!(alu_imm(5, 0x8000_0000, 31), 1);
    assert_eq!(alu_imm(5, 0xf000_0000, 0x400 | 4), 0xff00_0000);
    assert_eq!(alu_imm(1, 0x1234, 0), 0x1234);

    // Other upper bits are not instructions.
    for word in [opi(1, 3, 1, 0x400 | 1), opi(5, 3, 1, 0x200 | 1)] {
        let (mut cpu, mut bus) = start(&[word]);
        assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
    }
}

#[test]
fn compares() {
    let neg = 0xffff_ffff;
    assert_eq!(alu(0, 2, neg, 1), 1);
    assert_eq!(alu(0, 2, 1, neg), 0);
    assert_eq!(alu(0, 2, 5, 5), 0);
    assert_eq!(alu(0, 3, neg, 1), 0);
    assert_eq!(alu(0, 3, 1, neg), 1);
    assert_eq!(alu_imm(2, neg, 0), 1);
    assert_eq!(alu_imm(2, 0, -1), 0);
    // SLTIU compares with the sign-extended immediate as unsigned.
    assert_eq!(alu_imm(3, 5, -1), 1);
    assert_eq!(alu_imm(3, neg, -1), 0);
    assert_eq!(alu_imm(3, 0, 1), 1);
}

#[test]
fn branches() {
    let neg = 0xffff_ffff;
    // (funct3, rs1, rs2, taken)
    let cases = [
        (0, 5, 5, true),
        (0, 5, 6, false),
        (1, 5, 6, true),
        (1, 5, 5, false),
        (4, neg, 1, true),
        (4, 1, neg, false),
        (4, 3, 3, false),
        (5, 1, neg, true),
        (5, 3, 3, true),
        (5, neg, 1, false),
        (6, 1, neg, true),
        (6, neg, 1, false),
        (7, neg, 1, true),
        (7, 3, 3, true),
        (7, 1, neg, false),
    ];
    for (funct3, a, b, taken) in cases {
        let (mut cpu, mut bus) = start(&[NOP, b_type(0x40, 2, 1, funct3)]);
        cpu.set_x(1, a);
        cpu.set_x(2, b);
        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), StepResult::Done);
        let want = if taken { 0x44 } else { 8 };
        assert_eq!(cpu.pc(), want, "funct3 {funct3} {a:x} {b:x}");
    }

    // Backwards, and the far end of the range.
    let (mut cpu, mut bus) = start(&[NOP, NOP, b_type(-8, 0, 0, 0)]);
    cpu.run(&mut bus, 3);
    assert_eq!(cpu.pc(), 0);
    let (mut cpu, mut bus) = start(&[b_type(-4096, 0, 0, 0)]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc(), 0xffff_f000);
    let (mut cpu, mut bus) = start(&[b_type(4094, 0, 0, 0)]);
    cpu.step(&mut bus);
    // Bits 1–0 of a target are dropped.
    assert_eq!(cpu.pc(), 4092);

    // funct3 2 and 3 are no branches.
    let word = b_type(8, 0, 0, 2);
    let (mut cpu, mut bus) = start(&[word]);
    assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
}

#[test]
fn jal_and_jalr() {
    let (mut cpu, mut bus) = start(&[NOP, jal(RA, 0x100)]);
    cpu.run(&mut bus, 2);
    assert_eq!(cpu.pc(), 0x104);
    assert_eq!(cpu.x(1), 8);

    let (mut cpu, mut bus) = start(&[NOP, NOP, jal(T0, -8)]);
    cpu.run(&mut bus, 3);
    assert_eq!(cpu.pc(), 0);
    assert_eq!(cpu.x(5), 12);

    let (mut cpu, mut bus) = start(&[jal(RA, -0x10_0000)]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc(), 0xfff0_0000);
    let (mut cpu, mut bus) = start(&[jal(RA, 0xf_fffe)]);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc(), 0xf_fffc);

    // JALR: target from the old rs1 even if rd is the same register; the
    // low bits of the target are dropped.
    let (mut cpu, mut bus) = start(&[jalr(RA, RA, -3)]);
    cpu.set_x(1, 0x2002);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc(), 0x1ffc);
    assert_eq!(cpu.x(1), 4);

    let word = i_type(0, RA, 1, RA, 0x67);
    let (mut cpu, mut bus) = start(&[word]);
    assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
}

#[test]
fn loads_extend() {
    let (mut cpu, mut bus) = start(&[
        load(0, 3, 1, 0),
        load(4, 4, 1, 0),
        load(1, 5, 1, 0),
        load(5, 6, 1, 0),
        load(2, 7, 1, 0),
        load(0, 8, 1, 4),
        load(1, 9, 1, 4),
        load(2, 10, 2, -4),
    ]);
    bus.poke32(0x1000, 0x8182_8384);
    bus.poke32(0x1004, 0x0102_7f04);
    cpu.set_x(1, 0x1000);
    cpu.set_x(2, 0x1004);
    cpu.run(&mut bus, 8);
    assert_eq!(cpu.x(3), 0xffff_ff84);
    assert_eq!(cpu.x(4), 0x84);
    assert_eq!(cpu.x(5), 0xffff_8384);
    assert_eq!(cpu.x(6), 0x8384);
    assert_eq!(cpu.x(7), 0x8182_8384);
    assert_eq!(cpu.x(8), 4);
    assert_eq!(cpu.x(9), 0x7f04);
    assert_eq!(cpu.x(10), 0x8182_8384);

    for funct3 in [3, 6, 7] {
        let word = load(funct3, 3, 1, 0);
        let (mut cpu, mut bus) = start(&[word]);
        assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
    }
}

#[test]
fn stores() {
    let (mut cpu, mut bus) = start(&[
        s_type(0, 2, 1, 2),
        s_type(4, 2, 1, 1),
        s_type(8, 2, 1, 0),
        s_type(-4, 2, 1, 2),
        s_type(0x7ff, 2, 1, 0),
        s_type(-0x800, 2, 1, 0),
    ]);
    bus.poke32(0x1004, 0xffff_ffff);
    bus.poke32(0x1008, 0xffff_ffff);
    cpu.set_x(1, 0x1000);
    cpu.set_x(2, 0x1122_3344);
    cpu.run(&mut bus, 6);
    assert_eq!(bus.peek32(0x1000), 0x1122_3344);
    assert_eq!(bus.peek32(0x1004), 0xffff_3344);
    assert_eq!(bus.peek32(0x1008), 0xffff_ff44);
    assert_eq!(bus.peek32(0x0ffc), 0x1122_3344);
    assert_eq!(bus.byte(0x17ff), 0x44);
    assert_eq!(bus.byte(0x0800), 0x44);

    let word = s_type(0, 2, 1, 3);
    let (mut cpu, mut bus) = start(&[word]);
    assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
}

#[test]
fn unaligned_access_does_not_trap() {
    let (mut cpu, mut bus) = start(&[
        s_type(1, 2, 1, 2),
        load(2, 3, 1, 1),
        load(2, 4, 1, 3),
        s_type(7, 2, 1, 1),
        load(5, 5, 1, 7),
        load(1, 6, 1, 3),
    ]);
    cpu.set_x(1, 0x1000);
    cpu.set_x(2, 0xa1b2_c3d4);
    assert_eq!(cpu.run(&mut bus, 6), StepResult::Done);
    assert_eq!(bus.byte(0x1000), 0);
    assert_eq!(
        [1, 2, 3, 4].map(|n| bus.byte(0x1000 + n)),
        [0xd4, 0xc3, 0xb2, 0xa1]
    );
    assert_eq!(bus.byte(0x1005), 0);
    assert_eq!(cpu.x(3), 0xa1b2_c3d4);
    assert_eq!(cpu.x(4), 0x0000_a1b2);
    assert_eq!(cpu.x(5), 0xc3d4);
    assert_eq!(cpu.x(6), 0xffff_a1b2);
    assert_eq!(cpu.mcause(), 0);
}

#[test]
fn multiply() {
    let neg = |v: i32| v as u32;
    // MUL
    assert_eq!(alu(1, 0, 7, 6), 42);
    assert_eq!(alu(1, 0, neg(-7), 6), neg(-42));
    assert_eq!(alu(1, 0, 0x1_0000, 0x1_0000), 0);
    assert_eq!(alu(1, 0, 0x8000_0000, 0x8000_0000), 0);
    // MULH: signed × signed
    assert_eq!(alu(1, 1, neg(-1), neg(-1)), 0);
    assert_eq!(alu(1, 1, neg(-1), 1), 0xffff_ffff);
    assert_eq!(alu(1, 1, 0x8000_0000, 0x8000_0000), 0x4000_0000);
    assert_eq!(alu(1, 1, 0x7fff_ffff, 0x7fff_ffff), 0x3fff_ffff);
    assert_eq!(alu(1, 1, 0x8000_0000, 0x7fff_ffff), 0xc000_0000);
    // MULHSU: signed rs1 × unsigned rs2
    assert_eq!(alu(1, 2, neg(-1), 0xffff_ffff), 0xffff_ffff);
    assert_eq!(alu(1, 2, 0xffff_ffff, 1), 0xffff_ffff);
    assert_eq!(alu(1, 2, 1, 0xffff_ffff), 0);
    assert_eq!(alu(1, 2, 0x8000_0000, 0xffff_ffff), 0x8000_0000);
    assert_eq!(alu(1, 2, 0x7fff_ffff, 0xffff_ffff), 0x7fff_fffe);
    // MULHU: unsigned × unsigned
    assert_eq!(alu(1, 3, 0xffff_ffff, 0xffff_ffff), 0xffff_fffe);
    assert_eq!(alu(1, 3, 0x8000_0000, 2), 1);
    assert_eq!(alu(1, 3, 0xffff, 0xffff), 0);
}

#[test]
fn divide_and_remainder() {
    let neg = |v: i32| v as u32;
    let min = 0x8000_0000;
    // DIV rounds towards zero.
    assert_eq!(alu(1, 4, 20, 6), 3);
    assert_eq!(alu(1, 4, neg(-20), 6), neg(-3));
    assert_eq!(alu(1, 4, 20, neg(-6)), neg(-3));
    assert_eq!(alu(1, 4, neg(-20), neg(-6)), 3);
    assert_eq!(alu(1, 4, 20, 0), 0xffff_ffff);
    assert_eq!(alu(1, 4, min, neg(-1)), min);
    assert_eq!(alu(1, 4, min, 1), min);
    // DIVU
    assert_eq!(alu(1, 5, neg(-20), 6), 0x2aaa_aaa7);
    assert_eq!(alu(1, 5, 20, 0), 0xffff_ffff);
    assert_eq!(alu(1, 5, min, neg(-1)), 0);
    // REM has the dividend's sign.
    assert_eq!(alu(1, 6, 20, 6), 2);
    assert_eq!(alu(1, 6, neg(-20), 6), neg(-2));
    assert_eq!(alu(1, 6, 20, neg(-6)), 2);
    assert_eq!(alu(1, 6, neg(-20), neg(-6)), neg(-2));
    assert_eq!(alu(1, 6, neg(-20), 0), neg(-20));
    assert_eq!(alu(1, 6, min, neg(-1)), 0);
    // REMU
    assert_eq!(alu(1, 7, neg(-20), 6), 2);
    assert_eq!(alu(1, 7, 20, 0), 20);
    assert_eq!(alu(1, 7, min, neg(-1)), min);

    // Other funct7 values are not instructions.
    for word in [
        op(2, 0, 3, 1, 2),
        op(0x20, 1, 3, 1, 2),
        op(0x21, 0, 3, 1, 2),
    ] {
        let (mut cpu, mut bus) = start(&[word]);
        assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
    }
}

#[test]
fn csr_instructions() {
    let (mut cpu, mut bus) = start(&[
        csr(1, 3, CSR_MSCRATCH, 1),  // csrrw x3,mscratch,x1
        csr(2, 4, CSR_MSCRATCH, 2),  // csrrs x4,mscratch,x2
        csr(3, 5, CSR_MSCRATCH, 1),  // csrrc x5,mscratch,x1
        csr(5, 6, CSR_MSCRATCH, 9),  // csrrwi x6,mscratch,9
        csr(6, 7, CSR_MSCRATCH, 6),  // csrrsi x7,mscratch,6
        csr(7, 8, CSR_MSCRATCH, 12), // csrrci x8,mscratch,12
        csr(1, 1, CSR_MSCRATCH, 1),  // csrrw x1,mscratch,x1: a swap
    ]);
    cpu.csr_write(CSR_MSCRATCH, 0xdead_0000);
    cpu.set_x(1, 0x0000_ff0f);
    cpu.set_x(2, 0x00f0_00f0);
    cpu.step(&mut bus);
    assert_eq!((cpu.x(3), cpu.mscratch()), (0xdead_0000, 0x0000_ff0f));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(4), cpu.mscratch()), (0x0000_ff0f, 0x00f0_ffff));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(5), cpu.mscratch()), (0x00f0_ffff, 0x00f0_00f0));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(6), cpu.mscratch()), (0x00f0_00f0, 9));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(7), cpu.mscratch()), (9, 15));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(8), cpu.mscratch()), (15, 3));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(1), cpu.mscratch()), (3, 0x0000_ff0f));
    assert_eq!(cpu.pc(), 28);

    // funct3 4 is not a CSR instruction.
    let word = csr(4, 3, CSR_MSCRATCH, 1);
    let (mut cpu, mut bus) = start(&[word]);
    assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word));
}

#[test]
fn csr_set_and_clear_without_source_do_not_write() {
    let (mut cpu, mut bus) = start(&[
        csr(2, 3, CSR_MSCRATCH, 0), // csrrs x3,mscratch,x0 (csrr)
        csr(3, 4, CSR_MSCRATCH, 0), // csrrc x4,mscratch,x0
        csr(6, 5, CSR_MSCRATCH, 0), // csrrsi x5,mscratch,0
        csr(7, 6, CSR_MSCRATCH, 0), // csrrci x6,mscratch,0
        csr(1, 0, CSR_MSCRATCH, 1), // csrrw x0,mscratch,x1 (csrw)
        csr(5, 7, CSR_MSCRATCH, 0), // csrrwi x7,mscratch,0 does write
        csr(1, 8, CSR_MSCRATCH, 0), // csrrw x8,mscratch,x0 does write
    ]);
    cpu.csr_write(CSR_MSCRATCH, 0x1234_5678);
    cpu.set_x(1, 0x55);
    cpu.run(&mut bus, 4);
    assert_eq!(
        [3, 4, 5, 6].map(|n| cpu.x(n)),
        [0x1234_5678; 4],
        "each reads the value"
    );
    assert_eq!(cpu.mscratch(), 0x1234_5678);
    cpu.step(&mut bus);
    assert_eq!((cpu.x(0), cpu.mscratch()), (0, 0x55));
    cpu.step(&mut bus);
    assert_eq!((cpu.x(7), cpu.mscratch()), (0x55, 0));
    cpu.csr_write(CSR_MSCRATCH, 0x77);
    cpu.step(&mut bus);
    assert_eq!((cpu.x(8), cpu.mscratch()), (0x77, 0));

    // The line is not disturbed by a set or clear without a source: MEIP
    // is read, nothing is written.
    let (mut cpu, mut bus) = start(&[csr(2, 3, CSR_MIP, 0), csr(3, 4, CSR_MIP, 0)]);
    cpu.set_irq(true);
    cpu.run(&mut bus, 2);
    assert_eq!((cpu.x(3), cpu.x(4)), (MIP_MEIP, MIP_MEIP));
    assert!(cpu.irq());
}

#[test]
fn rvlite_csrs() {
    let mut cpu = Rv32::new(Model::Rvlite);
    cpu.reset(0x8000_0000);
    assert_eq!(cpu.pc(), 0x8000_0000);

    // mstatus keeps MIE and MPIE only (the firmware writes $1880).
    cpu.csr_write(CSR_MSTATUS, 0xffff_ffff);
    assert_eq!(cpu.csr_read(CSR_MSTATUS), MSTATUS_MIE | MSTATUS_MPIE);
    cpu.csr_write(CSR_MSTATUS, 0x1880);
    assert_eq!(cpu.csr_read(CSR_MSTATUS), MSTATUS_MPIE);
    // mie keeps MEIE only.
    cpu.csr_write(CSR_MIE, 0xffff_ffff);
    assert_eq!(cpu.csr_read(CSR_MIE), MIE_MEIE);
    // mtvec: direct mode whatever is written (the firmware once writes an
    // instruction word).
    cpu.csr_write(CSR_MTVEC, 0xf881_0113);
    assert_eq!(cpu.csr_read(CSR_MTVEC), 0xf881_0110);
    cpu.csr_write(CSR_MEPC, 0x1237);
    assert_eq!(cpu.csr_read(CSR_MEPC), 0x1237);
    // mip follows the line and cannot be written.
    cpu.csr_write(CSR_MIP, 0xffff_ffff);
    assert_eq!(cpu.csr_read(CSR_MIP), 0);
    cpu.set_irq(true);
    assert_eq!(cpu.csr_read(CSR_MIP), MIP_MEIP);
    cpu.csr_write(CSR_MIP, 0);
    assert_eq!(cpu.csr_read(CSR_MIP), MIP_MEIP);
    // mcause and mtval are not writable.
    cpu.csr_write(CSR_MCAUSE, 5);
    cpu.csr_write(CSR_MTVAL, 5);
    assert_eq!(cpu.csr_read(CSR_MCAUSE), 0);
    assert_eq!(cpu.csr_read(CSR_MTVAL), 0);
    // Identification.
    assert_eq!(cpu.csr_read(CSR_MISA), 0x4000_0100);
    assert_eq!(cpu.csr_read(CSR_MARCHID), 0x13);
    assert_eq!(cpu.csr_read(CSR_MIMPID), 0x475a_0001);
    assert_eq!(cpu.csr_read(0xf14), 0);

    // Everything else reads 0, ignores writes and never traps: the
    // firmware's start-up clears mcountinhibit, mcounteren and the counters.
    let unknown = [
        0x320, 0x306, 0xb00, 0xb80, 0xb02, 0xb82, 0x000, 0x7c0, 0xfff,
    ];
    let mut code = vec![addi(1, 0, -1)];
    for a in unknown {
        code.push(csr(1, 3, a, 1));
        code.push(csr(2, 4, a, 0));
    }
    let (mut cpu, mut bus) = start(&code);
    cpu.set_x(3, 1);
    cpu.set_x(4, 1);
    for _ in 0..code.len() {
        assert_eq!(cpu.step(&mut bus), StepResult::Done);
        assert_eq!(cpu.mcause(), 0);
    }
    assert_eq!((cpu.x(3), cpu.x(4)), (0, 0));

    // rvlite sign-extends the CSR immediate.
    let (cpu, _) = run(&[csr(6, 0, CSR_MSCRATCH, 0x10)]);
    assert_eq!(cpu.mscratch(), 0xffff_fff0);
    // The firmware's own: csrrsi mstatus,8 and csrrci mstatus,8.
    let (mut cpu, mut bus) = start(&[csr(6, 0, CSR_MSTATUS, 8), csr(7, 3, CSR_MSTATUS, 8)]);
    cpu.step(&mut bus);
    assert_eq!(cpu.mstatus(), MSTATUS_MIE);
    cpu.step(&mut bus);
    assert_eq!((cpu.x(3), cpu.mstatus()), (MSTATUS_MIE, 0));
}

#[test]
fn standard_csrs() {
    let mut cpu = Rv32::new(Model::Standard);
    cpu.reset(0);
    assert_eq!(cpu.model(), Model::Standard);
    assert_eq!(cpu.csr_read(CSR_MSTATUS), MSTATUS_MPP);
    assert_eq!(cpu.csr_read(CSR_MISA), 0x4000_1100);
    assert_eq!(cpu.csr_read(CSR_MARCHID), 0);
    cpu.csr_write(CSR_MCAUSE, 7);
    cpu.csr_write(CSR_MTVAL, 9);
    cpu.csr_write(CSR_MEPC, 0x1237);
    assert_eq!(cpu.csr_read(CSR_MCAUSE), 7);
    assert_eq!(cpu.csr_read(CSR_MTVAL), 9);
    assert_eq!(cpu.csr_read(CSR_MEPC), 0x1234);

    // mcycle and minstret count instructions.
    let mut bus = Ram::with(&[
        NOP,
        NOP,
        NOP,
        csr(2, 3, CSR_MCYCLE, 0),
        csr(2, 4, CSR_MINSTRET, 0),
        csr(2, 5, CSR_MINSTRETH, 0),
        csr(6, 0, CSR_MSCRATCH, 0x10),
        0x0000_0073 | 7 << 20,
    ]);
    cpu.run(&mut bus, 7);
    assert_eq!((cpu.x(3), cpu.x(4), cpu.x(5)), (3, 4, 0));
    assert_eq!(cpu.instructions(), 7);
    // The immediate is zero-extended.
    assert_eq!(cpu.mscratch(), 0x10);
    // An unknown SYSTEM instruction traps and leaves its word in mtval.
    assert_eq!(cpu.step(&mut bus), StepResult::Illegal(0x0070_0073));
    assert_eq!(cpu.csr_read(CSR_MTVAL), 0x0070_0073);
    assert_eq!(cpu.instructions(), 7);

    cpu.csr_write(CSR_MCYCLEH, 2);
    cpu.csr_write(CSR_MINSTRET, 5);
    assert_eq!(cpu.instructions(), 0x2_0000_0005);
    assert_eq!(cpu.csr_read(CSR_MINSTRETH), 2);
    assert_eq!(cpu.csr_read(CSR_MCYCLE), 5);
}

#[test]
fn ecall_and_mret_stack_the_interrupt_enable() {
    let mut code = vec![NOP; 0x40];
    code[2] = ECALL;
    code[0x20] = MRET;
    let (mut cpu, mut bus) = start(&code);
    cpu.csr_write(CSR_MTVEC, 0x80);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE);
    cpu.run(&mut bus, 2);
    assert_eq!(cpu.step(&mut bus), StepResult::Ecall);
    assert_eq!(cpu.pc(), 0x80);
    // mepc is the ECALL itself; the handler adds 4.
    assert_eq!(cpu.mepc(), 8);
    assert_eq!(cpu.mcause(), CAUSE_ECALL_M);
    assert_eq!(cpu.mstatus(), MSTATUS_MPIE);
    assert_eq!(cpu.instructions(), 2);

    cpu.csr_write(CSR_MEPC, cpu.mepc() + 4);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.pc(), 12);
    assert_eq!(cpu.mstatus(), MSTATUS_MIE | MSTATUS_MPIE);

    // With interrupts off: MPIE = 0 in the handler, MIE = 0 and MPIE = 1
    // after MRET.
    let (mut cpu, mut bus) = start(&code);
    cpu.csr_write(CSR_MTVEC, 0x80);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MPIE);
    cpu.run(&mut bus, 3);
    assert_eq!(cpu.mstatus(), 0);
    cpu.step(&mut bus);
    assert_eq!(cpu.mstatus(), MSTATUS_MPIE);
    assert_eq!(cpu.pc(), 8);

    // MRET with a raw mepc: the PC loses the low bits.
    let (mut cpu, mut bus) = start(&[MRET]);
    cpu.csr_write(CSR_MEPC, 0x43);
    cpu.step(&mut bus);
    assert_eq!(cpu.pc(), 0x40);
}

#[test]
fn ebreak_traps_with_cause_3() {
    let (mut cpu, mut bus) = start(&[NOP, EBREAK]);
    cpu.csr_write(CSR_MTVEC, 0x200);
    cpu.step(&mut bus);
    assert_eq!(cpu.step(&mut bus), StepResult::Ebreak);
    assert_eq!((cpu.pc(), cpu.mepc()), (0x200, 4));
    assert_eq!(cpu.mcause(), CAUSE_BREAKPOINT);
}

#[test]
fn illegal_instructions_trap_with_cause_2() {
    let words = [
        0x0000_0000,
        0xffff_ffff,
        0x0000_4501, // a compressed encoding (bits 1–0 = 01)
        0x0000_0002,
        0x1005_252f, // opcode $2F: the A extension
        0x0000_0053, // opcode $53: floating point
        0x0000_007f,
    ];
    for word in words {
        let (mut cpu, mut bus) = start(&[NOP, word]);
        cpu.csr_write(CSR_MTVEC, 0x300);
        cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE);
        cpu.set_x(10, 0x1000);
        cpu.step(&mut bus);
        assert_eq!(cpu.step(&mut bus), StepResult::Illegal(word), "{word:08x}");
        assert_eq!((cpu.pc(), cpu.mepc()), (0x300, 4));
        assert_eq!(cpu.mcause(), CAUSE_ILLEGAL);
        assert_eq!(cpu.mstatus(), MSTATUS_MPIE);
        assert_eq!(cpu.mtval(), word);
        // The guest of the rvlite model reads mtval as 0.
        assert_eq!(cpu.csr_read(CSR_MTVAL), 0);
        assert_eq!(cpu.instructions(), 1);
    }
}

#[test]
fn external_interrupt_needs_line_meie_and_mie() {
    let mut code = vec![NOP; 0x80];
    code[0x40] = MRET;
    let (mut cpu, mut bus) = start(&code);
    cpu.csr_write(CSR_MTVEC, 0x100);

    cpu.set_irq(true);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    cpu.csr_write(CSR_MIE, MIE_MEIE);
    assert!(!cpu.irq_pending());
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    cpu.csr_write(CSR_MIE, 0);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    cpu.csr_write(CSR_MIE, MIE_MEIE);
    cpu.set_irq(false);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.pc(), 16);

    // All three: the trap comes in front of the next instruction.
    cpu.set_irq(true);
    assert!(cpu.irq_pending());
    let fetches = bus.fetches;
    assert_eq!(cpu.step(&mut bus), StepResult::Interrupt);
    assert_eq!(bus.fetches, fetches, "no instruction ran");
    assert_eq!(cpu.instructions(), 4);
    assert_eq!((cpu.pc(), cpu.mepc()), (0x100, 16));
    assert_eq!(cpu.mcause(), CAUSE_EXTERNAL_IRQ);
    assert_eq!(cpu.mcause(), 0x8000_000b);
    assert_eq!(cpu.mstatus(), MSTATUS_MPIE);
    assert_eq!(cpu.mip(), MIP_MEIP);

    // The handler is not interrupted; the line is a level, so MRET with
    // the line still up leads straight back in.
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.pc(), 16);
    assert_eq!(cpu.step(&mut bus), StepResult::Interrupt);
    assert_eq!(cpu.mepc(), 16);
    cpu.set_irq(false);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.pc(), 20);
}

#[test]
fn interrupt_enabled_by_instructions() {
    // addi x1,x0,8 / slli x2,x1,8 (= $800) / csrrs mie / csrrsi mstatus,8
    let (mut cpu, mut bus) = start(&[
        addi(1, 0, 8),
        opi(1, 2, 1, 8),
        csr(2, 0, CSR_MIE, 2),
        csr(6, 0, CSR_MSTATUS, 8),
        NOP,
    ]);
    cpu.csr_write(CSR_MTVEC, 0x40);
    cpu.set_irq(true);
    assert_eq!(cpu.run(&mut bus, 100), StepResult::Interrupt);
    assert_eq!(cpu.mepc(), 16);
    assert_eq!(cpu.instructions(), 4);
    // run stopped at the interrupt entry.
    assert_eq!(cpu.pc(), 0x40);
}

#[test]
fn wfi_fence_and_unknown_system() {
    let fence = 0x0ff0_000f;
    let fence_i = 0x0000_100f;
    let (mut cpu, mut bus) = start(&[fence, fence_i, WFI, 0x0070_0073, NOP]);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.step(&mut bus), StepResult::Wfi);
    assert_eq!(cpu.pc(), 12);
    // rvlite: any other SYSTEM instruction with funct3 0 is a no-op.
    assert_eq!(cpu.step(&mut bus), StepResult::Done);
    assert_eq!(cpu.pc(), 16);
    assert_eq!(cpu.mcause(), 0);
    assert_eq!(cpu.instructions(), 4);

    // run reports the WFI.
    let (mut cpu, mut bus) = start(&[NOP, WFI, NOP]);
    assert_eq!(cpu.run(&mut bus, 10), StepResult::Wfi);
    assert_eq!(cpu.pc(), 8);
}

#[test]
fn x0_stays_zero() {
    let (mut cpu, mut bus) = start(&[
        lui(0, 0x12345),
        auipc(0, 1),
        addi(0, 0, 5),
        op(0, 0, 0, 1, 1),
        load(2, 0, 1, 0),
        csr(1, 0, CSR_MSCRATCH, 1),
        op(1, 0, 0, 1, 1),
        jalr(0, 0, 0x24),
        NOP,
        jal(0, 8),
        NOP,
        addi(3, 0, 1),
    ]);
    bus.poke32(0x1000, 0xffff_ffff);
    cpu.set_x(1, 0x1000);
    cpu.csr_write(CSR_MSCRATCH, 0x99);
    for _ in 0..10 {
        assert_eq!(cpu.step(&mut bus), StepResult::Done);
        assert_eq!(cpu.x(0), 0);
        assert_eq!(cpu.xs()[0], 0);
    }
    assert_eq!(cpu.x(3), 1);
    cpu.set_x(0, 7);
    cpu.set_reg("zero", 7);
    cpu.set_reg("x0", 7);
    assert_eq!(cpu.x(0), 0);
}

#[test]
fn reset_clears_the_csrs() {
    let (mut cpu, mut bus) = start(&[NOP, 0]);
    cpu.csr_write(CSR_MTVEC, 0x40);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE);
    cpu.csr_write(CSR_MIE, MIE_MEIE);
    cpu.csr_write(CSR_MSCRATCH, 5);
    cpu.set_x(7, 9);
    cpu.run(&mut bus, 2);
    cpu.set_irq(true);
    cpu.reset(0x8000_0002);
    assert_eq!(cpu.pc(), 0x8000_0000);
    assert_eq!(
        [
            cpu.mstatus(),
            cpu.mie(),
            cpu.mtvec(),
            cpu.mepc(),
            cpu.mcause(),
            cpu.mtval(),
            cpu.mscratch()
        ],
        [0; 7]
    );
    assert_eq!(cpu.instructions(), 0);
    assert_eq!(cpu.x(7), 9);
    assert!(cpu.irq());
    assert!(!cpu.irq_pending());
}

#[test]
fn cpu_trait_tick_regs_and_set_reg() {
    let (mut cpu, mut bus) = start(&[addi(10, 0, 42), s_type(1, 10, 2, 2), ECALL]);
    cpu.set_reg("sp", 0x2000);
    cpu.set_reg("MTVEC", 0x103);
    assert_eq!(
        cpu.tick(&mut bus),
        CycleResult::InstrDone { next_pc: 4 },
        "one tick is one instruction"
    );
    assert_eq!(cpu.tick(&mut bus), CycleResult::InstrDone { next_pc: 8 });
    // The adapter writes little-endian bytes at the exact address.
    assert_eq!(bus.peek32(0x2001), 42);
    assert_eq!(
        cpu.tick(&mut bus),
        CycleResult::InstrDone { next_pc: 0x100 }
    );

    let regs = cpu.regs();
    assert_eq!(regs.0.len(), 40);
    assert_eq!(regs.0[0].name, "PC");
    assert_eq!(regs.get("pc"), Some(0x100));
    assert_eq!(regs.get("a0"), Some(42));
    assert_eq!(regs.get("sp"), Some(0x2000));
    assert_eq!(regs.get("mepc"), Some(8));
    assert_eq!(regs.get("mcause"), Some(11));
    assert_eq!(regs.get("mtvec"), Some(0x100));
    assert!(regs.0.iter().all(|r| r.bits == 32));

    cpu.set_reg("x31", 1);
    cpu.set_reg("T6", 2);
    assert_eq!(cpu.x(31), 2);
    cpu.set_reg("x17", 3);
    assert_eq!(cpu.regs().get("a7"), Some(3));
    cpu.set_reg("fp", 4);
    assert_eq!(cpu.x(8), 4);
    cpu.set_reg("x32", 5);
    cpu.set_reg("x", 5);
    cpu.set_reg("nothing", 5);
    cpu.set_reg("PC", 0x1003);
    assert_eq!(cpu.pc(), 0x1000);
    cpu.set_reg("mstatus", 0xffff_ffff);
    cpu.set_reg("mie", 0xffff_ffff);
    cpu.set_reg("mcause", 2);
    cpu.set_reg("mtval", 3);
    cpu.set_reg("mscratch", 4);
    cpu.set_reg("mepc", 0x44);
    let regs = cpu.regs();
    assert_eq!(regs.get("mstatus"), Some(MSTATUS_MIE | MSTATUS_MPIE));
    assert_eq!(regs.get("mie"), Some(MIE_MEIE));
    assert_eq!(regs.get("mcause"), Some(2));
    assert_eq!(regs.get("mtval"), Some(3));
    assert_eq!(regs.get("mscratch"), Some(4));
    assert_eq!(regs.get("mepc"), Some(0x44));
    assert_eq!(regs.get("mip"), Some(0));
    // Enabling through the debugger arms the interrupt like the guest's
    // own write.
    cpu.set_irq(true);
    assert!(cpu.irq_pending());
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut bus) = start(&[addi(5, 0, -3), ECALL, NOP]);
    cpu.csr_write(CSR_MTVEC, 8);
    cpu.csr_write(CSR_MSCRATCH, 0x1234);
    cpu.csr_write(CSR_MIE, MIE_MEIE);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE);
    cpu.run(&mut bus, 3);
    cpu.csr_write(CSR_MSTATUS, MSTATUS_MIE | MSTATUS_MPIE);
    cpu.set_irq(true);
    assert!(cpu.irq_pending());
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();

    let mut other = Rv32::new(Model::Rvlite);
    other.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(other, cpu);
    // Both go on alike.
    assert_eq!(other.step(&mut bus), StepResult::Interrupt);
    assert_eq!(cpu.step(&mut bus), StepResult::Interrupt);
    assert_eq!(other, cpu);
}

#[test]
fn corrupt_savestates_never_panic() {
    let (mut cpu, mut bus) = start(&[addi(5, 0, -3), NOP]);
    cpu.run(&mut bus, 2);
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();

    // Truncated anywhere: an error, and the core is untouched.
    for len in 0..bytes.len() {
        let mut other = Rv32::new(Model::Rvlite);
        other.set_x(9, 77);
        let before = other.clone();
        assert!(other.load(&mut StateReader::new(&bytes[..len])).is_err());
        assert_eq!(other, before);
    }
    // Any single byte changed: an error or a core that still runs.
    for at in 0..bytes.len() {
        for flip in [0x01, 0x80, 0xff] {
            let mut bad = bytes.clone();
            bad[at] ^= flip;
            let mut other = Rv32::new(Model::Rvlite);
            if other.load(&mut StateReader::new(&bad)).is_ok() {
                assert_eq!(other.x(0), 0);
                assert_eq!(other.pc() & 3, 0);
                assert_eq!(other.mtvec() & 3, 0);
                other.run(&mut bus, 4);
                assert_eq!(other.pc() & 3, 0);
            }
        }
    }
    let mut other = Rv32::new(Model::Rvlite);
    assert!(other.load(&mut StateReader::new(&[])).is_err());
}

#[test]
fn disassembler() {
    let code = [
        addi(SP, SP, -120),            // 0
        lui(A0, 0x12345),              // 4
        auipc(T0, 0xe60),              // 8
        jal(RA, 0x100),                // 12
        jal(ZERO, -16),                // 16
        jalr(ZERO, RA, 0),             // 20
        jalr(RA, A0, 8),               // 24
        jalr(ZERO, A0, 0),             // 28
        b_type(-8, A1, A0, 1),         // 32
        b_type(16, ZERO, A0, 6),       // 36
        load(2, A0, S0, -4),           // 40
        load(4, A1, SP, 3),            // 44
        s_type(4, A0, SP, 2),          // 48
        s_type(-1, A1, S0, 0),         // 52
        NOP,                           // 56
        addi(A0, ZERO, -1),            // 60
        addi(A0, A1, 0),               // 64
        opi(1, A0, A1, 3),             // 68
        opi(5, A0, A1, 0x400 | 31),    // 72
        opi(7, A0, A1, 0xff),          // 76
        op(0x20, 0, A0, A1, A2),       // 80
        op(1, 0, A0, A1, A2),          // 84
        op(1, 2, A0, A1, A2),          // 88
        op(1, 7, A0, A1, A2),          // 92
        csr(7, ZERO, CSR_MSTATUS, 8),  // 96
        csr(1, A0, CSR_MTVEC, T0),     // 100
        csr(2, A0, 0x7c0, ZERO),       // 104
        ECALL,                         // 108
        EBREAK,                        // 112
        MRET,                          // 116
        WFI,                           // 120
        0x0ff0_000f,                   // 124
        0x0000_100f,                   // 128
        0xffff_ffff,                   // 132
        0x0000_4501,                   // 136
        op(0, 3, A0, A1, A2),          // 140
        opi(3, A0, A1, -1),            // 144
        op(0x20, 5, A0, A1, A2),       // 148
        load(5, A0, A1, 2),            // 152
        opi(1, A0, A1, 0x400 | 3),     // 156
        csr(6, A1, CSR_MINSTRETH, 31), // 160
    ];
    let bus = Ram::with(&code);
    let cpu = Rv32::default();
    let show = |a: u32| cpu.disassemble(&bus, a).to_string();
    assert_eq!(show(0), "addi sp,sp,-120");
    assert_eq!(show(4), "lui a0,0x12345");
    assert_eq!(show(8), "auipc t0,0xe60");
    assert_eq!(show(12), "jal ra,0x0000010c");
    assert_eq!(show(16), "j 0x00000000");
    assert_eq!(show(20), "ret");
    assert_eq!(show(24), "jalr ra,8(a0)");
    assert_eq!(show(28), "jr a0");
    assert_eq!(show(32), "bne a0,a1,0x00000018");
    assert_eq!(show(36), "bltu a0,zero,0x00000034");
    assert_eq!(show(40), "lw a0,-4(s0)");
    assert_eq!(show(44), "lbu a1,3(sp)");
    assert_eq!(show(48), "sw a0,4(sp)");
    assert_eq!(show(52), "sb a1,-1(s0)");
    assert_eq!(show(56), "nop");
    assert_eq!(show(60), "li a0,-1");
    assert_eq!(show(64), "mv a0,a1");
    assert_eq!(show(68), "slli a0,a1,3");
    assert_eq!(show(72), "srai a0,a1,31");
    assert_eq!(show(76), "andi a0,a1,255");
    assert_eq!(show(80), "sub a0,a1,a2");
    assert_eq!(show(84), "mul a0,a1,a2");
    assert_eq!(show(88), "mulhsu a0,a1,a2");
    assert_eq!(show(92), "remu a0,a1,a2");
    assert_eq!(show(96), "csrrci zero,mstatus,8");
    assert_eq!(show(100), "csrrw a0,mtvec,t0");
    assert_eq!(show(104), "csrrs a0,0x7c0,zero");
    assert_eq!(show(108), "ecall");
    assert_eq!(show(112), "ebreak");
    assert_eq!(show(116), "mret");
    assert_eq!(show(120), "wfi");
    assert_eq!(show(124), "fence");
    assert_eq!(show(128), "fence.i");
    assert_eq!(show(132), ".word 0xffffffff");
    assert_eq!(show(136), ".word 0x00004501");
    assert_eq!(show(140), "sltu a0,a1,a2");
    assert_eq!(show(144), "sltiu a0,a1,-1");
    assert_eq!(show(148), "sra a0,a1,a2");
    assert_eq!(show(152), "lhu a0,2(a1)");
    assert_eq!(show(156), ".word 0x40359513");
    assert_eq!(show(160), "csrrsi a1,minstreth,31");

    let at = |a: u32| cpu.disassemble(&bus, a);
    assert_eq!(at(0).bytes, vec![0x13, 0x01, 0x81, 0xf8]);
    assert_eq!(at(0).addr, 0);
    assert_eq!((at(0).flow, at(0).target), (Flow::Normal, None));
    assert_eq!((at(12).flow, at(12).target), (Flow::Call, Some(0x10c)));
    assert_eq!((at(16).flow, at(16).target), (Flow::Jump, Some(0)));
    assert_eq!((at(20).flow, at(20).target), (Flow::Return, None));
    assert_eq!(at(24).flow, Flow::Call);
    assert_eq!(at(28).flow, Flow::Jump);
    assert_eq!((at(32).flow, at(32).target), (Flow::Branch, Some(0x18)));
    assert_eq!(at(36).target, Some(0x34));
    assert_eq!(at(108).flow, Flow::Call);
    assert_eq!(at(116).flow, Flow::Return);
    assert_eq!(at(132).flow, Flow::Normal);
}
