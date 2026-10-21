//! Atari 8-bit chips: ANTIC, GTIA, POKEY. The PIA is the MOS 6520 also
//! used by the PET (`chips_cbm::pia`); `machine-atari8` wires it.

#![forbid(unsafe_code)]

pub mod antic;
pub mod gtia;
pub mod palette;
pub mod pokey;
