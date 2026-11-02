//! Disassembler with the Acorn mnemonics.

use emu_core::{Bus, Flow, Instr, Size};

const COND: [&str; 16] = [
    "EQ", "NE", "CS", "CC", "MI", "PL", "VS", "VC", "HI", "LS", "GE", "LT", "GT", "LE", "", "NV",
];
const OPS: [&str; 16] = [
    "AND", "EOR", "SUB", "RSB", "ADD", "ADC", "SBC", "RSC", "TST", "TEQ", "CMP", "CMN", "ORR",
    "MOV", "BIC", "MVN",
];
const SHIFTS: [&str; 4] = ["LSL", "LSR", "ASR", "ROR"];

fn reg(n: u32) -> &'static str {
    const NAMES: [&str; 16] = [
        "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
        "R14", "PC",
    ];
    NAMES[(n & 15) as usize]
}

/// Rm with its shift.
fn shifted(i: u32) -> String {
    let rm = reg(i);
    let kind = (i >> 5 & 3) as usize;
    if i & 0x10 != 0 {
        return format!("{rm},{} {}", SHIFTS[kind], reg(i >> 8));
    }
    match (kind, i >> 7 & 31) {
        (0, 0) => rm.to_string(),
        (3, 0) => format!("{rm},RRX"),
        (1 | 2, 0) => format!("{rm},{} #32", SHIFTS[kind]),
        (_, n) => format!("{rm},{} #{n}", SHIFTS[kind]),
    }
}

fn data_processing(i: u32, c: &str) -> (String, String) {
    let op = (i >> 21 & 15) as usize;
    let set = i & 0x0010_0000 != 0;
    let (rd, rn) = (i >> 12 & 15, i >> 16 & 15);
    let op2 = if i & 0x0200_0000 != 0 {
        format!("#&{:X}", (i & 0xff).rotate_right((i >> 8 & 15) * 2))
    } else {
        shifted(i)
    };
    match op {
        8..=11 => {
            let p = if rd == 15 { "P" } else { "" };
            (format!("{}{c}{p}", OPS[op]), format!("{},{op2}", reg(rn)))
        }
        13 | 15 => (
            format!("{}{c}{}", OPS[op], if set { "S" } else { "" }),
            format!("{},{op2}", reg(rd)),
        ),
        _ => (
            format!("{}{c}{}", OPS[op], if set { "S" } else { "" }),
            format!("{},{},{op2}", reg(rd), reg(rn)),
        ),
    }
}

fn single_transfer(i: u32, c: &str) -> (String, String) {
    let load = i & 0x0010_0000 != 0;
    let byte = if i & 0x0040_0000 != 0 { "B" } else { "" };
    let pre = i & 0x0100_0000 != 0;
    let wb = i & 0x0020_0000 != 0;
    let t = if !pre && wb { "T" } else { "" };
    let sign = if i & 0x0080_0000 != 0 { "" } else { "-" };
    let offset = if i & 0x0200_0000 != 0 {
        format!("{sign}{}", shifted(i & !0x10))
    } else {
        format!("#{sign}&{:X}", i & 0xfff)
    };
    let (rd, rn) = (reg(i >> 12), reg(i >> 16));
    let m = format!("{}{c}{byte}{t}", if load { "LDR" } else { "STR" });
    let o = if pre {
        format!("{rd},[{rn},{offset}]{}", if wb { "!" } else { "" })
    } else {
        format!("{rd},[{rn}],{offset}")
    };
    (m, o)
}

fn block_transfer(i: u32, c: &str) -> (String, String) {
    let load = i & 0x0010_0000 != 0;
    let dir = match (i >> 23 & 1, i >> 24 & 1) {
        (1, 0) => "IA",
        (1, 1) => "IB",
        (0, 0) => "DA",
        _ => "DB",
    };
    let regs: Vec<&str> = (0..16).filter(|n| i >> n & 1 != 0).map(reg).collect();
    let o = format!(
        "{}{},{{{}}}{}",
        reg(i >> 16),
        if i & 0x0020_0000 != 0 { "!" } else { "" },
        regs.join(","),
        if i & 0x0040_0000 != 0 { "^" } else { "" }
    );
    (format!("{}{c}{dir}", if load { "LDM" } else { "STM" }), o)
}

pub(crate) fn disassemble(bus: &dyn Bus, addr: u32) -> Instr {
    let addr = addr & 0x03ff_fffc;
    let i = bus.peek(addr, Size::Long);
    let c = COND[(i >> 28) as usize];
    let conditional = !c.is_empty();
    let writes_pc = i >> 12 & 15 == 15;
    let (mnemonic, operand, flow, target) = match i >> 25 & 7 {
        0 | 1 if i & 0x0200_0000 == 0 && i & 0x0fc0_00f0 == 0x90 => {
            let (rd, rs, rm, rn) = (i >> 16, i >> 8, i, i >> 12);
            let set = if i & 0x0010_0000 != 0 { "S" } else { "" };
            if i & 0x0020_0000 != 0 {
                (
                    format!("MLA{c}{set}"),
                    format!("{},{},{},{}", reg(rd), reg(rm), reg(rs), reg(rn)),
                    Flow::Normal,
                    None,
                )
            } else {
                (
                    format!("MUL{c}{set}"),
                    format!("{},{},{}", reg(rd), reg(rm), reg(rs)),
                    Flow::Normal,
                    None,
                )
            }
        }
        0 | 1 => {
            let (m, o) = data_processing(i, c);
            let test = (8..=11).contains(&(i >> 21 & 15));
            let flow = if writes_pc && !test {
                if i >> 21 & 15 == 13 && i & 0x0200_0000 == 0 && i & 15 == 14 {
                    Flow::Return
                } else {
                    Flow::Jump
                }
            } else {
                Flow::Normal
            };
            (m, o, flow, None)
        }
        2 | 3 => {
            let (m, o) = single_transfer(i, c);
            let flow = if writes_pc && i & 0x0010_0000 != 0 {
                Flow::Jump
            } else {
                Flow::Normal
            };
            (m, o, flow, None)
        }
        4 => {
            let (m, o) = block_transfer(i, c);
            let flow = if i & 0x0010_8000 == 0x0010_8000 {
                Flow::Return
            } else {
                Flow::Normal
            };
            (m, o, flow, None)
        }
        5 => {
            let link = i & 0x0100_0000 != 0;
            let dest = addr
                .wrapping_add(8)
                .wrapping_add(((i << 8) as i32 >> 6) as u32)
                & 0x03ff_ffff;
            let flow = match (link, conditional) {
                (true, _) => Flow::Call,
                (false, true) => Flow::Branch,
                (false, false) => Flow::Jump,
            };
            (
                format!("B{}{c}", if link { "L" } else { "" }),
                format!("&{dest:X}"),
                flow,
                Some(dest),
            )
        }
        7 if i & 0x0100_0000 != 0 => (
            format!("SWI{c}"),
            format!("&{:X}", i & 0xff_ffff),
            Flow::Call,
            None,
        ),
        _ => (
            format!("CDP{c}"),
            format!("&{:X}", i & 0x0fff_ffff),
            Flow::Normal,
            None,
        ),
    };
    Instr {
        addr,
        bytes: i.to_le_bytes().to_vec(),
        mnemonic,
        operand,
        target,
        flow,
    }
}
