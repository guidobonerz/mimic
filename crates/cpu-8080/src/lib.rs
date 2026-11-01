//! Intel 8080A.
//!
//! Registers A, B, C, D, E, H, L, SP, PC and the flags S Z AC P CY, 64 KB of
//! memory and 256 I/O ports. [`I8080::step`] runs one instruction (or
//! interrupt response, or one idle round of HLT) and returns its T-states.
//! Every machine cycle is reported to [`StatusBus::cycle`] with the status
//! word the 8080 puts on its data bus in T1, so a machine can follow the
//! bus cycle by cycle: the Altair's front panel lamps show exactly that.
//!
//! I/O ports appear on the [`Bus`] at [`IO_BASE`] + port, as with the Z80.
//!
//! **Interrupts.** The INT line is a level ([`I8080::set_int`]); it is
//! taken at the end of an instruction while INTE is set, not after EI (one
//! more instruction runs first). The acknowledge cycle fetches the jammed
//! opcode ([`I8080::set_int_opcode`], RST 7 by default: pull-ups on an idle
//! bus) and clears INTE.
//!
//! The opcodes Intel left out are aliases: $08 $10 $18 $20 $28 $30 $38 NOP,
//! $CB JMP, $D9 RET, $DD $ED $FD CALL.
//!
//! Sources: Intel, "8080 Microcomputer Systems User's Manual" (1975): the
//! status word, the instruction timing and flags, the machine cycle
//! sequences; Intel, "8080/8085 Assembly Language Programming Manual"
//! (1977). AC after ANA (the OR of both operands' bit 3) is common 8080
//! knowledge and checked by the CPU exerciser. The implementation is this
//! project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
#[cfg(test)]
mod tests;

use emu_core::{
    Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};

/// I/O port `n` is at `IO_BASE + n` on the bus.
pub const IO_BASE: u32 = 0x1_0000;

/// The status word's bits, as latched from the data bus in T1.
pub mod status {
    /// Interrupt acknowledge.
    pub const INTA: u8 = 0x01;
    /// /WO: high on a read or input, low on a write or output.
    pub const WO: u8 = 0x02;
    /// The address comes from the stack pointer.
    pub const STACK: u8 = 0x04;
    /// Halt acknowledge.
    pub const HLTA: u8 = 0x08;
    /// Output.
    pub const OUT: u8 = 0x10;
    /// Opcode fetch.
    pub const M1: u8 = 0x20;
    /// Input.
    pub const INP: u8 = 0x40;
    /// Memory read.
    pub const MEMR: u8 = 0x80;

    pub const FETCH: u8 = M1 | MEMR | WO;
    pub const READ: u8 = MEMR | WO;
    pub const WRITE: u8 = 0;
    pub const STACK_READ: u8 = STACK | MEMR | WO;
    pub const STACK_WRITE: u8 = STACK;
    pub const INPUT: u8 = INP | WO;
    pub const OUTPUT: u8 = OUT;
    pub const HALT: u8 = HLTA | MEMR | WO;
    pub const INT_ACK: u8 = INTA | M1 | WO;
}

/// The 8080's bus with its machine cycles.
pub trait StatusBus: Bus {
    /// A machine cycle of `t` T-states ran: `status`, the address and the
    /// byte on the data bus. Internal T-states repeat the last cycle's
    /// values.
    fn cycle(&mut self, status: u8, addr: u16, data: u8, t: u32);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct I8080 {
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    pub s: bool,
    pub z: bool,
    pub ac: bool,
    pub p: bool,
    pub cy: bool,
    /// The interrupt enable flip-flop.
    pub inte: bool,
    /// EI ran last: the next instruction is not interrupted.
    pub(crate) ei_delay: bool,
    pub halted: bool,
    int: bool,
    int_opcode: u8,
    /// T-states of the instruction running.
    pub(crate) t: u32,
    /// The last machine cycle: status, address, data.
    pub(crate) last: (u8, u16, u8),
    /// [`Cpu::tick`]: T-states left of the instruction it ran.
    left: u32,
}

impl Default for I8080 {
    fn default() -> Self {
        Self::new()
    }
}

impl I8080 {
    pub fn new() -> Self {
        Self {
            a: 0,
            b: 0,
            c: 0,
            d: 0,
            e: 0,
            h: 0,
            l: 0,
            sp: 0,
            pc: 0,
            s: false,
            z: false,
            ac: false,
            p: false,
            cy: false,
            inte: false,
            ei_delay: false,
            halted: false,
            int: false,
            int_opcode: 0xff,
            t: 0,
            last: (status::FETCH, 0, 0),
            left: 0,
        }
    }

    /// /RESET: PC 0, interrupts off, out of HLT. The other registers keep
    /// their values.
    pub fn reset(&mut self) {
        self.pc = 0;
        self.inte = false;
        self.ei_delay = false;
        self.halted = false;
        self.left = 0;
    }

    /// The INT line.
    pub fn set_int(&mut self, level: bool) {
        self.int = level;
    }

    /// The opcode a device puts on the bus in the acknowledge cycle.
    pub fn set_int_opcode(&mut self, op: u8) {
        self.int_opcode = op;
    }

    /// The flags as PUSH PSW stores them: S Z 0 AC 0 P 1 CY.
    pub fn psw(&self) -> u8 {
        u8::from(self.s) << 7
            | u8::from(self.z) << 6
            | u8::from(self.ac) << 4
            | u8::from(self.p) << 2
            | 0x02
            | u8::from(self.cy)
    }

    pub fn set_psw(&mut self, v: u8) {
        self.s = v & 0x80 != 0;
        self.z = v & 0x40 != 0;
        self.ac = v & 0x10 != 0;
        self.p = v & 0x04 != 0;
        self.cy = v & 0x01 != 0;
    }

    /// One instruction, interrupt response or idle round of HLT; returns
    /// its T-states.
    pub fn step<B: StatusBus>(&mut self, bus: &mut B) -> u32 {
        self.t = 0;
        let take = self.int && self.inte && !self.ei_delay;
        self.ei_delay = false;
        if take {
            self.inte = false;
            self.halted = false;
            let op = self.int_opcode;
            bus.cycle(status::INT_ACK, self.pc, op, 4);
            self.last = (status::INT_ACK, self.pc, op);
            self.t += 4;
            self.execute(bus, op);
        } else if self.halted {
            bus.cycle(status::HALT, self.pc, self.last.2, 4);
            self.t += 4;
        } else {
            let op = self.fetch(bus);
            self.execute(bus, op);
        }
        self.t
    }

    fn read_cycle<B: StatusBus>(&mut self, bus: &mut B, st: u8, addr: u32, t: u32) -> u8 {
        let v = bus.read(addr, Size::Byte) as u8;
        bus.cycle(st, addr as u16, v, t);
        self.last = (st, addr as u16, v);
        self.t += t;
        v
    }

    fn write_cycle<B: StatusBus>(&mut self, bus: &mut B, st: u8, addr: u32, v: u8) {
        bus.write(addr, Size::Byte, u32::from(v));
        bus.cycle(st, addr as u16, v, 3);
        self.last = (st, addr as u16, v);
        self.t += 3;
    }

    /// T-states inside a machine cycle, without bus activity.
    pub(crate) fn idle<B: StatusBus>(&mut self, bus: &mut B, t: u32) {
        let (st, addr, data) = self.last;
        bus.cycle(st, addr, data, t);
        self.t += t;
    }

    fn fetch<B: StatusBus>(&mut self, bus: &mut B) -> u8 {
        let v = self.read_cycle(bus, status::FETCH, u32::from(self.pc), 4);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    pub(crate) fn operand<B: StatusBus>(&mut self, bus: &mut B) -> u8 {
        let v = self.read_cycle(bus, status::READ, u32::from(self.pc), 3);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    pub(crate) fn operand16<B: StatusBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.operand(bus);
        let hi = self.operand(bus);
        u16::from_le_bytes([lo, hi])
    }

    pub(crate) fn mem_read<B: StatusBus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.read_cycle(bus, status::READ, u32::from(addr), 3)
    }

    pub(crate) fn mem_write<B: StatusBus>(&mut self, bus: &mut B, addr: u16, v: u8) {
        self.write_cycle(bus, status::WRITE, u32::from(addr), v);
    }

    pub(crate) fn io_read<B: StatusBus>(&mut self, bus: &mut B, port: u8) -> u8 {
        self.read_cycle(bus, status::INPUT, IO_BASE + u32::from(port), 3)
    }

    pub(crate) fn io_write<B: StatusBus>(&mut self, bus: &mut B, port: u8, v: u8) {
        self.write_cycle(bus, status::OUTPUT, IO_BASE + u32::from(port), v);
    }

    pub(crate) fn stack_read<B: StatusBus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.read_cycle(bus, status::STACK_READ, u32::from(addr), 3)
    }

    pub(crate) fn stack_write<B: StatusBus>(&mut self, bus: &mut B, addr: u16, v: u8) {
        self.write_cycle(bus, status::STACK_WRITE, u32::from(addr), v);
    }

    pub(crate) fn push<B: StatusBus>(&mut self, bus: &mut B, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.sp = self.sp.wrapping_sub(1);
        self.stack_write(bus, self.sp, hi);
        self.sp = self.sp.wrapping_sub(1);
        self.stack_write(bus, self.sp, lo);
    }

    pub(crate) fn pop<B: StatusBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.stack_read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        let hi = self.stack_read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }
}

/// [`StatusBus`] over a plain [`Bus`], for [`Cpu::tick`].
struct PlainBus<'a>(&'a mut dyn Bus);

impl Bus for PlainBus<'_> {
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

impl StatusBus for PlainBus<'_> {
    fn cycle(&mut self, _status: u8, _addr: u16, _data: u8, _t: u32) {}
}

impl Cpu for I8080 {
    /// The whole instruction runs on its first cycle; the others only count.
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.left == 0 {
            self.left = self.step(&mut PlainBus(bus));
        }
        self.left -= 1;
        if self.left == 0 {
            CycleResult::InstrDone {
                next_pc: u32::from(self.pc),
            }
        } else {
            CycleResult::Busy
        }
    }

    fn regs(&self) -> RegisterSet {
        let r = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            r("PC", u32::from(self.pc), 16),
            r("SP", u32::from(self.sp), 16),
            r("A", u32::from(self.a), 8),
            r("F", u32::from(self.psw()), 8),
            r("B", u32::from(self.b), 8),
            r("C", u32::from(self.c), 8),
            r("D", u32::from(self.d), 8),
            r("E", u32::from(self.e), 8),
            r("H", u32::from(self.h), 8),
            r("L", u32::from(self.l), 8),
            r("INTE", u32::from(self.inte), 1),
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16,
            "SP" => self.sp = v as u16,
            "A" => self.a = v as u8,
            "F" => self.set_psw(v as u8),
            "B" => self.b = v as u8,
            "C" => self.c = v as u8,
            "D" => self.d = v as u8,
            "E" => self.e = v as u8,
            "H" => self.h = v as u8,
            "L" => self.l = v as u8,
            "INTE" => self.inte = v != 0,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

const STATE_TAG: [u8; 4] = *b"8080";

impl Savestate for I8080 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [self.a, self.b, self.c, self.d, self.e, self.h, self.l] {
                w.u8(v);
            }
            w.u8(self.psw());
            w.u16(self.sp);
            w.u16(self.pc);
            for v in [self.inte, self.ei_delay, self.halted, self.int] {
                w.bool(v);
            }
            w.u8(self.int_opcode);
            w.u8(self.last.0);
            w.u16(self.last.1);
            w.u8(self.last.2);
            w.u32(self.left);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.a = c.u8()?;
        n.b = c.u8()?;
        n.c = c.u8()?;
        n.d = c.u8()?;
        n.e = c.u8()?;
        n.h = c.u8()?;
        n.l = c.u8()?;
        n.set_psw(c.u8()?);
        n.sp = c.u16()?;
        n.pc = c.u16()?;
        n.inte = c.bool()?;
        n.ei_delay = c.bool()?;
        n.halted = c.bool()?;
        n.int = c.bool()?;
        n.int_opcode = c.u8()?;
        n.last = (c.u8()?, c.u16()?, c.u8()?);
        n.left = c.u32()?;
        *self = n;
        Ok(())
    }
}
