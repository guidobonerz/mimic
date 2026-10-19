//! Core traits (Cpu, Bus, Machine), scheduler, savestates, DebugHooks, FrameRef.
//!
//! Everything the debugger, preview and frontends need lives here, so they
//! never depend on a concrete machine (golden rule 1).

#![forbid(unsafe_code)]

pub mod bus;
pub mod cpu;
pub mod cycle;
pub mod debug;
pub mod frame;
pub mod freeze;
pub mod input;
pub mod machine;
pub mod media;
pub mod port;
pub mod registry;
pub mod savestate;
pub mod testing;

pub use bus::{Bus, Size};
pub use cpu::{Cpu, CycleResult, Instr, Register, RegisterSet};
pub use cycle::Cycle;
pub use debug::{BpId, Breakpoint, BreakpointKind, DebugHooks, FreezeId};
pub use frame::FrameRef;
pub use freeze::{Freeze, FreezeMode};
pub use machine::{ChipInfo, DebugTarget, DeviceId, Machine, MemSpace, MemSpaceId, StopReason};
pub use savestate::{Savestate, StateError, StateReader, StateWriter};
