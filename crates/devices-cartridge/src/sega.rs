//! Sega SG-1000 / SC-3000 cartridges: a ROM image that the machine maps
//! itself (`machine-sega8`): the ROM from $0000, repeated to fill
//! $0000–$7FFF when it is smaller, on to $BFFF when it is larger than
//! 32 KB. Cartridge RAM (Safari Hunting's 2 KB, the BASIC cartridges' 16 or
//! 32 KB) is the machine's choice, not in the image (Charles MacDonald,
//! "Sega SC-3000H hardware notes", section 3).

use mimic_core::media::MediaError;

use crate::CartSlot;

/// The largest image: 48 KB, $0000–$BFFF.
pub const MAX_ROM: usize = 0xc000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegaCart {
    rom: Vec<u8>,
}

impl SegaCart {
    /// A cartridge from a raw ROM image.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.is_empty() {
            return Err(MediaError::Format("empty cartridge image".into()));
        }
        if bytes.len() > MAX_ROM {
            return Err(MediaError::Format(format!(
                "a {} byte image is larger than the 48 KB the slot maps",
                bytes.len()
            )));
        }
        Ok(Self {
            rom: bytes.to_vec(),
        })
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// The ROM byte at `addr`, if the ROM answers there: below $8000
    /// (repeated), or below $C000 when the image reaches there.
    pub fn read(&self, addr: u16) -> Option<u8> {
        let a = usize::from(addr);
        let len = self.rom.len();
        if a < len {
            Some(self.rom[a])
        } else if a < 0x8000 {
            Some(self.rom[a % len])
        } else {
            None
        }
    }
}

pub fn slot() -> CartSlot<SegaCart> {
    CartSlot::new(SegaCart::parse, |_| None)
}

/// The largest Master System / Game Gear image taken (4 MB).
pub const MAX_SMS_ROM: usize = 0x40_0000;

/// A Master System / Game Gear cartridge: the ROM, banked by the
/// machine's mapper (`machine-sega8`; SMS Power, "Mappers"). An image of
/// 512 bytes over a multiple of 16 KB has a copier's header, which is
/// dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmsCart {
    rom: Vec<u8>,
}

impl SmsCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let bytes = if bytes.len() > 0x4000 && bytes.len() % 0x4000 == 512 {
            &bytes[512..]
        } else {
            bytes
        };
        if bytes.is_empty() {
            return Err(MediaError::Format("empty cartridge image".into()));
        }
        if bytes.len() > MAX_SMS_ROM {
            return Err(MediaError::Format(format!(
                "a {} byte image is larger than 4 MB",
                bytes.len()
            )));
        }
        Ok(Self {
            rom: bytes.to_vec(),
        })
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// The byte at `offset` into the ROM, repeated past its end.
    pub fn byte(&self, offset: usize) -> u8 {
        self.rom[offset % self.rom.len()]
    }
}

pub fn sms_slot() -> CartSlot<SmsCart> {
    CartSlot::new(SmsCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_repeat_below_32_kb() {
        let mut rom = vec![0u8; 0x4000];
        rom[0] = 0x11;
        rom[0x3fff] = 0x22;
        let c = SegaCart::parse(&rom).unwrap();
        assert_eq!(c.read(0x4000), Some(0x11), "Safari Hunting's mirror");
        assert_eq!(c.read(0x7fff), Some(0x22));
        assert_eq!(c.read(0x8000), None, "the RAM socket's area");
        let big = SegaCart::parse(&vec![7u8; MAX_ROM]).unwrap();
        assert_eq!(big.read(0xbfff), Some(7));
        assert!(SegaCart::parse(&vec![0; MAX_ROM + 1]).is_err());
        assert!(SegaCart::parse(&[]).is_err());
    }
}
