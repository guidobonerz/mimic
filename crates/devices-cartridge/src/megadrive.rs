//! Mega Drive / Genesis cartridges: the ROM (big-endian words, `.bin` /
//! `.md` / `.gen`, or the interleaved `.smd` copier format) and what its
//! header says about backup memory.
//!
//! **SMD.** A 512-byte header (bytes 8 and 9 $AA $BB), then 16 KB blocks
//! of 8 KB of the even bytes and 8 KB of the odd ones (MacDonald's order;
//! as other notes give the halves the other way round, the order that
//! puts "SEGA" at $100 wins).
//!
//! **Header** (at $100): the system name ("SEGA MEGA DRIVE" / "SEGA
//! GENESIS"), at $1B0 "RA" with the backup RAM's type byte and its start
//! and end address (longs at $1B4 and $1B8); a start at an odd address
//! means the RAM sits on the odd bytes only. "SEGA SSF" as the system name
//! asks for the Super Street Fighter II mapper, as does an image larger
//! than 4 MB.
//!
//! Sources: Sega, *Genesis Software Manual* (the ROM header);
//! Charles MacDonald, *Sega Genesis hardware notes* (backup RAM at
//! $200001, EEPROM games) and *Super Magic Drive file format
//! information*. The implementation is this project's own.

use mimic_core::media::MediaError;

use crate::CartSlot;

/// The largest image: 10 MB (the SSF2 mapper's 64 × 512 KB would be more,
/// no cartridge has it).
pub const MAX_ROM: usize = 10 * 1024 * 1024;

/// Backup RAM as the header declares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackupRam {
    /// 68000 addresses, inclusive.
    pub start: u32,
    pub end: u32,
    /// Only the odd (or, if false and `even_only`, the even) bytes.
    pub odd_only: bool,
    pub even_only: bool,
}

impl BackupRam {
    /// Its size in bytes.
    pub fn len(&self) -> usize {
        let span = (self.end.saturating_sub(self.start) + 1) as usize;
        if self.odd_only || self.even_only {
            span.div_ceil(2)
        } else {
            span
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The RAM offset of `addr`, if it is a RAM byte.
    pub fn offset(&self, addr: u32) -> Option<usize> {
        if !(self.start..=self.end).contains(&addr) {
            return None;
        }
        let rel = (addr - self.start) as usize;
        if self.odd_only || self.even_only {
            let want_odd = self.odd_only;
            (((addr & 1) == 1) == want_odd).then_some(rel / 2)
        } else {
            Some(rel)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdCart {
    rom: Vec<u8>,
    pub backup: Option<BackupRam>,
    pub ssf2: bool,
}

impl MdCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let rom = if is_smd(bytes) {
            let even_first = deinterleave(&bytes[512..], true);
            if has_sega(&even_first) {
                even_first
            } else {
                deinterleave(&bytes[512..], false)
            }
        } else {
            bytes.to_vec()
        };
        if rom.len() < 0x200 {
            return Err(MediaError::Format("too small for a Mega Drive ROM".into()));
        }
        if rom.len() > MAX_ROM {
            return Err(MediaError::Format(format!(
                "a {} byte image is larger than 10 MB",
                rom.len()
            )));
        }
        let long = |a: usize| u32::from_be_bytes([rom[a], rom[a + 1], rom[a + 2], rom[a + 3]]);
        let backup = (&rom[0x1b0..0x1b2] == b"RA").then(|| {
            let start = long(0x1b4) & 0xff_ffff;
            let end = long(0x1b8) & 0xff_ffff;
            let kind = rom[0x1b2] >> 3 & 3;
            BackupRam {
                start,
                end: end.max(start),
                odd_only: start & 1 == 1 || kind == 3,
                even_only: kind == 2,
            }
        });
        let ssf2 = &rom[0x100..0x108] == b"SEGA SSF" || rom.len() > 0x40_0000;
        Ok(Self { rom, backup, ssf2 })
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// The ROM byte at `offset`, $FF past the end.
    pub fn byte(&self, offset: usize) -> u8 {
        self.rom.get(offset).copied().unwrap_or(0xff)
    }

    /// The region letters of the header at $1F0 ("J", "U", "E" or the
    /// newer hex digit).
    pub fn regions(&self) -> &[u8] {
        let r = &self.rom[0x1f0..0x1f3];
        let n = r.iter().position(|&c| c == b' ' || c == 0).unwrap_or(3);
        &r[..n]
    }
}

fn is_smd(b: &[u8]) -> bool {
    b.len() > 512 && (b.len() - 512).is_multiple_of(0x4000) && b[8] == 0xaa && b[9] == 0xbb
}

fn has_sega(rom: &[u8]) -> bool {
    rom.get(0x100..0x105)
        .is_some_and(|h| &h[..4] == b"SEGA" || &h[1..] == b"SEGA")
}

fn deinterleave(b: &[u8], even_first: bool) -> Vec<u8> {
    let (first, second) = if even_first { (0, 1) } else { (1, 0) };
    let mut out = vec![0; b.len()];
    for (block, o) in b.chunks(0x4000).zip(out.chunks_mut(0x4000)) {
        for i in 0..0x2000 {
            o[2 * i + first] = block[i];
            o[2 * i + second] = block[0x2000 + i];
        }
    }
    out
}

pub fn slot() -> CartSlot<MdCart> {
    CartSlot::new(MdCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Vec<u8> {
        let mut r = vec![0u8; 0x4000];
        r[0x100..0x10f].copy_from_slice(b"SEGA MEGA DRIVE");
        r[0x1b0..0x1bc]
            .copy_from_slice(&[0x52, 0x41, 0xf8, 0x20, 0, 0x20, 0, 1, 0, 0x20, 0x3f, 0xff]);
        r[0x1f0] = b'U';
        r[0x1f1] = b' ';
        r
    }

    #[test]
    fn the_header_declares_odd_byte_backup_ram() {
        let c = MdCart::parse(&image()).unwrap();
        let b = c.backup.unwrap();
        assert_eq!((b.start, b.end, b.odd_only), (0x20_0001, 0x20_3fff, true));
        assert_eq!(b.len(), 0x2000);
        assert_eq!(b.offset(0x20_0001), Some(0));
        assert_eq!(b.offset(0x20_0002), None);
        assert_eq!(b.offset(0x20_0003), Some(1));
        assert!(!c.ssf2);
        assert_eq!(c.regions(), b"U");
    }

    #[test]
    fn smd_images_are_deinterleaved() {
        let plain = image();
        let mut smd = vec![0u8; 512];
        smd[8] = 0xaa;
        smd[9] = 0xbb;
        let mut block = vec![0u8; 0x4000];
        for i in 0..0x2000 {
            block[i] = plain[2 * i];
            block[0x2000 + i] = plain[2 * i + 1];
        }
        smd.extend(&block);
        assert_eq!(MdCart::parse(&smd).unwrap().rom(), &plain[..]);
        // The other order of the halves is found by the header.
        let mut swapped = smd[..512].to_vec();
        swapped.extend(&block[0x2000..]);
        swapped.extend(&block[..0x2000]);
        assert_eq!(MdCart::parse(&swapped).unwrap().rom(), &plain[..]);
        assert!(MdCart::parse(&[0; 16]).is_err());
    }
}
