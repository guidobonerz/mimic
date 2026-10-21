//! ATR: Atari 8-bit disk image. A 16-byte header, then the sectors in
//! order from sector 1.
//!
//! | Offset | Content                                              |
//! | ------ | ---------------------------------------------------- |
//! | 0–1    | magic `$96 $02` (sum of "NICKATARI")                 |
//! | 2–3    | image size / 16, low word                            |
//! | 4–5    | sector size (128 or 256)                             |
//! | 6      | image size / 16, high byte                           |
//! | 7–15   | flags, unused                                        |
//!
//! Double-density images keep the three boot sectors at 128 bytes ("short",
//! the common layout) or pad them to 256 ("full"); the data length tells
//! them apart. Densities: single 720 × 128 (810), enhanced/medium 1040 ×
//! 128 (1050), double 720 × 256. Source: the ATR format as published with
//! the SIO2PC project (Nick Kennedy).

use emu_core::media::MediaError;

pub const HEADER: usize = 16;
const MAGIC: [u8; 2] = [0x96, 0x02];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Atr {
    /// Bytes per sector (128 or 256).
    pub sector_size: u16,
    pub sectors: u32,
    /// Double density with the boot sectors stored at 256 bytes.
    full_boot: bool,
}

impl Atr {
    /// Parse the header; `len` is the whole image's length.
    pub fn parse(header: &[u8], len: u64) -> Result<Self, MediaError> {
        if header.len() < HEADER || header[..2] != MAGIC {
            return Err(MediaError::Format("not an ATR image".into()));
        }
        let sector_size = u16::from_le_bytes([header[4], header[5]]);
        if sector_size != 128 && sector_size != 256 {
            return Err(MediaError::Format(format!("ATR sector size {sector_size}")));
        }
        let data = len.saturating_sub(HEADER as u64);
        let (sectors, full_boot) = if sector_size == 128 {
            (data / 128, false)
        } else if data >= 384 && (data - 384).is_multiple_of(256) {
            ((data - 384) / 256 + 3, false)
        } else {
            (data / 256, true)
        };
        if sectors == 0 || sectors > 0xffff {
            return Err(MediaError::Format("ATR without sectors".into()));
        }
        Ok(Self {
            sector_size,
            sectors: sectors as u32,
            full_boot,
        })
    }

    /// Bytes of sector `n` (boot sectors 1–3 are always 128).
    pub fn size_of(&self, n: u32) -> usize {
        if n <= 3 {
            128
        } else {
            usize::from(self.sector_size)
        }
    }

    /// Byte offset of sector `n` (1-based) in the image; `None` outside.
    pub fn offset(&self, n: u32) -> Option<u64> {
        if n == 0 || n > self.sectors {
            return None;
        }
        let i = u64::from(n - 1);
        let size = u64::from(self.sector_size);
        Some(
            HEADER as u64
                + if size == 128 || self.full_boot {
                    i * size
                } else if n <= 3 {
                    i * 128
                } else {
                    384 + (i - 3) * size
                },
        )
    }

    pub fn single_density(&self) -> bool {
        self.sector_size == 128 && self.sectors <= 720
    }

    pub fn enhanced_density(&self) -> bool {
        self.sector_size == 128 && self.sectors > 720
    }
}

/// A blank single-density (720 sectors) or enhanced-density (1040) image.
pub fn blank(sectors: u32) -> Vec<u8> {
    let size = 128 * sectors as usize;
    let paragraphs = (size / 16) as u32;
    let mut img = vec![0u8; HEADER + size];
    img[..2].copy_from_slice(&MAGIC);
    img[2..4].copy_from_slice(&(paragraphs as u16).to_le_bytes());
    img[4..6].copy_from_slice(&128u16.to_le_bytes());
    img[6] = (paragraphs >> 16) as u8;
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_and_enhanced_density() {
        let img = blank(720);
        let atr = Atr::parse(&img, img.len() as u64).unwrap();
        assert_eq!((atr.sector_size, atr.sectors), (128, 720));
        assert!(atr.single_density());
        assert_eq!(atr.offset(1), Some(16));
        assert_eq!(atr.offset(720), Some(16 + 719 * 128));
        assert_eq!(atr.offset(721), None);
        assert_eq!(atr.offset(0), None);
        let img = blank(1040);
        assert!(Atr::parse(&img, img.len() as u64)
            .unwrap()
            .enhanced_density());
    }

    #[test]
    fn double_density_boot_sector_layouts() {
        let mut header = blank(1)[..16].to_vec();
        header[4] = 0;
        header[5] = 1; // 256-byte sectors
        let short = Atr::parse(&header, 16 + 384 + 717 * 256).unwrap();
        assert_eq!(short.sectors, 720);
        assert_eq!(short.offset(3), Some(16 + 256));
        assert_eq!(short.offset(4), Some(16 + 384));
        assert_eq!(short.size_of(3), 128);
        assert_eq!(short.size_of(4), 256);
        let full = Atr::parse(&header, 16 + 720 * 256).unwrap();
        assert_eq!(full.sectors, 720);
        assert_eq!(full.offset(4), Some(16 + 3 * 256));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Atr::parse(&[0; 16], 1000).is_err());
        let mut h = blank(1)[..16].to_vec();
        h[4] = 100;
        assert!(Atr::parse(&h, 1000).is_err());
        assert!(Atr::parse(&blank(1)[..16], 16).is_err());
    }
}
