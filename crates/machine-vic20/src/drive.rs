//! Drive 8 on the serial bus: a [`Drive8`] (true 1541 or 1571, or virtual
//! drive) clocked next to the VIC-20's 1 108 404 Hz (PAL) or 1 022 727 Hz
//! (NTSC). The VIA wiring is in [`crate::io`].

pub use drives_cbm::SerialDrive as Drive8;
use emu_core::{DeviceId, MemSpaceId};

pub const DRIVE8: DeviceId = DeviceId("drive8");
/// The drive CPU's address space.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(3);
