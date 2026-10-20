//! The machine abstraction every frontend, debugger and preview talks to.

use crate::audio::AudioFormat;
use crate::beam::BeamPos;
use crate::bus::Size;
use crate::cpu::{Instr, RegisterSet};
use crate::cycle::Cycle;
use crate::debug::{BpId, DebugHooks};
use crate::frame::FrameRef;
use crate::input::TimedInput;
use crate::media::{MediaSlot, MediaSlotInfo, SlotId};
use crate::port::{PortDevice, PortError, PortId, PortInfo};
use crate::savestate::StateError;

/// Identifies an address space of a machine (CPU view, chip RAM, drive RAM…).
/// Small and dense: it indexes the per-space breakpoint bitmaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemSpaceId(pub u16);

/// A debug target named after the hardware: `"cpu"`, `"drive8"`, `"z80"`,
/// `"copper"`, `"blitter"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub &'static str);

/// Description of an address space for debugger memory views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemSpace {
    pub id: MemSpaceId,
    pub name: &'static str,
    /// The processor or chip this space belongs to.
    pub device: DeviceId,
    pub addr_bits: u8,
    pub big_endian: bool,
}

/// One chip register for debugger views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChipReg {
    pub name: &'static str,
    /// Bus address, if the register is memory-mapped.
    pub addr: Option<u32>,
    pub value: u32,
    pub bits: u8,
}

/// Register dump of one chip (`vic2`, `cia1`, `agnus`…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipInfo {
    pub name: &'static str,
    pub regs: Vec<ChipReg>,
}

/// Why `run_frame` / `step_cycle` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A full frame was emulated.
    FrameDone,
    /// A single-step request completed.
    StepDone,
    Breakpoint(BpId),
    Watchpoint {
        id: BpId,
        space: MemSpaceId,
        addr: u32,
        write: bool,
    },
    /// A processor halted itself (6502 JAM). Legitimate guest behaviour.
    CpuJam {
        device: DeviceId,
        pc: u32,
    },
    /// The emulation reached an illegal state; never a panic.
    Fault {
        device: DeviceId,
        reason: &'static str,
    },
}

/// Debugger access to one processor of a machine (main CPU, 1541, C128 Z80,
/// Copper…) and the address spaces it owns.
pub trait DebugTarget {
    fn regs(&self) -> RegisterSet;

    /// Set a register by name. Unknown names are ignored.
    fn set_reg(&mut self, name: &str, v: u32);

    /// Side-effect-free read (golden rule 2). `None` if this target does not
    /// own `space`.
    fn peek(&self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32>;

    /// Debugger write. Bypasses freezes and watchpoints; RAM is written
    /// directly, I/O registers go through the chip like a guest write.
    /// Returns false if the address is not writable (ROM) or `space` is not
    /// owned by this target.
    fn poke(&mut self, space: MemSpaceId, addr: u32, size: Size, v: u32) -> bool;

    /// Disassemble at `addr` in this target's CPU view, using peeks only.
    fn disassemble(&self, addr: u32) -> Instr;
}

/// An emulated computer. Debugger and preview depend only on this trait.
pub trait Machine: Send {
    /// Emulate until the end of the current frame or until a debug stop.
    fn run_frame(&mut self, dbg: &mut DebugHooks) -> StopReason;

    /// Emulate exactly one master cycle.
    fn step_cycle(&mut self) -> StopReason;

    /// The current field: full visible area including border, plus signal
    /// metadata. While halted mid-frame it is the partially drawn field:
    /// lines above [`Machine::beam`] are new, lines below are from the
    /// previous field.
    fn framebuffer(&self) -> FrameRef<'_>;

    /// Append the audio samples produced since the last call to `out`.
    /// Machines without sound append nothing.
    fn audio(&mut self, out: &mut Vec<i16>) -> AudioFormat;

    /// Where the video beam is now. `None` for machines without a beam
    /// (test machines). Raster breakpoints, line stepping, the raster trace
    /// and beam overlays use this, never chip-specific registers.
    fn beam(&self) -> Option<BeamPos>;

    fn memory_spaces(&self) -> Vec<MemSpace>;

    /// Register dumps for debugger views.
    fn chips(&self) -> Vec<ChipInfo>;

    /// All debug targets, main CPU first: "cpu", then 1541, C128 Z80, Copper, Blitter.
    fn devices(&self) -> Vec<DeviceId>;

    /// Debugger access to one of `devices()`; `None` for an unknown id.
    fn debug_target(&mut self, dev: DeviceId) -> Option<&mut dyn DebugTarget>;

    /// Queue an input event. It is applied exactly at `ev.at` (immediately if
    /// that cycle has passed); queued events are part of the savestate, so
    /// recorded input replays bit-exactly.
    fn input(&mut self, ev: TimedInput);

    /// Master cycles since power-on; the time base of [`TimedInput::at`].
    fn now(&self) -> Cycle;

    /// Physical connectors.
    fn ports(&self) -> Vec<PortInfo>;

    /// Plug a device into a port. Fails with [`PortError::Incompatible`] if
    /// the device's [`crate::port::PortKind`] does not match the port.
    fn connect(&mut self, port: PortId, dev: Box<dyn PortDevice>) -> Result<(), PortError>;

    fn disconnect(&mut self, port: PortId) -> Option<Box<dyn PortDevice>>;

    /// Drive 8, DF0:, D1:, tape, cartridge port…
    fn media_slots(&self) -> Vec<MediaSlotInfo>;

    /// `None` for an unknown slot id.
    fn media_slot(&mut self, slot: SlotId) -> Option<&mut dyn MediaSlot>;

    fn save_state(&self) -> Vec<u8>;

    /// Restore a state produced by `save_state`. Corrupt or incompatible data
    /// is rejected with an error and leaves the machine unchanged.
    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError>;
}
