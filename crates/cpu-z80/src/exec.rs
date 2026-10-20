//! Instruction execution. Opcodes are decoded by their fields
//! `x = op >> 6`, `y = op >> 3 & 7`, `z = op & 7`, `p = y >> 1`, `q = y & 1`
//! (C. Dinu, "Decoding Z80 Opcodes"); DD/FD substitute IX/IY for HL,
//! IXH/IXL for H/L and `(IX+d)` for `(HL)`.

use emu_core::{Bus, Size};

use crate::flag::{C, H, N, PV, S, X, Y, Z};
use crate::{IO_BASE, Z80};

/// Which register stands in for HL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Idx {
    Hl,
    Ix,
    Iy,
}

fn parity(v: u8) -> bool {
    v.count_ones().is_multiple_of(2)
}

/// S, Z and the X/Y copies of a result.
fn sz53(v: u8) -> u8 {
    (v & (S | Y | X)) | if v == 0 { Z } else { 0 }
}

/// S, Z, X/Y and parity of a result.
fn sz53p(v: u8) -> u8 {
    sz53(v) | if parity(v) { PV } else { 0 }
}

/// IM y for ED 46/4E/56/5E/66/6E/76/7E (the undocumented ones act as 0/1).
const IM_MODES: [u8; 8] = [0, 0, 1, 2, 0, 0, 1, 2];

impl Z80 {
    fn set_f(&mut self, f: u8) {
        self.f = f;
        self.flags_written = true;
    }

    /// M1: opcode fetch, refresh counter.
    fn fetch_opcode(&mut self, bus: &mut dyn Bus) -> u8 {
        let op = self.read(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        self.inc_r();
        op
    }

    fn imm8(&mut self, bus: &mut dyn Bus) -> u8 {
        let v = self.read(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn imm16(&mut self, bus: &mut dyn Bus) -> u16 {
        let lo = self.imm8(bus);
        let hi = self.imm8(bus);
        u16::from_le_bytes([lo, hi])
    }

    fn read16(&mut self, bus: &mut dyn Bus, a: u16) -> u16 {
        let lo = self.read(bus, a);
        let hi = self.read(bus, a.wrapping_add(1));
        u16::from_le_bytes([lo, hi])
    }

    fn write16(&mut self, bus: &mut dyn Bus, a: u16, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.write(bus, a, lo);
        self.write(bus, a.wrapping_add(1), hi);
    }

    fn port_in(&mut self, bus: &mut dyn Bus, port: u16) -> u8 {
        bus.read(IO_BASE + u32::from(port), Size::Byte) as u8
    }

    fn port_out(&mut self, bus: &mut dyn Bus, port: u16, v: u8) {
        bus.write(IO_BASE + u32::from(port), Size::Byte, u32::from(v));
    }

    fn idx_reg(&self, idx: Idx) -> u16 {
        match idx {
            Idx::Hl => self.hl(),
            Idx::Ix => self.ix,
            Idx::Iy => self.iy,
        }
    }

    fn set_idx_reg(&mut self, idx: Idx, v: u16) {
        match idx {
            Idx::Hl => self.set_hl(v),
            Idx::Ix => self.ix = v,
            Idx::Iy => self.iy = v,
        }
    }

    /// Register `r` (0–5, 7) with IXH/IXL/IYH/IYL for H/L.
    fn get_r(&self, r: u8, idx: Idx) -> u8 {
        match r {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => (self.idx_reg(idx) >> 8) as u8,
            5 => self.idx_reg(idx) as u8,
            _ => self.a,
        }
    }

    fn set_r(&mut self, r: u8, idx: Idx, v: u8) {
        match r {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => {
                let w = self.idx_reg(idx);
                self.set_idx_reg(idx, (w & 0x00ff) | (u16::from(v) << 8));
            }
            5 => {
                let w = self.idx_reg(idx);
                self.set_idx_reg(idx, (w & 0xff00) | u16::from(v));
            }
            _ => self.a = v,
        }
    }

    /// BC, DE, HL (or IX/IY), SP.
    fn get_rp(&self, p: u8, idx: Idx) -> u16 {
        match p {
            0 => self.bc(),
            1 => self.de(),
            2 => self.idx_reg(idx),
            _ => self.sp,
        }
    }

    fn set_rp(&mut self, p: u8, idx: Idx, v: u16) {
        match p {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_idx_reg(idx, v),
            _ => self.sp = v,
        }
    }

    /// BC, DE, HL (or IX/IY), AF.
    fn get_rp2(&self, p: u8, idx: Idx) -> u16 {
        if p == 3 {
            self.af()
        } else {
            self.get_rp(p, idx)
        }
    }

    fn set_rp2(&mut self, p: u8, idx: Idx, v: u16) {
        if p == 3 {
            self.set_af(v);
        } else {
            self.set_rp(p, idx, v);
        }
    }

    /// NZ, Z, NC, C, PO, PE, P, M.
    fn cond(&self, y: u8) -> bool {
        let f = self.f;
        match y {
            0 => f & Z == 0,
            1 => f & Z != 0,
            2 => f & C == 0,
            3 => f & C != 0,
            4 => f & PV == 0,
            5 => f & PV != 0,
            6 => f & S == 0,
            _ => f & S != 0,
        }
    }

    /// The memory operand for `(HL)`: HL, or IX/IY plus a displacement
    /// fetched now (which also sets MEMPTR).
    fn mem_addr(&mut self, bus: &mut dyn Bus, idx: Idx) -> u16 {
        if idx == Idx::Hl {
            return self.hl();
        }
        let d = self.imm8(bus) as i8;
        let a = self.idx_reg(idx).wrapping_add(d as u16);
        self.wz = a;
        a
    }

    fn jump_relative(&mut self, d: i8) {
        self.pc = self.pc.wrapping_add(d as u16);
        self.wz = self.pc;
    }

    /// One instruction with its prefixes; returns its T-states.
    pub(crate) fn execute(&mut self, bus: &mut dyn Bus) -> u32 {
        let op = self.fetch_opcode(bus);
        self.exec_main(bus, op, Idx::Hl)
    }

    /// DD/FD: the next opcode runs with IX/IY. Before another prefix the
    /// DD/FD acts alone like a 4 T-state NOP.
    fn index_prefix(&mut self, bus: &mut dyn Bus, idx: Idx) -> u32 {
        let next = bus.peek(u32::from(self.pc), Size::Byte) as u8;
        if matches!(next, 0xdd | 0xfd | 0xed) {
            self.prefix_only = true;
            return 4;
        }
        let op = self.fetch_opcode(bus);
        // The prefix counts as an instruction that wrote no flags.
        self.q = 0;
        self.exec_main(bus, op, idx)
    }

    fn exec_main(&mut self, bus: &mut dyn Bus, op: u8, idx: Idx) -> u32 {
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let (p, q) = (y >> 1, y & 1);
        let indexed = idx != Idx::Hl;
        // The DD/FD prefix's own M1 cycle.
        let pre = if indexed { 4 } else { 0 };
        match x {
            0 => match z {
                0 => match y {
                    0 => 4 + pre,
                    1 => {
                        let af = self.af();
                        self.set_af(self.af_);
                        self.af_ = af;
                        4 + pre
                    }
                    2 => {
                        let d = self.imm8(bus) as i8;
                        self.b = self.b.wrapping_sub(1);
                        if self.b != 0 {
                            self.jump_relative(d);
                            13 + pre
                        } else {
                            8 + pre
                        }
                    }
                    3 => {
                        let d = self.imm8(bus) as i8;
                        self.jump_relative(d);
                        12 + pre
                    }
                    _ => {
                        let d = self.imm8(bus) as i8;
                        if self.cond(y - 4) {
                            self.jump_relative(d);
                            12 + pre
                        } else {
                            7 + pre
                        }
                    }
                },
                1 => {
                    if q == 0 {
                        let nn = self.imm16(bus);
                        self.set_rp(p, idx, nn);
                        10 + pre
                    } else {
                        let a = self.idx_reg(idx);
                        let b = self.get_rp(p, idx);
                        self.wz = a.wrapping_add(1);
                        let r = self.add16(a, b);
                        self.set_idx_reg(idx, r);
                        11 + pre
                    }
                }
                2 => match (q, p) {
                    (0, 0 | 1) => {
                        let a = if p == 0 { self.bc() } else { self.de() };
                        self.write(bus, a, self.a);
                        self.wz = (u16::from(self.a) << 8) | (a.wrapping_add(1) & 0xff);
                        7 + pre
                    }
                    (0, 2) => {
                        let nn = self.imm16(bus);
                        let v = self.idx_reg(idx);
                        self.write16(bus, nn, v);
                        self.wz = nn.wrapping_add(1);
                        16 + pre
                    }
                    (0, _) => {
                        let nn = self.imm16(bus);
                        self.write(bus, nn, self.a);
                        self.wz = (u16::from(self.a) << 8) | (nn.wrapping_add(1) & 0xff);
                        13 + pre
                    }
                    (_, 0 | 1) => {
                        let a = if p == 0 { self.bc() } else { self.de() };
                        self.a = self.read(bus, a);
                        self.wz = a.wrapping_add(1);
                        7 + pre
                    }
                    (_, 2) => {
                        let nn = self.imm16(bus);
                        let v = self.read16(bus, nn);
                        self.set_idx_reg(idx, v);
                        self.wz = nn.wrapping_add(1);
                        16 + pre
                    }
                    (_, _) => {
                        let nn = self.imm16(bus);
                        self.a = self.read(bus, nn);
                        self.wz = nn.wrapping_add(1);
                        13 + pre
                    }
                },
                3 => {
                    let v = self.get_rp(p, idx);
                    let v = if q == 0 {
                        v.wrapping_add(1)
                    } else {
                        v.wrapping_sub(1)
                    };
                    self.set_rp(p, idx, v);
                    6 + pre
                }
                4 | 5 => {
                    let step = |s: &mut Self, v: u8| if z == 4 { s.inc8(v) } else { s.dec8(v) };
                    if y == 6 {
                        let a = self.mem_addr(bus, idx);
                        let v = self.read(bus, a);
                        let r = step(self, v);
                        self.write(bus, a, r);
                        if indexed {
                            23
                        } else {
                            11
                        }
                    } else {
                        let v = self.get_r(y, idx);
                        let r = step(self, v);
                        self.set_r(y, idx, r);
                        4 + pre
                    }
                }
                6 => {
                    if y == 6 {
                        let a = self.mem_addr(bus, idx);
                        let n = self.imm8(bus);
                        self.write(bus, a, n);
                        if indexed {
                            19
                        } else {
                            10
                        }
                    } else {
                        let n = self.imm8(bus);
                        self.set_r(y, idx, n);
                        7 + pre
                    }
                }
                _ => {
                    self.accumulator_op(y);
                    4 + pre
                }
            },
            1 => {
                if y == 6 && z == 6 {
                    self.halted = true;
                    4 + pre
                } else if y == 6 {
                    // LD (IX+d),r stores the real H/L.
                    let a = self.mem_addr(bus, idx);
                    let v = self.get_r(z, Idx::Hl);
                    self.write(bus, a, v);
                    if indexed {
                        19
                    } else {
                        7
                    }
                } else if z == 6 {
                    let a = self.mem_addr(bus, idx);
                    let v = self.read(bus, a);
                    self.set_r(y, Idx::Hl, v);
                    if indexed {
                        19
                    } else {
                        7
                    }
                } else {
                    let v = self.get_r(z, idx);
                    self.set_r(y, idx, v);
                    4 + pre
                }
            }
            2 => {
                if z == 6 {
                    let a = self.mem_addr(bus, idx);
                    let v = self.read(bus, a);
                    self.alu(y, v);
                    if indexed {
                        19
                    } else {
                        7
                    }
                } else {
                    let v = self.get_r(z, idx);
                    self.alu(y, v);
                    4 + pre
                }
            }
            _ => match z {
                0 => {
                    if self.cond(y) {
                        self.pc = self.pop(bus);
                        self.wz = self.pc;
                        11 + pre
                    } else {
                        5 + pre
                    }
                }
                1 => {
                    if q == 0 {
                        let v = self.pop(bus);
                        self.set_rp2(p, idx, v);
                        return 10 + pre;
                    }
                    match p {
                        0 => {
                            self.pc = self.pop(bus);
                            self.wz = self.pc;
                            10 + pre
                        }
                        1 => {
                            let (bc, de, hl) = (self.bc(), self.de(), self.hl());
                            self.set_bc(self.bc_);
                            self.set_de(self.de_);
                            self.set_hl(self.hl_);
                            (self.bc_, self.de_, self.hl_) = (bc, de, hl);
                            4 + pre
                        }
                        2 => {
                            self.pc = self.idx_reg(idx);
                            4 + pre
                        }
                        _ => {
                            self.sp = self.idx_reg(idx);
                            6 + pre
                        }
                    }
                }
                2 => {
                    let nn = self.imm16(bus);
                    self.wz = nn;
                    if self.cond(y) {
                        self.pc = nn;
                    }
                    10 + pre
                }
                3 => match y {
                    0 => {
                        let nn = self.imm16(bus);
                        self.pc = nn;
                        self.wz = nn;
                        10 + pre
                    }
                    1 => {
                        if indexed {
                            self.exec_index_cb(bus, idx)
                        } else {
                            let op = self.fetch_opcode(bus);
                            self.exec_cb(bus, op)
                        }
                    }
                    2 => {
                        let n = self.imm8(bus);
                        let port = u16::from_be_bytes([self.a, n]);
                        self.port_out(bus, port, self.a);
                        self.wz = (u16::from(self.a) << 8) | u16::from(n.wrapping_add(1));
                        11 + pre
                    }
                    3 => {
                        let n = self.imm8(bus);
                        let port = u16::from_be_bytes([self.a, n]);
                        self.a = self.port_in(bus, port);
                        self.wz = port.wrapping_add(1);
                        11 + pre
                    }
                    4 => {
                        let v = self.read16(bus, self.sp);
                        let r = self.idx_reg(idx);
                        // High byte first on the write-back.
                        let [lo, hi] = r.to_le_bytes();
                        self.write(bus, self.sp.wrapping_add(1), hi);
                        self.write(bus, self.sp, lo);
                        self.set_idx_reg(idx, v);
                        self.wz = v;
                        19 + pre
                    }
                    5 => {
                        let (de, hl) = (self.de(), self.hl());
                        self.set_de(hl);
                        self.set_hl(de);
                        4 + pre
                    }
                    6 => {
                        self.iff1 = false;
                        self.iff2 = false;
                        4 + pre
                    }
                    _ => {
                        self.iff1 = true;
                        self.iff2 = true;
                        self.ei = true;
                        4 + pre
                    }
                },
                4 => {
                    let nn = self.imm16(bus);
                    self.wz = nn;
                    if self.cond(y) {
                        let pc = self.pc;
                        self.push(bus, pc);
                        self.pc = nn;
                        17 + pre
                    } else {
                        10 + pre
                    }
                }
                5 => {
                    if q == 0 {
                        let v = self.get_rp2(p, idx);
                        self.push(bus, v);
                        return 11 + pre;
                    }
                    match p {
                        0 => {
                            let nn = self.imm16(bus);
                            self.wz = nn;
                            let pc = self.pc;
                            self.push(bus, pc);
                            self.pc = nn;
                            17 + pre
                        }
                        1 => self.index_prefix(bus, Idx::Ix),
                        2 => {
                            let op = self.fetch_opcode(bus);
                            self.exec_ed(bus, op)
                        }
                        _ => self.index_prefix(bus, Idx::Iy),
                    }
                }
                6 => {
                    let n = self.imm8(bus);
                    self.alu(y, n);
                    7 + pre
                }
                _ => {
                    let pc = self.pc;
                    self.push(bus, pc);
                    self.pc = u16::from(y) * 8;
                    self.wz = self.pc;
                    11 + pre
                }
            },
        }
    }

    /// RLCA, RRCA, RLA, RRA, DAA, CPL, SCF, CCF.
    fn accumulator_op(&mut self, y: u8) {
        let (a, f) = (self.a, self.f);
        let keep = f & (S | Z | PV);
        match y {
            0..=3 => {
                let (r, c) = match y {
                    0 => (a.rotate_left(1), a >> 7),
                    1 => (a.rotate_right(1), a & 1),
                    2 => ((a << 1) | (f & C), a >> 7),
                    _ => ((a >> 1) | ((f & C) << 7), a & 1),
                };
                self.a = r;
                self.set_f(keep | (r & (X | Y)) | c);
            }
            4 => self.daa(),
            5 => {
                self.a = !a;
                self.set_f((f & (S | Z | PV | C)) | H | N | (self.a & (X | Y)));
            }
            6 => {
                let xy = ((self.q ^ f) | a) & (X | Y);
                self.set_f(keep | xy | C);
            }
            _ => {
                let xy = ((self.q ^ f) | a) & (X | Y);
                let h = if f & C != 0 { H } else { 0 };
                self.set_f(keep | xy | h | ((f & C) ^ C));
            }
        }
    }

    fn daa(&mut self) {
        let (a, f) = (self.a, self.f);
        let lo = a & 0x0f;
        let mut diff = 0;
        let mut carry = f & C != 0;
        if f & H != 0 || lo > 9 {
            diff |= 0x06;
        }
        if carry || a > 0x99 {
            diff |= 0x60;
            carry = true;
        }
        let sub = f & N != 0;
        let r = if sub {
            a.wrapping_sub(diff)
        } else {
            a.wrapping_add(diff)
        };
        let half = if sub { f & H != 0 && lo < 6 } else { lo > 9 };
        self.a = r;
        self.set_f(sz53p(r) | (f & N) | if half { H } else { 0 } | if carry { C } else { 0 });
    }

    /// ADD, ADC, SUB, SBC, AND, XOR, OR, CP with A.
    fn alu(&mut self, op: u8, v: u8) {
        match op {
            0 => self.a = self.add8(v, 0),
            1 => self.a = self.add8(v, self.f & C),
            2 => self.a = self.sub8(v, 0),
            3 => self.a = self.sub8(v, self.f & C),
            4 => {
                self.a &= v;
                self.set_f(sz53p(self.a) | H);
            }
            5 => {
                self.a ^= v;
                self.set_f(sz53p(self.a));
            }
            6 => {
                self.a |= v;
                self.set_f(sz53p(self.a));
            }
            _ => {
                self.sub8(v, 0);
                // CP takes X/Y from the operand.
                self.set_f((self.f & !(X | Y)) | (v & (X | Y)));
            }
        }
    }

    fn add8(&mut self, v: u8, carry: u8) -> u8 {
        let a = self.a;
        let r16 = u16::from(a) + u16::from(v) + u16::from(carry);
        let r = r16 as u8;
        let overflow = (a ^ !v) & (a ^ r) & 0x80 != 0;
        self.set_f(
            sz53(r)
                | ((a ^ v ^ r) & H)
                | if overflow { PV } else { 0 }
                | if r16 > 0xff { C } else { 0 },
        );
        r
    }

    fn sub8(&mut self, v: u8, carry: u8) -> u8 {
        let a = self.a;
        let r16 = u16::from(a)
            .wrapping_sub(u16::from(v))
            .wrapping_sub(u16::from(carry));
        let r = r16 as u8;
        let overflow = (a ^ v) & (a ^ r) & 0x80 != 0;
        self.set_f(
            sz53(r)
                | ((a ^ v ^ r) & H)
                | if overflow { PV } else { 0 }
                | N
                | if r16 > 0xff { C } else { 0 },
        );
        r
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.set_f(
            (self.f & C)
                | sz53(r)
                | if v & 0x0f == 0x0f { H } else { 0 }
                | if v == 0x7f { PV } else { 0 },
        );
        r
    }

    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.set_f(
            (self.f & C)
                | sz53(r)
                | N
                | if v & 0x0f == 0 { H } else { 0 }
                | if v == 0x80 { PV } else { 0 },
        );
        r
    }

    /// ADD HL,rr: S, Z, P/V kept.
    fn add16(&mut self, a: u16, b: u16) -> u16 {
        let r32 = u32::from(a) + u32::from(b);
        let r = r32 as u16;
        self.set_f(
            (self.f & (S | Z | PV))
                | (((a ^ b ^ r) >> 8) as u8 & H)
                | ((r >> 8) as u8 & (X | Y))
                | if r32 > 0xffff { C } else { 0 },
        );
        r
    }

    fn adc16(&mut self, a: u16, b: u16) -> u16 {
        let r32 = u32::from(a) + u32::from(b) + u32::from(self.f & C);
        let r = r32 as u16;
        let overflow = (a ^ !b) & (a ^ r) & 0x8000 != 0;
        self.set_f(
            ((r >> 8) as u8 & (S | X | Y))
                | if r == 0 { Z } else { 0 }
                | (((a ^ b ^ r) >> 8) as u8 & H)
                | if overflow { PV } else { 0 }
                | if r32 > 0xffff { C } else { 0 },
        );
        r
    }

    fn sbc16(&mut self, a: u16, b: u16) -> u16 {
        let r32 = u32::from(a)
            .wrapping_sub(u32::from(b))
            .wrapping_sub(u32::from(self.f & C));
        let r = r32 as u16;
        let overflow = (a ^ b) & (a ^ r) & 0x8000 != 0;
        self.set_f(
            ((r >> 8) as u8 & (S | X | Y))
                | if r == 0 { Z } else { 0 }
                | (((a ^ b ^ r) >> 8) as u8 & H)
                | if overflow { PV } else { 0 }
                | N
                | if r32 > 0xffff { C } else { 0 },
        );
        r
    }

    /// RLC, RRC, RL, RR, SLA, SRA, SLL, SRL.
    fn rotate(&mut self, y: u8, v: u8) -> u8 {
        let cin = self.f & C;
        let (r, c) = match y {
            0 => (v.rotate_left(1), v >> 7),
            1 => (v.rotate_right(1), v & 1),
            2 => ((v << 1) | cin, v >> 7),
            3 => ((v >> 1) | (cin << 7), v & 1),
            4 => (v << 1, v >> 7),
            5 => ((v >> 1) | (v & 0x80), v & 1),
            6 => ((v << 1) | 1, v >> 7),
            _ => (v >> 1, v & 1),
        };
        self.set_f(sz53p(r) | c);
        r
    }

    /// BIT y; X/Y come from `xy` (the register, MEMPTR or address high byte).
    fn bit(&mut self, y: u8, v: u8, xy: u8) {
        let set = v & (1 << y) != 0;
        self.set_f(
            (self.f & C)
                | H
                | if set { 0 } else { Z | PV }
                | if y == 7 && set { S } else { 0 }
                | (xy & (X | Y)),
        );
    }

    fn exec_cb(&mut self, bus: &mut dyn Bus, op: u8) -> u32 {
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let mem = z == 6;
        let v = if mem {
            let hl = self.hl();
            self.read(bus, hl)
        } else {
            self.get_r(z, Idx::Hl)
        };
        let r = match x {
            0 => self.rotate(y, v),
            1 => {
                let xy = if mem { (self.wz >> 8) as u8 } else { v };
                self.bit(y, v, xy);
                return if mem { 12 } else { 8 };
            }
            2 => v & !(1 << y),
            _ => v | (1 << y),
        };
        if mem {
            let hl = self.hl();
            self.write(bus, hl, r);
            15
        } else {
            self.set_r(z, Idx::Hl, r);
            8
        }
    }

    /// DD CB d op / FD CB d op: always on `(IX+d)`; except for BIT the
    /// result is also copied to register `z` (undocumented).
    fn exec_index_cb(&mut self, bus: &mut dyn Bus, idx: Idx) -> u32 {
        let d = self.imm8(bus) as i8;
        let op = self.imm8(bus);
        let a = self.idx_reg(idx).wrapping_add(d as u16);
        self.wz = a;
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let v = self.read(bus, a);
        let r = match x {
            0 => self.rotate(y, v),
            1 => {
                self.bit(y, v, (a >> 8) as u8);
                return 20;
            }
            2 => v & !(1 << y),
            _ => v | (1 << y),
        };
        self.write(bus, a, r);
        if z != 6 {
            self.set_r(z, Idx::Hl, r);
        }
        23
    }

    fn exec_ed(&mut self, bus: &mut dyn Bus, op: u8) -> u32 {
        let (x, y, z) = (op >> 6, (op >> 3) & 7, op & 7);
        let (p, q) = (y >> 1, y & 1);
        if x == 2 && z <= 3 && y >= 4 {
            return self.block(bus, y, z);
        }
        if x != 1 {
            return 8;
        }
        match z {
            0 => {
                let bc = self.bc();
                let v = self.port_in(bus, bc);
                self.wz = bc.wrapping_add(1);
                self.set_f(sz53p(v) | (self.f & C));
                // ED 70 (IN F,(C)) only sets the flags.
                if y != 6 {
                    self.set_r(y, Idx::Hl, v);
                }
                12
            }
            1 => {
                let bc = self.bc();
                // ED 71 (OUT (C),0) puts 0 on the NMOS Z80.
                let v = if y == 6 { 0 } else { self.get_r(y, Idx::Hl) };
                self.port_out(bus, bc, v);
                self.wz = bc.wrapping_add(1);
                12
            }
            2 => {
                let hl = self.hl();
                let rp = self.get_rp(p, Idx::Hl);
                self.wz = hl.wrapping_add(1);
                let r = if q == 0 {
                    self.sbc16(hl, rp)
                } else {
                    self.adc16(hl, rp)
                };
                self.set_hl(r);
                15
            }
            3 => {
                let nn = self.imm16(bus);
                if q == 0 {
                    let v = self.get_rp(p, Idx::Hl);
                    self.write16(bus, nn, v);
                } else {
                    let v = self.read16(bus, nn);
                    self.set_rp(p, Idx::Hl, v);
                }
                self.wz = nn.wrapping_add(1);
                20
            }
            4 => {
                let v = self.a;
                self.a = 0;
                self.a = self.sub8(v, 0);
                8
            }
            5 => {
                // RETN and RETI both copy IFF2 to IFF1.
                self.pc = self.pop(bus);
                self.wz = self.pc;
                self.iff1 = self.iff2;
                14
            }
            6 => {
                self.im = IM_MODES[usize::from(y)];
                8
            }
            _ => match y {
                0 => {
                    self.i = self.a;
                    9
                }
                1 => {
                    self.r = self.a;
                    9
                }
                2 | 3 => {
                    self.a = if y == 2 { self.i } else { self.r };
                    let pv = if self.iff2 { PV } else { 0 };
                    self.set_f(sz53(self.a) | pv | (self.f & C));
                    self.p = true;
                    9
                }
                4 | 5 => {
                    let hl = self.hl();
                    let v = self.read(bus, hl);
                    let a = self.a;
                    let (new_v, new_a) = if y == 4 {
                        ((a << 4) | (v >> 4), (a & 0xf0) | (v & 0x0f))
                    } else {
                        ((v << 4) | (a & 0x0f), (a & 0xf0) | (v >> 4))
                    };
                    self.write(bus, hl, new_v);
                    self.a = new_a;
                    self.wz = hl.wrapping_add(1);
                    self.set_f(sz53p(new_a) | (self.f & C));
                    18
                }
                _ => 8,
            },
        }
    }

    /// LDI/LDD/CPI/CPD/INI/IND/OUTI/OUTD (y = 4, 5) and their repeating
    /// forms (y = 6, 7).
    fn block(&mut self, bus: &mut dyn Bus, y: u8, z: u8) -> u32 {
        let up = y & 1 == 0;
        let repeat = y >= 6;
        let step = |v: u16| {
            if up {
                v.wrapping_add(1)
            } else {
                v.wrapping_sub(1)
            }
        };
        // X/Y while repeating come from the high byte of the instruction's
        // address (PC after it is wound back).
        let rewind = |s: &mut Self| {
            s.pc = s.pc.wrapping_sub(2);
            (s.pc >> 8) as u8 & (X | Y)
        };
        match z {
            0 => {
                let hl = self.hl();
                let v = self.read(bus, hl);
                let de = self.de();
                self.write(bus, de, v);
                self.set_hl(step(hl));
                self.set_de(step(de));
                let bc = self.bc().wrapping_sub(1);
                self.set_bc(bc);
                let n = v.wrapping_add(self.a);
                let mut f = (self.f & (S | Z | C))
                    | if bc != 0 { PV } else { 0 }
                    | (n & X)
                    | ((n << 4) & Y);
                let t = if repeat && bc != 0 {
                    f = (f & !(X | Y)) | rewind(self);
                    self.wz = self.pc.wrapping_add(1);
                    21
                } else {
                    16
                };
                self.set_f(f);
                t
            }
            1 => {
                let hl = self.hl();
                let v = self.read(bus, hl);
                let r = self.a.wrapping_sub(v);
                let half = (self.a ^ v ^ r) & H;
                self.set_hl(step(hl));
                let bc = self.bc().wrapping_sub(1);
                self.set_bc(bc);
                self.wz = step(self.wz);
                let n = r.wrapping_sub(u8::from(half != 0));
                let mut f = (r & S)
                    | if r == 0 { Z } else { 0 }
                    | half
                    | if bc != 0 { PV } else { 0 }
                    | N
                    | (self.f & C)
                    | (n & X)
                    | ((n << 4) & Y);
                let t = if repeat && bc != 0 && r != 0 {
                    f = (f & !(X | Y)) | rewind(self);
                    self.wz = self.pc.wrapping_add(1);
                    21
                } else {
                    16
                };
                self.set_f(f);
                t
            }
            2 => {
                let bc = self.bc();
                let v = self.port_in(bus, bc);
                let hl = self.hl();
                self.write(bus, hl, v);
                self.wz = step(bc);
                self.b = self.b.wrapping_sub(1);
                self.set_hl(step(hl));
                let c = if up {
                    self.c.wrapping_add(1)
                } else {
                    self.c.wrapping_sub(1)
                };
                let k = u16::from(v) + u16::from(c);
                self.block_io_end(v, k, repeat, rewind)
            }
            _ => {
                let hl = self.hl();
                let v = self.read(bus, hl);
                self.b = self.b.wrapping_sub(1);
                let bc = self.bc();
                self.port_out(bus, bc, v);
                self.set_hl(step(hl));
                self.wz = step(bc);
                let k = u16::from(v) + u16::from(self.l);
                self.block_io_end(v, k, repeat, rewind)
            }
        }
    }

    /// Flags of INI/IND/OUTI/OUTD (`v` the byte moved, `k` its sum with C±1
    /// or L), and of the repeating forms when they repeat.
    fn block_io_end(
        &mut self,
        v: u8,
        k: u16,
        repeat: bool,
        rewind: impl Fn(&mut Self) -> u8,
    ) -> u32 {
        let b = self.b;
        let mut f = sz53(b)
            | if v & 0x80 != 0 { N } else { 0 }
            | if k > 0xff { H | C } else { 0 }
            | if parity((k as u8 & 7) ^ b) { PV } else { 0 };
        let t = if repeat && b != 0 {
            f = (f & !(X | Y)) | rewind(self);
            self.wz = self.pc.wrapping_add(1);
            let toggle = if f & C != 0 {
                f &= !H;
                if v & 0x80 != 0 {
                    if b & 0x0f == 0 {
                        f |= H;
                    }
                    !parity(b.wrapping_sub(1) & 7)
                } else {
                    if b & 0x0f == 0x0f {
                        f |= H;
                    }
                    !parity(b.wrapping_add(1) & 7)
                }
            } else {
                !parity(b & 7)
            };
            if toggle {
                f ^= PV;
            }
            21
        } else {
            16
        };
        self.set_f(f);
        t
    }
}
