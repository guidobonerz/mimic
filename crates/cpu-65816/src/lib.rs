//! WDC 65C816: the 16-bit extension of the 6502 with 24-bit addresses, the
//! core of the SNES's Ricoh 5A22.
//!
//! [`Cpu65816::step`] runs one instruction (or interrupt sequence) and
//! reports every bus cycle to the [`Bus816`]: reads, writes and the
//! internal operation cycles ("IO", no memory access), each with the
//! address the chip puts out. A machine whose cycles differ in length by
//! address (the SNES: 6, 8 or 12 master clocks) advances its clock per
//! callback. The interrupt inputs are asked for before every cycle; as on
//! the 6502, the state before an instruction's last cycle decides whether
//! an interrupt follows it (NMI edge-triggered, IRQ level-triggered and
//! masked by I).
//!
//! Registers: A (C in 16 bits; B the high byte), X, Y, S, D (direct
//! page), DBR (data bank), PBR (program bank), PC, P (N V M X D I Z C;
//! in emulation mode M and X read as 1 and bit 4 is B on the stack), E
//! (emulation flag, swapped with C by XCE).
//!
//! Sources: Western Design Center, *W65C816S 8/16-bit Microprocessor*
//! datasheet (cycle-by-cycle table 5-7, vectors, the emulation-mode
//! rules); David Eyes and Ron Lichty, *Programming the 65816*; cycle
//! details checked against Tom Harte's SingleStepTests (`65816/v1`). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{InterruptKind, Register, RegisterSet};

pub use disasm::disassemble;

pub const C: u8 = 0x01;
pub const Z: u8 = 0x02;
pub const I: u8 = 0x04;
pub const D: u8 = 0x08;
/// Index width (1 = 8 bits); B on the stack in emulation mode.
pub const X: u8 = 0x10;
/// Accumulator/memory width (1 = 8 bits).
pub const M: u8 = 0x20;
pub const V: u8 = 0x40;
pub const N: u8 = 0x80;

/// What a 65816 is connected to. Each call is one bus cycle.
pub trait Bus816 {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, v: u8);
    /// An internal operation cycle with `addr` on the address bus.
    fn idle(&mut self, addr: u32);
    /// Side-effect-free read (disassembler, debugger).
    fn peek(&self, addr: u32) -> u8;
    /// /NMI asserted (asked before every cycle).
    fn nmi(&mut self) -> bool {
        false
    }
    /// /IRQ asserted (asked before every cycle).
    fn irq(&mut self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu65816 {
    pub a: u16,
    pub x: u16,
    pub y: u16,
    pub s: u16,
    pub d: u16,
    pub dbr: u8,
    pub pbr: u8,
    pub pc: u16,
    pub p: u8,
    pub e: bool,
    /// An NMI edge was seen and not yet serviced.
    nmi_pending: bool,
    /// The last sampled /NMI level.
    nmi_line: bool,
    /// The /IRQ level and the I flag at the last sample.
    irq_line: bool,
    i_flag: bool,
    /// The interrupt to take before the next instruction.
    take: Option<InterruptKind>,
    /// WAI: waiting for an interrupt; STP: stopped until reset.
    pub waiting: bool,
    pub stopped: bool,
    reset_pending: bool,
    /// Bus cycles of the current step.
    cycles: u32,
}

impl Default for Cpu65816 {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu65816 {
    /// A CPU in emulation mode; [`Cpu65816::reset`] starts it.
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            s: 0x01ff,
            d: 0,
            dbr: 0,
            pbr: 0,
            pc: 0,
            p: M | X | I,
            e: true,
            nmi_pending: false,
            nmi_line: false,
            irq_line: false,
            i_flag: true,
            take: None,
            waiting: false,
            stopped: false,
            reset_pending: false,
            cycles: 0,
        }
    }

    /// The reset sequence runs at the next step (vector $FFFC).
    pub fn reset(&mut self) {
        self.reset_pending = true;
        self.stopped = false;
        self.waiting = false;
        self.take = None;
        self.nmi_pending = false;
    }

    /// PBR:PC.
    pub fn pc24(&self) -> u32 {
        u32::from(self.pbr) << 16 | u32::from(self.pc)
    }

    /// Run one instruction, interrupt sequence or wait cycle. Returns the
    /// bus cycles and the hardware interrupt entered, if any.
    pub fn step(&mut self, bus: &mut dyn Bus816) -> (u32, Option<InterruptKind>) {
        self.cycles = 0;
        let mut entered = None;
        if self.reset_pending {
            self.reset_pending = false;
            self.reset_sequence(bus);
        } else if self.stopped {
            self.io(bus, self.pc24());
        } else if self.waiting {
            self.io(bus, self.pc24());
            if self.nmi_pending || self.irq_line {
                self.waiting = false;
                self.decide();
            }
        } else if let Some(kind) = self.take.take() {
            self.interrupt(bus, kind);
            entered = Some(kind);
            self.decide();
        } else {
            exec::execute(self, bus);
            self.decide();
        }
        (self.cycles, entered)
    }

    /// After an instruction: the inputs sampled before its last cycle
    /// decide whether an interrupt follows.
    fn decide(&mut self) {
        if self.waiting || self.stopped {
            return;
        }
        if self.nmi_pending {
            self.nmi_pending = false;
            self.take = Some(InterruptKind::Nmi);
        } else if self.irq_line && !self.i_flag {
            self.take = Some(InterruptKind::Irq);
        }
    }

    fn sample(&mut self, bus: &mut dyn Bus816) {
        let nmi = bus.nmi();
        if nmi && !self.nmi_line {
            self.nmi_pending = true;
        }
        self.nmi_line = nmi;
        self.irq_line = bus.irq();
        self.i_flag = self.p & I != 0;
    }

    fn rd(&mut self, bus: &mut dyn Bus816, addr: u32) -> u8 {
        self.sample(bus);
        self.cycles += 1;
        bus.read(addr & 0xff_ffff)
    }

    fn wr(&mut self, bus: &mut dyn Bus816, addr: u32, v: u8) {
        self.sample(bus);
        self.cycles += 1;
        bus.write(addr & 0xff_ffff, v);
    }

    fn io(&mut self, bus: &mut dyn Bus816, addr: u32) {
        self.sample(bus);
        self.cycles += 1;
        bus.idle(addr & 0xff_ffff);
    }

    fn reset_sequence(&mut self, bus: &mut dyn Bus816) {
        self.e = true;
        self.p = (self.p | M | X | I) & !D;
        self.x &= 0xff;
        self.y &= 0xff;
        self.s = 0x0100 | (self.s & 0xff);
        self.d = 0;
        self.dbr = 0;
        self.pbr = 0;
        let pc = self.pc24();
        self.io(bus, pc);
        self.io(bus, pc);
        // Three stack cycles with the write line held off (reads).
        for _ in 0..3 {
            let s = u32::from(self.s);
            self.rd(bus, s);
            self.s = 0x0100 | (self.s.wrapping_sub(1) & 0xff);
        }
        let lo = self.rd(bus, 0xfffc);
        let hi = self.rd(bus, 0xfffd);
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    /// A hardware interrupt (NMI or IRQ): two cycles at PC, the pushes,
    /// the vector.
    fn interrupt(&mut self, bus: &mut dyn Bus816, kind: InterruptKind) {
        let pc = self.pc24();
        self.rd(bus, pc);
        self.io(bus, pc);
        let vector = match (kind, self.e) {
            (InterruptKind::Nmi, true) => 0xfffa,
            (InterruptKind::Nmi, false) => 0xffea,
            (_, true) => 0xfffe,
            (_, false) => 0xffee,
        };
        exec::enter(self, bus, vector, false);
    }

    /// Register dump for the debugger.
    pub fn regs(&self) -> RegisterSet {
        let reg = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", self.pc24(), 24),
            reg("A", u32::from(self.a), 16),
            reg("X", u32::from(self.x), 16),
            reg("Y", u32::from(self.y), 16),
            reg("S", u32::from(self.s), 16),
            reg("D", u32::from(self.d), 16),
            reg("DBR", u32::from(self.dbr), 8),
            reg("PBR", u32::from(self.pbr), 8),
            reg("P", u32::from(self.p), 8),
            reg("E", u32::from(self.e), 1),
        ])
    }

    pub fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => {
                self.pc = v as u16;
                if v > 0xffff {
                    self.pbr = (v >> 16) as u8;
                }
            }
            "A" | "C" => self.a = v as u16,
            "X" => self.x = v as u16,
            "Y" => self.y = v as u16,
            "S" | "SP" => self.s = v as u16,
            "D" | "DP" => self.d = v as u16,
            "DBR" | "DB" => self.dbr = v as u8,
            "PBR" | "PB" | "K" => self.pbr = v as u8,
            "P" => self.p = v as u8,
            "E" => self.e = v & 1 != 0,
            _ => return,
        }
        self.fix_widths();
    }

    /// Emulation mode forces M, X and page-1 S; an 8-bit index clears the
    /// high bytes of X and Y.
    pub(crate) fn fix_widths(&mut self) {
        if self.e {
            self.p |= M | X;
            self.s = 0x0100 | (self.s & 0xff);
        }
        if self.p & X != 0 {
            self.x &= 0xff;
            self.y &= 0xff;
        }
    }
}

const STATE_TAG: [u8; 4] = *b"W816";

impl Savestate for Cpu65816 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [self.a, self.x, self.y, self.s, self.d, self.pc] {
                w.u16(v);
            }
            for v in [self.dbr, self.pbr, self.p] {
                w.u8(v);
            }
            for v in [
                self.e,
                self.nmi_pending,
                self.nmi_line,
                self.irq_line,
                self.i_flag,
                self.waiting,
                self.stopped,
                self.reset_pending,
            ] {
                w.bool(v);
            }
            w.u8(match self.take {
                None => 0,
                Some(InterruptKind::Nmi) => 1,
                Some(_) => 2,
            });
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.a = c.u16()?;
        n.x = c.u16()?;
        n.y = c.u16()?;
        n.s = c.u16()?;
        n.d = c.u16()?;
        n.pc = c.u16()?;
        n.dbr = c.u8()?;
        n.pbr = c.u8()?;
        n.p = c.u8()?;
        n.e = c.bool()?;
        n.nmi_pending = c.bool()?;
        n.nmi_line = c.bool()?;
        n.irq_line = c.bool()?;
        n.i_flag = c.bool()?;
        n.waiting = c.bool()?;
        n.stopped = c.bool()?;
        n.reset_pending = c.bool()?;
        n.take = match c.u8()? {
            0 => None,
            1 => Some(InterruptKind::Nmi),
            2 => Some(InterruptKind::Irq),
            _ => return Err(StateError::Invalid("65816 interrupt")),
        };
        n.fix_widths();
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
