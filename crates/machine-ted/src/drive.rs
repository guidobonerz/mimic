//! Drive 8 on the serial bus: a [`Drive8`] (true 1541 or 1571, or virtual
//! drive) clocked next to the TED's double clock (1 773 447 Hz PAL,
//! 1 789 772 Hz NTSC). The CPU port wiring is in the crate docs.

pub use drives_cbm::SerialDrive as Drive8;
use emu_core::{DeviceId, MemSpaceId};

pub const DRIVE8: DeviceId = DeviceId("drive8");
/// The drive CPU's address space.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(3);
