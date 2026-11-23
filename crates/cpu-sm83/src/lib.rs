//! Sharp SM83, the CPU core of the Game Boy's LR35902 (and of the Game
//! Boy Color's CPU): an 8-bit processor between the 8080 and the Z80, with
//! neither's I/O ports, index registers or second register set.
//!
//! Registers A, F (flags Z N H C in the upper four bits; the lower four
//! read 0), B, C, D, E, H, L, SP, PC. Everything the processor does takes
//! whole machine cycles of four clocks, and every machine cycle is one
//! access to the bus or an internal one: [`Sm83::step`] runs one
//! instruction and reports each machine cycle to the [`MBus`] as it
//! happens — a read, a write or an idle cycle — so a machine advances its
//! other chips by four clocks inside these calls and every access lands on
//! its exact cycle.
//!
//! **Interrupts.** Five sources, their requests (IF) and enables (IE) kept
//! by the machine: [`MBus::pending`] answers IF AND IE. With IME set a
//! pending interrupt is taken before the next instruction in five machine
//! cycles (two internal, the two pushes of PC, the jump to `$40` + 8 × the
//! lowest pending bit); the bit is taken from what is pending after the
//! first push, as the hardware does (an interrupt whose request vanished
//! by then jumps to `$0000`). EI sets IME one instruction late.
//!
//! **HALT** stops until an interrupt is pending, enabled or not. With IME
//! clear and an interrupt already pending HALT does not stop and the byte
//! behind it is fetched twice (the "HALT bug"). **STOP** asks the machine
//! ([`MBus::stop`]): the Game Boy Color switches its speed there; otherwise
//! the processor stops until [`MBus::pending`] reports a joypad interrupt.
//!
//! The eleven opcodes without an instruction (`$D3`, `$DB`, `$DD`, `$E3`,
//! `$E4`, `$EB`, `$EC`, `$ED`, `$F4`, `$FC`, `$FD`) lock the processor up:
//! [`Sm83::locked`].
//!
//! Sources: *Game Boy CPU Manual* (the instruction set and its flags);
//! the Pan Docs (<https://gbdev.io/pandocs/>: CPU instruction set,
//! interrupts, HALT and its bug, STOP); the *Game Boy: Complete Technical
//! Reference* by Joonas Javanainen (the machine cycles of every
//! instruction and of the interrupt dispatch). The implementation is this
//! project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
#[cfg(test)]
mod tests;

use mimic_core::{
    Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};

/// The flags' bits in F.
pub mod flag {
    pub const Z: u8 = 0x80;
    pub const N: u8 = 0x40;
    pub const H: u8 = 0x20;
    pub const C: u8 = 0x10;
}

/// The joypad's interrupt bit in IF and IE, which ends STOP.
pub const INT_JOYPAD: u8 = 0x10;

/// The processor's bus, one call a machine cycle (four clocks).
pub trait MBus {
    /// A machine cycle that reads `addr`.
    fn read(&mut self, addr: u16) -> u8;
    /// A machine cycle that writes `addr`.
    fn write(&mut self, addr: u16, v: u8);
    /// A machine cycle without an access.
    fn idle(&mut self);
    /// The interrupts requested and enabled: IF AND IE, bits 0 to 4.
    fn pending(&self) -> u8;
    /// The interrupt with this bit is being served: clear its request.
    fn acknowledge(&mut self, bit: u8);
    /// STOP was executed. True: the machine switched its speed and the
    /// processor runs on; false: it stops until a joypad interrupt.
    fn stop(&mut self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sm83 {
    pub a: u8,
    /// The flags; the lower four bits are always 0.
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
    /// The interrupt master enable.
    pub ime: bool,
    /// EI ran last: IME is set after the next instruction.
    pub(crate) ime_next: bool,
    pub halted: bool,
    /// The HALT bug: the next fetch does not advance PC.
    pub(crate) halt_bug: bool,
    pub stopped: bool,
    /// An opcode without an instruction was executed.
    pub locked: bool,
    /// Machine cycles of the instruction running.
    pub(crate) m: u32,
    /// [`Cpu::tick`]: clocks left of the instruction it ran.
    left: u32,
}

impl Default for Sm83 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sm83 {
    /// The processor at power-on, before the boot ROM ran.
    pub fn new() -> Self {
        Self {
            a: 0,
            f: 0,
            b: 0,
            c: 0,
            d: 0,
            e: 0,
            h: 0,
            l: 0,
            sp: 0,
            pc: 0,
            ime: false,
            ime_next: false,
            halted: false,
            halt_bug: false,
            stopped: false,
            locked: false,
            m: 0,
            left: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
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

    pub fn set_af(&mut self, v: u16) {
        [self.a, self.f] = v.to_be_bytes();
        self.f &= 0xf0;
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

    /// One instruction, interrupt dispatch or idle cycle of HALT and STOP;
    /// returns its machine cycles.
    pub fn step<B: MBus>(&mut self, bus: &mut B) -> u32 {
        self.m = 0;
        if self.locked {
            self.idle(bus);
            return self.m;
        }
        let pending = bus.pending() & 0x1f;
        if self.stopped {
            if pending & INT_JOYPAD == 0 {
                self.idle(bus);
                return self.m;
            }
            self.stopped = false;
        }
        if self.halted {
            if pending == 0 {
                self.idle(bus);
                return self.m;
            }
            self.halted = false;
        }
        let ime = self.ime;
        // EI's delay: IME counts from the instruction after the next.
        if self.ime_next {
            self.ime = true;
            self.ime_next = false;
        }
        if ime && pending != 0 {
            self.interrupt(bus);
            return self.m;
        }
        let op = self.fetch(bus);
        if self.halt_bug {
            self.halt_bug = false;
            self.pc = self.pc.wrapping_sub(1);
        }
        self.execute(bus, op);
        self.m
    }

    fn interrupt<B: MBus>(&mut self, bus: &mut B) {
        self.ime = false;
        self.idle(bus);
        self.idle(bus);
        let [lo, hi] = self.pc.to_le_bytes();
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, hi);
        // Which interrupt it is is decided here: the push may have
        // written IE.
        let pending = bus.pending() & 0x1f;
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, lo);
        self.pc = if pending == 0 {
            0
        } else {
            let bit = pending.trailing_zeros() as u8;
            bus.acknowledge(1 << bit);
            0x40 + 8 * u16::from(bit)
        };
        self.idle(bus);
    }

    pub(crate) fn read<B: MBus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.m += 1;
        bus.read(addr)
    }

    pub(crate) fn write<B: MBus>(&mut self, bus: &mut B, addr: u16, v: u8) {
        self.m += 1;
        bus.write(addr, v);
    }

    pub(crate) fn idle<B: MBus>(&mut self, bus: &mut B) {
        self.m += 1;
        bus.idle();
    }

    pub(crate) fn fetch<B: MBus>(&mut self, bus: &mut B) -> u8 {
        let v = self.read(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    pub(crate) fn fetch16<B: MBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch(bus);
        let hi = self.fetch(bus);
        u16::from_le_bytes([lo, hi])
    }

    pub(crate) fn push<B: MBus>(&mut self, bus: &mut B, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, hi);
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, lo);
    }

    pub(crate) fn pop<B: MBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        let hi = self.read(bus, self.sp);
        self.sp = self.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }
}

/// [`MBus`] over a plain [`Bus`], for [`Cpu::tick`]: no interrupts.
struct PlainBus<'a>(&'a mut dyn Bus);

impl MBus for PlainBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        self.0.read(u32::from(addr), Size::Byte) as u8
    }
    fn write(&mut self, addr: u16, v: u8) {
        self.0.write(u32::from(addr), Size::Byte, u32::from(v));
    }
    fn idle(&mut self) {}
    fn pending(&self) -> u8 {
        0
    }
    fn acknowledge(&mut self, _bit: u8) {}
}

impl Cpu for Sm83 {
    /// The whole instruction runs on its first clock; the others only
    /// count. A machine uses [`Sm83::step`] with its own [`MBus`].
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.left == 0 {
            self.left = 4 * self.step(&mut PlainBus(bus));
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
            r("F", u32::from(self.f), 8),
            r("B", u32::from(self.b), 8),
            r("C", u32::from(self.c), 8),
            r("D", u32::from(self.d), 8),
            r("E", u32::from(self.e), 8),
            r("H", u32::from(self.h), 8),
            r("L", u32::from(self.l), 8),
            r("IME", u32::from(self.ime), 1),
            r("HALT", u32::from(self.halted), 1),
        ])
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16,
            "SP" => self.sp = v as u16,
            "A" => self.a = v as u8,
            "F" => self.f = v as u8 & 0xf0,
            "B" => self.b = v as u8,
            "C" => self.c = v as u8,
            "D" => self.d = v as u8,
            "E" => self.e = v as u8,
            "H" => self.h = v as u8,
            "L" => self.l = v as u8,
            "IME" => self.ime = v != 0,
            "HALT" => self.halted = v != 0,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

const STATE_TAG: [u8; 4] = *b"SM83";

impl Savestate for Sm83 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [
                self.a, self.f, self.b, self.c, self.d, self.e, self.h, self.l,
            ] {
                w.u8(v);
            }
            w.u16(self.sp);
            w.u16(self.pc);
            for v in [
                self.ime,
                self.ime_next,
                self.halted,
                self.halt_bug,
                self.stopped,
                self.locked,
            ] {
                w.bool(v);
            }
            w.u32(self.left);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.a = c.u8()?;
        n.f = c.u8()? & 0xf0;
        n.b = c.u8()?;
        n.c = c.u8()?;
        n.d = c.u8()?;
        n.e = c.u8()?;
        n.h = c.u8()?;
        n.l = c.u8()?;
        n.sp = c.u16()?;
        n.pc = c.u16()?;
        n.ime = c.bool()?;
        n.ime_next = c.bool()?;
        n.halted = c.bool()?;
        n.halt_bug = c.bool()?;
        n.stopped = c.bool()?;
        n.locked = c.bool()?;
        n.left = c.u32()?;
        *self = n;
        Ok(())
    }
}
