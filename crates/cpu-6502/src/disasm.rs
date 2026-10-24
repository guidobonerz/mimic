//! Disassembler. Uses `Bus::peek` only (golden rule 2).

use emu_core::{Bus, Flow, Instr, Size};

use crate::table::{Kind, Mode, Op, CMOS_TABLE, TABLE};

fn peek(bus: &dyn Bus, addr: u16) -> u8 {
    bus.peek(u32::from(addr), Size::Byte) as u8
}

pub(crate) fn disassemble(bus: &dyn Bus, addr: u16, cmos: bool) -> Instr {
    let opcode = peek(bus, addr);
    let (op, mode) = if cmos {
        CMOS_TABLE[usize::from(opcode)]
    } else {
        TABLE[usize::from(opcode)]
    };
    let bytes: Vec<u8> = (0..mode.bytes())
        .map(|i| peek(bus, addr.wrapping_add(i)))
        .collect();
    let b1 = bytes.get(1).copied().unwrap_or(0);
    let word = u16::from_le_bytes([b1, bytes.get(2).copied().unwrap_or(0)]);
    let next = addr.wrapping_add(mode.bytes());

    let (operand, target) = match mode {
        Mode::Imp if op.kind() == Kind::Rmw => ("A".to_owned(), None),
        Mode::Imp => (String::new(), None),
        Mode::Imm => (format!("#${b1:02X}"), None),
        Mode::Zp => (format!("${b1:02X}"), None),
        Mode::ZpX => (format!("${b1:02X},X"), None),
        Mode::ZpY => (format!("${b1:02X},Y"), None),
        Mode::Abs => {
            let target = matches!(op.kind(), Kind::Special).then_some(u32::from(word));
            (format!("${word:04X}"), target)
        }
        Mode::AbsX => (format!("${word:04X},X"), None),
        Mode::AbsY => (format!("${word:04X},Y"), None),
        Mode::Ind => (format!("(${word:04X})"), None),
        Mode::IndX => (format!("(${b1:02X},X)"), None),
        Mode::IndY => (format!("(${b1:02X}),Y"), None),
        Mode::Zpi => (format!("(${b1:02X})"), None),
        Mode::AbsXInd => (format!("(${word:04X},X)"), None),
        Mode::Rel => {
            let dest = next.wrapping_add(b1 as i8 as u16);
            (format!("${dest:04X}"), Some(u32::from(dest)))
        }
    };

    let mnemonic = match op {
        Op::Nop1 | Op::NopAbs | Op::NopAbs3 => "NOP".to_owned(),
        _ => format!("{op:?}").to_ascii_uppercase(),
    };
    let flow = match op {
        Op::Jsr => Flow::Call,
        Op::Rts | Op::Rti => Flow::Return,
        Op::Jmp => Flow::Jump,
        _ if mode == Mode::Rel => Flow::Branch,
        // BRK enters the interrupt handler, which returns past its padding
        // byte: stepped into, not over.
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

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::testing::FlatBus;

    fn dis(program: &[u8]) -> Instr {
        let mut bus = FlatBus::new(16);
        bus.load(0x1000, program);
        disassemble(&bus, 0x1000, false)
    }

    #[test]
    fn cmos_opcodes() {
        let mut bus = FlatBus::new(16);
        bus.load(0x1000, &[0xb2, 0x10, 0x7c, 0x00, 0x20, 0x1a, 0x80, 0xfe]);
        let d = |a| disassemble(&bus, a, true).to_string();
        assert_eq!(d(0x1000), "LDA ($10)");
        assert_eq!(d(0x1002), "JMP ($2000,X)");
        assert_eq!(d(0x1005), "INC A");
        assert_eq!(d(0x1006), "BRA $1006");
    }

    #[test]
    fn formats_addressing_modes() {
        assert_eq!(dis(&[0xa9, 0x0f]).to_string(), "LDA #$0F");
        assert_eq!(dis(&[0xbd, 0x34, 0x12]).to_string(), "LDA $1234,X");
        assert_eq!(dis(&[0xb1, 0xfb]).to_string(), "LDA ($FB),Y");
        assert_eq!(dis(&[0x0a]).to_string(), "ASL A");
        assert_eq!(dis(&[0xea]).to_string(), "NOP");
        assert_eq!(dis(&[0x6c, 0xfc, 0xff]).to_string(), "JMP ($FFFC)");
        assert_eq!(dis(&[0xa7, 0x10]).to_string(), "LAX $10");
    }

    #[test]
    fn branch_and_jump_targets() {
        let bne = dis(&[0xd0, 0xfe]);
        assert_eq!(bne.to_string(), "BNE $1000");
        assert_eq!(bne.target, Some(0x1000));
        assert_eq!(bne.bytes, [0xd0, 0xfe]);
        assert_eq!(dis(&[0x20, 0xd2, 0xff]).target, Some(0xffd2));
        assert_eq!(dis(&[0xad, 0x20, 0xd0]).target, None);
    }

    #[test]
    fn control_flow_kinds() {
        assert_eq!(dis(&[0x20, 0xd2, 0xff]).flow, Flow::Call);
        assert_eq!(dis(&[0x60]).flow, Flow::Return);
        assert_eq!(dis(&[0x40]).flow, Flow::Return);
        assert_eq!(dis(&[0x4c, 0x00, 0x10]).flow, Flow::Jump);
        assert_eq!(dis(&[0x6c, 0xfc, 0xff]).flow, Flow::Jump);
        assert_eq!(dis(&[0xd0, 0xfe]).flow, Flow::Branch);
        assert_eq!(dis(&[0x00]).flow, Flow::Normal, "BRK is stepped into");
        assert_eq!(dis(&[0xa9, 0x00]).flow, Flow::Normal);
    }
}
