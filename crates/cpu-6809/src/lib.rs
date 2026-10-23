//! Motorola 6809 (MC6809 / MC68A09): the CPU of the Vectrex.
//!
//! [`M6809::step`] runs one instruction (or one interrupt entry, or one
//! cycle of waiting in CWAI/SYNC) and reports every bus cycle to the
//! [`M6809Bus`]: reads, writes and the dead cycles in which the chip puts
//! $FFFF on the bus without accessing memory ([`M6809Bus::idle`]). Every
//! instruction takes the cycle count of the datasheet tables; dead cycles
//! sit where the datasheet's cycle-by-cycle descriptions put them for the
//! common addressing modes (one after the address, one between the read and
//! the write of a read-modify-write) and at the end otherwise.
//!
//! Registers A, B (D = A:B), X, Y, U, S, PC, DP and CC (E F H I N Z V C).
//! Interrupts: NMI (edge, armed once S has been loaded), FIRQ and IRQ
//! (levels), SWI/SWI2/SWI3, CWAI and SYNC. The undocumented opcodes known
//! to alias documented ones ($01 = NEG, $05 = LSR, $0B = DEC, $4E = CLRA …)
//! do so; $14/$15/$CD halt the CPU (HCF) until reset; other undefined
//! opcodes do nothing.
//!
//! Sources: Motorola, "MC6809 8-Bit Microprocessor" datasheet (instruction
//! and indexed-mode cycle tables, interrupt sequences); Motorola, "MC6809-
//! MC6809E Microprocessor Programming Manual" (M6809PM/AD, instruction
//! descriptions and condition codes). The implementation is this project's
//! own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
mod table;

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{Register, RegisterSet};

pub use disasm::disassemble;

pub const C: u8 = 0x01;
pub const V: u8 = 0x02;
pub const Z: u8 = 0x04;
pub const N: u8 = 0x08;
pub const I: u8 = 0x10;
pub const H: u8 = 0x20;
pub const F: u8 = 0x40;
pub const E: u8 = 0x80;

/// Interrupt vectors.
pub const VEC_SWI3: u16 = 0xfff2;
pub const VEC_SWI2: u16 = 0xfff4;
pub const VEC_FIRQ: u16 = 0xfff6;
pub const VEC_IRQ: u16 = 0xfff8;
pub const VEC_SWI: u16 = 0xfffa;
pub const VEC_NMI: u16 = 0xfffc;
pub const VEC_RESET: u16 = 0xfffe;

/// What a 6809 is connected to. Each call is one bus (E) cycle.
pub trait M6809Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, v: u8);
    /// A dead cycle (address $FFFF, no access).
    fn idle(&mut self);
    /// Side-effect-free read (disassembler, debugger).
    fn peek(&self, addr: u16) -> u8;
}

/// Why the CPU is not executing instructions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wait {
    #[default]
    None,
    /// CWAI: state is stacked, waiting for an unmasked interrupt.
    Cwai,
    /// SYNC: waiting for any interrupt line, masked or not.
    Sync,
    /// HCF (undefined $14/$15/$CD): only a reset helps.
    Halt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct M6809 {
    pub a: u8,
    pub b: u8,
    pub dp: u8,
    pub cc: u8,
    pub x: u16,
    pub y: u16,
    pub u: u16,
    pub s: u16,
    pub pc: u16,
    pub wait: Wait,
    /// NMI is ignored until the first load of S after reset.
    nmi_armed: bool,
    nmi_line: bool,
    nmi_pending: bool,
    /// FIRQ and IRQ input levels (true = asserted).
    pub firq: bool,
    pub irq: bool,
    cycles: u32,
    /// Cycle count the current instruction pads up to.
    target: u32,
}

impl Default for M6809 {
    fn default() -> Self {
        Self::new()
    }
}

impl M6809 {
    pub fn new() -> Self {
        Self {
            a: 0,
            b: 0,
            dp: 0,
            cc: I | F,
            x: 0,
            y: 0,
            u: 0,
            s: 0,
            pc: 0,
            wait: Wait::None,
            nmi_armed: false,
            nmi_line: false,
            nmi_pending: false,
            firq: false,
            irq: false,
            cycles: 0,
            target: 0,
        }
    }

    /// Reset: DP = 0, FIRQ and IRQ masked, NMI disarmed, PC from $FFFE.
    pub fn reset(&mut self, bus: &mut dyn M6809Bus) {
        self.dp = 0;
        self.cc |= I | F;
        self.wait = Wait::None;
        self.nmi_armed = false;
        self.nmi_pending = false;
        self.pc = u16::from_be_bytes([bus.read(VEC_RESET), bus.read(VEC_RESET + 1)]);
    }

    /// The NMI input (true = asserted); a new assertion is latched.
    pub fn set_nmi(&mut self, on: bool) {
        if on && !self.nmi_line {
            self.nmi_pending = true;
        }
        self.nmi_line = on;
    }

    pub fn d(&self) -> u16 {
        u16::from_be_bytes([self.a, self.b])
    }

    pub fn set_d(&mut self, v: u16) {
        [self.a, self.b] = v.to_be_bytes();
    }

    pub(crate) fn set_s(&mut self, v: u16) {
        self.s = v;
        self.nmi_armed = true;
    }

    /// Run one instruction or interrupt entry; returns its bus cycles.
    pub fn step(&mut self, bus: &mut dyn M6809Bus) -> u32 {
        self.cycles = 0;
        self.target = 0;
        let nmi = self.nmi_pending && self.nmi_armed;
        let firq = self.firq && self.cc & F == 0;
        let irq = self.irq && self.cc & I == 0;
        match self.wait {
            Wait::Halt => self.idle(bus),
            Wait::Sync => {
                if nmi || self.firq || self.irq {
                    self.wait = Wait::None;
                    if nmi || firq || irq {
                        self.interrupt(bus, nmi, firq);
                    } else {
                        // A masked interrupt ends SYNC; execution goes on.
                        self.idle(bus);
                        self.idle(bus);
                    }
                } else {
                    self.idle(bus);
                }
            }
            Wait::Cwai => {
                if nmi || firq || irq {
                    self.interrupt(bus, nmi, firq);
                } else {
                    self.idle(bus);
                }
            }
            Wait::None => {
                if nmi || firq || irq {
                    self.interrupt(bus, nmi, firq);
                } else {
                    exec::execute(self, bus);
                }
            }
        }
        while self.cycles < self.target {
            self.idle(bus);
        }
        self.cycles
    }

    /// Hardware interrupt entry (NMI before FIRQ before IRQ). After CWAI
    /// the state is already on the stack.
    fn interrupt(&mut self, bus: &mut dyn M6809Bus, nmi: bool, firq: bool) {
        let stacked = self.wait == Wait::Cwai;
        self.wait = Wait::None;
        let vector = if nmi {
            self.nmi_pending = false;
            VEC_NMI
        } else if firq {
            VEC_FIRQ
        } else {
            VEC_IRQ
        };
        if stacked {
            self.target = 3;
            self.idle(bus);
        } else if vector == VEC_FIRQ {
            self.target = 10;
            self.idle(bus);
            self.idle(bus);
            self.idle(bus);
            self.cc &= !E;
            self.push_s(bus, 0x81);
            self.idle(bus);
        } else {
            self.target = 19;
            self.idle(bus);
            self.idle(bus);
            self.idle(bus);
            self.cc |= E;
            self.push_s(bus, 0xff);
            self.idle(bus);
        }
        self.cc |= if vector == VEC_IRQ { I } else { I | F };
        self.pc = self.rd16(bus, vector);
    }

    pub(crate) fn rd(&mut self, bus: &mut dyn M6809Bus, addr: u16) -> u8 {
        self.cycles += 1;
        bus.read(addr)
    }

    pub(crate) fn rd16(&mut self, bus: &mut dyn M6809Bus, addr: u16) -> u16 {
        let hi = self.rd(bus, addr);
        let lo = self.rd(bus, addr.wrapping_add(1));
        u16::from_be_bytes([hi, lo])
    }

    pub(crate) fn wr(&mut self, bus: &mut dyn M6809Bus, addr: u16, v: u8) {
        self.cycles += 1;
        bus.write(addr, v);
    }

    pub(crate) fn wr16(&mut self, bus: &mut dyn M6809Bus, addr: u16, v: u16) {
        let [hi, lo] = v.to_be_bytes();
        self.wr(bus, addr, hi);
        self.wr(bus, addr.wrapping_add(1), lo);
    }

    pub(crate) fn idle(&mut self, bus: &mut dyn M6809Bus) {
        self.cycles += 1;
        bus.idle();
    }

    pub(crate) fn fetch(&mut self, bus: &mut dyn M6809Bus) -> u8 {
        let pc = self.pc;
        self.pc = pc.wrapping_add(1);
        self.rd(bus, pc)
    }

    pub(crate) fn fetch16(&mut self, bus: &mut dyn M6809Bus) -> u16 {
        let hi = self.fetch(bus);
        let lo = self.fetch(bus);
        u16::from_be_bytes([hi, lo])
    }

    /// Push the registers selected by a PSHS postbyte onto S (bit 6 = U).
    pub(crate) fn push_s(&mut self, bus: &mut dyn M6809Bus, mask: u8) {
        let mut sp = self.s;
        let other = self.u;
        self.push(bus, &mut sp, other, mask);
        self.s = sp;
    }

    /// Push onto the stack `sp` (`other` is the other stack pointer, pushed
    /// for bit 6). Order: PC, U/S, Y, X, DP, B, A, CC.
    pub(crate) fn push(&mut self, bus: &mut dyn M6809Bus, sp: &mut u16, other: u16, mask: u8) {
        let words = [
            (0x80, self.pc),
            (0x40, other),
            (0x20, self.y),
            (0x10, self.x),
        ];
        for (bit, v) in words {
            if mask & bit != 0 {
                let [hi, lo] = v.to_be_bytes();
                *sp = sp.wrapping_sub(1);
                self.wr(bus, *sp, lo);
                *sp = sp.wrapping_sub(1);
                self.wr(bus, *sp, hi);
            }
        }
        let bytes = [
            (0x08, self.dp),
            (0x04, self.b),
            (0x02, self.a),
            (0x01, self.cc),
        ];
        for (bit, v) in bytes {
            if mask & bit != 0 {
                *sp = sp.wrapping_sub(1);
                self.wr(bus, *sp, v);
            }
        }
    }

    fn pull8(&mut self, bus: &mut dyn M6809Bus, sp: &mut u16) -> u8 {
        let v = self.rd(bus, *sp);
        *sp = sp.wrapping_add(1);
        v
    }

    fn pull16(&mut self, bus: &mut dyn M6809Bus, sp: &mut u16) -> u16 {
        let hi = self.pull8(bus, sp);
        let lo = self.pull8(bus, sp);
        u16::from_be_bytes([hi, lo])
    }

    /// Pull from the stack `sp`; returns the value pulled for bit 6 (the
    /// other stack pointer), if any. Order: CC, A, B, DP, X, Y, U/S, PC.
    pub(crate) fn pull(&mut self, bus: &mut dyn M6809Bus, sp: &mut u16, mask: u8) -> Option<u16> {
        if mask & 0x01 != 0 {
            self.cc = self.pull8(bus, sp);
        }
        if mask & 0x02 != 0 {
            self.a = self.pull8(bus, sp);
        }
        if mask & 0x04 != 0 {
            self.b = self.pull8(bus, sp);
        }
        if mask & 0x08 != 0 {
            self.dp = self.pull8(bus, sp);
        }
        if mask & 0x10 != 0 {
            self.x = self.pull16(bus, sp);
        }
        if mask & 0x20 != 0 {
            self.y = self.pull16(bus, sp);
        }
        let other = (mask & 0x40 != 0).then(|| self.pull16(bus, sp));
        if mask & 0x80 != 0 {
            self.pc = self.pull16(bus, sp);
        }
        other
    }

    pub fn regs(&self) -> RegisterSet {
        let reg = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", u32::from(self.pc), 16),
            reg("A", u32::from(self.a), 8),
            reg("B", u32::from(self.b), 8),
            reg("X", u32::from(self.x), 16),
            reg("Y", u32::from(self.y), 16),
            reg("U", u32::from(self.u), 16),
            reg("S", u32::from(self.s), 16),
            reg("DP", u32::from(self.dp), 8),
            reg("CC", u32::from(self.cc), 8),
        ])
    }

    pub fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16,
            "A" => self.a = v as u8,
            "B" => self.b = v as u8,
            "D" => self.set_d(v as u16),
            "X" => self.x = v as u16,
            "Y" => self.y = v as u16,
            "U" => self.u = v as u16,
            "S" | "SP" => self.set_s(v as u16),
            "DP" => self.dp = v as u8,
            "CC" => self.cc = v as u8,
            _ => {}
        }
    }
}

const STATE_TAG: [u8; 4] = *b"6809";

impl Savestate for M6809 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [self.a, self.b, self.dp, self.cc] {
                w.u8(v);
            }
            for v in [self.x, self.y, self.u, self.s, self.pc] {
                w.u16(v);
            }
            w.u8(self.wait as u8);
            let flags = [
                self.nmi_armed,
                self.nmi_line,
                self.nmi_pending,
                self.firq,
                self.irq,
            ];
            for v in flags {
                w.bool(v);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.a = c.u8()?;
        n.b = c.u8()?;
        n.dp = c.u8()?;
        n.cc = c.u8()?;
        n.x = c.u16()?;
        n.y = c.u16()?;
        n.u = c.u16()?;
        n.s = c.u16()?;
        n.pc = c.u16()?;
        n.wait = match c.u8()? {
            0 => Wait::None,
            1 => Wait::Cwai,
            2 => Wait::Sync,
            _ => Wait::Halt,
        };
        n.nmi_armed = c.bool()?;
        n.nmi_line = c.bool()?;
        n.nmi_pending = c.bool()?;
        n.firq = c.bool()?;
        n.irq = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
