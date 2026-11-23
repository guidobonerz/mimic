//! Disassembler for RV32IM + Zicsr with the ABI register names and the
//! assembler's usual short forms (`nop`, `li`, `mv`, `j`, `jr`, `ret`).
//!
//! Mnemonics and operand order as in the RISC-V Unprivileged ISA manual and
//! the RISC-V assembly programmer's conventions.

use mimic_core::{Bus, Flow, Instr, Size};

use crate::exec::{imm_b, imm_i, imm_j, imm_s, EBREAK, ECALL, MRET, WFI};
use crate::ABI_NAMES;

fn reg(n: u32) -> &'static str {
    ABI_NAMES[(n & 31) as usize]
}

/// ra and t0 are the link registers of the calling convention.
fn is_link(n: u32) -> bool {
    n == 1 || n == 5
}

fn csr_name(addr: u32) -> String {
    let name = match addr {
        0x300 => "mstatus",
        0x301 => "misa",
        0x304 => "mie",
        0x305 => "mtvec",
        0x306 => "mcounteren",
        0x320 => "mcountinhibit",
        0x340 => "mscratch",
        0x341 => "mepc",
        0x342 => "mcause",
        0x343 => "mtval",
        0x344 => "mip",
        0xb00 => "mcycle",
        0xb02 => "minstret",
        0xb80 => "mcycleh",
        0xb82 => "minstreth",
        0xf11 => "mvendorid",
        0xf12 => "marchid",
        0xf13 => "mimpid",
        0xf14 => "mhartid",
        _ => return format!("0x{addr:03x}"),
    };
    name.to_string()
}

pub(crate) fn disassemble(bus: &dyn Bus, addr: u32) -> Instr {
    let mut bytes = [0u8; 4];
    for (n, b) in bytes.iter_mut().enumerate() {
        *b = bus.peek(addr.wrapping_add(n as u32), Size::Byte) as u8;
    }
    let i = u32::from_le_bytes(bytes);
    let (rd, rs1, rs2) = (i >> 7 & 31, i >> 15 & 31, i >> 20 & 31);
    let funct3 = (i >> 12 & 7) as usize;
    let funct7 = i >> 25;
    let simm = imm_i(i) as i32;
    let mut flow = Flow::Normal;
    let mut target = None;
    let unknown = || (".word".to_string(), format!("0x{i:08x}"));

    let (mnemonic, operand): (String, String) = match i & 0x7f {
        0x37 => ("lui".into(), format!("{},0x{:x}", reg(rd), i >> 12)),
        0x17 => ("auipc".into(), format!("{},0x{:x}", reg(rd), i >> 12)),
        0x6f => {
            let t = addr.wrapping_add(imm_j(i)) & !3;
            target = Some(t);
            if rd == 0 {
                flow = Flow::Jump;
                ("j".into(), format!("0x{t:08x}"))
            } else {
                flow = if is_link(rd) { Flow::Call } else { Flow::Jump };
                ("jal".into(), format!("{},0x{t:08x}", reg(rd)))
            }
        }
        0x67 if funct3 == 0 => {
            flow = if is_link(rd) {
                Flow::Call
            } else if is_link(rs1) {
                Flow::Return
            } else {
                Flow::Jump
            };
            if rd == 0 && rs1 == 1 && simm == 0 {
                ("ret".into(), String::new())
            } else if rd == 0 && simm == 0 {
                ("jr".into(), reg(rs1).to_string())
            } else {
                ("jalr".into(), format!("{},{simm}({})", reg(rd), reg(rs1)))
            }
        }
        0x63 if !matches!(funct3, 2 | 3) => {
            const NAMES: [&str; 8] = ["beq", "bne", "", "", "blt", "bge", "bltu", "bgeu"];
            let t = addr.wrapping_add(imm_b(i)) & !3;
            target = Some(t);
            flow = Flow::Branch;
            (
                NAMES[funct3].into(),
                format!("{},{},0x{t:08x}", reg(rs1), reg(rs2)),
            )
        }
        0x03 if !matches!(funct3, 3 | 6 | 7) => {
            const NAMES: [&str; 8] = ["lb", "lh", "lw", "", "lbu", "lhu", "", ""];
            (
                NAMES[funct3].into(),
                format!("{},{simm}({})", reg(rd), reg(rs1)),
            )
        }
        0x23 if funct3 < 3 => {
            const NAMES: [&str; 3] = ["sb", "sh", "sw"];
            (
                NAMES[funct3].into(),
                format!("{},{}({})", reg(rs2), imm_s(i) as i32, reg(rs1)),
            )
        }
        0x13 => match funct3 {
            1 | 5 => {
                let name = match (funct3, funct7) {
                    (1, 0) => "slli",
                    (5, 0) => "srli",
                    (5, 0x20) => "srai",
                    _ => "",
                };
                if name.is_empty() {
                    unknown()
                } else {
                    (name.into(), format!("{},{},{rs2}", reg(rd), reg(rs1)))
                }
            }
            0 if i == 0x13 => ("nop".into(), String::new()),
            0 if rs1 == 0 => ("li".into(), format!("{},{simm}", reg(rd))),
            0 if simm == 0 => ("mv".into(), format!("{},{}", reg(rd), reg(rs1))),
            _ => {
                const NAMES: [&str; 8] = ["addi", "", "slti", "sltiu", "xori", "", "ori", "andi"];
                (
                    NAMES[funct3].into(),
                    format!("{},{},{simm}", reg(rd), reg(rs1)),
                )
            }
        },
        0x33 => {
            const BASE: [&str; 8] = ["add", "sll", "slt", "sltu", "xor", "srl", "or", "and"];
            const MULDIV: [&str; 8] = [
                "mul", "mulh", "mulhsu", "mulhu", "div", "divu", "rem", "remu",
            ];
            let name = match (funct7, funct3) {
                (0, _) => BASE[funct3],
                (1, _) => MULDIV[funct3],
                (0x20, 0) => "sub",
                (0x20, 5) => "sra",
                _ => "",
            };
            if name.is_empty() {
                unknown()
            } else {
                (
                    name.into(),
                    format!("{},{},{}", reg(rd), reg(rs1), reg(rs2)),
                )
            }
        }
        0x0f if funct3 == 0 => ("fence".into(), String::new()),
        0x0f if funct3 == 1 => ("fence.i".into(), String::new()),
        0x73 => match (funct3, i) {
            (0, ECALL) => {
                // The handler comes back behind it.
                flow = Flow::Call;
                ("ecall".into(), String::new())
            }
            (0, EBREAK) => ("ebreak".into(), String::new()),
            (0, MRET) => {
                flow = Flow::Return;
                ("mret".into(), String::new())
            }
            (0, WFI) => ("wfi".into(), String::new()),
            (0 | 4, _) => unknown(),
            _ => {
                const NAMES: [&str; 8] = [
                    "", "csrrw", "csrrs", "csrrc", "", "csrrwi", "csrrsi", "csrrci",
                ];
                let csr = csr_name(i >> 20);
                let src = if funct3 & 4 != 0 {
                    rs1.to_string()
                } else {
                    reg(rs1).to_string()
                };
                (NAMES[funct3].into(), format!("{},{csr},{src}", reg(rd)))
            }
        },
        _ => unknown(),
    };

    Instr {
        addr,
        bytes: bytes.to_vec(),
        mnemonic,
        operand,
        target,
        flow,
    }
}
