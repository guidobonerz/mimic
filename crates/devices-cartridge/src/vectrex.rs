//! Vectrex cartridges: ROM at $0000–$7FFF, up to 32 KB, or 64 KB in two
//! 32 KB banks chosen by the VIA's PB6 line.
//!
//! **Plain.** A shorter image repeats up to the next power of two and that
//! block repeats across the 32 KB (the cartridge decodes only the address
//! lines its chips need); writes go nowhere.
//!
//! **64 KB.** PB6 (on the cartridge connector) selects the bank: high the
//! upper half of the image, low the lower half. After reset the VIA's port
//! B is an input and the line is pulled high, so the upper half starts.
//! This is the scheme homebrew 64 KB cartridges use as their documentation
//! describes it (see `docs/sources.md`), not checked against a board.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

const BANK: usize = 0x8000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectrexCart {
    rom: Vec<u8>,
    /// PB6 high: the upper bank of a 64 KB image.
    pub pb6: bool,
}

impl VectrexCart {
    /// A cartridge from a raw ROM image (.vec, .bin, .gam).
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.is_empty() {
            return Err(MediaError::Format("empty cartridge image".into()));
        }
        let rom = if bytes.len() <= BANK {
            let mut rom = bytes.to_vec();
            rom.resize(bytes.len().next_power_of_two(), 0xff);
            rom
        } else if bytes.len() <= 2 * BANK {
            let mut rom = bytes.to_vec();
            rom.resize(2 * BANK, 0xff);
            rom
        } else {
            return Err(MediaError::Format(format!(
                "cartridge image of {} bytes (up to 64 KB)",
                bytes.len()
            )));
        };
        Ok(Self { rom, pb6: true })
    }

    pub fn is_banked(&self) -> bool {
        self.rom.len() > BANK
    }

    pub fn len(&self) -> usize {
        self.rom.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rom.is_empty()
    }

    /// Power-on state (PB6 pulled high).
    pub fn reset(&mut self) {
        self.pb6 = true;
    }

    /// Read $0000–$7FFF.
    pub fn peek(&self, addr: u16) -> u8 {
        let a = usize::from(addr & 0x7fff);
        if self.is_banked() {
            self.rom[usize::from(self.pb6) * BANK + a]
        } else {
            self.rom[a & (self.rom.len() - 1)]
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"VXCA", 1, |w| w.bool(self.pb6));
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"VXCA", 1)?;
        self.pb6 = c.bool()?;
        Ok(())
    }
}

/// The Vectrex's cartridge slot.
pub fn slot() -> CartSlot<VectrexCart> {
    CartSlot::new(VectrexCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_images_repeat() {
        let mut rom = vec![0u8; 0x1000];
        rom[0] = 0x67;
        rom[0xfff] = 0x55;
        let c = VectrexCart::parse(&rom).unwrap();
        assert!(!c.is_banked());
        assert_eq!(c.peek(0x0000), 0x67);
        assert_eq!(c.peek(0x7000), 0x67);
        assert_eq!(c.peek(0x7fff), 0x55);
        let c = VectrexCart::parse(&vec![1u8; 0x1800]).unwrap();
        assert_eq!(c.peek(0x1900), 0xff);
        assert!(VectrexCart::parse(&[]).is_err());
        assert!(VectrexCart::parse(&vec![0; 0x10001]).is_err());
    }

    #[test]
    fn pb6_selects_the_bank_of_64_kb_images() {
        let mut rom = vec![0u8; 0x10000];
        rom[0x0000] = 1;
        rom[0x8000] = 2;
        let mut c = VectrexCart::parse(&rom).unwrap();
        assert!(c.is_banked());
        assert_eq!(c.peek(0), 2, "PB6 pulled high at power-on");
        c.pb6 = false;
        assert_eq!(c.peek(0), 1);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = VectrexCart::parse(&rom).unwrap();
        d.load(&mut StateReader::new(&data)).unwrap();
        assert!(!d.pb6);
        c.reset();
        assert_eq!(c.peek(0), 2);
    }
}
