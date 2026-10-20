//! Drive 8 on the serial bus: a [`Drive8`] (true 1571 or 1541, or the
//! virtual drive) clocked next to the C128's 985 248 Hz (PAL) or
//! 1 022 727 Hz (NTSC).
//!
//! C128 side (as on the C64): CIA2 PA3 ATN out, PA4 CLK out, PA5 DATA
//! out through 7406 inverters (1 = line pulled low); PA6 CLK in, PA7 DATA
//! in read the line level (1 = released).
//!
//! Fast serial (C128 System Guide; 1571 User's Guide): CIA1's serial port
//! shifts bytes over DATA (SP) clocked on SRQ (CNT). MMU MCR bit 3 (FSDIR)
//! turns the buffers around: set, CIA1 drives the lines; clear, it reads
//! them. SRQ also reaches CIA1's /FLAG input.

use chips_cbm::cia::Cia;
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

/// What CIA1 drives on the bus as fast serial sender (FSDIR set and the
/// serial port in output mode).
pub fn fast_serial_lines(cia1: &Cia, fsdir: bool) -> IecLines {
    if !fsdir || cia1.ta.cr & 0x40 == 0 {
        return IecLines::default();
    }
    let (cnt, sp) = cia1.serial_out();
    IecLines {
        data: !sp,
        srq: !cnt,
        ..IecLines::default()
    }
}
