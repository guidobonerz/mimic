//! Amiga floppy (MFM), Atari SIO drives, CPC FDC drives, datasettes.

#![forbid(unsafe_code)]

pub mod cpc_tape;
pub mod datasette;
pub mod recorder;
pub mod sio;

pub use datasette::Datasette;
