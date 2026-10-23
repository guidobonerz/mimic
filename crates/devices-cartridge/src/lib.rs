//! Cartridges: the C64/C128 expansion port with its bank-switching types
//! and EasyFlash ([`expansion`]), and the plain ROM cartridges of the
//! VIC-20 ([`vic20`]) and the C16/Plus/4 ([`ted`]), and the Atari XL/XE
//! cartridges ([`atari`]), the Atari 2600 cartridges ([`atari2600`]) and
//! the ColecoVision's plain and MegaCart cartridges ([`coleco`]) and the
//! NES cartridges with their mappers ([`nes`]).
//! Machines plug a
//! [`slot::CartSlot`] into their memory map; the images come from CRT files
//! (or raw ROM dumps) through the media layer.

#![forbid(unsafe_code)]

pub mod atari;
pub mod atari2600;
pub mod banked;
pub mod coleco;
pub mod easyflash;
pub mod expansion;
pub mod flash;
pub mod nes;
pub mod slot;
pub mod ted;
pub mod vic20;

pub use expansion::{CartLines, ExpansionCart};
pub use slot::CartSlot;
