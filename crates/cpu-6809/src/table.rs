//! Addressing modes, base cycle counts and mnemonics of the three opcode
//! pages (MC6809 datasheet, tables 9 and 10). Indexed modes add the
//! postbyte's cycles, PSH/PUL one per byte, RTI nine with E set, a taken
//! long conditional branch one.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Inh,
    Imm8,
    Imm16,
    Dir,
    Ext,
    Idx,
    Rel8,
    Rel16,
    /// PSHS/PULS (bit 6 = U) and PSHU/PULU (bit 6 = S).
    StackS,
    StackU,
    /// TFR/EXG register pair.
    Pair,
}

const BRANCH: [&str; 16] = [
    "BRA", "BRN", "BHI", "BLS", "BCC", "BCS", "BNE", "BEQ", "BVC", "BVS", "BPL", "BMI", "BGE",
    "BLT", "BGT", "BLE",
];
const RMW: [&str; 16] = [
    "NEG", "NEG", "NGC", "COM", "LSR", "LSR", "ROR", "ASR", "ASL", "ROL", "DEC", "DEC", "INC",
    "TST", "JMP", "CLR",
];
const ALU_A: [&str; 16] = [
    "SUBA", "CMPA", "SBCA", "SUBD", "ANDA", "BITA", "LDA", "STA", "EORA", "ADCA", "ORA", "ADDA",
    "CMPX", "JSR", "LDX", "STX",
];
const ALU_B: [&str; 16] = [
    "SUBB", "CMPB", "SBCB", "ADDD", "ANDB", "BITB", "LDB", "STB", "EORB", "ADCB", "ORB", "ADDB",
    "LDD", "STD", "LDU", "STU",
];

pub(crate) fn branch_name(cond: u8) -> &'static str {
    BRANCH[usize::from(cond & 15)]
}

/// Mode and base cycles of a page-0 opcode; None when undefined.
pub(crate) fn page0(op: u8) -> Option<(Mode, u8)> {
    let e = op & 15 == 0x0e;
    Some(match op {
        0x00..=0x0f => (Mode::Dir, if e { 3 } else { 6 }),
        0x12 | 0x1b => (Mode::Inh, 2),
        0x13 => (Mode::Inh, 2),
        0x16 => (Mode::Rel16, 5),
        0x17 => (Mode::Rel16, 9),
        0x19 | 0x1d => (Mode::Inh, 2),
        0x1a | 0x1c => (Mode::Imm8, 3),
        0x1e => (Mode::Pair, 8),
        0x1f => (Mode::Pair, 6),
        0x20..=0x2f => (Mode::Rel8, 3),
        0x30..=0x33 => (Mode::Idx, 4),
        0x34 | 0x35 => (Mode::StackS, 5),
        0x36 | 0x37 => (Mode::StackU, 5),
        0x39 => (Mode::Inh, 5),
        0x3a => (Mode::Inh, 3),
        0x3b => (Mode::Inh, 6),
        0x3c => (Mode::Imm8, 17),
        0x3d => (Mode::Inh, 11),
        0x3f => (Mode::Inh, 19),
        0x40..=0x5f => (Mode::Inh, 2),
        0x60..=0x6f => (Mode::Idx, if e { 3 } else { 6 }),
        0x70..=0x7f => (Mode::Ext, if e { 4 } else { 7 }),
        0x80..=0xff => return alu(op, 0),
        _ => return None,
    })
}

/// The $80–$FF block: column 0 immediate, 1 direct, 2 indexed, 3 extended.
fn alu(op: u8, extra: u8) -> Option<(Mode, u8)> {
    let col = usize::from((op >> 4) & 3);
    let k = op & 15;
    let b = op >= 0xc0;
    if col == 0 && (k == 7 || k == 15 || (b && k == 13)) {
        return None;
    }
    if !b && k == 13 && col == 0 {
        return Some((Mode::Rel8, 7));
    }
    let wide = matches!(k, 3 | 12 | 14 | 15) || (b && k == 13);
    let cycles = if k == 3 || (!b && k == 12) {
        [4, 6, 6, 7]
    } else if !b && k == 13 {
        [7, 7, 7, 8]
    } else if wide {
        [3, 5, 5, 6]
    } else {
        [2, 4, 4, 5]
    };
    let mode = [
        if wide { Mode::Imm16 } else { Mode::Imm8 },
        Mode::Dir,
        Mode::Idx,
        Mode::Ext,
    ][col];
    Some((mode, cycles[col] + extra))
}

/// Mode and base cycles (with the prefix) of a page-2 ($10) opcode.
pub(crate) fn page2(op: u8) -> Option<(Mode, u8)> {
    match op {
        0x21..=0x2f => Some((Mode::Rel16, 5)),
        0x3f => Some((Mode::Inh, 20)),
        0x83 | 0x93 | 0xa3 | 0xb3 | 0x8c | 0x9c | 0xac | 0xbc => alu(op, 1),
        0x8e | 0x9e | 0xae | 0xbe | 0x9f | 0xaf | 0xbf => alu(op, 1),
        0xce | 0xde | 0xee | 0xfe | 0xdf | 0xef | 0xff => alu(op, 1),
        _ => None,
    }
}

/// Mode and base cycles (with the prefix) of a page-3 ($11) opcode.
pub(crate) fn page3(op: u8) -> Option<(Mode, u8)> {
    match op {
        0x3f => Some((Mode::Inh, 20)),
        0x83 | 0x93 | 0xa3 | 0xb3 | 0x8c | 0x9c | 0xac | 0xbc => alu(op, 1),
        _ => None,
    }
}

/// The mnemonic of an opcode on page 0, 2 ($10) or 3 ($11).
pub(crate) fn name(page: u8, op: u8) -> String {
    let k = usize::from(op & 15);
    match (page, op) {
        (2, 0x21..=0x2f) => format!("L{}", branch_name(op)),
        (2, 0x3f) => "SWI2".into(),
        (3, 0x3f) => "SWI3".into(),
        (2, _) => match k {
            3 => "CMPD",
            12 => "CMPY",
            _ if op >= 0xc0 => ["LDS", "STS"][k & 1],
            _ => ["LDY", "STY"][k & 1],
        }
        .into(),
        (3, _) => if k == 3 { "CMPU" } else { "CMPS" }.into(),
        (_, 0x00..=0x0f) | (_, 0x60..=0x7f) => RMW[k].into(),
        (_, 0x40..=0x5f) => {
            let n = if k == 14 { "CLR" } else { RMW[k] };
            format!("{n}{}", if op < 0x50 { 'A' } else { 'B' })
        }
        (_, 0x20..=0x2f) => branch_name(op).into(),
        (_, 0x8d) => "BSR".into(),
        (_, 0x80..=0xbf) => ALU_A[k].into(),
        (_, 0xc0..=0xff) => ALU_B[k].into(),
        _ => match op {
            0x12 => "NOP",
            0x13 => "SYNC",
            0x14 | 0x15 => "HCF",
            0x16 => "LBRA",
            0x17 => "LBSR",
            0x19 => "DAA",
            0x1a => "ORCC",
            0x1b => "NOP",
            0x1c => "ANDCC",
            0x1d => "SEX",
            0x1e => "EXG",
            0x1f => "TFR",
            0x30 => "LEAX",
            0x31 => "LEAY",
            0x32 => "LEAS",
            0x33 => "LEAU",
            0x34 => "PSHS",
            0x35 => "PULS",
            0x36 => "PSHU",
            0x37 => "PULU",
            0x39 => "RTS",
            0x3a => "ABX",
            0x3b => "RTI",
            0x3c => "CWAI",
            0x3d => "MUL",
            0x3f => "SWI",
            _ => "???",
        }
        .into(),
    }
}
