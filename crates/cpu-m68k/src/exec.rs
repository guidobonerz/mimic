//! Instruction execution. Each instruction performs its bus cycles in the
//! order of the "Yet Another Cycle Hunting Table" (YACHT, Jorge Cwik, after
//! the M68000UM timing tables and US patent 4,325,121), noted in the
//! comments: `n` an internal microcycle (2 clocks), `np` a prefetch, `nr`/
//! `nR` reads (low/high word), `nw`/`nW` writes, `ns`/`nS` stack writes.

use emu_core::Size;

use crate::ea::{mode, Ea, Sz};
use crate::{sr, vector, Frame, Res, TimedBus68, M68000};

pub(crate) fn execute(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    match op >> 12 {
        0x0 => line0(cpu, bus, op),
        0x1 => moves(cpu, bus, op, Sz::B),
        0x2 => moves(cpu, bus, op, Sz::L),
        0x3 => moves(cpu, bus, op, Sz::W),
        0x4 => line4(cpu, bus, op),
        0x5 => line5(cpu, bus, op),
        0x6 => branch(cpu, bus, op),
        0x7 => moveq(cpu, bus, op),
        0x8 => line8(cpu, bus, op),
        0x9 => add_sub(cpu, bus, op, false),
        0xa => trap_now(cpu, bus, vector::LINE_A),
        0xb => line_b(cpu, bus, op),
        0xc => line_c(cpu, bus, op),
        0xd => add_sub(cpu, bus, op, true),
        0xe => shifts(cpu, bus, op),
        _ => trap_now(cpu, bus, vector::LINE_F),
    }
}

// ---------------------------------------------------------------- helpers

/// Illegal instruction, privilege violation, line A/F: "nn ns ns nS", the
/// instruction's own address on the stack.
fn trap_now(cpu: &mut M68000, bus: &mut dyn TimedBus68, vec: u8) -> Res {
    let pc = cpu.pc();
    cpu.idle(bus, 4);
    cpu.exception(bus, vec, pc, Frame::LowSrHigh)
}

fn illegal(cpu: &mut M68000, bus: &mut dyn TimedBus68) -> Res {
    trap_now(cpu, bus, vector::ILLEGAL)
}

fn privileged(cpu: &mut M68000, bus: &mut dyn TimedBus68) -> Res<bool> {
    if cpu.supervisor() {
        return Ok(true);
    }
    trap_now(cpu, bus, vector::PRIVILEGE)?;
    Ok(false)
}

/// Data alterable memory modes (2–6, 7.0, 7.1).
fn mem_alterable(m: u16, r: usize) -> bool {
    matches!(m, 2..=6) || (m == 7 && r <= 1)
}

fn data_alterable(m: u16, r: usize) -> bool {
    m == 0 || mem_alterable(m, r)
}

/// Data modes (all but An).
fn data_mode(m: u16, r: usize) -> bool {
    m != 1 && (m != 7 || r <= 4)
}

fn any_mode(m: u16, r: usize) -> bool {
    m != 7 || r <= 4
}

/// Control modes (JMP, JSR, LEA, PEA, MOVEM).
fn control(m: u16, r: usize) -> bool {
    m == 2 || m == 5 || m == 6 || (m == 7 && r <= 3)
}

impl M68000 {
    /// Source operand: address calculation and read.
    fn src(&mut self, bus: &mut dyn TimedBus68, m: u16, r: usize, sz: Sz) -> Res<u32> {
        let ea = self.ea(bus, m, r, sz)?.expect("checked mode");
        self.read_ea(bus, ea, sz)
    }

    /// Result into a destination that was read before (read-modify-write):
    /// "np nw" (longs "np nw nW", low word first) or a register.
    fn store_rmw(&mut self, bus: &mut dyn TimedBus68, ea: Ea, sz: Sz, v: u32) -> Res {
        match ea {
            Ea::D(r) => {
                self.set_d(r, sz, v);
                self.prefetch(bus)
            }
            Ea::Mem(addr) => {
                self.prefetch(bus)?;
                self.write_mem(bus, addr, sz, v, true)
            }
            Ea::A(_) | Ea::Imm(_) => unreachable!("not alterable"),
        }
    }

    fn add_flags(&mut self, sz: Sz, s: u32, d: u32, r: u32, addx: bool) {
        let msb = sz.msb();
        let (s, d, r) = (s & sz.mask(), d & sz.mask(), r & sz.mask());
        let c = ((s & d) | (!r & (s | d))) & msb != 0;
        let v = (!(s ^ d) & (r ^ d)) & msb != 0;
        self.set_flag(sr::C, c);
        self.set_flag(sr::X, c);
        self.set_flag(sr::V, v);
        self.set_flag(sr::N, r & msb != 0);
        if addx {
            if r != 0 {
                self.set_flag(sr::Z, false);
            }
        } else {
            self.set_flag(sr::Z, r == 0);
        }
    }

    fn sub_flags(&mut self, sz: Sz, s: u32, d: u32, r: u32, subx: bool, cmp: bool) {
        let msb = sz.msb();
        let (s, d, r) = (s & sz.mask(), d & sz.mask(), r & sz.mask());
        let c = ((s & !d) | (r & !d) | (s & r)) & msb != 0;
        let v = ((s ^ d) & (r ^ d)) & msb != 0;
        self.set_flag(sr::C, c);
        if !cmp {
            self.set_flag(sr::X, c);
        }
        self.set_flag(sr::V, v);
        self.set_flag(sr::N, r & msb != 0);
        if subx {
            if r != 0 {
                self.set_flag(sr::Z, false);
            }
        } else {
            self.set_flag(sr::Z, r == 0);
        }
    }

    fn add(&mut self, sz: Sz, s: u32, d: u32) -> u32 {
        let r = d.wrapping_add(s);
        self.add_flags(sz, s, d, r, false);
        r & sz.mask()
    }

    fn sub(&mut self, sz: Sz, s: u32, d: u32) -> u32 {
        let r = d.wrapping_sub(s);
        self.sub_flags(sz, s, d, r, false, false);
        r & sz.mask()
    }

    fn cmp(&mut self, sz: Sz, s: u32, d: u32) {
        let r = d.wrapping_sub(s);
        self.sub_flags(sz, s, d, r, false, true);
    }

    fn x(&self) -> u32 {
        u32::from(self.flag(sr::X))
    }

    /// Push a long: "nS ns" (high word at the lower address first).
    fn push_long(&mut self, bus: &mut dyn TimedBus68, v: u32) -> Res {
        let sp = self.a[7].wrapping_sub(4);
        self.write(bus, sp, Size::Word, v >> 16)?;
        self.write(bus, sp.wrapping_add(2), Size::Word, v & 0xffff)?;
        self.a[7] = sp;
        Ok(())
    }

    fn pop_long(&mut self, bus: &mut dyn TimedBus68) -> Res<u32> {
        let sp = self.a[7];
        let v = self.read_mem(bus, sp, Sz::L)?;
        self.a[7] = sp.wrapping_add(4);
        Ok(v)
    }
}

// ---------------------------------------------------------------- line 0

fn line0(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if op & 0x0100 != 0 {
        if m == 1 {
            return movep(cpu, bus, op);
        }
        // Dynamic bit operation: bit number in Dn.
        let bit = cpu.d[usize::from((op >> 9) & 7)];
        return bit_op(cpu, bus, op, bit, false);
    }
    match (op >> 9) & 7 {
        4 => {
            // Static bit operation: "np" for the bit number.
            if !data_mode(m, r)
                || (m == 7 && r == 4)
                || ((op >> 6) & 3 != 0 && !data_alterable(m, r))
            {
                return illegal(cpu, bus);
            }
            let bit = u32::from(cpu.ext(bus)?);
            bit_op(cpu, bus, op, bit, true)
        }
        7 => illegal(cpu, bus),
        kind => immediate(cpu, bus, op, kind),
    }
}

/// ORI, ANDI, SUBI, ADDI, EORI, CMPI (kind = bits 11–9: 0, 1, 2, 3, 5, 6).
fn immediate(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, kind: u16) -> Res {
    let (m, r) = mode(op);
    // To CCR / SR: ORI, ANDI, EORI only.
    if m == 7 && r == 4 {
        let to_sr = match (op >> 6) & 3 {
            0 => false,
            1 => true,
            _ => return illegal(cpu, bus),
        };
        if !matches!(kind, 0 | 1 | 5) {
            return illegal(cpu, bus);
        }
        if to_sr && !privileged(cpu, bus)? {
            return Ok(());
        }
        // "np nn nn np np": the queue is refilled after the change.
        let imm = cpu.ext(bus)?;
        let cur = if to_sr { cpu.sr() } else { cpu.sr() & 0xff };
        let v = match kind {
            0 => cur | imm,
            1 => cur & imm,
            _ => cur ^ imm,
        };
        if to_sr {
            cpu.set_sr(v);
        } else {
            cpu.set_ccr(v);
        }
        cpu.idle(bus, 8);
        cpu.set_au(cpu.au().wrapping_sub(2));
        cpu.fill_irc(bus)?;
        return cpu.prefetch(bus);
    }
    let Some(sz) = Sz::from_bits(op >> 6) else {
        return illegal(cpu, bus);
    };
    if !data_alterable(m, r) && !(kind == 6 && data_mode(m, r) && !(m == 7 && r == 4)) {
        return illegal(cpu, bus);
    }
    let imm = match sz {
        Sz::L => cpu.ext_long(bus)?,
        _ => u32::from(cpu.ext(bus)?) & sz.mask(),
    };
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let v = match kind {
        0 => {
            let v = d | imm;
            cpu.logic_flags(sz, v);
            v
        }
        1 => {
            let v = d & imm;
            cpu.logic_flags(sz, v);
            v
        }
        2 => cpu.sub(sz, imm, d),
        3 => cpu.add(sz, imm, d),
        5 => {
            let v = d ^ imm;
            cpu.logic_flags(sz, v);
            v
        }
        _ => {
            cpu.cmp(sz, imm, d);
            cpu.prefetch(bus)?;
            if sz == Sz::L && matches!(ea, Ea::D(_)) {
                cpu.idle(bus, 2);
            }
            return Ok(());
        }
    };
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 4);
        }
        return Ok(());
    }
    cpu.store_rmw(bus, ea, sz, v)
}

/// BTST, BCHG, BCLR, BSET (type in bits 7–6).
fn bit_op(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, bit: u32, imm: bool) -> Res {
    let (m, r) = mode(op);
    let kind = (op >> 6) & 3;
    if m == 1
        || (kind != 0 && !data_alterable(m, r))
        || !any_mode(m, r)
        || (imm && m == 7 && r == 4)
    {
        return illegal(cpu, bus);
    }
    if m == 0 {
        let n = bit & 31;
        let mask = 1u32 << n;
        let dv = cpu.d[r];
        cpu.set_flag(sr::Z, dv & mask == 0);
        cpu.prefetch(bus)?;
        match kind {
            0 => cpu.idle(bus, 2),
            1 | 3 => {
                cpu.d[r] = if kind == 1 { dv ^ mask } else { dv | mask };
                cpu.idle(bus, if n < 16 { 2 } else { 4 });
            }
            _ => {
                cpu.d[r] = dv & !mask;
                cpu.idle(bus, if n < 16 { 4 } else { 6 });
            }
        }
        return Ok(());
    }
    let ea = cpu.ea(bus, m, r, Sz::B)?.expect("checked");
    let v = cpu.read_ea(bus, ea, Sz::B)?;
    let mask = 1u32 << (bit & 7);
    cpu.set_flag(sr::Z, v & mask == 0);
    let nv = match kind {
        0 => {
            cpu.prefetch(bus)?;
            // BTST Dn,#imm: one more internal cycle.
            if matches!(ea, Ea::Imm(_)) {
                cpu.idle(bus, 2);
            }
            return Ok(());
        }
        1 => v ^ mask,
        2 => v & !mask,
        _ => v | mask,
    };
    cpu.store_rmw(bus, ea, Sz::B, nv)
}

/// MOVEP: bytes at d16(Ay), +2, +4, +6 (high byte first).
fn movep(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let dx = usize::from((op >> 9) & 7);
    let ay = usize::from(op & 7);
    let d = cpu.ext(bus)? as i16 as i32 as u32;
    let addr = cpu.a[ay].wrapping_add(d);
    let n = if op & 0x40 != 0 { 4 } else { 2 };
    if op & 0x80 != 0 {
        let v = cpu.d[dx];
        for i in 0..n {
            let byte = v >> (8 * (n - 1 - i)) & 0xff;
            cpu.write(bus, addr.wrapping_add(2 * i), Size::Byte, byte)?;
        }
    } else {
        let mut v = 0u32;
        for i in 0..n {
            v = v << 8 | cpu.read(bus, addr.wrapping_add(2 * i), Size::Byte)?;
        }
        if n == 2 {
            cpu.set_d(dx, Sz::W, v);
        } else {
            cpu.d[dx] = v;
        }
    }
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- MOVE

fn moves(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, sz: Sz) -> Res {
    let (sm, sr_) = mode(op);
    let dm = (op >> 6) & 7;
    let dr = usize::from((op >> 9) & 7);
    if !any_mode(sm, sr_)
        || (sz == Sz::B && sm == 1)
        || (dm == 1 && sz == Sz::B)
        || !data_alterable(dm, dr) && dm != 1
    {
        return illegal(cpu, bus);
    }
    let v = cpu.src(bus, sm, sr_, sz)?;
    if dm == 1 {
        // MOVEA: the whole register, words sign-extended; no flags.
        cpu.a[dr] = sz.sext(v);
        return cpu.prefetch(bus);
    }
    match dm {
        0 => {
            cpu.set_d(dr, sz, v);
            cpu.logic_flags(sz, v);
            cpu.prefetch(bus)
        }
        // (An)+: the register steps after the write.
        3 => {
            let addr = cpu.a[dr];
            cpu.logic_flags(sz, v);
            cpu.write_mem(bus, addr, sz, v, false)?;
            let step = if dr == 7 && sz == Sz::B {
                2
            } else {
                sz.bytes()
            };
            cpu.a[dr] = addr.wrapping_add(step);
            cpu.prefetch(bus)
        }
        // -(An): "np nw" (longs "np nw nW"), no extra internal cycle.
        4 => {
            let step = if dr == 7 && sz == Sz::B {
                2
            } else {
                sz.bytes()
            };
            let addr = cpu.a[dr].wrapping_sub(step);
            cpu.logic_flags(sz, v);
            cpu.prefetch(bus)?;
            if sz == Sz::L {
                // The low word goes first; the register follows each word.
                cpu.a[dr] = addr.wrapping_add(2);
                cpu.write(bus, addr.wrapping_add(2), Size::Word, v & 0xffff)?;
                cpu.a[dr] = addr;
                return cpu.write(bus, addr, Size::Word, v >> 16);
            }
            cpu.a[dr] = addr;
            cpu.write_mem(bus, addr, sz, v, true)
        }
        // (xxx).L from a memory source: "np nw np np" — the write uses the
        // low address word while it is still in IRC.
        7 if dr == 1 && sm >= 2 && !(sm == 7 && sr_ == 4) => {
            let hi = u32::from(cpu.ext(bus)?);
            let addr = hi << 16 | u32::from(cpu.irc());
            cpu.logic_flags(sz, v);
            cpu.write_mem(bus, addr, sz, v, false)?;
            cpu.ext(bus)?;
            cpu.prefetch(bus)
        }
        _ => {
            let Some(Ea::Mem(addr)) = cpu.ea(bus, dm, dr, sz)? else {
                return illegal(cpu, bus);
            };
            cpu.logic_flags(sz, v);
            cpu.write_mem(bus, addr, sz, v, false)?;
            cpu.prefetch(bus)
        }
    }
}

fn moveq(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    if op & 0x100 != 0 {
        return illegal(cpu, bus);
    }
    let v = op as u8 as i8 as i32 as u32;
    cpu.d[usize::from((op >> 9) & 7)] = v;
    cpu.logic_flags(Sz::L, v);
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- line 4

fn line4(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if op & 0x01c0 == 0x01c0 {
        return lea(cpu, bus, op);
    }
    if op & 0x01c0 == 0x0180 {
        return chk(cpu, bus, op);
    }
    match op & 0x0fc0 {
        0x0000 | 0x0040 | 0x0080 => unary(cpu, bus, op, Unary::Negx),
        0x00c0 => move_from_sr(cpu, bus, op),
        0x0200 | 0x0240 | 0x0280 => unary(cpu, bus, op, Unary::Clr),
        0x02c0 => illegal(cpu, bus),
        0x0400 | 0x0440 | 0x0480 => unary(cpu, bus, op, Unary::Neg),
        0x04c0 => move_to_ccr(cpu, bus, op, false),
        0x0600 | 0x0640 | 0x0680 => unary(cpu, bus, op, Unary::Not),
        0x06c0 => move_to_ccr(cpu, bus, op, true),
        0x0800 => nbcd(cpu, bus, op),
        0x0840 if m == 0 => {
            let v = cpu.d[r].rotate_left(16);
            cpu.d[r] = v;
            cpu.logic_flags(Sz::L, v);
            cpu.prefetch(bus)
        }
        0x0840 => pea(cpu, bus, op),
        0x0880 | 0x08c0 if m == 0 => {
            let v = if op & 0x40 != 0 {
                let v = cpu.d[r] as u16 as i16 as i32 as u32;
                cpu.d[r] = v;
                cpu.logic_flags(Sz::L, v);
                v
            } else {
                let v = cpu.d[r] as u8 as i8 as i16 as u16 as u32;
                cpu.set_d(r, Sz::W, v);
                cpu.logic_flags(Sz::W, v);
                v
            };
            let _ = v;
            cpu.prefetch(bus)
        }
        0x0880 | 0x08c0 => movem(cpu, bus, op, false),
        0x0a00 | 0x0a40 | 0x0a80 => tst(cpu, bus, op),
        0x0ac0 if op == 0x4afc => illegal(cpu, bus),
        0x0ac0 => tas(cpu, bus, op),
        0x0c80 | 0x0cc0 => movem(cpu, bus, op, true),
        0x0e40 => misc4e(cpu, bus, op),
        0x0e80 => jsr_jmp(cpu, bus, op, true),
        0x0ec0 => jsr_jmp(cpu, bus, op, false),
        _ => illegal(cpu, bus),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unary {
    Negx,
    Clr,
    Neg,
    Not,
}

/// NEGX, CLR, NEG, NOT: Dn "np" (longs "np n"); memory "nr np nw".
fn unary(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, kind: Unary) -> Res {
    let (m, r) = mode(op);
    let sz = Sz::from_bits(op >> 6).expect("matched");
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let v = match kind {
        Unary::Clr => {
            cpu.logic_flags(sz, 0);
            0
        }
        Unary::Not => {
            let v = !d & sz.mask();
            cpu.logic_flags(sz, v);
            v
        }
        Unary::Neg => cpu.sub(sz, d, 0),
        Unary::Negx => {
            let x = cpu.x();
            let res = 0u32.wrapping_sub(d).wrapping_sub(x);
            cpu.sub_flags(sz, d, 0, res, true, false);
            res & sz.mask()
        }
    };
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 2);
        }
        return Ok(());
    }
    cpu.store_rmw(bus, ea, sz, v)
}

/// MOVE from SR (not privileged on the 68000): Dn "np n"; memory "nr np nw".
fn move_from_sr(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, Sz::W)?.expect("checked");
    let v = u32::from(cpu.sr());
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, Sz::W, v);
        cpu.prefetch(bus)?;
        cpu.idle(bus, 2);
        return Ok(());
    }
    cpu.read_ea(bus, ea, Sz::W)?;
    cpu.store_rmw(bus, ea, Sz::W, v)
}

/// MOVE to CCR / to SR: "nn np np" after the operand.
fn move_to_ccr(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, to_sr: bool) -> Res {
    let (m, r) = mode(op);
    if !data_mode(m, r) {
        return illegal(cpu, bus);
    }
    if to_sr && !privileged(cpu, bus)? {
        return Ok(());
    }
    let v = cpu.src(bus, m, r, Sz::W)? as u16;
    if to_sr {
        cpu.set_sr(v);
    } else {
        cpu.set_ccr(v);
    }
    cpu.idle(bus, 4);
    cpu.set_au(cpu.au().wrapping_sub(2));
    cpu.fill_irc(bus)?;
    cpu.prefetch(bus)
}

fn nbcd(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, Sz::B)?.expect("checked");
    let d = cpu.read_ea(bus, ea, Sz::B)?;
    let v = bcd_sub(cpu, d, 0);
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, Sz::B, v);
        cpu.prefetch(bus)?;
        cpu.idle(bus, 2);
        return Ok(());
    }
    cpu.store_rmw(bus, ea, Sz::B, v)
}

/// PEA: (An) "np nS ns"; (d16,An) "np np nS ns"; (d8,An,Xn) "n np n np nS
/// ns"; (xxx).W "np nS ns np"; (xxx).L "np np nS ns np".
fn pea(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !control(m, r) {
        return illegal(cpu, bus);
    }
    let addr = control_ea(cpu, bus, m, r)?;
    match (m, r) {
        (7, 0) | (7, 1) => {
            cpu.push_long(bus, addr)?;
            cpu.prefetch(bus)
        }
        _ => {
            cpu.prefetch(bus)?;
            cpu.push_long(bus, addr)
        }
    }
}

/// Control address calculation as LEA/PEA do it: (d8,An,Xn) and
/// (d8,PC,Xn) take "n np n".
fn control_ea(cpu: &mut M68000, bus: &mut dyn TimedBus68, m: u16, r: usize) -> Res<u32> {
    Ok(match (m, r) {
        (2, _) => cpu.a[r],
        (5, _) => {
            let d = cpu.ext(bus)? as i16 as i32 as u32;
            cpu.a[r].wrapping_add(d)
        }
        (6, _) => {
            cpu.idle(bus, 2);
            let ext = cpu.ext(bus)?;
            cpu.idle(bus, 2);
            cpu.index(cpu.a[r], ext)
        }
        (7, 0) => cpu.ext(bus)? as i16 as i32 as u32,
        (7, 1) => cpu.ext_long(bus)?,
        (7, 2) => {
            let base = cpu.ext_pc();
            base.wrapping_add(cpu.ext(bus)? as i16 as i32 as u32)
        }
        _ => {
            cpu.idle(bus, 2);
            let base = cpu.ext_pc();
            let ext = cpu.ext(bus)?;
            cpu.idle(bus, 2);
            cpu.index(base, ext)
        }
    })
}

fn lea(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !control(m, r) {
        return illegal(cpu, bus);
    }
    let addr = control_ea(cpu, bus, m, r)?;
    cpu.a[usize::from((op >> 9) & 7)] = addr;
    cpu.prefetch(bus)
}

/// CHK.W <ea>,Dn: no trap "np nn n"; Dn > src "np nn" + trap; Dn < 0
/// "np nn n" + trap.
fn chk(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !data_mode(m, r) {
        return illegal(cpu, bus);
    }
    let bound = cpu.src(bus, m, r, Sz::W)? as i16;
    let dn = cpu.d[usize::from((op >> 9) & 7)] as i16;
    cpu.prefetch(bus)?;
    cpu.set_flag(sr::Z, dn == 0);
    cpu.set_flag(sr::V, false);
    cpu.set_flag(sr::C, false);
    // The upper bound is checked first.
    // N shows the sign of Dn when it traps; otherwise it is unchanged.
    if dn > bound {
        cpu.set_flag(sr::N, dn < 0);
        cpu.idle(bus, 4);
    } else if dn < 0 {
        cpu.set_flag(sr::N, true);
        cpu.idle(bus, 6);
    } else {
        cpu.idle(bus, 6);
        return Ok(());
    }
    let pc = cpu.pc();
    cpu.exception(bus, vector::CHK, pc, Frame::LowSrHigh)
}

fn tst(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    let sz = Sz::from_bits(op >> 6).expect("matched");
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let v = cpu.src(bus, m, r, sz)?;
    cpu.logic_flags(sz, v);
    cpu.prefetch(bus)
}

/// TAS: Dn "np"; memory: one read-modify-write cycle, then "np".
fn tas(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    match cpu.ea(bus, m, r, Sz::B)?.expect("checked") {
        Ea::D(dr) => {
            let v = cpu.d[dr] & 0xff;
            cpu.logic_flags(Sz::B, v);
            cpu.set_d(dr, Sz::B, v | 0x80);
        }
        Ea::Mem(addr) => {
            let old = cpu.read_modify_write(bus, addr, |v| v | 0x80)?;
            cpu.logic_flags(Sz::B, u32::from(old));
        }
        _ => unreachable!(),
    }
    cpu.prefetch(bus)
}

/// MOVEM. Registers to memory: "np (nw)* np" (longs "nW nw", -(An) "nw nW"
/// in reverse order); memory to registers: "np (nr)* nr np" with one extra
/// read after the last register.
fn movem(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, to_regs: bool) -> Res {
    let (m, r) = mode(op);
    let sz = if op & 0x40 != 0 { Sz::L } else { Sz::W };
    let ok = if to_regs {
        control(m, r) || m == 3
    } else {
        (control(m, r) && !(m == 7 && r >= 2)) || m == 4
    };
    if !ok {
        return illegal(cpu, bus);
    }
    let mask = cpu.ext(bus)?;
    let mut addr = match m {
        3 | 4 => cpu.a[r],
        _ => control_ea_movem(cpu, bus, m, r)?,
    };
    if to_regs {
        for i in 0..16 {
            if mask & (1 << i) == 0 {
                continue;
            }
            // (An)+ runs one word ahead of the transfer (an address error
            // leaves it there).
            if m == 3 {
                cpu.a[r] = addr.wrapping_add(2);
            }
            let v = sz.sext(cpu.read_mem(bus, addr, sz)?);
            if i < 8 {
                cpu.d[i] = v;
            } else {
                cpu.a[i - 8] = v;
            }
            addr = addr.wrapping_add(sz.bytes());
        }
        // The extra read.
        cpu.read(bus, addr, Size::Word)?;
        if m == 3 {
            cpu.a[r] = addr;
        }
        return cpu.prefetch(bus);
    }
    if m == 4 {
        // Predecrement: A7 … A0, D7 … D0 (mask bit 0 = A7).
        for i in 0..16 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let reg = 15 - i;
            let v = if reg < 8 { cpu.d[reg] } else { cpu.a[reg - 8] };
            addr = addr.wrapping_sub(sz.bytes());
            cpu.write_mem(bus, addr, sz, v, true)?;
        }
        // An in the list is stored with its initial value; it changes at
        // the end.
        cpu.a[r] = addr;
    } else {
        for i in 0..16 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let v = if i < 8 { cpu.d[i] } else { cpu.a[i - 8] };
            cpu.write_mem(bus, addr, sz, v, false)?;
            addr = addr.wrapping_add(sz.bytes());
        }
    }
    cpu.prefetch(bus)
}

/// MOVEM's address calculation after the mask word: (d8,An,Xn) and
/// (d8,PC,Xn) "n np".
fn control_ea_movem(cpu: &mut M68000, bus: &mut dyn TimedBus68, m: u16, r: usize) -> Res<u32> {
    Ok(match (m, r) {
        (2, _) => cpu.a[r],
        (5, _) => {
            let d = cpu.ext(bus)? as i16 as i32 as u32;
            cpu.a[r].wrapping_add(d)
        }
        (6, _) => {
            cpu.idle(bus, 2);
            let ext = cpu.ext(bus)?;
            cpu.index(cpu.a[r], ext)
        }
        (7, 0) => cpu.ext(bus)? as i16 as i32 as u32,
        (7, 1) => cpu.ext_long(bus)?,
        (7, 2) => {
            let base = cpu.ext_pc();
            base.wrapping_add(cpu.ext(bus)? as i16 as i32 as u32)
        }
        _ => {
            cpu.idle(bus, 2);
            let base = cpu.ext_pc();
            let ext = cpu.ext(bus)?;
            cpu.index(base, ext)
        }
    })
}

/// $4E40–$4E7F: TRAP, LINK, UNLK, MOVE USP, RESET, NOP, STOP, RTE, RTS,
/// TRAPV, RTR.
fn misc4e(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let r = usize::from(op & 7);
    match op & 0xfff8 {
        0x4e40 | 0x4e48 => {
            // TRAP #n: "nn ns ns nS", the next instruction on the stack.
            let pc = cpu.pc().wrapping_add(2);
            cpu.idle(bus, 4);
            return cpu.exception(bus, vector::TRAP0 + (op & 15) as u8, pc, Frame::LowSrHigh);
        }
        0x4e50 => {
            // LINK An,#d: "np nS ns np".
            let d = cpu.ext(bus)? as i16 as i32 as u32;
            let v = if r == 7 {
                cpu.a[7].wrapping_sub(4)
            } else {
                cpu.a[r]
            };
            cpu.push_long(bus, v)?;
            cpu.a[r] = cpu.a[7];
            cpu.a[7] = cpu.a[7].wrapping_add(d);
            return cpu.prefetch(bus);
        }
        0x4e58 => {
            // UNLK An: "nU nu np".
            cpu.a[7] = cpu.a[r];
            let v = cpu.pop_long(bus)?;
            cpu.a[r] = v;
            return cpu.prefetch(bus);
        }
        0x4e60 => {
            if privileged(cpu, bus)? {
                let v = cpu.a[r];
                cpu.set_usp(v);
                return cpu.prefetch(bus);
            }
            return Ok(());
        }
        0x4e68 => {
            if privileged(cpu, bus)? {
                cpu.a[r] = cpu.usp();
                return cpu.prefetch(bus);
            }
            return Ok(());
        }
        _ => {}
    }
    match op {
        0x4e70 => {
            // RESET: "nn (??-)* np", 132 clocks with /RSTO asserted.
            if !privileged(cpu, bus)? {
                return Ok(());
            }
            cpu.idle(bus, 4);
            bus_reset(bus);
            cpu.idle(bus, 124);
            cpu.prefetch(bus)
        }
        0x4e71 => cpu.prefetch(bus),
        0x4e72 => {
            // STOP #imm: load SR, wait for an interrupt.
            if !privileged(cpu, bus)? {
                return Ok(());
            }
            let v = cpu.ext(bus)?;
            cpu.set_sr(v);
            cpu.set_stopped();
            Ok(())
        }
        0x4e73 => {
            // RTE: "nU nu nu np np" — SR, then PC.
            if !privileged(cpu, bus)? {
                return Ok(());
            }
            let (new_sr, pc) = pop_sr_pc(cpu, bus)?;
            cpu.set_sr(new_sr);
            cpu.jump(bus, pc)?;
            cpu.prefetch(bus)
        }
        0x4e75 => {
            // RTS: "nU nu np np".
            let pc = cpu.pop_long(bus)?;
            cpu.jump(bus, pc)?;
            cpu.prefetch(bus)
        }
        0x4e76 => {
            // TRAPV: "np" or "np ns nS ns" + vector.
            cpu.prefetch(bus)?;
            if !cpu.flag(sr::V) {
                return Ok(());
            }
            let pc = cpu.pc();
            cpu.exception(bus, vector::TRAPV, pc, Frame::LowSrHigh)
        }
        0x4e77 => {
            // RTR: CCR, then PC.
            let (ccr, pc) = pop_sr_pc(cpu, bus)?;
            cpu.set_ccr(ccr);
            cpu.jump(bus, pc)?;
            cpu.prefetch(bus)
        }
        _ => illegal(cpu, bus),
    }
}

/// RTE/RTR's stack reads: PC high word, the status word, PC low word.
fn pop_sr_pc(cpu: &mut M68000, bus: &mut dyn TimedBus68) -> Res<(u16, u32)> {
    let sp = cpu.a[7];
    let hi = cpu.read(bus, sp.wrapping_add(2), Size::Word)?;
    let status = cpu.read(bus, sp, Size::Word)? as u16;
    let lo = cpu.read(bus, sp.wrapping_add(4), Size::Word)?;
    cpu.a[7] = sp.wrapping_add(6);
    Ok((status, hi << 16 | lo))
}

/// The RESET instruction's /RSTO pulse reaches the machine's devices.
fn bus_reset(bus: &mut dyn TimedBus68) {
    bus.reset_devices();
}

/// JSR (push the return address) and JMP.
fn jsr_jmp(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, jsr: bool) -> Res {
    let (m, r) = mode(op);
    if !control(m, r) {
        return illegal(cpu, bus);
    }
    // Return address: the instruction after the extension words.
    let ret = cpu.pc().wrapping_add(match (m, r) {
        (2, _) => 2,
        (7, 1) => 6,
        _ => 4,
    });
    // The extension words are taken from the queue without refilling it,
    // except the first word of an absolute long address.
    let target = match (m, r) {
        (2, _) => cpu.a[r],
        (5, _) => {
            cpu.idle(bus, 2);
            cpu.a[r].wrapping_add(cpu.irc() as i16 as i32 as u32)
        }
        (6, _) => {
            cpu.idle(bus, 6);
            cpu.index(cpu.a[r], cpu.irc())
        }
        (7, 0) => {
            cpu.idle(bus, 2);
            cpu.irc() as i16 as i32 as u32
        }
        (7, 1) => {
            let hi = u32::from(cpu.ext(bus)?);
            hi << 16 | u32::from(cpu.irc())
        }
        (7, 2) => {
            cpu.idle(bus, 2);
            cpu.ext_pc().wrapping_add(cpu.irc() as i16 as i32 as u32)
        }
        _ => {
            cpu.idle(bus, 6);
            cpu.index(cpu.ext_pc(), cpu.irc())
        }
    };
    cpu.jump(bus, target)?;
    if jsr {
        cpu.push_long(bus, ret)?;
    }
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- line 5

fn line5(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if op & 0xc0 == 0xc0 {
        let cc = (op >> 8) & 15;
        if m == 1 {
            return dbcc(cpu, bus, op, cc);
        }
        if !data_alterable(m, r) {
            return illegal(cpu, bus);
        }
        // Scc: Dn "np" (true: "np n"); memory "nr np nw".
        let t = cpu.condition(cc);
        let v = if t { 0xff } else { 0 };
        let ea = cpu.ea(bus, m, r, Sz::B)?.expect("checked");
        if let Ea::D(dr) = ea {
            cpu.set_d(dr, Sz::B, v);
            cpu.prefetch(bus)?;
            if t {
                cpu.idle(bus, 2);
            }
            return Ok(());
        }
        cpu.read_ea(bus, ea, Sz::B)?;
        return cpu.store_rmw(bus, ea, Sz::B, v);
    }
    // ADDQ / SUBQ.
    let sz = Sz::from_bits(op >> 6).expect("not 3");
    let data = match (op >> 9) & 7 {
        0 => 8,
        n => u32::from(n),
    };
    let sub = op & 0x100 != 0;
    if m == 1 {
        if sz == Sz::B {
            return illegal(cpu, bus);
        }
        cpu.a[r] = if sub {
            cpu.a[r].wrapping_sub(data)
        } else {
            cpu.a[r].wrapping_add(data)
        };
        cpu.prefetch(bus)?;
        cpu.idle(bus, if sz == Sz::L { 2 } else { 4 });
        return Ok(());
    }
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let v = if sub {
        cpu.sub(sz, data, d)
    } else {
        cpu.add(sz, data, d)
    };
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 4);
        }
        return Ok(());
    }
    cpu.store_rmw(bus, ea, sz, v)
}

/// DBcc Dn,d16: cc true "n n np np"; branch "n np np"; counter expired
/// "n np np np" (the branch target is read before the counter is known).
fn dbcc(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, cc: u16) -> Res {
    let r = usize::from(op & 7);
    let disp = cpu.irc() as i16 as i32 as u32;
    let target = cpu.ext_pc().wrapping_add(disp);
    cpu.idle(bus, 2);
    if cpu.condition(cc) {
        cpu.idle(bus, 2);
        cpu.ext(bus)?;
        return cpu.prefetch(bus);
    }
    let count = (cpu.d[r] as u16).wrapping_sub(1);
    cpu.set_d(r, Sz::W, u32::from(count));
    if count != 0xffff {
        cpu.jump(bus, target)?;
        return cpu.prefetch(bus);
    }
    // Expired: the target is fetched, then the queue continues after DBcc.
    let after = cpu.au();
    cpu.jump(bus, target)?;
    cpu.set_au(after);
    cpu.fill_irc(bus)?;
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- line 6

/// BRA/Bcc taken "n np np"; not taken .B "nn np", .W "nn np np"; BSR
/// "n nS ns np np".
fn branch(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let cc = (op >> 8) & 15;
    let base = cpu.ext_pc();
    let (disp, word) = match op as u8 {
        0 => (cpu.irc() as i16 as i32 as u32, true),
        d => (d as i8 as i32 as u32, false),
    };
    let target = base.wrapping_add(disp);
    if cc == 1 {
        // BSR: the return address follows the displacement.
        let ret = if word { base.wrapping_add(2) } else { base };
        cpu.idle(bus, 2);
        cpu.push_long(bus, ret)?;
        cpu.jump(bus, target)?;
        return cpu.prefetch(bus);
    }
    if cpu.condition(cc) {
        cpu.idle(bus, 2);
        cpu.jump(bus, target)?;
        return cpu.prefetch(bus);
    }
    cpu.idle(bus, 4);
    if word {
        cpu.ext(bus)?;
    }
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- lines 8, 9, B, C, D

/// OR, DIVU, DIVS, SBCD.
fn line8(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    match op & 0x1f0 {
        0x0c0..=0x0cf | 0x1c0..=0x1cf => {}
        0x100 => return bcd_op(cpu, bus, op, false),
        _ => {}
    }
    if op & 0xc0 == 0xc0 {
        return div(cpu, bus, op, op & 0x100 != 0);
    }
    logic(cpu, bus, op, |d, s| d | s)
}

/// AND, MULU, MULS, ABCD, EXG.
fn line_c(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    if op & 0xc0 == 0xc0 {
        return mul(cpu, bus, op, op & 0x100 != 0);
    }
    match op & 0x1f8 {
        0x100 | 0x108 => return bcd_op(cpu, bus, op, true),
        0x140 | 0x148 | 0x188 => {
            let x = usize::from((op >> 9) & 7);
            let y = usize::from(op & 7);
            match op & 0x1f8 {
                0x140 => cpu.d.swap(x, y),
                0x148 => cpu.a.swap(x, y),
                _ => std::mem::swap(&mut cpu.d[x], &mut cpu.a[y]),
            }
            cpu.prefetch(bus)?;
            cpu.idle(bus, 2);
            return Ok(());
        }
        _ => {}
    }
    logic(cpu, bus, op, |d, s| d & s)
}

/// AND/OR <ea>,Dn and Dn,<ea>.
fn logic(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, f: fn(u32, u32) -> u32) -> Res {
    let (m, r) = mode(op);
    let dn = usize::from((op >> 9) & 7);
    let sz = Sz::from_bits(op >> 6).expect("not 3");
    if op & 0x100 == 0 {
        if !data_mode(m, r) {
            return illegal(cpu, bus);
        }
        let s = cpu.src(bus, m, r, sz)?;
        let v = f(cpu.d[dn], s) & sz.mask();
        cpu.set_d(dn, sz, v);
        cpu.logic_flags(sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, if m == 0 || (m == 7 && r == 4) { 4 } else { 2 });
        }
        return Ok(());
    }
    if !mem_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let v = f(d, cpu.d[dn]) & sz.mask();
    cpu.logic_flags(sz, v);
    cpu.store_rmw(bus, ea, sz, v)
}

/// ADD/SUB, ADDA/SUBA, ADDX/SUBX.
fn add_sub(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, add: bool) -> Res {
    let (m, r) = mode(op);
    let reg = usize::from((op >> 9) & 7);
    if op & 0xc0 == 0xc0 {
        // ADDA/SUBA: "np nn" (longs from memory "np n").
        let sz = if op & 0x100 != 0 { Sz::L } else { Sz::W };
        if !any_mode(m, r) {
            return illegal(cpu, bus);
        }
        let s = sz.sext(cpu.src(bus, m, r, sz)?);
        cpu.a[reg] = if add {
            cpu.a[reg].wrapping_add(s)
        } else {
            cpu.a[reg].wrapping_sub(s)
        };
        cpu.prefetch(bus)?;
        let short = sz == Sz::L && m != 0 && m != 1 && !(m == 7 && r == 4);
        cpu.idle(bus, if short { 2 } else { 4 });
        return Ok(());
    }
    let sz = Sz::from_bits(op >> 6).expect("not 3");
    if op & 0x130 == 0x100 {
        return addx_subx(cpu, bus, op, sz, add);
    }
    if op & 0x100 == 0 {
        // <ea>,Dn.
        if !any_mode(m, r) || (m == 1 && sz == Sz::B) {
            return illegal(cpu, bus);
        }
        let s = cpu.src(bus, m, r, sz)?;
        let d = cpu.d[reg];
        let v = if add {
            cpu.add(sz, s, d)
        } else {
            cpu.sub(sz, s, d)
        };
        cpu.set_d(reg, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            let regs = m <= 1 || (m == 7 && r == 4);
            cpu.idle(bus, if regs { 4 } else { 2 });
        }
        return Ok(());
    }
    if !mem_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let s = cpu.d[reg];
    let v = if add {
        cpu.add(sz, s, d)
    } else {
        cpu.sub(sz, s, d)
    };
    cpu.store_rmw(bus, ea, sz, v)
}

/// ADDX/SUBX Dy,Dx "np" (longs "np nn"); -(Ay),-(Ax) "n nr nr np nw"
/// (longs "n nr nR nr nR nw np nW").
fn addx_subx(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, sz: Sz, add: bool) -> Res {
    let rx = usize::from((op >> 9) & 7);
    let ry = usize::from(op & 7);
    let x = cpu.x();
    let calc = |cpu: &mut M68000, s: u32, d: u32| {
        let r = if add {
            d.wrapping_add(s).wrapping_add(x)
        } else {
            d.wrapping_sub(s).wrapping_sub(x)
        };
        if add {
            cpu.add_flags(sz, s, d, r, true);
        } else {
            cpu.sub_flags(sz, s, d, r, true, false);
        }
        r & sz.mask()
    };
    if op & 8 == 0 {
        let v = calc(cpu, cpu.d[ry] & sz.mask(), cpu.d[rx] & sz.mask());
        cpu.set_d(rx, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 4);
        }
        return Ok(());
    }
    cpu.idle(bus, 2);
    let step = |r: usize| if r == 7 && sz == Sz::B { 2 } else { sz.bytes() };
    if sz == Sz::L {
        // Low words first: source, then destination.
        cpu.a[ry] = cpu.a[ry].wrapping_sub(2);
        let s_lo = cpu.read(bus, cpu.a[ry], Size::Word)?;
        cpu.a[ry] = cpu.a[ry].wrapping_sub(2);
        let s_hi = cpu.read(bus, cpu.a[ry], Size::Word)?;
        cpu.a[rx] = cpu.a[rx].wrapping_sub(2);
        let d_lo = cpu.read(bus, cpu.a[rx], Size::Word)?;
        cpu.a[rx] = cpu.a[rx].wrapping_sub(2);
        let d_hi = cpu.read(bus, cpu.a[rx], Size::Word)?;
        let v = calc(cpu, s_hi << 16 | s_lo, d_hi << 16 | d_lo);
        let addr = cpu.a[rx];
        cpu.write(bus, addr.wrapping_add(2), Size::Word, v & 0xffff)?;
        cpu.prefetch(bus)?;
        return cpu.write(bus, addr, Size::Word, v >> 16);
    }
    cpu.a[ry] = cpu.a[ry].wrapping_sub(step(ry));
    let s = cpu.read_mem(bus, cpu.a[ry], sz)?;
    cpu.a[rx] = cpu.a[rx].wrapping_sub(step(rx));
    let d = cpu.read_mem(bus, cpu.a[rx], sz)?;
    let v = calc(cpu, s, d);
    cpu.prefetch(bus)?;
    cpu.write_mem(bus, cpu.a[rx], sz, v, true)
}

/// CMP, CMPA, CMPM, EOR.
fn line_b(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    let reg = usize::from((op >> 9) & 7);
    if op & 0xc0 == 0xc0 {
        // CMPA: "np n".
        let sz = if op & 0x100 != 0 { Sz::L } else { Sz::W };
        if !any_mode(m, r) {
            return illegal(cpu, bus);
        }
        let s = sz.sext(cpu.src(bus, m, r, sz)?);
        let d = cpu.a[reg];
        cpu.cmp(Sz::L, s, d);
        cpu.prefetch(bus)?;
        cpu.idle(bus, 2);
        return Ok(());
    }
    let sz = Sz::from_bits(op >> 6).expect("not 3");
    if op & 0x100 == 0 {
        // CMP <ea>,Dn: "np" (longs "np n").
        if !any_mode(m, r) || (m == 1 && sz == Sz::B) {
            return illegal(cpu, bus);
        }
        let s = cpu.src(bus, m, r, sz)?;
        let d = cpu.d[reg] & sz.mask();
        cpu.cmp(sz, s, d);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 2);
        }
        return Ok(());
    }
    if m == 1 {
        // CMPM (Ay)+,(Ax)+: "nr nr np" (longs "nR nr nR nr np").
        let s = cpu.src(bus, 3, r, sz)?;
        let d = cpu.src(bus, 3, reg, sz)?;
        cpu.cmp(sz, s, d);
        return cpu.prefetch(bus);
    }
    // EOR Dn,<ea>.
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, sz)?.expect("checked");
    let d = cpu.read_ea(bus, ea, sz)?;
    let v = (d ^ cpu.d[reg]) & sz.mask();
    cpu.logic_flags(sz, v);
    if let Ea::D(dr) = ea {
        cpu.set_d(dr, sz, v);
        cpu.prefetch(bus)?;
        if sz == Sz::L {
            cpu.idle(bus, 4);
        }
        return Ok(());
    }
    cpu.store_rmw(bus, ea, sz, v)
}

/// MULU/MULS: "np" then 17 + m internal cycles (m: ones in the source for
/// MULU, 01/10 transitions in the source with a 0 appended for MULS).
fn mul(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, signed: bool) -> Res {
    let (m, r) = mode(op);
    if !data_mode(m, r) {
        return illegal(cpu, bus);
    }
    let s = cpu.src(bus, m, r, Sz::W)?;
    let dn = usize::from((op >> 9) & 7);
    let (v, ones) = if signed {
        let v = (s as u16 as i16 as i32).wrapping_mul(cpu.d[dn] as u16 as i16 as i32) as u32;
        let pattern = (s << 1) & 0x1ffff;
        ((v), ((pattern ^ (pattern >> 1)) & 0xffff).count_ones())
    } else {
        (
            (s & 0xffff) * (cpu.d[dn] & 0xffff),
            (s & 0xffff).count_ones(),
        )
    };
    cpu.d[dn] = v;
    cpu.logic_flags(Sz::L, v);
    cpu.prefetch(bus)?;
    cpu.idle(bus, 2 * (17 + ones));
    Ok(())
}

/// DIVU/DIVS: the division's internal cycles follow the 68000's
/// shift-and-subtract loop (YACHT flowcharts); by zero: "nn nn" + trap.
fn div(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, signed: bool) -> Res {
    let (m, r) = mode(op);
    if !data_mode(m, r) {
        return illegal(cpu, bus);
    }
    let s = cpu.src(bus, m, r, Sz::W)? & 0xffff;
    let dn = usize::from((op >> 9) & 7);
    let dividend = cpu.d[dn];
    cpu.set_flag(sr::C, false);
    if s == 0 {
        // N, Z, V are "undefined" in the manual; the 68000 clears them
        // (Harte data, DIVU test 5745).
        for f in [sr::N, sr::Z, sr::V] {
            cpu.set_flag(f, false);
        }
        cpu.idle(bus, 8);
        // The next instruction's address (its first word is in IRC).
        let pc = cpu.au().wrapping_sub(2);
        return cpu.exception(bus, vector::ZERO_DIVIDE, pc, Frame::LowSrHigh);
    }
    if !signed {
        let q = dividend / s;
        if q > 0xffff {
            cpu.set_flag(sr::V, true);
            cpu.idle(bus, 6);
            return cpu.prefetch(bus);
        }
        let rem = dividend % s;
        cpu.d[dn] = rem << 16 | q;
        cpu.set_flag(sr::V, false);
        cpu.set_flag(sr::N, q & 0x8000 != 0);
        cpu.set_flag(sr::Z, q == 0);
        cpu.idle(bus, divu_clocks(dividend, s) - 4);
        return cpu.prefetch(bus);
    }
    let a = dividend as i32;
    let b = s as u16 as i16 as i32;
    let q = (a as i64) / (b as i64);
    if !(-0x8000..=0x7fff).contains(&q) {
        cpu.set_flag(sr::V, true);
        cpu.idle(bus, divs_clocks(a, b) - 4);
        return cpu.prefetch(bus);
    }
    let rem = ((a as i64) % (b as i64)) as i32;
    let q = q as i32;
    cpu.d[dn] = (rem as u32) << 16 | (q as u32 & 0xffff);
    cpu.set_flag(sr::V, false);
    cpu.set_flag(sr::N, q < 0);
    cpu.set_flag(sr::Z, q == 0);
    cpu.idle(bus, divs_clocks(a, b) - 4);
    cpu.prefetch(bus)
}

/// DIVU clocks after the operand (including the closing prefetch), from
/// the 68000's restoring division: per quotient bit 2 clocks plus 2 more
/// when the shifted-out bit was 0, less 1 microcycle when it subtracts.
fn divu_clocks(dividend: u32, divisor: u32) -> u32 {
    let mut cycles = 38u32;
    let hdivisor = divisor << 16;
    let mut d = dividend;
    for _ in 0..15 {
        let msb = d & 0x8000_0000 != 0;
        d <<= 1;
        if msb {
            d = d.wrapping_sub(hdivisor);
        } else {
            cycles += 2;
            if d >= hdivisor {
                d -= hdivisor;
                cycles -= 1;
            }
        }
    }
    cycles * 2
}

/// DIVS clocks after the operand (including the closing prefetch): a sign
/// fix-up around an unsigned division of the magnitudes, with one more
/// microcycle per quotient bit that comes out 0.
fn divs_clocks(dividend: i32, divisor: i32) -> u32 {
    let mut cycles = 6u32;
    if dividend < 0 {
        cycles += 1;
    }
    let (ad, as_) = (dividend.unsigned_abs(), divisor.unsigned_abs());
    // An overflow (the quotient does not fit 16 bits signed) is found early.
    let q = i64::from(dividend) / i64::from(divisor);
    if !(-0x8000..=0x7fff).contains(&q) {
        return (cycles + 2) * 2;
    }
    cycles += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            cycles -= 1;
        } else {
            cycles += 1;
        }
    }
    let mut aq = ad / as_;
    for _ in 0..15 {
        if aq & 0x8000 == 0 {
            cycles += 1;
        }
        aq <<= 1;
    }
    cycles * 2
}

/// ABCD/SBCD Dy,Dx "np n"; -(Ay),-(Ax) "n nr nr np nw".
fn bcd_op(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16, add: bool) -> Res {
    let rx = usize::from((op >> 9) & 7);
    let ry = usize::from(op & 7);
    if op & 8 == 0 {
        let (s, d) = (cpu.d[ry] & 0xff, cpu.d[rx] & 0xff);
        let v = if add {
            bcd_add(cpu, s, d)
        } else {
            bcd_sub(cpu, s, d)
        };
        cpu.set_d(rx, Sz::B, v);
        cpu.prefetch(bus)?;
        cpu.idle(bus, 2);
        return Ok(());
    }
    cpu.idle(bus, 2);
    let step = |r: usize| if r == 7 { 2 } else { 1 };
    cpu.a[ry] = cpu.a[ry].wrapping_sub(step(ry));
    let s = cpu.read(bus, cpu.a[ry], Size::Byte)?;
    cpu.a[rx] = cpu.a[rx].wrapping_sub(step(rx));
    let d = cpu.read(bus, cpu.a[rx], Size::Byte)?;
    let v = if add {
        bcd_add(cpu, s, d)
    } else {
        bcd_sub(cpu, s, d)
    };
    cpu.prefetch(bus)?;
    cpu.write(bus, cpu.a[rx], Size::Byte, v)
}

/// d + s + X in BCD, valid for any input bytes: the correction comes from
/// the binary carries out of each nibble and the decimal carry; V and N
/// follow the corrected result (after the analysis of the 68000's BCD
/// instructions by Flamewing). Z is only cleared.
fn bcd_add(cpu: &mut M68000, s: u32, d: u32) -> u32 {
    let x = cpu.x();
    let ss = d + s + x;
    let bc = ((d & s) | (!ss & (d | s))) & 0x88;
    let dc = (((ss + 0x66) ^ ss) & 0x110) >> 1;
    let corf = (bc | dc) - ((bc | dc) >> 2);
    let rr = ss + corf;
    let carry = (bc | (ss & !rr)) & 0x80 != 0;
    let r = rr & 0xff;
    cpu.set_flag(sr::C, carry);
    cpu.set_flag(sr::X, carry);
    cpu.set_flag(sr::V, !ss & rr & 0x80 != 0);
    cpu.set_flag(sr::N, r & 0x80 != 0);
    if r != 0 {
        cpu.set_flag(sr::Z, false);
    }
    r
}

/// d − s − X in BCD (SBCD, NBCD with d = 0).
fn bcd_sub(cpu: &mut M68000, s: u32, d: u32) -> u32 {
    let x = cpu.x();
    let dd = d.wrapping_sub(s).wrapping_sub(x);
    let bc = ((!d & s) | (dd & !(d ^ s))) & 0x88;
    let corf = bc - (bc >> 2);
    let rr = dd.wrapping_sub(corf);
    let carry = (bc | (!dd & rr)) & 0x80 != 0;
    let r = rr & 0xff;
    cpu.set_flag(sr::C, carry);
    cpu.set_flag(sr::X, carry);
    cpu.set_flag(sr::V, dd & !rr & 0x80 != 0);
    cpu.set_flag(sr::N, r & 0x80 != 0);
    if r != 0 {
        cpu.set_flag(sr::Z, false);
    }
    r
}

// ---------------------------------------------------------------- line E

/// Shifts and rotates. Registers: "np n* n" (longs "np n* nn"), n* = one
/// microcycle per bit; memory (word, one bit): "nr np nw".
fn shifts(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let left = op & 0x100 != 0;
    if op & 0xc0 == 0xc0 {
        let (m, r) = mode(op);
        if op & 0x800 != 0 || !mem_alterable(m, r) {
            return illegal(cpu, bus);
        }
        let ea = cpu.ea(bus, m, r, Sz::W)?.expect("checked");
        let v = cpu.read_ea(bus, ea, Sz::W)?;
        let res = shift(cpu, (op >> 9) & 3, left, Sz::W, v, 1);
        return cpu.store_rmw(bus, ea, Sz::W, res);
    }
    let sz = Sz::from_bits(op >> 6).expect("not 3");
    let dy = usize::from(op & 7);
    let count = if op & 0x20 != 0 {
        cpu.d[usize::from((op >> 9) & 7)] & 63
    } else {
        match (op >> 9) & 7 {
            0 => 8,
            n => u32::from(n),
        }
    };
    let v = cpu.d[dy] & sz.mask();
    let res = shift(cpu, (op >> 3) & 3, left, sz, v, count);
    cpu.set_d(dy, sz, res);
    cpu.prefetch(bus)?;
    cpu.idle(bus, 2 * count + if sz == Sz::L { 4 } else { 2 });
    Ok(())
}

/// One shift/rotate of `v` by `count` (kind: 0 AS, 1 LS, 2 ROX, 3 RO).
fn shift(cpu: &mut M68000, kind: u16, left: bool, sz: Sz, v: u32, count: u32) -> u32 {
    let bits = sz.bytes() * 8;
    let msb = sz.msb();
    let mask = sz.mask();
    let mut v = v & mask;
    let mut c = false;
    let mut overflow = false;
    match kind {
        0 | 1 => {
            for _ in 0..count {
                if left {
                    c = v & msb != 0;
                    let nv = (v << 1) & mask;
                    if (nv ^ v) & msb != 0 {
                        overflow = true;
                    }
                    v = nv;
                } else {
                    c = v & 1 != 0;
                    let sign = if kind == 0 { v & msb } else { 0 };
                    v = (v >> 1) | sign;
                }
            }
            // Shifted right past the operand's width, ASR ends with C and
            // X clear (Harte data), although the result is all sign bits.
            if kind == 0 && !left && count > sz.bytes() * 8 {
                c = false;
            }
            if count > 0 {
                cpu.set_flag(sr::X, c);
            }
            cpu.set_flag(sr::V, kind == 0 && overflow);
            cpu.set_flag(sr::C, count > 0 && c);
        }
        2 => {
            let mut x = cpu.flag(sr::X);
            for _ in 0..count {
                if left {
                    let out = v & msb != 0;
                    v = ((v << 1) | u32::from(x)) & mask;
                    x = out;
                } else {
                    let out = v & 1 != 0;
                    v = (v >> 1) | if x { msb } else { 0 };
                    x = out;
                }
            }
            cpu.set_flag(sr::X, x);
            cpu.set_flag(sr::C, x);
            cpu.set_flag(sr::V, false);
        }
        _ => {
            let n = count % bits;
            if left {
                v = ((v << n) | (v >> ((bits - n) % bits))) & mask;
                if n == 0 {
                    v &= mask;
                }
            } else {
                v = ((v >> n) | (v << ((bits - n) % bits))) & mask;
            }
            let c = count > 0 && if left { v & 1 != 0 } else { v & msb != 0 };
            cpu.set_flag(sr::C, c);
            cpu.set_flag(sr::V, false);
        }
    }
    cpu.set_flag(sr::N, v & msb != 0);
    cpu.set_flag(sr::Z, v == 0);
    v
}
