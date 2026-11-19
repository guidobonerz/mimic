//! Sega's own video chips: the VDP of the Master System, SMS 2 and Game
//! Gear ([`vdp`]) and the Mega Drive's 315-5313 ([`mdvdp`]).

#![forbid(unsafe_code)]

pub mod mdvdp;
pub mod vdp;

pub use mdvdp::MdVdp;
pub use vdp::{SmsVdp, VdpKind};
