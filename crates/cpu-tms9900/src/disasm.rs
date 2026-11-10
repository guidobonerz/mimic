//! Disassembler with TI's mnemonics and operand syntax (`*R1+`,
//! `@>8C00(R2)`, hexadecimal with `>`).

use emu_core::{Bus, Flow, Instr, Size};

const FORMAT1: [&str; 12] = [
    "SZC", "SZCB", "S", "SB", "C", "CB", "A", "AB", "MOV", "MOVB", "SOC", "SOCB",
];
const JUMPS: [&str; 13] = [
    "JMP", "JLT", "JLE", "JEQ", "JHE", "JGT", "JNE", "JNC", "JOC", "JNO", "JL", "JH", "JOP",
];
const FORMAT3: [&str; 8] = ["COC", "CZC", "XOR", "XOP", "LDCR", "STCR", "MPY", "DIV"];
const SHIFTS: [&str; 4] = ["SRA", "SRL", "SLA", "SRC"];
const FORMAT6: [&str; 14] = [
    "BLWP", "B", "X", "CLR", "NEG", "INV", "INC", "INCT", "DEC", "DECT", "BL", "SWPB", "SETO",
    "ABS",
];

pub(crate) fn disassemble(bus: &dyn Bus, addr: u32) -> Instr {
    let word = |i: u32| bus.peek(addr.wrapping_add(i * 2) & 0xfffe, Size::Word) as u16;
    let op = word(0);
    let mut len = 1u32;
    // A general operand; symbolic ones take the next word.
    let mut gen = |mode: u16, reg: u16| -> String {
        match mode {
            0 => format!("R{reg}"),
            1 => format!("*R{reg}"),
            2 => {
                let v = word(len);
                len += 1;
                if reg == 0 {
                    format!("@>{v:04X}")
                } else {
                    format!("@>{v:04X}(R{reg})")
                }
            }
            _ => format!("*R{reg}+"),
        }
    };
    let (mut mnemonic, mut operand, mut flow, mut target) = (
        String::from("DATA"),
        format!(">{op:04X}"),
        Flow::Normal,
        None,
    );
    let next = addr.wrapping_add(2) & 0xffff;
    match op >> 12 {
        0x4..=0xf => {
            mnemonic = FORMAT1[usize::from((op >> 12) - 4)].into();
            let s = gen((op >> 4) & 3, op & 15);
            let d = gen((op >> 10) & 3, (op >> 6) & 15);
            operand = format!("{s},{d}");
        }
        0x1 => {
            let disp = (op & 0xff) as i8;
            let code = usize::from((op >> 8) & 0x0f);
            if code < JUMPS.len() {
                let t = next.wrapping_add((i32::from(disp) * 2) as u32) & 0xffff;
                mnemonic = JUMPS[code].into();
                operand = format!(">{t:04X}");
                target = Some(t);
                flow = if code == 0 { Flow::Jump } else { Flow::Branch };
                if op == 0x1000 {
                    mnemonic = "NOP".into();
                    operand.clear();
                    flow = Flow::Normal;
                    target = None;
                }
            } else {
                mnemonic = ["SBO", "SBZ", "TB"][code - 13].into();
                operand = format!("{disp}");
            }
        }
        0x2 | 0x3 => {
            let k = usize::from((op >> 10) & 7);
            let d = (op >> 6) & 15;
            mnemonic = FORMAT3[k].into();
            let s = gen((op >> 4) & 3, op & 15);
            operand = match k {
                3..=5 => format!("{s},{d}"),
                _ => format!("{s},R{d}"),
            };
            if k == 3 {
                flow = Flow::Call;
            }
        }
        _ => match op >> 8 {
            0x08..=0x0b => {
                mnemonic = SHIFTS[usize::from((op >> 8) - 8)].into();
                operand = format!("R{},{}", op & 15, (op >> 4) & 15);
            }
            0x04..=0x07 if op < 0x0780 => {
                let k = usize::from((op - 0x0400) >> 6);
                mnemonic = FORMAT6[k].into();
                let (mode, reg) = ((op >> 4) & 3, op & 15);
                if mode == 2 && reg == 0 {
                    target = Some(u32::from(word(1)));
                }
                operand = gen(mode, reg);
                flow = match k {
                    0 | 10 => Flow::Call,
                    1 if op == 0x045b => Flow::Return,
                    1 => Flow::Jump,
                    _ => Flow::Normal,
                };
            }
            0x02 | 0x03 => {
                let w = op & 15;
                let imm = |len: &mut u32| {
                    let v = word(*len);
                    *len += 1;
                    format!(">{v:04X}")
                };
                match op & 0xffe0 {
                    0x0200 => {
                        (mnemonic, operand) = ("LI".into(), format!("R{w},{}", imm(&mut len)))
                    }
                    0x0220 => {
                        (mnemonic, operand) = ("AI".into(), format!("R{w},{}", imm(&mut len)))
                    }
                    0x0240 => {
                        (mnemonic, operand) = ("ANDI".into(), format!("R{w},{}", imm(&mut len)))
                    }
                    0x0260 => {
                        (mnemonic, operand) = ("ORI".into(), format!("R{w},{}", imm(&mut len)))
                    }
                    0x0280 => {
                        (mnemonic, operand) = ("CI".into(), format!("R{w},{}", imm(&mut len)))
                    }
                    0x02a0 => (mnemonic, operand) = ("STWP".into(), format!("R{w}")),
                    0x02c0 => (mnemonic, operand) = ("STST".into(), format!("R{w}")),
                    0x02e0 => (mnemonic, operand) = ("LWPI".into(), imm(&mut len)),
                    0x0300 => (mnemonic, operand) = ("LIMI".into(), imm(&mut len)),
                    0x0340 => (mnemonic, operand) = ("IDLE".into(), String::new()),
                    0x0360 => (mnemonic, operand) = ("RSET".into(), String::new()),
                    0x0380 => {
                        (mnemonic, operand) = ("RTWP".into(), String::new());
                        flow = Flow::Return;
                    }
                    0x03a0 => (mnemonic, operand) = ("CKON".into(), String::new()),
                    0x03c0 => (mnemonic, operand) = ("CKOF".into(), String::new()),
                    0x03e0 => (mnemonic, operand) = ("LREX".into(), String::new()),
                    _ => {}
                }
            }
            _ => {}
        },
    }
    let bytes = (0..len)
        .flat_map(|i| word(i).to_be_bytes())
        .collect::<Vec<u8>>();
    Instr {
        addr,
        bytes,
        mnemonic,
        operand,
        target,
        flow,
    }
}
