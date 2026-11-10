//! Texas Instruments TMS9900, the 16-bit CPU of the TI-99/4A.
//!
//! The registers are PC, WP (workspace pointer) and ST (status); the 16
//! general registers R0–R15 are the words at WP, WP+2 … WP+30 in memory,
//! and every register access is a memory access. Memory is addressed in
//! bytes, read and written in big-endian words at even addresses (a byte
//! operation reads the word and, when it writes, writes the whole word
//! back: the 9900 always reads its destination first). I/O goes over the
//! CRU, a serial bit bus of 4096 bit addresses ([`Tms9900Bus::cru_read`],
//! [`Tms9900Bus::cru_write`]).
//!
//! **Status register** (bit 0 is the most significant): L> A> EQ C OV OP X,
//! bits 12–15 the interrupt mask ([`st`]).
//!
//! **Interrupts.** INTREQ with a level 1–15 ([`Tms9900::set_intreq`]) is
//! taken at the end of an instruction when the level is at most the mask:
//! a context switch through the vector at 4 × level, the mask becomes
//! level − 1. LOAD ([`Tms9900::load_interrupt`]) switches through `>FFFC`, RESET
//! ([`Tms9900::reset`]) through `>0000` with ST cleared. No interrupt is
//! taken after BLWP, XOP or a context switch: the first instruction of the
//! new routine always runs.
//!
//! **Timing.** [`Tms9900::step`] returns the clock cycles of an instruction
//! as the datasheet lists them (Table 3, "instruction execution times",
//! with the address modification times of the operands), plus the wait
//! states the bus adds to each memory access
//! ([`Tms9900Bus::wait_states`]). DIV takes a fixed 108 cycles without
//! overflow (the datasheet gives 92–124 depending on the data). Opcodes
//! the 9900 does not define run as 6-cycle no-operations.
//!
//! Sources: Texas Instruments, *TMS 9900 Microprocessor Data Manual*
//! (1976): instruction set, formats, status bits, the CRU, interrupts and
//! execution times; Texas Instruments, *9900 Family Systems Design and
//! Data Book* (1978), the external instructions. The implementation is
//! this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
#[cfg(test)]
mod tests;

use emu_core::{
    Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};

/// Status register bits.
pub mod st {
    /// Logical greater than.
    pub const LGT: u16 = 0x8000;
    /// Arithmetic greater than.
    pub const AGT: u16 = 0x4000;
    pub const EQ: u16 = 0x2000;
    pub const C: u16 = 0x1000;
    pub const OV: u16 = 0x0800;
    /// Odd parity (byte operations).
    pub const OP: u16 = 0x0400;
    /// An XOP is running.
    pub const X: u16 = 0x0200;
    /// The interrupt mask.
    pub const MASK: u16 = 0x000f;
}

/// The external instructions, signalled on A0–A2 with CRUCLK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum External {
    Idle,
    Rset,
    Ckon,
    Ckof,
    Lrex,
}

/// The 9900's bus: memory through [`Bus`] (big-endian words at even
/// addresses, [`Size::Word`]), the CRU and the external instructions.
pub trait Tms9900Bus: Bus {
    /// Extra clock cycles a memory access at `addr` takes.
    fn wait_states(&mut self, _addr: u16) -> u32 {
        0
    }
    /// The CRU input bit at `bit` (0–4095).
    fn cru_read(&mut self, bit: u16) -> bool;
    /// Set the CRU output bit at `bit`.
    fn cru_write(&mut self, bit: u16, value: bool);
    /// An external instruction ran (IDLE also every idle round).
    fn external(&mut self, _op: External) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tms9900 {
    pub pc: u16,
    pub wp: u16,
    pub st: u16,
    /// Waiting for an interrupt after IDLE.
    pub idle: bool,
    /// INTREQ and its level.
    intreq: Option<u8>,
    /// A LOAD (non-maskable) is pending.
    load: bool,
    /// A RESET is pending.
    reset: bool,
    /// No interrupt after this instruction (BLWP, XOP, a context switch).
    no_int: bool,
    /// Clock cycles of the instruction running.
    pub(crate) cycles: u32,
    /// [`Cpu::tick`]: cycles left of the instruction it ran.
    left: u32,
}

impl Default for Tms9900 {
    fn default() -> Self {
        Self::new()
    }
}

impl Tms9900 {
    /// A 9900 that starts with a RESET on its first step.
    pub fn new() -> Self {
        Self {
            pc: 0,
            wp: 0,
            st: 0,
            idle: false,
            intreq: None,
            load: false,
            reset: true,
            no_int: false,
            cycles: 0,
            left: 0,
        }
    }

    /// The RESET line: the next step switches context through `>0000`.
    pub fn reset(&mut self) {
        self.reset = true;
        self.left = 0;
    }

    /// The LOAD line (an edge): the next step switches through `>FFFC`.
    pub fn load_interrupt(&mut self) {
        self.load = true;
    }

    /// INTREQ with its level (`None`: no request). A level, as the IC0–IC3
    /// lines carry it, 1–15.
    pub fn set_intreq(&mut self, level: Option<u8>) {
        self.intreq = level.map(|l| l.clamp(1, 15));
    }

    /// One instruction, context switch or idle round; returns its clock
    /// cycles.
    pub fn step<B: Tms9900Bus>(&mut self, bus: &mut B) -> u32 {
        self.cycles = 0;
        if self.reset {
            self.reset = false;
            self.load = false;
            self.idle = false;
            self.st = 0;
            self.context_switch(bus, 0x0000);
            self.cycles += 26;
            self.no_int = true;
            return self.cycles;
        }
        if self.load {
            self.load = false;
            self.idle = false;
            self.context_switch(bus, 0xfffc);
            self.cycles += 22;
            self.no_int = true;
            return self.cycles;
        }
        let blocked = std::mem::take(&mut self.no_int);
        if let Some(level) = self.intreq.filter(|_| !blocked) {
            if u16::from(level) <= self.st & st::MASK {
                self.idle = false;
                self.context_switch(bus, u16::from(level) * 4);
                self.st = (self.st & !st::MASK) | (u16::from(level) - 1);
                self.cycles += 22;
                self.no_int = true;
                return self.cycles;
            }
        }
        if self.idle {
            bus.external(External::Idle);
            self.cycles += 2;
            return self.cycles;
        }
        let op = self.fetch(bus);
        self.execute(bus, op);
        self.cycles
    }

    /// New WP and PC from the vector at `vector`; the old WP, PC and ST go
    /// to the new R13, R14 and R15.
    pub(crate) fn context_switch<B: Tms9900Bus>(&mut self, bus: &mut B, vector: u16) {
        let wp = self.read(bus, vector);
        let pc = self.read(bus, vector.wrapping_add(2));
        self.switch_to(bus, wp, pc);
    }

    pub(crate) fn switch_to<B: Tms9900Bus>(&mut self, bus: &mut B, wp: u16, pc: u16) {
        let (old_wp, old_pc, old_st) = (self.wp, self.pc, self.st);
        self.wp = wp;
        self.pc = pc;
        self.write(bus, self.reg_addr(13), old_wp);
        self.write(bus, self.reg_addr(14), old_pc);
        self.write(bus, self.reg_addr(15), old_st);
    }

    /// The address of register `r` (0–15; 16 is the word after R15, as
    /// MPY and DIV reach it with R15).
    pub(crate) fn reg_addr(&self, r: u16) -> u16 {
        self.wp.wrapping_add(r * 2)
    }

    /// A memory word read (the address made even), with its wait states.
    pub(crate) fn read<B: Tms9900Bus>(&mut self, bus: &mut B, addr: u16) -> u16 {
        let a = addr & 0xfffe;
        self.cycles += bus.wait_states(a);
        bus.read(u32::from(a), Size::Word) as u16
    }

    pub(crate) fn write<B: Tms9900Bus>(&mut self, bus: &mut B, addr: u16, v: u16) {
        let a = addr & 0xfffe;
        self.cycles += bus.wait_states(a);
        bus.write(u32::from(a), Size::Word, u32::from(v));
    }

    pub(crate) fn fetch<B: Tms9900Bus>(&mut self, bus: &mut B) -> u16 {
        let v = self.read(bus, self.pc);
        self.pc = self.pc.wrapping_add(2);
        v
    }

    /// Register `r` of the current workspace, read through memory.
    pub fn register(&self, bus: &dyn Bus, r: u16) -> u16 {
        bus.peek(u32::from(self.reg_addr(r & 15)), Size::Word) as u16
    }
}

/// [`Tms9900Bus`] over a plain [`Bus`], for [`Cpu::tick`]: no CRU.
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

impl Tms9900Bus for PlainBus<'_> {
    fn cru_read(&mut self, _bit: u16) -> bool {
        true
    }
    fn cru_write(&mut self, _bit: u16, _value: bool) {}
}

impl Cpu for Tms9900 {
    /// The whole instruction runs on its first cycle; the others only count.
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.left == 0 {
            self.left = self.step(&mut PlainBus(bus)).max(1);
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
        let r = |name, value: u16| Register {
            name,
            value: u32::from(value),
            bits: 16,
        };
        RegisterSet(vec![r("PC", self.pc), r("WP", self.wp), r("ST", self.st)])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16 & 0xfffe,
            "WP" => self.wp = v as u16 & 0xfffe,
            "ST" => self.st = v as u16,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

const STATE_TAG: [u8; 4] = *b"9900";

impl Savestate for Tms9900 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.u16(self.pc);
            w.u16(self.wp);
            w.u16(self.st);
            for b in [self.idle, self.load, self.reset, self.no_int] {
                w.bool(b);
            }
            w.u8(self.intreq.unwrap_or(0));
            w.u32(self.left);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.pc = c.u16()?;
        n.wp = c.u16()?;
        n.st = c.u16()?;
        n.idle = c.bool()?;
        n.load = c.bool()?;
        n.reset = c.bool()?;
        n.no_int = c.bool()?;
        n.intreq = match c.u8()? {
            0 => None,
            l @ 1..=15 => Some(l),
            _ => return Err(StateError::Invalid("tms9900 interrupt level")),
        };
        n.left = c.u32()?;
        *self = n;
        Ok(())
    }
}
