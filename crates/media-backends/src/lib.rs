//! Media backends: image file, copy-on-write overlay, host directory, real hardware.

#![forbid(unsafe_code)]

pub mod host_dir;
pub mod image;
pub mod roms;
