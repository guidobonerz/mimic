//! Amiga floppy (MFM), Atari SIO drives, CPC FDC drives, datasettes.

#![forbid(unsafe_code)]

pub mod datasette;

pub use datasette::Datasette;
