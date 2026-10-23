//! Sony SPC700: the 8-bit CPU of the SNES's sound module (S-SMP).
//!
//! [`Spc700::step`] runs one instruction and reports every bus cycle to
//! the [`SpcBus`]: reads (also the dummy reads the chip makes, whose value
//! it throws away — they matter where reading has side effects, like the
//! S-SMP's timer counters), writes and internal cycles.
//!
//! Registers A, X, Y, SP (stack at $0100–$01FF), PC, PSW (N V P B H I Z
//! C; P puts the direct page at $0100 instead of $0000). YA is A (low)
//! and Y (high) as a 16-bit pair.
//!
//! Sources: Sony SPC700 instruction set (as documented in the SNES
//! development manual, book 1, "Sound"); anomie's "SPC700 doc"; Martin
//! Korth, fullsnes, "SNES APU SPC700 CPU" (DIV overflow, DAA/DAS, cycle
//! counts); bus cycles checked against Tom Harte's SingleStepTests
//! (`spc700/v1`). The implementation is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};
use emu_core::{Register, RegisterSet};

pub use disasm::disassemble;

pub const C: u8 = 0x01;
pub const Z: u8 = 0x02;
pub const I: u8 = 0x04;
pub const H: u8 = 0x08;
pub const B: u8 = 0x10;
pub const P: u8 = 0x20;
pub const V: u8 = 0x40;
pub const N: u8 = 0x80;

/// What an SPC700 is connected to. Each call is one bus cycle.
pub trait SpcBus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, v: u8);
    /// An internal cycle.
    fn idle(&mut self);
    /// Side-effect-free read (disassembler, debugger).
    fn peek(&self, addr: u16) -> u8;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spc700 {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub psw: u8,
    /// SLEEP or STOP executed: the CPU only idles (until reset).
    pub halted: bool,
    cycles: u32,
}

impl Default for Spc700 {
    fn default() -> Self {
        Self::new()
    }
}

impl Spc700 {
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xef,
            pc: 0xffc0,
            psw: 0x02,
            halted: false,
            cycles: 0,
        }
    }

    /// Reset: PC from the vector at $FFFE (the IPL ROM's entry), the
    /// other registers as at power-on.
    pub fn reset(&mut self, bus: &mut dyn SpcBus) {
        *self = Self::new();
        let lo = bus.read(0xfffe);
        let hi = bus.read(0xffff);
        self.pc = u16::from_le_bytes([lo, hi]);
    }

    /// Run one instruction; returns its bus cycles.
    pub fn step(&mut self, bus: &mut dyn SpcBus) -> u32 {
        self.cycles = 0;
        if self.halted {
            // SLEEP/STOP: the clock keeps reading the next opcode.
            let pc = self.pc;
            self.rd(bus, pc);
            self.idle(bus);
        } else {
            exec::execute(self, bus);
        }
        self.cycles
    }

    fn rd(&mut self, bus: &mut dyn SpcBus, addr: u16) -> u8 {
        self.cycles += 1;
        bus.read(addr)
    }

    fn wr(&mut self, bus: &mut dyn SpcBus, addr: u16, v: u8) {
        self.cycles += 1;
        bus.write(addr, v);
    }

    fn idle(&mut self, bus: &mut dyn SpcBus) {
        self.cycles += 1;
        bus.idle();
    }

    pub fn ya(&self) -> u16 {
        u16::from_le_bytes([self.a, self.y])
    }

    pub fn regs(&self) -> RegisterSet {
        let reg = |name, value: u32, bits| Register { name, value, bits };
        RegisterSet(vec![
            reg("PC", u32::from(self.pc), 16),
            reg("A", u32::from(self.a), 8),
            reg("X", u32::from(self.x), 8),
            reg("Y", u32::from(self.y), 8),
            reg("SP", u32::from(self.sp), 8),
            reg("PSW", u32::from(self.psw), 8),
        ])
    }

    pub fn set_reg(&mut self, name: &str, v: u32) {
        match name.to_ascii_uppercase().as_str() {
            "PC" => self.pc = v as u16,
            "A" => self.a = v as u8,
            "X" => self.x = v as u8,
            "Y" => self.y = v as u8,
            "SP" | "S" => self.sp = v as u8,
            "PSW" | "P" => self.psw = v as u8,
            _ => {}
        }
    }
}

const STATE_TAG: [u8; 4] = *b"S700";

impl Savestate for Spc700 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in [self.a, self.x, self.y, self.sp, self.psw] {
                w.u8(v);
            }
            w.u16(self.pc);
            w.bool(self.halted);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.a = c.u8()?;
        n.x = c.u8()?;
        n.y = c.u8()?;
        n.sp = c.u8()?;
        n.psw = c.u8()?;
        n.pc = c.u16()?;
        n.halted = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
