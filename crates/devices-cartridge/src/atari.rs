//! Atari 8-bit cartridges (the XL/XE cartridge slot): ROM on RD4
//! (`$8000–$9FFF`) and RD5 (`$A000–$BFFF`), bank switching through the
//! CCTL area `$D500–$D5FF`. RD5 also drives TRIG3 and switches the XL's
//! internal BASIC off.
//!
//! | CAR type          | Size       | Mapping                                        |
//! | ----------------- | ---------- | ---------------------------------------------- |
//! | 1 standard 8K     | 8 KB       | `$A000`                                        |
//! | 2 standard 16K    | 16 KB      | `$8000–$BFFF`                                  |
//! | 12/13/14/23/24/25 XEGS | 32 KB–1 MB | bank (write to `$D5xx`, value) at `$8000`, last bank fixed at `$A000` |
//! | 33–38 switchable XEGS | 32 KB–1 MB | as XEGS; bit 7 of the value switches the cartridge off |
//! | 22/8 Williams     | 32/64 KB   | bank (access `$D500–$D507`) at `$A000`; `$D508–$D50F` off |
//! | 41 Atarimax 128K  | 128 KB     | bank (access `$D500–$D50F`) at `$A000`; `$D510–$D51F` off |
//!
//! Sources: Altirra Hardware Reference Manual, "Cartridges"; the CAR type
//! list of the Atari800 project's documentation.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::car::Car;

use crate::slot::CartSlot;

const BANK: usize = 0x2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Standard8,
    Standard16,
    Xegs { switchable: bool },
    Williams,
    Atarimax,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtariCart {
    pub kind: u32,
    scheme: Scheme,
    rom: Vec<u8>,
    bank: usize,
    enabled: bool,
}

impl AtariCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        Self::from_car(Car::parse(bytes)?)
    }

    pub fn from_car(car: Car) -> Result<Self, MediaError> {
        let (scheme, size) = match car.kind {
            1 => (Scheme::Standard8, 0x2000),
            2 => (Scheme::Standard16, 0x4000),
            12 => (Scheme::Xegs { switchable: false }, 0x8000),
            13 => (Scheme::Xegs { switchable: false }, 0x10000),
            14 => (Scheme::Xegs { switchable: false }, 0x20000),
            23 => (Scheme::Xegs { switchable: false }, 0x40000),
            24 => (Scheme::Xegs { switchable: false }, 0x80000),
            25 => (Scheme::Xegs { switchable: false }, 0x100000),
            33 => (Scheme::Xegs { switchable: true }, 0x8000),
            34 => (Scheme::Xegs { switchable: true }, 0x10000),
            35 => (Scheme::Xegs { switchable: true }, 0x20000),
            36 => (Scheme::Xegs { switchable: true }, 0x40000),
            37 => (Scheme::Xegs { switchable: true }, 0x80000),
            38 => (Scheme::Xegs { switchable: true }, 0x100000),
            22 => (Scheme::Williams, 0x8000),
            8 => (Scheme::Williams, 0x10000),
            41 => (Scheme::Atarimax, 0x20000),
            k => {
                return Err(MediaError::Format(format!(
                    "Atari cartridge type {k} is not supported"
                )))
            }
        };
        if car.rom.len() != size {
            return Err(MediaError::Format(format!(
                "cartridge type {} needs {} bytes, the image has {}",
                car.kind,
                size,
                car.rom.len()
            )));
        }
        Ok(Self {
            kind: car.kind,
            scheme,
            rom: car.rom,
            bank: 0,
            enabled: true,
        })
    }

    fn banks(&self) -> usize {
        self.rom.len() / BANK
    }

    /// Power-up banking.
    pub fn reset(&mut self) {
        self.bank = 0;
        self.enabled = true;
    }

    /// RD4: the cartridge answers at `$8000–$9FFF`.
    pub fn rd4(&self) -> bool {
        self.enabled && matches!(self.scheme, Scheme::Standard16 | Scheme::Xegs { .. })
    }

    /// RD5: the cartridge answers at `$A000–$BFFF`.
    pub fn rd5(&self) -> bool {
        self.enabled
    }

    /// A read of `$8000–$BFFF`; `None` where the cartridge is not mapped.
    pub fn read(&self, addr: u16) -> Option<u8> {
        let a = usize::from(addr);
        match addr {
            0x8000..=0x9fff if self.rd4() => Some(match self.scheme {
                Scheme::Standard16 => self.rom[a - 0x8000],
                _ => self.rom[self.bank * BANK + (a - 0x8000)],
            }),
            0xa000..=0xbfff if self.rd5() => Some(match self.scheme {
                Scheme::Standard8 => self.rom[a - 0xa000],
                Scheme::Standard16 => self.rom[a - 0x8000],
                Scheme::Xegs { .. } => self.rom[(self.banks() - 1) * BANK + (a - 0xa000)],
                Scheme::Williams | Scheme::Atarimax => self.rom[self.bank * BANK + (a - 0xa000)],
            }),
            _ => None,
        }
    }

    /// An access to `$D500–$D5FF` (`value` for writes).
    pub fn access(&mut self, addr: u16, value: Option<u8>) {
        let low = usize::from(addr & 0xff);
        match self.scheme {
            Scheme::Xegs { switchable } => {
                if let Some(v) = value {
                    self.bank = usize::from(v) & (self.banks() - 1);
                    self.enabled = !(switchable && v & 0x80 != 0);
                }
            }
            Scheme::Williams => match low {
                0x00..=0x07 => {
                    self.bank = low & (self.banks() - 1);
                    self.enabled = true;
                }
                0x08..=0x0f => self.enabled = false,
                _ => {}
            },
            Scheme::Atarimax => match low {
                0x00..=0x0f => {
                    self.bank = low;
                    self.enabled = true;
                }
                0x10..=0x1f => self.enabled = false,
                _ => {}
            },
            Scheme::Standard8 | Scheme::Standard16 => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"ACRT", 1, |w| {
            w.u32(self.kind);
            w.u32(self.bank as u32);
            w.bool(self.enabled);
        });
    }

    /// Banking state; the ROM is the inserted image.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"ACRT", 1)?;
        let kind = c.u32()?;
        let bank = c.u32()? as usize;
        let enabled = c.bool()?;
        if kind != self.kind || bank >= self.banks() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        self.bank = bank;
        self.enabled = enabled;
        Ok(())
    }
}

/// The XL/XE cartridge slot.
pub fn slot() -> CartSlot<AtariCart> {
    CartSlot::new(AtariCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM whose bytes name their 8 KB bank.
    fn car(kind: u32, size: usize) -> AtariCart {
        let rom = (0..size).map(|i| (i / BANK) as u8).collect();
        AtariCart::from_car(Car { kind, rom }).unwrap()
    }

    #[test]
    fn standard_cartridges() {
        let c = car(1, 0x2000);
        assert!(c.rd5() && !c.rd4());
        assert_eq!(c.read(0xa000), Some(0));
        assert_eq!(c.read(0x8000), None);
        let c = car(2, 0x4000);
        assert!(c.rd4() && c.rd5());
        assert_eq!(c.read(0x8000), Some(0));
        assert_eq!(c.read(0xbfff), Some(1));
    }

    #[test]
    fn xegs_banks_and_switch_off() {
        let mut c = car(13, 0x10000);
        assert_eq!(c.read(0x8000), Some(0));
        assert_eq!(c.read(0xa000), Some(7), "last bank fixed");
        c.access(0xd500, Some(5));
        assert_eq!(c.read(0x8000), Some(5));
        c.access(0xd5ff, Some(0x0b));
        assert_eq!(c.read(0x8000), Some(3), "masked to 8 banks");
        c.access(0xd500, None);
        assert_eq!(c.read(0x8000), Some(3), "reads do not switch");
        c.access(0xd500, Some(0x80));
        assert!(c.rd5(), "plain XEGS ignores bit 7");
        let mut s = car(34, 0x10000);
        s.access(0xd500, Some(0x81));
        assert!(!s.rd4() && !s.rd5());
        assert_eq!(s.read(0xa000), None);
    }

    #[test]
    fn williams_and_atarimax_switch_on_any_access() {
        let mut w = car(8, 0x10000);
        w.access(0xd506, None);
        assert_eq!(w.read(0xa000), Some(6));
        w.access(0xd508, Some(0));
        assert!(!w.rd5());
        w.access(0xd501, None);
        assert_eq!(w.read(0xa000), Some(1));
        let mut m = car(41, 0x20000);
        m.access(0xd50f, None);
        assert_eq!(m.read(0xa000), Some(15));
        m.access(0xd510, None);
        assert!(!m.rd5());
        m.reset();
        assert_eq!(m.read(0xa000), Some(0));
    }

    #[test]
    fn validation_and_state() {
        assert!(AtariCart::from_car(Car {
            kind: 12,
            rom: vec![0; 0x4000]
        })
        .is_err());
        assert!(AtariCart::from_car(Car {
            kind: 999,
            rom: vec![0; 0x2000]
        })
        .is_err());
        let mut c = car(14, 0x20000);
        c.access(0xd500, Some(9));
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = car(14, 0x20000);
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
        let mut other = car(13, 0x10000);
        assert!(other.load(&mut StateReader::new(&bytes)).is_err());
    }
}
