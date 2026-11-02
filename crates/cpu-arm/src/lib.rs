//! Acorn RISC Machine ARM2 (VL86C010): the 26-bit ARM of the Archimedes.
//!
//! Sixteen 32-bit registers visible at a time, R13–R14 banked for SVC and
//! IRQ mode, R8–R14 for FIQ mode. R15 holds the 24-bit word address of the
//! program counter in bits 25–2 and the processor status around it: bits
//! 31–28 N Z C V, 27 I, 26 F, 1–0 the mode (0 user, 1 FIQ, 2 IRQ, 3 SVC).
//!
//! **Pipeline.** Fetch, decode, execute as on the chip: an instruction runs
//! with PC its own address + 8 (+ 12 from its second cycle on), a word
//! written over the next two instructions takes no effect, and a prefetch
//! abort only counts if the instruction reaches execution.
//!
//! **Timing.** [`Arm2::step`] runs one instruction. Every bus cycle goes
//! through [`ArmBus`], tagged sequential or not, and internal cycles through
//! [`ArmBus::idle`], so the machine charges each as its memory takes it
//! (MEMC: S and I one clock, N two). Table 21 of the data manual.
//!
//! **Traps** (vector, mode): reset 0, SVC; undefined instruction 4, SVC; SWI
//! 8, SVC; prefetch abort $C, SVC; data abort $10, SVC; address exception
//! $14, SVC; IRQ $18, IRQ; FIRQ $1C, FIQ. All set I, reset and FIRQ also F.
//! Coprocessor instructions are undefined on the ARM2: the Archimedes
//! emulates floating point in that trap.
//!
//! **Aborts.** LDR and STR act as if they had not run. LDM and STM go on to
//! the end without changing registers, update the base if asked and then
//! trap.
//!
//! Sources: VLSI Technology, "Acorn RISC Machine (ARM) Family Data Manual"
//! (1990), VL86C010 chapters: register model, instruction descriptions,
//! R15 as an operand, the abort rules, table 21 (instruction timing). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
#[cfg(test)]
mod tests;

use emu_core::{
    Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};

/// The program counter's bits in R15.
pub const PC_MASK: u32 = 0x03ff_fffc;
/// The status bits in R15.
pub const PSR_MASK: u32 = !PC_MASK;
pub const F_N: u32 = 1 << 31;
pub const F_Z: u32 = 1 << 30;
pub const F_C: u32 = 1 << 29;
pub const F_V: u32 = 1 << 28;
pub const F_I: u32 = 1 << 27;
pub const F_F: u32 = 1 << 26;

pub const MODE_USR: u32 = 0;
pub const MODE_FIQ: u32 = 1;
pub const MODE_IRQ: u32 = 2;
pub const MODE_SVC: u32 = 3;

/// The ARM2's memory interface. `seq`: the access follows on from the last
/// (an S-cycle, else an N-cycle). `privileged`: the processor is in a
/// non-user mode (/TRANS; LDRT and STRT force it off). `None` or `false`:
/// the memory system signalled an abort.
pub trait ArmBus {
    fn fetch(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32>;
    /// The word at `addr & !3` (writes: `addr` keeps its low bits on the
    /// bus).
    fn read_word(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32>;
    fn read_byte(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u8>;
    fn write_word(&mut self, addr: u32, v: u32, seq: bool, privileged: bool) -> bool;
    fn write_byte(&mut self, addr: u32, v: u8, seq: bool, privileged: bool) -> bool;
    /// `n` internal cycles.
    fn idle(&mut self, n: u32);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fetched {
    pub word: u32,
    pub aborted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm2 {
    /// R0–R14 of the current mode.
    pub r: [u32; 15],
    /// R15's program counter bits: the address of the running instruction
    /// + 8.
    pub pc: u32,
    /// R15's status bits.
    pub psr: u32,
    /// R8–R14 of user mode (IRQ and SVC mode share R8–R12 with it).
    user: [u32; 7],
    /// R8–R14 of FIQ mode.
    fiq: [u32; 7],
    /// R13–R14 of IRQ mode.
    irq: [u32; 2],
    /// R13–R14 of SVC mode.
    svc: [u32; 2],
    pub(crate) pipe: [Fetched; 2],
    /// The next fetch is an N-cycle (after a data access).
    pub(crate) fetch_n: bool,
    irq_line: bool,
    firq_line: bool,
    /// [`Cpu::tick`]: cycles of the instruction it ran still to count.
    left: u32,
}

impl Default for Arm2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Arm2 {
    /// Held in reset: [`Arm2::reset`] starts it.
    pub fn new() -> Self {
        Self {
            r: [0; 15],
            pc: 0,
            psr: F_I | F_F | MODE_SVC,
            user: [0; 7],
            fiq: [0; 7],
            irq: [0; 2],
            svc: [0; 2],
            pipe: [Fetched::default(); 2],
            fetch_n: false,
            irq_line: false,
            firq_line: false,
            left: 0,
        }
    }

    /// /RESET: SVC mode, interrupts off, R14_svc holds the old PC and PSR,
    /// execution restarts at 0.
    pub fn reset<B: ArmBus>(&mut self, bus: &mut B) {
        let old = self.r15();
        self.set_psr((self.psr & !3) | MODE_SVC | F_I | F_F);
        self.r[14] = old;
        self.left = 0;
        self.branch(0, bus);
    }

    /// /IRQ pulled low.
    pub fn set_irq(&mut self, level: bool) {
        self.irq_line = level;
    }

    /// /FIRQ pulled low.
    pub fn set_firq(&mut self, level: bool) {
        self.firq_line = level;
    }

    pub fn mode(&self) -> u32 {
        self.psr & 3
    }

    pub fn r15(&self) -> u32 {
        self.pc | self.psr
    }

    /// The address of the instruction that runs next.
    pub fn next_pc(&self) -> u32 {
        self.pc.wrapping_sub(8) & PC_MASK
    }

    pub(crate) fn privileged(&self) -> bool {
        self.mode() != MODE_USR
    }

    /// Set the status bits; a new mode brings its registers in.
    pub(crate) fn set_psr(&mut self, new: u32) {
        let new = new & PSR_MASK;
        let (old_mode, new_mode) = (self.psr & 3, new & 3);
        if old_mode != new_mode {
            // R8–R12: FIQ has its own, all others the user bank's.
            if (old_mode == MODE_FIQ) != (new_mode == MODE_FIQ) {
                if old_mode == MODE_FIQ {
                    self.fiq[..5].copy_from_slice(&self.r[8..13]);
                    self.r[8..13].copy_from_slice(&self.user[..5]);
                } else {
                    self.user[..5].copy_from_slice(&self.r[8..13]);
                    self.r[8..13].copy_from_slice(&self.fiq[..5]);
                }
            }
            let (r13, r14) = (self.r[13], self.r[14]);
            match old_mode {
                MODE_FIQ => self.fiq[5..7].copy_from_slice(&[r13, r14]),
                MODE_IRQ => self.irq = [r13, r14],
                MODE_SVC => self.svc = [r13, r14],
                _ => self.user[5..7].copy_from_slice(&[r13, r14]),
            }
            let [r13, r14] = match new_mode {
                MODE_FIQ => [self.fiq[5], self.fiq[6]],
                MODE_IRQ => self.irq,
                MODE_SVC => self.svc,
                _ => [self.user[5], self.user[6]],
            };
            self.r[13] = r13;
            self.r[14] = r14;
        }
        self.psr = new;
    }

    /// Register `n` (0–14) of the user bank, whatever the mode.
    pub(crate) fn user_reg(&self, n: usize) -> u32 {
        let mode = self.mode();
        match n {
            8..=12 if mode == MODE_FIQ => self.user[n - 8],
            13..=14 if mode != MODE_USR => self.user[n - 8],
            _ => self.r[n],
        }
    }

    pub(crate) fn set_user_reg(&mut self, n: usize, v: u32) {
        let mode = self.mode();
        match n {
            8..=12 if mode == MODE_FIQ => self.user[n - 8] = v,
            13..=14 if mode != MODE_USR => self.user[n - 8] = v,
            _ => self.r[n] = v,
        }
    }

    /// One instruction, or the entry to an interrupt. Returns the bus and
    /// internal cycles it took.
    pub fn step<B: ArmBus>(&mut self, bus: &mut B) -> u32 {
        let mut counted = Counted { bus, cycles: 0 };
        if self.firq_line && self.psr & F_F == 0 {
            // The instruction in execute is the one that runs afterwards.
            self.trap(
                0x1c,
                MODE_FIQ,
                F_I | F_F,
                self.pc.wrapping_sub(4),
                &mut counted,
            );
        } else if self.irq_line && self.psr & F_I == 0 {
            self.trap(0x18, MODE_IRQ, F_I, self.pc.wrapping_sub(4), &mut counted);
        } else {
            self.execute(&mut counted);
        }
        counted.cycles
    }

    /// Take a trap: `link` (a PC) with the old status goes to R14 of the
    /// new mode.
    pub(crate) fn trap<B: ArmBus>(
        &mut self,
        vector: u32,
        mode: u32,
        mask: u32,
        link: u32,
        bus: &mut B,
    ) {
        let saved = (link & PC_MASK) | self.psr;
        self.set_psr((self.psr & !3) | mode | mask);
        self.r[14] = saved;
        self.branch(vector, bus);
    }

    /// One fetch into the end of the pipeline.
    pub(crate) fn prefetch<B: ArmBus>(&mut self, seq: bool, bus: &mut B) -> Fetched {
        let privileged = self.privileged();
        match bus.fetch(self.pc, seq, privileged) {
            Some(word) => Fetched {
                word,
                aborted: false,
            },
            None => Fetched {
                word: 0,
                aborted: true,
            },
        }
    }

    /// Jump: the pipeline fills from `addr` with an N and an S cycle.
    pub(crate) fn branch<B: ArmBus>(&mut self, addr: u32, bus: &mut B) {
        self.pc = addr & PC_MASK;
        self.pipe[0] = self.prefetch(false, bus);
        self.pc = (self.pc + 4) & PC_MASK;
        self.pipe[1] = self.prefetch(true, bus);
        self.pc = (self.pc + 4) & PC_MASK;
        self.fetch_n = false;
    }
}

/// Counts what passes through to the machine's bus.
struct Counted<'a, B: ArmBus> {
    bus: &'a mut B,
    cycles: u32,
}

impl<B: ArmBus> ArmBus for Counted<'_, B> {
    fn fetch(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        self.cycles += 1;
        self.bus.fetch(addr, seq, privileged)
    }
    fn read_word(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u32> {
        self.cycles += 1;
        self.bus.read_word(addr, seq, privileged)
    }
    fn read_byte(&mut self, addr: u32, seq: bool, privileged: bool) -> Option<u8> {
        self.cycles += 1;
        self.bus.read_byte(addr, seq, privileged)
    }
    fn write_word(&mut self, addr: u32, v: u32, seq: bool, privileged: bool) -> bool {
        self.cycles += 1;
        self.bus.write_word(addr, v, seq, privileged)
    }
    fn write_byte(&mut self, addr: u32, v: u8, seq: bool, privileged: bool) -> bool {
        self.cycles += 1;
        self.bus.write_byte(addr, v, seq, privileged)
    }
    fn idle(&mut self, n: u32) {
        self.cycles += n;
        self.bus.idle(n);
    }
}

/// [`ArmBus`] over a plain [`Bus`], for [`Cpu::tick`].
struct PlainBus<'a>(&'a mut dyn Bus);

impl ArmBus for PlainBus<'_> {
    fn fetch(&mut self, addr: u32, _seq: bool, _privileged: bool) -> Option<u32> {
        Some(self.0.read(addr, Size::Long))
    }
    fn read_word(&mut self, addr: u32, _seq: bool, _privileged: bool) -> Option<u32> {
        Some(self.0.read(addr, Size::Long))
    }
    fn read_byte(&mut self, addr: u32, _seq: bool, _privileged: bool) -> Option<u8> {
        Some(self.0.read(addr, Size::Byte) as u8)
    }
    fn write_word(&mut self, addr: u32, v: u32, _seq: bool, _privileged: bool) -> bool {
        self.0.write(addr, Size::Long, v);
        true
    }
    fn write_byte(&mut self, addr: u32, v: u8, _seq: bool, _privileged: bool) -> bool {
        self.0.write(addr, Size::Byte, u32::from(v));
        true
    }
    fn idle(&mut self, _n: u32) {}
}

impl Cpu for Arm2 {
    /// The whole instruction runs on its first cycle; the others only count.
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        if self.left == 0 {
            self.left = self.step(&mut PlainBus(bus));
        }
        self.left -= 1;
        if self.left == 0 {
            CycleResult::InstrDone {
                next_pc: self.next_pc(),
            }
        } else {
            CycleResult::Busy
        }
    }

    fn regs(&self) -> RegisterSet {
        const NAMES: [&str; 15] = [
            "R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9", "R10", "R11", "R12", "R13",
            "R14",
        ];
        let mut list: Vec<Register> = NAMES
            .iter()
            .zip(self.r)
            .map(|(&name, value)| Register {
                name,
                value,
                bits: 32,
            })
            .collect();
        list.push(Register {
            name: "PC",
            value: self.next_pc(),
            bits: 26,
        });
        list.push(Register {
            name: "PSR",
            value: self.psr,
            bits: 32,
        });
        RegisterSet(list)
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let upper = name.to_ascii_uppercase();
        if let Some(n) = upper
            .strip_prefix('R')
            .and_then(|s| s.parse::<usize>().ok())
        {
            if n < 15 {
                self.r[n] = v;
            }
        } else if upper == "PSR" {
            self.set_psr(v);
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

const STATE_TAG: [u8; 4] = *b"ARM2";

impl Savestate for Arm2 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for v in self.r {
                w.u32(v);
            }
            w.u32(self.pc);
            w.u32(self.psr);
            for v in self
                .user
                .iter()
                .chain(&self.fiq)
                .chain(&self.irq)
                .chain(&self.svc)
            {
                w.u32(*v);
            }
            for p in self.pipe {
                w.u32(p.word);
                w.bool(p.aborted);
            }
            w.bool(self.fetch_n);
            w.bool(self.irq_line);
            w.bool(self.firq_line);
            w.u32(self.left);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        for v in &mut n.r {
            *v = c.u32()?;
        }
        n.pc = c.u32()? & PC_MASK;
        n.psr = c.u32()? & PSR_MASK;
        for v in n
            .user
            .iter_mut()
            .chain(&mut n.fiq)
            .chain(&mut n.irq)
            .chain(&mut n.svc)
        {
            *v = c.u32()?;
        }
        for p in &mut n.pipe {
            p.word = c.u32()?;
            p.aborted = c.bool()?;
        }
        n.fetch_n = c.bool()?;
        n.irq_line = c.bool()?;
        n.firq_line = c.bool()?;
        n.left = c.u32()?;
        *self = n;
        Ok(())
    }
}
