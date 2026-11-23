//! Instruction execution. Every machine cycle goes through the bus
//! ([`MBus`]): the fetch, each operand, each memory access and each
//! internal cycle, in the order the hardware has them.

use crate::flag::{C, H, N, Z};
use crate::{MBus, Sm83};

impl Sm83 {
    /// Register 0 B, 1 C, 2 D, 3 E, 4 H, 5 L, 7 A; 6 is (HL), a machine
    /// cycle.
    fn get<B: MBus>(&mut self, bus: &mut B, n: u8) -> u8 {
        match n & 7 {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            6 => self.read(bus, self.hl()),
            _ => self.a,
        }
    }

    fn set<B: MBus>(&mut self, bus: &mut B, n: u8, v: u8) {
        match n & 7 {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            6 => self.write(bus, self.hl(), v),
            _ => self.a = v,
        }
    }

    /// Register pair 0 BC, 1 DE, 2 HL, 3 SP.
    fn pair(&self, n: u8) -> u16 {
        match n & 3 {
            0 => self.bc(),
            1 => self.de(),
            2 => self.hl(),
            _ => self.sp,
        }
    }

    fn set_pair(&mut self, n: u8, v: u16) {
        match n & 3 {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_hl(v),
            _ => self.sp = v,
        }
    }

    fn flag(&self, f: u8) -> bool {
        self.f & f != 0
    }

    fn flags(&mut self, z: bool, n: bool, h: bool, c: bool) {
        self.f = u8::from(z) << 7 | u8::from(n) << 6 | u8::from(h) << 5 | u8::from(c) << 4;
    }

    /// Condition 0 NZ, 1 Z, 2 NC, 3 C.
    fn cond(&self, n: u8) -> bool {
        match n & 3 {
            0 => !self.flag(Z),
            1 => self.flag(Z),
            2 => !self.flag(C),
            _ => self.flag(C),
        }
    }

    /// ADD, ADC, SUB, SBC, AND, XOR, OR, CP with A.
    fn alu(&mut self, op: u8, v: u8) {
        let a = self.a;
        let carry = u8::from(self.flag(C));
        match op & 7 {
            0 | 1 => {
                let c = if op & 7 == 1 { carry } else { 0 };
                let r = u16::from(a) + u16::from(v) + u16::from(c);
                self.flags(r as u8 == 0, false, (a & 15) + (v & 15) + c > 15, r > 0xff);
                self.a = r as u8;
            }
            2 | 3 | 7 => {
                let c = if op & 7 == 3 { carry } else { 0 };
                let r = u16::from(a)
                    .wrapping_sub(u16::from(v))
                    .wrapping_sub(u16::from(c));
                self.flags(r as u8 == 0, true, (a & 15) < (v & 15) + c, r > 0xff);
                if op & 7 != 7 {
                    self.a = r as u8;
                }
            }
            4 => {
                self.a &= v;
                self.flags(self.a == 0, false, true, false);
            }
            5 => {
                self.a ^= v;
                self.flags(self.a == 0, false, false, false);
            }
            _ => {
                self.a |= v;
                self.flags(self.a == 0, false, false, false);
            }
        }
    }

    fn inc(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.f = self.f & C | u8::from(r == 0) << 7 | u8::from(r & 15 == 0) << 5;
        r
    }

    fn dec(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.f = self.f & C | N | u8::from(r == 0) << 7 | u8::from(r & 15 == 15) << 5;
        r
    }

    /// SP + a signed byte, as ADD SP,e and LD HL,SP+e compute it: the
    /// flags are those of the low bytes' addition.
    fn sp_plus(&mut self, e: u8) -> u16 {
        let sp = self.sp;
        let h = (sp & 15) + u16::from(e & 15) > 15;
        let c = (sp & 0xff) + u16::from(e) > 0xff;
        self.flags(false, false, h, c);
        sp.wrapping_add(e as i8 as u16)
    }

    /// The rotates and shifts of the `$CB` page (0 RLC, 1 RRC, 2 RL, 3 RR,
    /// 4 SLA, 5 SRA, 6 SWAP, 7 SRL).
    fn shift(&mut self, op: u8, v: u8) -> u8 {
        let carry = u8::from(self.flag(C));
        let (r, c) = match op & 7 {
            0 => (v.rotate_left(1), v >> 7),
            1 => (v.rotate_right(1), v & 1),
            2 => (v << 1 | carry, v >> 7),
            3 => (v >> 1 | carry << 7, v & 1),
            4 => (v << 1, v >> 7),
            5 => (v >> 1 | v & 0x80, v & 1),
            6 => (v.rotate_left(4), 0),
            _ => (v >> 1, v & 1),
        };
        self.flags(r == 0, false, false, c != 0);
        r
    }

    fn cb<B: MBus>(&mut self, bus: &mut B) {
        let op = self.fetch(bus);
        let reg = op & 7;
        let bit = op >> 3 & 7;
        let v = self.get(bus, reg);
        match op >> 6 {
            0 => {
                let r = self.shift(bit, v);
                self.set(bus, reg, r);
            }
            1 => {
                self.f = self.f & C | H | u8::from(v >> bit & 1 == 0) << 7;
            }
            2 => self.set(bus, reg, v & !(1 << bit)),
            _ => self.set(bus, reg, v | 1 << bit),
        }
    }

    fn daa(&mut self) {
        let mut a = self.a;
        let mut carry = self.flag(C);
        if self.flag(N) {
            if carry {
                a = a.wrapping_sub(0x60);
            }
            if self.flag(H) {
                a = a.wrapping_sub(6);
            }
        } else {
            if carry || a > 0x99 {
                a = a.wrapping_add(0x60);
                carry = true;
            }
            if self.flag(H) || a & 15 > 9 {
                a = a.wrapping_add(6);
            }
        }
        self.f = self.f & N | u8::from(a == 0) << 7 | u8::from(carry) << 4;
        self.a = a;
    }

    pub(crate) fn execute<B: MBus>(&mut self, bus: &mut B, op: u8) {
        let dst = op >> 3 & 7;
        let rp = op >> 4 & 3;
        match op {
            0x00 => {}
            0x10 => {
                // STOP: the byte behind it is skipped.
                self.pc = self.pc.wrapping_add(1);
                self.stopped = !bus.stop();
            }
            0x76 => {
                if !self.ime && bus.pending() & 0x1f != 0 {
                    self.halt_bug = true;
                } else {
                    self.halted = true;
                }
            }
            0x40..=0x7f => {
                let v = self.get(bus, op);
                self.set(bus, dst, v);
            }
            0x80..=0xbf => {
                let v = self.get(bus, op);
                self.alu(dst, v);
            }
            0xcb => self.cb(bus),
            0x01 | 0x11 | 0x21 | 0x31 => {
                let v = self.fetch16(bus);
                self.set_pair(rp, v);
            }
            0x02 | 0x12 => self.write(bus, self.pair(rp), self.a),
            0x0a | 0x1a => self.a = self.read(bus, self.pair(rp)),
            0x22 | 0x32 => {
                let hl = self.hl();
                self.write(bus, hl, self.a);
                self.set_hl(if op == 0x22 {
                    hl.wrapping_add(1)
                } else {
                    hl.wrapping_sub(1)
                });
            }
            0x2a | 0x3a => {
                let hl = self.hl();
                self.a = self.read(bus, hl);
                self.set_hl(if op == 0x2a {
                    hl.wrapping_add(1)
                } else {
                    hl.wrapping_sub(1)
                });
            }
            0x03 | 0x13 | 0x23 | 0x33 => {
                self.set_pair(rp, self.pair(rp).wrapping_add(1));
                self.idle(bus);
            }
            0x0b | 0x1b | 0x2b | 0x3b => {
                self.set_pair(rp, self.pair(rp).wrapping_sub(1));
                self.idle(bus);
            }
            0x09 | 0x19 | 0x29 | 0x39 => {
                let (hl, v) = (self.hl(), self.pair(rp));
                let r = u32::from(hl) + u32::from(v);
                let half = (hl & 0xfff) + (v & 0xfff) > 0xfff;
                self.f = self.f & Z | u8::from(half) << 5 | u8::from(r > 0xffff) << 4;
                self.set_hl(r as u16);
                self.idle(bus);
            }
            0x08 => {
                let addr = self.fetch16(bus);
                let [lo, hi] = self.sp.to_le_bytes();
                self.write(bus, addr, lo);
                self.write(bus, addr.wrapping_add(1), hi);
            }
            0x07 | 0x0f | 0x17 | 0x1f => {
                // RLCA, RRCA, RLA, RRA: Z is cleared.
                self.a = self.shift(dst, self.a);
                self.f &= C;
            }
            0x18 => {
                let e = self.fetch(bus);
                self.pc = self.pc.wrapping_add(e as i8 as u16);
                self.idle(bus);
            }
            0x20 | 0x28 | 0x30 | 0x38 => {
                let e = self.fetch(bus);
                if self.cond(dst) {
                    self.pc = self.pc.wrapping_add(e as i8 as u16);
                    self.idle(bus);
                }
            }
            0x27 => self.daa(),
            0x2f => {
                self.a = !self.a;
                self.f |= N | H;
            }
            0x37 => self.f = self.f & Z | C,
            0x3f => self.f = (self.f & (Z | C)) ^ C,
            _ if op < 0x40 => match op & 7 {
                4 => {
                    let v = self.get(bus, dst);
                    let r = self.inc(v);
                    self.set(bus, dst, r);
                }
                5 => {
                    let v = self.get(bus, dst);
                    let r = self.dec(v);
                    self.set(bus, dst, r);
                }
                _ => {
                    let v = self.fetch(bus);
                    self.set(bus, dst, v);
                }
            },
            0xc1 | 0xd1 | 0xe1 => {
                let v = self.pop(bus);
                self.set_pair(rp, v);
            }
            0xf1 => {
                let v = self.pop(bus);
                self.set_af(v);
            }
            0xc5 | 0xd5 | 0xe5 | 0xf5 => {
                self.idle(bus);
                let v = if op == 0xf5 { self.af() } else { self.pair(rp) };
                self.push(bus, v);
            }
            0xc3 => {
                self.pc = self.fetch16(bus);
                self.idle(bus);
            }
            0xc2 | 0xca | 0xd2 | 0xda => {
                let addr = self.fetch16(bus);
                if self.cond(dst) {
                    self.pc = addr;
                    self.idle(bus);
                }
            }
            0xe9 => self.pc = self.hl(),
            0xcd => {
                let addr = self.fetch16(bus);
                self.idle(bus);
                self.push(bus, self.pc);
                self.pc = addr;
            }
            0xc4 | 0xcc | 0xd4 | 0xdc => {
                let addr = self.fetch16(bus);
                if self.cond(dst) {
                    self.idle(bus);
                    self.push(bus, self.pc);
                    self.pc = addr;
                }
            }
            0xc9 | 0xd9 => {
                self.pc = self.pop(bus);
                self.idle(bus);
                if op == 0xd9 {
                    // RETI: without EI's delay.
                    self.ime = true;
                }
            }
            0xc0 | 0xc8 | 0xd0 | 0xd8 => {
                self.idle(bus);
                if self.cond(dst) {
                    self.pc = self.pop(bus);
                    self.idle(bus);
                }
            }
            0xc7 | 0xcf | 0xd7 | 0xdf | 0xe7 | 0xef | 0xf7 | 0xff => {
                self.idle(bus);
                self.push(bus, self.pc);
                self.pc = u16::from(op & 0x38);
            }
            0xc6 | 0xce | 0xd6 | 0xde | 0xe6 | 0xee | 0xf6 | 0xfe => {
                let v = self.fetch(bus);
                self.alu(dst, v);
            }
            0xe0 => {
                let n = self.fetch(bus);
                self.write(bus, 0xff00 | u16::from(n), self.a);
            }
            0xf0 => {
                let n = self.fetch(bus);
                self.a = self.read(bus, 0xff00 | u16::from(n));
            }
            0xe2 => self.write(bus, 0xff00 | u16::from(self.c), self.a),
            0xf2 => self.a = self.read(bus, 0xff00 | u16::from(self.c)),
            0xea => {
                let addr = self.fetch16(bus);
                self.write(bus, addr, self.a);
            }
            0xfa => {
                let addr = self.fetch16(bus);
                self.a = self.read(bus, addr);
            }
            0xe8 => {
                let e = self.fetch(bus);
                self.sp = self.sp_plus(e);
                self.idle(bus);
                self.idle(bus);
            }
            0xf8 => {
                let e = self.fetch(bus);
                let v = self.sp_plus(e);
                self.set_hl(v);
                self.idle(bus);
            }
            0xf9 => {
                self.sp = self.hl();
                self.idle(bus);
            }
            0xf3 => {
                self.ime = false;
                self.ime_next = false;
            }
            0xfb => self.ime_next = !self.ime,
            // No instruction: the processor locks up.
            _ => self.locked = true,
        }
    }
}
