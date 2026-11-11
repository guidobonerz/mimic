//! Disassembly with the same prefix rules as execution: `42 42 op` is a Q
//! instruction where `op` has a Q form, `EA op` with a `(zp),Z` opcode is
//! flat (`[$nn],Z`), `42 42 EA op` both.

use emu_core::{Flow, Instr};

use crate::exec::{is_izz, Q_OPS};
use crate::table::{Mn, OPS};
use crate::Mode;

/// The Q mnemonic of an 8-bit one.
fn q_name(mn: Mn) -> &'static str {
    match mn {
        Mn::Lda => "LDQ",
        Mn::Sta => "STQ",
        Mn::Adc => "ADCQ",
        Mn::Sbc => "SBCQ",
        Mn::Cmp => "CMPQ",
        Mn::Eor => "EORQ",
        Mn::And => "ANDQ",
        Mn::Ora => "ORQ",
        Mn::Bit => "BITQ",
        Mn::Asl => "ASLQ",
        Mn::Asr => "ASRQ",
        Mn::Lsr => "LSRQ",
        Mn::Rol => "ROLQ",
        Mn::Ror => "RORQ",
        Mn::Inc => "INQ",
        Mn::Dec => "DEQ",
        other => other.name(),
    }
}

pub fn disassemble(peek: &dyn Fn(u16) -> u8, addr: u16) -> Instr {
    let mut pc = addr;
    let mut next = || {
        let v = peek(pc);
        pc = pc.wrapping_add(1);
        v
    };
    let mut code = next();
    let (mut q, mut flat) = (false, false);
    if code == 0x42 && peek(addr.wrapping_add(1)) == 0x42 {
        let third = peek(addr.wrapping_add(2));
        if third == 0xea && is_izz(peek(addr.wrapping_add(3))) {
            next();
            next();
            code = next();
            q = true;
            flat = true;
        } else if Q_OPS[usize::from(third)] {
            next();
            code = next();
            q = true;
        }
    } else if code == 0xea && is_izz(peek(addr.wrapping_add(1))) {
        code = next();
        flat = true;
    }
    let op = OPS[usize::from(code)];
    let mnemonic = if q { q_name(op.mn) } else { op.mn.name() };
    let byte = |n: &mut dyn FnMut() -> u8| n();
    let mut target = None;
    let operand = match op.mode {
        Mode::Imp => String::new(),
        Mode::Acc => {
            if q {
                String::new()
            } else {
                "A".into()
            }
        }
        Mode::Imm => format!("#${:02X}", byte(&mut next)),
        Mode::Imm16 => {
            let lo = next();
            let hi = next();
            format!("#${:04X}", u16::from_le_bytes([lo, hi]))
        }
        Mode::Zp => format!("${:02X}", next()),
        Mode::Zpx => format!("${:02X},X", next()),
        Mode::Zpy => format!("${:02X},Y", next()),
        Mode::Abs | Mode::Absx | Mode::Absy | Mode::Ind | Mode::Iax => {
            let lo = next();
            let hi = next();
            let a = u16::from_le_bytes([lo, hi]);
            if matches!(op.mn, Mn::Jmp | Mn::Jsr) && op.mode == Mode::Abs {
                target = Some(u32::from(a));
            }
            match op.mode {
                Mode::Abs => format!("${a:04X}"),
                Mode::Absx => format!("${a:04X},X"),
                Mode::Absy => format!("${a:04X},Y"),
                Mode::Ind => format!("(${a:04X})"),
                _ => format!("(${a:04X},X)"),
            }
        }
        Mode::Izx => format!("(${:02X},X)", next()),
        Mode::Izy => format!("(${:02X}),Y", next()),
        Mode::Isy => format!("(${:02X},SP),Y", next()),
        Mode::Izz => {
            let n = next();
            let with_z = !q || op.mn == Mn::Lda;
            match (flat, with_z) {
                (true, true) => format!("[${n:02X}],Z"),
                (true, false) => format!("[${n:02X}]"),
                (false, true) => format!("(${n:02X}),Z"),
                (false, false) => format!("(${n:02X})"),
            }
        }
        Mode::Rel => {
            let off = next() as i8;
            let t = pc.wrapping_add(off as u16);
            target = Some(u32::from(t));
            format!("${t:04X}")
        }
        Mode::Rel16 => {
            let lo = next();
            let hi = next();
            let t = pc
                .wrapping_sub(1)
                .wrapping_add(u16::from_le_bytes([lo, hi]));
            target = Some(u32::from(t));
            format!("${t:04X}")
        }
        Mode::Zrel => {
            let n = next();
            let off = next() as i8;
            let t = pc.wrapping_add(off as u16);
            target = Some(u32::from(t));
            format!("${n:02X},${t:04X}")
        }
    };
    let flow = match op.mn {
        Mn::Jsr | Mn::Bsr => Flow::Call,
        Mn::Rts | Mn::Rtn | Mn::Rti => Flow::Return,
        Mn::Jmp | Mn::Bra => Flow::Jump,
        _ if matches!(op.mode, Mode::Rel | Mode::Rel16 | Mode::Zrel) => Flow::Branch,
        _ => Flow::Normal,
    };
    let len = pc.wrapping_sub(addr);
    Instr {
        addr: u32::from(addr),
        bytes: (0..len).map(|i| peek(addr.wrapping_add(i))).collect(),
        mnemonic: mnemonic.to_string(),
        operand,
        target,
        flow,
    }
}
