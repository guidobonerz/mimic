//! Core traits (Cpu, Bus, Machine), scheduler, savestates, DebugHooks, FrameRef.
//!
//! Everything the debugger, preview and frontends need lives here, so they
//! never depend on a concrete machine (golden rule 1).

#![forbid(unsafe_code)]

pub mod audio;
pub mod beam;
pub mod bus;
pub mod controls;
pub mod cpu;
pub mod cycle;
pub mod debug;
pub mod frame;
pub mod freeze;
pub mod gfx;
pub mod hooked;
pub mod host;
pub mod input;
pub mod machine;
pub mod media;
pub mod overlay;
pub mod port;
pub mod registry;
pub mod rewind;
pub mod savestate;
pub mod scheduler;
pub mod testing;
pub mod vector;

pub use audio::{AudioFormat, Decimator, SAMPLE_RATE};
pub use beam::{BeamPos, TraceConfig, TraceEntry, TraceEvent, TraceRange};
pub use bus::{Bus, Size};
pub use controls::{ControlId, ControlInfo, ControlKind};
pub use cpu::{Cpu, CycleResult, Flow, Instr, Register, RegisterSet};
pub use cycle::Cycle;
pub use debug::{BpId, Breakpoint, BreakpointKind, DebugHooks, FreezeId, InterruptKind};
pub use frame::{FrameRef, ScreenId, ScreenInfo};
pub use freeze::{Freeze, FreezeMode};
pub use gfx::{GfxKind, GfxOptions, GfxView, GfxViewInfo};
pub use hooked::HookedBus;
pub use machine::{
    ChipInfo, DebugTarget, DeviceId, Machine, MemSpace, MemSpaceId, ResetKind, StopReason,
};
pub use savestate::{Savestate, StateError, StateReader, StateWriter};
pub use scheduler::{ClockDivider, Event, EventId, Scheduler};
pub use vector::{Segment, VectorFrame};
