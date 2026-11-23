//! Disassembler with the mnemonics of the Game Boy's documentation (the
//! Z80's, with `LDH` for the page `$FF00` and `(HL+)`/`(HL-)`).

use mimic_core::{Bus, Flow, Instr, Size};

const REG: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const PAIR: [&str; 4] = ["BC", "DE", "HL", "SP"];
const COND: [&str; 4] = ["NZ", "Z", "NC", "C"];
const ALU: [&str; 8] = ["ADD", "ADC", "SUB", "SBC", "AND", "XOR", "OR", "CP"];
const SHIFT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SWAP", "SRL"];

pub(crate) fn disassemble(bus: &dyn Bus, addr: u32) -> Instr {
    let at = |i: u32| bus.peek(addr.wrapping_add(i) & 0xffff, Size::Byte) as u8;
    let op = at(0);
    let d8 = format!("${:02X}", at(1));
    let word = u16::from_le_bytes([at(1), at(2)]);
    let a16 = format!("${word:04X}");
    let jump = Some(u32::from(word));
    let rel = (addr.wrapping_add(2) as u16).wrapping_add(at(1) as i8 as u16);
    let e8 = format!("${rel:04X}");
    let dst = usize::from(op >> 3 & 7);
    let src = usize::from(op & 7);
    let rp = usize::from(op >> 4 & 3);
    let cc = COND[dst & 3];
    let n = |m: &str, o: &str, len: u32| (m.to_string(), o.to_string(), len, Flow::Normal, None);
    let alu = |i: usize, o: &str| {
        // ADD, ADC and SBC name A; the others do not.
        if matches!(i, 0 | 1 | 3) {
            format!("A,{o}")
        } else {
            o.to_string()
        }
    };
    let (mnemonic, operand, len, flow, target) = match op {
        0x00 => n("NOP", "", 1),
        0x10 => n("STOP", "", 2),
        0x76 => n("HALT", "", 1),
        0x08 => n("LD", &format!("({a16}),SP"), 3),
        0x01 | 0x11 | 0x21 | 0x31 => n("LD", &format!("{},{a16}", PAIR[rp]), 3),
        0x02 | 0x12 => n("LD", &format!("({}),A", PAIR[rp]), 1),
        0x0a | 0x1a => n("LD", &format!("A,({})", PAIR[rp]), 1),
        0x22 => n("LD", "(HL+),A", 1),
        0x32 => n("LD", "(HL-),A", 1),
        0x2a => n("LD", "A,(HL+)", 1),
        0x3a => n("LD", "A,(HL-)", 1),
        0x03 | 0x13 | 0x23 | 0x33 => n("INC", PAIR[rp], 1),
        0x0b | 0x1b | 0x2b | 0x3b => n("DEC", PAIR[rp], 1),
        0x09 | 0x19 | 0x29 | 0x39 => n("ADD", &format!("HL,{}", PAIR[rp]), 1),
        0x07 => n("RLCA", "", 1),
        0x0f => n("RRCA", "", 1),
        0x17 => n("RLA", "", 1),
        0x1f => n("RRA", "", 1),
        0x27 => n("DAA", "", 1),
        0x2f => n("CPL", "", 1),
        0x37 => n("SCF", "", 1),
        0x3f => n("CCF", "", 1),
        0x18 => ("JR".into(), e8, 2, Flow::Jump, Some(u32::from(rel))),
        0x20 | 0x28 | 0x30 | 0x38 => (
            "JR".into(),
            format!("{cc},{e8}"),
            2,
            Flow::Branch,
            Some(u32::from(rel)),
        ),
        _ if op < 0x40 => match op & 7 {
            4 => n("INC", REG[dst], 1),
            5 => n("DEC", REG[dst], 1),
            _ => n("LD", &format!("{},{d8}", REG[dst]), 2),
        },
        0x40..=0x7f => n("LD", &format!("{},{}", REG[dst], REG[src]), 1),
        0x80..=0xbf => n(ALU[dst], &alu(dst, REG[src]), 1),
        0xcb => {
            let op = at(1);
            let (reg, bit) = (REG[usize::from(op & 7)], op >> 3 & 7);
            match op >> 6 {
                0 => n(SHIFT[usize::from(bit)], reg, 2),
                1 => n("BIT", &format!("{bit},{reg}"), 2),
                2 => n("RES", &format!("{bit},{reg}"), 2),
                _ => n("SET", &format!("{bit},{reg}"), 2),
            }
        }
        0xc1 | 0xd1 | 0xe1 => n("POP", PAIR[rp], 1),
        0xf1 => n("POP", "AF", 1),
        0xc5 | 0xd5 | 0xe5 => n("PUSH", PAIR[rp], 1),
        0xf5 => n("PUSH", "AF", 1),
        0xc3 => ("JP".into(), a16, 3, Flow::Jump, jump),
        0xc2 | 0xca | 0xd2 | 0xda => ("JP".into(), format!("{cc},{a16}"), 3, Flow::Branch, jump),
        0xe9 => ("JP".into(), "HL".into(), 1, Flow::Jump, None),
        0xcd => ("CALL".into(), a16, 3, Flow::Call, jump),
        0xc4 | 0xcc | 0xd4 | 0xdc => ("CALL".into(), format!("{cc},{a16}"), 3, Flow::Call, jump),
        0xc9 => ("RET".into(), String::new(), 1, Flow::Return, None),
        0xd9 => ("RETI".into(), String::new(), 1, Flow::Return, None),
        0xc0 | 0xc8 | 0xd0 | 0xd8 => ("RET".into(), cc.into(), 1, Flow::Return, None),
        0xc7 | 0xcf | 0xd7 | 0xdf | 0xe7 | 0xef | 0xf7 | 0xff => (
            "RST".into(),
            format!("${:02X}", op & 0x38),
            1,
            Flow::Call,
            Some(u32::from(op & 0x38)),
        ),
        0xc6 | 0xce | 0xd6 | 0xde | 0xe6 | 0xee | 0xf6 | 0xfe => n(ALU[dst], &alu(dst, &d8), 2),
        0xe0 => n("LDH", &format!("($FF{:02X}),A", at(1)), 2),
        0xf0 => n("LDH", &format!("A,($FF{:02X})", at(1)), 2),
        0xe2 => n("LDH", "(C),A", 1),
        0xf2 => n("LDH", "A,(C)", 1),
        0xea => n("LD", &format!("({a16}),A"), 3),
        0xfa => n("LD", &format!("A,({a16})"), 3),
        0xe8 => n("ADD", &format!("SP,{}", at(1) as i8), 2),
        0xf8 => n("LD", &format!("HL,SP{:+}", at(1) as i8), 2),
        0xf9 => n("LD", "SP,HL", 1),
        0xf3 => n("DI", "", 1),
        0xfb => n("EI", "", 1),
        _ => n("???", &format!("${op:02X}"), 1),
    };
    Instr {
        addr,
        bytes: (0..len).map(at).collect(),
        mnemonic,
        operand,
        target,
        flow,
    }
}
