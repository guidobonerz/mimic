//! Amiga floppy (MFM), Atari SIO drives, CPC FDC drives, Apple Disk II, the Macintosh IWM with Sony drives, datasettes.

#![forbid(unsafe_code)]

pub mod amiga_drive;
pub mod apple_tape;
pub mod cpc_drive;
pub mod cpc_tape;
pub mod datasette;
pub mod disk2;
pub mod iwm;
pub mod recorder;
pub mod sio;

pub use datasette::Datasette;
