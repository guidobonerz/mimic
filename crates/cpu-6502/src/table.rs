//! NMOS 6502 opcode matrix including the undocumented opcodes.
//!
//! Source: MOS MCS6500 hardware manual, "6502 instruction set" tables and
//! the "NMOS 6510 Unintended Opcodes" document (Groepaz).

/// Operation, named after the common mnemonic (undocumented opcodes use the
/// names from "NMOS 6510 Unintended Opcodes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    // documented
    Adc,
    And,
    Asl,
    Bcc,
    Bcs,
    Beq,
    Bit,
    Bmi,
    Bne,
    Bpl,
    Brk,
    Bvc,
    Bvs,
    Clc,
    Cld,
    Cli,
    Clv,
    Cmp,
    Cpx,
    Cpy,
    Dec,
    Dex,
    Dey,
    Eor,
    Inc,
    Inx,
    Iny,
    Jmp,
    Jsr,
    Lda,
    Ldx,
    Ldy,
    Lsr,
    Nop,
    Ora,
    Pha,
    Php,
    Pla,
    Plp,
    Rol,
    Ror,
    Rti,
    Rts,
    Sbc,
    Sec,
    Sed,
    Sei,
    Sta,
    Stx,
    Sty,
    Tax,
    Tay,
    Tsx,
    Txa,
    Txs,
    Tya,
    // undocumented
    Alr,
    Anc,
    Ane,
    Arr,
    Dcp,
    Isc,
    Jam,
    Las,
    Lax,
    Lxa,
    Rla,
    Rra,
    Sax,
    Sbx,
    Sha,
    Shx,
    Shy,
    Slo,
    Sre,
    Tas,
}

/// Bus access pattern of an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Reads its operand.
    Read,
    /// Writes a register-derived value.
    Write,
    /// Read-modify-write (dummy write of the unmodified value first).
    Rmw,
    /// No memory operand.
    Implied,
    /// Own cycle sequence (stack, jumps, branches, BRK, JAM).
    Special,
}

impl Op {
    pub(crate) const fn kind(self) -> Kind {
        use Op::*;
        match self {
            Adc | And | Bit | Cmp | Cpx | Cpy | Eor | Lda | Ldx | Ldy | Nop | Ora | Sbc | Alr
            | Anc | Ane | Arr | Las | Lax | Lxa | Sbx => Kind::Read,
            Sta | Stx | Sty | Sax | Sha | Shx | Shy | Tas => Kind::Write,
            Asl | Dec | Inc | Lsr | Rol | Ror | Dcp | Isc | Rla | Rra | Slo | Sre => Kind::Rmw,
            Clc | Cld | Cli | Clv | Dex | Dey | Inx | Iny | Sec | Sed | Sei | Tax | Tay | Tsx
            | Txa | Txs | Tya => Kind::Implied,
            Bcc | Bcs | Beq | Bmi | Bne | Bpl | Bvc | Bvs | Brk | Jmp | Jsr | Pha | Php | Pla
            | Plp | Rti | Rts | Jam => Kind::Special,
        }
    }
}

/// Addressing mode. Accumulator forms (`ASL A`) are `Imp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Imp,
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    Ind,
    IndX,
    IndY,
    Rel,
}

impl Mode {
    /// Instruction length in bytes.
    pub(crate) const fn bytes(self) -> u16 {
        match self {
            Mode::Imp => 1,
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind => 3,
            _ => 2,
        }
    }
}

use Mode::*;
use Op::*;

#[rustfmt::skip]
pub(crate) const TABLE: [(Op, Mode); 256] = [
    // 0x00
    (Brk, Imp), (Ora, IndX), (Jam, Imp), (Slo, IndX), (Nop, Zp),   (Ora, Zp),   (Asl, Zp),   (Slo, Zp),
    (Php, Imp), (Ora, Imm),  (Asl, Imp), (Anc, Imm),  (Nop, Abs),  (Ora, Abs),  (Asl, Abs),  (Slo, Abs),
    // 0x10
    (Bpl, Rel), (Ora, IndY), (Jam, Imp), (Slo, IndY), (Nop, ZpX),  (Ora, ZpX),  (Asl, ZpX),  (Slo, ZpX),
    (Clc, Imp), (Ora, AbsY), (Nop, Imp), (Slo, AbsY), (Nop, AbsX), (Ora, AbsX), (Asl, AbsX), (Slo, AbsX),
    // 0x20
    (Jsr, Abs), (And, IndX), (Jam, Imp), (Rla, IndX), (Bit, Zp),   (And, Zp),   (Rol, Zp),   (Rla, Zp),
    (Plp, Imp), (And, Imm),  (Rol, Imp), (Anc, Imm),  (Bit, Abs),  (And, Abs),  (Rol, Abs),  (Rla, Abs),
    // 0x30
    (Bmi, Rel), (And, IndY), (Jam, Imp), (Rla, IndY), (Nop, ZpX),  (And, ZpX),  (Rol, ZpX),  (Rla, ZpX),
    (Sec, Imp), (And, AbsY), (Nop, Imp), (Rla, AbsY), (Nop, AbsX), (And, AbsX), (Rol, AbsX), (Rla, AbsX),
    // 0x40
    (Rti, Imp), (Eor, IndX), (Jam, Imp), (Sre, IndX), (Nop, Zp),   (Eor, Zp),   (Lsr, Zp),   (Sre, Zp),
    (Pha, Imp), (Eor, Imm),  (Lsr, Imp), (Alr, Imm),  (Jmp, Abs),  (Eor, Abs),  (Lsr, Abs),  (Sre, Abs),
    // 0x50
    (Bvc, Rel), (Eor, IndY), (Jam, Imp), (Sre, IndY), (Nop, ZpX),  (Eor, ZpX),  (Lsr, ZpX),  (Sre, ZpX),
    (Cli, Imp), (Eor, AbsY), (Nop, Imp), (Sre, AbsY), (Nop, AbsX), (Eor, AbsX), (Lsr, AbsX), (Sre, AbsX),
    // 0x60
    (Rts, Imp), (Adc, IndX), (Jam, Imp), (Rra, IndX), (Nop, Zp),   (Adc, Zp),   (Ror, Zp),   (Rra, Zp),
    (Pla, Imp), (Adc, Imm),  (Ror, Imp), (Arr, Imm),  (Jmp, Ind),  (Adc, Abs),  (Ror, Abs),  (Rra, Abs),
    // 0x70
    (Bvs, Rel), (Adc, IndY), (Jam, Imp), (Rra, IndY), (Nop, ZpX),  (Adc, ZpX),  (Ror, ZpX),  (Rra, ZpX),
    (Sei, Imp), (Adc, AbsY), (Nop, Imp), (Rra, AbsY), (Nop, AbsX), (Adc, AbsX), (Ror, AbsX), (Rra, AbsX),
    // 0x80
    (Nop, Imm), (Sta, IndX), (Nop, Imm), (Sax, IndX), (Sty, Zp),   (Sta, Zp),   (Stx, Zp),   (Sax, Zp),
    (Dey, Imp), (Nop, Imm),  (Txa, Imp), (Ane, Imm),  (Sty, Abs),  (Sta, Abs),  (Stx, Abs),  (Sax, Abs),
    // 0x90
    (Bcc, Rel), (Sta, IndY), (Jam, Imp), (Sha, IndY), (Sty, ZpX),  (Sta, ZpX),  (Stx, ZpY),  (Sax, ZpY),
    (Tya, Imp), (Sta, AbsY), (Txs, Imp), (Tas, AbsY), (Shy, AbsX), (Sta, AbsX), (Shx, AbsY), (Sha, AbsY),
    // 0xA0
    (Ldy, Imm), (Lda, IndX), (Ldx, Imm), (Lax, IndX), (Ldy, Zp),   (Lda, Zp),   (Ldx, Zp),   (Lax, Zp),
    (Tay, Imp), (Lda, Imm),  (Tax, Imp), (Lxa, Imm),  (Ldy, Abs),  (Lda, Abs),  (Ldx, Abs),  (Lax, Abs),
    // 0xB0
    (Bcs, Rel), (Lda, IndY), (Jam, Imp), (Lax, IndY), (Ldy, ZpX),  (Lda, ZpX),  (Ldx, ZpY),  (Lax, ZpY),
    (Clv, Imp), (Lda, AbsY), (Tsx, Imp), (Las, AbsY), (Ldy, AbsX), (Lda, AbsX), (Ldx, AbsY), (Lax, AbsY),
    // 0xC0
    (Cpy, Imm), (Cmp, IndX), (Nop, Imm), (Dcp, IndX), (Cpy, Zp),   (Cmp, Zp),   (Dec, Zp),   (Dcp, Zp),
    (Iny, Imp), (Cmp, Imm),  (Dex, Imp), (Sbx, Imm),  (Cpy, Abs),  (Cmp, Abs),  (Dec, Abs),  (Dcp, Abs),
    // 0xD0
    (Bne, Rel), (Cmp, IndY), (Jam, Imp), (Dcp, IndY), (Nop, ZpX),  (Cmp, ZpX),  (Dec, ZpX),  (Dcp, ZpX),
    (Cld, Imp), (Cmp, AbsY), (Nop, Imp), (Dcp, AbsY), (Nop, AbsX), (Cmp, AbsX), (Dec, AbsX), (Dcp, AbsX),
    // 0xE0
    (Cpx, Imm), (Sbc, IndX), (Nop, Imm), (Isc, IndX), (Cpx, Zp),   (Sbc, Zp),   (Inc, Zp),   (Isc, Zp),
    (Inx, Imp), (Sbc, Imm),  (Nop, Imp), (Sbc, Imm),  (Cpx, Abs),  (Sbc, Abs),  (Inc, Abs),  (Isc, Abs),
    // 0xF0
    (Beq, Rel), (Sbc, IndY), (Jam, Imp), (Isc, IndY), (Nop, ZpX),  (Sbc, ZpX),  (Inc, ZpX),  (Isc, ZpX),
    (Sed, Imp), (Sbc, AbsY), (Nop, Imp), (Isc, AbsY), (Nop, AbsX), (Sbc, AbsX), (Inc, AbsX), (Isc, AbsX),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jam_opcodes() {
        let jams: Vec<usize> = (0..256).filter(|&i| TABLE[i].0 == Jam).collect();
        assert_eq!(
            jams,
            [0x02, 0x12, 0x22, 0x32, 0x42, 0x52, 0x62, 0x72, 0x92, 0xb2, 0xd2, 0xf2]
        );
    }

    #[test]
    fn spot_checks() {
        assert_eq!(TABLE[0xa9], (Lda, Imm));
        assert_eq!(TABLE[0x6c], (Jmp, Ind));
        assert_eq!(TABLE[0x96], (Stx, ZpY));
        assert_eq!(TABLE[0xbe], (Ldx, AbsY));
        assert_eq!(TABLE[0x9e], (Shx, AbsY));
        assert_eq!(TABLE[0xeb], (Sbc, Imm));
        assert_eq!(TABLE[0x0a], (Asl, Imp));
    }

    #[test]
    fn documented_opcode_count() {
        // 151 documented opcodes on the NMOS 6502.
        let documented = (0..256)
            .filter(|&i| {
                let (op, _) = TABLE[i];
                !matches!(
                    op,
                    Alr | Anc
                        | Ane
                        | Arr
                        | Dcp
                        | Isc
                        | Jam
                        | Las
                        | Lax
                        | Lxa
                        | Rla
                        | Rra
                        | Sax
                        | Sbx
                        | Sha
                        | Shx
                        | Shy
                        | Slo
                        | Sre
                        | Tas
                ) && !(op == Nop && i != 0xea)
                    && i != 0xeb
            })
            .count();
        assert_eq!(documented, 151);
    }
}
