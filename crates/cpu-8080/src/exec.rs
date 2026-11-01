//! Instruction execution. T-states from the 8080 User's Manual's
//! instruction summary: every machine cycle goes through the bus, so the
//! sum is the table's value.

use crate::{status, StatusBus, I8080};

fn parity(v: u8) -> bool {
    v.count_ones() & 1 == 0
}

impl I8080 {
    fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }

    fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }

    /// Register pair 0 BC, 1 DE, 2 HL, 3 SP.
    fn pair(&self, n: u8) -> u16 {
        match n & 3 {
            0 => u16::from_be_bytes([self.b, self.c]),
            1 => u16::from_be_bytes([self.d, self.e]),
            2 => self.hl(),
            _ => self.sp,
        }
    }

    fn set_pair(&mut self, n: u8, v: u16) {
        match n & 3 {
            0 => [self.b, self.c] = v.to_be_bytes(),
            1 => [self.d, self.e] = v.to_be_bytes(),
            2 => self.set_hl(v),
            _ => self.sp = v,
        }
    }

    /// Register 0 B, 1 C, 2 D, 3 E, 4 H, 5 L, 7 A (6 is memory: M).
    fn reg(&self, n: u8) -> u8 {
        match n & 7 {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => self.h,
            5 => self.l,
            _ => self.a,
        }
    }

    fn set_reg8(&mut self, n: u8, v: u8) {
        match n & 7 {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => self.h = v,
            5 => self.l = v,
            _ => self.a = v,
        }
    }

    fn szp(&mut self, v: u8) {
        self.s = v & 0x80 != 0;
        self.z = v == 0;
        self.p = parity(v);
    }

    /// Condition 0 NZ, 1 Z, 2 NC, 3 C, 4 PO, 5 PE, 6 P, 7 M.
    fn cond(&self, n: u8) -> bool {
        match n & 7 {
            0 => !self.z,
            1 => self.z,
            2 => !self.cy,
            3 => self.cy,
            4 => !self.p,
            5 => self.p,
            6 => !self.s,
            _ => self.s,
        }
    }

    fn add(&mut self, v: u8, carry: bool) {
        let c = u8::from(carry);
        let r = u16::from(self.a) + u16::from(v) + u16::from(c);
        self.ac = (self.a & 0xf) + (v & 0xf) + c > 0xf;
        self.cy = r > 0xff;
        self.a = r as u8;
        self.szp(r as u8);
    }

    /// The result of A - v - borrow with its flags; the 8080 adds the
    /// complement, so AC is that addition's carry out of bit 3.
    fn sub(&mut self, v: u8, borrow: bool) -> u8 {
        let b = u8::from(borrow);
        let r = i16::from(self.a) - i16::from(v) - i16::from(b);
        self.ac = (self.a & 0xf) + (!v & 0xf) + (1 - b) > 0xf;
        self.cy = r < 0;
        self.szp(r as u8);
        r as u8
    }

    /// ALU operation 0 ADD, 1 ADC, 2 SUB, 3 SBB, 4 ANA, 5 XRA, 6 ORA, 7 CMP.
    fn alu(&mut self, op: u8, v: u8) {
        match op & 7 {
            0 => self.add(v, false),
            1 => self.add(v, self.cy),
            2 => self.a = self.sub(v, false),
            3 => self.a = self.sub(v, self.cy),
            4 => {
                self.ac = (self.a | v) & 8 != 0;
                self.cy = false;
                self.a &= v;
                self.szp(self.a);
            }
            5 => {
                self.ac = false;
                self.cy = false;
                self.a ^= v;
                self.szp(self.a);
            }
            6 => {
                self.ac = false;
                self.cy = false;
                self.a |= v;
                self.szp(self.a);
            }
            _ => {
                self.sub(v, false);
            }
        }
    }

    fn daa(&mut self) {
        let lo = self.a & 0xf;
        let hi = self.a >> 4;
        let mut add = 0;
        if self.ac || lo > 9 {
            add = 6;
        }
        if self.cy || hi > 9 || (hi == 9 && lo > 9) {
            add |= 0x60;
            self.cy = true;
        }
        self.ac = lo + (add & 0xf) > 0xf;
        self.a = self.a.wrapping_add(add);
        self.szp(self.a);
    }

    /// INR / DCR: S Z AC P change, CY stays.
    fn step_value(&mut self, v: u8, up: bool) -> u8 {
        let r = if up {
            v.wrapping_add(1)
        } else {
            v.wrapping_sub(1)
        };
        self.ac = if up { r & 0xf == 0 } else { r & 0xf != 0xf };
        self.szp(r);
        r
    }

    pub(crate) fn execute<B: StatusBus>(&mut self, bus: &mut B, op: u8) {
        let dst = (op >> 3) & 7;
        let src = op & 7;
        let rp = (op >> 4) & 3;
        match op {
            0x00 | 0x08 | 0x10 | 0x18 | 0x20 | 0x28 | 0x30 | 0x38 => {}
            0x01 | 0x11 | 0x21 | 0x31 => {
                let v = self.operand16(bus);
                self.set_pair(rp, v);
            }
            0x02 | 0x12 => self.mem_write(bus, self.pair(rp), self.a),
            0x0a | 0x1a => self.a = self.mem_read(bus, self.pair(rp)),
            0x03 | 0x13 | 0x23 | 0x33 => {
                self.idle(bus, 1);
                self.set_pair(rp, self.pair(rp).wrapping_add(1));
            }
            0x0b | 0x1b | 0x2b | 0x3b => {
                self.idle(bus, 1);
                self.set_pair(rp, self.pair(rp).wrapping_sub(1));
            }
            0x34 | 0x35 => {
                let addr = self.hl();
                let v = self.mem_read(bus, addr);
                let r = self.step_value(v, op & 1 == 0);
                self.mem_write(bus, addr, r);
            }
            0x04 | 0x0c | 0x14 | 0x1c | 0x24 | 0x2c | 0x3c | 0x05 | 0x0d | 0x15 | 0x1d | 0x25
            | 0x2d | 0x3d => {
                self.idle(bus, 1);
                let r = self.step_value(self.reg(dst), op & 1 == 0);
                self.set_reg8(dst, r);
            }
            0x36 => {
                let v = self.operand(bus);
                self.mem_write(bus, self.hl(), v);
            }
            0x06 | 0x0e | 0x16 | 0x1e | 0x26 | 0x2e | 0x3e => {
                let v = self.operand(bus);
                self.set_reg8(dst, v);
            }
            0x07 => {
                self.cy = self.a & 0x80 != 0;
                self.a = self.a.rotate_left(1);
            }
            0x0f => {
                self.cy = self.a & 1 != 0;
                self.a = self.a.rotate_right(1);
            }
            0x17 => {
                let c = u8::from(self.cy);
                self.cy = self.a & 0x80 != 0;
                self.a = self.a << 1 | c;
            }
            0x1f => {
                let c = u8::from(self.cy);
                self.cy = self.a & 1 != 0;
                self.a = self.a >> 1 | c << 7;
            }
            0x09 | 0x19 | 0x29 | 0x39 => {
                self.idle(bus, 6);
                let r = u32::from(self.hl()) + u32::from(self.pair(rp));
                self.cy = r > 0xffff;
                self.set_hl(r as u16);
            }
            0x22 => {
                let addr = self.operand16(bus);
                self.mem_write(bus, addr, self.l);
                self.mem_write(bus, addr.wrapping_add(1), self.h);
            }
            0x2a => {
                let addr = self.operand16(bus);
                self.l = self.mem_read(bus, addr);
                self.h = self.mem_read(bus, addr.wrapping_add(1));
            }
            0x32 => {
                let addr = self.operand16(bus);
                self.mem_write(bus, addr, self.a);
            }
            0x3a => {
                let addr = self.operand16(bus);
                self.a = self.mem_read(bus, addr);
            }
            0x27 => self.daa(),
            0x2f => self.a = !self.a,
            0x37 => self.cy = true,
            0x3f => self.cy = !self.cy,
            0x76 => {
                self.halted = true;
                let (_, addr, data) = self.last;
                bus.cycle(status::HALT, addr, data, 3);
                self.t += 3;
            }
            0x40..=0x7f => {
                if src == 6 {
                    let v = self.mem_read(bus, self.hl());
                    self.set_reg8(dst, v);
                } else if dst == 6 {
                    self.mem_write(bus, self.hl(), self.reg(src));
                } else {
                    self.idle(bus, 1);
                    self.set_reg8(dst, self.reg(src));
                }
            }
            0x80..=0xbf => {
                let v = if src == 6 {
                    self.mem_read(bus, self.hl())
                } else {
                    self.reg(src)
                };
                self.alu(dst, v);
            }
            0xc0 | 0xc8 | 0xd0 | 0xd8 | 0xe0 | 0xe8 | 0xf0 | 0xf8 => {
                self.idle(bus, 1);
                if self.cond(dst) {
                    self.pc = self.pop(bus);
                }
            }
            0xc1 | 0xd1 | 0xe1 => {
                let v = self.pop(bus);
                self.set_pair(rp, v);
            }
            0xf1 => {
                let v = self.pop(bus);
                self.a = (v >> 8) as u8;
                self.set_psw(v as u8);
            }
            0xc5 | 0xd5 | 0xe5 => {
                self.idle(bus, 1);
                self.push(bus, self.pair(rp));
            }
            0xf5 => {
                self.idle(bus, 1);
                self.push(bus, u16::from_be_bytes([self.a, self.psw()]));
            }
            0xc2 | 0xca | 0xd2 | 0xda | 0xe2 | 0xea | 0xf2 | 0xfa => {
                let addr = self.operand16(bus);
                if self.cond(dst) {
                    self.pc = addr;
                }
            }
            0xc3 | 0xcb => self.pc = self.operand16(bus),
            0xc4 | 0xcc | 0xd4 | 0xdc | 0xe4 | 0xec | 0xf4 | 0xfc => {
                self.idle(bus, 1);
                let addr = self.operand16(bus);
                if self.cond(dst) {
                    self.push(bus, self.pc);
                    self.pc = addr;
                }
            }
            0xcd | 0xdd | 0xed | 0xfd => {
                self.idle(bus, 1);
                let addr = self.operand16(bus);
                self.push(bus, self.pc);
                self.pc = addr;
            }
            0xc6 | 0xce | 0xd6 | 0xde | 0xe6 | 0xee | 0xf6 | 0xfe => {
                let v = self.operand(bus);
                self.alu(dst, v);
            }
            0xc7 | 0xcf | 0xd7 | 0xdf | 0xe7 | 0xef | 0xf7 | 0xff => {
                self.idle(bus, 1);
                self.push(bus, self.pc);
                self.pc = u16::from(op & 0x38);
            }
            0xc9 | 0xd9 => self.pc = self.pop(bus),
            0xd3 => {
                let port = self.operand(bus);
                self.io_write(bus, port, self.a);
            }
            0xdb => {
                let port = self.operand(bus);
                self.a = self.io_read(bus, port);
            }
            0xe3 => {
                let at = self.sp.wrapping_add(1);
                let lo = self.stack_read(bus, self.sp);
                let hi = self.stack_read(bus, at);
                self.stack_write(bus, at, self.h);
                self.stack_write(bus, self.sp, self.l);
                self.idle(bus, 2);
                self.h = hi;
                self.l = lo;
            }
            0xe9 => {
                self.idle(bus, 1);
                self.pc = self.hl();
            }
            0xeb => {
                std::mem::swap(&mut self.d, &mut self.h);
                std::mem::swap(&mut self.e, &mut self.l);
            }
            0xf3 => self.inte = false,
            0xf9 => {
                self.idle(bus, 1);
                self.sp = self.hl();
            }
            0xfb => {
                self.inte = true;
                self.ei_delay = true;
            }
        }
    }
}
