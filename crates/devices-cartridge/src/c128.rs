//! C128 external function ROM: a cartridge for the C128 mode. Its ROM
//! sits in the external function ROM space the MMU maps in at `$8000-$BFFF`
//! (low) and `$C000-$FFFF` (high), up to 32 KB; the KERNAL looks for a
//! "CBM" signature at `$8007` (low) and `$C007` (high) at reset and calls
//! the cartridge. In C64 mode the cartridge is not there (GAME and EXROM
//! stay high).
//!
//! Images: CRT files with the signature "C128 CARTRIDGE  " (VICE's test
//! cartridges, `testprogs/testbench/selftest/c128-pass.crt`), each CHIP
//! loaded at `$8000` (low) or `$C000` (high). Source: Commodore 128
//! Programmer's Reference Guide, chapter 13 ("function ROM" and the
//! KERNAL's PHOENIX routine); the CRT format as documented with the VICE
//! cartridge tools.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::Crt;

use crate::expansion::{CartLines, ExpansionCart};

/// External function ROM size: low and high, 16 KB each.
pub const SIZE: usize = 0x8000;

pub struct FunctionRom {
    rom: Box<[u8; SIZE]>,
}

impl FunctionRom {
    pub fn from_crt(crt: &Crt) -> Result<Self, MediaError> {
        let mut rom = Box::new([0xff; SIZE]);
        for chip in &crt.chips {
            let at = usize::from(chip.load)
                .checked_sub(0x8000)
                .filter(|o| o + chip.data.len() <= SIZE)
                .ok_or_else(|| {
                    MediaError::Format(format!(
                        "C128 cartridge: CHIP at ${:04X} outside $8000-$FFFF",
                        chip.load
                    ))
                })?;
            rom[at..at + chip.data.len()].copy_from_slice(&chip.data);
        }
        Ok(Self { rom })
    }
}

impl ExpansionCart for FunctionRom {
    fn name(&self) -> &'static str {
        "C128 function ROM"
    }

    fn lines(&self) -> CartLines {
        CartLines::default()
    }

    fn peek_roml(&self, _addr: u16) -> Option<u8> {
        None
    }

    fn peek_romh(&self, _addr: u16) -> Option<u8> {
        None
    }

    fn peek_function(&self, offset: u16) -> Option<u8> {
        self.rom.get(usize::from(offset)).copied()
    }

    fn reset(&mut self) {}

    fn save(&self, _w: &mut StateWriter) {}

    fn load(&mut self, _r: &mut StateReader<'_>) -> Result<(), StateError> {
        Ok(())
    }
}
