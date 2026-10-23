//! Disassembler (Motorola syntax). Reads with `M6809Bus::peek` only.

use emu_core::{Flow, Instr};

use crate::table::{self, Mode};
use crate::M6809Bus;

const INDEX_REGS: [&str; 4] = ["X", "Y", "U", "S"];
const PAIR_REGS: [&str; 16] = [
    "D", "X", "Y", "U", "S", "PC", "?", "?", "A", "B", "CC", "DP", "?", "?", "?", "?",
];

fn hex(v: i32) -> String {
    if v < 0 {
        format!("-${:X}", -v)
    } else {
        format!("${v:X}")
    }
}

/// Disassemble the instruction at `addr`.
pub fn disassemble(bus: &dyn M6809Bus, addr: u16) -> Instr {
    let mut pc = addr;
    let mut bytes = Vec::new();
    let next = |pc: &mut u16, bytes: &mut Vec<u8>| {
        let v = bus.peek(*pc);
        *pc = pc.wrapping_add(1);
        bytes.push(v);
        v
    };
    let mut page = 0;
    let mut op = next(&mut pc, &mut bytes);
    while op == 0x10 || op == 0x11 {
        page = if op == 0x10 { 2 } else { 3 };
        op = next(&mut pc, &mut bytes);
    }
    let info = match page {
        2 => table::page2(op),
        3 => table::page3(op),
        _ => table::page0(op),
    };
    let Some((mode, _)) = info else {
        return Instr {
            addr: u32::from(addr),
            bytes,
            mnemonic: if page == 0 {
                table::name(0, op)
            } else {
                "???".into()
            },
            operand: String::new(),
            target: None,
            flow: Flow::Normal,
        };
    };
    let mnemonic = table::name(page, op);
    let mut target = None;
    let operand = match mode {
        Mode::Inh => String::new(),
        Mode::Imm8 => format!("#${:02X}", next(&mut pc, &mut bytes)),
        Mode::Imm16 => {
            let hi = next(&mut pc, &mut bytes);
            let lo = next(&mut pc, &mut bytes);
            format!("#${:04X}", u16::from_be_bytes([hi, lo]))
        }
        Mode::Dir => {
            let lo = next(&mut pc, &mut bytes);
            format!("<${lo:02X}")
        }
        Mode::Ext => {
            let hi = next(&mut pc, &mut bytes);
            let lo = next(&mut pc, &mut bytes);
            let a = u16::from_be_bytes([hi, lo]);
            if mnemonic == "JMP" || mnemonic == "JSR" {
                target = Some(u32::from(a));
            }
            format!("${a:04X}")
        }
        Mode::Rel8 => {
            let off = next(&mut pc, &mut bytes) as i8;
            let t = pc.wrapping_add(off as u16);
            target = Some(u32::from(t));
            format!("${t:04X}")
        }
        Mode::Rel16 => {
            let hi = next(&mut pc, &mut bytes);
            let lo = next(&mut pc, &mut bytes);
            let t = pc.wrapping_add(u16::from_be_bytes([hi, lo]));
            target = Some(u32::from(t));
            format!("${t:04X}")
        }
        Mode::StackS | Mode::StackU => {
            let mask = next(&mut pc, &mut bytes);
            let other = if mode == Mode::StackS { "U" } else { "S" };
            let names = ["CC", "A", "B", "DP", "X", "Y", other, "PC"];
            (0..8)
                .filter(|b| mask & (1 << b) != 0)
                .map(|b| names[b])
                .collect::<Vec<_>>()
                .join(",")
        }
        Mode::Pair => {
            let p = next(&mut pc, &mut bytes);
            format!(
                "{},{}",
                PAIR_REGS[usize::from(p >> 4)],
                PAIR_REGS[usize::from(p & 15)]
            )
        }
        Mode::Idx => {
            let post = next(&mut pc, &mut bytes);
            let r = INDEX_REGS[usize::from((post >> 5) & 3)];
            if post & 0x80 == 0 {
                let off = i32::from(((post & 0x1f) << 3) as i8 >> 3);
                format!("{},{r}", hex(off))
            } else {
                let inner = match post & 15 {
                    0 => format!(",{r}+"),
                    1 => format!(",{r}++"),
                    2 => format!(",-{r}"),
                    3 => format!(",--{r}"),
                    4 => format!(",{r}"),
                    5 => format!("B,{r}"),
                    6 => format!("A,{r}"),
                    8 => {
                        let off = next(&mut pc, &mut bytes) as i8;
                        format!("{},{r}", hex(i32::from(off)))
                    }
                    9 => {
                        let hi = next(&mut pc, &mut bytes);
                        let lo = next(&mut pc, &mut bytes);
                        format!("{},{r}", hex(i32::from(i16::from_be_bytes([hi, lo]))))
                    }
                    11 => format!("D,{r}"),
                    12 => {
                        let off = next(&mut pc, &mut bytes) as i8;
                        format!("${:04X},PCR", pc.wrapping_add(off as u16))
                    }
                    13 => {
                        let hi = next(&mut pc, &mut bytes);
                        let lo = next(&mut pc, &mut bytes);
                        let t = pc.wrapping_add(u16::from_be_bytes([hi, lo]));
                        format!("${t:04X},PCR")
                    }
                    15 => {
                        let hi = next(&mut pc, &mut bytes);
                        let lo = next(&mut pc, &mut bytes);
                        format!("${:04X}", u16::from_be_bytes([hi, lo]))
                    }
                    _ => "?".to_string(),
                };
                if post & 0x10 != 0 {
                    format!("[{inner}]")
                } else {
                    inner
                }
            }
        }
    };
    let flow = match mnemonic.as_str() {
        "JMP" | "BRA" | "LBRA" => Flow::Jump,
        "JSR" | "BSR" | "LBSR" => Flow::Call,
        "RTS" | "RTI" => Flow::Return,
        "PULS" | "PULU" if operand.contains("PC") => Flow::Return,
        m if (mode == Mode::Rel8 || mode == Mode::Rel16) && m != "BRN" && m != "LBRN" => {
            Flow::Branch
        }
        _ => Flow::Normal,
    };
    Instr {
        addr: u32::from(addr),
        bytes,
        mnemonic,
        operand,
        target,
        flow,
    }
}
