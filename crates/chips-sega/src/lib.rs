//! Sega's own 8-bit chips: the VDP of the Master System, SMS 2 and Game
//! Gear ([`vdp`]).

#![forbid(unsafe_code)]

pub mod vdp;

pub use vdp::{SmsVdp, VdpKind};
