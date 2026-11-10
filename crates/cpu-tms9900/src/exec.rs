//! Decoding and executing the 9900's instructions (Data Manual, section 3:
//! formats I–IX; Table 3 for the clock cycles).

use crate::{st, External, Tms9900, Tms9900Bus};

/// An operand: its address, and whether it is a byte.
#[derive(Clone, Copy)]
struct Operand {
    addr: u16,
    byte: bool,
}

/// Odd parity of a byte (OP set when the number of ones is odd).
fn parity(v: u8) -> bool {
    v.count_ones() % 2 == 1
}

impl Tms9900 {
    fn set(&mut self, bit: u16, on: bool) {
        if on {
            self.st |= bit;
        } else {
            self.st &= !bit;
        }
    }

    /// L> A> EQ from comparing a word with zero.
    fn lae(&mut self, v: u16) {
        self.set(st::LGT, v != 0);
        self.set(st::AGT, (v as i16) > 0);
        self.set(st::EQ, v == 0);
    }

    /// L> A> EQ and OP of a byte.
    fn lae_byte(&mut self, v: u8) {
        self.set(st::LGT, v != 0);
        self.set(st::AGT, (v as i8) > 0);
        self.set(st::EQ, v == 0);
        self.set(st::OP, parity(v));
    }

    /// Flags of a word or byte result.
    fn lae_of(&mut self, v: u16, byte: bool) {
        if byte {
            self.lae_byte(v as u8);
        } else {
            self.lae(v);
        }
    }

    /// The address of operand `mode`/`reg` (Ts/S or Td/D); the address
    /// modification time goes to the cycle count (Data Manual, Table 3).
    fn operand<B: Tms9900Bus>(&mut self, bus: &mut B, mode: u16, reg: u16, byte: bool) -> Operand {
        let ra = self.reg_addr(reg);
        let addr = match mode {
            0 => ra,
            1 => {
                self.cycles += 4;
                self.read(bus, ra)
            }
            2 => {
                self.cycles += 8;
                let sym = self.fetch(bus);
                if reg == 0 {
                    sym
                } else {
                    sym.wrapping_add(self.read(bus, ra))
                }
            }
            _ => {
                self.cycles += if byte { 6 } else { 8 };
                let a = self.read(bus, ra);
                self.write(bus, ra, a.wrapping_add(if byte { 1 } else { 2 }));
                a
            }
        };
        Operand { addr, byte }
    }

    /// The value of an operand (a byte in the low 8 bits).
    fn get<B: Tms9900Bus>(&mut self, bus: &mut B, o: Operand) -> u16 {
        let w = self.read(bus, o.addr);
        match (o.byte, o.addr & 1) {
            (false, _) => w,
            (true, 0) => w >> 8,
            (true, _) => w & 0xff,
        }
    }

    /// Store into an operand whose word was read as `old` (the 9900 reads
    /// every destination first).
    fn put<B: Tms9900Bus>(&mut self, bus: &mut B, o: Operand, old: u16, v: u16) {
        let w = match (o.byte, o.addr & 1) {
            (false, _) => v,
            (true, 0) => (old & 0x00ff) | (v << 8),
            (true, _) => (old & 0xff00) | (v & 0xff),
        };
        self.write(bus, o.addr, w);
    }

    /// Read a destination for read-modify-write: its word and its value.
    fn get_dest<B: Tms9900Bus>(&mut self, bus: &mut B, o: Operand) -> (u16, u16) {
        let w = self.read(bus, o.addr);
        let v = match (o.byte, o.addr & 1) {
            (false, _) => w,
            (true, 0) => w >> 8,
            (true, _) => w & 0xff,
        };
        (w, v)
    }

    fn reg<B: Tms9900Bus>(&mut self, bus: &mut B, r: u16) -> u16 {
        let a = self.reg_addr(r);
        self.read(bus, a)
    }

    fn set_reg_mem<B: Tms9900Bus>(&mut self, bus: &mut B, r: u16, v: u16) {
        let a = self.reg_addr(r);
        self.write(bus, a, v);
    }

    /// The CRU base: bits 3–14 of R12.
    fn cru_base<B: Tms9900Bus>(&mut self, bus: &mut B) -> u16 {
        (self.reg(bus, 12) >> 1) & 0x0fff
    }

    /// Addition with carry and overflow (`bits` 8 or 16).
    fn add(&mut self, a: u16, b: u16, byte: bool) -> u16 {
        let (mask, sign) = if byte { (0xff, 0x80) } else { (0xffff, 0x8000) };
        let sum = u32::from(a & mask) + u32::from(b & mask);
        let r = (sum as u16) & mask;
        self.set(st::C, sum > u32::from(mask));
        self.set(st::OV, (!(a ^ b) & (a ^ r) & sign) != 0);
        self.lae_of(r, byte);
        r
    }

    /// `a - b`: carry is "no borrow".
    fn sub(&mut self, a: u16, b: u16, byte: bool) -> u16 {
        let (mask, sign) = if byte { (0xff, 0x80) } else { (0xffff, 0x8000) };
        let r = a.wrapping_sub(b) & mask;
        self.set(st::C, (a & mask) >= (b & mask));
        self.set(st::OV, ((a ^ b) & (a ^ r) & sign) != 0);
        self.lae_of(r, byte);
        r
    }

    /// C/CB/CI: compare `a` (source) with `b` (destination).
    fn compare(&mut self, a: u16, b: u16, byte: bool) {
        if byte {
            let (a, b) = (a as u8, b as u8);
            self.set(st::LGT, a > b);
            self.set(st::AGT, (a as i8) > (b as i8));
            self.set(st::EQ, a == b);
            self.set(st::OP, parity(a));
        } else {
            self.set(st::LGT, a > b);
            self.set(st::AGT, (a as i16) > (b as i16));
            self.set(st::EQ, a == b);
        }
    }

    pub(crate) fn execute<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        match op >> 12 {
            0x4..=0xf => self.format1(bus, op),
            0x1 => self.jump_or_bit(bus, op),
            0x2 | 0x3 => self.format3(bus, op),
            0x0 => match op >> 8 {
                0x08..=0x0b => self.shift(bus, op),
                0x04..=0x07 => self.format6(bus, op),
                0x02 | 0x03 => self.format8(bus, op),
                _ => self.cycles += 6,
            },
            _ => unreachable!(),
        }
    }

    /// Format I: two general operands.
    fn format1<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let byte = op & 0x1000 != 0;
        let (td, d, ts, s) = ((op >> 10) & 3, (op >> 6) & 15, (op >> 4) & 3, op & 15);
        self.cycles += 14;
        let so = self.operand(bus, ts, s, byte);
        let src = self.get(bus, so);
        let dst = self.operand(bus, td, d, byte);
        let (old, dv) = self.get_dest(bus, dst);
        let mask = if byte { 0xff } else { 0xffff };
        let r = match op >> 13 {
            // SZC(B)
            2 => {
                let r = dv & !src & mask;
                self.lae_of(r, byte);
                r
            }
            // S(B)
            3 => self.sub(dv, src, byte),
            // C(B): no store
            4 => {
                self.compare(src, dv, byte);
                return;
            }
            // A(B)
            5 => self.add(dv, src, byte),
            // MOV(B)
            6 => {
                self.lae_of(src, byte);
                src
            }
            // SOC(B)
            _ => {
                let r = (dv | src) & mask;
                self.lae_of(r, byte);
                r
            }
        };
        self.put(bus, dst, old, r);
    }

    /// Jumps (format II) and the single-bit CRU instructions.
    fn jump_or_bit<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let disp = (op & 0xff) as i8;
        let s = self.st;
        let (lgt, agt, eq) = (s & st::LGT != 0, s & st::AGT != 0, s & st::EQ != 0);
        let take = match op >> 8 {
            0x10 => true,
            0x11 => !agt && !eq,
            0x12 => !lgt || eq,
            0x13 => eq,
            0x14 => lgt || eq,
            0x15 => agt,
            0x16 => !eq,
            0x17 => s & st::C == 0,
            0x18 => s & st::C != 0,
            0x19 => s & st::OV == 0,
            0x1a => !lgt && !eq,
            0x1b => lgt && !eq,
            0x1c => s & st::OP != 0,
            code => {
                // SBO, SBZ, TB
                self.cycles += 12;
                let bit = self.cru_base(bus).wrapping_add(disp as u16) & 0x0fff;
                match code {
                    0x1d => bus.cru_write(bit, true),
                    0x1e => bus.cru_write(bit, false),
                    _ => {
                        let v = bus.cru_read(bit);
                        self.set(st::EQ, v);
                    }
                }
                return;
            }
        };
        if take {
            self.cycles += 10;
            self.pc = self.pc.wrapping_add((i16::from(disp) * 2) as u16);
        } else {
            self.cycles += 8;
        }
    }

    /// COC, CZC, XOR, XOP, LDCR, STCR, MPY, DIV.
    fn format3<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let (d, ts, s) = ((op >> 6) & 15, (op >> 4) & 3, op & 15);
        match op >> 10 {
            0x08..=0x0a => {
                self.cycles += 14;
                let so = self.operand(bus, ts, s, false);
                let src = self.get(bus, so);
                let ra = self.reg_addr(d);
                let dv = self.read(bus, ra);
                match op >> 10 {
                    0x08 => self.set(st::EQ, src & dv == src),
                    0x09 => self.set(st::EQ, src & dv == 0),
                    _ => {
                        let r = src ^ dv;
                        self.lae(r);
                        self.write(bus, ra, r);
                    }
                }
            }
            0x0b => {
                // XOP: vector at >0040 + 4 × D, R11 = the operand's address.
                self.cycles += 36;
                let so = self.operand(bus, ts, s, false);
                let _ = self.get(bus, so);
                let vector = 0x0040 + d * 4;
                let wp = self.read(bus, vector);
                let pc = self.read(bus, vector + 2);
                self.switch_to(bus, wp, pc);
                self.set_reg_mem(bus, 11, so.addr);
                self.st |= st::X;
                self.no_int = true;
            }
            0x0c | 0x0d => {
                let count = if d == 0 { 16 } else { d };
                let byte = count <= 8;
                let base = self.cru_base(bus);
                let so = self.operand(bus, ts, s, byte);
                if op >> 10 == 0x0c {
                    // LDCR
                    self.cycles += 20 + 2 * u32::from(count);
                    let v = self.get(bus, so);
                    for i in 0..count {
                        bus.cru_write((base + i) & 0x0fff, v >> i & 1 != 0);
                    }
                    self.lae_of(v, byte);
                } else {
                    // STCR
                    self.cycles += match count {
                        1..=7 => 42,
                        8 => 44,
                        9..=15 => 58,
                        _ => 60,
                    };
                    let (old, _) = self.get_dest(bus, so);
                    let mut v = 0u16;
                    for i in 0..count {
                        if bus.cru_read((base + i) & 0x0fff) {
                            v |= 1 << i;
                        }
                    }
                    self.lae_of(v, byte);
                    self.put(bus, so, old, v);
                }
            }
            0x0e => {
                // MPY: R(D):R(D+1) = R(D) × source, unsigned.
                self.cycles += 52;
                let so = self.operand(bus, ts, s, false);
                let src = self.get(bus, so);
                let hi_a = self.reg_addr(d);
                let m = self.read(bus, hi_a);
                let p = u32::from(src) * u32::from(m);
                self.write(bus, hi_a, (p >> 16) as u16);
                self.write(bus, hi_a.wrapping_add(2), p as u16);
            }
            _ => {
                // DIV: R(D):R(D+1) / source; overflow when the source is
                // not greater than R(D).
                let so = self.operand(bus, ts, s, false);
                let src = self.get(bus, so);
                let hi_a = self.reg_addr(d);
                let hi = self.read(bus, hi_a);
                if src <= hi {
                    self.cycles += 16;
                    self.st |= st::OV;
                    return;
                }
                self.cycles += 108;
                let lo = self.read(bus, hi_a.wrapping_add(2));
                let n = (u32::from(hi) << 16) | u32::from(lo);
                let (q, r) = (n / u32::from(src), n % u32::from(src));
                self.st &= !st::OV;
                self.write(bus, hi_a, q as u16);
                self.write(bus, hi_a.wrapping_add(2), r as u16);
            }
        }
    }

    /// SRA, SRL, SLA, SRC.
    fn shift<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let w = op & 15;
        let mut n = (op >> 4) & 15;
        if n == 0 {
            self.cycles += 8;
            n = self.reg(bus, 0) & 15;
            if n == 0 {
                n = 16;
            }
        }
        self.cycles += 12 + 2 * u32::from(n);
        let ra = self.reg_addr(w);
        let mut v = self.read(bus, ra);
        let mut carry = false;
        let mut overflow = false;
        for _ in 0..n {
            match op >> 8 {
                0x08 => {
                    carry = v & 1 != 0;
                    v = ((v as i16) >> 1) as u16;
                }
                0x09 => {
                    carry = v & 1 != 0;
                    v >>= 1;
                }
                0x0a => {
                    carry = v & 0x8000 != 0;
                    let r = v << 1;
                    overflow |= (r ^ v) & 0x8000 != 0;
                    v = r;
                }
                _ => {
                    carry = v & 1 != 0;
                    v = v.rotate_right(1);
                }
            }
        }
        self.set(st::C, carry);
        if op >> 8 == 0x0a {
            self.set(st::OV, overflow);
        }
        self.lae(v);
        self.write(bus, ra, v);
    }

    /// Format VI: one general operand.
    fn format6<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let (ts, s) = ((op >> 4) & 3, op & 15);
        let kind = op & 0xffc0;
        if kind >= 0x0780 {
            self.cycles += 6;
            return;
        }
        let o = self.operand(bus, ts, s, false);
        match kind {
            0x0400 => {
                // BLWP
                self.cycles += 26;
                let wp = self.read(bus, o.addr);
                let pc = self.read(bus, o.addr.wrapping_add(2));
                self.switch_to(bus, wp, pc);
                self.no_int = true;
            }
            0x0440 => {
                // B
                self.cycles += 8;
                let _ = self.read(bus, o.addr);
                self.pc = o.addr;
            }
            0x0480 => {
                // X: run the operand as an instruction (its own cycles
                // minus the 4 of a fetch).
                self.cycles += 8;
                let inner = self.read(bus, o.addr);
                let before = self.cycles;
                self.execute(bus, inner);
                self.cycles = self.cycles.max(before + 4) - 4;
            }
            0x0680 => {
                // BL
                self.cycles += 12;
                let _ = self.read(bus, o.addr);
                let ret = self.pc;
                self.set_reg_mem(bus, 11, ret);
                self.pc = o.addr;
            }
            0x0740 => {
                // ABS: flags from the operand; negative values are negated.
                let (_, v) = self.get_dest(bus, o);
                self.lae(v);
                self.st &= !(st::C | st::OV);
                if v & 0x8000 != 0 {
                    self.cycles += 14;
                    self.set(st::OV, v == 0x8000);
                    self.write(bus, o.addr, v.wrapping_neg());
                } else {
                    self.cycles += 12;
                }
            }
            _ => {
                let (old, v) = self.get_dest(bus, o);
                let r = match kind {
                    0x04c0 => {
                        self.cycles += 10;
                        0
                    }
                    0x0500 => {
                        self.cycles += 12;
                        let r = v.wrapping_neg();
                        self.set(st::C, v == 0);
                        self.set(st::OV, v == 0x8000);
                        self.lae(r);
                        r
                    }
                    0x0540 => {
                        self.cycles += 10;
                        let r = !v;
                        self.lae(r);
                        r
                    }
                    0x0580 => {
                        self.cycles += 10;
                        self.add(v, 1, false)
                    }
                    0x05c0 => {
                        self.cycles += 10;
                        self.add(v, 2, false)
                    }
                    0x0600 => {
                        self.cycles += 10;
                        self.sub(v, 1, false)
                    }
                    0x0640 => {
                        self.cycles += 10;
                        self.sub(v, 2, false)
                    }
                    0x06c0 => {
                        self.cycles += 10;
                        v.swap_bytes()
                    }
                    _ => {
                        // SETO
                        self.cycles += 10;
                        0xffff
                    }
                };
                self.put(bus, o, old, r);
            }
        }
    }

    /// Format VIII (immediates, register-only, control).
    fn format8<B: Tms9900Bus>(&mut self, bus: &mut B, op: u16) {
        let w = op & 15;
        match op & 0xffe0 {
            0x0200 => {
                self.cycles += 12;
                let v = self.fetch(bus);
                self.lae(v);
                self.set_reg_mem(bus, w, v);
            }
            k @ (0x0220 | 0x0240 | 0x0260) => {
                self.cycles += 14;
                let imm = self.fetch(bus);
                let v = self.reg(bus, w);
                let r = match k {
                    0x0220 => self.add(v, imm, false),
                    0x0240 => {
                        let r = v & imm;
                        self.lae(r);
                        r
                    }
                    _ => {
                        let r = v | imm;
                        self.lae(r);
                        r
                    }
                };
                self.set_reg_mem(bus, w, r);
            }
            0x0280 => {
                self.cycles += 14;
                let imm = self.fetch(bus);
                let v = self.reg(bus, w);
                self.compare(v, imm, false);
            }
            0x02a0 => {
                self.cycles += 8;
                let v = self.wp;
                self.set_reg_mem(bus, w, v);
            }
            0x02c0 => {
                self.cycles += 8;
                let v = self.st;
                self.set_reg_mem(bus, w, v);
            }
            0x02e0 => {
                self.cycles += 10;
                self.wp = self.fetch(bus) & 0xfffe;
            }
            0x0300 => {
                self.cycles += 16;
                let v = self.fetch(bus);
                self.st = (self.st & !st::MASK) | (v & st::MASK);
            }
            0x0380 => {
                // RTWP
                self.cycles += 14;
                let st_ = self.reg(bus, 15);
                let pc = self.reg(bus, 14);
                let wp = self.reg(bus, 13);
                self.st = st_;
                self.pc = pc & 0xfffe;
                self.wp = wp & 0xfffe;
            }
            k @ (0x0340 | 0x0360 | 0x03a0 | 0x03c0 | 0x03e0) => {
                self.cycles += 12;
                let ext = match k {
                    0x0340 => {
                        self.idle = true;
                        External::Idle
                    }
                    0x0360 => {
                        self.st &= !st::MASK;
                        External::Rset
                    }
                    0x03a0 => External::Ckon,
                    0x03c0 => External::Ckof,
                    _ => External::Lrex,
                };
                bus.external(ext);
            }
            _ => self.cycles += 6,
        }
    }
}
