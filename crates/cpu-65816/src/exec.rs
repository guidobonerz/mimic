//! Instruction execution, cycle by cycle (W65C816S datasheet, table 5-7).

use crate::{Bus816, Cpu65816, C, D, I, M, N, V, X, Z};

/// How the byte after an address is found: data addresses carry into the
/// next bank, direct-page and stack addresses wrap within bank 0.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wrap {
    Long,
    Bank0,
}

#[derive(Clone, Copy)]
struct Ea {
    addr: u32,
    wrap: Wrap,
}

impl Ea {
    fn next(self) -> u32 {
        match self.wrap {
            Wrap::Long => (self.addr + 1) & 0xff_ffff,
            Wrap::Bank0 => (self.addr & 0xff_0000) | ((self.addr + 1) & 0xffff),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Imm,
    Dp,
    DpX,
    DpY,
    Abs,
    AbsX,
    AbsY,
    Long,
    LongX,
    DpInd,
    DpIndX,
    DpIndY,
    DpIndLong,
    DpIndLongY,
    Sr,
    SrIndY,
}

/// Indexed reads take the extra cycle only on a page crossing (or with
/// 16-bit index registers); writes and read-modify-writes always.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

/// The addressing mode of the eight ALU columns (ORA … SBC), by the low
/// five opcode bits.
fn alu_mode(op: u8) -> Mode {
    match op & 0x1f {
        0x01 => Mode::DpIndX,
        0x03 => Mode::Sr,
        0x05 => Mode::Dp,
        0x07 => Mode::DpIndLong,
        0x09 => Mode::Imm,
        0x0d => Mode::Abs,
        0x0f => Mode::Long,
        0x11 => Mode::DpIndY,
        0x12 => Mode::DpInd,
        0x13 => Mode::SrIndY,
        0x15 => Mode::DpX,
        0x17 => Mode::DpIndLongY,
        0x19 => Mode::AbsY,
        0x1d => Mode::AbsX,
        _ => Mode::LongX,
    }
}

impl Cpu65816 {
    fn m8(&self) -> bool {
        self.p & M != 0
    }

    fn x8(&self) -> bool {
        self.p & X != 0
    }

    fn fetch(&mut self, bus: &mut dyn Bus816) -> u8 {
        let a = self.pc24();
        self.pc = self.pc.wrapping_add(1);
        self.rd(bus, a)
    }

    fn fetch16(&mut self, bus: &mut dyn Bus816) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// The address of the last fetched program byte.
    fn last_pc(&self) -> u32 {
        u32::from(self.pbr) << 16 | u32::from(self.pc.wrapping_sub(1))
    }

    /// A direct-page address: in emulation mode with DL = 0 it wraps
    /// within the page.
    fn dp(&self, off: u16) -> u32 {
        if self.e && self.d & 0xff == 0 {
            u32::from((self.d & 0xff00) | (off & 0xff))
        } else {
            u32::from(self.d.wrapping_add(off))
        }
    }

    /// The extra cycle of direct-page modes when DL is not zero.
    fn dl_penalty(&mut self, bus: &mut dyn Bus816) {
        if self.d & 0xff != 0 {
            let a = self.last_pc();
            self.io(bus, a);
        }
    }

    fn set_nz(&mut self, v: u16, wide: bool) {
        let (zero, neg) = if wide {
            (v == 0, v & 0x8000 != 0)
        } else {
            (v & 0xff == 0, v & 0x80 != 0)
        };
        self.p = (self.p & !(Z | N)) | if zero { Z } else { 0 } | if neg { N } else { 0 };
    }

    fn flag(&mut self, f: u8, on: bool) {
        if on {
            self.p |= f;
        } else {
            self.p &= !f;
        }
    }

    fn set_a(&mut self, v: u16) {
        if self.m8() {
            self.a = (self.a & 0xff00) | (v & 0xff);
        } else {
            self.a = v;
        }
    }

    fn acc(&self) -> u16 {
        if self.m8() {
            self.a & 0xff
        } else {
            self.a
        }
    }

    fn read_val(&mut self, bus: &mut dyn Bus816, ea: Ea, wide: bool) -> u16 {
        let lo = self.rd(bus, ea.addr);
        if wide {
            let hi = self.rd(bus, ea.next());
            u16::from_le_bytes([lo, hi])
        } else {
            u16::from(lo)
        }
    }

    fn write_val(&mut self, bus: &mut dyn Bus816, ea: Ea, v: u16, wide: bool) {
        self.wr(bus, ea.addr, v as u8);
        if wide {
            self.wr(bus, ea.next(), (v >> 8) as u8);
        }
    }

    fn data(&self, addr: u16) -> u32 {
        u32::from(self.dbr) << 16 | u32::from(addr)
    }

    /// Indexed data address with the page-crossing cycle.
    fn indexed(&mut self, bus: &mut dyn Bus816, base: u16, idx: u16, access: Access) -> Ea {
        let addr = (self.data(base) + u32::from(idx)) & 0xff_ffff;
        let crossed = (base & 0xff) + (idx & 0xff) > 0xff;
        if access == Access::Write || !self.x8() || crossed {
            let a = self.data((base & 0xff00) | (base.wrapping_add(idx) & 0xff));
            self.io(bus, a);
        }
        Ea {
            addr,
            wrap: Wrap::Long,
        }
    }

    /// Resolve a memory operand (all modes but immediate).
    fn ea(&mut self, bus: &mut dyn Bus816, mode: Mode, access: Access) -> Ea {
        let bank0 = |addr| Ea {
            addr,
            wrap: Wrap::Bank0,
        };
        let long = |addr| Ea {
            addr,
            wrap: Wrap::Long,
        };
        match mode {
            Mode::Imm => unreachable!("immediate has no address"),
            Mode::Dp => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                bank0(self.dp(o))
            }
            Mode::DpX | Mode::DpY => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                let a = self.last_pc();
                self.io(bus, a);
                let idx = if mode == Mode::DpX { self.x } else { self.y };
                bank0(self.dp(o.wrapping_add(idx)))
            }
            Mode::Abs => {
                let aa = self.fetch16(bus);
                long(self.data(aa))
            }
            Mode::AbsX => {
                let aa = self.fetch16(bus);
                self.indexed(bus, aa, self.x, access)
            }
            Mode::AbsY => {
                let aa = self.fetch16(bus);
                self.indexed(bus, aa, self.y, access)
            }
            Mode::Long | Mode::LongX => {
                let aa = self.fetch16(bus);
                let bank = self.fetch(bus);
                let mut addr = u32::from(bank) << 16 | u32::from(aa);
                if mode == Mode::LongX {
                    addr = (addr + u32::from(self.x)) & 0xff_ffff;
                }
                long(addr)
            }
            Mode::DpInd => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                let lo = self.rd(bus, self.dp(o));
                let hi = self.rd(bus, self.dp(o.wrapping_add(1)));
                long(self.data(u16::from_le_bytes([lo, hi])))
            }
            Mode::DpIndX => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                let a = self.last_pc();
                self.io(bus, a);
                let p = o.wrapping_add(self.x);
                let lo = self.rd(bus, self.dp(p));
                let hi = self.rd(bus, self.dp(p.wrapping_add(1)));
                long(self.data(u16::from_le_bytes([lo, hi])))
            }
            Mode::DpIndY => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                let lo = self.rd(bus, self.dp(o));
                let hi = self.rd(bus, self.dp(o.wrapping_add(1)));
                let base = u16::from_le_bytes([lo, hi]);
                self.indexed(bus, base, self.y, access)
            }
            Mode::DpIndLong | Mode::DpIndLongY => {
                let o = u16::from(self.fetch(bus));
                self.dl_penalty(bus);
                let p = |n: u16| u32::from(self.d.wrapping_add(o).wrapping_add(n));
                let (p0, p1, p2) = (p(0), p(1), p(2));
                let lo = self.rd(bus, p0);
                let hi = self.rd(bus, p1);
                let bank = self.rd(bus, p2);
                let mut addr = u32::from(bank) << 16 | u32::from(u16::from_le_bytes([lo, hi]));
                if mode == Mode::DpIndLongY {
                    addr = (addr + u32::from(self.y)) & 0xff_ffff;
                }
                long(addr)
            }
            Mode::Sr => {
                let o = u16::from(self.fetch(bus));
                let a = self.last_pc();
                self.io(bus, a);
                bank0(u32::from(self.s.wrapping_add(o)))
            }
            Mode::SrIndY => {
                let o = u16::from(self.fetch(bus));
                let a = self.last_pc();
                self.io(bus, a);
                let p = self.s.wrapping_add(o);
                let lo = self.rd(bus, u32::from(p));
                let hi = self.rd(bus, u32::from(p.wrapping_add(1)));
                self.io(bus, u32::from(p.wrapping_add(1)));
                let base = self.data(u16::from_le_bytes([lo, hi]));
                long((base + u32::from(self.y)) & 0xff_ffff)
            }
        }
    }

    /// Read an operand of the given width (immediate or memory).
    fn operand(&mut self, bus: &mut dyn Bus816, mode: Mode, wide: bool) -> u16 {
        if mode == Mode::Imm {
            if wide {
                self.fetch16(bus)
            } else {
                u16::from(self.fetch(bus))
            }
        } else {
            let ea = self.ea(bus, mode, Access::Read);
            self.read_val(bus, ea, wide)
        }
    }

    // ---- stack

    fn push(&mut self, bus: &mut dyn Bus816, v: u8) {
        let s = u32::from(self.s);
        self.wr(bus, s, v);
        self.s = if self.e {
            0x0100 | (self.s.wrapping_sub(1) & 0xff)
        } else {
            self.s.wrapping_sub(1)
        };
    }

    fn pull(&mut self, bus: &mut dyn Bus816) -> u8 {
        self.s = if self.e {
            0x0100 | (self.s.wrapping_add(1) & 0xff)
        } else {
            self.s.wrapping_add(1)
        };
        let s = u32::from(self.s);
        self.rd(bus, s)
    }

    /// The 65816's own stack instructions use all 16 bits of S even in
    /// emulation mode (S is put back into page 1 afterwards).
    fn push_new(&mut self, bus: &mut dyn Bus816, v: u8) {
        let s = u32::from(self.s);
        self.wr(bus, s, v);
        self.s = self.s.wrapping_sub(1);
    }

    fn pull_new(&mut self, bus: &mut dyn Bus816) -> u8 {
        self.s = self.s.wrapping_add(1);
        let s = u32::from(self.s);
        self.rd(bus, s)
    }

    fn push16(&mut self, bus: &mut dyn Bus816, v: u16) {
        self.push(bus, (v >> 8) as u8);
        self.push(bus, v as u8);
    }

    fn pull16(&mut self, bus: &mut dyn Bus816) -> u16 {
        let lo = self.pull(bus);
        let hi = self.pull(bus);
        u16::from_le_bytes([lo, hi])
    }

    // ---- arithmetic

    fn adc(&mut self, m: u16) {
        let wide = !self.m8();
        let a = self.acc();
        let (mask, sign) = if wide {
            (0xffffu32, 0x8000u32)
        } else {
            (0xff, 0x80)
        };
        let (a32, m32, c) = (u32::from(a), u32::from(m), u32::from(self.p & C));
        let r = if self.p & D == 0 {
            let r = a32 + m32 + c;
            self.flag(V, !(a32 ^ m32) & (a32 ^ r) & sign != 0);
            self.flag(C, r > mask);
            r & mask
        } else {
            let nibbles = if wide { 4 } else { 2 };
            let mut carry = c;
            let mut r = 0u32;
            for i in 0..nibbles {
                let sh = 4 * i;
                let mut s = ((a32 >> sh) & 0xf) + ((m32 >> sh) & 0xf) + carry;
                if i == nibbles - 1 {
                    let pre = r | ((s & 0xf) << sh);
                    self.flag(V, !(a32 ^ m32) & (a32 ^ pre) & sign != 0);
                }
                if s > 9 {
                    s += 6;
                }
                carry = u32::from(s > 0xf);
                r |= (s & 0xf) << sh;
            }
            self.flag(C, carry != 0);
            r
        };
        self.set_a(r as u16);
        self.set_nz(r as u16, wide);
    }

    fn sbc(&mut self, m: u16) {
        let wide = !self.m8();
        if self.p & D == 0 {
            let inv = if wide { !m } else { !m & 0xff };
            self.adc(inv);
            return;
        }
        let a = self.acc();
        let (mask, sign) = if wide {
            (0xffffu32, 0x8000u32)
        } else {
            (0xff, 0x80)
        };
        let (a32, m32) = (u32::from(a), u32::from(m));
        let bin = a32 + (!m32 & mask) + u32::from(self.p & C);
        self.flag(V, (a32 ^ m32) & (a32 ^ bin) & sign != 0);
        let nibbles = if wide { 4 } else { 2 };
        let mut borrow = 1 - i32::from(self.p & C);
        let mut r = 0u32;
        for i in 0..nibbles {
            let sh = 4 * i;
            let mut s = ((a32 >> sh) & 0xf) as i32 - ((m32 >> sh) & 0xf) as i32 - borrow;
            if s < 0 {
                s -= 6;
                borrow = 1;
            } else {
                borrow = 0;
            }
            r |= ((s as u32) & 0xf) << sh;
        }
        self.flag(C, borrow == 0);
        self.set_a(r as u16);
        self.set_nz(r as u16, wide);
    }

    fn compare(&mut self, reg: u16, m: u16, wide: bool) {
        let r = reg.wrapping_sub(m);
        self.flag(C, reg >= m);
        self.set_nz(r, wide);
    }

    /// The read-modify-write operations; returns the new value.
    fn modify(&mut self, op: u8, v: u16, wide: bool) -> u16 {
        let top = if wide { 0x8000 } else { 0x80 };
        let mask = if wide { 0xffff } else { 0xff };
        let r = match op >> 5 {
            0 if op & 0x0f == 0x04 || op & 0x0f == 0x0c => {
                // TSB ($04/$0C) and TRB ($14/$1C).
                let a = self.acc();
                self.flag(Z, a & v == 0);
                return if op & 0x10 == 0 { v | a } else { v & !a };
            }
            0 => {
                self.flag(C, v & top != 0);
                (v << 1) & mask
            }
            1 => {
                let r = ((v << 1) | u16::from(self.p & C)) & mask;
                self.flag(C, v & top != 0);
                r
            }
            2 => {
                self.flag(C, v & 1 != 0);
                v >> 1
            }
            3 => {
                let r = (v >> 1) | if self.p & C != 0 { top } else { 0 };
                self.flag(C, v & 1 != 0);
                r
            }
            6 => v.wrapping_sub(1) & mask,
            _ => v.wrapping_add(1) & mask,
        };
        self.set_nz(r, wide);
        r
    }
}

/// The hardware or software interrupt entry after its first cycles:
/// pushes (PBR in native mode), the vector.
pub(crate) fn enter(cpu: &mut Cpu65816, bus: &mut dyn Bus816, vector: u16, brk: bool) {
    if !cpu.e {
        cpu.push(bus, cpu.pbr);
    }
    cpu.push16(bus, cpu.pc);
    let p = if cpu.e {
        (cpu.p & !X) | if brk { X } else { 0 }
    } else {
        cpu.p
    };
    cpu.push(bus, p);
    cpu.p = (cpu.p | I) & !D;
    cpu.pbr = 0;
    let lo = cpu.rd(bus, u32::from(vector));
    let hi = cpu.rd(bus, u32::from(vector) + 1);
    cpu.pc = u16::from_le_bytes([lo, hi]);
}

/// Execute one instruction.
pub(crate) fn execute(cpu: &mut Cpu65816, bus: &mut dyn Bus816) {
    cpu.fix_widths();
    run(cpu, bus);
    if cpu.e {
        cpu.s = 0x0100 | (cpu.s & 0xff);
    }
}

fn run(cpu: &mut Cpu65816, bus: &mut dyn Bus816) {
    let op = cpu.fetch(bus);
    let implied = |cpu: &mut Cpu65816, bus: &mut dyn Bus816| {
        let a = cpu.pc24();
        cpu.io(bus, a);
    };
    match op {
        // ---- ALU columns: ORA AND EOR ADC (STA) LDA CMP SBC
        _ if matches!(
            op & 0x1f,
            0x01 | 0x03
                | 0x05
                | 0x07
                | 0x09
                | 0x0d
                | 0x0f
                | 0x11
                | 0x12
                | 0x13
                | 0x15
                | 0x17
                | 0x19
                | 0x1d
                | 0x1f
        ) =>
        {
            if op == 0x89 {
                // BIT #imm: Z only.
                let wide = !cpu.m8();
                let m = cpu.operand(bus, Mode::Imm, wide);
                let a = cpu.acc();
                cpu.flag(Z, a & m == 0);
                return;
            }
            let mode = alu_mode(op);
            let wide = !cpu.m8();
            if op >> 5 == 4 {
                let ea = cpu.ea(bus, mode, Access::Write);
                let a = cpu.a;
                cpu.write_val(bus, ea, a, wide);
                return;
            }
            let m = cpu.operand(bus, mode, wide);
            match op >> 5 {
                0 => {
                    let r = cpu.acc() | m;
                    cpu.set_a(r);
                    cpu.set_nz(r, wide);
                }
                1 => {
                    let r = cpu.acc() & m;
                    cpu.set_a(r);
                    cpu.set_nz(r, wide);
                }
                2 => {
                    let r = cpu.acc() ^ m;
                    cpu.set_a(r);
                    cpu.set_nz(r, wide);
                }
                3 => cpu.adc(m),
                5 => {
                    cpu.set_a(m);
                    cpu.set_nz(m, wide);
                }
                6 => {
                    let a = cpu.acc();
                    cpu.compare(a, m, wide);
                }
                _ => cpu.sbc(m),
            }
        }

        // ---- BIT
        0x24 | 0x2c | 0x34 | 0x3c => {
            let mode = match op {
                0x24 => Mode::Dp,
                0x2c => Mode::Abs,
                0x34 => Mode::DpX,
                _ => Mode::AbsX,
            };
            let wide = !cpu.m8();
            let m = cpu.operand(bus, mode, wide);
            let a = cpu.acc();
            cpu.flag(Z, a & m == 0);
            let (n, v) = if wide { (0x8000, 0x4000) } else { (0x80, 0x40) };
            cpu.flag(N, m & n != 0);
            cpu.flag(V, m & v != 0);
        }

        // ---- LDX LDY CPX CPY
        0xa2 | 0xa6 | 0xae | 0xb6 | 0xbe | 0xa0 | 0xa4 | 0xac | 0xb4 | 0xbc | 0xe0 | 0xe4
        | 0xec | 0xc0 | 0xc4 | 0xcc => {
            let mode = match op & 0x1f {
                0x00 | 0x02 => Mode::Imm,
                0x04 | 0x06 => Mode::Dp,
                0x0c | 0x0e => Mode::Abs,
                0x14 => Mode::DpX,
                0x16 => Mode::DpY,
                0x1c => Mode::AbsX,
                _ => Mode::AbsY,
            };
            let wide = !cpu.x8();
            let m = cpu.operand(bus, mode, wide);
            match op {
                0xa2 | 0xa6 | 0xae | 0xb6 | 0xbe => {
                    cpu.x = m;
                    cpu.set_nz(m, wide);
                }
                0xa0 | 0xa4 | 0xac | 0xb4 | 0xbc => {
                    cpu.y = m;
                    cpu.set_nz(m, wide);
                }
                0xe0 | 0xe4 | 0xec => {
                    let x = cpu.x;
                    cpu.compare(x, m, wide);
                }
                _ => {
                    let y = cpu.y;
                    cpu.compare(y, m, wide);
                }
            }
        }

        // ---- STX STY STZ
        0x86 | 0x8e | 0x96 | 0x84 | 0x8c | 0x94 | 0x64 | 0x74 | 0x9c | 0x9e => {
            let (mode, value, wide) = match op {
                0x86 => (Mode::Dp, cpu.x, !cpu.x8()),
                0x8e => (Mode::Abs, cpu.x, !cpu.x8()),
                0x96 => (Mode::DpY, cpu.x, !cpu.x8()),
                0x84 => (Mode::Dp, cpu.y, !cpu.x8()),
                0x8c => (Mode::Abs, cpu.y, !cpu.x8()),
                0x94 => (Mode::DpX, cpu.y, !cpu.x8()),
                0x64 => (Mode::Dp, 0, !cpu.m8()),
                0x74 => (Mode::DpX, 0, !cpu.m8()),
                0x9c => (Mode::Abs, 0, !cpu.m8()),
                _ => (Mode::AbsX, 0, !cpu.m8()),
            };
            let ea = cpu.ea(bus, mode, Access::Write);
            cpu.write_val(bus, ea, value, wide);
        }

        // ---- read-modify-write on memory
        0x06 | 0x0e | 0x16 | 0x1e | 0x26 | 0x2e | 0x36 | 0x3e | 0x46 | 0x4e | 0x56 | 0x5e
        | 0x66 | 0x6e | 0x76 | 0x7e | 0xc6 | 0xce | 0xd6 | 0xde | 0xe6 | 0xee | 0xf6 | 0xfe
        | 0x04 | 0x0c | 0x14 | 0x1c => {
            let mode = match op & 0x1f {
                0x04 | 0x06 | 0x14 => Mode::Dp,
                0x0c | 0x0e | 0x1c => Mode::Abs,
                0x16 => Mode::DpX,
                _ => Mode::AbsX,
            };
            let wide = !cpu.m8();
            let ea = cpu.ea(bus, mode, Access::Write);
            let v = cpu.read_val(bus, ea, wide);
            let last = if wide { ea.next() } else { ea.addr };
            if cpu.e {
                cpu.wr(bus, ea.addr, v as u8);
            } else {
                cpu.io(bus, last);
            }
            let r = cpu.modify(op, v, wide);
            if wide {
                cpu.wr(bus, ea.next(), (r >> 8) as u8);
            }
            cpu.wr(bus, ea.addr, r as u8);
        }

        // ---- accumulator shifts, INC A, DEC A
        0x0a | 0x2a | 0x4a | 0x6a | 0x1a | 0x3a => {
            implied(cpu, bus);
            let wide = !cpu.m8();
            let code = match op {
                0x1a => 0xe0,
                0x3a => 0xc0,
                _ => op & 0xe0,
            };
            let v = cpu.acc();
            let r = cpu.modify(code, v, wide);
            cpu.set_a(r);
        }

        // ---- branches
        0x10 | 0x30 | 0x50 | 0x70 | 0x90 | 0xb0 | 0xd0 | 0xf0 | 0x80 => {
            let off = cpu.fetch(bus) as i8 as u16;
            let taken = match op {
                0x10 => cpu.p & N == 0,
                0x30 => cpu.p & N != 0,
                0x50 => cpu.p & V == 0,
                0x70 => cpu.p & V != 0,
                0x90 => cpu.p & C == 0,
                0xb0 => cpu.p & C != 0,
                0xd0 => cpu.p & Z == 0,
                0xf0 => cpu.p & Z != 0,
                _ => true,
            };
            if taken {
                let from = cpu.pc;
                let a = cpu.last_pc();
                cpu.io(bus, a);
                cpu.pc = from.wrapping_add(off);
                if cpu.e && (from ^ cpu.pc) & 0xff00 != 0 {
                    cpu.io(bus, a);
                }
            }
        }
        0x82 => {
            // BRL
            let off = cpu.fetch16(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            cpu.pc = cpu.pc.wrapping_add(off);
        }

        // ---- jumps
        0x4c => cpu.pc = cpu.fetch16(bus),
        0x5c => {
            let aa = cpu.fetch16(bus);
            cpu.pbr = cpu.fetch(bus);
            cpu.pc = aa;
        }
        0x6c => {
            let aa = cpu.fetch16(bus);
            let lo = cpu.rd(bus, u32::from(aa));
            let hi = cpu.rd(bus, u32::from(aa.wrapping_add(1)));
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0xdc => {
            let aa = cpu.fetch16(bus);
            let lo = cpu.rd(bus, u32::from(aa));
            let hi = cpu.rd(bus, u32::from(aa.wrapping_add(1)));
            cpu.pbr = cpu.rd(bus, u32::from(aa.wrapping_add(2)));
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0x7c => {
            let aa = cpu.fetch16(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            let p = aa.wrapping_add(cpu.x);
            let bank = u32::from(cpu.pbr) << 16;
            let lo = cpu.rd(bus, bank | u32::from(p));
            let hi = cpu.rd(bus, bank | u32::from(p.wrapping_add(1)));
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0x20 => {
            let aa = cpu.fetch16(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            let ret = cpu.pc.wrapping_sub(1);
            cpu.push16(bus, ret);
            cpu.pc = aa;
        }
        0x22 => {
            let aa = cpu.fetch16(bus);
            let pbr = cpu.pbr;
            let s = u32::from(cpu.s);
            cpu.push_new(bus, pbr);
            cpu.io(bus, s);
            let bank = cpu.fetch(bus);
            let ret = cpu.pc.wrapping_sub(1);
            cpu.push_new(bus, (ret >> 8) as u8);
            cpu.push_new(bus, ret as u8);
            cpu.pbr = bank;
            cpu.pc = aa;
        }
        0xfc => {
            let lo = cpu.fetch(bus);
            let ret = cpu.pc;
            cpu.push_new(bus, (ret >> 8) as u8);
            cpu.push_new(bus, ret as u8);
            let hi = cpu.fetch(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            let p = u16::from_le_bytes([lo, hi]).wrapping_add(cpu.x);
            let bank = u32::from(cpu.pbr) << 16;
            let lo = cpu.rd(bus, bank | u32::from(p));
            let hi = cpu.rd(bus, bank | u32::from(p.wrapping_add(1)));
            cpu.pc = u16::from_le_bytes([lo, hi]);
        }
        0x60 => {
            implied(cpu, bus);
            implied(cpu, bus);
            let pc = cpu.pull16(bus);
            let s = u32::from(cpu.s);
            cpu.io(bus, s);
            cpu.pc = pc.wrapping_add(1);
        }
        0x6b => {
            implied(cpu, bus);
            implied(cpu, bus);
            let lo = cpu.pull_new(bus);
            let hi = cpu.pull_new(bus);
            cpu.pbr = cpu.pull_new(bus);
            cpu.pc = u16::from_le_bytes([lo, hi]).wrapping_add(1);
        }
        0x40 => {
            implied(cpu, bus);
            implied(cpu, bus);
            cpu.p = cpu.pull(bus);
            cpu.fix_widths();
            cpu.pc = cpu.pull16(bus);
            if !cpu.e {
                cpu.pbr = cpu.pull(bus);
            }
        }

        // ---- BRK, COP
        0x00 | 0x02 => {
            cpu.fetch(bus);
            let vector = match (op, cpu.e) {
                (0x00, true) => 0xfffe,
                (0x00, false) => 0xffe6,
                (_, true) => 0xfff4,
                (_, false) => 0xffe4,
            };
            enter(cpu, bus, vector, true);
        }

        // ---- pushes
        0x48 | 0xda | 0x5a | 0x08 | 0x8b | 0x4b | 0x0b => {
            implied(cpu, bus);
            match op {
                0x48 => {
                    let a = cpu.a;
                    if cpu.m8() {
                        cpu.push(bus, a as u8);
                    } else {
                        cpu.push16(bus, a);
                    }
                }
                0xda | 0x5a => {
                    let v = if op == 0xda { cpu.x } else { cpu.y };
                    if cpu.x8() {
                        cpu.push(bus, v as u8);
                    } else {
                        cpu.push16(bus, v);
                    }
                }
                0x08 => {
                    let p = cpu.p;
                    cpu.push(bus, p);
                }
                0x8b => {
                    let b = cpu.dbr;
                    cpu.push(bus, b);
                }
                0x4b => {
                    let k = cpu.pbr;
                    cpu.push(bus, k);
                }
                _ => {
                    let d = cpu.d;
                    cpu.push_new(bus, (d >> 8) as u8);
                    cpu.push_new(bus, d as u8);
                }
            }
        }

        // ---- pulls
        0x68 | 0xfa | 0x7a | 0x28 | 0xab | 0x2b => {
            implied(cpu, bus);
            implied(cpu, bus);
            match op {
                0x68 => {
                    let wide = !cpu.m8();
                    let v = if wide {
                        cpu.pull16(bus)
                    } else {
                        u16::from(cpu.pull(bus))
                    };
                    cpu.set_a(v);
                    cpu.set_nz(v, wide);
                }
                0xfa | 0x7a => {
                    let wide = !cpu.x8();
                    let v = if wide {
                        cpu.pull16(bus)
                    } else {
                        u16::from(cpu.pull(bus))
                    };
                    if op == 0xfa {
                        cpu.x = v;
                    } else {
                        cpu.y = v;
                    }
                    cpu.set_nz(v, wide);
                }
                0x28 => {
                    cpu.p = cpu.pull(bus);
                    cpu.fix_widths();
                }
                0xab => {
                    let b = cpu.pull_new(bus);
                    cpu.dbr = b;
                    cpu.set_nz(u16::from(b), false);
                }
                _ => {
                    let lo = cpu.pull_new(bus);
                    let hi = cpu.pull_new(bus);
                    cpu.d = u16::from_le_bytes([lo, hi]);
                    let d = cpu.d;
                    cpu.set_nz(d, true);
                }
            }
        }

        // ---- PEA PEI PER
        0xf4 => {
            let v = cpu.fetch16(bus);
            cpu.push_new(bus, (v >> 8) as u8);
            cpu.push_new(bus, v as u8);
        }
        0xd4 => {
            let o = u16::from(cpu.fetch(bus));
            cpu.dl_penalty(bus);
            // PEI's pointer does not wrap within the page in emulation mode.
            let p = cpu.d.wrapping_add(o);
            let lo = cpu.rd(bus, u32::from(p));
            let hi = cpu.rd(bus, u32::from(p.wrapping_add(1)));
            cpu.push_new(bus, hi);
            cpu.push_new(bus, lo);
        }
        0x62 => {
            let off = cpu.fetch16(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            let v = cpu.pc.wrapping_add(off);
            cpu.push_new(bus, (v >> 8) as u8);
            cpu.push_new(bus, v as u8);
        }

        // ---- transfers
        0xaa | 0xa8 | 0x8a | 0x98 | 0xba | 0x9a | 0x5b | 0x7b | 0x1b | 0x3b | 0x9b | 0xbb => {
            implied(cpu, bus);
            let (x8, m8) = (cpu.x8(), cpu.m8());
            match op {
                0xaa | 0xa8 => {
                    let v = if x8 { cpu.a & 0xff } else { cpu.a };
                    if op == 0xaa {
                        cpu.x = v;
                    } else {
                        cpu.y = v;
                    }
                    cpu.set_nz(v, !x8);
                }
                0x8a | 0x98 => {
                    let v = if op == 0x8a { cpu.x } else { cpu.y };
                    cpu.set_a(v);
                    cpu.set_nz(v, !m8);
                }
                0xba => {
                    let v = if x8 { cpu.s & 0xff } else { cpu.s };
                    cpu.x = v;
                    cpu.set_nz(v, !x8);
                }
                0x9a => {
                    cpu.s = if cpu.e {
                        0x0100 | (cpu.x & 0xff)
                    } else {
                        cpu.x
                    };
                }
                0x5b => {
                    cpu.d = cpu.a;
                    let d = cpu.d;
                    cpu.set_nz(d, true);
                }
                0x7b => {
                    cpu.a = cpu.d;
                    let a = cpu.a;
                    cpu.set_nz(a, true);
                }
                0x1b => {
                    cpu.s = if cpu.e {
                        0x0100 | (cpu.a & 0xff)
                    } else {
                        cpu.a
                    };
                }
                0x3b => {
                    cpu.a = cpu.s;
                    let a = cpu.a;
                    cpu.set_nz(a, true);
                }
                0x9b => {
                    cpu.y = cpu.x;
                    let y = cpu.y;
                    cpu.set_nz(y, !x8);
                }
                _ => {
                    cpu.x = cpu.y;
                    let x = cpu.x;
                    cpu.set_nz(x, !x8);
                }
            }
        }
        0xeb => {
            implied(cpu, bus);
            implied(cpu, bus);
            cpu.a = cpu.a.rotate_left(8);
            let a = cpu.a;
            cpu.set_nz(a & 0xff, false);
        }

        // ---- flags
        0x18 | 0x38 | 0x58 | 0x78 | 0xd8 | 0xf8 | 0xb8 => {
            implied(cpu, bus);
            match op {
                0x18 => cpu.p &= !C,
                0x38 => cpu.p |= C,
                0x58 => cpu.p &= !I,
                0x78 => cpu.p |= I,
                0xd8 => cpu.p &= !D,
                0xf8 => cpu.p |= D,
                _ => cpu.p &= !V,
            }
        }
        0xc2 | 0xe2 => {
            let v = cpu.fetch(bus);
            let a = cpu.last_pc();
            cpu.io(bus, a);
            if op == 0xc2 {
                cpu.p &= !v;
            } else {
                cpu.p |= v;
            }
            cpu.fix_widths();
        }
        0xfb => {
            implied(cpu, bus);
            let c = cpu.p & C != 0;
            cpu.flag(C, cpu.e);
            cpu.e = c;
            if c {
                cpu.p |= M | X;
            }
            cpu.fix_widths();
        }

        // ---- INX INY DEX DEY
        0xe8 | 0xc8 | 0xca | 0x88 => {
            implied(cpu, bus);
            let wide = !cpu.x8();
            let mask = if wide { 0xffff } else { 0xff };
            let reg = if op == 0xe8 || op == 0xca {
                &mut cpu.x
            } else {
                &mut cpu.y
            };
            *reg = if op == 0xe8 || op == 0xc8 {
                reg.wrapping_add(1)
            } else {
                reg.wrapping_sub(1)
            } & mask;
            let v = *reg;
            cpu.set_nz(v, wide);
        }

        // ---- block moves
        0x54 | 0x44 => {
            let dst = cpu.fetch(bus);
            let src = cpu.fetch(bus);
            cpu.dbr = dst;
            let v = cpu.rd(bus, u32::from(src) << 16 | u32::from(cpu.x));
            let d = u32::from(dst) << 16 | u32::from(cpu.y);
            cpu.wr(bus, d, v);
            cpu.io(bus, d);
            cpu.io(bus, d);
            let step = |r: u16| {
                if op == 0x54 {
                    r.wrapping_add(1)
                } else {
                    r.wrapping_sub(1)
                }
            };
            cpu.x = step(cpu.x);
            cpu.y = step(cpu.y);
            if cpu.x8() {
                cpu.x &= 0xff;
                cpu.y &= 0xff;
            }
            cpu.a = cpu.a.wrapping_sub(1);
            if cpu.a != 0xffff {
                cpu.pc = cpu.pc.wrapping_sub(3);
            }
        }

        // ---- WAI STP NOP WDM
        0xcb | 0xdb => {
            implied(cpu, bus);
            implied(cpu, bus);
            if op == 0xcb {
                cpu.waiting = true;
            } else {
                cpu.stopped = true;
            }
        }
        0xea => implied(cpu, bus),
        0x42 => {
            // WDM: its operand byte is skipped in an internal cycle.
            let a = cpu.pc24();
            cpu.io(bus, a);
            cpu.pc = cpu.pc.wrapping_add(1);
        }
        _ => unreachable!("opcode {op:02x}"),
    }
}
