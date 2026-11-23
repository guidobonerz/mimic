//! RISC-V RV32IM + Zicsr in machine mode: the "rvlite" soft CPU in the FPGA
//! of the Ultimate 64 Elite II board, which runs the board's firmware next
//! to the C64.
//!
//! Thirty-two 32-bit registers (x0 reads 0), a program counter and a handful
//! of machine-mode CSRs. No compressed instructions, no A, F or D extension,
//! no user or supervisor mode, no memory protection, no bus errors.
//!
//! **Instructions.** RV32I (FENCE and FENCE.I do nothing), the M extension,
//! Zicsr, and of the privileged instructions ECALL, EBREAK, MRET and WFI.
//! rvlite has no divider and decodes DIV/DIVU/REM/REMU as multiplies; the
//! firmware is built without them. Here they divide as the specification
//! says (a superset, as the multiply answers are of no use to anyone).
//!
//! **Memory.** Accesses go to the bus at the exact address; a 16- or 32-bit
//! access at an odd address does not trap, the bus serves it as little-endian
//! bytes from that address. (rvlite itself only does aligned accesses right;
//! the firmware's unaligned 32-bit stores have to work.) Jump targets lose
//! their bits 1–0: there is no misaligned-fetch trap either.
//!
//! **Traps.** Direct mode only: `mepc` = the PC of the trapping instruction
//! (also for ECALL) or of the instruction an interrupt came in front of,
//! `mcause` = the cause, MPIE = MIE, MIE = 0, PC = `mtvec`. MRET: MIE = MPIE,
//! MPIE = 1, PC = `mepc`. Causes: 2 illegal instruction, 3 EBREAK, 11 ECALL,
//! $8000000B the external interrupt.
//!
//! **Interrupt.** One level-sensitive line ([`Rv32::set_irq`]), visible as
//! `mip.MEIP`, taken in front of an instruction while `mie.MEIE` and
//! `mstatus.MIE` are set. No timer or software interrupt (no CLINT).
//!
//! **CSRs** as the guest sees them, by [`Model`]:
//!
//! | CSR | Address | rvlite | standard |
//! | --- | --- | --- | --- |
//! | `mstatus` | $300 | bit 3 MIE, bit 7 MPIE; the rest reads 0 | MPP (bits 12–11) reads 11 as well |
//! | `misa` | $301 | $40000100 (read only, no M bit) | $40001100 |
//! | `mie` | $304 | bit 11 MEIE | the same |
//! | `mtvec` | $305 | writes lose bits 1–0 (direct mode) | the same |
//! | `mscratch` | $340 | read/write | the same |
//! | `mepc` | $341 | read/write | writes lose bits 1–0 |
//! | `mcause` | $342 | set by traps, writes ignored | read/write |
//! | `mtval` | $343 | reads 0 | the instruction word of an illegal instruction, read/write |
//! | `mip` | $344 | bit 11 = the interrupt line, writes ignored | the same |
//! | `mcycle`, `minstret` (+`h`) | $B00, $B02, $B80, $B82 | read 0 | the 64-bit instruction counter, read/write |
//! | `marchid`, `mimpid` | $F12, $F13 | $13, $475A0001 | 0 |
//! | all others | | read 0, writes ignored | the same |
//!
//! No CSR access ever traps. The rvlite model also keeps two of the core's
//! oddities: the 5-bit immediate of CSRRWI/CSRRSI/CSRRCI is sign-extended
//! (the firmware only uses values below 16), and an unknown SYSTEM
//! instruction with funct3 = 0 does nothing. The standard model zero-extends
//! and traps.
//!
//! **Timing.** [`Rv32::step`] runs one instruction, or the entry into the
//! interrupt handler. There is no cycle model below that: the machine decides
//! how many instructions make up a stretch of its time. [`Cpu::tick`] is one
//! such step as well.
//!
//! **Speed.** [`Rv32::step`] and [`Rv32::run`] are generic over [`Rv32Bus`],
//! so a machine's memory accesses inline into the interpreter;
//! [`Cpu::tick`] runs the same core through an adapter over `dyn Bus`.
//!
//! Sources: The RISC-V Instruction Set Manual, Volume I: Unprivileged ISA
//! (RV32I, M, Zicsr chapters: encodings, immediates, division results, the
//! CSR instructions' read and write rules, ABI register names) and Volume
//! II: Privileged Architecture (machine-mode CSR addresses, `mstatus`,
//! `mtvec`, `mcause` codes, trap entry and MRET, WFI). The rvlite behaviour
//! from the documentation of the UE2-C64U-Emulator project
//! (github.com/Jondalar/UE2-C64U-Emulator, GPL): `docs/hw/01-cpu-boot-memory.md`
//! and `docs/specs/S01-cpu-rv32.md`, read for facts only (the instruction
//! subset, the CSR table, trap and interrupt rules, what the firmware relies
//! on); its code was not opened. The implementation is this project's own.

#![forbid(unsafe_code)]

mod disasm;
mod exec;
#[cfg(test)]
mod tests;

use mimic_core::{
    Bus, Cpu, CycleResult, Instr, Register, RegisterSet, Savestate, Size, StateError, StateReader,
    StateWriter,
};

/// `mstatus` bit 3: machine interrupts enabled.
pub const MSTATUS_MIE: u32 = 1 << 3;
/// `mstatus` bit 7: MIE as it was before the last trap.
pub const MSTATUS_MPIE: u32 = 1 << 7;
/// `mstatus` bits 12–11: the previous privilege mode, always machine (11).
pub const MSTATUS_MPP: u32 = 3 << 11;
/// `mie` bit 11: machine external interrupt enabled.
pub const MIE_MEIE: u32 = 1 << 11;
/// `mip` bit 11: the external interrupt line.
pub const MIP_MEIP: u32 = 1 << 11;

/// `mcause` of an illegal instruction.
pub const CAUSE_ILLEGAL: u32 = 2;
/// `mcause` of EBREAK.
pub const CAUSE_BREAKPOINT: u32 = 3;
/// `mcause` of ECALL from machine mode.
pub const CAUSE_ECALL_M: u32 = 11;
/// `mcause` of the machine external interrupt.
pub const CAUSE_EXTERNAL_IRQ: u32 = 0x8000_000b;

pub const CSR_MSTATUS: u16 = 0x300;
pub const CSR_MISA: u16 = 0x301;
pub const CSR_MIE: u16 = 0x304;
pub const CSR_MTVEC: u16 = 0x305;
pub const CSR_MSCRATCH: u16 = 0x340;
pub const CSR_MEPC: u16 = 0x341;
pub const CSR_MCAUSE: u16 = 0x342;
pub const CSR_MTVAL: u16 = 0x343;
pub const CSR_MIP: u16 = 0x344;
pub const CSR_MCYCLE: u16 = 0xb00;
pub const CSR_MINSTRET: u16 = 0xb02;
pub const CSR_MCYCLEH: u16 = 0xb80;
pub const CSR_MINSTRETH: u16 = 0xb82;
pub const CSR_MARCHID: u16 = 0xf12;
pub const CSR_MIMPID: u16 = 0xf13;

/// The ABI names of x0–x31.
pub const ABI_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

/// The memory interface of the fast path. Addresses are the full 32 bits.
/// 16- and 32-bit accesses may come at any address and mean little-endian
/// bytes from there on; nothing can fail.
pub trait Rv32Bus {
    /// The instruction word at `addr` (bits 1–0 of `addr` are always 0).
    fn fetch(&mut self, addr: u32) -> u32;
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, v: u8);
    fn write16(&mut self, addr: u32, v: u16);
    fn write32(&mut self, addr: u32, v: u32);
}

/// What [`Rv32::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// An instruction ran.
    Done,
    /// WFI ran (as a no-op): until the interrupt line or the enables change,
    /// the program only waits, and a machine may skip that time.
    Wfi,
    /// No instruction ran: the external interrupt was taken, PC is at
    /// `mtvec`.
    Interrupt,
    /// ECALL trapped (cause 11).
    Ecall,
    /// EBREAK trapped (cause 3).
    Ebreak,
    /// This instruction word trapped as illegal (cause 2).
    Illegal(u32),
}

/// Which core's CSR behaviour the guest sees (see the table in the crate
/// documentation). Configuration, not part of the savestate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    /// The rvlite core of the Ultimate boards.
    #[default]
    Rvlite,
    /// A plain machine-mode RV32IM hart with counters and `mtval`.
    Standard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rv32 {
    model: Model,
    /// x0–x31; x0 is kept at 0.
    pub(crate) x: [u32; 32],
    /// The address of the instruction that runs next (bits 1–0 are 0).
    pub(crate) pc: u32,
    /// MIE and MPIE only.
    pub(crate) mstatus: u32,
    /// MEIE only.
    pub(crate) mie: u32,
    pub(crate) mtvec: u32,
    pub(crate) mepc: u32,
    pub(crate) mcause: u32,
    pub(crate) mtval: u32,
    pub(crate) mscratch: u32,
    /// Instructions run since reset (`mcycle` and `minstret` at once).
    pub(crate) instret: u64,
    /// The external interrupt line (`mip.MEIP`).
    pub(crate) meip: bool,
    /// The line, MEIE and MIE are all set: the next step traps. Derived.
    pub(crate) irq_ready: bool,
}

impl Default for Rv32 {
    fn default() -> Self {
        Self::new(Model::Rvlite)
    }
}

impl Rv32 {
    /// A core with everything at 0; [`Rv32::reset`] gives it its start
    /// address.
    pub fn new(model: Model) -> Self {
        Self {
            model,
            x: [0; 32],
            pc: 0,
            mstatus: 0,
            mie: 0,
            mtvec: 0,
            mepc: 0,
            mcause: 0,
            mtval: 0,
            mscratch: 0,
            instret: 0,
            meip: false,
            irq_ready: false,
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// Reset: execution starts at `pc` (rvlite on the board: $80000000) with
    /// interrupts off. The x registers keep their values, as on the core;
    /// the CSRs and the instruction counter start at 0 (`mtvec` is undefined
    /// after a real reset). The interrupt line stays as the machine drives
    /// it.
    pub fn reset(&mut self, pc: u32) {
        self.pc = pc & !3;
        self.mstatus = 0;
        self.mie = 0;
        self.mtvec = 0;
        self.mepc = 0;
        self.mcause = 0;
        self.mtval = 0;
        self.mscratch = 0;
        self.instret = 0;
        self.update_irq();
    }

    /// The external interrupt line (level): `mip.MEIP`.
    pub fn set_irq(&mut self, level: bool) {
        self.meip = level;
        self.update_irq();
    }

    pub fn irq(&self) -> bool {
        self.meip
    }

    /// The next [`Rv32::step`] enters the interrupt handler.
    pub fn irq_pending(&self) -> bool {
        self.irq_ready
    }

    /// The address of the instruction that runs next.
    pub fn pc(&self) -> u32 {
        self.pc
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc & !3;
    }

    /// Register x`n` (`n` counts modulo 32).
    pub fn x(&self, n: usize) -> u32 {
        self.x[n & 31]
    }

    /// Set register x`n` (`n` counts modulo 32); x0 stays 0.
    pub fn set_x(&mut self, n: usize, v: u32) {
        self.x[n & 31] = v;
        self.x[0] = 0;
    }

    /// x0–x31.
    pub fn xs(&self) -> &[u32; 32] {
        &self.x
    }

    /// `mstatus` as stored: MIE and MPIE.
    pub fn mstatus(&self) -> u32 {
        self.mstatus
    }

    /// `mie` as stored: MEIE.
    pub fn mie(&self) -> u32 {
        self.mie
    }

    /// `mip`: MEIP follows the interrupt line.
    pub fn mip(&self) -> u32 {
        if self.meip {
            MIP_MEIP
        } else {
            0
        }
    }

    pub fn mtvec(&self) -> u32 {
        self.mtvec
    }

    pub fn mepc(&self) -> u32 {
        self.mepc
    }

    /// The cause of the last trap.
    pub fn mcause(&self) -> u32 {
        self.mcause
    }

    /// The instruction word of the last illegal-instruction trap, 0 after
    /// any other trap. Kept for the host in both models; the guest of the
    /// rvlite model reads 0.
    pub fn mtval(&self) -> u32 {
        self.mtval
    }

    pub fn mscratch(&self) -> u32 {
        self.mscratch
    }

    /// Instructions run since reset (trapping ones and interrupt entries not
    /// counted): `mcycle` and `minstret` of the standard model.
    pub fn instructions(&self) -> u64 {
        self.instret
    }

    pub fn set_instructions(&mut self, n: u64) {
        self.instret = n;
    }

    /// A CSR as an instruction reads it. Never traps; unknown ones read 0.
    pub fn csr_read(&self, addr: u16) -> u32 {
        let standard = self.model == Model::Standard;
        match addr & 0xfff {
            CSR_MSTATUS if standard => self.mstatus | MSTATUS_MPP,
            CSR_MSTATUS => self.mstatus,
            CSR_MISA if standard => 0x4000_1100,
            CSR_MISA => 0x4000_0100,
            CSR_MIE => self.mie,
            CSR_MTVEC => self.mtvec,
            CSR_MSCRATCH => self.mscratch,
            CSR_MEPC => self.mepc,
            CSR_MCAUSE => self.mcause,
            CSR_MTVAL if standard => self.mtval,
            CSR_MIP => self.mip(),
            CSR_MCYCLE | CSR_MINSTRET if standard => self.instret as u32,
            CSR_MCYCLEH | CSR_MINSTRETH if standard => (self.instret >> 32) as u32,
            CSR_MARCHID if !standard => 0x13,
            CSR_MIMPID if !standard => 0x475a_0001,
            _ => 0,
        }
    }

    /// A CSR as an instruction writes it. Never traps; unknown and read-only
    /// ones ignore the write.
    pub fn csr_write(&mut self, addr: u16, v: u32) {
        let standard = self.model == Model::Standard;
        match addr & 0xfff {
            CSR_MSTATUS => {
                self.mstatus = v & (MSTATUS_MIE | MSTATUS_MPIE);
                self.update_irq();
            }
            CSR_MIE => {
                self.mie = v & MIE_MEIE;
                self.update_irq();
            }
            CSR_MTVEC => self.mtvec = v & !3,
            CSR_MSCRATCH => self.mscratch = v,
            CSR_MEPC if standard => self.mepc = v & !3,
            CSR_MEPC => self.mepc = v,
            CSR_MCAUSE if standard => self.mcause = v,
            CSR_MTVAL if standard => self.mtval = v,
            CSR_MCYCLE | CSR_MINSTRET if standard => {
                self.instret = (self.instret & !0xffff_ffff) | u64::from(v);
            }
            CSR_MCYCLEH | CSR_MINSTRETH if standard => {
                self.instret = (self.instret & 0xffff_ffff) | (u64::from(v) << 32);
            }
            _ => {}
        }
    }

    pub(crate) fn update_irq(&mut self) {
        self.irq_ready = self.meip && self.mie & MIE_MEIE != 0 && self.mstatus & MSTATUS_MIE != 0;
    }

    /// Trap entry: `mepc` = `pc`, MPIE = MIE, MIE = 0, PC = `mtvec`.
    pub(crate) fn trap(&mut self, cause: u32, pc: u32, tval: u32) {
        self.mepc = pc;
        self.mcause = cause;
        self.mtval = tval;
        let mpie = if self.mstatus & MSTATUS_MIE != 0 {
            MSTATUS_MPIE
        } else {
            0
        };
        self.mstatus = mpie;
        self.irq_ready = false;
        self.pc = self.mtvec;
    }
}

/// [`Rv32Bus`] over a plain [`Bus`], for [`Cpu::tick`]: everything as single
/// bytes, so the byte order does not depend on the bus.
struct PlainBus<'a>(&'a mut dyn Bus);

impl PlainBus<'_> {
    fn bytes<const N: usize>(&mut self, addr: u32) -> [u8; N] {
        let mut b = [0; N];
        for (i, v) in b.iter_mut().enumerate() {
            *v = self.0.read(addr.wrapping_add(i as u32), Size::Byte) as u8;
        }
        b
    }

    fn put(&mut self, addr: u32, bytes: &[u8]) {
        for (i, v) in bytes.iter().enumerate() {
            self.0
                .write(addr.wrapping_add(i as u32), Size::Byte, u32::from(*v));
        }
    }
}

impl Rv32Bus for PlainBus<'_> {
    fn fetch(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes(self.bytes(addr))
    }
    fn read8(&mut self, addr: u32) -> u8 {
        self.0.read(addr, Size::Byte) as u8
    }
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes(self.bytes(addr))
    }
    fn read32(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes(self.bytes(addr))
    }
    fn write8(&mut self, addr: u32, v: u8) {
        self.0.write(addr, Size::Byte, u32::from(v));
    }
    fn write16(&mut self, addr: u32, v: u16) {
        self.put(addr, &v.to_le_bytes());
    }
    fn write32(&mut self, addr: u32, v: u32) {
        self.put(addr, &v.to_le_bytes());
    }
}

/// The CSRs shown by [`Cpu::regs`], after PC and x1–x31.
const CSR_NAMES: [&str; 8] = [
    "mstatus", "mie", "mip", "mtvec", "mepc", "mcause", "mtval", "mscratch",
];

impl Cpu for Rv32 {
    /// One tick is one [`Rv32::step`]: a whole instruction or an interrupt
    /// entry. The core has no cycle model below that.
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult {
        self.step(&mut PlainBus(bus));
        CycleResult::InstrDone { next_pc: self.pc }
    }

    /// PC, x1–x31 under their ABI names, then the CSRs as stored.
    fn regs(&self) -> RegisterSet {
        let reg = |name, value| Register {
            name,
            value,
            bits: 32,
        };
        let mut list = Vec::with_capacity(40);
        list.push(reg("PC", self.pc));
        for (name, value) in ABI_NAMES.into_iter().zip(self.x).skip(1) {
            list.push(reg(name, value));
        }
        let csrs = [
            self.mstatus,
            self.mie,
            self.mip(),
            self.mtvec,
            self.mepc,
            self.mcause,
            self.mtval,
            self.mscratch,
        ];
        for (name, value) in CSR_NAMES.into_iter().zip(csrs) {
            list.push(reg(name, value));
        }
        RegisterSet(list)
    }

    /// `PC`, an ABI name (`ra`, `a0`, `fp` …), `x0`–`x31` or a CSR name;
    /// case does not matter. `mip` and `zero` cannot be set.
    fn set_reg(&mut self, name: &str, v: u32) {
        let lower = name.to_ascii_lowercase();
        if let Some(n) = ABI_NAMES.iter().position(|a| *a == lower) {
            self.set_x(n, v);
            return;
        }
        if let Some(n) = lower
            .strip_prefix('x')
            .and_then(|s| s.parse::<usize>().ok())
        {
            if n < 32 {
                self.set_x(n, v);
            }
            return;
        }
        match lower.as_str() {
            "pc" => self.set_pc(v),
            "fp" => self.set_x(8, v),
            "mstatus" => self.csr_write(CSR_MSTATUS, v),
            "mie" => self.csr_write(CSR_MIE, v),
            "mtvec" => self.csr_write(CSR_MTVEC, v),
            "mepc" => self.csr_write(CSR_MEPC, v),
            "mscratch" => self.csr_write(CSR_MSCRATCH, v),
            // The guest of the rvlite model cannot write these two; the
            // debugger can.
            "mcause" => self.mcause = v,
            "mtval" => self.mtval = v,
            _ => {}
        }
    }

    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr {
        disasm::disassemble(bus, addr)
    }
}

const STATE_TAG: [u8; 4] = *b"RV32";

impl Savestate for Rv32 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.u32(self.pc);
            for v in self.x {
                w.u32(v);
            }
            w.u32(self.mstatus);
            w.u32(self.mie);
            w.u32(self.mtvec);
            w.u32(self.mepc);
            w.u32(self.mcause);
            w.u32(self.mtval);
            w.u32(self.mscratch);
            w.u64(self.instret);
            w.bool(self.meip);
        });
    }

    /// Values a running core cannot hold are brought into range (PC and
    /// `mtvec` bits 1–0, x0, unused `mstatus` and `mie` bits); on an error
    /// the core is left as it was.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.pc = c.u32()? & !3;
        for v in &mut n.x {
            *v = c.u32()?;
        }
        n.x[0] = 0;
        n.mstatus = c.u32()? & (MSTATUS_MIE | MSTATUS_MPIE);
        n.mie = c.u32()? & MIE_MEIE;
        n.mtvec = c.u32()? & !3;
        n.mepc = c.u32()?;
        n.mcause = c.u32()?;
        n.mtval = c.u32()?;
        n.mscratch = c.u32()?;
        n.instret = c.u64()?;
        n.meip = c.bool()?;
        n.update_irq();
        *self = n;
        Ok(())
    }
}
