//! Motorola 68000 (Amiga 500), later 68020 (A1200).
//!
//! The complete 68000 instruction set with its exceptions (address error,
//! illegal instruction, privilege violation, trace, TRAP/TRAPV/CHK, division
//! by zero, line A/F, interrupts with autovectors) and its bus behaviour:
//! the two-word prefetch queue (IRD holds the opcode being executed, IRC the
//! next word), every bus cycle in the order the chip performs it, and the
//! internal clocks between them.
//!
//! **Timing.** A bus cycle takes 4 clocks; internal operations take 2
//! clocks each. [`M68000::step_timed`] executes one instruction (or one
//! exception) and before every bus cycle tells the [`TimedBus68`] on which
//! clock of the instruction it starts, so the bus can bring the rest of the
//! machine up to that moment and stretch the cycle (the Amiga's Agnus gives
//! the CPU only free chip-bus slots). The order and placement of the cycles
//! is checked against the bus transactions of Tom Harte's 68000 tests.
//!
//! **Bus.** Addresses are 24 bits; data goes through [`Bus`] in bytes or
//! words (the data bus is 16 bits wide, longs are two word cycles). A word
//! or long access at an odd address raises an address error.
//!
//! **Interrupts.** [`M68000::set_ipl`] is the level on IPL0–2 (0 = none, 7
//! is non-maskable). Interrupts are acknowledged with autovectors (the
//! Amiga's VPA), vectors 25–31.
//!
//! Sources: Motorola M68000 Family Programmer's Reference Manual
//! (M68000PM/AD) and MC68000 User's Manual (M68000UM/AD), sections on
//! exception processing and instruction execution times; Tom Harte's
//! SingleStepTests 680x0 (MIT) for the bus-cycle order; "YACHT — Yet
//! Another Cycle Hunting Table" (gist cbmeeks/e759c7061d61ec4ac354a7df44a4a8f1)
//! for the per-instruction cycle sequences; Flamewing's analysis of the
//! BCD instructions for ABCD/SBCD/NBCD flags. The implementation
//! is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod ea;
mod exec;

use emu_core::{
    Bus, Cpu, CycleResult, Instr, InterruptKind, Register, RegisterSet, Savestate, Size,
    StateError, StateReader, StateWriter,
};

/// Status register bits.
pub mod sr {
    pub const T: u16 = 0x8000;
    pub const S: u16 = 0x2000;
    pub const IPL: u16 = 0x0700;
    pub const X: u16 = 0x10;
    pub const N: u16 = 0x08;
    pub const Z: u16 = 0x04;
    pub const V: u16 = 0x02;
    pub const C: u16 = 0x01;
    /// Bits that exist on the 68000.
    pub const MASK: u16 = 0xa71f;
}

/// Function codes on FC0–2.
pub mod fc {
    pub const USER_DATA: u8 = 1;
    pub const USER_PROGRAM: u8 = 2;
    pub const SUPERVISOR_DATA: u8 = 5;
    pub const SUPERVISOR_PROGRAM: u8 = 6;
    pub const INT_ACK: u8 = 7;
}

/// The kind of a bus cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// TAS: read, modify and write in one indivisible 10-clock cycle.
    ReadModifyWrite,
    /// Interrupt acknowledge (autovector).
    IntAck,
}

/// A bus that can stretch the 68000's bus cycles ([`M68000::step_timed`]).
pub trait TimedBus68: Bus {
    /// A bus cycle begins on clock `t` of the current instruction (counted
    /// from 0, including earlier waits). Returns the wait clocks to insert
    /// before it completes (late DTACK); the cycle then takes `4 + waits`
    /// clocks (`10 + waits` for [`Access::ReadModifyWrite`]).
    fn bus_cycle(&mut self, access: Access, fc: u8, addr: u32, size: Size, t: u32) -> u32;

    /// The RESET instruction asserts /RSTO: external devices reset.
    fn reset_devices(&mut self) {}
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

impl TimedBus68 for NoWait<'_> {
    fn bus_cycle(&mut self, _: Access, _: u8, _: u32, _: Size, _: u32) -> u32 {
        0
    }
}

/// Why an instruction stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Word or long access at an odd address.
    AddressError {
        addr: u32,
        write: bool,
        /// Instruction (program space) access.
        program: bool,
    },
}

/// Exception vector numbers.
pub(crate) mod vector {
    pub const ADDRESS_ERROR: u8 = 3;
    pub const ILLEGAL: u8 = 4;
    pub const ZERO_DIVIDE: u8 = 5;
    pub const CHK: u8 = 6;
    pub const TRAPV: u8 = 7;
    pub const PRIVILEGE: u8 = 8;
    pub const TRACE: u8 = 9;
    pub const LINE_A: u8 = 10;
    pub const LINE_F: u8 = 11;
    pub const SPURIOUS: u8 = 24;
    pub const TRAP0: u8 = 32;
}

pub(crate) type Res<T = ()> = Result<T, Fault>;

/// What the last [`M68000::step_timed`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Executed an instruction (or took an exception); `clocks` it took.
    Done { clocks: u32 },
    /// STOP: waiting for an interrupt (`clocks` spent waiting).
    Stopped { clocks: u32 },
    /// Double bus fault: the CPU halted until reset.
    Halted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct M68000 {
    pub d: [u32; 8],
    /// A0–A7; A7 is the stack pointer of the current mode.
    pub a: [u32; 8],
    /// The other mode's stack pointer (USP in supervisor mode, SSP in user
    /// mode).
    other_sp: u32,
    sr: u16,
    /// The instruction being executed (IR), for exception frames; IRD
    /// may already hold the next one after an early prefetch.
    ir: u16,
    /// Opcode being executed and the prefetched word after it.
    ird: u16,
    irc: u16,
    /// Address of the next prefetch; the instruction in IRD is at `au - 4`.
    au: u32,
    /// Interrupt level on IPL0–2 and the level 7 edge latch.
    ipl: u8,
    nmi_seen: bool,
    stopped: bool,
    halted: bool,
    /// Clock of the current step (bus cycle placement).
    t: u32,
    /// Interrupt entered during the last step, for the debugger.
    entered: Option<InterruptKind>,
    /// Trace pending after the current instruction (T set at its start).
    trace: bool,
}

impl Default for M68000 {
    fn default() -> Self {
        Self::new()
    }
}

impl M68000 {
    pub fn new() -> Self {
        Self {
            d: [0; 8],
            a: [0; 8],
            other_sp: 0,
            sr: sr::S | sr::IPL,
            ir: 0,
            ird: 0,
            irc: 0,
            au: 0,
            ipl: 0,
            nmi_seen: false,
            stopped: false,
            halted: false,
            t: 0,
            entered: None,
            trace: false,
        }
    }

    pub fn sr(&self) -> u16 {
        self.sr
    }

    /// Set SR, switching stack pointers when S changes.
    pub fn set_sr(&mut self, v: u16) {
        let v = v & sr::MASK;
        if (v ^ self.sr) & sr::S != 0 {
            std::mem::swap(&mut self.a[7], &mut self.other_sp);
        }
        self.sr = v;
    }

    pub fn supervisor(&self) -> bool {
        self.sr & sr::S != 0
    }

    pub fn usp(&self) -> u32 {
        if self.supervisor() {
            self.other_sp
        } else {
            self.a[7]
        }
    }

    pub fn ssp(&self) -> u32 {
        if self.supervisor() {
            self.a[7]
        } else {
            self.other_sp
        }
    }

    pub fn set_usp(&mut self, v: u32) {
        if self.supervisor() {
            self.other_sp = v;
        } else {
            self.a[7] = v;
        }
    }

    pub fn set_ssp(&mut self, v: u32) {
        if self.supervisor() {
            self.a[7] = v;
        } else {
            self.other_sp = v;
        }
    }

    /// Address of the instruction in IRD.
    pub fn pc(&self) -> u32 {
        self.au.wrapping_sub(4) & 0xff_ffff
    }

    /// The prefetch queue: IRD (opcode) and IRC.
    pub fn prefetch_queue(&self) -> [u16; 2] {
        [self.ird, self.irc]
    }

    /// Place the CPU at `pc` with `queue` already prefetched (tests,
    /// savestates, debugger PC changes refill it themselves).
    pub fn set_pc_and_queue(&mut self, pc: u32, queue: [u16; 2]) {
        self.au = pc.wrapping_add(4);
        self.ird = queue[0];
        self.irc = queue[1];
    }

    /// Interrupt level on IPL0–2 (0 = none). Level 7 is edge-triggered.
    pub fn set_ipl(&mut self, level: u8) {
        if level != 7 {
            self.nmi_seen = false;
        }
        self.ipl = level & 7;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Interrupt entered by the last step (debugger interrupt breakpoints).
    pub fn take_interrupt(&mut self) -> Option<InterruptKind> {
        self.entered.take()
    }

    /// RESET: supervisor mode, interrupts masked, SSP and PC from vectors
    /// 0 and 1, then the prefetch. Takes the bus like an instruction.
    pub fn reset_timed(&mut self, bus: &mut dyn TimedBus68) {
        self.t = 0;
        self.halted = false;
        self.stopped = false;
        self.trace = false;
        self.sr = (self.sr & !sr::T) | sr::S | sr::IPL;
        let r: Res = (|| {
            self.idle(bus, 16);
            let ssp = self.read_long(bus, fc::SUPERVISOR_PROGRAM, 0)?;
            let pc = self.read_long(bus, fc::SUPERVISOR_PROGRAM, 4)?;
            self.a[7] = ssp;
            self.jump(bus, pc)?;
            self.prefetch(bus)
        })();
        if r.is_err() {
            self.halted = true;
        }
    }

    /// One instruction (or exception, or STOP wait) with bus timing.
    pub fn step_timed(&mut self, bus: &mut dyn TimedBus68) -> Step {
        self.t = 0;
        if self.halted {
            return Step::Halted;
        }
        if self.interrupt_pending() {
            let level = self.ipl;
            if level == 7 {
                self.nmi_seen = true;
            }
            self.stopped = false;
            self.entered = Some(if level == 7 {
                InterruptKind::Nmi
            } else {
                InterruptKind::Irq
            });
            if let Err(f) = self.interrupt(bus, level) {
                self.fault(bus, f);
            }
            return self.done();
        }
        if self.stopped {
            self.idle(bus, 4);
            return Step::Stopped { clocks: self.t };
        }
        self.trace = self.sr & sr::T != 0;
        let op = self.ird;
        self.ir = op;
        match exec::execute(self, bus, op) {
            Ok(()) => {
                if self.trace && !self.halted {
                    let pc = self.pc();
                    self.idle(bus, 4);
                    if let Err(f) = self.exception(bus, vector::TRACE, pc, Frame::LowHighSr) {
                        self.fault(bus, f);
                    }
                }
            }
            Err(f) => self.fault(bus, f),
        }
        self.done()
    }

    fn done(&self) -> Step {
        if self.halted {
            Step::Halted
        } else {
            Step::Done { clocks: self.t }
        }
    }

    fn interrupt_pending(&self) -> bool {
        let mask = ((self.sr & sr::IPL) >> 8) as u8;
        (self.ipl == 7 && !self.nmi_seen) || (self.ipl > mask && self.ipl < 7)
    }

    // ------------------------------------------------------------ bus

    pub(crate) fn idle(&mut self, _bus: &mut dyn TimedBus68, clocks: u32) {
        self.t += clocks;
    }

    fn data_fc(&self) -> u8 {
        if self.supervisor() {
            fc::SUPERVISOR_DATA
        } else {
            fc::USER_DATA
        }
    }

    pub(crate) fn program_fc(&self) -> u8 {
        if self.supervisor() {
            fc::SUPERVISOR_PROGRAM
        } else {
            fc::USER_PROGRAM
        }
    }

    fn cycle(&mut self, bus: &mut dyn TimedBus68, access: Access, fc: u8, addr: u32, size: Size) {
        let waits = bus.bus_cycle(access, fc, addr, size, self.t);
        self.t += waits
            + if access == Access::ReadModifyWrite {
                10
            } else {
                4
            };
    }

    pub(crate) fn read_at(
        &mut self,
        bus: &mut dyn TimedBus68,
        fc: u8,
        addr: u32,
        size: Size,
    ) -> Res<u32> {
        if size != Size::Byte && addr & 1 != 0 {
            // The frame records the full 32-bit address.
            return Err(Fault::AddressError {
                addr,
                write: false,
                program: fc & 3 == 2,
            });
        }
        let addr = addr & 0xff_ffff;
        self.cycle(bus, Access::Read, fc, addr, size);
        Ok(bus.read(addr, size))
    }

    pub(crate) fn write_at(
        &mut self,
        bus: &mut dyn TimedBus68,
        fc: u8,
        addr: u32,
        size: Size,
        v: u32,
    ) -> Res {
        if size != Size::Byte && addr & 1 != 0 {
            return Err(Fault::AddressError {
                addr,
                write: true,
                program: false,
            });
        }
        let addr = addr & 0xff_ffff;
        self.cycle(bus, Access::Write, fc, addr, size);
        bus.write(addr, size, v);
        Ok(())
    }

    /// Data read of a byte or word.
    pub(crate) fn read(&mut self, bus: &mut dyn TimedBus68, addr: u32, size: Size) -> Res<u32> {
        let fc = self.data_fc();
        self.read_at(bus, fc, addr, size)
    }

    pub(crate) fn write(&mut self, bus: &mut dyn TimedBus68, addr: u32, size: Size, v: u32) -> Res {
        let fc = self.data_fc();
        self.write_at(bus, fc, addr, size, v)
    }

    /// Long read: high word, then low word.
    pub(crate) fn read_long(&mut self, bus: &mut dyn TimedBus68, fc: u8, addr: u32) -> Res<u32> {
        let hi = self.read_at(bus, fc, addr, Size::Word)?;
        let lo = self.read_at(bus, fc, addr.wrapping_add(2), Size::Word)?;
        Ok(hi << 16 | lo)
    }

    /// TAS's indivisible read-modify-write of a byte: returns the old value
    /// and writes `f(old)`.
    pub(crate) fn read_modify_write(
        &mut self,
        bus: &mut dyn TimedBus68,
        addr: u32,
        f: impl FnOnce(u8) -> u8,
    ) -> Res<u8> {
        let addr = addr & 0xff_ffff;
        let fc = self.data_fc();
        self.cycle(bus, Access::ReadModifyWrite, fc, addr, Size::Byte);
        let old = bus.read(addr, Size::Byte) as u8;
        bus.write(addr, Size::Byte, u32::from(f(old)));
        Ok(old)
    }

    /// Program-space word at `addr`.
    fn read_program(&mut self, bus: &mut dyn TimedBus68, addr: u32) -> Res<u16> {
        let fc = self.program_fc();
        Ok(self.read_at(bus, fc, addr, Size::Word)? as u16)
    }

    /// Take the extension word in IRC and refill IRC.
    pub(crate) fn ext(&mut self, bus: &mut dyn TimedBus68) -> Res<u16> {
        let w = self.irc;
        self.fill_irc(bus)?;
        Ok(w)
    }

    /// Two extension words as a long.
    pub(crate) fn ext_long(&mut self, bus: &mut dyn TimedBus68) -> Res<u32> {
        let hi = u32::from(self.ext(bus)?);
        let lo = u32::from(self.ext(bus)?);
        Ok(hi << 16 | lo)
    }

    /// Read the next program word into IRC.
    pub(crate) fn fill_irc(&mut self, bus: &mut dyn TimedBus68) -> Res {
        self.irc = self.read_program(bus, self.au)?;
        self.au = self.au.wrapping_add(2);
        Ok(())
    }

    /// The end-of-instruction prefetch: IRC moves to IRD, IRC is refilled.
    pub(crate) fn prefetch(&mut self, bus: &mut dyn TimedBus68) -> Res {
        self.ird = self.irc;
        self.fill_irc(bus)
    }

    /// Jump to `pc`: the first word is fetched into IRC (an odd target is an
    /// address error); the closing [`Self::prefetch`] then completes the
    /// queue.
    pub(crate) fn jump(&mut self, bus: &mut dyn TimedBus68, pc: u32) -> Res {
        self.au = pc;
        self.fill_irc(bus)
    }

    /// Address of the extension words being read (PC-relative modes).
    pub(crate) fn ext_pc(&self) -> u32 {
        self.au.wrapping_sub(2)
    }

    pub(crate) fn irc(&self) -> u16 {
        self.irc
    }

    pub(crate) fn au(&self) -> u32 {
        self.au
    }

    pub(crate) fn set_au(&mut self, v: u32) {
        self.au = v;
    }

    pub(crate) fn set_stopped(&mut self) {
        self.stopped = true;
    }

    // ------------------------------------------------------------ flags

    pub(crate) fn flag(&self, f: u16) -> bool {
        self.sr & f != 0
    }

    pub(crate) fn set_flag(&mut self, f: u16, on: bool) {
        if on {
            self.sr |= f;
        } else {
            self.sr &= !f;
        }
    }

    /// Set CCR (low byte of SR).
    pub(crate) fn set_ccr(&mut self, v: u16) {
        self.sr = (self.sr & 0xff00) | (v & 0x1f);
    }

    /// Condition code test (Bcc, DBcc, Scc).
    pub(crate) fn condition(&self, cc: u16) -> bool {
        let (n, z, v, c) = (
            self.flag(sr::N),
            self.flag(sr::Z),
            self.flag(sr::V),
            self.flag(sr::C),
        );
        match cc & 15 {
            0 => true,
            1 => false,
            2 => !c && !z,
            3 => c || z,
            4 => !c,
            5 => c,
            6 => !z,
            7 => z,
            8 => !v,
            9 => v,
            10 => !n,
            11 => n,
            12 => n == v,
            13 => n != v,
            14 => !z && n == v,
            _ => z || n != v,
        }
    }

    // ------------------------------------------------------------ exceptions

    /// Group 1/2 exception: supervisor mode, stack `pc` and the old SR in
    /// the order `frame` gives, take the vector, prefetch.
    pub(crate) fn exception(
        &mut self,
        bus: &mut dyn TimedBus68,
        vec: u8,
        pc: u32,
        frame: Frame,
    ) -> Res {
        let old_sr = self.sr;
        self.set_sr((self.sr | sr::S) & !sr::T);
        let sp = self.a[7];
        let (lo, hi, srv) = (pc & 0xffff, pc >> 16, u32::from(old_sr));
        let slots = match frame {
            // PC low, SR, PC high.
            Frame::LowSrHigh => [(2, lo), (6, srv), (4, hi)],
            // PC low, PC high, SR.
            Frame::LowHighSr => [(2, lo), (4, hi), (6, srv)],
        };
        for (off, v) in slots {
            self.write(bus, sp.wrapping_sub(off), Size::Word, v)?;
        }
        self.a[7] = sp.wrapping_sub(6);
        self.vector_jump(bus, vec)
    }

    /// Read exception vector `vec` and start there.
    fn vector_jump(&mut self, bus: &mut dyn TimedBus68, vec: u8) -> Res {
        let addr = u32::from(vec) * 4;
        let pc = self.read_long(bus, fc::SUPERVISOR_DATA, addr)?;
        self.jump(bus, pc)?;
        self.idle(bus, 2);
        self.prefetch(bus)
    }

    fn interrupt(&mut self, bus: &mut dyn TimedBus68, level: u8) -> Res {
        let old_sr = self.sr;
        self.set_sr((self.sr | sr::S) & !sr::T & !sr::IPL | (u16::from(level) << 8));
        self.idle(bus, 6);
        let pc = self.au.wrapping_sub(4);
        let sp = self.a[7];
        self.write(bus, sp.wrapping_sub(2), Size::Word, pc & 0xffff)?;
        self.idle(bus, 4);
        // Autovector acknowledge (VPA): the cycle is stretched to the E clock.
        self.cycle(
            bus,
            Access::IntAck,
            fc::INT_ACK,
            0xff_fff0 | u32::from(level) << 1,
            Size::Byte,
        );
        self.idle(bus, 4);
        self.write(bus, sp.wrapping_sub(6), Size::Word, u32::from(old_sr))?;
        self.write(bus, sp.wrapping_sub(4), Size::Word, pc >> 16)?;
        self.a[7] = sp.wrapping_sub(6);
        self.vector_jump(bus, vector::SPURIOUS + level)
    }

    /// An address error during an instruction: the 7-word group 0 frame,
    /// or a halt (double bus fault) if that fails too.
    fn fault(&mut self, bus: &mut dyn TimedBus68, f: Fault) {
        let Fault::AddressError {
            addr,
            write,
            program,
        } = f;
        let old_sr = self.sr;
        self.set_sr((self.sr | sr::S) & !sr::T);
        self.idle(bus, 4);
        // Status word: R/W (1 = read), I/N (1 = not an instruction), FC.
        let fcode = u16::from(if program {
            if old_sr & sr::S != 0 {
                fc::SUPERVISOR_PROGRAM
            } else {
                fc::USER_PROGRAM
            }
        } else if old_sr & sr::S != 0 {
            fc::SUPERVISOR_DATA
        } else {
            fc::USER_DATA
        });
        // Bit 3 (I/N) is set for program fetches, as the Harte data shows.
        let status = (self.ir & 0xffe0) | u16::from(!write) << 4 | u16::from(program) << 3 | fcode;
        let pc = self.au.wrapping_sub(4);
        let sp = self.a[7];
        let r: Res = (|| {
            self.write(bus, sp.wrapping_sub(2), Size::Word, pc & 0xffff)?;
            self.write(bus, sp.wrapping_sub(6), Size::Word, u32::from(old_sr))?;
            self.write(bus, sp.wrapping_sub(4), Size::Word, pc >> 16)?;
            self.write(bus, sp.wrapping_sub(8), Size::Word, u32::from(self.ir))?;
            self.write(bus, sp.wrapping_sub(10), Size::Word, addr & 0xffff)?;
            self.write(bus, sp.wrapping_sub(14), Size::Word, u32::from(status))?;
            self.write(bus, sp.wrapping_sub(12), Size::Word, addr >> 16)?;
            self.a[7] = sp.wrapping_sub(14);
            self.vector_jump(bus, vector::ADDRESS_ERROR)
        })();
        if r.is_err() {
            self.halted = true;
        }
    }

    fn regs_list(&self) -> RegisterSet {
        let mut v = Vec::with_capacity(20);
        const D: [&str; 8] = ["D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7"];
        const A: [&str; 8] = ["A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7"];
        for (i, name) in D.iter().enumerate() {
            v.push(Register {
                name,
                value: self.d[i],
                bits: 32,
            });
        }
        for (i, name) in A.iter().enumerate() {
            v.push(Register {
                name,
                value: self.a[i],
                bits: 32,
            });
        }
        v.push(Register {
            name: "PC",
            value: self.pc(),
            bits: 24,
        });
        v.push(Register {
            name: "SR",
            value: u32::from(self.sr),
            bits: 16,
        });
        v.push(Register {
            name: "USP",
            value: self.usp(),
            bits: 32,
        });
        v.push(Register {
            name: "SSP",
            value: self.ssp(),
            bits: 32,
        });
        RegisterSet(v)
    }
}

/// The order in which a group 1/2 exception stacks PC and SR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// TRAP, illegal instruction, privilege violation, CHK, division by
    /// zero.
    LowSrHigh,
    /// Trace, TRAPV.
    LowHighSr,
}

impl Cpu for M68000 {
    /// One instruction on a bus without wait states (the step's clocks are
    /// not modelled cycle by cycle; machines use [`M68000::step_timed`]).
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        match self.step_timed(&mut NoWait(bus)) {
            Step::Halted => CycleResult::Jammed,
            Step::Stopped { .. } => CycleResult::Busy,
            Step::Done { .. } => CycleResult::InstrDone { next_pc: self.pc() },
        }
    }

    fn regs(&self) -> RegisterSet {
        self.regs_list()
    }

    fn set_reg(&mut self, name: &str, v: u32) {
        let name = name.to_ascii_uppercase();
        let idx = |p: &str| name.strip_prefix(p).and_then(|n| n.parse::<usize>().ok());
        match name.as_str() {
            "PC" => {
                // The queue must be refilled from the new address by the
                // caller's next step; keep it consistent for peeking.
                self.au = (v & 0xff_fffe).wrapping_add(4);
            }
            "SR" => self.set_sr(v as u16),
            "USP" => self.set_usp(v),
            "SSP" => self.set_ssp(v),
            _ => {
                if let Some(i) = idx("D").filter(|&i| i < 8) {
                    self.d[i] = v;
                } else if let Some(i) = idx("A").filter(|&i| i < 8) {
                    self.a[i] = v;
                }
            }
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

impl M68000 {
    /// After a debugger PC change: refill the queue from the new PC
    /// (peeking, without bus cycles).
    pub fn refill_queue(&mut self, bus: &dyn Bus) {
        let pc = self.pc();
        self.ird = bus.peek(pc, Size::Word) as u16;
        self.irc = bus.peek(pc.wrapping_add(2) & 0xff_ffff, Size::Word) as u16;
    }
}

const TAG: [u8; 4] = *b"M68K";
const VERSION: u16 = 1;

impl Savestate for M68000 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for v in self.d.iter().chain(&self.a) {
                w.u32(*v);
            }
            w.u32(self.other_sp);
            w.u16(self.sr);
            w.u16(self.ird);
            w.u16(self.irc);
            w.u32(self.au);
            w.u8(self.ipl);
            for b in [self.nmi_seen, self.stopped, self.halted] {
                w.bool(b);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        for v in n.d.iter_mut().chain(n.a.iter_mut()) {
            *v = c.u32()?;
        }
        n.other_sp = c.u32()?;
        n.sr = c.u16()? & sr::MASK;
        n.ird = c.u16()?;
        n.irc = c.u16()?;
        n.au = c.u32()?;
        n.ipl = c.u8()? & 7;
        n.nmi_seen = c.bool()?;
        n.stopped = c.bool()?;
        n.halted = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 KB of RAM without wait states.
    struct Ram(Vec<u8>);

    impl Bus for Ram {
        fn read(&mut self, addr: u32, size: Size) -> u32 {
            self.peek(addr, size)
        }
        fn peek(&self, addr: u32, size: Size) -> u32 {
            let b = |a: u32| u32::from(self.0[(a & 0xffff) as usize]);
            match size {
                Size::Byte => b(addr),
                _ => b(addr) << 8 | b(addr + 1),
            }
        }
        fn write(&mut self, addr: u32, size: Size, v: u32) {
            let a = (addr & 0xffff) as usize;
            match size {
                Size::Byte => self.0[a] = v as u8,
                _ => {
                    self.0[a] = (v >> 8) as u8;
                    self.0[(a + 1) & 0xffff] = v as u8;
                }
            }
        }
        fn addr_bits(&self) -> u8 {
            24
        }
    }

    /// Reset vectors (SSP $8000, PC $400) and a counting loop at $400:
    /// MOVEQ #0,D0; loop: ADDQ.L #1,D0; MOVE.L D0,-(A7); BRA.S loop.
    fn machine() -> Ram {
        let mut ram = Ram(vec![0; 0x10000]);
        let words: [(u32, u16); 9] = [
            (0, 0),
            (2, 0x8000),
            (4, 0),
            (6, 0x0400),
            (0x400, 0x7000),
            (0x402, 0x5280),
            (0x404, 0x2f00),
            (0x406, 0x60fa),
            (0x408, 0x4e71),
        ];
        for (a, w) in words {
            ram.write(a, Size::Word, u32::from(w));
        }
        ram
    }

    #[test]
    fn savestate_round_trip() {
        let mut bus = machine();
        let mut cpu = M68000::new();
        cpu.reset_timed(&mut NoWait(&mut bus));
        for _ in 0..7 {
            cpu.tick(&mut bus);
        }
        let mut w = StateWriter::new();
        cpu.save(&mut w);
        let data = w.into_bytes();
        let mut other = M68000::new();
        other.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(other.regs(), cpu.regs());
        let mut bus2 = Ram(bus.0.clone());
        for _ in 0..20 {
            cpu.tick(&mut bus);
            other.tick(&mut bus2);
        }
        assert_eq!(other.regs(), cpu.regs());
        assert_eq!(bus2.0, bus.0);
        assert!(other
            .load(&mut StateReader::new(&data[..data.len() - 1]))
            .is_err());
    }
}
