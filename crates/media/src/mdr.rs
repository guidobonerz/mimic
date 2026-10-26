//! MDR: ZX Microdrive cartridge images.
//!
//! 254 sectors of 543 bytes in the order they pass the head, then one byte
//! that is non-zero for a write-protected cartridge (137 923 bytes; images
//! without it, 137 922 bytes, are writable). A sector is the 15-byte
//! header block (HDFLAG with bit 0 set, HDNUMB the sector number, two
//! unused bytes, HDNAME the cartridge name, HDCHK) followed by the 528-byte
//! record block (RECFLG, RECNUM, RECLEN, RECNAM, DESCHK, 512 data bytes,
//! DSCHK); the Interface 1 ROM writes and checks the flags and checksums
//! itself, so the image keeps the blocks as they are.
//!
//! Source: World of Spectrum, "MDR file format"; Ian Logan, *Spectrum
//! Microdrive Book* (the block layout). The implementation is this
//! project's own.

use emu_core::media::MediaError;

pub const SECTORS: usize = 254;
pub const HEADER_LEN: usize = 15;
pub const RECORD_LEN: usize = 528;
pub const SECTOR_LEN: usize = HEADER_LEN + RECORD_LEN;
/// Image size with the write-protect byte.
pub const IMAGE_LEN: usize = SECTORS * SECTOR_LEN + 1;

/// A cartridge: the sectors in tape order and the write-protect tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cartridge {
    pub sectors: Vec<[u8; SECTOR_LEN]>,
    pub protected: bool,
}

impl Cartridge {
    /// An unformatted cartridge (all zero).
    pub fn blank() -> Self {
        Self {
            sectors: vec![[0; SECTOR_LEN]; SECTORS],
            protected: false,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() != IMAGE_LEN && bytes.len() != IMAGE_LEN - 1 {
            return Err(MediaError::Format(format!(
                "MDR: {} bytes (expected {IMAGE_LEN})",
                bytes.len()
            )));
        }
        let sectors = bytes[..SECTORS * SECTOR_LEN]
            .chunks(SECTOR_LEN)
            .map(|c| c.try_into().unwrap_or([0; SECTOR_LEN]))
            .collect();
        Ok(Self {
            sectors,
            protected: bytes.get(SECTORS * SECTOR_LEN).is_some_and(|&p| p != 0),
        })
    }

    /// The image with its write-protect byte.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = self.sectors.iter().flatten().copied().collect();
        out.push(u8::from(self.protected));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_sizes() {
        let mut c = Cartridge::blank();
        c.sectors[3][0] = 1;
        c.protected = true;
        let b = c.to_bytes();
        assert_eq!(b.len(), IMAGE_LEN);
        assert_eq!(Cartridge::parse(&b).unwrap(), c);
        let short = Cartridge::parse(&b[..IMAGE_LEN - 1]).unwrap();
        assert!(!short.protected);
        assert!(Cartridge::parse(&[0; 100]).is_err());
    }
}
