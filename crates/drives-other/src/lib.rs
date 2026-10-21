//! Amiga floppy (MFM), Atari SIO drives, CPC FDC drives, datasettes.

#![forbid(unsafe_code)]

pub mod amiga_drive;
pub mod cpc_drive;
pub mod cpc_tape;
pub mod datasette;
pub mod recorder;
pub mod sio;

pub use datasette::Datasette;
