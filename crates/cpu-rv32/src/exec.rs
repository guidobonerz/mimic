//! The interpreter: one flat decode on the opcode bits per instruction.
//!
//! Encodings and immediates from the RISC-V Unprivileged ISA manual (RV32I
//! base formats R/I/S/B/U/J, the M extension's results for division by zero
//! and overflow, the Zicsr rules on when a CSR is written).

use crate::{
    Model, Rv32, Rv32Bus, StepResult, CAUSE_BREAKPOINT, CAUSE_ECALL_M, CAUSE_EXTERNAL_IRQ,
    CAUSE_ILLEGAL, MSTATUS_MIE, MSTATUS_MPIE,
};

pub(crate) const ECALL: u32 = 0x0000_0073;
pub(crate) const EBREAK: u32 = 0x0010_0073;
pub(crate) const MRET: u32 = 0x3020_0073;
pub(crate) const WFI: u32 = 0x1050_0073;

/// I-type immediate: bits 31–20, sign-extended.
#[inline(always)]
pub(crate) fn imm_i(i: u32) -> u32 {
    ((i as i32) >> 20) as u32
}

/// S-type immediate: bits 31–25 and 11–7.
#[inline(always)]
pub(crate) fn imm_s(i: u32) -> u32 {
    (((i as i32) >> 20) as u32 & !0x1f) | (i >> 7 & 0x1f)
}

/// B-type immediate: bit 31 → 12, bit 7 → 11, bits 30–25 → 10–5, bits 11–8
/// → 4–1.
#[inline(always)]
pub(crate) fn imm_b(i: u32) -> u32 {
    (((i as i32) >> 19) as u32 & 0xffff_f000)
        | (i << 4 & 0x800)
        | (i >> 20 & 0x7e0)
        | (i >> 7 & 0x1e)
}

/// J-type immediate: bit 31 → 20, bits 19–12 in place, bit 20 → 11, bits
/// 30–21 → 10–1.
#[inline(always)]
pub(crate) fn imm_j(i: u32) -> u32 {
    (((i as i32) >> 11) as u32 & 0xfff0_0000)
        | (i & 0x000f_f000)
        | (i >> 9 & 0x800)
        | (i >> 20 & 0x7fe)
}

impl Rv32 {
    #[inline(always)]
    fn set(&mut self, rd: usize, v: u32) {
        self.x[rd] = v;
        self.x[0] = 0;
    }

    /// Up to `n` steps; stops after the first that is not
    /// [`StepResult::Done`] and returns it (an interrupt entry, WFI or a
    /// trap), else `Done`.
    pub fn run<B: Rv32Bus>(&mut self, bus: &mut B, n: u32) -> StepResult {
        for _ in 0..n {
            let r = self.step(bus);
            if r != StepResult::Done {
                return r;
            }
        }
        StepResult::Done
    }

    /// One instruction, or the entry into the interrupt handler if the
    /// external interrupt is pending and enabled.
    #[inline]
    pub fn step<B: Rv32Bus>(&mut self, bus: &mut B) -> StepResult {
        if self.irq_ready {
            self.trap(CAUSE_EXTERNAL_IRQ, self.pc, 0);
            return StepResult::Interrupt;
        }
        let pc = self.pc;
        let i = bus.fetch(pc);
        let rd = (i >> 7 & 31) as usize;
        let rs1 = self.x[(i >> 15 & 31) as usize];
        let rs2 = self.x[(i >> 20 & 31) as usize];
        let funct3 = i >> 12 & 7;
        let mut next = pc.wrapping_add(4);
        // Compressed encodings (bits 1–0 not 11) fall through to the last
        // arm with every other unknown opcode.
        match i & 0x7f {
            // LUI
            0x37 => self.set(rd, i & 0xffff_f000),
            // AUIPC
            0x17 => self.set(rd, pc.wrapping_add(i & 0xffff_f000)),
            // JAL
            0x6f => {
                self.set(rd, next);
                next = pc.wrapping_add(imm_j(i)) & !3;
            }
            // JALR
            0x67 => {
                if funct3 != 0 {
                    return self.illegal(i, pc);
                }
                self.set(rd, next);
                next = rs1.wrapping_add(imm_i(i)) & !3;
            }
            // BEQ, BNE, BLT, BGE, BLTU, BGEU
            0x63 => {
                let taken = match funct3 {
                    0 => rs1 == rs2,
                    1 => rs1 != rs2,
                    4 => (rs1 as i32) < (rs2 as i32),
                    5 => (rs1 as i32) >= (rs2 as i32),
                    6 => rs1 < rs2,
                    7 => rs1 >= rs2,
                    _ => return self.illegal(i, pc),
                };
                if taken {
                    next = pc.wrapping_add(imm_b(i)) & !3;
                }
            }
            // LB, LH, LW, LBU, LHU
            0x03 => {
                let addr = rs1.wrapping_add(imm_i(i));
                let v = match funct3 {
                    0 => bus.read8(addr) as i8 as u32,
                    1 => bus.read16(addr) as i16 as u32,
                    2 => bus.read32(addr),
                    4 => u32::from(bus.read8(addr)),
                    5 => u32::from(bus.read16(addr)),
                    _ => return self.illegal(i, pc),
                };
                self.set(rd, v);
            }
            // SB, SH, SW
            0x23 => {
                let addr = rs1.wrapping_add(imm_s(i));
                match funct3 {
                    0 => bus.write8(addr, rs2 as u8),
                    1 => bus.write16(addr, rs2 as u16),
                    2 => bus.write32(addr, rs2),
                    _ => return self.illegal(i, pc),
                }
            }
            // ADDI, SLTI, SLTIU, XORI, ORI, ANDI, SLLI, SRLI, SRAI
            0x13 => {
                let imm = imm_i(i);
                let shamt = i >> 20 & 31;
                let v = match funct3 {
                    0 => rs1.wrapping_add(imm),
                    1 if i >> 25 == 0 => rs1 << shamt,
                    2 => u32::from((rs1 as i32) < (imm as i32)),
                    3 => u32::from(rs1 < imm),
                    4 => rs1 ^ imm,
                    5 if i >> 25 == 0 => rs1 >> shamt,
                    5 if i >> 25 == 0x20 => ((rs1 as i32) >> shamt) as u32,
                    6 => rs1 | imm,
                    7 => rs1 & imm,
                    _ => return self.illegal(i, pc),
                };
                self.set(rd, v);
            }
            // funct7 0: ADD, SLL, SLT, SLTU, XOR, SRL, OR, AND; $20: SUB,
            // SRA; 1: MUL, MULH, MULHSU, MULHU, DIV, DIVU, REM, REMU
            0x33 => {
                let v = match (i >> 25, funct3) {
                    (0, 0) => rs1.wrapping_add(rs2),
                    (0, 1) => rs1 << (rs2 & 31),
                    (0, 2) => u32::from((rs1 as i32) < (rs2 as i32)),
                    (0, 3) => u32::from(rs1 < rs2),
                    (0, 4) => rs1 ^ rs2,
                    (0, 5) => rs1 >> (rs2 & 31),
                    (0, 6) => rs1 | rs2,
                    (0, 7) => rs1 & rs2,
                    (0x20, 0) => rs1.wrapping_sub(rs2),
                    (0x20, 5) => ((rs1 as i32) >> (rs2 & 31)) as u32,
                    (1, 0) => rs1.wrapping_mul(rs2),
                    (1, 1) => ((i64::from(rs1 as i32) * i64::from(rs2 as i32)) >> 32) as u32,
                    (1, 2) => ((i64::from(rs1 as i32) * i64::from(rs2)) >> 32) as u32,
                    (1, 3) => ((u64::from(rs1) * u64::from(rs2)) >> 32) as u32,
                    // Division by zero: all ones; overflow: the dividend.
                    (1, 4) if rs2 == 0 => u32::MAX,
                    (1, 4) => (rs1 as i32).wrapping_div(rs2 as i32) as u32,
                    (1, 5) if rs2 == 0 => u32::MAX,
                    (1, 5) => rs1 / rs2,
                    // Remainder of a division by zero: the dividend;
                    // overflow: 0.
                    (1, 6) if rs2 == 0 => rs1,
                    (1, 6) => (rs1 as i32).wrapping_rem(rs2 as i32) as u32,
                    (1, 7) if rs2 == 0 => rs1,
                    (1, 7) => rs1 % rs2,
                    _ => return self.illegal(i, pc),
                };
                self.set(rd, v);
            }
            // FENCE, FENCE.I: nothing to do.
            0x0f => {}
            0x73 => return self.system(i, pc, rs1),
            _ => return self.illegal(i, pc),
        }
        self.pc = next;
        self.instret = self.instret.wrapping_add(1);
        StepResult::Done
    }

    #[cold]
    fn illegal(&mut self, i: u32, pc: u32) -> StepResult {
        self.trap(CAUSE_ILLEGAL, pc, i);
        StepResult::Illegal(i)
    }

    /// Opcode $73: ECALL, EBREAK, MRET, WFI and the six CSR instructions.
    fn system(&mut self, i: u32, pc: u32, rs1: u32) -> StepResult {
        let funct3 = i >> 12 & 7;
        let mut result = StepResult::Done;
        let mut next = pc.wrapping_add(4);
        match funct3 {
            0 => match i {
                ECALL => {
                    self.trap(CAUSE_ECALL_M, pc, 0);
                    return StepResult::Ecall;
                }
                EBREAK => {
                    self.trap(CAUSE_BREAKPOINT, pc, 0);
                    return StepResult::Ebreak;
                }
                MRET => {
                    let mie = if self.mstatus & MSTATUS_MPIE != 0 {
                        MSTATUS_MIE
                    } else {
                        0
                    };
                    self.mstatus = mie | MSTATUS_MPIE;
                    self.update_irq();
                    next = self.mepc & !3;
                }
                WFI => result = StepResult::Wfi,
                // rvlite lets every other one pass as a no-op.
                _ if self.model == Model::Rvlite => {}
                _ => return self.illegal(i, pc),
            },
            4 => return self.illegal(i, pc),
            _ => {
                let csr = (i >> 20) as u16;
                let field = i >> 15 & 31;
                let src = if funct3 & 4 == 0 {
                    rs1
                } else if self.model == Model::Rvlite {
                    // rvlite sign-extends the 5-bit immediate.
                    ((field << 27) as i32 >> 27) as u32
                } else {
                    field
                };
                let old = self.csr_read(csr);
                // CSRRS and CSRRC with x0 or a zero immediate do not write.
                match funct3 & 3 {
                    1 => self.csr_write(csr, src),
                    2 if field != 0 => self.csr_write(csr, old | src),
                    3 if field != 0 => self.csr_write(csr, old & !src),
                    _ => {}
                }
                self.set((i >> 7 & 31) as usize, old);
            }
        }
        self.pc = next;
        self.instret = self.instret.wrapping_add(1);
        result
    }
}
