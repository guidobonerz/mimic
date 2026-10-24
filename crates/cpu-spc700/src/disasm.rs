//! Disassembler (Sony syntax). Reads with `SpcBus::peek` only.

use emu_core::{Flow, Instr};

use crate::SpcBus;

/// Operand templates: `d` direct-page byte, `a` absolute word, `i`
/// immediate, `r` relative branch, `t` second direct-page byte of a
/// two-address instruction, `m` memory bit (13-bit address, 3-bit bit).
const OPS: [&str; 256] = [
    "NOP",
    "TCALL 0",
    "SET1 d.0",
    "BBS d.0,r",
    "OR A,d",
    "OR A,!a",
    "OR A,(X)",
    "OR A,[d+X]",
    "OR A,#i",
    "OR t,d",
    "OR1 C,m",
    "ASL d",
    "ASL !a",
    "PUSH PSW",
    "TSET1 !a",
    "BRK", //
    "BPL r",
    "TCALL 1",
    "CLR1 d.0",
    "BBC d.0,r",
    "OR A,d+X",
    "OR A,!a+X",
    "OR A,!a+Y",
    "OR A,[d]+Y",
    "OR t,#i",
    "OR (X),(Y)",
    "DECW d",
    "ASL d+X",
    "ASL A",
    "DEC X",
    "CMP X,!a",
    "JMP [!a+X]", //
    "CLRP",
    "TCALL 2",
    "SET1 d.1",
    "BBS d.1,r",
    "AND A,d",
    "AND A,!a",
    "AND A,(X)",
    "AND A,[d+X]",
    "AND A,#i",
    "AND t,d",
    "OR1 C,/m",
    "ROL d",
    "ROL !a",
    "PUSH A",
    "CBNE d,r",
    "BRA r", //
    "BMI r",
    "TCALL 3",
    "CLR1 d.1",
    "BBC d.1,r",
    "AND A,d+X",
    "AND A,!a+X",
    "AND A,!a+Y",
    "AND A,[d]+Y",
    "AND t,#i",
    "AND (X),(Y)",
    "INCW d",
    "ROL d+X",
    "ROL A",
    "INC X",
    "CMP X,d",
    "CALL !a", //
    "SETP",
    "TCALL 4",
    "SET1 d.2",
    "BBS d.2,r",
    "EOR A,d",
    "EOR A,!a",
    "EOR A,(X)",
    "EOR A,[d+X]",
    "EOR A,#i",
    "EOR t,d",
    "AND1 C,m",
    "LSR d",
    "LSR !a",
    "PUSH X",
    "TCLR1 !a",
    "PCALL i", //
    "BVC r",
    "TCALL 5",
    "CLR1 d.2",
    "BBC d.2,r",
    "EOR A,d+X",
    "EOR A,!a+X",
    "EOR A,!a+Y",
    "EOR A,[d]+Y",
    "EOR t,#i",
    "EOR (X),(Y)",
    "CMPW YA,d",
    "LSR d+X",
    "LSR A",
    "MOV X,A",
    "CMP Y,!a",
    "JMP !a", //
    "CLRC",
    "TCALL 6",
    "SET1 d.3",
    "BBS d.3,r",
    "CMP A,d",
    "CMP A,!a",
    "CMP A,(X)",
    "CMP A,[d+X]",
    "CMP A,#i",
    "CMP t,d",
    "AND1 C,/m",
    "ROR d",
    "ROR !a",
    "PUSH Y",
    "DBNZ d,r",
    "RET", //
    "BVS r",
    "TCALL 7",
    "CLR1 d.3",
    "BBC d.3,r",
    "CMP A,d+X",
    "CMP A,!a+X",
    "CMP A,!a+Y",
    "CMP A,[d]+Y",
    "CMP t,#i",
    "CMP (X),(Y)",
    "ADDW YA,d",
    "ROR d+X",
    "ROR A",
    "MOV A,X",
    "CMP Y,d",
    "RETI", //
    "SETC",
    "TCALL 8",
    "SET1 d.4",
    "BBS d.4,r",
    "ADC A,d",
    "ADC A,!a",
    "ADC A,(X)",
    "ADC A,[d+X]",
    "ADC A,#i",
    "ADC t,d",
    "EOR1 C,m",
    "DEC d",
    "DEC !a",
    "MOV Y,#i",
    "POP PSW",
    "MOV t,#i", //
    "BCC r",
    "TCALL 9",
    "CLR1 d.4",
    "BBC d.4,r",
    "ADC A,d+X",
    "ADC A,!a+X",
    "ADC A,!a+Y",
    "ADC A,[d]+Y",
    "ADC t,#i",
    "ADC (X),(Y)",
    "SUBW YA,d",
    "DEC d+X",
    "DEC A",
    "MOV X,SP",
    "DIV YA,X",
    "XCN A", //
    "EI",
    "TCALL 10",
    "SET1 d.5",
    "BBS d.5,r",
    "SBC A,d",
    "SBC A,!a",
    "SBC A,(X)",
    "SBC A,[d+X]",
    "SBC A,#i",
    "SBC t,d",
    "MOV1 C,m",
    "INC d",
    "INC !a",
    "CMP Y,#i",
    "POP A",
    "MOV (X)+,A", //
    "BCS r",
    "TCALL 11",
    "CLR1 d.5",
    "BBC d.5,r",
    "SBC A,d+X",
    "SBC A,!a+X",
    "SBC A,!a+Y",
    "SBC A,[d]+Y",
    "SBC t,#i",
    "SBC (X),(Y)",
    "MOVW YA,d",
    "INC d+X",
    "INC A",
    "MOV SP,X",
    "DAS A",
    "MOV A,(X)+", //
    "DI",
    "TCALL 12",
    "SET1 d.6",
    "BBS d.6,r",
    "MOV d,A",
    "MOV !a,A",
    "MOV (X),A",
    "MOV [d+X],A",
    "CMP X,#i",
    "MOV !a,X",
    "MOV1 m,C",
    "MOV d,Y",
    "MOV !a,Y",
    "MOV X,#i",
    "POP X",
    "MUL YA", //
    "BNE r",
    "TCALL 13",
    "CLR1 d.6",
    "BBC d.6,r",
    "MOV d+X,A",
    "MOV !a+X,A",
    "MOV !a+Y,A",
    "MOV [d]+Y,A",
    "MOV d,X",
    "MOV d+Y,X",
    "MOVW d,YA",
    "MOV d+X,Y",
    "DEC Y",
    "MOV A,Y",
    "CBNE d+X,r",
    "DAA A", //
    "CLRV",
    "TCALL 14",
    "SET1 d.7",
    "BBS d.7,r",
    "MOV A,d",
    "MOV A,!a",
    "MOV A,(X)",
    "MOV A,[d+X]",
    "MOV A,#i",
    "MOV X,!a",
    "NOT1 m",
    "MOV Y,d",
    "MOV Y,!a",
    "NOTC",
    "POP Y",
    "SLEEP", //
    "BEQ r",
    "TCALL 15",
    "CLR1 d.7",
    "BBC d.7,r",
    "MOV A,d+X",
    "MOV A,!a+X",
    "MOV A,!a+Y",
    "MOV A,[d]+Y",
    "MOV X,d",
    "MOV X,d+Y",
    "MOV t,d",
    "MOV Y,d+X",
    "INC Y",
    // NOTE: wrong text. $FD is `MOV Y,A` ($DD is `MOV A,Y`); found while
    // porting this table to the raw assembler. Display only, execution is
    // unaffected.
    "MOV A,Y",
    "DBNZ Y,r",
    "STOP", //
];

/// Disassemble the instruction at `addr`.
pub fn disassemble(bus: &dyn SpcBus, addr: u16) -> Instr {
    let at = |n: u16| bus.peek(addr.wrapping_add(n));
    let op = at(0);
    let text = OPS[usize::from(op)];
    let (mnemonic, template) = text.split_once(' ').unwrap_or((text, ""));
    let mut target = None;
    let (operand, len) = match template.split_once(',') {
        // Two-address forms store the source byte first.
        Some(("t", "d")) => (format!("${:02X},${:02X}", at(2), at(1)), 3),
        Some(("t", "#i")) => (format!("${:02X},#${:02X}", at(2), at(1)), 3),
        _ => {
            let mut operand = String::new();
            let mut n = 1u16;
            for c in template.chars() {
                match c {
                    'd' => {
                        operand.push_str(&format!("${:02X}", at(n)));
                        n += 1;
                    }
                    'a' => {
                        let w = u16::from_le_bytes([at(n), at(n + 1)]);
                        n += 2;
                        if matches!(op, 0x3f | 0x5f) {
                            target = Some(u32::from(w));
                        }
                        operand.push_str(&format!("${w:04X}"));
                    }
                    'i' => {
                        let b = at(n);
                        n += 1;
                        if op == 0x4f {
                            target = Some(0xff00 | u32::from(b));
                        }
                        operand.push_str(&format!("${b:02X}"));
                    }
                    'r' => {
                        let rel = at(n) as i8;
                        n += 1;
                        let t = addr.wrapping_add(n).wrapping_add(rel as u16);
                        target = Some(u32::from(t));
                        operand.push_str(&format!("${t:04X}"));
                    }
                    'm' => {
                        let w = u16::from_le_bytes([at(n), at(n + 1)]);
                        n += 2;
                        operand.push_str(&format!("${:04X}.{}", w & 0x1fff, w >> 13));
                    }
                    _ => operand.push(c),
                }
            }
            (operand, n)
        }
    };
    let flow = match op {
        0x3f | 0x4f | 0x0f => Flow::Call,
        _ if op & 0x0f == 0x01 => Flow::Call,
        0x6f | 0x7f => Flow::Return,
        0x5f | 0x1f | 0x2f => Flow::Jump,
        _ if template.ends_with('r') => Flow::Branch,
        _ => Flow::Normal,
    };
    Instr {
        addr: u32::from(addr),
        bytes: (0..len).map(at).collect(),
        mnemonic: mnemonic.into(),
        operand,
        target,
        flow,
    }
}
