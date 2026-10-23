//! ColecoVision cartridges: plain ROM up to 32 KB at $8000–$FFFF, and the
//! MegaCart (128 KB to 1 MB in 16 KB banks).
//!
//! **Plain.** The ROM fills $8000 upwards; a shorter image repeats up to
//! the next power of two and that block repeats across the 32 KB (the
//! cartridge decodes only the address lines its chips need), so an 8 KB
//! game also answers at $A000, $C000 and $E000.
//!
//! **MegaCart.** Images larger than 32 KB (a power of two, 64 KB to 1 MB):
//! the last 16 KB bank is fixed at $8000–$BFFF, $C000–$FFFF shows the
//! selected bank, and any access to $FFC0–$FFFF selects bank `addr & (banks
//! − 1)` (the low address bits); at power-on bank 0 is selected. This is
//! the scheme as the MegaCart's homebrew documentation describes it (see
//! `docs/sources.md`), not checked against a board.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

const BANK: usize = 0x4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColecoCart {
    rom: Vec<u8>,
    /// MegaCart: banks of 16 KB; `None` for a plain cartridge.
    banks: Option<usize>,
    /// The bank at $C000 (MegaCart).
    pub bank: usize,
}

impl ColecoCart {
    /// A cartridge from a raw ROM image.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.is_empty() {
            return Err(MediaError::Format("empty cartridge image".into()));
        }
        if bytes.len() <= 0x8000 {
            let size = bytes.len().next_power_of_two().max(0x2000);
            let mut rom = bytes.to_vec();
            rom.resize(size, 0xff);
            return Ok(Self {
                rom,
                banks: None,
                bank: 0,
            });
        }
        if !bytes.len().is_power_of_two() || bytes.len() > 0x10_0000 {
            return Err(MediaError::Format(format!(
                "cartridge image of {} bytes (up to 32 KB, or a MegaCart of 64 KB to 1 MB in powers of two)",
                bytes.len()
            )));
        }
        Ok(Self {
            rom: bytes.to_vec(),
            banks: Some(bytes.len() / BANK),
            bank: 0,
        })
    }

    pub fn is_megacart(&self) -> bool {
        self.banks.is_some()
    }

    pub fn len(&self) -> usize {
        self.rom.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rom.is_empty()
    }

    /// Power-on state.
    pub fn reset(&mut self) {
        self.bank = 0;
    }

    /// Side-effect-free read of $8000–$FFFF.
    pub fn peek(&self, addr: u16) -> u8 {
        let a = usize::from(addr & 0x7fff);
        match self.banks {
            None => self.rom[a & (self.rom.len() - 1)],
            Some(banks) => {
                let bank = if a < BANK { banks - 1 } else { self.bank };
                self.rom[bank * BANK + (a & (BANK - 1))]
            }
        }
    }

    /// A CPU access (read or write) to $8000–$FFFF: the MegaCart switches
    /// banks on $FFC0–$FFFF. Returns the byte a read sees.
    pub fn access(&mut self, addr: u16) -> u8 {
        let v = self.peek(addr);
        if let Some(banks) = self.banks {
            if addr >= 0xffc0 {
                self.bank = usize::from(addr) & (banks - 1);
            }
        }
        v
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"CVCA", 1, |w| {
            w.u32(self.bank as u32);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"CVCA", 1)?;
        let bank = c.u32()? as usize;
        if self.banks.is_some_and(|b| bank >= b) {
            return Err(StateError::Invalid("MegaCart bank"));
        }
        self.bank = bank;
        Ok(())
    }
}

/// The ColecoVision's cartridge slot.
pub fn slot() -> CartSlot<ColecoCart> {
    CartSlot::new(ColecoCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_cartridges_repeat_to_fill_32_kb() {
        let mut rom = vec![0u8; 0x2000];
        rom[0] = 0xaa;
        rom[0x1fff] = 0x55;
        let c = ColecoCart::parse(&rom).unwrap();
        assert!(!c.is_megacart());
        assert_eq!(c.peek(0x8000), 0xaa);
        assert_eq!(c.peek(0xa000), 0xaa);
        assert_eq!(c.peek(0xffff), 0x55);
        // 12 KB pads to 16 KB with $FF.
        let c = ColecoCart::parse(&vec![1u8; 0x3000]).unwrap();
        assert_eq!(c.peek(0xb000), 0xff);
        assert_eq!(c.peek(0xc000), 1);
        assert!(ColecoCart::parse(&[]).is_err());
        assert!(ColecoCart::parse(&vec![0; 0x9000]).is_err());
    }

    #[test]
    fn megacart_fixes_the_last_bank_and_switches_on_ffc0() {
        let mut rom = vec![0u8; 0x2_0000];
        for b in 0..8 {
            rom[b * BANK] = b as u8;
        }
        let mut c = ColecoCart::parse(&rom).unwrap();
        assert!(c.is_megacart());
        assert_eq!(c.peek(0x8000), 7, "last bank fixed");
        assert_eq!(c.peek(0xc000), 0, "bank 0 at power-on");
        assert_eq!(c.access(0xffc3), 0);
        assert_eq!(c.peek(0xc000), 3);
        c.access(0xfffd);
        assert_eq!(c.bank, 5, "only the bank bits count");
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = ColecoCart::parse(&rom).unwrap();
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d.bank, 5);
        c.reset();
        assert_eq!(c.peek(0xc000), 0);
    }
}
