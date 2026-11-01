//! Disassembler with the Intel mnemonics.

use emu_core::{Bus, Flow, Instr, Size};

const REG: [&str; 8] = ["B", "C", "D", "E", "H", "L", "M", "A"];
const PAIR: [&str; 4] = ["B", "D", "H", "SP"];
const COND: [&str; 8] = ["NZ", "Z", "NC", "C", "PO", "PE", "P", "M"];
const ALU: [&str; 8] = ["ADD", "ADC", "SUB", "SBB", "ANA", "XRA", "ORA", "CMP"];
const ALU_IMM: [&str; 8] = ["ADI", "ACI", "SUI", "SBI", "ANI", "XRI", "ORI", "CPI"];

pub(crate) fn disassemble(bus: &dyn Bus, addr: u32) -> Instr {
    let at = |i: u32| bus.peek(addr.wrapping_add(i) & 0xffff, Size::Byte) as u8;
    let op = at(0);
    let d8 = format!("${:02X}", at(1));
    let a16 = format!("${:04X}", u16::from_le_bytes([at(1), at(2)]));
    let jump = Some(u32::from(u16::from_le_bytes([at(1), at(2)])));
    let dst = usize::from(op >> 3 & 7);
    let src = usize::from(op & 7);
    let rp = usize::from(op >> 4 & 3);
    let n = |m: &str, o: &str, len: u32| (m.to_string(), o.to_string(), len, Flow::Normal, None);
    let (mnemonic, operand, len, flow, target) = match op {
        0x00 => n("NOP", "", 1),
        0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 | 0x38 => n("*NOP", "", 1),
        0x01 | 0x11 | 0x21 | 0x31 => n("LXI", &format!("{},{a16}", PAIR[rp]), 3),
        0x02 | 0x12 => n("STAX", PAIR[rp], 1),
        0x0a | 0x1a => n("LDAX", PAIR[rp], 1),
        0x03 | 0x13 | 0x23 | 0x33 => n("INX", PAIR[rp], 1),
        0x0b | 0x1b | 0x2b | 0x3b => n("DCX", PAIR[rp], 1),
        0x09 | 0x19 | 0x29 | 0x39 => n("DAD", PAIR[rp], 1),
        0x07 => n("RLC", "", 1),
        0x0f => n("RRC", "", 1),
        0x17 => n("RAL", "", 1),
        0x1f => n("RAR", "", 1),
        0x22 => n("SHLD", &a16, 3),
        0x2a => n("LHLD", &a16, 3),
        0x32 => n("STA", &a16, 3),
        0x3a => n("LDA", &a16, 3),
        0x27 => n("DAA", "", 1),
        0x2f => n("CMA", "", 1),
        0x37 => n("STC", "", 1),
        0x3f => n("CMC", "", 1),
        _ if op < 0x40 => match op & 7 {
            4 => n("INR", REG[dst], 1),
            5 => n("DCR", REG[dst], 1),
            _ => n("MVI", &format!("{},{d8}", REG[dst]), 2),
        },
        0x76 => n("HLT", "", 1),
        0x40..=0x7f => n("MOV", &format!("{},{}", REG[dst], REG[src]), 1),
        0x80..=0xbf => n(ALU[dst], REG[src], 1),
        0xc1 | 0xd1 | 0xe1 => n("POP", PAIR[rp], 1),
        0xc5 | 0xd5 | 0xe5 => n("PUSH", PAIR[rp], 1),
        0xf1 => n("POP", "PSW", 1),
        0xf5 => n("PUSH", "PSW", 1),
        0xc3 | 0xcb => ("JMP".into(), a16, 3, Flow::Jump, jump),
        0xc9 | 0xd9 => ("RET".into(), String::new(), 1, Flow::Return, None),
        0xcd | 0xdd | 0xed | 0xfd => ("CALL".into(), a16, 3, Flow::Call, jump),
        0xd3 => n("OUT", &d8, 2),
        0xdb => n("IN", &d8, 2),
        0xe3 => n("XTHL", "", 1),
        0xe9 => ("PCHL".into(), String::new(), 1, Flow::Jump, None),
        0xeb => n("XCHG", "", 1),
        0xf3 => n("DI", "", 1),
        0xf9 => n("SPHL", "", 1),
        0xfb => n("EI", "", 1),
        _ => match op & 7 {
            0 => (
                format!("R{}", COND[dst]),
                String::new(),
                1,
                Flow::Return,
                None,
            ),
            2 => (format!("J{}", COND[dst]), a16, 3, Flow::Branch, jump),
            4 => (format!("C{}", COND[dst]), a16, 3, Flow::Call, jump),
            6 => n(ALU_IMM[dst], &d8, 2),
            _ => (
                "RST".into(),
                dst.to_string(),
                1,
                Flow::Call,
                Some(u32::from(op & 0x38)),
            ),
        },
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
