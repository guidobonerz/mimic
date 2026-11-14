//! BASIC interpreter debugging: the program in a machine's BASIC memory,
//! where the interpreter is in it, its variables and its GOSUB/FOR stack,
//! so a debugger can list, break and step by BASIC lines and statements
//! without knowing the dialect (golden rule 1). Machines with a BASIC offer
//! it through [`crate::Machine::basic`].
//!
//! Everything is read with side-effect-free reads (golden rule 2). The
//! debugger never runs the interpreter itself: it asks the machine for
//! [`BasicStopPoint`]s, ordinary breakpoints at the places where the
//! interpreter begins a statement or a line, arms them only while a BASIC
//! breakpoint or step is active (golden rule 5), and decides at each hit
//! from [`BasicDebug::state`] whether to stop.

use crate::debug::BreakpointKind;
use crate::machine::MemSpaceId;

/// What the interpreter is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasicMode {
    /// No interpreter to look at: not started yet, or replaced by a
    /// machine-code program or a cartridge.
    Inactive,
    /// Waiting for or executing a line typed in direct mode.
    Direct,
    /// Running the stored program.
    Running,
}

impl BasicMode {
    pub fn name(self) -> &'static str {
        match self {
            BasicMode::Inactive => "inactive",
            BasicMode::Direct => "direct",
            BasicMode::Running => "running",
        }
    }
}

/// A place in the program: a line and the statement in it (0 = the first;
/// always 0 for dialects that only know lines).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BasicPos {
    pub line: u32,
    pub statement: u16,
}

/// How finely the machine's stop points resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasicGranularity {
    /// Every statement.
    Statement,
    /// Only the first statement of a line.
    Line,
}

impl BasicGranularity {
    pub fn name(self) -> &'static str {
        match self {
            BasicGranularity::Statement => "statement",
            BasicGranularity::Line => "line",
        }
    }
}

/// The interpreter a machine has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicInfo {
    /// Naming the interpreter as its machine does ("Commodore BASIC V2").
    pub dialect: String,
    /// The finest of the machine's stop points.
    pub granularity: BasicGranularity,
    /// The memory space the addresses of lines and variables are in.
    pub space: MemSpaceId,
}

/// Where the interpreter is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicState {
    pub mode: BasicMode,
    /// The statement being executed (at a stop point: about to be).
    /// `None` outside a stored line.
    pub pos: Option<BasicPos>,
    /// The interpreter's text pointer, where it has one.
    pub text_addr: Option<u32>,
    /// Open GOSUBs: what step over and step out compare.
    pub depth: u32,
}

/// One stored program line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicLine {
    pub number: u32,
    /// Address of the line in [`BasicInfo::space`].
    pub addr: u32,
    /// The line as stored (tokens, link and number fields included).
    pub raw: Vec<u8>,
    /// The line as `LIST` shows it, without its number, in Unicode.
    pub text: String,
    /// Where each statement begins in `text` (byte offsets; the first is 0).
    pub statements: Vec<u32>,
}

/// A value of a variable.
#[derive(Clone, Debug, PartialEq)]
pub enum BasicValue {
    Int(i64),
    Float(f64),
    /// In Unicode; the machine converts from and to its character set.
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasicType {
    Int,
    Float,
    Str,
}

impl BasicType {
    pub fn name(self) -> &'static str {
        match self {
            BasicType::Int => "int",
            BasicType::Float => "float",
            BasicType::Str => "string",
        }
    }
}

/// A variable or an array.
#[derive(Clone, Debug, PartialEq)]
pub struct BasicVar {
    /// As written in the program, with its type sign ("A", "N%", "S$").
    pub name: String,
    pub ty: BasicType,
    /// Where the variable's entry is in [`BasicInfo::space`].
    pub addr: u32,
    /// The value; `None` for an array (see [`BasicDebug::array`]).
    pub value: Option<BasicValue>,
    /// An array's sizes per dimension (elements, not the highest index);
    /// empty for a plain variable.
    pub dims: Vec<u32>,
}

/// One entry of the interpreter's stack, innermost first.
#[derive(Clone, Debug, PartialEq)]
pub enum BasicFrame {
    Gosub {
        /// The GOSUB statement; RETURN continues after it.
        from: BasicPos,
    },
    For {
        var: String,
        limit: BasicValue,
        step: BasicValue,
        /// The FOR statement; NEXT loops to the statement after it.
        from: BasicPos,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasicError {
    /// The interpreter is not there ([`BasicMode::Inactive`]).
    Inactive,
    NoSuchVariable,
    /// A string for a number or the other way round.
    TypeMismatch,
    /// The value does not fit the variable, or the index the array.
    OutOfRange,
    /// The dialect cannot do this (e.g. lengthen a string in place).
    Unsupported,
}

impl std::fmt::Display for BasicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BasicError::Inactive => "BASIC is not active",
            BasicError::NoSuchVariable => "no such variable",
            BasicError::TypeMismatch => "type mismatch",
            BasicError::OutOfRange => "out of range",
            BasicError::Unsupported => "not supported by this BASIC",
        })
    }
}

impl std::error::Error for BasicError {}

/// A place where the interpreter begins a statement or a line: an exec
/// breakpoint in its dispatch loop, or a watchpoint on its "current line"
/// variable. Contract: when the breakpoint stops the machine,
/// [`BasicDebug::state`] already reports the statement about to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicStopPoint {
    pub kind: BreakpointKind,
    pub granularity: BasicGranularity,
}

/// Debugger access to a machine's BASIC interpreter. The reading methods
/// answer with what memory holds, also while the machine is halted in the
/// middle of a statement; with [`BasicMode::Inactive`] they return nothing.
pub trait BasicDebug {
    fn info(&self) -> BasicInfo;

    fn state(&self) -> BasicState;

    /// The stored program in line order.
    fn program(&self) -> Vec<BasicLine>;

    /// Plain variables with their values and arrays with their sizes, in
    /// the interpreter's order.
    fn variables(&self) -> Vec<BasicVar>;

    /// `count` elements of array `name` from element `first`, counted
    /// through all dimensions in storage order; fewer at the array's end.
    fn array(&self, name: &str, first: u32, count: u32) -> Result<Vec<BasicValue>, BasicError>;

    /// The GOSUB/FOR stack, innermost first.
    fn stack(&self) -> Vec<BasicFrame>;

    /// Write a variable (`index: None`) or an array element. Integers are
    /// accepted for float variables and whole floats for integer ones.
    fn set_variable(
        &mut self,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<(), BasicError>;

    /// Where the debugger has to stop the machine to follow the program.
    /// May change (ROM banking, another interpreter): asked for again
    /// before every run with BASIC breakpoints or a BASIC step.
    fn stop_points(&self) -> Vec<BasicStopPoint>;
}
