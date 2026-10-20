//! Z80 disassembler (Zilog mnemonics, hex operands with `$`), decoded by
//! the same opcode fields as the executor.

use emu_core::{Bus, Flow, Instr, Size};

const R: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RP: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RP2: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC: [&str; 8] = ["NZ", "Z", "NC", "C", "PO", "PE", "P", "M"];
const ALU: [(&str, &str); 8] = [
    ("ADD", "A,"),
    ("ADC", "A,"),
    ("SUB", ""),
    ("SBC", "A,"),
    ("AND", ""),
    ("XOR", ""),
    ("OR", ""),
    ("CP", ""),
];
const ROT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SLL", "SRL"];
const ACC: [&str; 8] = ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"];
const BLOCK: [[&str; 4]; 4] = [
    ["LDI", "CPI", "INI", "OUTI"],
    ["LDD", "CPD", "IND", "OUTD"],
    ["LDIR", "CPIR", "INIR", "OTIR"],
    ["LDDR", "CPDR", "INDR", "OTDR"],
];

struct Reader<'a> {
    bus: &'a dyn Bus,
    addr: u16,
    bytes: Vec<u8>,
}

impl Reader<'_> {
    fn byte(&mut self) -> u8 {
        let a = self.addr.wrapping_add(self.bytes.len() as u16);
        let v = self.bus.peek(u32::from(a), Size::Byte) as u8;
        self.bytes.push(v);
        v
    }

    fn word(&mut self) -> u16 {
        let lo = self.byte();
        let hi = self.byte();
        u16::from_le_bytes([lo, hi])
    }

    /// Address after the bytes read so far.
    fn next(&self) -> u16 {
        self.addr.wrapping_add(self.bytes.len() as u16)
    }
}

/// Text of what the instruction does, its branch target and flow.
struct Decoded {
    mnemonic: String,
    operand: String,
    target: Option<u16>,
    flow: Flow,
}

fn op(mnemonic: &str, operand: impl Into<String>) -> Decoded {
    Decoded {
        mnemonic: mnemonic.into(),
        operand: operand.into(),
        target: None,
        flow: Flow::Normal,
    }
}

fn flow(mut d: Decoded, flow: Flow, target: Option<u16>) -> Decoded {
    d.flow = flow;
    d.target = target;
    d
}

/// Register names with the index register `ix` ("HL", "IX" or "IY").
struct Names {
    ix: &'static str,
    /// `(IX+d)` text once the displacement is known.
    mem: String,
}

impl Names {
    fn r(&self, i: u8) -> String {
        match (i, self.ix) {
            (6, _) => self.mem.clone(),
            (4, "IX") => "IXH".into(),
            (5, "IX") => "IXL".into(),
            (4, "IY") => "IYH".into(),
            (5, "IY") => "IYL".into(),
            _ => R[usize::from(i)].into(),
        }
    }

    fn rp(&self, p: u8) -> &'static str {
        if p == 2 {
            self.ix
        } else {
            RP[usize::from(p)]
        }
    }

    fn rp2(&self, p: u8) -> &'static str {
        if p == 2 {
            self.ix
        } else {
            RP2[usize::from(p)]
        }
    }
}

fn displacement(rd: &mut Reader<'_>, ix: &str) -> String {
    let d = rd.byte() as i8;
    if d < 0 {
        format!("({ix}-${:02X})", -i16::from(d))
    } else {
        format!("({ix}+${d:02X})")
    }
}

fn relative(rd: &mut Reader<'_>) -> u16 {
    let d = rd.byte() as i8;
    rd.next().wrapping_add(d as u16)
}

fn main(rd: &mut Reader<'_>, opcode: u8, ix: &'static str) -> Decoded {
    let (x, y, z) = (opcode >> 6, (opcode >> 3) & 7, opcode & 7);
    let (p, q) = (y >> 1, y & 1);
    let mut n = Names {
        ix,
        mem: "(HL)".into(),
    };
    // The displacement of (IX+d) comes right after the opcode.
    let uses_mem = match x {
        0 => (z == 4 || z == 5 || z == 6) && y == 6,
        1 => (y == 6) != (z == 6),
        2 => z == 6,
        _ => false,
    };
    if ix != "HL" && uses_mem {
        n.mem = displacement(rd, ix);
    }
    // With (IX+d) the other operand is the real H or L.
    let plain = Names {
        ix: "HL",
        mem: n.mem.clone(),
    };
    match x {
        0 => match z {
            0 => match y {
                0 => op("NOP", ""),
                1 => op("EX", "AF,AF'"),
                2 => {
                    let t = relative(rd);
                    flow(op("DJNZ", format!("${t:04X}")), Flow::Branch, Some(t))
                }
                3 => {
                    let t = relative(rd);
                    flow(op("JR", format!("${t:04X}")), Flow::Jump, Some(t))
                }
                _ => {
                    let t = relative(rd);
                    let cc = CC[usize::from(y - 4)];
                    flow(op("JR", format!("{cc},${t:04X}")), Flow::Branch, Some(t))
                }
            },
            1 if q == 0 => {
                let nn = rd.word();
                op("LD", format!("{},${nn:04X}", n.rp(p)))
            }
            1 => op("ADD", format!("{},{}", n.rp(2), n.rp(p))),
            2 => match (q, p) {
                (0, 0) => op("LD", "(BC),A"),
                (0, 1) => op("LD", "(DE),A"),
                (0, 2) => {
                    let nn = rd.word();
                    op("LD", format!("(${nn:04X}),{}", n.rp(2)))
                }
                (0, _) => {
                    let nn = rd.word();
                    op("LD", format!("(${nn:04X}),A"))
                }
                (_, 0) => op("LD", "A,(BC)"),
                (_, 1) => op("LD", "A,(DE)"),
                (_, 2) => {
                    let nn = rd.word();
                    op("LD", format!("{},(${nn:04X})", n.rp(2)))
                }
                (_, _) => {
                    let nn = rd.word();
                    op("LD", format!("A,(${nn:04X})"))
                }
            },
            3 => op(if q == 0 { "INC" } else { "DEC" }, n.rp(p)),
            4 => op("INC", n.r(y)),
            5 => op("DEC", n.r(y)),
            6 => {
                let v = rd.byte();
                op("LD", format!("{},${v:02X}", n.r(y)))
            }
            _ => op(ACC[usize::from(y)], ""),
        },
        1 if y == 6 && z == 6 => op("HALT", ""),
        1 if y == 6 => op("LD", format!("{},{}", n.mem, plain.r(z))),
        1 if z == 6 => op("LD", format!("{},{}", plain.r(y), n.mem)),
        1 => op("LD", format!("{},{}", n.r(y), n.r(z))),
        2 => {
            let (m, prefix) = ALU[usize::from(y)];
            op(m, format!("{prefix}{}", n.r(z)))
        }
        _ => match z {
            0 => flow(op("RET", CC[usize::from(y)]), Flow::Return, None),
            1 if q == 0 => op("POP", n.rp2(p)),
            1 => match p {
                0 => flow(op("RET", ""), Flow::Return, None),
                1 => op("EXX", ""),
                2 => flow(op("JP", format!("({})", n.rp(2))), Flow::Jump, None),
                _ => op("LD", format!("SP,{}", n.rp(2))),
            },
            2 => {
                let nn = rd.word();
                let cc = CC[usize::from(y)];
                flow(op("JP", format!("{cc},${nn:04X}")), Flow::Branch, Some(nn))
            }
            3 => match y {
                0 => {
                    let nn = rd.word();
                    flow(op("JP", format!("${nn:04X}")), Flow::Jump, Some(nn))
                }
                1 => cb(rd, ix),
                2 => {
                    let v = rd.byte();
                    op("OUT", format!("(${v:02X}),A"))
                }
                3 => {
                    let v = rd.byte();
                    op("IN", format!("A,(${v:02X})"))
                }
                4 => op("EX", format!("(SP),{}", n.rp(2))),
                5 => op("EX", "DE,HL"),
                6 => op("DI", ""),
                _ => op("EI", ""),
            },
            4 => {
                let nn = rd.word();
                let cc = CC[usize::from(y)];
                flow(op("CALL", format!("{cc},${nn:04X}")), Flow::Call, Some(nn))
            }
            5 if q == 0 => op("PUSH", n.rp2(p)),
            5 => match p {
                0 => {
                    let nn = rd.word();
                    flow(op("CALL", format!("${nn:04X}")), Flow::Call, Some(nn))
                }
                2 => {
                    let o = rd.byte();
                    ed(rd, o)
                }
                _ if ix != "HL" => op("NOP", "; prefix"),
                _ => {
                    let o = rd.byte();
                    let next = if p == 1 { "IX" } else { "IY" };
                    if matches!(o, 0xdd | 0xfd | 0xed) {
                        rd.bytes.pop();
                        op("NOP", "; prefix")
                    } else {
                        main(rd, o, next)
                    }
                }
            },
            6 => {
                let v = rd.byte();
                let (m, prefix) = ALU[usize::from(y)];
                op(m, format!("{prefix}${v:02X}"))
            }
            _ => {
                let t = u16::from(y) * 8;
                flow(op("RST", format!("${t:02X}")), Flow::Call, Some(t))
            }
        },
    }
}

fn cb(rd: &mut Reader<'_>, ix: &'static str) -> Decoded {
    let (mem, o) = if ix == "HL" {
        ("(HL)".to_string(), rd.byte())
    } else {
        let m = displacement(rd, ix);
        (m, rd.byte())
    };
    let (x, y, z) = (o >> 6, (o >> 3) & 7, o & 7);
    let target = if z == 6 || ix != "HL" {
        mem.clone()
    } else {
        R[usize::from(z)].to_string()
    };
    // DD CB with a register field also copies the result there.
    let copy = if ix != "HL" && z != 6 && x != 1 {
        format!(",{}", R[usize::from(z)])
    } else {
        String::new()
    };
    match x {
        0 => op(ROT[usize::from(y)], format!("{target}{copy}")),
        1 => op("BIT", format!("{y},{target}")),
        2 => op("RES", format!("{y},{target}{copy}")),
        _ => op("SET", format!("{y},{target}{copy}")),
    }
}

fn ed(rd: &mut Reader<'_>, o: u8) -> Decoded {
    let (x, y, z) = (o >> 6, (o >> 3) & 7, o & 7);
    let (p, q) = (y >> 1, y & 1);
    if x == 2 && z <= 3 && y >= 4 {
        return op(BLOCK[usize::from(y - 4)][usize::from(z)], "");
    }
    if x != 1 {
        return op("NOP", "; ED");
    }
    match z {
        0 if y == 6 => op("IN", "(C)"),
        0 => op("IN", format!("{},(C)", R[usize::from(y)])),
        1 if y == 6 => op("OUT", "(C),0"),
        1 => op("OUT", format!("(C),{}", R[usize::from(y)])),
        2 => op(
            if q == 0 { "SBC" } else { "ADC" },
            format!("HL,{}", RP[usize::from(p)]),
        ),
        3 => {
            let nn = rd.word();
            if q == 0 {
                op("LD", format!("(${nn:04X}),{}", RP[usize::from(p)]))
            } else {
                op("LD", format!("{},(${nn:04X})", RP[usize::from(p)]))
            }
        }
        4 => op("NEG", ""),
        5 => flow(
            op(if y == 1 { "RETI" } else { "RETN" }, ""),
            Flow::Return,
            None,
        ),
        6 => op("IM", ["0", "0", "1", "2"][usize::from(y & 3)]),
        _ => match y {
            0 => op("LD", "I,A"),
            1 => op("LD", "R,A"),
            2 => op("LD", "A,I"),
            3 => op("LD", "A,R"),
            4 => op("RRD", ""),
            5 => op("RLD", ""),
            _ => op("NOP", "; ED"),
        },
    }
}

pub fn disassemble(bus: &dyn Bus, addr: u16) -> Instr {
    let mut rd = Reader {
        bus,
        addr,
        bytes: Vec::new(),
    };
    let opcode = rd.byte();
    let d = main(&mut rd, opcode, "HL");
    Instr {
        addr: u32::from(addr),
        bytes: rd.bytes,
        mnemonic: d.mnemonic,
        operand: d.operand,
        target: d.target.map(u32::from),
        flow: d.flow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::testing::FlatBus;

    fn dis(code: &[u8]) -> (String, usize) {
        let mut bus = FlatBus::new(16);
        for (i, &b) in code.iter().enumerate() {
            bus.write(0x100 + i as u32, Size::Byte, u32::from(b));
        }
        let i = disassemble(&bus, 0x100);
        (format!("{i}"), i.bytes.len())
    }

    #[test]
    fn plain_prefixed_and_indexed() {
        assert_eq!(dis(&[0x00]), ("NOP".into(), 1));
        assert_eq!(dis(&[0x3e, 0x12]), ("LD A,$12".into(), 2));
        assert_eq!(dis(&[0x21, 0x34, 0x12]), ("LD HL,$1234".into(), 3));
        assert_eq!(dis(&[0xcb, 0x46]), ("BIT 0,(HL)".into(), 2));
        assert_eq!(dis(&[0xed, 0xb0]), ("LDIR".into(), 2));
        assert_eq!(dis(&[0xed, 0x4b, 0x00, 0x80]), ("LD BC,($8000)".into(), 4));
        assert_eq!(dis(&[0xdd, 0x7e, 0xfe]), ("LD A,(IX-$02)".into(), 3));
        assert_eq!(
            dis(&[0xfd, 0x36, 0x05, 0x99]),
            ("LD (IY+$05),$99".into(), 4)
        );
        assert_eq!(dis(&[0xdd, 0x66, 0x01]), ("LD H,(IX+$01)".into(), 3));
        assert_eq!(dis(&[0xdd, 0x26, 0x07]), ("LD IXH,$07".into(), 3));
        assert_eq!(dis(&[0xdd, 0xcb, 0x03, 0x06]), ("RLC (IX+$03)".into(), 4));
        assert_eq!(
            dis(&[0xfd, 0xcb, 0x03, 0xc0]),
            ("SET 0,(IY+$03),B".into(), 4)
        );
        assert_eq!(dis(&[0xdd, 0xe9]), ("JP (IX)".into(), 2));
        assert_eq!(dis(&[0xdd, 0xdd]), ("NOP ; prefix".into(), 1));
    }

    #[test]
    fn branch_targets_and_flow() {
        let mut bus = FlatBus::new(16);
        for (i, &b) in [0x18u8, 0xfe].iter().enumerate() {
            bus.write(0x200 + i as u32, Size::Byte, u32::from(b));
        }
        let i = disassemble(&bus, 0x200);
        assert_eq!((i.target, i.flow), (Some(0x200), Flow::Jump));
        bus.write(0x300, Size::Byte, 0xcd);
        bus.write(0x301, Size::Byte, 0x34);
        bus.write(0x302, Size::Byte, 0x12);
        let i = disassemble(&bus, 0x300);
        assert_eq!((i.target, i.flow), (Some(0x1234), Flow::Call));
        bus.write(0x400, Size::Byte, 0xc9);
        assert_eq!(disassemble(&bus, 0x400).flow, Flow::Return);
        bus.write(0x500, Size::Byte, 0x10);
        bus.write(0x501, Size::Byte, 0x02);
        assert_eq!(disassemble(&bus, 0x500).flow, Flow::Branch);
    }
}
