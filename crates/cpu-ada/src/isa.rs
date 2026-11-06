//! Ada's instruction set: every 16-bit word decoded into an [`Op`] and
//! back ([`decode`], [`encode`]), and its assembler text ([`Op::text`]).
//! The assembler (`asm-ada`) encodes through this module, so CPU,
//! disassembler and assembler share one table (SPEC.md 3.3–3.10).

use std::fmt;

/// ALU functions of opcode 0 (`Rd = Rd op Rs`), by funct number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    Add,
    Sub,
    Adc,
    Sbc,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    Asr,
    Rol,
    Mov,
    Cmp,
    Tst,
    Mul,
}

pub const ALU: [(Alu, &str); 16] = [
    (Alu::Add, "add"),
    (Alu::Sub, "sub"),
    (Alu::Adc, "adc"),
    (Alu::Sbc, "sbc"),
    (Alu::And, "and"),
    (Alu::Or, "or"),
    (Alu::Xor, "xor"),
    (Alu::Not, "not"),
    (Alu::Shl, "shl"),
    (Alu::Shr, "shr"),
    (Alu::Asr, "asr"),
    (Alu::Rol, "rol"),
    (Alu::Mov, "mov"),
    (Alu::Cmp, "cmp"),
    (Alu::Tst, "tst"),
    (Alu::Mul, "mul"),
];

/// Opcode 13: multiply high, divide, remainder, negate, extend, swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Md {
    Mulhu,
    Mulhs,
    Divu,
    Divs,
    Remu,
    Rems,
    Neg,
    Sxb,
    Zxb,
    Swab,
}

pub const MD: [(Md, &str); 10] = [
    (Md::Mulhu, "mulhu"),
    (Md::Mulhs, "mulhs"),
    (Md::Divu, "divu"),
    (Md::Divs, "divs"),
    (Md::Remu, "remu"),
    (Md::Rems, "rems"),
    (Md::Neg, "neg"),
    (Md::Sxb, "sxb"),
    (Md::Zxb, "zxb"),
    (Md::Swab, "swab"),
];

/// Opcode 9: loads and stores through S3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Xop {
    Lwx,
    Swx,
    Lbx,
    Sbx,
}

pub const XOP: [(Xop, &str); 4] = [
    (Xop::Lwx, "lwx"),
    (Xop::Swx, "swx"),
    (Xop::Lbx, "lbx"),
    (Xop::Sbx, "sbx"),
];

/// Branch conditions (SPEC.md 3.7), by number.
pub const COND: [&str; 16] = [
    "beq", "bne", "bcs", "bcc", "bmi", "bpl", "bvs", "bvc", "bhi", "bls", "bge", "blt", "bgt",
    "ble", "bra", "bnv",
];

/// Alternative names the assembler also takes.
pub const COND_ALIASES: [(&str, u8); 2] = [("bhs", 2), ("blo", 3)];

/// Memory opcodes 5–8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mem {
    Lw,
    Sw,
    Lb,
    Sb,
}

pub const MEM: [(Mem, &str); 4] = [
    (Mem::Lw, "lw"),
    (Mem::Sw, "sw"),
    (Mem::Lb, "lb"),
    (Mem::Sb, "sb"),
];

/// Immediate opcodes 1–4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Imm {
    Addi,
    Movi,
    Lui,
    Cmpi,
}

pub const IMM: [(Imm, &str); 4] = [
    (Imm::Addi, "addi"),
    (Imm::Movi, "movi"),
    (Imm::Lui, "lui"),
    (Imm::Cmpi, "cmpi"),
];

/// One decoded instruction. Register numbers are 0–12, segment numbers
/// 0–3; `off` of a memory op is in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Alu {
        f: Alu,
        rd: u8,
        rs: u8,
    },
    /// `imm` is the 8-bit field; ADDI sign-extends it, the others don't.
    Imm {
        f: Imm,
        rd: u8,
        imm: u8,
    },
    Mem {
        f: Mem,
        rd: u8,
        rb: u8,
        off: u8,
    },
    X {
        f: Xop,
        post: bool,
        rd: u8,
        rs: u8,
    },
    /// Offset in words from the next instruction.
    Branch {
        cond: u8,
        off: i8,
    },
    Bl {
        off: i16,
    },
    Jr {
        rs: u8,
    },
    Jalr {
        rd: u8,
        rs: u8,
    },
    Mts {
        sn: u8,
        rs: u8,
    },
    Mfs {
        rd: u8,
        sn: u8,
    },
    Jf {
        rd: u8,
        rs: u8,
    },
    Callf {
        rd: u8,
        rs: u8,
    },
    Md {
        f: Md,
        rd: u8,
        rs: u8,
    },
    Trap {
        n: u8,
    },
    Rti,
    Ei,
    Di,
    Halt,
    Mtsr {
        rs: u8,
    },
    Mfsr {
        rd: u8,
    },
    Mfsc {
        rd: u8,
        c: u8,
    },
    Mtsc {
        c: u8,
        rs: u8,
    },
    Nop,
    Brk,
    Illegal(u16),
}

/// Number of general registers (R0–R12).
pub const REGS: u8 = 13;

fn reg_ok(r: u8) -> bool {
    r < REGS
}

/// Decode a word (SPEC.md 3.3–3.10). Fields an instruction does not use
/// must be 0, register fields must name R0–R12, else [`Op::Illegal`].
#[inline]
pub fn decode(w: u16) -> Op {
    let op = (w >> 12) as u8;
    let a = ((w >> 8) & 15) as u8;
    let b = ((w >> 4) & 15) as u8;
    let f = (w & 15) as u8;
    let imm8 = (w & 0xff) as u8;
    let ill = Op::Illegal(w);
    match op {
        0 if reg_ok(a) && reg_ok(b) => Op::Alu {
            f: ALU[usize::from(f)].0,
            rd: a,
            rs: b,
        },
        1..=4 if reg_ok(a) => Op::Imm {
            f: IMM[usize::from(op - 1)].0,
            rd: a,
            imm: imm8,
        },
        5..=8 if reg_ok(a) && reg_ok(b) => {
            let m = MEM[usize::from(op - 5)].0;
            let off = if matches!(m, Mem::Lw | Mem::Sw) {
                f * 2
            } else {
                f
            };
            Op::Mem {
                f: m,
                rd: a,
                rb: b,
                off,
            }
        }
        9 if f < 8 && reg_ok(a) && reg_ok(b) => Op::X {
            f: XOP[usize::from(f & 3)].0,
            post: f >= 4,
            rd: a,
            rs: b,
        },
        0xa => Op::Branch {
            cond: a,
            off: imm8 as i8,
        },
        0xb => {
            let off12 = (w & 0x0fff) as i16;
            Op::Bl {
                off: (off12 << 4) >> 4,
            }
        }
        0xc => match f {
            0 if a == 0 && reg_ok(b) => Op::Jr { rs: b },
            1 if reg_ok(a) && reg_ok(b) => Op::Jalr { rd: a, rs: b },
            2 if (1..4).contains(&a) && reg_ok(b) => Op::Mts { sn: a, rs: b },
            3 if reg_ok(a) && b < 4 => Op::Mfs { rd: a, sn: b },
            4 if reg_ok(a) && reg_ok(b) => Op::Jf { rd: a, rs: b },
            5 if reg_ok(a) && reg_ok(b) => Op::Callf { rd: a, rs: b },
            _ => ill,
        },
        0xd if usize::from(f) < MD.len() && reg_ok(a) && reg_ok(b) => Op::Md {
            f: MD[usize::from(f)].0,
            rd: a,
            rs: b,
        },
        0xe => match f {
            0 => Op::Trap { n: trap_number(w) },
            1 if a == 0 && b == 0 => Op::Rti,
            2 if a == 0 && b == 0 => Op::Ei,
            3 if a == 0 && b == 0 => Op::Di,
            4 if a == 0 && b == 0 => Op::Halt,
            5 if a == 0 && reg_ok(b) => Op::Mtsr { rs: b },
            6 if reg_ok(a) && b == 0 => Op::Mfsr { rd: a },
            7 if reg_ok(a) => Op::Mfsc { rd: a, c: b },
            8 if reg_ok(b) => Op::Mtsc { c: a, rs: b },
            9 if a == 0 && b == 0 => Op::Nop,
            0xa if a == 0 && b == 0 => Op::Brk,
            _ => ill,
        },
        _ => ill,
    }
}

fn r3(op: u16, a: u8, b: u8, f: u8) -> u16 {
    op << 12 | u16::from(a & 15) << 8 | u16::from(b & 15) << 4 | u16::from(f & 15)
}

fn index_of<T: PartialEq + Copy>(table: &[(T, &str)], v: T) -> u8 {
    table.iter().position(|(t, _)| *t == v).unwrap_or(0) as u8
}

/// Encode an op; the inverse of [`decode`] for every op it returns.
/// Out-of-range fields are masked (the assembler checks ranges first).
pub fn encode(op: &Op) -> u16 {
    match *op {
        Op::Alu { f, rd, rs } => r3(0, rd, rs, index_of(&ALU, f)),
        Op::Imm { f, rd, imm } => {
            (u16::from(index_of(&IMM, f)) + 1) << 12 | u16::from(rd & 15) << 8 | u16::from(imm)
        }
        Op::Mem { f, rd, rb, off } => {
            let n = index_of(&MEM, f);
            let field = if matches!(f, Mem::Lw | Mem::Sw) {
                off / 2
            } else {
                off
            };
            r3(5 + u16::from(n), rd, rb, field)
        }
        Op::X { f, post, rd, rs } => r3(9, rd, rs, index_of(&XOP, f) + if post { 4 } else { 0 }),
        Op::Branch { cond, off } => 0xa000 | u16::from(cond & 15) << 8 | u16::from(off as u8),
        Op::Bl { off } => 0xb000 | (off as u16 & 0x0fff),
        Op::Jr { rs } => r3(0xc, 0, rs, 0),
        Op::Jalr { rd, rs } => r3(0xc, rd, rs, 1),
        Op::Mts { sn, rs } => r3(0xc, sn, rs, 2),
        Op::Mfs { rd, sn } => r3(0xc, rd, sn, 3),
        Op::Jf { rd, rs } => r3(0xc, rd, rs, 4),
        Op::Callf { rd, rs } => r3(0xc, rd, rs, 5),
        Op::Md { f, rd, rs } => r3(0xd, rd, rs, index_of(&MD, f)),
        Op::Trap { n } => 0xe000 | u16::from(n) << 4,
        Op::Rti => 0xe001,
        Op::Ei => 0xe002,
        Op::Di => 0xe003,
        Op::Halt => 0xe004,
        Op::Mtsr { rs } => r3(0xe, 0, rs, 5),
        Op::Mfsr { rd } => r3(0xe, rd, 0, 6),
        Op::Mfsc { rd, c } => r3(0xe, rd, c, 7),
        Op::Mtsc { c, rs } => r3(0xe, c, rs, 8),
        Op::Nop => 0xe009,
        Op::Brk => 0xe00a,
        Op::Illegal(w) => w,
    }
}

/// The TRAP number sits in bits 11–4 (the Rd and Rs fields).
pub fn trap_number(w: u16) -> u8 {
    (w >> 4) as u8
}

fn rn(r: u8) -> String {
    format!("r{r}")
}

impl Op {
    /// Mnemonic and operands in the assembler's syntax; branch targets as
    /// absolute offsets for an instruction at offset `pc`.
    pub fn text(&self, pc: u16) -> (String, String) {
        let next = pc.wrapping_add(2);
        let s = |m: &str, o: String| (m.to_string(), o);
        match *self {
            Op::Alu { f, rd, rs } => s(
                ALU[usize::from(index_of(&ALU, f))].1,
                format!("{}, {}", rn(rd), rn(rs)),
            ),
            Op::Imm { f, rd, imm } => {
                let v = if f == Imm::Addi {
                    format!("#{}", imm as i8)
                } else {
                    format!("#${imm:02x}")
                };
                s(
                    IMM[usize::from(index_of(&IMM, f))].1,
                    format!("{}, {v}", rn(rd)),
                )
            }
            Op::Mem { f, rd, rb, off } => s(
                MEM[usize::from(index_of(&MEM, f))].1,
                format!("{}, {off}({})", rn(rd), rn(rb)),
            ),
            Op::X { f, post, rd, rs } => s(
                XOP[usize::from(index_of(&XOP, f))].1,
                format!("{}, ({}){}", rn(rd), rn(rs), if post { "+" } else { "" }),
            ),
            Op::Branch { cond, off } => {
                let t = next.wrapping_add((i16::from(off) * 2) as u16);
                s(COND[usize::from(cond)], format!("${t:04x}"))
            }
            Op::Bl { off } => {
                let t = next.wrapping_add((off * 2) as u16);
                s("bl", format!("${t:04x}"))
            }
            Op::Jr { rs } => s("jr", rn(rs)),
            Op::Jalr { rd, rs } => s("jalr", format!("{}, {}", rn(rd), rn(rs))),
            Op::Mts { sn, rs } => s("mts", format!("s{sn}, {}", rn(rs))),
            Op::Mfs { rd, sn } => s("mfs", format!("{}, s{sn}", rn(rd))),
            Op::Jf { rd, rs } => s("jf", format!("{}:{}", rn(rd), rn(rs))),
            Op::Callf { rd, rs } => s("callf", format!("{}:{}", rn(rd), rn(rs))),
            Op::Md { f, rd, rs } => s(
                MD[usize::from(index_of(&MD, f))].1,
                format!("{}, {}", rn(rd), rn(rs)),
            ),
            Op::Trap { n } => s("trap", format!("#{n}")),
            Op::Rti => s("rti", String::new()),
            Op::Ei => s("ei", String::new()),
            Op::Di => s("di", String::new()),
            Op::Halt => s("halt", String::new()),
            Op::Mtsr { rs } => s("mtsr", rn(rs)),
            Op::Mfsr { rd } => s("mfsr", rn(rd)),
            Op::Mfsc { rd, c } => s("mfsc", format!("{}, #{c}", rn(rd))),
            Op::Mtsc { c, rs } => s("mtsc", format!("#{c}, {}", rn(rs))),
            Op::Nop => s("nop", String::new()),
            Op::Brk => s("brk", String::new()),
            Op::Illegal(w) => s(".word", format!("${w:04x}")),
        }
    }

    /// Branch or jump target offset for an instruction at `pc`, when it is
    /// in the instruction itself.
    pub fn target(&self, pc: u16) -> Option<u16> {
        let next = pc.wrapping_add(2);
        match *self {
            Op::Branch { off, .. } => Some(next.wrapping_add((i16::from(off) * 2) as u16)),
            Op::Bl { off } => Some(next.wrapping_add((off * 2) as u16)),
            _ => None,
        }
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (m, o) = self.text(0);
        if o.is_empty() {
            f.write_str(&m)
        } else {
            write!(f, "{m} {o}")
        }
    }
}
