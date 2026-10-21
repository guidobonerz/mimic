//! Atari chips: ANTIC, GTIA, POKEY (8-bit computers; their PIA is the MOS
//! 6520 also used by the PET, `chips_cbm::pia`), and the Atari 2600's TIA
//! and its MOS 6532 RIOT.

#![forbid(unsafe_code)]

pub mod antic;
pub mod gtia;
pub mod palette;
pub mod pokey;
pub mod riot;
pub mod tia;
