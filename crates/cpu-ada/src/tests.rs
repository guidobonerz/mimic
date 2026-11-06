//! Every instruction of SPEC.md section 3 against its specified result,
//! flags and cycles; exceptions, interrupts, the decode/encode round
//! trip and the savestate.

use super::*;
use isa::{decode, encode, Alu, Imm, Md, Mem, Op, Xop};
use std::collections::BTreeMap;

/// Sparse little-endian memory over the whole linear space.
#[derive(Default)]
struct Ram(BTreeMap<u32, u8>);

impl Ram {
    fn byte(&self, a: u32) -> u8 {
        self.0.get(&a).copied().unwrap_or(0)
    }
    fn put(&mut self, a: u32, v: u8) {
        self.0.insert(a, v);
    }
    fn word(&self, a: u32) -> u16 {
        u16::from_le_bytes([self.byte(a), self.byte(a + 1)])
    }
    fn put_word(&mut self, a: u32, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.put(a, lo);
        self.put(a + 1, hi);
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.peek(addr, size)
    }
    fn peek(&self, addr: u32, size: Size) -> u32 {
        match size {
            Size::Byte => u32::from(self.byte(addr)),
            _ => u32::from(self.word(addr)),
        }
    }
    fn write(&mut self, addr: u32, size: Size, v: u32) {
        match size {
            Size::Byte => self.put(addr, v as u8),
            _ => self.put_word(addr, v as u16),
        }
    }
    fn addr_bits(&self) -> u8 {
        32
    }
}

const CODE_SEG: u16 = 0x0040;

/// A CPU in supervisor mode at 0040:0000 with `ops` there and the vector
/// table pointing vector v at FFFB:1000 + 16 v.
fn setup(ops: &[Op]) -> (Ada, Ram) {
    let mut ram = Ram::default();
    for (i, op) in ops.iter().enumerate() {
        ram.put_word(Ada::linear(CODE_SEG, 2 * i as u16), encode(op));
    }
    for v in 0..32u32 {
        ram.put_word(VECTORS + 4 * v, ROM_SEGMENT);
        ram.put_word(VECTORS + 4 * v + 2, 0x1000 + 16 * v as u16);
    }
    let mut cpu = Ada::new();
    cpu.s[0] = CODE_SEG;
    cpu.pc = 0;
    (cpu, ram)
}

/// Run `n` instructions, returning their cycles.
fn run(cpu: &mut Ada, ram: &mut Ram, n: usize) -> u32 {
    (0..n).map(|_| u32::from(cpu.step(ram))).sum()
}

fn alu(f: Alu, rd: u8, rs: u8) -> Op {
    Op::Alu { f, rd, rs }
}

fn imm(f: Imm, rd: u8, imm: u8) -> Op {
    Op::Imm { f, rd, imm }
}

/// `f` on (a, b) with carry `c`: result and the flags N Z C V.
fn alu_case(f: Alu, a: u16, b: u16, c: bool) -> (u16, u16, u32) {
    let (mut cpu, mut ram) = setup(&[alu(f, 1, 2)]);
    cpu.r[1] = a;
    cpu.r[2] = b;
    cpu.sr = (cpu.sr & !(C | V | Z | N)) | if c { C } else { 0 };
    let cycles = run(&mut cpu, &mut ram, 1);
    (cpu.r[1], cpu.sr & (C | V | Z | N), cycles)
}

#[test]
fn alu_functions() {
    let cases: &[(Alu, u16, u16, bool, u16, u16)] = &[
        (Alu::Add, 0x7fff, 1, false, 0x8000, N | V),
        (Alu::Add, 0xffff, 1, false, 0, Z | C),
        (Alu::Sub, 5, 7, false, 0xfffe, N),
        (Alu::Sub, 7, 5, false, 2, C),
        (Alu::Sub, 0x8000, 1, false, 0x7fff, C | V),
        (Alu::Adc, 1, 1, true, 3, 0),
        (Alu::Adc, 0xffff, 0, true, 0, Z | C),
        (Alu::Sbc, 5, 2, false, 2, C),
        (Alu::Sbc, 5, 5, false, 0xffff, N),
        (Alu::Sbc, 5, 5, true, 0, Z | C),
        (Alu::And, 0xf0f0, 0x8f00, true, 0x8000, N | C),
        (Alu::Or, 0x0f00, 0x00f0, false, 0x0ff0, 0),
        (Alu::Xor, 0x1234, 0x1234, false, 0, Z),
        (Alu::Not, 0x5555, 0x00ff, false, 0xff00, N),
        (Alu::Shl, 0x8001, 1, false, 0x0002, C),
        (Alu::Shl, 0x0001, 15, false, 0x8000, N),
        (Alu::Shl, 0x1234, 16, true, 0x1234, C), // count 0: C kept
        (Alu::Shr, 0x0003, 1, false, 0x0001, C),
        (Alu::Shr, 0x8000, 15, false, 0x0001, 0),
        (Alu::Asr, 0x8000, 4, false, 0xf800, N),
        (Alu::Asr, 0x8001, 1, false, 0xc000, N | C),
        (Alu::Rol, 0x8001, 1, false, 0x0003, C),
        (Alu::Rol, 0x1234, 4, false, 0x2341, C),
        (Alu::Mov, 0x1111, 0x8000, false, 0x8000, N),
        (Alu::Mul, 300, 300, false, 0x5f90, 0), // 90000 & $FFFF
        (Alu::Mul, 0xffff, 2, false, 0xfffe, N),
    ];
    for &(f, a, b, c, want, flags) in cases {
        let (r, got, cycles) = alu_case(f, a, b, c);
        assert_eq!((r, got), (want, flags), "{f:?} {a:#06x}, {b:#06x}");
        assert_eq!(cycles, if f == Alu::Mul { 2 } else { 1 }, "{f:?} cycles");
    }
}

#[test]
fn cmp_and_tst_change_only_flags() {
    let (r, flags, _) = alu_case(Alu::Cmp, 3, 5, false);
    assert_eq!((r, flags), (3, N));
    let (r, flags, _) = alu_case(Alu::Cmp, 5, 5, false);
    assert_eq!((r, flags), (5, Z | C));
    let (r, flags, _) = alu_case(Alu::Tst, 0xf0, 0x0f, false);
    assert_eq!((r, flags), (0xf0, Z));
}

#[test]
fn r0_stays_zero_but_sets_flags() {
    let (mut cpu, mut ram) = setup(&[imm(Imm::Movi, 0, 5), alu(Alu::Add, 0, 0)]);
    run(&mut cpu, &mut ram, 2);
    assert_eq!(cpu.r[0], 0);
    assert_ne!(cpu.sr & Z, 0, "ADD R0, R0 sets Z");
}

#[test]
fn immediates() {
    let (mut cpu, mut ram) = setup(&[
        imm(Imm::Movi, 1, 0xfe),
        imm(Imm::Lui, 1, 0x12),
        imm(Imm::Addi, 1, 0x80), // −128
        imm(Imm::Movi, 2, 200),
        imm(Imm::Cmpi, 2, 200),
    ]);
    assert_eq!(run(&mut cpu, &mut ram, 1), 1);
    assert_eq!(cpu.r[1], 0x00fe, "MOVI: high byte 0");
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[1], 0x12fe, "LUI keeps the low byte");
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[1], 0x127e, "ADDI sign-extends");
    assert_eq!(cpu.sr & C, C);
    run(&mut cpu, &mut ram, 2);
    assert_eq!(cpu.sr & (Z | C), Z | C, "CMPI zero-extends: 200 = 200");
}

#[test]
fn loads_and_stores_pick_their_segment() {
    let (mut cpu, mut ram) = setup(&[
        Op::Mem {
            f: Mem::Sw,
            rd: 1,
            rb: 2,
            off: 30,
        },
        Op::Mem {
            f: Mem::Lw,
            rd: 3,
            rb: 2,
            off: 30,
        },
        Op::Mem {
            f: Mem::Sw,
            rd: 1,
            rb: 12,
            off: 0,
        },
        Op::Mem {
            f: Mem::Sb,
            rd: 1,
            rb: 2,
            off: 15,
        },
        Op::Mem {
            f: Mem::Lb,
            rd: 4,
            rb: 2,
            off: 15,
        },
        Op::Mem {
            f: Mem::Lw,
            rd: 5,
            rb: 6,
            off: 0,
        },
    ]);
    cpu.s[1] = 0x0002;
    cpu.s[2] = 0x0003;
    cpu.r[1] = 0xbeef;
    cpu.r[2] = 0x0100;
    cpu.r[12] = 0x0200;
    cpu.r[6] = 0x0301; // odd: bit 0 ignored
    ram.put_word(Ada::linear(2, 0x0300), 0x4242);
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!(ram.word(Ada::linear(2, 0x011e)), 0xbeef);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[3], 0xbeef);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(ram.word(Ada::linear(3, 0x0200)), 0xbeef, "R12 base: S2");
    run(&mut cpu, &mut ram, 2);
    assert_eq!(ram.byte(Ada::linear(2, 0x010f)), 0xef);
    assert_eq!(cpu.r[4], 0x00ef, "LB zero-extends");
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[5], 0x4242);
    assert_eq!(cpu.sr & (Z | N | C | V), 0, "loads leave the flags");
}

#[test]
fn x_instructions_use_s3_and_post_increment() {
    let x = |f, post, rd, rs| Op::X { f, post, rd, rs };
    let (mut cpu, mut ram) = setup(&[
        x(Xop::Swx, true, 1, 2),
        x(Xop::Sbx, true, 1, 2),
        x(Xop::Lwx, false, 3, 4),
        x(Xop::Lbx, true, 5, 4),
        x(Xop::Lwx, true, 6, 6),
    ]);
    cpu.s[3] = 0x0010;
    cpu.r[1] = 0xa55a;
    cpu.r[2] = 0xfffe;
    cpu.r[4] = 0x2000;
    cpu.r[6] = 0x3000;
    ram.put_word(Ada::linear(0x10, 0x2000), 0x7788);
    ram.put_word(Ada::linear(0x11, 0x3000), 0x1234);
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!(ram.word(Ada::linear(0x10, 0xfffe)), 0xa55a);
    assert_eq!((cpu.r[2], cpu.s[3]), (0, 0x11), "wrap carries into S3");
    run(&mut cpu, &mut ram, 1);
    assert_eq!(ram.byte(Ada::linear(0x11, 0)), 0x5a);
    assert_eq!(cpu.r[2], 1);
    cpu.s[3] = 0x10;
    run(&mut cpu, &mut ram, 2);
    assert_eq!(cpu.r[3], 0x7788);
    assert_eq!((cpu.r[5], cpu.r[4]), (0x88, 0x2001));
    cpu.s[3] = 0x11;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[6], 0x1234, "Rd = Rs: the loaded value wins");
}

/// A branch with `cond` taken (or not) on flags `sr`.
fn branch(cond: u8, sr: u16) -> (bool, u32) {
    let (mut cpu, mut ram) = setup(&[Op::Branch { cond, off: 3 }]);
    cpu.sr = (cpu.sr & !(C | V | Z | N)) | sr;
    let cycles = run(&mut cpu, &mut ram, 1);
    (cpu.pc == 8, cycles)
}

#[test]
fn branch_conditions() {
    let cases: &[(u8, u16, bool)] = &[
        (0, Z, true),
        (0, 0, false),
        (1, 0, true),
        (2, C, true),
        (3, C, false),
        (4, N, true),
        (5, N, false),
        (6, V, true),
        (7, V, false),
        (8, C, true),
        (8, C | Z, false),
        (9, Z | C, true),
        (9, C, false),
        (10, N | V, true),
        (10, N, false),
        (11, N, true),
        (11, 0, false),
        (12, 0, true),
        (12, Z, false),
        (13, Z, true),
        (13, V, true),
        (13, 0, false),
        (14, 0, true),
        (15, Z | C | N | V, false),
    ];
    for &(cond, sr, taken) in cases {
        let (t, cycles) = branch(cond, sr);
        assert_eq!(t, taken, "{} with {sr:#x}", isa::COND[usize::from(cond)]);
        assert_eq!(cycles, if taken { 2 } else { 1 });
    }
}

#[test]
fn branch_and_link_offsets() {
    let (mut cpu, mut ram) = setup(&[]);
    cpu.pc = 0x0100;
    ram.put_word(
        Ada::linear(CODE_SEG, 0x100),
        encode(&Op::Branch {
            cond: 14,
            off: -128,
        }),
    );
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x0102 - 256);
    cpu.pc = 0x0100;
    ram.put_word(Ada::linear(CODE_SEG, 0x100), encode(&Op::Bl { off: -2048 }));
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!((cpu.pc, cpu.r[11]), (0x0102u16.wrapping_sub(4096), 0x0102));
    cpu.pc = 0x0100;
    ram.put_word(Ada::linear(CODE_SEG, 0x100), encode(&Op::Bl { off: 2047 }));
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x0102 + 4094);
}

#[test]
fn jumps_and_segments() {
    let (mut cpu, mut ram) = setup(&[Op::Jalr { rd: 5, rs: 5 }]);
    cpu.r[5] = 0x0040;
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!(
        (cpu.pc, cpu.r[5]),
        (0x0040, 2),
        "target read before the link"
    );
    ram.put_word(Ada::linear(CODE_SEG, 0x40), encode(&Op::Jr { rs: 6 }));
    cpu.r[6] = 0x0080;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x80);
    for (i, op) in [
        Op::Mts { sn: 3, rs: 1 },
        Op::Mfs { rd: 2, sn: 3 },
        Op::Callf { rd: 7, rs: 8 },
    ]
    .iter()
    .enumerate()
    {
        ram.put_word(Ada::linear(CODE_SEG, 0x80 + 2 * i as u16), encode(op));
    }
    cpu.r[1] = 0x1234;
    cpu.r[7] = 0x0050;
    cpu.r[8] = 0x0010;
    assert_eq!(run(&mut cpu, &mut ram, 2), 2, "MTS/MFS one cycle each");
    assert_eq!((cpu.s[3], cpu.r[2]), (0x1234, 0x1234));
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.s[0], cpu.pc), (0x0050, 0x0010));
    assert_eq!((cpu.r[10], cpu.r[11]), (CODE_SEG, 0x86));
    ram.put_word(Ada::linear(0x50, 0x10), encode(&Op::Jf { rd: 10, rs: 11 }));
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.s[0], cpu.pc), (CODE_SEG, 0x86), "RETF");
}

#[test]
fn multiply_high_divide_and_misc() {
    let md = |f, a: u16, b: u16| {
        let (mut cpu, mut ram) = setup(&[Op::Md { f, rd: 1, rs: 2 }]);
        cpu.r[1] = a;
        cpu.r[2] = b;
        let cycles = run(&mut cpu, &mut ram, 1);
        (cpu.r[1], cpu.sr & (N | Z | V | C), cycles)
    };
    assert_eq!(md(Md::Mulhu, 0xffff, 0xffff), (0xfffe, N, 2));
    assert_eq!(md(Md::Mulhs, 0xffff, 0xffff), (0, Z, 2), "−1 × −1 = 1");
    assert_eq!(md(Md::Mulhs, 0x8000, 2), (0xffff, N, 2));
    assert_eq!(md(Md::Divu, 100, 7), (14, 0, 18));
    assert_eq!(md(Md::Divs, (-100i16) as u16, 7), ((-14i16) as u16, N, 18));
    assert_eq!(md(Md::Divs, 0x8000, 0xffff), (0x8000, N | V, 18));
    assert_eq!(md(Md::Remu, 100, 7), (2, 0, 18));
    assert_eq!(md(Md::Rems, (-100i16) as u16, 7), ((-2i16) as u16, N, 18));
    assert_eq!(md(Md::Neg, 0, 1), (0xffff, N, 2));
    assert_eq!(md(Md::Neg, 0, 0), (0, Z | C, 2));
    assert_eq!(md(Md::Sxb, 0, 0x1280), (0xff80, N, 2));
    assert_eq!(md(Md::Zxb, 0, 0x1280), (0x0080, 0, 2));
    assert_eq!(md(Md::Swab, 0, 0x1234), (0x3412, 0, 2));
}

#[test]
fn divide_by_zero_raises_exception_3() {
    let (mut cpu, mut ram) = setup(&[Op::Md {
        f: Md::Divu,
        rd: 1,
        rs: 2,
    }]);
    cpu.r[1] = 9;
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!(cpu.r[1], 9);
    assert_eq!((cpu.s[0], cpu.pc), (ROM_SEGMENT, 0x1000 + 16 * 3));
    assert_eq!(
        (cpu.spc, cpu.ss0, cpu.cause),
        (0, CODE_SEG, 3),
        "returns to the DIV"
    );
}

#[test]
fn trap_and_rti() {
    let (mut cpu, mut ram) = setup(&[Op::Trap { n: 0x42 }]);
    cpu.sr = C; // user mode, IPL 0
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!((cpu.s[0], cpu.pc), (ROM_SEGMENT, 0x1000 + 16 * 4));
    assert_eq!(cpu.cause, 0x4204);
    assert_eq!((cpu.spc, cpu.ss0, cpu.ssr), (2, CODE_SEG, C));
    assert_ne!(cpu.sr & S, 0, "supervisor");
    ram.put_word(Ada::linear(ROM_SEGMENT, 0x1040), encode(&Op::Rti));
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!((cpu.s[0], cpu.pc, cpu.sr), (CODE_SEG, 2, C));
}

#[test]
fn system_instructions() {
    let (mut cpu, mut ram) = setup(&[
        Op::Ei,
        Op::Mfsr { rd: 1 },
        Op::Di,
        Op::Mtsr { rs: 2 },
        Op::Mtsc { c: SC_SPC, rs: 3 },
        Op::Mfsc { rd: 4, c: SC_SPC },
        Op::Mfsc { rd: 5, c: SC_CYCLE },
        Op::Mfsc {
            rd: 6,
            c: SC_CYCLEH,
        },
        Op::Nop,
    ]);
    cpu.r[2] = 0xffff;
    cpu.r[3] = 0x4321;
    run(&mut cpu, &mut ram, 2);
    assert_eq!(cpu.r[1], S, "EI: IPL 0");
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.sr & IPL, IPL);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.sr, SR_MASK, "MTSR keeps only existing bits");
    run(&mut cpu, &mut ram, 2);
    assert_eq!(cpu.r[4], 0x4321);
    cpu.cycles = 0x0001_2345;
    run(&mut cpu, &mut ram, 2);
    assert_eq!((cpu.r[5], cpu.r[6]), (0x2345, 1), "CYCLE latches CYCLEH");
    let sr = cpu.sr;
    assert_eq!(run(&mut cpu, &mut ram, 1), 1);
    assert_eq!(cpu.sr, sr, "NOP");
}

#[test]
fn privileged_instructions_trap_in_user_mode() {
    for op in [
        Op::Rti,
        Op::Ei,
        Op::Di,
        Op::Halt,
        Op::Mtsr { rs: 1 },
        Op::Mfsc { rd: 1, c: 0 },
        Op::Mtsc { c: 0, rs: 1 },
    ] {
        let (mut cpu, mut ram) = setup(&[op]);
        cpu.sr = 0;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.pc, 0x1000 + 16 * 2, "{op}");
        assert_eq!((cpu.spc, cpu.cause), (0, 2), "{op}");
    }
    let (mut cpu, mut ram) = setup(&[Op::Mfsr { rd: 1 }]);
    cpu.sr = C;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.r[1], C, "MFSR is allowed in user mode");
}

#[test]
fn illegal_words_raise_exception_1() {
    for w in [
        0xf000u16, 0x0d00, 0x9008, 0xc006, 0xc200, 0xc100, 0xd00a, 0xe00b, 0xe101, 0x0e00,
    ] {
        assert_eq!(decode(w), Op::Illegal(w), "{w:#06x}");
        let (mut cpu, mut ram) = setup(&[Op::Illegal(w)]);
        run(&mut cpu, &mut ram, 1);
        assert_eq!(
            (cpu.pc, cpu.spc, cpu.cause),
            (0x1000 + 16, 0, 1),
            "{w:#06x}"
        );
    }
}

#[test]
fn brk_stops_for_a_debugger_or_raises_exception_5() {
    let (mut cpu, mut ram) = setup(&[Op::Brk]);
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.pc, cpu.spc), (0x1000 + 16 * 5, 2));
    assert_eq!(cpu.sr & (S | IPL), S | IPL, "supervisor, IPL 7");
    let (mut cpu, mut ram) = setup(&[Op::Brk]);
    cpu.brk_stops = true;
    run(&mut cpu, &mut ram, 1);
    assert!(cpu.brk_hit);
    assert_eq!(cpu.pc, 2);
}

#[test]
fn interrupts_by_level_and_nmi_edge() {
    let (mut cpu, mut ram) = setup(&[Op::Nop, Op::Nop, Op::Nop]);
    cpu.sr = S | 3 << IPL_SHIFT;
    cpu.set_irq_level(3);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 2, "level 3 blocked by IPL 3");
    cpu.set_irq_level(4);
    assert_eq!(run(&mut cpu, &mut ram, 1), 2);
    assert_eq!((cpu.pc, cpu.spc, cpu.cause), (0x1000 + 16 * 12, 2, 12));
    assert_eq!(cpu.sr & IPL, IPL, "entry blocks further interrupts");
    assert_eq!(cpu.ssr & IPL, 3 << IPL_SHIFT, "RTI restores IPL 3");
    // NMI: taken with IPL 7 on its rising edge only.
    let (mut cpu, mut ram) = setup(&[Op::Nop, Op::Nop]);
    cpu.set_irq_level(7);
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 0x1000 + 16 * 15);
    cpu.pc = 0;
    cpu.s[0] = CODE_SEG;
    run(&mut cpu, &mut ram, 1);
    assert_eq!(cpu.pc, 2, "no new edge, no second NMI");
}

#[test]
fn halt_waits_for_an_interrupt() {
    let (mut cpu, mut ram) = setup(&[Op::Ei, Op::Halt, Op::Nop]);
    run(&mut cpu, &mut ram, 2);
    run(&mut cpu, &mut ram, 5);
    assert!(cpu.halted);
    assert_eq!(cpu.pc, 4);
    cpu.set_irq_level(1);
    run(&mut cpu, &mut ram, 1);
    assert!(!cpu.halted);
    assert_eq!(
        (cpu.pc, cpu.spc),
        (0x1000 + 16 * 9, 4),
        "continues after HALT"
    );
}

#[test]
fn tick_spreads_an_instruction_over_its_cycles() {
    let (mut cpu, mut ram) = setup(&[Op::Md {
        f: Md::Divu,
        rd: 1,
        rs: 2,
    }]);
    cpu.r[2] = 1;
    for _ in 0..17 {
        assert_eq!(cpu.tick(&mut ram), CycleResult::Busy);
    }
    assert_eq!(
        cpu.tick(&mut ram),
        CycleResult::InstrDone {
            next_pc: Ada::linear(CODE_SEG, 2)
        }
    );
    assert_eq!(cpu.cycles, 18);
}

#[test]
fn every_legal_word_encodes_back_and_has_text() {
    let mut legal = 0;
    for w in 0..=0xffffu16 {
        let op = decode(w);
        if let Op::Illegal(_) = op {
            continue;
        }
        legal += 1;
        assert_eq!(encode(&op), w, "{op}");
        assert!(!op.text(0).0.is_empty());
    }
    assert_eq!(legal, 39_381, "legal words");
}

#[test]
fn disassembly_and_registers() {
    use emu_core::Cpu as _;
    let (mut cpu, mut ram) = setup(&[Op::Branch { cond: 1, off: -1 }, Op::Jr { rs: 11 }]);
    let i = cpu.disassemble(&ram, Ada::linear(CODE_SEG, 0));
    assert_eq!(i.to_string(), "bne $0000");
    assert_eq!(i.flow, Flow::Branch);
    assert_eq!(i.target, Some(Ada::linear(CODE_SEG, 0)));
    assert_eq!(
        cpu.disassemble(&ram, Ada::linear(CODE_SEG, 2)).flow,
        Flow::Return
    );
    cpu.set_reg("r3", 0x1234);
    cpu.set_reg("r0", 9);
    cpu.set_reg("s2", 7);
    let regs = cpu.regs();
    assert_eq!(
        (regs.get("r3"), regs.get("r0"), regs.get("s2")),
        (Some(0x1234), Some(0), Some(7))
    );
    run(&mut cpu, &mut ram, 1);
}

#[test]
fn savestate_round_trip() {
    let (mut cpu, mut ram) = setup(&[
        Op::Md {
            f: Md::Divu,
            rd: 1,
            rs: 2,
        },
        Op::Nop,
    ]);
    cpu.r[2] = 3;
    cpu.set_irq_level(2);
    for _ in 0..5 {
        cpu.tick(&mut ram);
    }
    let mut w = StateWriter::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut back = Ada::new();
    back.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(back, cpu);
    assert!(Ada::new()
        .load(&mut StateReader::new(&bytes[..bytes.len() - 1]))
        .is_err());
}

#[test]
fn the_vector_base_moves_the_table() {
    // Vectors at 0010:0040: entry 4 (TRAP) -> 0010:0200.
    let (mut cpu, mut ram) = setup(&[Op::Trap { n: 1 }]);
    ram.put_word(Ada::linear(0x0010, 0x0040 + 16), 0x0010);
    ram.put_word(Ada::linear(0x0010, 0x0040 + 18), 0x0200);
    assert_eq!(cpu.vbr, ROM_SEGMENT, "reset: the ROM's table");
    cpu.vbr = 0x0010;
    run(&mut cpu, &mut ram, 1);
    assert_eq!((cpu.s[0], cpu.pc), (0x0010, 0x0200));
}
