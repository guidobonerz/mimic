//! Disassembler. Reads with `Bus816::peek` only (golden rule 2). The
//! length of immediate operands depends on the M and X flags, which the
//! caller passes (normally the CPU's current ones).

use emu_core::{Flow, Instr};

use crate::Bus816;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Am {
    Imp,
    Acc,
    /// Immediate, width from M.
    ImmM,
    /// Immediate, width from X.
    ImmX,
    Imm8,
    Dp,
    DpX,
    DpY,
    Abs,
    AbsX,
    AbsY,
    Long,
    LongX,
    DpInd,
    DpIndX,
    DpIndY,
    DpIndLong,
    DpIndLongY,
    Sr,
    SrIndY,
    Rel,
    RelLong,
    AbsInd,
    AbsIndX,
    AbsIndLong,
    Move,
}

const NAMES: [&str; 256] = [
    "BRK", "ORA", "COP", "ORA", "TSB", "ORA", "ASL", "ORA", "PHP", "ORA", "ASL", "PHD", "TSB",
    "ORA", "ASL", "ORA", //
    "BPL", "ORA", "ORA", "ORA", "TRB", "ORA", "ASL", "ORA", "CLC", "ORA", "INC", "TCS", "TRB",
    "ORA", "ASL", "ORA", //
    "JSR", "AND", "JSL", "AND", "BIT", "AND", "ROL", "AND", "PLP", "AND", "ROL", "PLD", "BIT",
    "AND", "ROL", "AND", //
    "BMI", "AND", "AND", "AND", "BIT", "AND", "ROL", "AND", "SEC", "AND", "DEC", "TSC", "BIT",
    "AND", "ROL", "AND", //
    "RTI", "EOR", "WDM", "EOR", "MVP", "EOR", "LSR", "EOR", "PHA", "EOR", "LSR", "PHK", "JMP",
    "EOR", "LSR", "EOR", //
    "BVC", "EOR", "EOR", "EOR", "MVN", "EOR", "LSR", "EOR", "CLI", "EOR", "PHY", "TCD", "JML",
    "EOR", "LSR", "EOR", //
    "RTS", "ADC", "PER", "ADC", "STZ", "ADC", "ROR", "ADC", "PLA", "ADC", "ROR", "RTL", "JMP",
    "ADC", "ROR", "ADC", //
    "BVS", "ADC", "ADC", "ADC", "STZ", "ADC", "ROR", "ADC", "SEI", "ADC", "PLY", "TDC", "JMP",
    "ADC", "ROR", "ADC", //
    "BRA", "STA", "BRL", "STA", "STY", "STA", "STX", "STA", "DEY", "BIT", "TXA", "PHB", "STY",
    "STA", "STX", "STA", //
    "BCC", "STA", "STA", "STA", "STY", "STA", "STX", "STA", "TYA", "STA", "TXS", "TXY", "STZ",
    "STA", "STZ", "STA", //
    "LDY", "LDA", "LDX", "LDA", "LDY", "LDA", "LDX", "LDA", "TAY", "LDA", "TAX", "PLB", "LDY",
    "LDA", "LDX", "LDA", //
    "BCS", "LDA", "LDA", "LDA", "LDY", "LDA", "LDX", "LDA", "CLV", "LDA", "TSX", "TYX", "LDY",
    "LDA", "LDX", "LDA", //
    "CPY", "CMP", "REP", "CMP", "CPY", "CMP", "DEC", "CMP", "INY", "CMP", "DEX", "WAI", "CPY",
    "CMP", "DEC", "CMP", //
    "BNE", "CMP", "CMP", "CMP", "PEI", "CMP", "DEC", "CMP", "CLD", "CMP", "PHX", "STP", "JML",
    "CMP", "DEC", "CMP", //
    "CPX", "SBC", "SEP", "SBC", "CPX", "SBC", "INC", "SBC", "INX", "SBC", "NOP", "XBA", "CPX",
    "SBC", "INC", "SBC", //
    "BEQ", "SBC", "SBC", "SBC", "PEA", "SBC", "INC", "SBC", "SED", "SBC", "PLX", "XCE", "JSR",
    "SBC", "INC", "SBC", //
];

fn mode(op: u8) -> Am {
    match op {
        0x00 | 0x02 | 0x42 | 0xc2 | 0xe2 => Am::Imm8,
        0x20 | 0x4c => Am::Abs,
        0x22 | 0x5c => Am::Long,
        0x6c => Am::AbsInd,
        0x7c | 0xfc => Am::AbsIndX,
        0xdc => Am::AbsIndLong,
        0x82 | 0x62 => Am::RelLong,
        0x44 | 0x54 => Am::Move,
        0xf4 => Am::Abs,
        0xd4 => Am::DpInd,
        0x0a | 0x1a | 0x2a | 0x3a | 0x4a | 0x6a => Am::Acc,
        0xa0 | 0xa2 | 0xc0 | 0xe0 => Am::ImmX,
        0x89 => Am::ImmM,
        0x96 | 0xb6 => Am::DpY,
        0xbe => Am::AbsY,
        _ if op & 0x1f == 0x10 || op == 0x80 => Am::Rel,
        _ => match op & 0x1f {
            0x01 => Am::DpIndX,
            0x03 => Am::Sr,
            0x04..=0x06 => Am::Dp,
            0x07 => Am::DpIndLong,
            0x09 => Am::ImmM,
            0x0c..=0x0e => Am::Abs,
            0x0f => Am::Long,
            0x11 => Am::DpIndY,
            0x12 => Am::DpInd,
            0x13 => Am::SrIndY,
            0x14..=0x16 => Am::DpX,
            0x17 => Am::DpIndLongY,
            0x19 => Am::AbsY,
            0x1c..=0x1e => Am::AbsX,
            0x1f => Am::LongX,
            _ => Am::Imp,
        },
    }
}

/// Disassemble the instruction at `addr` (24 bits) with the given M and X
/// widths (true = 8 bits).
pub fn disassemble(bus: &dyn Bus816, addr: u32, m8: bool, x8: bool) -> Instr {
    let at = |n: u32| bus.peek((addr & 0xff_0000) | ((addr + n) & 0xffff));
    let op = at(0);
    let am = mode(op);
    let len = match am {
        Am::Imp | Am::Acc => 1,
        Am::ImmM => 3 - u32::from(m8),
        Am::ImmX => 3 - u32::from(x8),
        Am::Imm8 | Am::Dp | Am::DpX | Am::DpY | Am::DpInd | Am::DpIndX | Am::DpIndY => 2,
        Am::DpIndLong | Am::DpIndLongY | Am::Sr | Am::SrIndY | Am::Rel => 2,
        Am::Abs | Am::AbsX | Am::AbsY | Am::AbsInd | Am::AbsIndX | Am::AbsIndLong => 3,
        Am::RelLong | Am::Move => 3,
        Am::Long | Am::LongX => 4,
    };
    let bytes: Vec<u8> = (0..len).map(at).collect();
    let b1 = u32::from(*bytes.get(1).unwrap_or(&0));
    let w = b1 | u32::from(*bytes.get(2).unwrap_or(&0)) << 8;
    let l = w | u32::from(*bytes.get(3).unwrap_or(&0)) << 16;
    let next = (addr & 0xffff) + len;
    let bank = addr & 0xff_0000;
    let mut target = None;
    let operand = match am {
        Am::Imp => String::new(),
        Am::Acc => "A".into(),
        Am::Imm8 => format!("#${b1:02X}"),
        Am::ImmM | Am::ImmX => {
            if len == 2 {
                format!("#${b1:02X}")
            } else {
                format!("#${w:04X}")
            }
        }
        Am::Dp => format!("${b1:02X}"),
        Am::DpX => format!("${b1:02X},X"),
        Am::DpY => format!("${b1:02X},Y"),
        Am::Abs => {
            if matches!(op, 0x20 | 0x4c) {
                target = Some(bank | w);
            }
            if op == 0xf4 {
                format!("#${w:04X}")
            } else {
                format!("${w:04X}")
            }
        }
        Am::AbsX => format!("${w:04X},X"),
        Am::AbsY => format!("${w:04X},Y"),
        Am::Long => {
            if matches!(op, 0x22 | 0x5c) {
                target = Some(l);
            }
            format!("${l:06X}")
        }
        Am::LongX => format!("${l:06X},X"),
        Am::DpInd => format!("(${b1:02X})"),
        Am::DpIndX => format!("(${b1:02X},X)"),
        Am::DpIndY => format!("(${b1:02X}),Y"),
        Am::DpIndLong => format!("[${b1:02X}]"),
        Am::DpIndLongY => format!("[${b1:02X}],Y"),
        Am::Sr => format!("${b1:02X},S"),
        Am::SrIndY => format!("(${b1:02X},S),Y"),
        Am::Rel => {
            let t = bank | (next.wrapping_add(b1 as u8 as i8 as u32) & 0xffff);
            target = Some(t);
            format!("${t:06X}")
        }
        Am::RelLong => {
            let t = bank | (next.wrapping_add(w) & 0xffff);
            if op == 0x82 {
                target = Some(t);
            }
            format!("${t:06X}")
        }
        Am::AbsInd => format!("(${w:04X})"),
        Am::AbsIndX => format!("(${w:04X},X)"),
        Am::AbsIndLong => format!("[${w:04X}]"),
        Am::Move => format!("${:02X},${b1:02X}", w >> 8),
    };
    let flow = match op {
        0x20 | 0x22 | 0xfc => Flow::Call,
        0x60 | 0x6b | 0x40 => Flow::Return,
        0x4c | 0x5c | 0x6c | 0x7c | 0xdc | 0x80 | 0x82 => Flow::Jump,
        _ if am == Am::Rel => Flow::Branch,
        _ => Flow::Normal,
    };
    Instr {
        addr,
        bytes,
        mnemonic: NAMES[usize::from(op)].into(),
        operand,
        target,
        flow,
    }
}
