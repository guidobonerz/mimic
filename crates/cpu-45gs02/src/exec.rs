//! Instruction execution: decoding the prefixes, the addressing modes, the
//! 8-bit, 16-bit and 32-bit (Q) operations and the cycle counts.

use emu_core::{Bus, Size};

use crate::flag::{B as FB, C, D, E, I, N, V, Z};
use crate::table::{Mn, Op, COMPOUND, OPS};
use crate::{Cpu45gs02, Mode, Speed, ADDR_MASK};

/// An effective address: a CPU address (through MAP) or a flat one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ea {
    Cpu(u16),
    Flat(u32),
}

impl Ea {
    fn plus(self, k: u16) -> Ea {
        match self {
            Ea::Cpu(a) => Ea::Cpu(a.wrapping_add(k)),
            Ea::Flat(a) => Ea::Flat(a.wrapping_add(u32::from(k)) & ADDR_MASK),
        }
    }
}

/// The `(zp),Z` opcodes, which the `EA` prefix makes flat.
pub(crate) fn is_izz(op: u8) -> bool {
    op & 0x1f == 0x12
}

/// Opcodes with a defined Q form after `42 42` (from the guide's table:
/// not the reserved RESQ/RSVQ ones).
pub(crate) static Q_OPS: [bool; 256] = q_ops();

const fn q_ops() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < COMPOUND.len() {
        let (code, text, _, _) = COMPOUND[i];
        let b = text.as_bytes();
        let reserved = b[0] == b'R' && b[1] == b'E' && b[2] == b'S'
            || b[0] == b'R' && b[1] == b'S' && b[2] == b'V';
        if code >> 8 == 0x4242 && !reserved {
            t[(code & 0xff) as usize] = true;
        }
        i += 1;
    }
    t
}

/// The cycles and flags of a compound instruction from the guide's table.
fn compound_cycles(code: u32) -> Option<(u8, u8)> {
    COMPOUND
        .iter()
        .find(|c| c.0 == code)
        .map(|&(_, _, n, f)| (n, f))
}

/// What the prefixes asked for.
#[derive(Clone, Copy, Default)]
struct Pre {
    q: bool,
    flat: bool,
}

impl Cpu45gs02 {
    fn fetch(&mut self, bus: &mut dyn Bus) -> u8 {
        let v = self.rd(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16(&mut self, bus: &mut dyn Bus) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// The code byte `k` after PC, without side effects.
    fn peek_code(&self, bus: &mut dyn Bus, k: u16) -> u8 {
        bus.peek(self.map.translate(self.pc.wrapping_add(k)), Size::Byte) as u8
    }

    fn zp(&self, n: u8) -> u16 {
        u16::from_le_bytes([n, self.b])
    }

    pub(crate) fn read_ea(&self, bus: &mut dyn Bus, ea: Ea) -> u8 {
        match ea {
            Ea::Cpu(a) => self.rd(bus, a),
            Ea::Flat(a) => bus.read(a & ADDR_MASK, Size::Byte) as u8,
        }
    }

    pub(crate) fn write_ea(&self, bus: &mut dyn Bus, ea: Ea, v: u8) {
        match ea {
            Ea::Cpu(a) => self.wr(bus, a, v),
            Ea::Flat(a) => bus.write(a & ADDR_MASK, Size::Byte, u32::from(v)),
        }
    }

    fn read32(&self, bus: &mut dyn Bus, ea: Ea) -> u32 {
        u32::from_le_bytes([
            self.read_ea(bus, ea),
            self.read_ea(bus, ea.plus(1)),
            self.read_ea(bus, ea.plus(2)),
            self.read_ea(bus, ea.plus(3)),
        ])
    }

    fn write32(&self, bus: &mut dyn Bus, ea: Ea, v: u32) {
        for (k, b) in v.to_le_bytes().into_iter().enumerate() {
            self.write_ea(bus, ea.plus(k as u16), b);
        }
    }

    fn read16(&self, bus: &mut dyn Bus, ea: Ea) -> u16 {
        u16::from_le_bytes([self.read_ea(bus, ea), self.read_ea(bus, ea.plus(1))])
    }

    fn set_nz(&mut self, v: u8) {
        self.p = (self.p & !(N | Z)) | (v & N) | if v == 0 { Z } else { 0 };
    }

    fn set_nz16(&mut self, v: u16) {
        self.p = (self.p & !(N | Z)) | ((v >> 8) as u8 & N) | if v == 0 { Z } else { 0 };
    }

    fn set_nz32(&mut self, v: u32) {
        self.p = (self.p & !(N | Z)) | ((v >> 24) as u8 & N) | if v == 0 { Z } else { 0 };
    }

    fn set_flag(&mut self, f: u8, on: bool) {
        if on {
            self.p |= f;
        } else {
            self.p &= !f;
        }
    }

    /// The effective address of a memory operand, and whether indexing
    /// crossed a page.
    fn operand(&mut self, bus: &mut dyn Bus, mode: Mode, mn: Mn, pre: Pre) -> (Ea, bool) {
        let cross = |base: u16, ea: u16| base & 0xff00 != ea & 0xff00;
        match mode {
            Mode::Zp => {
                let n = self.fetch(bus);
                (Ea::Cpu(self.zp(n)), false)
            }
            Mode::Zpx => {
                let n = self.fetch(bus).wrapping_add(self.x);
                (Ea::Cpu(self.zp(n)), false)
            }
            Mode::Zpy => {
                let n = self.fetch(bus).wrapping_add(self.y);
                (Ea::Cpu(self.zp(n)), false)
            }
            Mode::Abs => (Ea::Cpu(self.fetch16(bus)), false),
            Mode::Absx => {
                let base = self.fetch16(bus);
                let ea = base.wrapping_add(u16::from(self.x));
                (Ea::Cpu(ea), cross(base, ea))
            }
            Mode::Absy => {
                let base = self.fetch16(bus);
                let ea = base.wrapping_add(u16::from(self.y));
                (Ea::Cpu(ea), cross(base, ea))
            }
            Mode::Izx => {
                let n = self.fetch(bus).wrapping_add(self.x);
                let lo = self.rd(bus, self.zp(n));
                let hi = self.rd(bus, self.zp(n.wrapping_add(1)));
                (Ea::Cpu(u16::from_le_bytes([lo, hi])), false)
            }
            Mode::Izy => {
                let n = self.fetch(bus);
                let lo = self.rd(bus, self.zp(n));
                let hi = self.rd(bus, self.zp(n.wrapping_add(1)));
                let base = u16::from_le_bytes([lo, hi]);
                let ea = base.wrapping_add(u16::from(self.y));
                (Ea::Cpu(ea), cross(base, ea))
            }
            Mode::Izz => {
                let n = self.fetch(bus);
                // A Q instruction does not add Z (Z is part of Q), but
                // LDQ does.
                let add_z = !pre.q || mn == Mn::Lda;
                let z = if add_z { self.z } else { 0 };
                if pre.flat {
                    let ptr = u32::from_le_bytes([
                        self.rd(bus, self.zp(n)),
                        self.rd(bus, self.zp(n.wrapping_add(1))),
                        self.rd(bus, self.zp(n.wrapping_add(2))),
                        self.rd(bus, self.zp(n.wrapping_add(3))),
                    ]);
                    (Ea::Flat(ptr.wrapping_add(u32::from(z)) & ADDR_MASK), false)
                } else {
                    let lo = self.rd(bus, self.zp(n));
                    let hi = self.rd(bus, self.zp(n.wrapping_add(1)));
                    let base = u16::from_le_bytes([lo, hi]);
                    let ea = base.wrapping_add(u16::from(z));
                    (Ea::Cpu(ea), cross(base, ea))
                }
            }
            Mode::Isy => {
                let d = self.fetch(bus);
                let at = self.sp_step(i32::from(d));
                let lo = self.rd(bus, at);
                let hi = self.rd(bus, self.sp_step(i32::from(d) + 1));
                let base = u16::from_le_bytes([lo, hi]);
                let ea = base.wrapping_add(u16::from(self.y));
                (Ea::Cpu(ea), cross(base, ea))
            }
            _ => unreachable!("not a memory mode: {mode:?}"),
        }
    }

    /// The operand byte of a read instruction (immediate or memory).
    fn value(&mut self, bus: &mut dyn Bus, op: &Op, pre: Pre) -> (u8, bool) {
        if op.mode == Mode::Imm {
            return (self.fetch(bus), false);
        }
        let (ea, cross) = self.operand(bus, op.mode, op.mn, pre);
        (self.read_ea(bus, ea), cross)
    }

    fn adc(&mut self, v: u8) {
        let a = u16::from(self.a);
        let m = u16::from(v);
        let c = u16::from(self.p & C);
        if self.p & D == 0 {
            let s = a + m + c;
            self.set_flag(V, (!(a ^ m) & (a ^ s) & 0x80) != 0);
            self.set_flag(C, s > 0xff);
            self.a = s as u8;
        } else {
            let mut lo = (a & 0x0f) + (m & 0x0f) + c;
            if lo > 9 {
                lo += 6;
            }
            let mut s = (a & 0xf0) + (m & 0xf0) + lo;
            self.set_flag(V, (!(a ^ m) & (a ^ s) & 0x80) != 0);
            if s > 0x9f {
                s += 0x60;
            }
            self.set_flag(C, s > 0xff);
            self.a = s as u8;
        }
        self.set_nz(self.a);
    }

    fn sbc(&mut self, v: u8) {
        let a = i16::from(self.a);
        let m = i16::from(v);
        let borrow = 1 - i16::from(self.p & C);
        let bin = a - m - borrow;
        self.set_flag(V, ((a ^ m) & (a ^ bin) & 0x80) != 0);
        self.set_flag(C, bin >= 0);
        if self.p & D == 0 {
            self.a = bin as u8;
        } else {
            let mut lo = (a & 0x0f) - (m & 0x0f) - borrow;
            let mut hi = (a >> 4) - (m >> 4);
            if lo < 0 {
                lo -= 6;
                hi -= 1;
            }
            if hi < 0 {
                hi -= 6;
            }
            self.a = ((hi << 4) | (lo & 0x0f)) as u8;
        }
        self.set_nz(self.a);
    }

    fn compare(&mut self, r: u8, v: u8) {
        let d = r.wrapping_sub(v);
        self.set_flag(C, r >= v);
        self.set_nz(d);
    }

    /// The cycles of `op` at the current speed.
    fn cost(&self, op: &Op, cross: bool, taken: bool, taken_cross: bool) -> u32 {
        use crate::table::{B, D as FD, I as FI, P, R};
        let slow = matches!(self.speed, Speed::Mhz1 | Speed::Mhz2);
        let (n, f) = if slow {
            (op.slow, op.slow_flags)
        } else {
            (op.fast, op.fast_flags)
        };
        let mut c = i32::from(n);
        if slow && matches!(op.mode, Mode::Imp | Mode::Acc) {
            // At 1 and 2 MHz no instruction is shorter than the 6502's two
            // cycles.
            c = c.max(2);
        }
        if f & P != 0 && cross {
            c += 1;
        }
        if f & B != 0 && taken {
            c += 1 + i32::from(taken_cross);
        }
        match self.speed {
            Speed::Mhz3_5 if f & FD != 0 => c -= 1,
            Speed::Mhz40 => c += i32::from(f & R != 0) + i32::from(f & FI != 0),
            _ => {}
        }
        c.max(1) as u32
    }

    /// The cycles of a compound instruction (`code` as in the guide).
    fn compound_cost(&self, code: u32, op: &Op, cross: bool) -> u32 {
        use crate::table::{I as FI, P, R};
        match compound_cycles(code) {
            Some((n, f)) if n > 0 => {
                let mut c = u32::from(n);
                if f & P != 0 && cross {
                    c += 1;
                }
                if self.speed == Speed::Mhz40 {
                    c += u32::from(f & R != 0) + u32::from(f & FI != 0);
                }
                c
            }
            _ => self.cost(op, cross, false, false) + 2,
        }
    }

    /// A branch by `off` from PC; whether it crossed a page.
    fn branch_to(&mut self, target: u16) -> bool {
        let crossed = target & 0xff00 != self.pc & 0xff00;
        self.pc = target;
        crossed
    }

    /// Execute one instruction; its cycle count.
    pub(crate) fn execute(&mut self, bus: &mut dyn Bus) -> u32 {
        let mut code = self.fetch(bus);
        let mut pre = Pre::default();
        let mut compound = 0u32;
        if code == 0x42 && self.peek_code(bus, 0) == 0x42 {
            let third = self.peek_code(bus, 1);
            if third == 0xea && is_izz(self.peek_code(bus, 2)) {
                self.fetch(bus);
                self.fetch(bus);
                self.map_lock = false;
                code = self.fetch(bus);
                pre = Pre {
                    q: true,
                    flat: true,
                };
                compound = 0x4242_ea00 | u32::from(code);
            } else if Q_OPS[usize::from(third)] {
                self.fetch(bus);
                code = self.fetch(bus);
                pre.q = true;
                compound = 0x0042_4200 | u32::from(code);
            }
        } else if code == 0xea {
            // EOM: interrupts may come again; before a (zp),Z instruction
            // also the flat prefix.
            self.map_lock = false;
            if is_izz(self.peek_code(bus, 0)) {
                code = self.fetch(bus);
                pre.flat = true;
                compound = 0xea00 | u32::from(code);
            }
        }
        let op = OPS[usize::from(code)];
        if pre.q {
            let cross = self.exec_q(bus, &op, pre);
            return self.compound_cost(compound, &op, cross);
        }
        let (cross, taken, taken_cross) = self.exec8(bus, code, &op, pre);
        if compound != 0 {
            self.compound_cost(compound, &op, cross)
        } else {
            self.cost(&op, cross, taken, taken_cross)
        }
    }

    /// An RMW on memory: read, the `$D019` dummy write, `f`, write.
    fn rmw(&mut self, bus: &mut dyn Bus, ea: Ea, f: impl FnOnce(&mut Self, u8) -> u8) {
        let v = self.read_ea(bus, ea);
        if ea == Ea::Cpu(0xd019) {
            self.write_ea(bus, ea, v);
        }
        let r = f(self, v);
        self.write_ea(bus, ea, r);
    }

    fn shift(&mut self, mn: Mn, v: u8) -> u8 {
        let c_in = self.p & C;
        let (r, c_out) = match mn {
            Mn::Asl => (v << 1, v & 0x80 != 0),
            Mn::Lsr => (v >> 1, v & 1 != 0),
            Mn::Rol => (v << 1 | c_in, v & 0x80 != 0),
            Mn::Ror => (v >> 1 | c_in << 7, v & 1 != 0),
            Mn::Asr => ((v as i8 >> 1) as u8, v & 1 != 0),
            Mn::Inc => {
                let r = v.wrapping_add(1);
                self.set_nz(r);
                return r;
            }
            Mn::Dec => {
                let r = v.wrapping_sub(1);
                self.set_nz(r);
                return r;
            }
            _ => unreachable!(),
        };
        self.set_flag(C, c_out);
        self.set_nz(r);
        r
    }

    /// The 8-bit (and 16-bit word) instructions. Returns (page crossed,
    /// branch taken, the taken branch crossed a page).
    fn exec8(&mut self, bus: &mut dyn Bus, code: u8, op: &Op, pre: Pre) -> (bool, bool, bool) {
        let mut cross = false;
        let mut taken = false;
        let mut tcross = false;
        match op.mn {
            Mn::Lda | Mn::Ldx | Mn::Ldy | Mn::Ldz => {
                let (v, c) = self.value(bus, op, pre);
                cross = c;
                match op.mn {
                    Mn::Lda => self.a = v,
                    Mn::Ldx => self.x = v,
                    Mn::Ldy => self.y = v,
                    _ => self.z = v,
                }
                self.set_nz(v);
            }
            Mn::Sta | Mn::Stx | Mn::Sty | Mn::Stz => {
                let (ea, c) = self.operand(bus, op.mode, op.mn, pre);
                cross = c;
                let v = match op.mn {
                    Mn::Sta => self.a,
                    Mn::Stx => self.x,
                    Mn::Sty => self.y,
                    // The 4510's STZ stores the Z register.
                    _ => self.z,
                };
                self.write_ea(bus, ea, v);
            }
            Mn::Ora | Mn::And | Mn::Eor | Mn::Adc | Mn::Sbc | Mn::Cmp => {
                let (v, c) = self.value(bus, op, pre);
                cross = c;
                match op.mn {
                    Mn::Ora => {
                        self.a |= v;
                        self.set_nz(self.a);
                    }
                    Mn::And => {
                        self.a &= v;
                        self.set_nz(self.a);
                    }
                    Mn::Eor => {
                        self.a ^= v;
                        self.set_nz(self.a);
                    }
                    Mn::Adc => self.adc(v),
                    Mn::Sbc => self.sbc(v),
                    _ => self.compare(self.a, v),
                }
            }
            Mn::Cpx | Mn::Cpy | Mn::Cpz => {
                let (v, c) = self.value(bus, op, pre);
                cross = c;
                let r = match op.mn {
                    Mn::Cpx => self.x,
                    Mn::Cpy => self.y,
                    _ => self.z,
                };
                self.compare(r, v);
            }
            Mn::Bit => {
                let (v, c) = self.value(bus, op, pre);
                cross = c;
                self.p = (self.p & !(N | V)) | (v & (N | V));
                self.set_flag(Z, self.a & v == 0);
            }
            Mn::Asl | Mn::Lsr | Mn::Rol | Mn::Ror | Mn::Asr | Mn::Inc | Mn::Dec => {
                if op.mode == Mode::Acc {
                    self.a = self.shift(op.mn, self.a);
                } else {
                    let (ea, c) = self.operand(bus, op.mode, op.mn, pre);
                    cross = c;
                    let mn = op.mn;
                    self.rmw(bus, ea, |s, v| s.shift(mn, v));
                }
            }
            Mn::Neg => {
                self.a = self.a.wrapping_neg();
                self.set_nz(self.a);
            }
            Mn::Tsb | Mn::Trb => {
                let (ea, _) = self.operand(bus, op.mode, op.mn, pre);
                let set = op.mn == Mn::Tsb;
                self.rmw(bus, ea, |s, v| {
                    s.set_flag(Z, s.a & v == 0);
                    if set {
                        v | s.a
                    } else {
                        v & !s.a
                    }
                });
            }
            Mn::Rmb0
            | Mn::Rmb1
            | Mn::Rmb2
            | Mn::Rmb3
            | Mn::Rmb4
            | Mn::Rmb5
            | Mn::Rmb6
            | Mn::Rmb7
            | Mn::Smb0
            | Mn::Smb1
            | Mn::Smb2
            | Mn::Smb3
            | Mn::Smb4
            | Mn::Smb5
            | Mn::Smb6
            | Mn::Smb7 => {
                let (ea, _) = self.operand(bus, Mode::Zp, op.mn, pre);
                let bit = 1u8 << (code >> 4 & 7);
                let set = code & 0x80 != 0;
                self.rmw(bus, ea, |_, v| if set { v | bit } else { v & !bit });
            }
            Mn::Bbr0
            | Mn::Bbr1
            | Mn::Bbr2
            | Mn::Bbr3
            | Mn::Bbr4
            | Mn::Bbr5
            | Mn::Bbr6
            | Mn::Bbr7
            | Mn::Bbs0
            | Mn::Bbs1
            | Mn::Bbs2
            | Mn::Bbs3
            | Mn::Bbs4
            | Mn::Bbs5
            | Mn::Bbs6
            | Mn::Bbs7 => {
                let n = self.fetch(bus);
                let off = self.fetch(bus) as i8;
                let v = self.rd(bus, self.zp(n));
                let bit = v >> (code >> 4 & 7) & 1 != 0;
                if bit == (code & 0x80 != 0) {
                    taken = true;
                    tcross = self.branch_to(self.pc.wrapping_add(off as u16));
                }
            }
            Mn::Bpl
            | Mn::Bmi
            | Mn::Bvc
            | Mn::Bvs
            | Mn::Bcc
            | Mn::Bcs
            | Mn::Bne
            | Mn::Beq
            | Mn::Bra => {
                let cond = match op.mn {
                    Mn::Bpl => self.p & N == 0,
                    Mn::Bmi => self.p & N != 0,
                    Mn::Bvc => self.p & V == 0,
                    Mn::Bvs => self.p & V != 0,
                    Mn::Bcc => self.p & C == 0,
                    Mn::Bcs => self.p & C != 0,
                    Mn::Bne => self.p & Z == 0,
                    Mn::Beq => self.p & Z != 0,
                    _ => true,
                };
                let target = if op.mode == Mode::Rel16 {
                    let off = self.fetch16(bus);
                    // Relative to the instruction's last byte.
                    self.pc.wrapping_sub(1).wrapping_add(off)
                } else {
                    let off = self.fetch(bus) as i8;
                    self.pc.wrapping_add(off as u16)
                };
                if cond {
                    taken = true;
                    tcross = self.branch_to(target);
                }
            }
            Mn::Bsr => {
                let off = self.fetch16(bus);
                let ret = self.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8);
                self.push(bus, ret as u8);
                self.pc = ret.wrapping_add(off);
            }
            Mn::Jmp => {
                self.pc = match op.mode {
                    Mode::Abs => self.fetch16(bus),
                    Mode::Ind => {
                        let ptr = self.fetch16(bus);
                        self.read16(bus, Ea::Cpu(ptr))
                    }
                    _ => {
                        let ptr = self.fetch16(bus).wrapping_add(u16::from(self.x));
                        self.read16(bus, Ea::Cpu(ptr))
                    }
                };
            }
            Mn::Jsr => {
                let target = match op.mode {
                    Mode::Abs => self.fetch16(bus),
                    Mode::Ind => {
                        let ptr = self.fetch16(bus);
                        self.read16(bus, Ea::Cpu(ptr))
                    }
                    _ => {
                        let ptr = self.fetch16(bus).wrapping_add(u16::from(self.x));
                        self.read16(bus, Ea::Cpu(ptr))
                    }
                };
                let ret = self.pc.wrapping_sub(1);
                self.push(bus, (ret >> 8) as u8);
                self.push(bus, ret as u8);
                self.pc = target;
            }
            Mn::Rts | Mn::Rtn => {
                let n = if op.mn == Mn::Rtn { self.fetch(bus) } else { 0 };
                let lo = self.pull(bus);
                let hi = self.pull(bus);
                self.sp = self.sp_step(i32::from(n));
                self.pc = u16::from_le_bytes([lo, hi]).wrapping_add(1);
            }
            Mn::Rti => {
                let p = self.pull(bus);
                self.p = (p & !(FB | E)) | (self.p & E);
                let lo = self.pull(bus);
                let hi = self.pull(bus);
                self.pc = u16::from_le_bytes([lo, hi]);
            }
            Mn::Brk => {
                self.pc = self.pc.wrapping_add(1);
                self.interrupt(bus, 0xfffe, true);
            }
            Mn::Pha => self.push(bus, self.a),
            Mn::Phx => self.push(bus, self.x),
            Mn::Phy => self.push(bus, self.y),
            Mn::Phz => self.push(bus, self.z),
            Mn::Php => self.push(bus, self.p | FB),
            Mn::Pla | Mn::Plx | Mn::Ply | Mn::Plz => {
                let v = self.pull(bus);
                match op.mn {
                    Mn::Pla => self.a = v,
                    Mn::Plx => self.x = v,
                    Mn::Ply => self.y = v,
                    _ => self.z = v,
                }
                self.set_nz(v);
            }
            Mn::Plp => {
                let p = self.pull(bus);
                self.p = (p & !(FB | E)) | (self.p & E);
            }
            Mn::Phw => {
                let w = if op.mode == Mode::Imm16 {
                    self.fetch16(bus)
                } else {
                    let a = self.fetch16(bus);
                    self.read16(bus, Ea::Cpu(a))
                };
                // The low byte first: the word is high byte first in
                // memory (unlike JSR's return address).
                self.push(bus, w as u8);
                self.push(bus, (w >> 8) as u8);
            }
            Mn::Inw | Mn::Dew => {
                let n = self.fetch(bus);
                let lo = Ea::Cpu(self.zp(n));
                let hi = Ea::Cpu(self.zp(n.wrapping_add(1)));
                let w = u16::from_le_bytes([self.read_ea(bus, lo), self.read_ea(bus, hi)]);
                let r = if op.mn == Mn::Inw {
                    w.wrapping_add(1)
                } else {
                    w.wrapping_sub(1)
                };
                self.write_ea(bus, lo, r as u8);
                self.write_ea(bus, hi, (r >> 8) as u8);
                self.set_nz16(r);
            }
            Mn::Asw | Mn::Row => {
                let a = self.fetch16(bus);
                let w = self.read16(bus, Ea::Cpu(a));
                let c_in = u16::from(self.p & C);
                let r = if op.mn == Mn::Asw {
                    w << 1
                } else {
                    w << 1 | c_in
                };
                self.set_flag(C, w & 0x8000 != 0);
                self.write_ea(bus, Ea::Cpu(a), r as u8);
                self.write_ea(bus, Ea::Cpu(a.wrapping_add(1)), (r >> 8) as u8);
                self.set_nz16(r);
            }
            Mn::Map => {
                self.map.set(self.a, self.x, self.y, self.z);
                self.map_lock = true;
            }
            Mn::Eom => {}
            Mn::Clc => self.p &= !C,
            Mn::Sec => self.p |= C,
            Mn::Cli => self.p &= !I,
            Mn::Sei => self.p |= I,
            Mn::Cld => self.p &= !D,
            Mn::Sed => self.p |= D,
            Mn::Clv => self.p &= !V,
            Mn::Cle => self.p &= !E,
            Mn::See => self.p |= E,
            Mn::Inx => {
                self.x = self.x.wrapping_add(1);
                self.set_nz(self.x);
            }
            Mn::Iny => {
                self.y = self.y.wrapping_add(1);
                self.set_nz(self.y);
            }
            Mn::Inz => {
                self.z = self.z.wrapping_add(1);
                self.set_nz(self.z);
            }
            Mn::Dex => {
                self.x = self.x.wrapping_sub(1);
                self.set_nz(self.x);
            }
            Mn::Dey => {
                self.y = self.y.wrapping_sub(1);
                self.set_nz(self.y);
            }
            Mn::Dez => {
                self.z = self.z.wrapping_sub(1);
                self.set_nz(self.z);
            }
            Mn::Tax => {
                self.x = self.a;
                self.set_nz(self.x);
            }
            Mn::Tay => {
                self.y = self.a;
                self.set_nz(self.y);
            }
            Mn::Taz => {
                self.z = self.a;
                self.set_nz(self.z);
            }
            Mn::Txa => {
                self.a = self.x;
                self.set_nz(self.a);
            }
            Mn::Tya => {
                self.a = self.y;
                self.set_nz(self.a);
            }
            Mn::Tza => {
                self.a = self.z;
                self.set_nz(self.a);
            }
            Mn::Tab => self.b = self.a,
            Mn::Tba => {
                self.a = self.b;
                self.set_nz(self.a);
            }
            Mn::Tsx => {
                self.x = self.sp as u8;
                self.set_nz(self.x);
            }
            Mn::Txs => self.sp = (self.sp & 0xff00) | u16::from(self.x),
            Mn::Tsy => {
                self.y = (self.sp >> 8) as u8;
                self.set_nz(self.y);
            }
            Mn::Tys => self.sp = u16::from(self.y) << 8 | (self.sp & 0x00ff),
        }
        (cross, taken, tcross)
    }

    /// A Q instruction (prefix `42 42`); whether indexing crossed a page.
    fn exec_q(&mut self, bus: &mut dyn Bus, op: &Op, pre: Pre) -> bool {
        let q = self.q();
        if op.mode == Mode::Acc {
            let r = self.shift_q(op.mn, q);
            self.set_q(r);
            return false;
        }
        let (ea, cross) = self.operand(bus, op.mode, op.mn, pre);
        match op.mn {
            Mn::Lda => {
                let v = self.read32(bus, ea);
                self.set_q(v);
                self.set_nz32(v);
            }
            Mn::Sta => self.write32(bus, ea, q),
            Mn::Ora | Mn::And | Mn::Eor => {
                let v = self.read32(bus, ea);
                let r = match op.mn {
                    Mn::Ora => q | v,
                    Mn::And => q & v,
                    _ => q ^ v,
                };
                self.set_q(r);
                self.set_nz32(r);
            }
            Mn::Adc => {
                let v = self.read32(bus, ea);
                let s = u64::from(q) + u64::from(v) + u64::from(self.p & C);
                let r = s as u32;
                self.set_flag(C, s > 0xffff_ffff);
                self.set_flag(V, (!(q ^ v) & (q ^ r) & 0x8000_0000) != 0);
                self.set_q(r);
                self.set_nz32(r);
            }
            Mn::Sbc => {
                let v = self.read32(bus, ea);
                let borrow = 1 - i64::from(self.p & C);
                let d = i64::from(q) - i64::from(v) - borrow;
                let r = d as u32;
                self.set_flag(C, d >= 0);
                self.set_flag(V, ((q ^ v) & (q ^ r) & 0x8000_0000) != 0);
                self.set_q(r);
                self.set_nz32(r);
            }
            Mn::Cmp => {
                let v = self.read32(bus, ea);
                self.set_flag(C, q >= v);
                self.set_nz32(q.wrapping_sub(v));
            }
            Mn::Bit => {
                let v = self.read32(bus, ea);
                self.p = (self.p & !(N | V)) | ((v >> 24) as u8 & (N | V));
                self.set_flag(Z, q & v == 0);
            }
            Mn::Asl | Mn::Lsr | Mn::Rol | Mn::Ror | Mn::Asr | Mn::Inc | Mn::Dec => {
                let v = self.read32(bus, ea);
                let r = self.shift_q(op.mn, v);
                self.write32(bus, ea, r);
            }
            _ => {}
        }
        cross
    }

    fn shift_q(&mut self, mn: Mn, v: u32) -> u32 {
        let c_in = u32::from(self.p & C);
        let (r, c_out) = match mn {
            Mn::Asl => (v << 1, v & 0x8000_0000 != 0),
            Mn::Lsr => (v >> 1, v & 1 != 0),
            Mn::Rol => (v << 1 | c_in, v & 0x8000_0000 != 0),
            Mn::Ror => (v >> 1 | c_in << 31, v & 1 != 0),
            Mn::Asr => ((v as i32 >> 1) as u32, v & 1 != 0),
            Mn::Inc | Mn::Dec => {
                let r = if mn == Mn::Inc {
                    v.wrapping_add(1)
                } else {
                    v.wrapping_sub(1)
                };
                self.set_nz32(r);
                return r;
            }
            _ => return v,
        };
        self.set_flag(C, c_out);
        self.set_nz32(r);
        r
    }
}
