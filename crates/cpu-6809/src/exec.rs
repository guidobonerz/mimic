//! Instruction execution. Condition codes as in the programming manual's
//! instruction descriptions; H is defined for the 8-bit additions only.

use crate::table::{self, Mode};
use crate::{M6809Bus, Wait, C, E, F, H, I, M6809, N, V, VEC_SWI, VEC_SWI2, VEC_SWI3, Z};

pub(crate) fn execute(c: &mut M6809, bus: &mut dyn M6809Bus) {
    let op = c.fetch(bus);
    match op {
        0x10 | 0x11 => {
            // Further prefixes are ignored; the last one counts.
            let mut page = op;
            let mut op2 = c.fetch(bus);
            while op2 == 0x10 || op2 == 0x11 {
                page = op2;
                op2 = c.fetch(bus);
            }
            let info = if page == 0x10 {
                table::page2(op2)
            } else {
                table::page3(op2)
            };
            match info {
                Some((mode, cycles)) => {
                    c.target += u32::from(cycles);
                    if page == 0x10 {
                        page2(c, bus, op2, mode);
                    } else {
                        page3(c, bus, op2, mode);
                    }
                }
                // Undefined: runs as the page-0 opcode.
                None => {
                    c.target += 1;
                    page0(c, bus, op2);
                }
            }
        }
        _ => page0(c, bus, op),
    }
}

fn page0(c: &mut M6809, bus: &mut dyn M6809Bus, op: u8) {
    let Some((mode, cycles)) = table::page0(op) else {
        match op {
            0x14 | 0x15 | 0xcd => c.wait = Wait::Halt,
            0x87 | 0xc7 => {
                c.fetch(bus);
            }
            0x8f | 0xcf => {
                c.fetch16(bus);
            }
            _ => {}
        }
        c.target += 2;
        return;
    };
    c.target += u32::from(cycles);
    match op {
        0x00..=0x0f | 0x60..=0x7f => rmw_mem(c, bus, op, mode),
        0x40..=0x4f => c.a = rmw(c, op, c.a),
        0x50..=0x5f => c.b = rmw(c, op, c.b),
        0x12 | 0x1b => {}
        0x13 => c.wait = Wait::Sync,
        0x16 => {
            let off = c.fetch16(bus);
            c.pc = c.pc.wrapping_add(off);
        }
        0x17 => {
            let off = c.fetch16(bus);
            c.idle(bus);
            c.idle(bus);
            c.idle(bus);
            c.idle(bus);
            c.push_s(bus, 0x80);
            c.pc = c.pc.wrapping_add(off);
        }
        0x19 => daa(c),
        0x1a => c.cc |= c.fetch(bus),
        0x1c => c.cc &= c.fetch(bus),
        0x1d => {
            c.a = if c.b & 0x80 != 0 { 0xff } else { 0 };
            set_nz16(c, c.d());
        }
        0x1e | 0x1f => {
            let post = c.fetch(bus);
            let (r1, r2) = (post >> 4, post & 15);
            let v1 = get_reg(c, r1);
            if op == 0x1e {
                let v2 = get_reg(c, r2);
                set_reg(c, r1, v2);
            }
            set_reg(c, r2, v1);
        }
        0x20..=0x2f => {
            let off = c.fetch(bus) as i8;
            if cond(c, op) {
                c.pc = c.pc.wrapping_add(off as u16);
            }
        }
        0x30..=0x33 => {
            let ea = addr(c, bus, mode);
            match op {
                0x30 => {
                    c.x = ea;
                    set_z(c, ea == 0);
                }
                0x31 => {
                    c.y = ea;
                    set_z(c, ea == 0);
                }
                0x32 => c.set_s(ea),
                _ => c.u = ea,
            }
        }
        0x34 | 0x36 => {
            let mask = c.fetch(bus);
            c.idle(bus);
            c.idle(bus);
            c.target += u32::from(stack_bytes(mask));
            if op == 0x34 {
                let (mut sp, other) = (c.s, c.u);
                c.push(bus, &mut sp, other, mask);
                c.s = sp;
            } else {
                let (mut sp, other) = (c.u, c.s);
                c.push(bus, &mut sp, other, mask);
                c.u = sp;
            }
        }
        0x35 | 0x37 => {
            let mask = c.fetch(bus);
            c.idle(bus);
            c.idle(bus);
            c.target += u32::from(stack_bytes(mask));
            if op == 0x35 {
                let mut sp = c.s;
                let other = c.pull(bus, &mut sp, mask);
                c.set_s(sp);
                if let Some(u) = other {
                    c.u = u;
                }
            } else {
                let mut sp = c.u;
                let other = c.pull(bus, &mut sp, mask);
                c.u = sp;
                if let Some(s) = other {
                    c.set_s(s);
                }
            }
        }
        0x39 => {
            c.idle(bus);
            let mut sp = c.s;
            c.pull(bus, &mut sp, 0x80);
            c.s = sp;
        }
        0x3a => c.x = c.x.wrapping_add(u16::from(c.b)),
        0x3b => {
            c.idle(bus);
            let mut sp = c.s;
            c.pull(bus, &mut sp, 0x01);
            let mask = if c.cc & E != 0 {
                c.target += 9;
                0xfe
            } else {
                0x80
            };
            c.pull(bus, &mut sp, mask);
            c.s = sp;
        }
        0x3c => {
            let m = c.fetch(bus);
            c.cc = (c.cc & m) | E;
            c.idle(bus);
            c.idle(bus);
            c.push_s(bus, 0xff);
            c.wait = Wait::Cwai;
        }
        0x3d => {
            let r = u16::from(c.a) * u16::from(c.b);
            c.set_d(r);
            c.cc &= !(Z | C);
            if r == 0 {
                c.cc |= Z;
            }
            if r & 0x80 != 0 {
                c.cc |= C;
            }
        }
        0x3f => software_interrupt(c, bus, VEC_SWI, true),
        0x8d => {
            let off = c.fetch(bus) as i8;
            c.idle(bus);
            c.idle(bus);
            c.idle(bus);
            c.push_s(bus, 0x80);
            c.pc = c.pc.wrapping_add(off as u16);
        }
        0x80..=0xff => alu(c, bus, op, mode),
        _ => {}
    }
}

fn page2(c: &mut M6809, bus: &mut dyn M6809Bus, op: u8, mode: Mode) {
    match op {
        0x21..=0x2f => {
            let off = c.fetch16(bus);
            if cond(c, op) {
                c.target += 1;
                c.pc = c.pc.wrapping_add(off);
            }
        }
        0x3f => software_interrupt(c, bus, VEC_SWI2, false),
        0x83 | 0x93 | 0xa3 | 0xb3 => {
            let m = operand16(c, bus, mode);
            sub16(c, c.d(), m);
        }
        0x8c | 0x9c | 0xac | 0xbc => {
            let m = operand16(c, bus, mode);
            sub16(c, c.y, m);
        }
        0x8e | 0x9e | 0xae | 0xbe => {
            c.y = operand16(c, bus, mode);
            set_nz16_v0(c, c.y);
        }
        0xce | 0xde | 0xee | 0xfe => {
            let v = operand16(c, bus, mode);
            c.set_s(v);
            set_nz16_v0(c, v);
        }
        0x9f | 0xaf | 0xbf => store16(c, bus, mode, c.y),
        _ => store16(c, bus, mode, c.s),
    }
}

fn page3(c: &mut M6809, bus: &mut dyn M6809Bus, op: u8, mode: Mode) {
    match op {
        0x3f => software_interrupt(c, bus, VEC_SWI3, false),
        0x83 | 0x93 | 0xa3 | 0xb3 => {
            let m = operand16(c, bus, mode);
            sub16(c, c.u, m);
        }
        _ => {
            let m = operand16(c, bus, mode);
            sub16(c, c.s, m);
        }
    }
}

fn software_interrupt(c: &mut M6809, bus: &mut dyn M6809Bus, vector: u16, mask: bool) {
    c.idle(bus);
    c.cc |= E;
    c.push_s(bus, 0xff);
    if mask {
        c.cc |= I | F;
    }
    c.idle(bus);
    c.pc = c.rd16(bus, vector);
}

fn stack_bytes(mask: u8) -> u8 {
    (0..8)
        .filter(|b| mask & (1 << b) != 0)
        .map(|b| if b >= 4 { 2 } else { 1 })
        .sum()
}

/// TFR/EXG register codes: 0 D, 1 X, 2 Y, 3 U, 4 S, 5 PC, 8 A, 9 B, 10 CC,
/// 11 DP. An 8-bit register read as 16 bits has $FF above it; undefined
/// codes read $FFFF.
fn get_reg(c: &M6809, r: u8) -> u16 {
    match r {
        0 => c.d(),
        1 => c.x,
        2 => c.y,
        3 => c.u,
        4 => c.s,
        5 => c.pc,
        8 => 0xff00 | u16::from(c.a),
        9 => 0xff00 | u16::from(c.b),
        10 => 0xff00 | u16::from(c.cc),
        11 => 0xff00 | u16::from(c.dp),
        _ => 0xffff,
    }
}

fn set_reg(c: &mut M6809, r: u8, v: u16) {
    match r {
        0 => c.set_d(v),
        1 => c.x = v,
        2 => c.y = v,
        3 => c.u = v,
        4 => c.set_s(v),
        5 => c.pc = v,
        8 => c.a = v as u8,
        9 => c.b = v as u8,
        10 => c.cc = v as u8,
        11 => c.dp = v as u8,
        _ => {}
    }
}

fn cond(c: &M6809, op: u8) -> bool {
    let f = |m| c.cc & m != 0;
    let nv = f(N) != f(V);
    match op & 15 {
        0 => true,
        1 => false,
        2 => !f(C) && !f(Z),
        3 => f(C) || f(Z),
        4 => !f(C),
        5 => f(C),
        6 => !f(Z),
        7 => f(Z),
        8 => !f(V),
        9 => f(V),
        10 => !f(N),
        11 => f(N),
        12 => !nv,
        13 => nv,
        14 => !f(Z) && !nv,
        _ => f(Z) || nv,
    }
}

/// The effective address of a memory operand; immediates address the
/// bytes after the opcode.
fn addr(c: &mut M6809, bus: &mut dyn M6809Bus, mode: Mode) -> u16 {
    match mode {
        Mode::Imm8 => {
            let a = c.pc;
            c.pc = a.wrapping_add(1);
            a
        }
        Mode::Imm16 => {
            let a = c.pc;
            c.pc = a.wrapping_add(2);
            a
        }
        Mode::Dir => {
            let lo = c.fetch(bus);
            c.idle(bus);
            u16::from_be_bytes([c.dp, lo])
        }
        Mode::Ext => {
            let a = c.fetch16(bus);
            c.idle(bus);
            a
        }
        _ => {
            let a = indexed(c, bus);
            c.idle(bus);
            a
        }
    }
}

/// Decode an indexed postbyte. The cycles the datasheet adds for the mode
/// are spent here (offset bytes read, the rest dead cycles).
fn indexed(c: &mut M6809, bus: &mut dyn M6809Bus) -> u16 {
    let post = c.fetch(bus);
    let r = (post >> 5) & 3;
    let base = match r {
        0 => c.x,
        1 => c.y,
        2 => c.u,
        _ => c.s,
    };
    if post & 0x80 == 0 {
        let off = (((post & 0x1f) << 3) as i8 >> 3) as u16;
        c.target += 1;
        c.idle(bus);
        return base.wrapping_add(off);
    }
    let set = |c: &mut M6809, v: u16| match r {
        0 => c.x = v,
        1 => c.y = v,
        2 => c.u = v,
        _ => c.set_s(v),
    };
    // (address, cycles added, of them operand bytes read)
    let (ea, extra, read): (u16, u8, u8) = match post & 15 {
        0 => {
            set(c, base.wrapping_add(1));
            (base, 2, 0)
        }
        1 => {
            set(c, base.wrapping_add(2));
            (base, 3, 0)
        }
        2 => {
            let v = base.wrapping_sub(1);
            set(c, v);
            (v, 2, 0)
        }
        3 => {
            let v = base.wrapping_sub(2);
            set(c, v);
            (v, 3, 0)
        }
        5 => (base.wrapping_add(c.b as i8 as u16), 1, 0),
        6 => (base.wrapping_add(c.a as i8 as u16), 1, 0),
        8 => {
            let off = c.fetch(bus) as i8 as u16;
            (base.wrapping_add(off), 1, 1)
        }
        9 => {
            let off = c.fetch16(bus);
            (base.wrapping_add(off), 4, 2)
        }
        11 => (base.wrapping_add(c.d()), 4, 0),
        12 => {
            let off = c.fetch(bus) as i8 as u16;
            (c.pc.wrapping_add(off), 1, 1)
        }
        13 => {
            let off = c.fetch16(bus);
            (c.pc.wrapping_add(off), 5, 2)
        }
        15 => (c.fetch16(bus), 2, 2),
        // 4 (,R) and the undefined 7, 10, 14.
        _ => (base, 0, 0),
    };
    for _ in read..extra {
        c.idle(bus);
    }
    c.target += u32::from(extra);
    if post & 0x10 == 0 {
        return ea;
    }
    c.target += 3;
    c.idle(bus);
    c.rd16(bus, ea)
}

fn operand8(c: &mut M6809, bus: &mut dyn M6809Bus, mode: Mode) -> u8 {
    if mode == Mode::Imm8 {
        return c.fetch(bus);
    }
    let a = addr(c, bus, mode);
    c.rd(bus, a)
}

fn operand16(c: &mut M6809, bus: &mut dyn M6809Bus, mode: Mode) -> u16 {
    if mode == Mode::Imm16 {
        return c.fetch16(bus);
    }
    let a = addr(c, bus, mode);
    c.rd16(bus, a)
}

fn store16(c: &mut M6809, bus: &mut dyn M6809Bus, mode: Mode, v: u16) {
    let a = addr(c, bus, mode);
    set_nz16_v0(c, v);
    c.wr16(bus, a, v);
}

fn set_z(c: &mut M6809, z: bool) {
    c.cc &= !Z;
    if z {
        c.cc |= Z;
    }
}

fn set_nz8(c: &mut M6809, v: u8) {
    c.cc &= !(N | Z);
    if v & 0x80 != 0 {
        c.cc |= N;
    }
    if v == 0 {
        c.cc |= Z;
    }
}

fn set_nz16(c: &mut M6809, v: u16) {
    c.cc &= !(N | Z);
    if v & 0x8000 != 0 {
        c.cc |= N;
    }
    if v == 0 {
        c.cc |= Z;
    }
}

fn set_nz8_v0(c: &mut M6809, v: u8) {
    set_nz8(c, v);
    c.cc &= !V;
}

fn set_nz16_v0(c: &mut M6809, v: u16) {
    set_nz16(c, v);
    c.cc &= !V;
}

fn sub8(c: &mut M6809, a: u8, m: u8, borrow: bool) -> u8 {
    let r16 = u16::from(a)
        .wrapping_sub(u16::from(m))
        .wrapping_sub(u16::from(borrow));
    let r = r16 as u8;
    set_nz8(c, r);
    c.cc &= !(V | C);
    if r16 & 0x100 != 0 {
        c.cc |= C;
    }
    if (a ^ m) & (a ^ r) & 0x80 != 0 {
        c.cc |= V;
    }
    r
}

fn add8(c: &mut M6809, a: u8, m: u8, carry: bool) -> u8 {
    let r16 = u16::from(a) + u16::from(m) + u16::from(carry);
    let r = r16 as u8;
    set_nz8(c, r);
    c.cc &= !(V | C | H);
    if r16 & 0x100 != 0 {
        c.cc |= C;
    }
    if !(a ^ m) & (a ^ r) & 0x80 != 0 {
        c.cc |= V;
    }
    if (a ^ m ^ r) & 0x10 != 0 {
        c.cc |= H;
    }
    r
}

fn sub16(c: &mut M6809, a: u16, m: u16) -> u16 {
    let r32 = u32::from(a).wrapping_sub(u32::from(m));
    let r = r32 as u16;
    set_nz16(c, r);
    c.cc &= !(V | C);
    if r32 & 0x10000 != 0 {
        c.cc |= C;
    }
    if (a ^ m) & (a ^ r) & 0x8000 != 0 {
        c.cc |= V;
    }
    r
}

fn add16(c: &mut M6809, a: u16, m: u16) -> u16 {
    let r32 = u32::from(a) + u32::from(m);
    let r = r32 as u16;
    set_nz16(c, r);
    c.cc &= !(V | C);
    if r32 & 0x10000 != 0 {
        c.cc |= C;
    }
    if !(a ^ m) & (a ^ r) & 0x8000 != 0 {
        c.cc |= V;
    }
    r
}

/// The read-modify-write operations ($0x, $4x–$7x) on a byte.
fn rmw(c: &mut M6809, op: u8, m: u8) -> u8 {
    let carry = c.cc & C != 0;
    match op & 15 {
        0 | 1 => sub8(c, 0, m, false),
        // Undefined $x2: NEG, or COM when C is set.
        2 if !carry => sub8(c, 0, m, false),
        2 | 3 => {
            let r = !m;
            set_nz8_v0(c, r);
            c.cc |= C;
            r
        }
        4 | 5 => {
            let r = m >> 1;
            set_nz8(c, r);
            c.cc = (c.cc & !C) | (m & 1);
            r
        }
        6 => {
            let r = (m >> 1) | (u8::from(carry) << 7);
            set_nz8(c, r);
            c.cc = (c.cc & !C) | (m & 1);
            r
        }
        7 => {
            let r = (m >> 1) | (m & 0x80);
            set_nz8(c, r);
            c.cc = (c.cc & !C) | (m & 1);
            r
        }
        8 | 9 => {
            let r = (m << 1) | if op & 15 == 9 { u8::from(carry) } else { 0 };
            set_nz8(c, r);
            c.cc &= !(V | C);
            c.cc |= m >> 7;
            if (m ^ (m << 1)) & 0x80 != 0 {
                c.cc |= V;
            }
            r
        }
        10 | 11 => {
            let r = m.wrapping_sub(1);
            set_nz8(c, r);
            c.cc &= !V;
            if m == 0x80 {
                c.cc |= V;
            }
            r
        }
        12 => {
            let r = m.wrapping_add(1);
            set_nz8(c, r);
            c.cc &= !V;
            if m == 0x7f {
                c.cc |= V;
            }
            r
        }
        13 => {
            set_nz8_v0(c, m);
            m
        }
        // $xF CLR (and the undefined $4E/$5E).
        _ => {
            c.cc = (c.cc & !(N | V | C)) | Z;
            0
        }
    }
}

fn rmw_mem(c: &mut M6809, bus: &mut dyn M6809Bus, op: u8, mode: Mode) {
    let a = addr(c, bus, mode);
    if op & 15 == 14 {
        c.pc = a;
        return;
    }
    let m = c.rd(bus, a);
    c.idle(bus);
    let r = rmw(c, op, m);
    if op & 15 != 13 {
        c.wr(bus, a, r);
    }
}

fn daa(c: &mut M6809) {
    let a = c.a;
    let lo = a & 15;
    let hi = a >> 4;
    let mut fix = 0u8;
    if c.cc & H != 0 || lo > 9 {
        fix |= 0x06;
    }
    if c.cc & C != 0 || hi > 9 || (hi > 8 && lo > 9) {
        fix |= 0x60;
    }
    let r16 = u16::from(a) + u16::from(fix);
    c.a = r16 as u8;
    set_nz8(c, c.a);
    c.cc &= !V;
    if r16 & 0x100 != 0 {
        c.cc |= C;
    }
}

/// The $80–$FF block on A (columns $8–$B) or B ($C–$F).
fn alu(c: &mut M6809, bus: &mut dyn M6809Bus, op: u8, mode: Mode) {
    let b = op >= 0xc0;
    match op & 15 {
        3 => {
            let m = operand16(c, bus, mode);
            let r = if b {
                add16(c, c.d(), m)
            } else {
                sub16(c, c.d(), m)
            };
            c.set_d(r);
        }
        12 if !b => {
            let m = operand16(c, bus, mode);
            sub16(c, c.x, m);
        }
        12 => {
            let v = operand16(c, bus, mode);
            c.set_d(v);
            set_nz16_v0(c, v);
        }
        13 if !b => {
            let a = addr(c, bus, mode);
            c.push_s(bus, 0x80);
            c.pc = a;
        }
        13 => store16(c, bus, mode, c.d()),
        14 => {
            let v = operand16(c, bus, mode);
            if b {
                c.u = v;
            } else {
                c.x = v;
            }
            set_nz16_v0(c, v);
        }
        15 => store16(c, bus, mode, if b { c.u } else { c.x }),
        7 => {
            let a = addr(c, bus, mode);
            let v = if b { c.b } else { c.a };
            set_nz8_v0(c, v);
            c.wr(bus, a, v);
        }
        k => {
            let acc = if b { c.b } else { c.a };
            let m = operand8(c, bus, mode);
            let carry = c.cc & C != 0;
            let r = match k {
                0 | 1 => sub8(c, acc, m, false),
                2 => sub8(c, acc, m, carry),
                4 | 5 => {
                    let r = acc & m;
                    set_nz8_v0(c, r);
                    r
                }
                6 => {
                    set_nz8_v0(c, m);
                    m
                }
                8 => {
                    let r = acc ^ m;
                    set_nz8_v0(c, r);
                    r
                }
                9 => add8(c, acc, m, carry),
                10 => {
                    let r = acc | m;
                    set_nz8_v0(c, r);
                    r
                }
                _ => add8(c, acc, m, false),
            };
            // CMP and BIT only set the flags.
            if k != 1 && k != 5 {
                if b {
                    c.b = r;
                } else {
                    c.a = r;
                }
            }
        }
    }
}
