//! Devices that reach outside the machine: the Ultimate 64 / 1541
//! Ultimate command interface ([`uci`]) on the C64's expansion port and
//! the WiC64 WLAN adapter on the user port ([`wic64`]); [`vrouter`] is a
//! virtual router (user-mode NAT) for machines with an Ethernet interface.
//!
//! They never touch the host themselves (golden rule 10): network and
//! files come through the [`mimic_core::host`] traits the frontend lends the
//! machine, wrapped in a journal so recordings replay exactly.
//! [`testing`] has scripted stand-ins for tests (no real network).

#![forbid(unsafe_code)]

pub mod testing;
pub mod uci;
pub mod vrouter;
pub mod wic64;

pub use uci::Uci;
pub use wic64::Wic64;
