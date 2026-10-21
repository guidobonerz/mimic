//! Atari 2600 cartridges. The cartridge sees A0–A12 of the 6507; it
//! answers in the upper half (A12 = 1, 4 KB) and may watch every bus cycle
//! for bank-switching hotspots.
//!
//! | Scheme | Size  | Mapping                                   |
//! | ------ | ----- | ----------------------------------------- |
//! | 2K     | 2 KB  | mirrored twice into the 4 KB window       |
//! | 4K     | 4 KB  | the whole window                          |
//!
//! Images are raw ROM dumps (`.a26`, `.bin`); the scheme follows from
//! the size. Source: Kevin Horton, "Cart Information" (sizes.txt,
//! <https://www.qotile.net/minidig/docs/sizes.txt>).

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// 2 KB or 4 KB, no bank switching.
    Plain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cart2600 {
    pub scheme: Scheme,
    rom: Vec<u8>,
}

impl Cart2600 {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        match bytes.len() {
            0x800 | 0x1000 => Ok(Self {
                scheme: Scheme::Plain,
                rom: bytes.to_vec(),
            }),
            n => Err(MediaError::Format(format!(
                "Atari 2600 cartridge of {n} bytes (2 KB or 4 KB supported)"
            ))),
        }
    }

    /// The byte at `addr` (A12 = 1) without side effects.
    pub fn peek(&self, addr: u16) -> u8 {
        self.rom[usize::from(addr) & (self.rom.len() - 1)]
    }

    /// One bus cycle at `addr` (A0–A12); `write` carries the data of a
    /// write. Returns what the cartridge drives onto the data bus.
    pub fn access(&mut self, addr: u16, write: Option<u8>) -> Option<u8> {
        (addr & 0x1000 != 0 && write.is_none()).then(|| self.peek(addr))
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"C26 ", 1, |w| w.u32(self.rom.len() as u32));
    }

    /// Banking state; the ROM is the inserted image.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"C26 ", 1)?;
        if c.u32()? as usize != self.rom.len() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        Ok(())
    }
}

/// The 2600's cartridge slot.
pub fn slot() -> CartSlot<Cart2600> {
    CartSlot::new(Cart2600::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_images_mirror_into_the_window() {
        let mut rom = vec![0u8; 0x800];
        rom[0] = 0x11;
        rom[0x7ff] = 0x22;
        let mut c = Cart2600::parse(&rom).unwrap();
        assert_eq!(c.access(0x1000, None), Some(0x11));
        assert_eq!(c.access(0x1800, None), Some(0x11), "2K mirrored");
        assert_eq!(c.access(0x1fff, None), Some(0x22));
        assert_eq!(c.access(0x0080, None), None, "not in the cartridge window");
        assert_eq!(c.access(0x1000, Some(5)), None);
        assert!(Cart2600::parse(&[0; 100]).is_err());
    }
}
