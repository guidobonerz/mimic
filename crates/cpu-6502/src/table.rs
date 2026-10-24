//! NMOS 6502 opcode matrix including the undocumented opcodes, and the
//! CMOS 65C02's.
//!
//! Sources: MOS MCS6500 hardware manual, "6502 instruction set" tables and
//! the "NMOS 6510 Unintended Opcodes" document (Groepaz); for the 65C02 the
//! Rockwell R65C00 family and WDC W65C02S data sheets (opcode matrix; the
//! NCR/GTE/Synertek parts lack the Rockwell bit instructions) and Tom
//! Harte's `synertek65c02` tests for the undefined opcodes' lengths.

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
    // 65C02
    Bra,
    Phx,
    Phy,
    Plx,
    Ply,
    Stz,
    Trb,
    Tsb,
    /// Undefined 65C02 opcode: one byte, one cycle.
    Nop1,
    /// Undefined 65C02 opcodes $5C, $DC, $FC and $1F … $FF: three bytes,
    /// four cycles (the last byte is read again).
    NopAbs,
    /// Undefined 65C02 opcodes $0F … $EF: three bytes, three cycles.
    NopAbs3,
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
            Sta | Stx | Sty | Sax | Sha | Shx | Shy | Tas | Stz => Kind::Write,
            Asl | Dec | Inc | Lsr | Rol | Ror | Dcp | Isc | Rla | Rra | Slo | Sre | Trb | Tsb => {
                Kind::Rmw
            }
            Clc | Cld | Cli | Clv | Dex | Dey | Inx | Iny | Sec | Sed | Sei | Tax | Tay | Tsx
            | Txa | Txs | Tya => Kind::Implied,
            Bcc | Bcs | Beq | Bmi | Bne | Bpl | Bvc | Bvs | Brk | Jmp | Jsr | Pha | Php | Pla
            | Plp | Rti | Rts | Jam | Bra | Phx | Phy | Plx | Ply | Nop1 | NopAbs | NopAbs3 => {
                Kind::Special
            }
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
    /// 65C02 `(zp)`.
    Zpi,
    /// 65C02 `JMP (abs,X)`.
    AbsXInd,
}

impl Mode {
    /// Instruction length in bytes.
    pub(crate) const fn bytes(self) -> u16 {
        match self {
            Mode::Imp => 1,
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind | Mode::AbsXInd => 3,
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

/// The CMOS 65C02 (without the Rockwell bit instructions): the NMOS
/// matrix with the new instructions and addressing modes, every other
/// undefined opcode a NOP.
pub(crate) const CMOS_TABLE: [(Op, Mode); 256] = cmos_table();

const fn cmos_table() -> [(Op, Mode); 256] {
    let mut t = TABLE;
    let mut i = 0;
    while i < 256 {
        let (op, mode) = TABLE[i];
        let lo = i & 0x0f;
        let odd_row = i & 0x10 != 0;
        t[i] = if lo == 0x03 || lo == 0x0b {
            (Nop1, Imp)
        } else if lo == 0x07 {
            (Nop, if odd_row { ZpX } else { Zp })
        } else if lo == 0x0f {
            (if odd_row { NopAbs } else { NopAbs3 }, Abs)
        } else if lo == 0x02 && matches!(op, Jam | Nop) {
            if odd_row {
                // ORA AND EOR ADC STA LDA CMP SBC (zp): the (zp),Y column's ops.
                (TABLE[i - 1].0, Zpi)
            } else {
                (Nop, Imm)
            }
        } else {
            match op {
                Slo | Rla | Sre | Rra | Sax | Lax | Dcp | Isc | Anc | Alr | Arr | Ane | Lxa
                | Sbx | Las | Tas | Sha | Shx | Shy => (Nop1, Imp),
                _ => (op, mode),
            }
        };
        i += 1;
    }
    t[0x04] = (Tsb, Zp);
    t[0x0c] = (Tsb, Abs);
    t[0x14] = (Trb, Zp);
    t[0x1c] = (Trb, Abs);
    t[0x1a] = (Inc, Imp);
    t[0x3a] = (Dec, Imp);
    t[0x34] = (Bit, ZpX);
    t[0x3c] = (Bit, AbsX);
    t[0x89] = (Bit, Imm);
    t[0x5a] = (Phy, Imp);
    t[0x7a] = (Ply, Imp);
    t[0xda] = (Phx, Imp);
    t[0xfa] = (Plx, Imp);
    t[0x64] = (Stz, Zp);
    t[0x74] = (Stz, ZpX);
    t[0x9c] = (Stz, Abs);
    t[0x9e] = (Stz, AbsX);
    t[0x7c] = (Jmp, AbsXInd);
    t[0x80] = (Bra, Rel);
    t[0x5c] = (NopAbs, Abs);
    t[0xdc] = (NopAbs, Abs);
    t[0xfc] = (NopAbs, Abs);
    t[0xcb] = (Nop, Imp);
    t[0xdb] = (Nop, ZpX);
    t[0x44] = (Nop, Zp);
    t[0x54] = (Nop, ZpX);
    t[0xd4] = (Nop, ZpX);
    t[0xf4] = (Nop, ZpX);
    t
}

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
    fn cmos_spot_checks() {
        assert_eq!(CMOS_TABLE[0xb2], (Lda, Zpi));
        assert_eq!(CMOS_TABLE[0x92], (Sta, Zpi));
        assert_eq!(CMOS_TABLE[0x02], (Nop, Imm));
        assert_eq!(CMOS_TABLE[0xa2], (Ldx, Imm));
        assert_eq!(CMOS_TABLE[0xeb], (Nop1, Imp));
        assert_eq!(CMOS_TABLE[0x9e], (Stz, AbsX));
        assert_eq!(CMOS_TABLE[0xa9], (Lda, Imm));
        assert_eq!(CMOS_TABLE[0x1a], (Inc, Imp));
        assert!(CMOS_TABLE.iter().all(|e| e.0 != Jam));
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
