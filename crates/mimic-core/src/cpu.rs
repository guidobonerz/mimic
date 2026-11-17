//! Processor abstraction.

use std::fmt;

use crate::bus::Bus;

/// Outcome of a single CPU cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleResult {
    /// The cycle was part of an instruction that is still in progress.
    Busy,
    /// This cycle finished an instruction. `next_pc` is the address of the
    /// next opcode fetch; machines check exec breakpoints against it.
    InstrDone { next_pc: u32 },
    /// The CPU has halted itself (6502 JAM, 68000 double bus fault) and keeps
    /// returning this until reset.
    Jammed,
}

/// One named register value, as shown by the debugger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Register {
    pub name: &'static str,
    pub value: u32,
    /// Register width in bits.
    pub bits: u8,
}

/// Ordered register dump (order is stable for deterministic output).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegisterSet(pub Vec<Register>);

impl RegisterSet {
    /// Value of the register with the given name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<u32> {
        self.0
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .map(|r| r.value)
    }
}

/// How an instruction changes the flow of control. Lets the debugger do
/// `step_over` and `step_out` without knowing the CPU (golden rule 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flow {
    /// Continues with the next instruction.
    #[default]
    Normal,
    /// Subroutine call that returns to the next instruction (JSR, CALL, BSR).
    Call,
    /// Return from a subroutine or interrupt (RTS, RTI, RET).
    Return,
    /// Unconditional jump.
    Jump,
    /// Conditional branch.
    Branch,
}

/// A disassembled instruction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instr {
    pub addr: u32,
    pub bytes: Vec<u8>,
    pub mnemonic: String,
    pub operand: String,
    /// Branch/jump target, if statically known.
    pub target: Option<u32>,
    pub flow: Flow,
}

impl fmt::Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.operand.is_empty() {
            f.write_str(&self.mnemonic)
        } else {
            write!(f, "{} {}", self.mnemonic, self.operand)
        }
    }
}

/// A processor core, advanced one bus cycle at a time (golden rule 4).
pub trait Cpu {
    /// Execute exactly one cycle.
    fn tick(&mut self, bus: &mut dyn Bus) -> CycleResult;

    /// Register dump: generic name -> value.
    fn regs(&self) -> RegisterSet;

    /// Set a register by name. Unknown names are ignored.
    fn set_reg(&mut self, name: &str, v: u32);

    /// Disassemble the instruction at `addr`, using `Bus::peek` only.
    fn disassemble(&self, bus: &dyn Bus, addr: u32) -> Instr;
}
