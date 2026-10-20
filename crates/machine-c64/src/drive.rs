//! Drive 8 on the serial bus: a [`Drive8`] (true 1541, 1571 or 1581, or virtual
//! drive) clocked next to the C64's 985 248 Hz (PAL) or 1 022 727 Hz (NTSC).
//!
//! C64 side (schematic 251469): CIA2 PA3 ATN out, PA4 CLK out, PA5 DATA
//! out through 7406 inverters (1 = line pulled low); PA6 CLK in, PA7 DATA
//! in read the line level (1 = released).

use drives_cbm::IecLines;
pub use drives_cbm::SerialDrive as Drive8;
use emu_core::{DeviceId, MemSpaceId};

pub const DRIVE8: DeviceId = DeviceId("drive8");
/// The drive CPU's address space.
pub const DRIVE_SPACE: MemSpaceId = MemSpaceId(3);

/// What the computer pulls on the bus, from CIA2 port A.
pub fn c64_lines(cia2_pa: u8) -> IecLines {
    IecLines {
        atn: cia2_pa & 0x08 != 0,
        clk: cia2_pa & 0x10 != 0,
        data: cia2_pa & 0x20 != 0,
        srq: false,
    }
}
