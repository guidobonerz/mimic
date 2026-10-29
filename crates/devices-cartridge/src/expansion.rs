//! The C64/C128 expansion port as a cartridge sees it.
//!
//! Lines (active low): /GAME and /EXROM select the memory configuration
//! with the PLA (8 KB: EXROM; 16 KB: both; Ultimax: GAME), /ROML and
//! /ROMH select the cartridge for `$8000–$9FFF` and `$A000–$BFFF` (16 KB) or
//! `$E000–$FFFF` (Ultimax; the VIC-II then also sees ROMH at `$3000` of each
//! bank), /IO1 and /IO2 for `$DE00` and `$DF00`; /NMI and /IRQ for freezers.
//! Source: Commodore 64 Programmer's Reference Guide, appendix (expansion
//! port); C64 memory map and PLA.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::{hw, Crt, CrtMachine};

use crate::banked::{Banked, Scheme};
use crate::easyflash::EasyFlash;

/// Lines the cartridge pulls (true = low = asserted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CartLines {
    pub game: bool,
    pub exrom: bool,
    pub nmi: bool,
    pub irq: bool,
}

/// A cartridge in the expansion port. Addresses are CPU addresses; a
/// `None` read leaves the bus open.
pub trait ExpansionCart: Send {
    /// The cartridge type, for the debugger and messages.
    fn name(&self) -> &'static str;

    fn lines(&self) -> CartLines;

    /// `$8000–$9FFF` with /ROML.
    fn peek_roml(&self, addr: u16) -> Option<u8>;

    /// `$A000–$BFFF` or `$E000–$FFFF` with /ROMH.
    fn peek_romh(&self, addr: u16) -> Option<u8>;

    fn read_roml(&mut self, addr: u16) -> Option<u8> {
        self.peek_roml(addr)
    }

    fn read_romh(&mut self, addr: u16) -> Option<u8> {
        self.peek_romh(addr)
    }

    /// A write while /ROML is selected (flash, RAM carts).
    fn write_roml(&mut self, _addr: u16, _v: u8) {}

    fn write_romh(&mut self, _addr: u16, _v: u8) {}

    /// `$DE00–$DFFF` (/IO1, /IO2).
    fn peek_io(&self, _addr: u16) -> Option<u8> {
        None
    }

    fn read_io(&mut self, addr: u16) -> Option<u8> {
        self.peek_io(addr)
    }

    fn write_io(&mut self, _addr: u16, _v: u8) {}

    /// /RESET.
    fn reset(&mut self);

    /// One CPU cycle (freezers count, timers run); most carts do nothing.
    fn tick(&mut self) {}

    /// The cartridge's button (freezers); `false` if it has none.
    fn press_button(&mut self) -> bool {
        false
    }

    /// Erase the 64 KB flash sector holding 16 KB bank `bank` of ROML
    /// (`high = false`) or ROMH, as the Ultimate's EASYFLASH_ERASE does;
    /// `false` for cartridges without flash.
    fn erase_flash_sector(&mut self, _bank: u8, _high: bool) -> bool {
        false
    }

    /// The image with what the machine wrote (flash), if anything changed.
    fn written_image(&self) -> Option<Crt> {
        None
    }

    fn save(&self, w: &mut StateWriter);

    /// Atomic: nothing changes unless the whole state is valid.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError>;
}

/// The cartridge a C64 CRT describes.
pub fn from_crt(crt: &Crt) -> Result<Box<dyn ExpansionCart>, MediaError> {
    if crt.machine != CrtMachine::C64 {
        return Err(MediaError::Format(format!(
            "a {:?} cartridge does not fit the expansion port",
            crt.machine
        )));
    }
    let scheme = match crt.hardware {
        hw::NORMAL => Scheme::Normal,
        hw::OCEAN => Scheme::Ocean,
        hw::FUN_PLAY => Scheme::FunPlay,
        hw::SUPER_GAMES => Scheme::SuperGames,
        hw::SYSTEM_3 => Scheme::System3,
        hw::DINAMIC => Scheme::Dinamic,
        hw::MAGIC_DESK => Scheme::MagicDesk,
        hw::EASYFLASH => return Ok(Box::new(EasyFlash::from_crt(crt)?)),
        other => {
            return Err(MediaError::Format(format!(
                "cartridge type {other} is not supported yet"
            )))
        }
    };
    Ok(Box::new(Banked::from_crt(crt, scheme)?))
}
