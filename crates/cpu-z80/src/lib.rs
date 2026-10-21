//! Zilog Z80 (C128, CPC).
//!
//! The documented and undocumented instruction set of the NMOS Z80: all
//! prefixes (CB, ED, DD, FD, DD CB, FD CB), the undocumented X/Y flags
//! (bits 3 and 5) including the internal MEMPTR (`WZ`) and Q registers that
//! feed them, the flags of interrupted block instructions, IXH/IXL/IYH/IYL,
//! interrupt modes 0–2, NMI, HALT, the EI delay and the refresh counter R.
//!
//! Sources: Zilog Z80 CPU User Manual (UM0080); Sean Young, "The
//! Undocumented Z80 Documented"; "MEMPTR, esoteric register of the Z80"
//! (boo_boo, Vladimir Kladov); David Banks and Peter Helcmanovsky on the
//! flags of interrupted block instructions. The implementation is this
//! project's own.
//!
//! **Bus.** Memory uses addresses `$0000–$FFFF`; I/O ports appear at
//! [`IO_BASE`]` + port` (the full 16-bit port address), so machines route
//! both through one [`Bus`] and the debugger can watch ports too.
//!
//! **Timing.** [`Cpu::tick`] is one T-state. An instruction executes in its
//! first T-state and then spends its exact T-state count; the bus accesses
//! happen in the right order but all in that first T-state.
//!
//! Machines whose hardware stretches bus cycles (the CPC's Gate Array) use
//! [`Z80::step_timed`] with a [`TimedBus`] instead: every access is an
//! M-cycle placed on its T-state (opcode fetch 4 T-states, memory read and
//! write 3, I/O 4, with the internal T-states between them as on the real
//! chip), and before each access the bus is told on which T-state of the
//! instruction it samples /WAIT, so it can insert wait states and bring the
//! rest of the machine up to that moment first. The placement is checked
//! against the per-T-state bus traces of the Harte tests.
//!
//! **Interrupts.** `set_int` is the level of /INT, `set_nmi` the level of
//! /NMI (edge-triggered). In mode 2 the vector byte comes from
//! [`Z80::set_int_vector`]; in mode 0 the byte on the data bus is executed
//! if it is an RST (others act like RST 38h).

#![forbid(unsafe_code)]

mod disasm;
mod exec;

use emu_core::{
    Bus, Cpu, CycleResult, Instr, InterruptKind, Register, RegisterSet, Savestate, Size,
    StateError, StateReader, StateWriter,
};

/// I/O port `p` is at bus address `IO_BASE + p`.
pub const IO_BASE: u32 = 0x1_0000;

/// The kind of an M-cycle's bus access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Opcode fetch (M1); /WAIT sampled on T2.
    Fetch,
    /// Memory read or write; /WAIT sampled on T2.
    Read,
    Write,
    /// I/O read or write; /WAIT sampled on the automatic wait state TW.
    IoRead,
    IoWrite,
    /// Interrupt acknowledge (M1 with /IORQ, two automatic wait states).
    IntAck,
}

/// A bus that can stretch the Z80's M-cycles ([`Z80::step_timed`]).
pub trait TimedBus: Bus {
    /// The access of the coming M-cycle samples /WAIT on T-state `t` of the
    /// instruction (counted from 0, including earlier wait states). Returns
    /// the wait states to insert there; the access itself follows on
    /// T-state `t + waits`.
    fn wait_states(&mut self, access: Access, t: u32) -> u32;
}

/// A plain [`Bus`] that never waits.
struct NoWait<'a>(&'a mut dyn Bus);

impl Bus for NoWait<'_> {
    fn read(&mut self, addr: u32, size: Size) -> u32 {
        self.0.read(addr, size)
    }

    fn peek(&self, addr: u32, size: Size) -> u32 {
        self.0.peek(addr, size)
    }

    fn write(&mut self, addr: u32, size: Size, v: u32) {
        self.0.write(addr, size, v);
    }

    fn addr_bits(&self) -> u8 {
        self.0.addr_bits()
    }
}

impl TimedBus for NoWait<'_> {
    fn wait_states(&mut self, _: Access, _: u32) -> u32 {
        0
    }
}

pub mod flag {
    pub const C: u8 = 0x01;
    pub const N: u8 = 0x02;
    pub const PV: u8 = 0x04;
    pub const X: u8 = 0x08;
    pub const H: u8 = 0x10;
    pub const Y: u8 = 0x20;
    pub const Z: u8 = 0x40;
    pub const S: u8 = 0x80;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Z80 {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// The alternate set AF', BC', DE', HL'.
    pub af_: u16,
    pub bc_: u16,
    pub de_: u16,
    pub hl_: u16,
    pub ix: u16,
    pub iy: u16,
    pub sp: u16,
    pub pc: u16,
    /// MEMPTR, the internal address latch.
    pub wz: u16,
    pub i: u8,
    pub r: u8,
    pub iff1: bool,
    pub iff2: bool,
    pub im: u8,
    /// Flags written by the last instruction, 0 if it wrote none (SCF/CCF
    /// X/Y behaviour).
    pub q: u8,
    /// The last instruction was LD A,I or LD A,R.
    pub p: bool,
    /// The last instruction was EI: interrupts wait one more instruction.
    pub ei: bool,
    pub halted: bool,
    /// The last instruction was a prefix acting alone (DD/FD before another
    /// prefix): no interrupt may follow.
    prefix_only: bool,
    int_line: bool,
    nmi_line: bool,
    nmi_pending: bool,
    int_vector: u8,
    /// T-states left in the current instruction.
    remaining: u32,
    entered: Option<InterruptKind>,
    /// Flags written during the current instruction.
    flags_written: bool,
    /// T-states of the current instruction so far, and the wait states
    /// among them (only meaningful inside a step).
    t: u32,
    waits: u32,
}

impl Z80 {
    pub fn new() -> Self {
        let mut z = Self::default();
        z.reset();
        z
    }

    /// RESET: PC, I, R, IFFs and IM cleared; AF and SP become $FFFF (the
    /// other registers keep their values).
    pub fn reset(&mut self) {
        self.pc = 0;
        self.i = 0;
        self.r = 0;
        self.iff1 = false;
        self.iff2 = false;
        self.im = 0;
        self.a = 0xff;
        self.f = 0xff;
        self.sp = 0xffff;
        self.halted = false;
        self.ei = false;
        self.p = false;
        self.q = 0;
        self.prefix_only = false;
        self.nmi_pending = false;
        self.remaining = 0;
        self.entered = None;
    }

    /// /INT level (true = asserted).
    pub fn set_int(&mut self, active: bool) {
        self.int_line = active;
    }

    /// /NMI level (true = asserted); the falling edge requests an NMI.
    pub fn set_nmi(&mut self, active: bool) {
        if active && !self.nmi_line {
            self.nmi_pending = true;
        }
        self.nmi_line = active;
    }

    /// The byte a device puts on the data bus when /INT is acknowledged.
    pub fn set_int_vector(&mut self, v: u8) {
        self.int_vector = v;
    }

    /// The interrupt entered by the last instruction boundary, once.
    pub fn take_interrupt(&mut self) -> Option<InterruptKind> {
        self.entered.take()
    }

    /// Between instructions (the next tick starts one).
    pub fn at_boundary(&self) -> bool {
        self.remaining == 0
    }

    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }

    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }

    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }

    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }

    pub fn set_bc(&mut self, v: u16) {
        [self.b, self.c] = v.to_be_bytes();
    }

    pub fn set_de(&mut self, v: u16) {
        [self.d, self.e] = v.to_be_bytes();
    }

    pub fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }

    pub fn set_af(&mut self, v: u16) {
        [self.a, self.f] = v.to_be_bytes();
    }

    /// Run one whole instruction (or interrupt response, or one HALT
    /// T-state group) and return its T-states. [`Cpu::tick`] spreads this
    /// over T-states; tests and fast paths may call it directly.
    pub fn step(&mut self, bus: &mut dyn Bus) -> u32 {
        self.step_timed(&mut NoWait(bus))
    }

    /// [`Z80::step`] on a bus that may insert wait states; the result
    /// includes them.
    pub fn step_timed(&mut self, bus: &mut dyn TimedBus) -> u32 {
        self.t = 0;
        self.waits = 0;
        let t = self.step_inner(bus);
        // Internal T-states after the last access.
        debug_assert!(self.t - self.waits <= t, "accesses beyond the instruction");
        let total = t + self.waits;
        self.t = 0;
        self.waits = 0;
        total
    }

    fn step_inner(&mut self, bus: &mut dyn TimedBus) -> u32 {
        // No interrupt is taken right after EI, and none between a prefix
        // and its instruction.
        let after_prefix = self.prefix_only;
        let block = self.ei || after_prefix;
        self.ei = false;
        self.prefix_only = false;
        if self.nmi_pending && !after_prefix {
            self.nmi_pending = false;
            return self.nmi(bus);
        }
        if self.int_line && self.iff1 && !block {
            return self.irq(bus);
        }
        if self.halted {
            // HALT executes NOPs (refresh continues) until an interrupt.
            self.m_cycle(bus, Access::Fetch, 4, 1);
            self.inc_r();
            self.q = 0;
            self.p = false;
            return 4;
        }
        self.flags_written = false;
        self.p = false;
        let t = self.execute(bus);
        self.q = if self.flags_written { self.f } else { 0 };
        self.flags_written = false;
        t
    }

    fn nmi(&mut self, bus: &mut dyn TimedBus) -> u32 {
        self.halted = false;
        self.iff1 = false;
        // An opcode fetch whose byte is ignored, one T-state longer.
        self.m_cycle(bus, Access::Fetch, 4, 1);
        self.internal(1);
        self.inc_r();
        let pc = self.pc;
        self.push(bus, pc);
        self.pc = 0x66;
        self.wz = 0x66;
        self.q = 0;
        self.p = false;
        self.entered = Some(InterruptKind::Nmi);
        11
    }

    fn irq(&mut self, bus: &mut dyn TimedBus) -> u32 {
        self.halted = false;
        self.iff1 = false;
        self.iff2 = false;
        // Acknowledge: M1 with two automatic wait states (7 T-states), the
        // vector byte read after them.
        self.m_cycle(bus, Access::IntAck, 7, 3);
        self.inc_r();
        self.q = 0;
        self.p = false;
        self.entered = Some(InterruptKind::Irq);
        let pc = self.pc;
        self.push(bus, pc);
        match self.im {
            2 => {
                let table = u16::from_be_bytes([self.i, self.int_vector]);
                let lo = self.read(bus, table);
                let hi = self.read(bus, table.wrapping_add(1));
                self.pc = u16::from_le_bytes([lo, hi]);
                self.wz = self.pc;
                19
            }
            1 => {
                self.pc = 0x38;
                self.wz = 0x38;
                13
            }
            _ => {
                let v = self.int_vector;
                self.pc = if v & 0xc7 == 0xc7 {
                    u16::from(v & 0x38)
                } else {
                    0x38
                };
                self.wz = self.pc;
                13
            }
        }
    }

    pub(crate) fn inc_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7f);
    }

    /// An M-cycle of `len` T-states whose access samples /WAIT on its
    /// T-state `strobe`; the bus may stretch it there.
    pub(crate) fn m_cycle(
        &mut self,
        bus: &mut dyn TimedBus,
        access: Access,
        len: u32,
        strobe: u32,
    ) {
        let w = bus.wait_states(access, self.t + strobe);
        self.waits += w;
        self.t += len + w;
    }

    /// T-states without a bus access between two M-cycles.
    pub(crate) fn internal(&mut self, n: u32) {
        self.t += n;
    }

    pub(crate) fn read(&mut self, bus: &mut dyn TimedBus, addr: u16) -> u8 {
        self.m_cycle(bus, Access::Read, 3, 1);
        bus.read(u32::from(addr), Size::Byte) as u8
    }

    pub(crate) fn write(&mut self, bus: &mut dyn TimedBus, addr: u16, v: u8) {
        self.m_cycle(bus, Access::Write, 3, 1);
        bus.write(u32::from(addr), Size::Byte, u32::from(v));
    }

    pub(crate) fn push(&mut self, bus: &mut dyn TimedBus, v: u16) {
        let [hi, lo] = v.to_be_bytes();
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, hi);
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, lo);
    }

    pub(crate) fn pop(&mut self, bus: &mut dyn TimedBus) -> u16 {
        let lo = self.read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        let hi = self.read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }
}

impl Cpu for Z80 {
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.remaining == 0 {
            self.remaining = self.step(bus);
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            CycleResult::InstrDone {
                next_pc: u32::from(self.pc),
            }
        } else {
            CycleResult::Busy
        }
    }

    fn regs(&self) -> RegisterSet {
        let r8 = |name, v: u8| Register {
            name,
            value: u32::from(v),
            bits: 8,
        };
        let r16 = |name, v: u16| Register {
            name,
            value: u32::from(v),
            bits: 16,
        };
        let r1 = |name, v: bool| Register {
            name,
            value: u32::from(v),
            bits: 1,
        };
        RegisterSet(vec![
            r16("PC", self.pc),
            r16("SP", self.sp),
            r8("A", self.a),
            r8("F", self.f),
            r8("B", self.b),
            r8("C", self.c),
            r8("D", self.d),
            r8("E", self.e),
            r8("H", self.h),
            r8("L", self.l),
            r16("IX", self.ix),
            r16("IY", self.iy),
            r16("AF'", self.af_),
            r16("BC'", self.bc_),
            r16("DE'", self.de_),
            r16("HL'", self.hl_),
            r8("I", self.i),
            r8("R", self.r),
            Register {
                name: "IM",
                value: u32::from(self.im),
                bits: 2,
            },
            r1("IFF1", self.iff1),
            r1("IFF2", self.iff2),
            r16("WZ", self.wz),
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let b = v as u8;
        let w = v as u16;
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = w,
            "SP" => self.sp = w,
            "A" => self.a = b,
            "F" => self.f = b,
            "B" => self.b = b,
            "C" => self.c = b,
            "D" => self.d = b,
            "E" => self.e = b,
            "H" => self.h = b,
            "L" => self.l = b,
            "AF" => self.set_af(w),
            "BC" => self.set_bc(w),
            "DE" => self.set_de(w),
            "HL" => self.set_hl(w),
            "IX" => self.ix = w,
            "IY" => self.iy = w,
            "AF'" => self.af_ = w,
            "BC'" => self.bc_ = w,
            "DE'" => self.de_ = w,
            "HL'" => self.hl_ = w,
            "I" => self.i = b,
            "R" => self.r = b,
            "IM" => self.im = b.min(2),
            "IFF1" => self.iff1 = v != 0,
            "IFF2" => self.iff2 = v != 0,
            "WZ" => self.wz = w,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr as u16)
    }
}

const STATE_TAG: [u8; 4] = *b"Z80C";
const STATE_VERSION: u16 = 1;

impl Savestate for Z80 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, STATE_VERSION, |w| {
            for b in [
                self.a,
                self.f,
                self.b,
                self.c,
                self.d,
                self.e,
                self.h,
                self.l,
                self.i,
                self.r,
                self.im,
                self.q,
                self.int_vector,
            ] {
                w.u8(b);
            }
            for v in [
                self.af_, self.bc_, self.de_, self.hl_, self.ix, self.iy, self.sp, self.pc, self.wz,
            ] {
                w.u16(v);
            }
            for b in [
                self.iff1,
                self.iff2,
                self.p,
                self.ei,
                self.halted,
                self.prefix_only,
                self.int_line,
                self.nmi_line,
                self.nmi_pending,
            ] {
                w.bool(b);
            }
            w.u32(self.remaining);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, STATE_VERSION)?;
        let mut z = Z80::default();
        let mut bytes = [0u8; 13];
        c.raw_into(&mut bytes)?;
        [
            z.a,
            z.f,
            z.b,
            z.c,
            z.d,
            z.e,
            z.h,
            z.l,
            z.i,
            z.r,
            z.im,
            z.q,
            z.int_vector,
        ] = bytes;
        if z.im > 2 {
            return Err(StateError::Invalid("Z80 interrupt mode"));
        }
        for v in [
            &mut z.af_, &mut z.bc_, &mut z.de_, &mut z.hl_, &mut z.ix, &mut z.iy, &mut z.sp,
            &mut z.pc, &mut z.wz,
        ] {
            *v = c.u16()?;
        }
        for b in [
            &mut z.iff1,
            &mut z.iff2,
            &mut z.p,
            &mut z.ei,
            &mut z.halted,
            &mut z.prefix_only,
            &mut z.int_line,
            &mut z.nmi_line,
            &mut z.nmi_pending,
        ] {
            *b = c.bool()?;
        }
        z.remaining = c.u32()?;
        // The longest instruction (or interrupt response) takes 23 T-states.
        if z.remaining > 23 {
            return Err(StateError::Invalid("Z80 T-state counter"));
        }
        *self = z;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
