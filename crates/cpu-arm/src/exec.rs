//! Instruction execution.

use crate::{Arm2, ArmBus, F_C, F_I, F_N, F_V, F_Z, MODE_SVC, PC_MASK, PSR_MASK};

/// The address bus is 26 bits: a data access above it is an address
/// exception.
const ADDRESS_LIMIT: u32 = 1 << 26;

impl Arm2 {
    fn condition(&self, instr: u32) -> bool {
        let (n, z, c, v) = (
            self.psr & F_N != 0,
            self.psr & F_Z != 0,
            self.psr & F_C != 0,
            self.psr & F_V != 0,
        );
        match instr >> 28 {
            0 => z,
            1 => !z,
            2 => c,
            3 => !c,
            4 => n,
            5 => !n,
            6 => v,
            7 => !v,
            8 => c && !z,
            9 => !c || z,
            10 => n == v,
            11 => n != v,
            12 => !z && n == v,
            13 => z || n != v,
            14 => true,
            _ => false,
        }
    }

    /// Run the instruction in the execute stage. Its first cycle fetches
    /// the next word; PC stays at instruction + 8 until the end, `pc + 4`
    /// serves what the data manual calls + 12.
    pub(crate) fn execute<B: ArmBus>(&mut self, bus: &mut B) {
        let current = self.pipe[0];
        let seq = !std::mem::take(&mut self.fetch_n);
        let incoming = self.prefetch(seq, bus);
        let start_pc = self.pc;
        self.pipe = [self.pipe[1], incoming];
        if current.aborted {
            // Undefined by the instruction: the fetch failed.
            self.trap(0x0c, MODE_SVC, F_I, start_pc.wrapping_sub(4), bus);
            return;
        }
        let instr = current.word;
        let mut branched = false;
        if self.condition(instr) {
            branched = match instr >> 25 & 7 {
                0 | 1 => self.data_processing(instr, bus),
                2 | 3 => {
                    if instr & 0x0200_0000 != 0 && instr & 0x10 != 0 {
                        self.undefined(bus);
                        true
                    } else {
                        self.single_transfer(instr, bus)
                    }
                }
                4 => self.block_transfer(instr, bus),
                5 => {
                    if instr & 0x0100_0000 != 0 {
                        self.r[14] = self.pc.wrapping_sub(4) & PC_MASK | self.psr;
                    }
                    let offset = ((instr << 8) as i32 >> 6) as u32;
                    self.branch(self.pc.wrapping_add(offset), bus);
                    true
                }
                6 => {
                    self.undefined(bus);
                    true
                }
                _ => {
                    if instr & 0x0100_0000 != 0 {
                        self.trap(0x08, MODE_SVC, F_I, self.pc.wrapping_sub(4), bus);
                    } else {
                        self.undefined(bus);
                    }
                    true
                }
            };
        }
        if !branched {
            self.pc = (self.pc + 4) & PC_MASK;
        }
    }

    fn undefined<B: ArmBus>(&mut self, bus: &mut B) {
        self.trap(0x04, MODE_SVC, F_I, self.pc.wrapping_sub(4), bus);
    }

    /// Register `n` as an operand: R15 with the status bits if `with_psr`,
    /// `extra` further on than instruction + 8.
    fn operand(&self, n: usize, with_psr: bool, extra: u32) -> u32 {
        if n == 15 {
            let pc = self.pc.wrapping_add(extra) & PC_MASK;
            if with_psr {
                pc | self.psr
            } else {
                pc
            }
        } else {
            self.r[n]
        }
    }

    /// Barrel shifter on Rm: value and carry out.
    fn shifted<B: ArmBus>(&mut self, instr: u32, bus: &mut B) -> (u32, bool) {
        let rm = (instr & 15) as usize;
        let kind = instr >> 5 & 3;
        let carry = self.psr & F_C != 0;
        if instr & 0x10 != 0 {
            // By a register: one internal cycle, and PC moved on.
            bus.idle(1);
            let rs = (instr >> 8 & 15) as usize;
            let amount = self.operand(rs, false, 4) & 0xff;
            let v = self.operand(rm, true, 4);
            if amount == 0 {
                return (v, carry);
            }
            return shift(v, kind, amount, carry);
        }
        let v = self.operand(rm, true, 0);
        let amount = instr >> 7 & 31;
        match (kind, amount) {
            (0, 0) => (v, carry),
            (1 | 2, 0) => shift(v, kind, 32, carry),
            (3, 0) => ((v >> 1) | (u32::from(carry) << 31), v & 1 != 0),
            _ => shift(v, kind, amount, carry),
        }
    }

    /// Data processing, and multiply in its encoding space. True if the
    /// instruction changed the program flow.
    fn data_processing<B: ArmBus>(&mut self, instr: u32, bus: &mut B) -> bool {
        let imm = instr & 0x0200_0000 != 0;
        if !imm && instr & 0x90 == 0x90 {
            if instr & 0x0fc0_00f0 == 0x90 {
                self.multiply(instr, bus);
                return false;
            }
            self.undefined(bus);
            return true;
        }
        let set = instr & 0x0010_0000 != 0;
        let op = instr >> 21 & 15;
        let rd = (instr >> 12 & 15) as usize;
        let (op2, shifter_c) = if imm {
            let rot = (instr >> 8 & 15) * 2;
            let v = (instr & 0xff).rotate_right(rot);
            let c = if rot == 0 {
                self.psr & F_C != 0
            } else {
                v & 0x8000_0000 != 0
            };
            (v, c)
        } else {
            self.shifted(instr, bus)
        };
        // The register-shift form reads Rn after the second cycle's PC.
        let extra = if !imm && instr & 0x10 != 0 { 4 } else { 0 };
        let rn = self.operand((instr >> 16 & 15) as usize, false, extra);
        let carry = self.psr & F_C != 0;
        let (result, c, v) = match op {
            0 | 8 => (rn & op2, shifter_c, None),
            1 | 9 => (rn ^ op2, shifter_c, None),
            2 | 10 => sub(rn, op2, true),
            3 => sub(op2, rn, true),
            4 | 11 => sub(rn, !op2, false),
            5 => sub(rn, !op2, carry),
            6 => sub(rn, op2, carry),
            7 => sub(op2, rn, carry),
            12 => (rn | op2, shifter_c, None),
            13 => (op2, shifter_c, None),
            14 => (rn & !op2, shifter_c, None),
            _ => (!op2, shifter_c, None),
        };
        let test = (8..=11).contains(&op);
        if rd == 15 {
            if set {
                self.write_status(result);
            }
            if !test {
                self.branch(result, bus);
                return true;
            }
            return false;
        }
        if set {
            let mut psr = self.psr & !(F_N | F_Z | F_C);
            if result == 0 {
                psr |= F_Z;
            }
            if result & 0x8000_0000 != 0 {
                psr |= F_N;
            }
            if c {
                psr |= F_C;
            }
            if let Some(v) = v {
                psr = (psr & !F_V) | if v { F_V } else { 0 };
            }
            self.psr = psr;
        }
        if !test {
            self.r[rd] = result;
        }
        false
    }

    /// R15 written with S: flags always, I, F and the mode only outside
    /// user mode.
    fn write_status(&mut self, value: u32) {
        let value = value & PSR_MASK;
        if self.privileged() {
            self.set_psr(value);
        } else {
            self.psr = (self.psr & !0xf000_0000) | (value & 0xf000_0000);
        }
    }

    fn multiply<B: ArmBus>(&mut self, instr: u32, bus: &mut B) {
        let rd = (instr >> 16 & 15) as usize;
        let rn = (instr >> 12 & 15) as usize;
        let rs = (instr >> 8 & 15) as usize;
        let rm = (instr & 15) as usize;
        let accumulate = instr & 0x0020_0000 != 0;
        let by = self.operand(rs, false, 0);
        let mut result = self.operand(rm, true, 4).wrapping_mul(by);
        if accumulate {
            result = result.wrapping_add(self.operand(rn, true, 0));
        }
        // Two bits of Rs a cycle, ending when the rest is zero: table 21.
        let bits = 32 - by.leading_zeros();
        let m = if by < 2 { 1 } else { (bits + 2) / 2 }.min(16);
        bus.idle(m);
        if rd != 15 {
            self.r[rd] = result;
        }
        if instr & 0x0010_0000 != 0 {
            let mut psr = self.psr & !(F_N | F_Z);
            if result == 0 {
                psr |= F_Z;
            }
            if result & 0x8000_0000 != 0 {
                psr |= F_N;
            }
            self.psr = psr;
        }
    }

    /// LDR, STR.
    fn single_transfer<B: ArmBus>(&mut self, instr: u32, bus: &mut B) -> bool {
        let pre = instr & 0x0100_0000 != 0;
        let up = instr & 0x0080_0000 != 0;
        let byte = instr & 0x0040_0000 != 0;
        let write_back = instr & 0x0020_0000 != 0;
        let load = instr & 0x0010_0000 != 0;
        let rn = (instr >> 16 & 15) as usize;
        let rd = (instr >> 12 & 15) as usize;
        let offset = if instr & 0x0200_0000 != 0 {
            // A register, shifted by a constant only.
            let v = self.operand((instr & 15) as usize, true, 0);
            let amount = instr >> 7 & 31;
            let kind = instr >> 5 & 3;
            let carry = self.psr & F_C != 0;
            match (kind, amount) {
                (0, 0) => v,
                (1 | 2, 0) => shift(v, kind, 32, carry).0,
                (3, 0) => (v >> 1) | (u32::from(carry) << 31),
                _ => shift(v, kind, amount, carry).0,
            }
        } else {
            instr & 0xfff
        };
        let base = self.operand(rn, false, 0);
        let moved = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let addr = if pre { moved } else { base };
        // T: post-index with W forces the user view of memory.
        let privileged = self.privileged() && (pre || !write_back);
        self.fetch_n = true;
        if addr >= ADDRESS_LIMIT {
            self.trap(0x14, MODE_SVC, F_I, self.pc, bus);
            return true;
        }
        let stores_base = (!pre || write_back) && rn != 15;
        if load {
            let value = if byte {
                bus.read_byte(addr, false, privileged).map(u32::from)
            } else {
                bus.read_word(addr & !3, false, privileged)
                    .map(|w| w.rotate_right((addr & 3) * 8))
            };
            let Some(value) = value else {
                self.trap(0x10, MODE_SVC, F_I, self.pc, bus);
                return true;
            };
            bus.idle(1);
            self.fetch_n = false;
            if stores_base {
                self.r[rn] = moved;
            }
            if rd == 15 {
                self.branch(value, bus);
                return true;
            }
            self.r[rd] = value;
        } else {
            let value = self.operand(rd, true, 4);
            let done = if byte {
                bus.write_byte(addr, value as u8, false, privileged)
            } else {
                // The address goes out whole: MEMC decodes A1–A0 of a write.
                bus.write_word(addr, value, false, privileged)
            };
            if !done {
                self.trap(0x10, MODE_SVC, F_I, self.pc, bus);
                return true;
            }
            if stores_base {
                self.r[rn] = moved;
            }
        }
        false
    }

    /// LDM, STM.
    fn block_transfer<B: ArmBus>(&mut self, instr: u32, bus: &mut B) -> bool {
        let pre = instr & 0x0100_0000 != 0;
        let up = instr & 0x0080_0000 != 0;
        let s = instr & 0x0040_0000 != 0;
        let write_back = instr & 0x0020_0000 != 0;
        let load = instr & 0x0010_0000 != 0;
        let rn = (instr >> 16 & 15) as usize;
        let mut list = instr & 0xffff;
        // An empty list transfers R15 and moves the base by sixteen words.
        let span = if list == 0 {
            list = 0x8000;
            16
        } else {
            list.count_ones()
        };
        let base = self.operand(rn, true, 0);
        let lowest = if up {
            base.wrapping_add(if pre { 4 } else { 0 })
        } else {
            base.wrapping_sub(span * 4)
                .wrapping_add(if pre { 0 } else { 4 })
        };
        let new_base = if up {
            base.wrapping_add(span * 4)
        } else {
            base.wrapping_sub(span * 4)
        };
        let privileged = self.privileged();
        let with_r15 = list & 0x8000 != 0;
        let user_bank = s && privileged && !(load && with_r15);
        let writes_base = write_back && rn != 15;
        self.fetch_n = !load;
        if lowest >= ADDRESS_LIMIT {
            self.trap(0x14, MODE_SVC, F_I, self.pc, bus);
            return true;
        }
        let mut aborted = false;
        let mut addr = lowest & !3;
        let mut seq = false;
        let mut new_pc = None;
        for (index, n) in (0..16usize).filter(|n| list >> n & 1 != 0).enumerate() {
            if load {
                let word = if aborted {
                    bus.read_word(addr & (ADDRESS_LIMIT - 1), seq, privileged);
                    None
                } else {
                    bus.read_word(addr & (ADDRESS_LIMIT - 1), seq, privileged)
                };
                aborted |= word.is_none();
                if let Some(w) = word {
                    match n {
                        15 => new_pc = Some(w),
                        _ if user_bank => self.set_user_reg(n, w),
                        _ => self.r[n] = w,
                    }
                }
            } else {
                let value = match n {
                    15 => self.operand(15, true, 4),
                    _ if user_bank => self.user_reg(n),
                    _ => self.r[n],
                };
                if !bus.write_word(addr & (ADDRESS_LIMIT - 1), value, seq, privileged) {
                    aborted = true;
                }
            }
            if index == 0 && writes_base {
                // The base changes at the end of the second cycle, so the
                // first stored register still shows the old value.
                if !load || !aborted {
                    self.r[rn] = new_base;
                }
            }
            addr = addr.wrapping_add(4);
            seq = true;
        }
        if load {
            bus.idle(1);
            self.fetch_n = false;
        }
        if aborted {
            if writes_base {
                self.r[rn] = new_base;
            }
            self.trap(0x10, MODE_SVC, F_I, self.pc, bus);
            return true;
        }
        if let Some(w) = new_pc {
            if s {
                self.write_status(w);
            }
            self.branch(w, bus);
            return true;
        }
        false
    }
}

/// `a - b - !carry` with ARM's flags; a borrow clears C. Subtraction and
/// addition (`b` inverted) share it.
fn sub(a: u32, b: u32, carry: bool) -> (u32, bool, Option<bool>) {
    let wide = u64::from(a) + u64::from(!b) + u64::from(carry);
    let r = wide as u32;
    let v = (a ^ b) & (a ^ r) & 0x8000_0000 != 0;
    (r, wide >> 32 != 0, Some(v))
}

/// The barrel shifter for amounts ≥ 1.
fn shift(v: u32, kind: u32, amount: u32, _carry: bool) -> (u32, bool) {
    match kind {
        0 => match amount {
            1..=31 => (v << amount, v >> (32 - amount) & 1 != 0),
            32 => (0, v & 1 != 0),
            _ => (0, false),
        },
        1 => match amount {
            1..=31 => (v >> amount, v >> (amount - 1) & 1 != 0),
            32 => (0, v >> 31 != 0),
            _ => (0, false),
        },
        2 => {
            if amount < 32 {
                (((v as i32) >> amount) as u32, v >> (amount - 1) & 1 != 0)
            } else {
                (((v as i32) >> 31) as u32, v >> 31 != 0)
            }
        }
        _ => {
            let a = amount & 31;
            if a == 0 {
                (v, v >> 31 != 0)
            } else {
                (v.rotate_right(a), v >> (a - 1) & 1 != 0)
            }
        }
    }
}
