//! Disassembler. Uses `Bus::peek` only (golden rule 2).

use emu_core::{Bus, Instr, Size};

use crate::table::{Kind, Mode, TABLE};

fn peek(bus: &dyn Bus, addr: u16) -> u8 {
    bus.peek(u32::from(addr), Size::Byte) as u8
}

pub(crate) fn disassemble(bus: &dyn Bus, addr: u16) -> Instr {
    let opcode = peek(bus, addr);
    let (op, mode) = TABLE[usize::from(opcode)];
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
        Mode::Rel => {
            let dest = next.wrapping_add(b1 as i8 as u16);
            (format!("${dest:04X}"), Some(u32::from(dest)))
        }
    };

    Instr {
        addr: u32::from(addr),
        bytes,
        mnemonic: format!("{op:?}").to_ascii_uppercase(),
        operand,
        target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::testing::FlatBus;

    fn dis(program: &[u8]) -> Instr {
        let mut bus = FlatBus::new(16);
        bus.load(0x1000, program);
        disassemble(&bus, 0x1000)
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
}
