//! Opcode table, generated from the MEGA65 user guide's
//! `instruction_sets/4510.opc`, `4510.cycles`, `6502.opc`, `6502.cycles`,
//! `45GS02.opc` and `45GS02.cycles` (github.com/MEGA65/mega65-user-guide,
//! fetched 2026-11-11) by a script of this project. The 4510 counts of
//! BBR0–BBR3, BMI and the (zp,X)/(zp),Y forms of EOR, ADC and SBC are
//! set to their siblings' values where the guide's table has an evident
//! slip (a missing count, 3 cycles for an indirect mode).
//!
//! Cycle flags (the guide's legend): `R` and `I` add a cycle at 40 MHz,
//! `P` adds one when indexing crosses a page, `B` one for a taken branch
//! (and one more when it crosses a page), `D` subtracts one at 3.5 MHz.

use crate::Mode;

/// The 4510 mnemonics (`RTN` is RTS #imm8, `EOM` the 6502 NOP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mn {
    Adc,
    And,
    Asl,
    Asr,
    Asw,
    Bbr0,
    Bbr1,
    Bbr2,
    Bbr3,
    Bbr4,
    Bbr5,
    Bbr6,
    Bbr7,
    Bbs0,
    Bbs1,
    Bbs2,
    Bbs3,
    Bbs4,
    Bbs5,
    Bbs6,
    Bbs7,
    Bcc,
    Bcs,
    Beq,
    Bit,
    Bmi,
    Bne,
    Bpl,
    Bra,
    Brk,
    Bsr,
    Bvc,
    Bvs,
    Clc,
    Cld,
    Cle,
    Cli,
    Clv,
    Cmp,
    Cpx,
    Cpy,
    Cpz,
    Dec,
    Dew,
    Dex,
    Dey,
    Dez,
    Eom,
    Eor,
    Inc,
    Inw,
    Inx,
    Iny,
    Inz,
    Jmp,
    Jsr,
    Lda,
    Ldx,
    Ldy,
    Ldz,
    Lsr,
    Map,
    Neg,
    Ora,
    Pha,
    Php,
    Phw,
    Phx,
    Phy,
    Phz,
    Pla,
    Plp,
    Plx,
    Ply,
    Plz,
    Rmb0,
    Rmb1,
    Rmb2,
    Rmb3,
    Rmb4,
    Rmb5,
    Rmb6,
    Rmb7,
    Rol,
    Ror,
    Row,
    Rti,
    Rtn,
    Rts,
    Sbc,
    Sec,
    Sed,
    See,
    Sei,
    Smb0,
    Smb1,
    Smb2,
    Smb3,
    Smb4,
    Smb5,
    Smb6,
    Smb7,
    Sta,
    Stx,
    Sty,
    Stz,
    Tab,
    Tax,
    Tay,
    Taz,
    Tba,
    Trb,
    Tsb,
    Tsx,
    Tsy,
    Txa,
    Txs,
    Tya,
    Tys,
    Tza,
}

impl Mn {
    pub fn name(self) -> &'static str {
        match self {
            Mn::Adc => "ADC",
            Mn::And => "AND",
            Mn::Asl => "ASL",
            Mn::Asr => "ASR",
            Mn::Asw => "ASW",
            Mn::Bbr0 => "BBR0",
            Mn::Bbr1 => "BBR1",
            Mn::Bbr2 => "BBR2",
            Mn::Bbr3 => "BBR3",
            Mn::Bbr4 => "BBR4",
            Mn::Bbr5 => "BBR5",
            Mn::Bbr6 => "BBR6",
            Mn::Bbr7 => "BBR7",
            Mn::Bbs0 => "BBS0",
            Mn::Bbs1 => "BBS1",
            Mn::Bbs2 => "BBS2",
            Mn::Bbs3 => "BBS3",
            Mn::Bbs4 => "BBS4",
            Mn::Bbs5 => "BBS5",
            Mn::Bbs6 => "BBS6",
            Mn::Bbs7 => "BBS7",
            Mn::Bcc => "BCC",
            Mn::Bcs => "BCS",
            Mn::Beq => "BEQ",
            Mn::Bit => "BIT",
            Mn::Bmi => "BMI",
            Mn::Bne => "BNE",
            Mn::Bpl => "BPL",
            Mn::Bra => "BRA",
            Mn::Brk => "BRK",
            Mn::Bsr => "BSR",
            Mn::Bvc => "BVC",
            Mn::Bvs => "BVS",
            Mn::Clc => "CLC",
            Mn::Cld => "CLD",
            Mn::Cle => "CLE",
            Mn::Cli => "CLI",
            Mn::Clv => "CLV",
            Mn::Cmp => "CMP",
            Mn::Cpx => "CPX",
            Mn::Cpy => "CPY",
            Mn::Cpz => "CPZ",
            Mn::Dec => "DEC",
            Mn::Dew => "DEW",
            Mn::Dex => "DEX",
            Mn::Dey => "DEY",
            Mn::Dez => "DEZ",
            Mn::Eom => "EOM",
            Mn::Eor => "EOR",
            Mn::Inc => "INC",
            Mn::Inw => "INW",
            Mn::Inx => "INX",
            Mn::Iny => "INY",
            Mn::Inz => "INZ",
            Mn::Jmp => "JMP",
            Mn::Jsr => "JSR",
            Mn::Lda => "LDA",
            Mn::Ldx => "LDX",
            Mn::Ldy => "LDY",
            Mn::Ldz => "LDZ",
            Mn::Lsr => "LSR",
            Mn::Map => "MAP",
            Mn::Neg => "NEG",
            Mn::Ora => "ORA",
            Mn::Pha => "PHA",
            Mn::Php => "PHP",
            Mn::Phw => "PHW",
            Mn::Phx => "PHX",
            Mn::Phy => "PHY",
            Mn::Phz => "PHZ",
            Mn::Pla => "PLA",
            Mn::Plp => "PLP",
            Mn::Plx => "PLX",
            Mn::Ply => "PLY",
            Mn::Plz => "PLZ",
            Mn::Rmb0 => "RMB0",
            Mn::Rmb1 => "RMB1",
            Mn::Rmb2 => "RMB2",
            Mn::Rmb3 => "RMB3",
            Mn::Rmb4 => "RMB4",
            Mn::Rmb5 => "RMB5",
            Mn::Rmb6 => "RMB6",
            Mn::Rmb7 => "RMB7",
            Mn::Rol => "ROL",
            Mn::Ror => "ROR",
            Mn::Row => "ROW",
            Mn::Rti => "RTI",
            Mn::Rtn => "RTS",
            Mn::Rts => "RTS",
            Mn::Sbc => "SBC",
            Mn::Sec => "SEC",
            Mn::Sed => "SED",
            Mn::See => "SEE",
            Mn::Sei => "SEI",
            Mn::Smb0 => "SMB0",
            Mn::Smb1 => "SMB1",
            Mn::Smb2 => "SMB2",
            Mn::Smb3 => "SMB3",
            Mn::Smb4 => "SMB4",
            Mn::Smb5 => "SMB5",
            Mn::Smb6 => "SMB6",
            Mn::Smb7 => "SMB7",
            Mn::Sta => "STA",
            Mn::Stx => "STX",
            Mn::Sty => "STY",
            Mn::Stz => "STZ",
            Mn::Tab => "TAB",
            Mn::Tax => "TAX",
            Mn::Tay => "TAY",
            Mn::Taz => "TAZ",
            Mn::Tba => "TBA",
            Mn::Trb => "TRB",
            Mn::Tsb => "TSB",
            Mn::Tsx => "TSX",
            Mn::Tsy => "TSY",
            Mn::Txa => "TXA",
            Mn::Txs => "TXS",
            Mn::Tya => "TYA",
            Mn::Tys => "TYS",
            Mn::Tza => "TZA",
        }
    }
}

pub const R: u8 = 1;
pub const P: u8 = 2;
pub const B: u8 = 4;
pub const D: u8 = 8;
pub const I: u8 = 16;

/// One opcode of the 4510 personality.
#[derive(Clone, Copy, Debug)]
pub struct Op {
    pub mn: Mn,
    pub mode: Mode,
    /// Cycles at 3.5 and 40 MHz (before the flags).
    pub fast: u8,
    pub fast_flags: u8,
    /// Cycles at 1 and 2 MHz: the 6502's for its own opcodes, else the
    /// 4510's.
    pub slow: u8,
    pub slow_flags: u8,
}

pub static OPS: [Op; 256] = [
    // $00
    Op {
        mn: Mn::Brk,
        mode: Mode::Imp,
        fast: 7,
        fast_flags: 0,
        slow: 7,
        slow_flags: 0,
    },
    // $01
    Op {
        mn: Mn::Ora,
        mode: Mode::Izx,
        fast: 6,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $02
    Op {
        mn: Mn::Cle,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $03
    Op {
        mn: Mn::See,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $04
    Op {
        mn: Mn::Tsb,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $05
    Op {
        mn: Mn::Ora,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $06
    Op {
        mn: Mn::Asl,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $07
    Op {
        mn: Mn::Rmb0,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $08
    Op {
        mn: Mn::Php,
        mode: Mode::Imp,
        fast: 2,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $09
    Op {
        mn: Mn::Ora,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $0A
    Op {
        mn: Mn::Asl,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $0B
    Op {
        mn: Mn::Tsy,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $0C
    Op {
        mn: Mn::Tsb,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $0D
    Op {
        mn: Mn::Ora,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $0E
    Op {
        mn: Mn::Asl,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: R,
        slow: 6,
        slow_flags: 0,
    },
    // $0F
    Op {
        mn: Mn::Bbr0,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $10
    Op {
        mn: Mn::Bpl,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $11
    Op {
        mn: Mn::Ora,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $12
    Op {
        mn: Mn::Ora,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $13
    Op {
        mn: Mn::Bpl,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $14
    Op {
        mn: Mn::Trb,
        mode: Mode::Zp,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $15
    Op {
        mn: Mn::Ora,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R,
        slow: 4,
        slow_flags: P,
    },
    // $16
    Op {
        mn: Mn::Asl,
        mode: Mode::Zpx,
        fast: 4,
        fast_flags: R,
        slow: 6,
        slow_flags: 0,
    },
    // $17
    Op {
        mn: Mn::Rmb1,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $18
    Op {
        mn: Mn::Clc,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $19
    Op {
        mn: Mn::Ora,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: P,
    },
    // $1A
    Op {
        mn: Mn::Inc,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $1B
    Op {
        mn: Mn::Inz,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $1C
    Op {
        mn: Mn::Trb,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $1D
    Op {
        mn: Mn::Ora,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $1E
    Op {
        mn: Mn::Asl,
        mode: Mode::Absx,
        fast: 5,
        fast_flags: R | P,
        slow: 7,
        slow_flags: 0,
    },
    // $1F
    Op {
        mn: Mn::Bbr1,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $20
    Op {
        mn: Mn::Jsr,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: 0,
        slow: 6,
        slow_flags: 0,
    },
    // $21
    Op {
        mn: Mn::And,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $22
    Op {
        mn: Mn::Jsr,
        mode: Mode::Ind,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $23
    Op {
        mn: Mn::Jsr,
        mode: Mode::Iax,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $24
    Op {
        mn: Mn::Bit,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $25
    Op {
        mn: Mn::And,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $26
    Op {
        mn: Mn::Rol,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $27
    Op {
        mn: Mn::Rmb2,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $28
    Op {
        mn: Mn::Plp,
        mode: Mode::Imp,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $29
    Op {
        mn: Mn::And,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $2A
    Op {
        mn: Mn::Rol,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $2B
    Op {
        mn: Mn::Tys,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $2C
    Op {
        mn: Mn::Bit,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $2D
    Op {
        mn: Mn::And,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $2E
    Op {
        mn: Mn::Rol,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: R,
        slow: 6,
        slow_flags: 0,
    },
    // $2F
    Op {
        mn: Mn::Bbr2,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $30
    Op {
        mn: Mn::Bmi,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $31
    Op {
        mn: Mn::And,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $32
    Op {
        mn: Mn::And,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $33
    Op {
        mn: Mn::Bmi,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $34
    Op {
        mn: Mn::Bit,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 3,
        slow_flags: 0,
    },
    // $35
    Op {
        mn: Mn::And,
        mode: Mode::Zpx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $36
    Op {
        mn: Mn::Rol,
        mode: Mode::Zpx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $37
    Op {
        mn: Mn::Rmb3,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $38
    Op {
        mn: Mn::Sec,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $39
    Op {
        mn: Mn::And,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: P,
    },
    // $3A
    Op {
        mn: Mn::Dec,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $3B
    Op {
        mn: Mn::Dez,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $3C
    Op {
        mn: Mn::Bit,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $3D
    Op {
        mn: Mn::And,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $3E
    Op {
        mn: Mn::Rol,
        mode: Mode::Absx,
        fast: 5,
        fast_flags: R | P,
        slow: 7,
        slow_flags: 0,
    },
    // $3F
    Op {
        mn: Mn::Bbr3,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $40
    Op {
        mn: Mn::Rti,
        mode: Mode::Imp,
        fast: 6,
        fast_flags: 0,
        slow: 6,
        slow_flags: 0,
    },
    // $41
    Op {
        mn: Mn::Eor,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $42
    Op {
        mn: Mn::Neg,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $43
    Op {
        mn: Mn::Asr,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $44
    Op {
        mn: Mn::Asr,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $45
    Op {
        mn: Mn::Eor,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $46
    Op {
        mn: Mn::Lsr,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $47
    Op {
        mn: Mn::Rmb4,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $48
    Op {
        mn: Mn::Pha,
        mode: Mode::Imp,
        fast: 2,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $49
    Op {
        mn: Mn::Eor,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $4A
    Op {
        mn: Mn::Lsr,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $4B
    Op {
        mn: Mn::Taz,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $4C
    Op {
        mn: Mn::Jmp,
        mode: Mode::Abs,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $4D
    Op {
        mn: Mn::Eor,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $4E
    Op {
        mn: Mn::Lsr,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: R,
        slow: 6,
        slow_flags: 0,
    },
    // $4F
    Op {
        mn: Mn::Bbr4,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $50
    Op {
        mn: Mn::Bvc,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $51
    Op {
        mn: Mn::Eor,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $52
    Op {
        mn: Mn::Eor,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $53
    Op {
        mn: Mn::Bvc,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $54
    Op {
        mn: Mn::Asr,
        mode: Mode::Zpx,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $55
    Op {
        mn: Mn::Eor,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $56
    Op {
        mn: Mn::Lsr,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $57
    Op {
        mn: Mn::Rmb5,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $58
    Op {
        mn: Mn::Cli,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $59
    Op {
        mn: Mn::Eor,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $5A
    Op {
        mn: Mn::Phy,
        mode: Mode::Imp,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $5B
    Op {
        mn: Mn::Tab,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $5C
    Op {
        mn: Mn::Map,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $5D
    Op {
        mn: Mn::Eor,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $5E
    Op {
        mn: Mn::Lsr,
        mode: Mode::Absx,
        fast: 5,
        fast_flags: R | P,
        slow: 7,
        slow_flags: 0,
    },
    // $5F
    Op {
        mn: Mn::Bbr5,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $60
    Op {
        mn: Mn::Rts,
        mode: Mode::Imp,
        fast: 6,
        fast_flags: 0,
        slow: 6,
        slow_flags: 0,
    },
    // $61
    Op {
        mn: Mn::Adc,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $62
    Op {
        mn: Mn::Rtn,
        mode: Mode::Imm,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $63
    Op {
        mn: Mn::Bsr,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $64
    Op {
        mn: Mn::Stz,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $65
    Op {
        mn: Mn::Adc,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $66
    Op {
        mn: Mn::Ror,
        mode: Mode::Zp,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $67
    Op {
        mn: Mn::Rmb6,
        mode: Mode::Zp,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $68
    Op {
        mn: Mn::Pla,
        mode: Mode::Imp,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $69
    Op {
        mn: Mn::Adc,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $6A
    Op {
        mn: Mn::Ror,
        mode: Mode::Acc,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $6B
    Op {
        mn: Mn::Tza,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $6C
    Op {
        mn: Mn::Jmp,
        mode: Mode::Ind,
        fast: 5,
        fast_flags: R,
        slow: 5,
        slow_flags: 0,
    },
    // $6D
    Op {
        mn: Mn::Adc,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $6E
    Op {
        mn: Mn::Ror,
        mode: Mode::Abs,
        fast: 6,
        fast_flags: R,
        slow: 6,
        slow_flags: 0,
    },
    // $6F
    Op {
        mn: Mn::Bbr6,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $70
    Op {
        mn: Mn::Bvs,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $71
    Op {
        mn: Mn::Adc,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $72
    Op {
        mn: Mn::Adc,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $73
    Op {
        mn: Mn::Bvs,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $74
    Op {
        mn: Mn::Stz,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $75
    Op {
        mn: Mn::Adc,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $76
    Op {
        mn: Mn::Ror,
        mode: Mode::Zpx,
        fast: 5,
        fast_flags: R | P | D,
        slow: 6,
        slow_flags: 0,
    },
    // $77
    Op {
        mn: Mn::Rmb7,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $78
    Op {
        mn: Mn::Sei,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $79
    Op {
        mn: Mn::Adc,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: P,
    },
    // $7A
    Op {
        mn: Mn::Ply,
        mode: Mode::Imp,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $7B
    Op {
        mn: Mn::Tba,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $7C
    Op {
        mn: Mn::Jmp,
        mode: Mode::Iax,
        fast: 6,
        fast_flags: P,
        slow: 6,
        slow_flags: 0,
    },
    // $7D
    Op {
        mn: Mn::Adc,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: P,
    },
    // $7E
    Op {
        mn: Mn::Ror,
        mode: Mode::Absx,
        fast: 5,
        fast_flags: R | P | D,
        slow: 7,
        slow_flags: 0,
    },
    // $7F
    Op {
        mn: Mn::Bbr7,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $80
    Op {
        mn: Mn::Bra,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: 0,
    },
    // $81
    Op {
        mn: Mn::Sta,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: P,
        slow: 6,
        slow_flags: 0,
    },
    // $82
    Op {
        mn: Mn::Sta,
        mode: Mode::Isy,
        fast: 6,
        fast_flags: P,
        slow: 6,
        slow_flags: 0,
    },
    // $83
    Op {
        mn: Mn::Bra,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $84
    Op {
        mn: Mn::Sty,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $85
    Op {
        mn: Mn::Sta,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $86
    Op {
        mn: Mn::Stx,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $87
    Op {
        mn: Mn::Smb0,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $88
    Op {
        mn: Mn::Dey,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $89
    Op {
        mn: Mn::Bit,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $8A
    Op {
        mn: Mn::Txa,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $8B
    Op {
        mn: Mn::Sty,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $8C
    Op {
        mn: Mn::Sty,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $8D
    Op {
        mn: Mn::Sta,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $8E
    Op {
        mn: Mn::Stx,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $8F
    Op {
        mn: Mn::Bbs0,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $90
    Op {
        mn: Mn::Bcc,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $91
    Op {
        mn: Mn::Sta,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: P,
        slow: 6,
        slow_flags: 0,
    },
    // $92
    Op {
        mn: Mn::Sta,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: P,
        slow: 5,
        slow_flags: 0,
    },
    // $93
    Op {
        mn: Mn::Bcc,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $94
    Op {
        mn: Mn::Sty,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $95
    Op {
        mn: Mn::Sta,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $96
    Op {
        mn: Mn::Stx,
        mode: Mode::Zpy,
        fast: 3,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $97
    Op {
        mn: Mn::Smb1,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $98
    Op {
        mn: Mn::Tya,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $99
    Op {
        mn: Mn::Sta,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: P,
        slow: 5,
        slow_flags: 0,
    },
    // $9A
    Op {
        mn: Mn::Txs,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $9B
    Op {
        mn: Mn::Stx,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $9C
    Op {
        mn: Mn::Stz,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $9D
    Op {
        mn: Mn::Sta,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: P,
        slow: 5,
        slow_flags: 0,
    },
    // $9E
    Op {
        mn: Mn::Stz,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: P,
        slow: 4,
        slow_flags: 0,
    },
    // $9F
    Op {
        mn: Mn::Bbs1,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $A0
    Op {
        mn: Mn::Ldy,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $A1
    Op {
        mn: Mn::Lda,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $A2
    Op {
        mn: Mn::Ldx,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $A3
    Op {
        mn: Mn::Ldz,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $A4
    Op {
        mn: Mn::Ldy,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $A5
    Op {
        mn: Mn::Lda,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $A6
    Op {
        mn: Mn::Ldx,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $A7
    Op {
        mn: Mn::Smb2,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $A8
    Op {
        mn: Mn::Tay,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $A9
    Op {
        mn: Mn::Lda,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $AA
    Op {
        mn: Mn::Tax,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $AB
    Op {
        mn: Mn::Ldz,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $AC
    Op {
        mn: Mn::Ldy,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $AD
    Op {
        mn: Mn::Lda,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $AE
    Op {
        mn: Mn::Ldx,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $AF
    Op {
        mn: Mn::Bbs2,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $B0
    Op {
        mn: Mn::Bcs,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $B1
    Op {
        mn: Mn::Lda,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $B2
    Op {
        mn: Mn::Lda,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $B3
    Op {
        mn: Mn::Bcs,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $B4
    Op {
        mn: Mn::Ldy,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $B5
    Op {
        mn: Mn::Lda,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $B6
    Op {
        mn: Mn::Ldx,
        mode: Mode::Zpy,
        fast: 5,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $B7
    Op {
        mn: Mn::Smb3,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $B8
    Op {
        mn: Mn::Clv,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $B9
    Op {
        mn: Mn::Lda,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $BA
    Op {
        mn: Mn::Tsx,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $BB
    Op {
        mn: Mn::Ldz,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $BC
    Op {
        mn: Mn::Ldy,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $BD
    Op {
        mn: Mn::Lda,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $BE
    Op {
        mn: Mn::Ldx,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $BF
    Op {
        mn: Mn::Bbs3,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $C0
    Op {
        mn: Mn::Cpy,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $C1
    Op {
        mn: Mn::Cmp,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $C2
    Op {
        mn: Mn::Cpz,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $C3
    Op {
        mn: Mn::Dew,
        mode: Mode::Zp,
        fast: 7,
        fast_flags: R | D,
        slow: 7,
        slow_flags: 0,
    },
    // $C4
    Op {
        mn: Mn::Cpy,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $C5
    Op {
        mn: Mn::Cmp,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $C6
    Op {
        mn: Mn::Dec,
        mode: Mode::Zp,
        fast: 5,
        fast_flags: R | D,
        slow: 5,
        slow_flags: 0,
    },
    // $C7
    Op {
        mn: Mn::Smb4,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $C8
    Op {
        mn: Mn::Iny,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $C9
    Op {
        mn: Mn::Cmp,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $CA
    Op {
        mn: Mn::Dex,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $CB
    Op {
        mn: Mn::Asw,
        mode: Mode::Abs,
        fast: 7,
        fast_flags: R | D,
        slow: 7,
        slow_flags: 0,
    },
    // $CC
    Op {
        mn: Mn::Cpy,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $CD
    Op {
        mn: Mn::Cmp,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $CE
    Op {
        mn: Mn::Dec,
        mode: Mode::Abs,
        fast: 6,
        fast_flags: R | D,
        slow: 6,
        slow_flags: 0,
    },
    // $CF
    Op {
        mn: Mn::Bbs4,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $D0
    Op {
        mn: Mn::Bne,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $D1
    Op {
        mn: Mn::Cmp,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $D2
    Op {
        mn: Mn::Cmp,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $D3
    Op {
        mn: Mn::Bne,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $D4
    Op {
        mn: Mn::Cpz,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $D5
    Op {
        mn: Mn::Cmp,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $D6
    Op {
        mn: Mn::Dec,
        mode: Mode::Zpx,
        fast: 5,
        fast_flags: R | P | D,
        slow: 6,
        slow_flags: 0,
    },
    // $D7
    Op {
        mn: Mn::Smb5,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $D8
    Op {
        mn: Mn::Cld,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $D9
    Op {
        mn: Mn::Cmp,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $DA
    Op {
        mn: Mn::Phx,
        mode: Mode::Imp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $DB
    Op {
        mn: Mn::Phz,
        mode: Mode::Imp,
        fast: 3,
        fast_flags: 0,
        slow: 3,
        slow_flags: 0,
    },
    // $DC
    Op {
        mn: Mn::Cpz,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $DD
    Op {
        mn: Mn::Cmp,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $DE
    Op {
        mn: Mn::Dec,
        mode: Mode::Absx,
        fast: 6,
        fast_flags: R | P | D,
        slow: 7,
        slow_flags: 0,
    },
    // $DF
    Op {
        mn: Mn::Bbs5,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $E0
    Op {
        mn: Mn::Cpx,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $E1
    Op {
        mn: Mn::Sbc,
        mode: Mode::Izx,
        fast: 5,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $E2
    Op {
        mn: Mn::Lda,
        mode: Mode::Isy,
        fast: 6,
        fast_flags: R | P,
        slow: 6,
        slow_flags: 0,
    },
    // $E3
    Op {
        mn: Mn::Inw,
        mode: Mode::Zp,
        fast: 7,
        fast_flags: R | D,
        slow: 7,
        slow_flags: 0,
    },
    // $E4
    Op {
        mn: Mn::Cpx,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $E5
    Op {
        mn: Mn::Sbc,
        mode: Mode::Zp,
        fast: 3,
        fast_flags: R,
        slow: 3,
        slow_flags: 0,
    },
    // $E6
    Op {
        mn: Mn::Inc,
        mode: Mode::Zp,
        fast: 5,
        fast_flags: R | D,
        slow: 5,
        slow_flags: 0,
    },
    // $E7
    Op {
        mn: Mn::Smb6,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $E8
    Op {
        mn: Mn::Inx,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $E9
    Op {
        mn: Mn::Sbc,
        mode: Mode::Imm,
        fast: 2,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $EA
    Op {
        mn: Mn::Eom,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 1,
        slow_flags: 0,
    },
    // $EB
    Op {
        mn: Mn::Row,
        mode: Mode::Abs,
        fast: 5,
        fast_flags: R | D,
        slow: 5,
        slow_flags: 0,
    },
    // $EC
    Op {
        mn: Mn::Cpx,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $ED
    Op {
        mn: Mn::Sbc,
        mode: Mode::Abs,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $EE
    Op {
        mn: Mn::Inc,
        mode: Mode::Abs,
        fast: 6,
        fast_flags: R | D,
        slow: 6,
        slow_flags: 0,
    },
    // $EF
    Op {
        mn: Mn::Bbs6,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
    // $F0
    Op {
        mn: Mn::Beq,
        mode: Mode::Rel,
        fast: 2,
        fast_flags: B,
        slow: 2,
        slow_flags: B,
    },
    // $F1
    Op {
        mn: Mn::Sbc,
        mode: Mode::Izy,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: P,
    },
    // $F2
    Op {
        mn: Mn::Sbc,
        mode: Mode::Izz,
        fast: 5,
        fast_flags: R | P,
        slow: 5,
        slow_flags: 0,
    },
    // $F3
    Op {
        mn: Mn::Beq,
        mode: Mode::Rel16,
        fast: 3,
        fast_flags: B,
        slow: 3,
        slow_flags: 0,
    },
    // $F4
    Op {
        mn: Mn::Phw,
        mode: Mode::Imm16,
        fast: 5,
        fast_flags: 0,
        slow: 5,
        slow_flags: 0,
    },
    // $F5
    Op {
        mn: Mn::Sbc,
        mode: Mode::Zpx,
        fast: 3,
        fast_flags: R | P,
        slow: 4,
        slow_flags: 0,
    },
    // $F6
    Op {
        mn: Mn::Inc,
        mode: Mode::Zpx,
        fast: 5,
        fast_flags: R | P | D,
        slow: 6,
        slow_flags: 0,
    },
    // $F7
    Op {
        mn: Mn::Smb7,
        mode: Mode::Zp,
        fast: 4,
        fast_flags: R,
        slow: 4,
        slow_flags: 0,
    },
    // $F8
    Op {
        mn: Mn::Sed,
        mode: Mode::Imp,
        fast: 1,
        fast_flags: 0,
        slow: 2,
        slow_flags: 0,
    },
    // $F9
    Op {
        mn: Mn::Sbc,
        mode: Mode::Absy,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $FA
    Op {
        mn: Mn::Plx,
        mode: Mode::Imp,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $FB
    Op {
        mn: Mn::Plz,
        mode: Mode::Imp,
        fast: 4,
        fast_flags: 0,
        slow: 4,
        slow_flags: 0,
    },
    // $FC
    Op {
        mn: Mn::Phw,
        mode: Mode::Abs,
        fast: 7,
        fast_flags: 0,
        slow: 7,
        slow_flags: 0,
    },
    // $FD
    Op {
        mn: Mn::Sbc,
        mode: Mode::Absx,
        fast: 4,
        fast_flags: R | P,
        slow: 4,
        slow_flags: P,
    },
    // $FE
    Op {
        mn: Mn::Inc,
        mode: Mode::Absx,
        fast: 6,
        fast_flags: R | P | D,
        slow: 7,
        slow_flags: 0,
    },
    // $FF
    Op {
        mn: Mn::Bbs7,
        mode: Mode::Zrel,
        fast: 4,
        fast_flags: R | B,
        slow: 4,
        slow_flags: 0,
    },
];

/// The compound instructions: prefix bytes and opcode as written in the
/// guide (`4242xx` Q, `EAxx` flat, `4242EAxx` flat Q), the mnemonic and
/// the cycles with their flags.
pub const COMPOUND: &[(u32, &str, u8, u8)] = &[
    (0x424201, "RESQ ($nn,X)", 10, R | P | I),
    (0x424205, "ORQ $nn", 8, R),
    (0x424206, "ASLQ $nn", 12, R | D),
    (0x42420A, "ASLQ ", 3, 0),
    (0x42420D, "ORQ $nnnn", 9, R),
    (0x42420E, "ASLQ $nnnn", 13, R | D),
    (0x424211, "RESQ ($nn),Y", 10, R | P | I),
    (0x424212, "ORQ ($nn)", 10, R | P),
    (0x424215, "RESQ $nn,X", 8, R | P),
    (0x424216, "ASLQ $nn,X", 12, R | P | D),
    (0x424219, "RESQ $nnnn,Y", 9, R | P),
    (0x42421A, "INQ ", 3, 0),
    (0x42421D, "RESQ $nnnn,X", 9, R | P),
    (0x42421E, "ASLQ $nnnn,X", 13, R | P | D),
    (0x424221, "RESQ ($nn,X)", 10, R | I),
    (0x424224, "BITQ $nn", 8, R),
    (0x424225, "ANDQ $nn", 8, R),
    (0x424226, "ROLQ $nn", 12, R | D),
    (0x42422A, "ROLQ ", 3, 0),
    (0x42422C, "BITQ $nnnn", 9, R),
    (0x42422D, "ANDQ $nnnn", 9, R),
    (0x42422E, "ROLQ $nnnn", 13, R | D),
    (0x424231, "RESQ ($nn),Y", 10, R | P | I),
    (0x424232, "ANDQ ($nn)", 10, R | P | I),
    (0x424234, "RESQ $nn,X", 8, R | P),
    (0x424235, "RESQ $nn,X", 8, R | P),
    (0x424236, "ROLQ $nn,X", 12, R | P | D),
    (0x424239, "RESQ $nnnn,Y", 10, R | P),
    (0x42423A, "DEQ ", 3, 0),
    (0x42423C, "RESQ $nnnn,X", 9, R | P),
    (0x42423D, "RESQ $nnnn,X", 10, R | P),
    (0x42423E, "ROLQ $nnnn,X", 13, R | P | D),
    (0x424241, "RESQ ($nn,X)", 10, R | P | I),
    (0x424243, "ASRQ ", 3, 0),
    (0x424244, "ASRQ $nn", 12, R | D),
    (0x424245, "EORQ $nn", 8, R),
    (0x424246, "LSRQ $nn", 12, R | D),
    (0x42424A, "LSRQ ", 3, 0),
    (0x42424D, "EORQ $nnnn", 9, R),
    (0x42424E, "LSRQ $nnnn", 13, R | D),
    (0x424251, "RESQ ($nn),Y", 10, R | P | I),
    (0x424252, "EORQ ($nn)", 10, R | P | I),
    (0x424254, "ASRQ $nn,X", 12, R | P | D),
    (0x424255, "RESQ $nn,X", 8, R | P),
    (0x424256, "LSRQ $nn,X", 12, R | P | D),
    (0x424259, "RESQ $nnnn,Y", 9, R | P),
    (0x42425D, "RESQ $nnnn,X", 9, R | P),
    (0x42425E, "LSRQ $nnnn,X", 13, R | P | D),
    (0x424261, "RESQ ($nn,X)", 10, R | I),
    (0x424265, "ADCQ $nn", 8, R),
    (0x424266, "RORQ $nn", 12, R | D),
    (0x42426A, "RORQ ", 3, 0),
    (0x42426D, "ADCQ $nnnn", 9, R),
    (0x42426E, "RORQ $nnnn", 13, R | D),
    (0x424271, "RESQ ($nn),Y", 10, R | P | I),
    (0x424272, "ADCQ ($nn)", 10, R | P | I),
    (0x424275, "RESQ $nn,X", 8, R | P),
    (0x424276, "RORQ $nn,X", 12, R | P | D),
    (0x424279, "RESQ $nnnn,Y", 10, R | P),
    (0x42427D, "RESQ $nnnn,X", 10, R | P),
    (0x42427E, "RORQ $nnnn,X", 13, R | P | D),
    (0x424281, "RSVQ ($nn,X)", 10, P | I),
    (0x424282, "RSVQ ($nn,SP),Y", 10, P | I),
    (0x424285, "STQ $nn", 8, 0),
    (0x42428D, "STQ $nnnn", 9, 0),
    (0x424291, "RSVQ ($nn),Y", 10, P | I),
    (0x424292, "STQ ($nn)", 10, P | I),
    (0x424295, "RSVQ $nn,X", 8, P),
    (0x424299, "RSVQ $nnnn,Y", 9, P),
    (0x42429D, "RSVQ $nnnn,X", 9, P),
    (0x4242A1, "RSVQ ($nn,X)", 10, R | P | I),
    (0x4242A5, "LDQ $nn", 8, R),
    (0x4242AD, "LDQ $nnnn", 9, R),
    (0x4242B2, "LDQ ($nn),Z", 10, R | P | I),
    (0x4242C1, "RSVQ ($nn,X)", 10, R | P | I),
    (0x4242C5, "CMPQ $nn", 8, R),
    (0x4242C6, "DEQ $nn", 12, R | D),
    (0x4242CD, "CMPQ $nnnn", 9, R),
    (0x4242CE, "DEQ $nnnn", 13, R | D),
    (0x4242D1, "RSVQ ($nn),Y", 10, R | P | I),
    (0x4242D2, "CMPQ ($nn)", 10, R | P | I),
    (0x4242D5, "RSVQ $nn,X", 8, R | P),
    (0x4242D6, "DEQ $nn,X", 12, R | P | D),
    (0x4242D9, "RSVQ $nnnn,Y", 9, R | P),
    (0x4242DD, "RSVQ $nnnn,X", 9, R | P),
    (0x4242DE, "DEQ $nnnn,X", 13, R | P | D),
    (0x4242E1, "RSVQ ($nn,X)", 10, R | P | I),
    (0x4242E5, "SBCQ $nn", 8, R),
    (0x4242E6, "INQ $nn", 13, R | D),
    (0x4242EA12, "ORQ [$nn]", 13, R | P),
    (0x4242EA32, "ANDQ [$nn]", 13, R | P | I),
    (0x4242EA52, "EORQ [$nn]", 13, R | P | I),
    (0x4242EA72, "ADCQ [$nn]", 13, R | P | I),
    (0x4242EA92, "STQ [$nn]", 13, P | I),
    (0x4242EAB2, "LDQ [$nn],Z", 13, R | P | I),
    (0x4242EAD2, "CMPQ [$nn]", 13, R | P | I),
    (0x4242EAF2, "SBCQ [$nn]", 13, R | P | I),
    (0x4242ED, "SBCQ $nnnn", 9, R),
    (0x4242EE, "INQ $nnnn", 14, R | D),
    (0x4242F1, "RSVQ ($nn),Y", 10, R | P | I),
    (0x4242F2, "SBCQ ($nn)", 10, R | P | I),
    (0x4242F5, "RSVQ $nn,X", 8, R | P),
    (0x4242F6, "INQ $nn,X", 13, R | P | D),
    (0x4242F9, "RSVQ $nnnn,Y", 8, R | P),
    (0x4242FD, "RSVQ $nnnn,X", 9, R | P),
    (0x4242FE, "INQ $nnnn,X", 14, R | P | D),
    (0xEA12, "ORA [$nn],Z", 7, R | P | I),
    (0xEA32, "AND [$nn],Z", 7, R | P | I),
    (0xEA52, "EOR [$nn],Z", 7, R | P | I),
    (0xEA72, "ADC [$nn],Z", 7, R | P | I),
    (0xEA92, "STA [$nn],Z", 8, P | I),
    (0xEAB2, "LDA [$nn],Z", 7, R | P | I),
    (0xEAD2, "CMP [$nn],Z", 7, R | P | I),
    (0xEAF2, "SBC [$nn],Z", 0, 0),
];
