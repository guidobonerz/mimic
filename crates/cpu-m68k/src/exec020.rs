//! The 68020's additions to the instruction set (M68000 Family
//! Programmer's Reference Manual, section 4 "Integer instructions", and
//! section 6 "Exception processing" for RTE's frame formats). The
//! 68020's timing comes from its own tables (`crate::timing020`); the
//! internal clocks of the 68000 sequences here are ignored on it.

use emu_core::Size;

use crate::ea::{mode, Ea, Sz};
use crate::exec::{control, data_alterable, illegal, mem_alterable, privileged, trap_now};
use crate::{fc, sr, vector, Frame, Res, TimedBus68, M68000};

/// Executes an instruction the 68020 adds (or a 68020 form of a 68000
/// instruction); `None` leaves the opcode to the 68000 decoder.
pub(crate) fn execute(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Option<Res> {
    let r = usize::from(op & 7);
    Some(match op >> 12 {
        0x0 if op & 0x0100 == 0 => match (op >> 9) & 7 {
            0..=2 if op & 0xc0 == 0xc0 => chk2_cmp2(cpu, bus, op),
            // CALLM/RTM: no module support (as on the 68EC020).
            3 if op & 0xc0 == 0xc0 => illegal(cpu, bus),
            5..=7 if op & 0xc0 == 0xc0 => {
                if op & 0x3f == 0x3c && (op >> 9) & 7 != 5 {
                    cas2(cpu, bus, op)
                } else {
                    cas(cpu, bus, op)
                }
            }
            7 => moves(cpu, bus, op),
            _ => return None,
        },
        0x4 => match op {
            0x4e74 => rtd(cpu, bus),
            0x4e7a | 0x4e7b => movec(cpu, bus, op),
            _ => match op & 0xfff8 {
                0x4808 => link_long(cpu, bus, r),
                // BKPT: no breakpoint acknowledge cycle on the Amiga.
                0x4848 => illegal(cpu, bus),
                0x49c0 => {
                    // EXTB.L Dn.
                    let v = cpu.d[r] as u8 as i8 as i32 as u32;
                    cpu.d[r] = v;
                    cpu.logic_flags(Sz::L, v);
                    cpu.prefetch(bus)
                }
                _ => match op & 0xffc0 {
                    // MOVE from SR is privileged from the 68010 on.
                    0x40c0 if !cpu.supervisor() => trap_now(cpu, bus, vector::PRIVILEGE),
                    0x42c0 => move_from_ccr(cpu, bus, op),
                    0x4c00 => mull(cpu, bus, op),
                    0x4c40 => divl(cpu, bus, op),
                    _ => return None,
                },
            },
        },
        0x5 if op & 0xf0f8 == 0x50f8 && (2..=4).contains(&r) => trapcc(cpu, bus, op),
        0x8 if op & 0x01f0 == 0x0140 => pack(cpu, bus, op),
        0x8 if op & 0x01f0 == 0x0180 => unpk(cpu, bus, op),
        0xe if op & 0x08c0 == 0x08c0 => bitfield(cpu, bus, op),
        _ => return None,
    })
}

// ---------------------------------------------------------------- misc

/// RTD #d16: RTS, then the displacement is added to the stack pointer.
fn rtd(cpu: &mut M68000, bus: &mut dyn TimedBus68) -> Res {
    let d = cpu.ext(bus)? as i16 as i32 as u32;
    let pc = cpu.pop_long(bus)?;
    cpu.a[7] = cpu.a[7].wrapping_add(d);
    cpu.jump(bus, pc)?;
    cpu.prefetch(bus)
}

/// LINK.L An,#d32.
fn link_long(cpu: &mut M68000, bus: &mut dyn TimedBus68, r: usize) -> Res {
    let d = cpu.ext_long(bus)?;
    let v = if r == 7 {
        cpu.a[7].wrapping_sub(4)
    } else {
        cpu.a[r]
    };
    cpu.push_long(bus, v)?;
    cpu.a[r] = cpu.a[7];
    cpu.a[7] = cpu.a[7].wrapping_add(d);
    cpu.prefetch(bus)
}

/// MOVE CCR,<ea> (68010 and later): the CCR in the low byte of a word.
fn move_from_ccr(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !data_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ea = cpu.ea(bus, m, r, Sz::W)?.expect("checked");
    let v = u32::from(cpu.sr() & 0x1f);
    match ea {
        Ea::D(dr) => {
            cpu.set_d(dr, Sz::W, v);
            cpu.prefetch(bus)
        }
        Ea::Mem(addr) => {
            cpu.prefetch(bus)?;
            cpu.write_mem(bus, addr, Sz::W, v, false)
        }
        _ => unreachable!("not alterable"),
    }
}

/// MOVEC Rc,Rn / Rn,Rc (privileged). Control registers: SFC, DFC, CACR,
/// USP, VBR, CAAR, MSP, ISP.
fn movec(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    if !privileged(cpu, bus)? {
        return Ok(());
    }
    let ext = cpu.ext(bus)?;
    let reg = usize::from((ext >> 12) & 7);
    let addr_reg = ext & 0x8000 != 0;
    let to_control = op & 1 != 0;
    let code = ext & 0xfff;
    if !matches!(code, 0x000..=0x002 | 0x800..=0x804) {
        return illegal(cpu, bus);
    }
    if to_control {
        let v = if addr_reg { cpu.a[reg] } else { cpu.d[reg] };
        match code {
            0x000 => cpu.sfc = (v & 7) as u8,
            0x001 => cpu.dfc = (v & 7) as u8,
            // E and F are the only bits that read back; C and CE clear
            // the cache or one entry at once.
            0x002 => cpu.write_cacr(v),
            0x800 => cpu.set_usp(v),
            0x801 => cpu.vbr = v,
            0x802 => cpu.caar = v,
            0x803 => cpu.set_msp(v),
            _ => cpu.set_isp(v),
        }
    } else {
        let v = match code {
            0x000 => u32::from(cpu.sfc),
            0x001 => u32::from(cpu.dfc),
            0x002 => cpu.cacr,
            0x800 => cpu.usp(),
            0x801 => cpu.vbr,
            0x802 => cpu.caar,
            0x803 => cpu.msp(),
            _ => cpu.isp(),
        };
        if addr_reg {
            cpu.a[reg] = v;
        } else {
            cpu.d[reg] = v;
        }
    }
    cpu.idle(bus, 6);
    cpu.prefetch(bus)
}

/// MOVES <ea>,Rn / Rn,<ea> (privileged): the access uses SFC/DFC as the
/// function code.
fn moves(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    let Some(sz) = Sz::from_bits(op >> 6) else {
        return illegal(cpu, bus);
    };
    if !mem_alterable(m, r) {
        return illegal(cpu, bus);
    }
    if !privileged(cpu, bus)? {
        return Ok(());
    }
    let ext = cpu.ext(bus)?;
    let reg = usize::from((ext >> 12) & 7);
    let addr_reg = ext & 0x8000 != 0;
    let Ea::Mem(addr) = cpu.ea(bus, m, r, sz)?.expect("checked") else {
        unreachable!("memory mode")
    };
    if ext & 0x0800 != 0 {
        let v = if addr_reg { cpu.a[reg] } else { cpu.d[reg] };
        let fc = cpu.dfc;
        match sz {
            Sz::L => cpu.write_long_at(bus, fc, addr, v)?,
            Sz::W => cpu.write_at(bus, fc, addr, Size::Word, v & 0xffff)?,
            Sz::B => cpu.write_at(bus, fc, addr, Size::Byte, v & 0xff)?,
        }
    } else {
        let fc = cpu.sfc;
        let v = match sz {
            Sz::L => cpu.read_long_at(bus, fc, addr)?,
            Sz::W => cpu.read_at(bus, fc, addr, Size::Word)?,
            Sz::B => cpu.read_at(bus, fc, addr, Size::Byte)?,
        };
        if addr_reg {
            cpu.a[reg] = sz.sext(v);
        } else {
            cpu.set_d(reg, sz, v);
        }
    }
    cpu.prefetch(bus)
}

/// RTE with the 68020's frame formats: 0 (four words), 1 (throwaway: the
/// frame is dropped and RTE starts over on the stack the loaded SR
/// selects), 2 (six words); anything else is a format error.
pub(crate) fn rte(cpu: &mut M68000, bus: &mut dyn TimedBus68) -> Res {
    loop {
        let sp = cpu.a[7];
        let new_sr = cpu.read_at(bus, fc::SUPERVISOR_DATA, sp, Size::Word)? as u16;
        let pc = cpu.read_long_at(bus, fc::SUPERVISOR_DATA, sp.wrapping_add(2))?;
        let format = cpu.read_at(bus, fc::SUPERVISOR_DATA, sp.wrapping_add(6), Size::Word)?;
        match format >> 12 {
            0 => {
                cpu.a[7] = sp.wrapping_add(8);
                cpu.set_sr(new_sr);
            }
            1 => {
                cpu.a[7] = sp.wrapping_add(8);
                cpu.set_sr(new_sr);
                if cpu.supervisor() {
                    continue;
                }
                // A throwaway frame should never return to user mode; stop
                // here rather than reading a user stack as a frame.
                return trap_now(cpu, bus, vector::FORMAT_ERROR);
            }
            2 => {
                cpu.a[7] = sp.wrapping_add(12);
                cpu.set_sr(new_sr);
            }
            _ => return trap_now(cpu, bus, vector::FORMAT_ERROR),
        }
        cpu.jump(bus, pc)?;
        return cpu.prefetch(bus);
    }
}

/// TRAPcc, TRAPcc.W #d16, TRAPcc.L #d32: a TRAPV-like exception (vector
/// 7, format 2) when the condition holds.
fn trapcc(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    match op & 7 {
        2 => {
            cpu.ext(bus)?;
        }
        3 => {
            cpu.ext_long(bus)?;
        }
        _ => {}
    }
    cpu.prefetch(bus)?;
    if !cpu.condition((op >> 8) & 15) {
        return Ok(());
    }
    let pc = cpu.pc();
    cpu.exception(bus, vector::TRAPV, pc, Frame::LowSrHigh)
}

// ---------------------------------------------------------------- arithmetic

/// MULU.L/MULS.L <ea>,Dl (32-bit result, V on overflow) and
/// <ea>,Dh:Dl (64-bit result).
fn mull(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !crate::exec::data_mode(m, r) {
        return illegal(cpu, bus);
    }
    let ext = cpu.ext(bus)?;
    let s = cpu.src(bus, m, r, Sz::L)?;
    let dl = usize::from((ext >> 12) & 7);
    let dh = usize::from(ext & 7);
    let signed = ext & 0x0800 != 0;
    let wide = ext & 0x0400 != 0;
    let product: u64 = if signed {
        ((s as i32 as i64) * (cpu.d[dl] as i32 as i64)) as u64
    } else {
        u64::from(s) * u64::from(cpu.d[dl])
    };
    cpu.set_flag(sr::C, false);
    if wide {
        cpu.d[dh] = (product >> 32) as u32;
        cpu.d[dl] = product as u32;
        cpu.set_flag(sr::V, false);
        cpu.set_flag(sr::N, product & 0x8000_0000_0000_0000 != 0);
        cpu.set_flag(sr::Z, product == 0);
    } else {
        let low = product as u32;
        let overflow = if signed {
            (product as i64) != i64::from(low as i32)
        } else {
            product >> 32 != 0
        };
        cpu.d[dl] = low;
        cpu.set_flag(sr::V, overflow);
        cpu.set_flag(sr::N, low & 0x8000_0000 != 0);
        cpu.set_flag(sr::Z, low == 0);
    }
    cpu.prefetch(bus)?;
    cpu.idle(bus, 40);
    Ok(())
}

/// DIVU.L/DIVS.L <ea>,Dq (32/32), <ea>,Dr:Dq (32-bit remainder and
/// quotient), DIVUL/DIVSL <ea>,Dr:Dq (64/32).
fn divl(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !crate::exec::data_mode(m, r) {
        return illegal(cpu, bus);
    }
    let ext = cpu.ext(bus)?;
    let divisor = cpu.src(bus, m, r, Sz::L)?;
    let dq = usize::from((ext >> 12) & 7);
    let dr = usize::from(ext & 7);
    let signed = ext & 0x0800 != 0;
    let wide = ext & 0x0400 != 0;
    cpu.set_flag(sr::C, false);
    if divisor == 0 {
        cpu.idle(bus, 8);
        let pc = cpu.au().wrapping_sub(2);
        return cpu.exception(bus, vector::ZERO_DIVIDE, pc, Frame::LowSrHigh);
    }
    let (quotient, remainder, overflow) = if signed {
        let dividend: i64 = if wide {
            (i64::from(cpu.d[dr]) << 32) | i64::from(cpu.d[dq])
        } else {
            i64::from(cpu.d[dq] as i32)
        };
        let d = i64::from(divisor as i32);
        let q = dividend.wrapping_div(d);
        let rem = dividend.wrapping_rem(d);
        (q as u32, rem as u32, q != i64::from(q as i32))
    } else {
        let dividend: u64 = if wide {
            (u64::from(cpu.d[dr]) << 32) | u64::from(cpu.d[dq])
        } else {
            u64::from(cpu.d[dq])
        };
        let q = dividend / u64::from(divisor);
        let rem = dividend % u64::from(divisor);
        (q as u32, rem as u32, q >> 32 != 0)
    };
    if overflow {
        // N and Z clear (A1200 run of tools/m68020_random.py).
        cpu.set_flag(sr::V, true);
        cpu.set_flag(sr::N, false);
        cpu.set_flag(sr::Z, false);
    } else {
        cpu.set_flag(sr::V, false);
        cpu.set_flag(sr::N, quotient & 0x8000_0000 != 0);
        cpu.set_flag(sr::Z, quotient == 0);
        // Dr keeps the remainder (unless it is Dq, which takes the
        // quotient; the 32-bit form without a distinct Dr discards it).
        if dr != dq {
            cpu.d[dr] = remainder;
        }
        cpu.d[dq] = quotient;
    }
    cpu.prefetch(bus)?;
    cpu.idle(bus, 80);
    Ok(())
}

/// CHK2/CMP2 <ea>,Rn: Rn against the bounds pair at <ea>; Z if it equals
/// a bound, C if it lies outside; CHK2 traps (vector 6) when outside.
fn chk2_cmp2(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    if !control(m, r) {
        return illegal(cpu, bus);
    }
    let sz = Sz::from_bits(op >> 9).expect("0..=2");
    let ext = cpu.ext(bus)?;
    let reg = usize::from((ext >> 12) & 7);
    let addr_reg = ext & 0x8000 != 0;
    let chk = ext & 0x0800 != 0;
    let Ea::Mem(addr) = cpu.ea(bus, m, r, sz)?.expect("checked") else {
        unreachable!("control mode")
    };
    let lower = cpu.read_mem(bus, addr, sz)?;
    let upper = cpu.read_mem(bus, addr.wrapping_add(sz.bytes()), sz)?;
    // Address registers compare in full with sign-extended bounds; data
    // registers in the operand size.
    let (lower, upper, value) = if addr_reg {
        (sz.sext(lower), sz.sext(upper), cpu.a[reg])
    } else {
        (lower, upper, cpu.d[reg] & sz.mask())
    };
    // Works for signed and unsigned bounds alike: the distance from the
    // lower bound compared with the range's length.
    let outside = value.wrapping_sub(lower) > upper.wrapping_sub(lower);
    cpu.set_flag(sr::Z, value == lower || value == upper);
    cpu.set_flag(sr::C, outside);
    cpu.prefetch(bus)?;
    cpu.idle(bus, 8);
    if chk && outside {
        let pc = cpu.pc();
        return cpu.exception(bus, vector::CHK, pc, Frame::LowSrHigh);
    }
    Ok(())
}

/// CAS Dc,Du,<ea>: compare the operand with Dc; equal: Du is stored,
/// else Dc takes the operand.
fn cas(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    let sz = match (op >> 9) & 3 {
        1 => Sz::B,
        2 => Sz::W,
        _ => Sz::L,
    };
    if !mem_alterable(m, r) {
        return illegal(cpu, bus);
    }
    let ext = cpu.ext(bus)?;
    let du = usize::from((ext >> 6) & 7);
    let dc = usize::from(ext & 7);
    let Ea::Mem(addr) = cpu.ea(bus, m, r, sz)?.expect("checked") else {
        unreachable!("memory mode")
    };
    let operand = cpu.read_mem(bus, addr, sz)?;
    let compare = cpu.d[dc] & sz.mask();
    cpu.cmp(sz, compare, operand);
    if cpu.flag(sr::Z) {
        let v = cpu.d[du] & sz.mask();
        cpu.write_mem(bus, addr, sz, v, false)?;
    } else {
        cpu.set_d(dc, sz, operand);
    }
    cpu.prefetch(bus)
}

/// CAS2 Dc1:Dc2,Du1:Du2,(Rn1):(Rn2).
fn cas2(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let sz = if (op >> 9) & 3 == 2 { Sz::W } else { Sz::L };
    let ext1 = cpu.ext(bus)?;
    let ext2 = cpu.ext(bus)?;
    let reg = |cpu: &M68000, ext: u16| {
        let n = usize::from((ext >> 12) & 7);
        if ext & 0x8000 != 0 {
            cpu.a[n]
        } else {
            cpu.d[n]
        }
    };
    let addr1 = reg(cpu, ext1);
    let addr2 = reg(cpu, ext2);
    let (du1, dc1) = (usize::from((ext1 >> 6) & 7), usize::from(ext1 & 7));
    let (du2, dc2) = (usize::from((ext2 >> 6) & 7), usize::from(ext2 & 7));
    let op1 = cpu.read_mem(bus, addr1, sz)?;
    let op2 = cpu.read_mem(bus, addr2, sz)?;
    cpu.cmp(sz, cpu.d[dc1] & sz.mask(), op1);
    if cpu.flag(sr::Z) {
        cpu.cmp(sz, cpu.d[dc2] & sz.mask(), op2);
    }
    if cpu.flag(sr::Z) {
        let v1 = cpu.d[du1] & sz.mask();
        let v2 = cpu.d[du2] & sz.mask();
        cpu.write_mem(bus, addr1, sz, v1, false)?;
        cpu.write_mem(bus, addr2, sz, v2, false)?;
    } else {
        cpu.set_d(dc1, sz, op1);
        cpu.set_d(dc2, sz, op2);
    }
    cpu.prefetch(bus)
}

/// PACK Dy,Dx,#adj / -(Ay),-(Ax),#adj: two BCD digits of a word (after
/// the adjustment) into one byte.
fn pack(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let x = usize::from((op >> 9) & 7);
    let y = usize::from(op & 7);
    let adj = u32::from(cpu.ext(bus)?);
    if op & 8 == 0 {
        let w = (cpu.d[y] & 0xffff).wrapping_add(adj);
        let b = (w >> 4) & 0xf0 | (w & 0x0f);
        cpu.set_d(x, Sz::B, b);
        return cpu.prefetch(bus);
    }
    // The source word is read as two bytes, low byte first.
    cpu.a[y] = cpu.a[y].wrapping_sub(1);
    let lo = cpu.read(bus, cpu.a[y], Size::Byte)?;
    cpu.a[y] = cpu.a[y].wrapping_sub(1);
    let hi = cpu.read(bus, cpu.a[y], Size::Byte)?;
    let w = (hi << 8 | lo).wrapping_add(adj);
    let b = (w >> 4) & 0xf0 | (w & 0x0f);
    cpu.a[x] = cpu.a[x].wrapping_sub(1);
    cpu.write(bus, cpu.a[x], Size::Byte, b)?;
    cpu.prefetch(bus)
}

/// UNPK Dy,Dx,#adj / -(Ay),-(Ax),#adj: one byte of two BCD digits into a
/// word of two separated digits, plus the adjustment.
fn unpk(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let x = usize::from((op >> 9) & 7);
    let y = usize::from(op & 7);
    let adj = u32::from(cpu.ext(bus)?);
    let spread = |b: u32| ((b & 0xf0) << 4 | (b & 0x0f)).wrapping_add(adj) & 0xffff;
    if op & 8 == 0 {
        let w = spread(cpu.d[y] & 0xff);
        cpu.set_d(x, Sz::W, w);
        return cpu.prefetch(bus);
    }
    cpu.a[y] = cpu.a[y].wrapping_sub(1);
    let b = cpu.read(bus, cpu.a[y], Size::Byte)?;
    let w = spread(b);
    cpu.a[x] = cpu.a[x].wrapping_sub(1);
    cpu.write(bus, cpu.a[x], Size::Byte, w & 0xff)?;
    cpu.a[x] = cpu.a[x].wrapping_sub(1);
    cpu.write(bus, cpu.a[x], Size::Byte, w >> 8)?;
    cpu.prefetch(bus)
}

// ---------------------------------------------------------------- bit fields

/// A bit field's location: in a data register (offset modulo 32, the
/// field wraps) or in memory (a byte address and a bit offset 0–7; up to
/// five bytes).
enum Field {
    Reg { r: usize, offset: u32 },
    Mem { addr: u32, bit: u32, bytes: u32 },
}

/// BFTST, BFEXTU, BFCHG, BFEXTS, BFCLR, BFFFO, BFSET, BFINS (type in
/// bits 10–8). The extension word: bit 11 Do (offset in Dn, bits 8–6),
/// bits 10–6 the offset, bit 5 Dw (width in Dn, bits 2–0), bits 4–0 the
/// width (0 = 32), bits 14–12 the register of BFEXTx/BFFFO/BFINS.
fn bitfield(cpu: &mut M68000, bus: &mut dyn TimedBus68, op: u16) -> Res {
    let (m, r) = mode(op);
    let kind = (op >> 8) & 7;
    let reads_only = matches!(kind, 0 | 1 | 3 | 5);
    let ok = m == 0
        || if reads_only {
            control(m, r)
        } else {
            control(m, r) && !(m == 7 && r >= 2)
        };
    if !ok {
        return illegal(cpu, bus);
    }
    let ext = cpu.ext(bus)?;
    let offset = if ext & 0x0800 != 0 {
        cpu.d[usize::from((ext >> 6) & 7)]
    } else {
        u32::from((ext >> 6) & 31)
    };
    let width = if ext & 0x0020 != 0 {
        cpu.d[usize::from(ext & 7)] & 31
    } else {
        u32::from(ext & 31)
    };
    let width = if width == 0 { 32 } else { width };
    let dn = usize::from((ext >> 12) & 7);
    let field = if m == 0 {
        Field::Reg {
            r,
            offset: offset & 31,
        }
    } else {
        let Ea::Mem(base) = cpu.ea(bus, m, r, Sz::B)?.expect("checked") else {
            unreachable!("control mode")
        };
        let addr = base.wrapping_add((offset as i32 >> 3) as u32);
        let bit = offset & 7;
        Field::Mem {
            addr,
            bit,
            bytes: (bit + width).div_ceil(8),
        }
    };
    // The bytes (or register) holding the field, as a value whose top bit
    // is bit 0 of the first byte.
    let mut data = match field {
        Field::Reg { r, offset } => u64::from(cpu.d[r].rotate_left(offset)) << 32,
        Field::Mem { addr, bytes, .. } => {
            let mut v = 0u64;
            for i in 0..bytes {
                v = v << 8 | u64::from(cpu.read(bus, addr.wrapping_add(i), Size::Byte)?);
            }
            v << (64 - 8 * bytes)
        }
    };
    let skip = match field {
        Field::Reg { .. } => 0,
        Field::Mem { bit, .. } => bit,
    };
    let shift = 64 - skip - width;
    let mask: u64 = if width == 32 {
        0xffff_ffff
    } else {
        (1u64 << width) - 1
    };
    let value = ((data >> shift) & mask) as u32;
    cpu.set_flag(sr::N, value & (1 << (width - 1)) != 0);
    cpu.set_flag(sr::Z, value == 0);
    cpu.set_flag(sr::V, false);
    cpu.set_flag(sr::C, false);
    let new = match kind {
        0 => None,
        1 => {
            cpu.d[dn] = value;
            None
        }
        3 => {
            cpu.d[dn] = if value & (1 << (width - 1)) != 0 && width < 32 {
                value | !((1u32 << width) - 1)
            } else {
                value
            };
            None
        }
        5 => {
            // BFFFO: the offset of the first set bit, or offset + width.
            let first = if value == 0 {
                width
            } else {
                value.leading_zeros() - (32 - width)
            };
            cpu.d[dn] = offset.wrapping_add(first);
            None
        }
        2 => Some(!value & mask as u32),
        4 => Some(0),
        6 => Some(mask as u32),
        _ => {
            let v = cpu.d[dn] & mask as u32;
            cpu.set_flag(sr::N, v & (1 << (width - 1)) != 0);
            cpu.set_flag(sr::Z, v == 0);
            Some(v)
        }
    };
    if let Some(v) = new {
        data = (data & !(mask << shift)) | (u64::from(v) << shift);
        match field {
            Field::Reg { r, offset } => {
                cpu.d[r] = ((data >> 32) as u32).rotate_right(offset);
            }
            Field::Mem { addr, bytes, .. } => {
                for i in 0..bytes {
                    let b = (data >> (56 - 8 * i)) & 0xff;
                    cpu.write(bus, addr.wrapping_add(i), Size::Byte, b as u32)?;
                }
            }
        }
    }
    cpu.prefetch(bus)?;
    cpu.idle(bus, 8);
    Ok(())
}
