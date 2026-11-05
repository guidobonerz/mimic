//! IEC bus, 1541, 1571, 1581 (each with its own CPU), the virtual IEC
//! drive, the IEEE-488 bus with a virtual drive for the PET, and the
//! C16/Plus/4 TCBM bus with the 1551.

#![forbid(unsafe_code)]

pub mod c1541;
pub mod c1581;
pub mod dos;
pub mod iec;
pub mod ieee;
pub mod ieee_drive;
pub mod serial;
pub mod tcbm;
pub mod virtual_drive;
pub mod wd177x;

pub use c1541::{CbmDrive, DriveModel};
pub use c1581::Drive1581;
pub use dos::VirtualDos;
pub use iec::IecLines;
pub use ieee::IeeeLines;
pub use ieee_drive::IeeeDrive;
pub use serial::{DriveKind, DriveRoms, SerialDrive, TrueDrive};
pub use tcbm::Tcbm1551;
pub use virtual_drive::VirtualDrive;
