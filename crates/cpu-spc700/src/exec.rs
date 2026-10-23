//! Instruction execution, cycle by cycle.

use crate::{Spc700, SpcBus, B, C, H, I, N, P, V, Z};

impl Spc700 {
    fn fetch(&mut self, bus: &mut dyn SpcBus) -> u8 {
        let pc = self.pc;
        self.pc = pc.wrapping_add(1);
        self.rd(bus, pc)
    }

    fn fetch16(&mut self, bus: &mut dyn SpcBus) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// The dummy read of the next opcode byte in implied instructions.
    fn dummy_pc(&mut self, bus: &mut dyn SpcBus) {
        let pc = self.pc;
        self.rd(bus, pc);
    }

    fn dp(&self, a: u8) -> u16 {
        if self.psw & P != 0 {
            0x100 | u16::from(a)
        } else {
            u16::from(a)
        }
    }

    fn push(&mut self, bus: &mut dyn SpcBus, v: u8) {
        let a = 0x100 | u16::from(self.sp);
        self.wr(bus, a, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn pop(&mut self, bus: &mut dyn SpcBus) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        let a = 0x100 | u16::from(self.sp);
        self.rd(bus, a)
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.psw |= f;
        } else {
            self.psw &= !f;
        }
    }

    fn nz(&mut self, v: u8) {
        self.flag(Z, v == 0);
        self.flag(N, v & 0x80 != 0);
    }

    fn nz16(&mut self, v: u16) {
        self.flag(Z, v == 0);
        self.flag(N, v & 0x8000 != 0);
    }

    fn adc(&mut self, a: u8, m: u8) -> u8 {
        let c = u16::from(self.psw & C);
        let r = u16::from(a) + u16::from(m) + c;
        self.flag(C, r > 0xff);
        self.flag(H, u16::from(a & 0xf) + u16::from(m & 0xf) + c > 0xf);
        self.flag(V, !(a ^ m) & (a ^ r as u8) & 0x80 != 0);
        self.nz(r as u8);
        r as u8
    }

    fn compare(&mut self, reg: u8, m: u8) {
        self.flag(C, reg >= m);
        self.nz(reg.wrapping_sub(m));
    }

    /// The ALU of rows 0–B: OR AND EOR CMP ADC SBC (`group` = op >> 5).
    /// Returns `None` for CMP (nothing to write back).
    fn alu(&mut self, group: u8, a: u8, m: u8) -> Option<u8> {
        let r = match group {
            0 => a | m,
            1 => a & m,
            2 => a ^ m,
            3 => {
                self.compare(a, m);
                return None;
            }
            4 => return Some(self.adc(a, m)),
            _ => return Some(self.adc(a, !m)),
        };
        self.nz(r);
        Some(r)
    }

    fn word(&mut self, bus: &mut dyn SpcBus, d: u8, idle: bool) -> u16 {
        let lo = self.rd(bus, self.dp(d));
        if idle {
            self.idle(bus);
        }
        let hi = self.rd(bus, self.dp(d.wrapping_add(1)));
        u16::from_le_bytes([lo, hi])
    }

    /// The source operand of the ALU columns 4–7 and MOV A,…: returns the
    /// value read.
    fn source(&mut self, bus: &mut dyn SpcBus, op: u8) -> u8 {
        let odd = op & 0x10 != 0;
        match (op & 0x0f, odd) {
            (0x4, false) => {
                let d = self.fetch(bus);
                self.rd(bus, self.dp(d))
            }
            (0x5, false) => {
                let a = self.fetch16(bus);
                self.rd(bus, a)
            }
            (0x6, false) => {
                self.dummy_pc(bus);
                self.rd(bus, self.dp(self.x))
            }
            (0x7, false) => {
                let a = self.ind_x(bus);
                self.rd(bus, a)
            }
            (0x8, false) => self.fetch(bus),
            (0x4, true) => {
                let d = self.fetch(bus);
                self.idle(bus);
                self.rd(bus, self.dp(d.wrapping_add(self.x)))
            }
            (0x5, true) | (0x6, true) => {
                let a = self.fetch16(bus);
                self.idle(bus);
                let i = if op & 0x0f == 5 { self.x } else { self.y };
                self.rd(bus, a.wrapping_add(u16::from(i)))
            }
            _ => {
                let a = self.ind_y(bus, false);
                self.rd(bus, a)
            }
        }
    }

    /// [d+X]: the pointer at d+X.
    fn ind_x(&mut self, bus: &mut dyn SpcBus) -> u16 {
        let d = self.fetch(bus).wrapping_add(self.x);
        self.idle(bus);
        let lo = self.rd(bus, self.dp(d));
        let hi = self.rd(bus, self.dp(d.wrapping_add(1)));
        u16::from_le_bytes([lo, hi])
    }

    /// [d]+Y: the pointer at d plus Y. Reads take their internal cycle
    /// before the pointer, the store MOV [d]+Y,A after it.
    fn ind_y(&mut self, bus: &mut dyn SpcBus, store: bool) -> u16 {
        let d = self.fetch(bus);
        if !store {
            self.idle(bus);
        }
        let lo = self.rd(bus, self.dp(d));
        let hi = self.rd(bus, self.dp(d.wrapping_add(1)));
        if store {
            self.idle(bus);
        }
        u16::from_le_bytes([lo, hi]).wrapping_add(u16::from(self.y))
    }

    /// The destination address of the stores MOV …,A (columns 4–7).
    fn store_addr(&mut self, bus: &mut dyn SpcBus, op: u8) -> u16 {
        match op {
            0xc4 => {
                let d = self.fetch(bus);
                self.dp(d)
            }
            0xc5 => self.fetch16(bus),
            0xc6 => {
                self.dummy_pc(bus);
                self.dp(self.x)
            }
            0xc7 => self.ind_x(bus),
            0xd4 => {
                let d = self.fetch(bus);
                self.idle(bus);
                self.dp(d.wrapping_add(self.x))
            }
            0xd5 | 0xd6 => {
                let a = self.fetch16(bus);
                self.idle(bus);
                let i = if op == 0xd5 { self.x } else { self.y };
                a.wrapping_add(u16::from(i))
            }
            _ => self.ind_y(bus, true),
        }
    }

    fn shift(&mut self, op: u8, v: u8) -> u8 {
        let r = match op >> 5 {
            0 => {
                self.flag(C, v & 0x80 != 0);
                v << 1
            }
            1 => {
                let r = (v << 1) | (self.psw & C);
                self.flag(C, v & 0x80 != 0);
                r
            }
            2 => {
                self.flag(C, v & 1 != 0);
                v >> 1
            }
            3 => {
                let r = (v >> 1) | ((self.psw & C) << 7);
                self.flag(C, v & 1 != 0);
                r
            }
            4 => v.wrapping_sub(1),
            _ => v.wrapping_add(1),
        };
        self.nz(r);
        r
    }

    fn branch(&mut self, bus: &mut dyn SpcBus, rel: u8, taken: bool) {
        if taken {
            self.idle(bus);
            self.idle(bus);
            self.pc = self.pc.wrapping_add(rel as i8 as u16);
        }
    }

    /// The memory bit of OR1/AND1/EOR1/MOV1/NOT1: address, bit number
    /// and the byte read.
    fn mem_bit(&mut self, bus: &mut dyn SpcBus) -> (u16, u8, u8) {
        let w = self.fetch16(bus);
        let (addr, bit) = (w & 0x1fff, (w >> 13) as u8);
        let v = self.rd(bus, addr);
        (addr, bit, v)
    }
}

pub(crate) fn execute(cpu: &mut Spc700, bus: &mut dyn SpcBus) {
    let op = cpu.fetch(bus);
    let lo = op & 0x0f;
    let row = op >> 4;
    match op {
        // ---- ALU rows (OR AND EOR CMP ADC SBC), columns 4–9
        _ if row < 0x0c && (4..=9).contains(&lo) => {
            let group = op >> 5;
            let odd = row & 1 != 0;
            match (lo, odd) {
                (0x8, true) => {
                    // op d,#i
                    let m = cpu.fetch(bus);
                    let d = cpu.fetch(bus);
                    let addr = cpu.dp(d);
                    let v = cpu.rd(bus, addr);
                    match cpu.alu(group, v, m) {
                        Some(r) => cpu.wr(bus, addr, r),
                        None => cpu.idle(bus),
                    }
                }
                (0x9, false) => {
                    // op dd,ds
                    let s = cpu.fetch(bus);
                    let m = cpu.rd(bus, cpu.dp(s));
                    let d = cpu.fetch(bus);
                    let addr = cpu.dp(d);
                    let v = cpu.rd(bus, addr);
                    match cpu.alu(group, v, m) {
                        Some(r) => cpu.wr(bus, addr, r),
                        None => cpu.idle(bus),
                    }
                }
                (0x9, true) => {
                    // op (X),(Y)
                    cpu.dummy_pc(bus);
                    let m = cpu.rd(bus, cpu.dp(cpu.y));
                    let addr = cpu.dp(cpu.x);
                    let v = cpu.rd(bus, addr);
                    match cpu.alu(group, v, m) {
                        Some(r) => cpu.wr(bus, addr, r),
                        None => cpu.idle(bus),
                    }
                }
                _ => {
                    let m = cpu.source(bus, op);
                    let a = cpu.a;
                    if let Some(r) = cpu.alu(group, a, m) {
                        cpu.a = r;
                    }
                }
            }
        }

        // ---- MOV A,…
        0xe4..=0xe8 | 0xf4..=0xf7 => {
            let m = cpu.source(bus, op);
            cpu.a = m;
            cpu.nz(m);
        }
        0xbf => {
            cpu.dummy_pc(bus);
            let m = cpu.rd(bus, cpu.dp(cpu.x));
            cpu.idle(bus);
            cpu.x = cpu.x.wrapping_add(1);
            cpu.a = m;
            cpu.nz(m);
        }

        // ---- MOV …,A
        0xc4..=0xc7 | 0xd4..=0xd7 => {
            let addr = cpu.store_addr(bus, op);
            cpu.rd(bus, addr);
            let a = cpu.a;
            cpu.wr(bus, addr, a);
        }
        0xaf => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            let addr = cpu.dp(cpu.x);
            let a = cpu.a;
            cpu.wr(bus, addr, a);
            cpu.x = cpu.x.wrapping_add(1);
        }

        // ---- MOV X/Y stores
        0xd8 | 0xd9 | 0xc9 | 0xcb | 0xdb | 0xcc => {
            let (addr, v) = match op {
                0xd8 | 0xcb => {
                    let d = cpu.fetch(bus);
                    (cpu.dp(d), if op == 0xd8 { cpu.x } else { cpu.y })
                }
                0xd9 | 0xdb => {
                    let d = cpu.fetch(bus);
                    cpu.idle(bus);
                    let (i, v) = if op == 0xd9 {
                        (cpu.y, cpu.x)
                    } else {
                        (cpu.x, cpu.y)
                    };
                    (cpu.dp(d.wrapping_add(i)), v)
                }
                _ => {
                    let a = cpu.fetch16(bus);
                    (a, if op == 0xc9 { cpu.x } else { cpu.y })
                }
            };
            cpu.rd(bus, addr);
            cpu.wr(bus, addr, v);
        }

        // ---- MOV X/Y loads and CMP X/Y
        0xcd | 0xf8 | 0xf9 | 0xe9 | 0x8d | 0xeb | 0xfb | 0xec | 0xc8 | 0x3e | 0x1e | 0xad
        | 0x7e | 0x5e => {
            let m = match op {
                0xcd | 0x8d | 0xc8 | 0xad => cpu.fetch(bus),
                0xf8 | 0xeb | 0x3e | 0x7e => {
                    let d = cpu.fetch(bus);
                    cpu.rd(bus, cpu.dp(d))
                }
                0xf9 | 0xfb => {
                    let d = cpu.fetch(bus);
                    cpu.idle(bus);
                    let i = if op == 0xf9 { cpu.y } else { cpu.x };
                    cpu.rd(bus, cpu.dp(d.wrapping_add(i)))
                }
                _ => {
                    let a = cpu.fetch16(bus);
                    cpu.rd(bus, a)
                }
            };
            match op {
                0xcd | 0xf8 | 0xf9 | 0xe9 => {
                    cpu.x = m;
                    cpu.nz(m);
                }
                0x8d | 0xeb | 0xfb | 0xec => {
                    cpu.y = m;
                    cpu.nz(m);
                }
                0xc8 | 0x3e | 0x1e => {
                    let x = cpu.x;
                    cpu.compare(x, m);
                }
                _ => {
                    let y = cpu.y;
                    cpu.compare(y, m);
                }
            }
        }

        // ---- MOV d,#i and MOV dd,ds
        0x8f => {
            let m = cpu.fetch(bus);
            let d = cpu.fetch(bus);
            let addr = cpu.dp(d);
            cpu.rd(bus, addr);
            cpu.wr(bus, addr, m);
        }
        0xfa => {
            let s = cpu.fetch(bus);
            let m = cpu.rd(bus, cpu.dp(s));
            let d = cpu.fetch(bus);
            let addr = cpu.dp(d);
            cpu.wr(bus, addr, m);
        }

        // ---- shifts, INC, DEC on memory and A
        0x0b | 0x0c | 0x1b | 0x2b | 0x2c | 0x3b | 0x4b | 0x4c | 0x5b | 0x6b | 0x6c | 0x7b
        | 0x8b | 0x8c | 0x9b | 0xab | 0xac | 0xbb => {
            let addr = match lo {
                0xb if row & 1 == 0 => {
                    let d = cpu.fetch(bus);
                    cpu.dp(d)
                }
                0xb => {
                    let d = cpu.fetch(bus);
                    cpu.idle(bus);
                    cpu.dp(d.wrapping_add(cpu.x))
                }
                _ => cpu.fetch16(bus),
            };
            let v = cpu.rd(bus, addr);
            let r = cpu.shift(op, v);
            cpu.wr(bus, addr, r);
        }
        0x1c | 0x3c | 0x5c | 0x7c | 0x9c | 0xbc => {
            cpu.dummy_pc(bus);
            let a = cpu.a;
            cpu.a = cpu.shift(op, a);
        }
        0x1d | 0x3d | 0xdc | 0xfc => {
            cpu.dummy_pc(bus);
            let r = match op {
                0x1d => {
                    cpu.x = cpu.x.wrapping_sub(1);
                    cpu.x
                }
                0x3d => {
                    cpu.x = cpu.x.wrapping_add(1);
                    cpu.x
                }
                0xdc => {
                    cpu.y = cpu.y.wrapping_sub(1);
                    cpu.y
                }
                _ => {
                    cpu.y = cpu.y.wrapping_add(1);
                    cpu.y
                }
            };
            cpu.nz(r);
        }

        // ---- transfers
        0x5d | 0x7d | 0x9d | 0xbd | 0xdd | 0xfd => {
            cpu.dummy_pc(bus);
            match op {
                0x5d => {
                    cpu.x = cpu.a;
                    cpu.nz(cpu.x);
                }
                0x7d => {
                    cpu.a = cpu.x;
                    cpu.nz(cpu.a);
                }
                0x9d => {
                    cpu.x = cpu.sp;
                    cpu.nz(cpu.x);
                }
                0xbd => cpu.sp = cpu.x,
                0xdd => {
                    cpu.a = cpu.y;
                    cpu.nz(cpu.a);
                }
                _ => {
                    cpu.y = cpu.a;
                    cpu.nz(cpu.y);
                }
            }
        }

        // ---- flags
        0x20 | 0x40 | 0x60 | 0x80 | 0xe0 => {
            cpu.dummy_pc(bus);
            match op {
                0x20 => cpu.psw &= !P,
                0x40 => cpu.psw |= P,
                0x60 => cpu.psw &= !C,
                0x80 => cpu.psw |= C,
                _ => cpu.psw &= !(V | H),
            }
        }
        0xa0 | 0xc0 | 0xed => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            match op {
                0xa0 => cpu.psw |= I,
                0xc0 => cpu.psw &= !I,
                _ => cpu.psw ^= C,
            }
        }
        0x00 => cpu.dummy_pc(bus),

        // ---- branches
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xb0 | 0xd0 | 0xf0 | 0x2f => {
            let rel = cpu.fetch(bus);
            let f = cpu.psw;
            let taken = match op {
                0x10 => f & N == 0,
                0x30 => f & N != 0,
                0x50 => f & V == 0,
                0x70 => f & V != 0,
                0x90 => f & C == 0,
                0xb0 => f & C != 0,
                0xd0 => f & Z == 0,
                0xf0 => f & Z != 0,
                _ => true,
            };
            cpu.branch(bus, rel, taken);
        }
        _ if lo == 0x3 => {
            // BBS/BBC d.bit,rel
            let d = cpu.fetch(bus);
            let v = cpu.rd(bus, cpu.dp(d));
            cpu.idle(bus);
            let rel = cpu.fetch(bus);
            let set = v >> (op >> 5) & 1 != 0;
            cpu.branch(bus, rel, set == (row & 1 == 0));
        }
        0x2e | 0xde => {
            let d = cpu.fetch(bus);
            let addr = if op == 0xde {
                cpu.idle(bus);
                cpu.dp(d.wrapping_add(cpu.x))
            } else {
                cpu.dp(d)
            };
            let v = cpu.rd(bus, addr);
            cpu.idle(bus);
            let rel = cpu.fetch(bus);
            let a = cpu.a;
            cpu.branch(bus, rel, a != v);
        }
        0x6e => {
            let d = cpu.fetch(bus);
            let addr = cpu.dp(d);
            let v = cpu.rd(bus, addr).wrapping_sub(1);
            cpu.wr(bus, addr, v);
            let rel = cpu.fetch(bus);
            cpu.branch(bus, rel, v != 0);
        }
        0xfe => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            cpu.y = cpu.y.wrapping_sub(1);
            let rel = cpu.fetch(bus);
            let y = cpu.y;
            cpu.branch(bus, rel, y != 0);
        }

        // ---- jumps and calls
        0x5f => cpu.pc = cpu.fetch16(bus),
        0x1f => {
            let a = cpu.fetch16(bus).wrapping_add(u16::from(cpu.x));
            cpu.idle(bus);
            let lo = cpu.rd(bus, a);
            let hi = cpu.rd(bus, a.wrapping_add(1));
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0x3f => {
            let a = cpu.fetch16(bus);
            cpu.idle(bus);
            let pc = cpu.pc;
            cpu.push(bus, (pc >> 8) as u8);
            cpu.push(bus, pc as u8);
            cpu.idle(bus);
            cpu.idle(bus);
            cpu.pc = a;
        }
        0x4f => {
            let u = cpu.fetch(bus);
            cpu.idle(bus);
            let pc = cpu.pc;
            cpu.push(bus, (pc >> 8) as u8);
            cpu.push(bus, pc as u8);
            cpu.idle(bus);
            cpu.pc = 0xff00 | u16::from(u);
        }
        _ if lo == 0x1 => {
            // TCALL n
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            let pc = cpu.pc;
            cpu.push(bus, (pc >> 8) as u8);
            cpu.push(bus, pc as u8);
            cpu.idle(bus);
            let vector = 0xffde - 2 * u16::from(row);
            let lo = cpu.rd(bus, vector);
            let hi = cpu.rd(bus, vector + 1);
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0x0f => {
            cpu.dummy_pc(bus);
            let pc = cpu.pc;
            cpu.push(bus, (pc >> 8) as u8);
            cpu.push(bus, pc as u8);
            let psw = cpu.psw;
            cpu.push(bus, psw);
            cpu.idle(bus);
            let lo = cpu.rd(bus, 0xffde);
            let hi = cpu.rd(bus, 0xffdf);
            cpu.pc = u16::from_le_bytes([lo, hi]);
            cpu.psw = (cpu.psw | B) & !I;
        }
        0x6f | 0x7f => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            if op == 0x7f {
                cpu.psw = cpu.pop(bus);
            }
            let lo = cpu.pop(bus);
            let hi = cpu.pop(bus);
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }

        // ---- stack
        0x0d | 0x2d | 0x4d | 0x6d => {
            cpu.dummy_pc(bus);
            let v = match op {
                0x0d => cpu.psw,
                0x2d => cpu.a,
                0x4d => cpu.x,
                _ => cpu.y,
            };
            cpu.push(bus, v);
            cpu.idle(bus);
        }
        0x8e | 0xae | 0xce | 0xee => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            let v = cpu.pop(bus);
            match op {
                0x8e => cpu.psw = v,
                0xae => cpu.a = v,
                0xce => cpu.x = v,
                _ => cpu.y = v,
            }
        }

        // ---- single bits
        _ if lo == 0x2 => {
            let d = cpu.fetch(bus);
            let addr = cpu.dp(d);
            let v = cpu.rd(bus, addr);
            let bit = 1 << (op >> 5);
            let r = if row & 1 == 0 { v | bit } else { v & !bit };
            cpu.wr(bus, addr, r);
        }
        0x0e | 0x4e => {
            let a = cpu.fetch16(bus);
            let v = cpu.rd(bus, a);
            cpu.rd(bus, a);
            let acc = cpu.a;
            cpu.nz(acc.wrapping_sub(v));
            let r = if op == 0x0e { v | acc } else { v & !acc };
            cpu.wr(bus, a, r);
        }
        0x0a | 0x2a | 0x4a | 0x6a | 0x8a | 0xaa => {
            let (_, n, v) = cpu.mem_bit(bus);
            let bit = v >> n & 1 != 0;
            let c = cpu.psw & C != 0;
            let r = match op {
                0x0a => c | bit,
                0x2a => c | !bit,
                0x4a => c & bit,
                0x6a => c & !bit,
                0x8a => c ^ bit,
                _ => bit,
            };
            if matches!(op, 0x0a | 0x2a | 0x8a) {
                cpu.idle(bus);
            }
            cpu.flag(C, r);
        }
        0xca => {
            let w = cpu.fetch16(bus);
            let (addr, bit) = (w & 0x1fff, (w >> 13) as u8);
            let v = cpu.rd(bus, addr);
            cpu.idle(bus);
            let r = if cpu.psw & C != 0 {
                v | 1 << bit
            } else {
                v & !(1 << bit)
            };
            cpu.wr(bus, addr, r);
        }
        0xea => {
            let (addr, bit, v) = cpu.mem_bit(bus);
            cpu.wr(bus, addr, v ^ 1 << bit);
        }

        // ---- 16-bit
        0x1a | 0x3a => {
            let d = cpu.fetch(bus);
            let lo_addr = cpu.dp(d);
            let lo = cpu.rd(bus, lo_addr);
            let delta: u16 = if op == 0x3a { 1 } else { 0xffff };
            let lo_new = u16::from(lo).wrapping_add(delta);
            cpu.wr(bus, lo_addr, lo_new as u8);
            let hi_addr = cpu.dp(d.wrapping_add(1));
            let hi = cpu.rd(bus, hi_addr);
            let w = u16::from_le_bytes([lo, hi]).wrapping_add(delta);
            cpu.wr(bus, hi_addr, (w >> 8) as u8);
            cpu.nz16(w);
        }
        0x5a | 0x7a | 0x9a | 0xba => {
            let d = cpu.fetch(bus);
            let w = cpu.word(bus, d, op != 0x5a);
            let ya = cpu.ya();
            match op {
                0x5a => {
                    cpu.flag(C, ya >= w);
                    cpu.nz16(ya.wrapping_sub(w));
                }
                0x7a => {
                    let r = u32::from(ya) + u32::from(w);
                    cpu.flag(C, r > 0xffff);
                    cpu.flag(H, (ya & 0xfff) + (w & 0xfff) > 0xfff);
                    cpu.flag(V, !(ya ^ w) & (ya ^ r as u16) & 0x8000 != 0);
                    set_ya(cpu, r as u16);
                }
                0x9a => {
                    let r = ya.wrapping_sub(w);
                    cpu.flag(C, ya >= w);
                    cpu.flag(H, (ya & 0xfff) >= (w & 0xfff));
                    cpu.flag(V, (ya ^ w) & (ya ^ r) & 0x8000 != 0);
                    set_ya(cpu, r);
                }
                _ => set_ya(cpu, w),
            }
        }
        0xda => {
            let d = cpu.fetch(bus);
            let addr = cpu.dp(d);
            cpu.rd(bus, addr);
            let (a, y) = (cpu.a, cpu.y);
            cpu.wr(bus, addr, a);
            let hi = cpu.dp(d.wrapping_add(1));
            cpu.wr(bus, hi, y);
        }

        // ---- MUL DIV XCN DAA DAS
        0xcf => {
            cpu.dummy_pc(bus);
            for _ in 0..7 {
                cpu.idle(bus);
            }
            let r = u16::from(cpu.y) * u16::from(cpu.a);
            cpu.a = r as u8;
            cpu.y = (r >> 8) as u8;
            cpu.nz(cpu.y);
        }
        0x9e => {
            cpu.dummy_pc(bus);
            for _ in 0..10 {
                cpu.idle(bus);
            }
            let ya = u32::from(cpu.ya());
            let x = u32::from(cpu.x);
            cpu.flag(H, (cpu.x & 0xf) <= (cpu.y & 0xf));
            cpu.flag(V, u32::from(cpu.y) >= x);
            if u32::from(cpu.y) < x << 1 {
                cpu.a = (ya / x) as u8;
                cpu.y = (ya % x) as u8;
            } else {
                let t = ya - (x << 9);
                cpu.a = (255 - t / (256 - x)) as u8;
                cpu.y = (x + t % (256 - x)) as u8;
            }
            cpu.nz(cpu.a);
        }
        0x9f => {
            cpu.dummy_pc(bus);
            for _ in 0..3 {
                cpu.idle(bus);
            }
            cpu.a = cpu.a.rotate_left(4);
            cpu.nz(cpu.a);
        }
        0xdf | 0xbe => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            let mut a = cpu.a;
            if op == 0xdf {
                if cpu.psw & C != 0 || a > 0x99 {
                    a = a.wrapping_add(0x60);
                    cpu.psw |= C;
                }
                if cpu.psw & H != 0 || a & 0xf > 9 {
                    a = a.wrapping_add(6);
                }
            } else {
                if cpu.psw & C == 0 || a > 0x99 {
                    a = a.wrapping_sub(0x60);
                    cpu.psw &= !C;
                }
                if cpu.psw & H == 0 || a & 0xf > 9 {
                    a = a.wrapping_sub(6);
                }
            }
            cpu.a = a;
            cpu.nz(a);
        }

        // ---- SLEEP STOP
        0xef | 0xff => {
            cpu.dummy_pc(bus);
            cpu.idle(bus);
            cpu.halted = true;
        }
        _ => unreachable!("opcode {op:02x}"),
    }
}

fn set_ya(cpu: &mut Spc700, v: u16) {
    cpu.a = v as u8;
    cpu.y = (v >> 8) as u8;
    cpu.nz16(v);
}
