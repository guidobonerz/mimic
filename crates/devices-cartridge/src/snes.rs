//! SNES cartridges: LoROM, HiROM and ExHiROM boards with battery-backed
//! SRAM, from .sfc/.smc images (with or without a 512-byte copier header).
//!
//! **Detection.** The internal header sits at $7FC0 (LoROM), $FFC0 (HiROM)
//! or $40FFC0 (ExHiROM) of the image. Each candidate scores points for a
//! checksum and complement adding up to $FFFF, a map mode byte ($xFD5)
//! that names that layout, a reset vector in $8000–$FFFF and a printable
//! title; the best one wins.
//!
//! **Mapping** (24-bit CPU addresses):
//! - LoROM: ROM in 32 KB banks at $00–$7D/$80–$FF:$8000–$FFFF, mirrored at
//!   $40–$6F/$C0–$EF:$0000–$7FFF; SRAM at $70–$7D/$F0–$FF:$0000–$7FFF
//!   (without SRAM those are ROM mirrors too).
//! - HiROM: ROM in 64 KB banks at $40–$7D/$C0–$FF, the upper halves also
//!   at $00–$3F/$80–$BF:$8000–$FFFF; SRAM at $20–$3F/$A0–$BF:$6000–$7FFF.
//! - ExHiROM: banks $C0–$FF (and $80–$BF upper halves) show the first
//!   4 MB, $40–$7D (and $00–$3F) the rest; SRAM at $80–$BF:$6000–$7FFF.
//!
//! ROM sizes that are not a power of two repeat their last part the way
//! the boards wire them (the bigger chip first, then the smaller one,
//! mirrored up to the next power of two).
//!
//! Not supported (the image is refused): the coprocessor boards (DSP-n,
//! Super FX, SA-1, S-DD1, SPC7110, CX4 …) — the DSP-1 is planned.
//!
//! Sources: Martin Korth, fullsnes, "SNES Cartridge ROM Header", "SNES
//! Cart LoROM Mapping", "SNES Cart HiROM Mapping", "SNES Cartridge
//! ROM-Image Headers and File Extensions" (read 2026-10-23). The
//! implementation is this project's own.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnesMap {
    LoRom,
    HiRom,
    ExHiRom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnesCart {
    rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub map: SnesMap,
    pub title: String,
    /// The header asks for 3.58 MHz ROM ($xFD5 bit 4).
    pub fast: bool,
    /// The country code is a PAL one.
    pub pal: bool,
    pub battery: bool,
}

/// `addr` into an image of `size` bytes, laid out as the boards wire
/// ROMs whose size is not a power of two.
fn mirror(addr: usize, size: usize) -> usize {
    if size == 0 {
        return 0;
    }
    let (mut a, mut size, mut base) = (addr, size, 0);
    let mut mask = 1usize << 24;
    while a >= size {
        while a & mask == 0 {
            mask >>= 1;
        }
        a -= mask;
        if size > mask {
            size -= mask;
            base += mask;
        }
        mask >>= 1;
    }
    base + a
}

fn score(rom: &[u8], at: usize, map: SnesMap) -> i32 {
    let Some(h) = rom.get(at..at + 0x40) else {
        return i32::MIN;
    };
    let w = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let mut s = 0;
    if w(0x1c) ^ w(0x1e) == 0xffff {
        s += 4;
    }
    let mode = h[0x15] & 0x0f;
    let fits = match map {
        SnesMap::LoRom => matches!(mode, 0 | 2 | 3),
        SnesMap::HiRom => matches!(mode, 1 | 0xa),
        SnesMap::ExHiRom => mode == 5,
    };
    if fits {
        s += 2;
    }
    if h[0x15] & 0xe0 == 0x20 {
        s += 1;
    }
    let reset = w(0x3c);
    if reset >= 0x8000 {
        s += 2;
    } else {
        s -= 4;
    }
    if h[..21].iter().all(|c| (0x20..0x7f).contains(c)) {
        s += 1;
    }
    s
}

impl SnesCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let bad = |m: String| Err(MediaError::Format(m));
        let data = if bytes.len() & 0x3ff == 0x200 {
            &bytes[0x200..]
        } else {
            bytes
        };
        if data.len() < 0x8000 {
            return bad(format!(
                "SNES image of {} bytes (at least 32 KB)",
                data.len()
            ));
        }
        let mut candidates = vec![(0x7fc0, SnesMap::LoRom), (0xffc0, SnesMap::HiRom)];
        if data.len() > 0x40_0000 {
            candidates.push((0x40_ffc0, SnesMap::ExHiRom));
        }
        let (at, map) = candidates
            .into_iter()
            .max_by_key(|&(at, map)| score(data, at, map))
            .expect("candidates");
        if score(data, at, map) == i32::MIN {
            return bad("no SNES header found".into());
        }
        let h = &data[at..at + 0x40];
        // $00 ROM, $01 + RAM, $02 + RAM + battery; $x3 and up have a
        // coprocessor.
        let chipset = h[0x16];
        if chipset > 0x02 {
            return bad(format!(
                "SNES cartridge with a coprocessor (chipset ${chipset:02X}) is not supported"
            ));
        }
        let sram_kb = if h[0x18] == 0 || h[0x18] > 8 {
            0
        } else {
            1usize << h[0x18]
        };
        let title = String::from_utf8_lossy(&h[..21]).trim().to_string();
        Ok(Self {
            rom: data.to_vec(),
            sram: vec![0; sram_kb * 1024],
            map,
            title,
            fast: h[0x15] & 0x10 != 0,
            pal: (2..=12).contains(&h[0x19]),
            battery: chipset == 0x02,
        })
    }

    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    fn rom_at(&self, off: usize) -> u8 {
        self.rom[mirror(off, self.rom.len())]
    }

    fn sram_index(&self, bank: u8, addr: u16) -> Option<usize> {
        if self.sram.is_empty() {
            return None;
        }
        let n = self.sram.len();
        match self.map {
            SnesMap::LoRom => ((0x70..=0x7d).contains(&(bank & 0x7f)) && addr < 0x8000)
                .then(|| ((usize::from(bank & 0x0f) << 15) | usize::from(addr)) % n),
            SnesMap::HiRom => ((0x20..=0x3f).contains(&(bank & 0x7f))
                && (0x6000..0x8000).contains(&addr))
            .then(|| ((usize::from(bank & 0x1f) << 13) | usize::from(addr & 0x1fff)) % n),
            SnesMap::ExHiRom => ((0x80..=0xbf).contains(&bank) && (0x6000..0x8000).contains(&addr))
                .then(|| ((usize::from(bank & 0x1f) << 13) | usize::from(addr & 0x1fff)) % n),
        }
    }

    /// Side-effect-free read; `None` where the cartridge does not answer
    /// (open bus).
    pub fn peek(&self, addr: u32) -> Option<u8> {
        let bank = (addr >> 16) as u8;
        let a = addr as u16;
        if let Some(i) = self.sram_index(bank, a) {
            return Some(self.sram[i]);
        }
        let b = usize::from(bank & 0x7f);
        match self.map {
            SnesMap::LoRom => {
                if (0x7e..0x80).contains(&bank) || a < 0x8000 && b < 0x40 {
                    return None;
                }
                Some(self.rom_at((b << 15) | usize::from(a & 0x7fff)))
            }
            SnesMap::HiRom | SnesMap::ExHiRom => {
                let full = b >= 0x40 && !(0x7e..0x80).contains(&usize::from(bank));
                if !full && a < 0x8000 {
                    return None;
                }
                let mut off = (usize::from(bank & 0x3f) << 16) | usize::from(a);
                if self.map == SnesMap::ExHiRom && bank & 0x80 == 0 {
                    off += 0x40_0000;
                }
                Some(self.rom_at(off))
            }
        }
    }

    pub fn read(&mut self, addr: u32) -> Option<u8> {
        self.peek(addr)
    }

    pub fn write(&mut self, addr: u32, v: u8) {
        if let Some(i) = self.sram_index((addr >> 16) as u8, addr as u16) {
            self.sram[i] = v;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"SNSC", 1, |w| w.bytes(&self.sram));
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"SNSC", 1)?;
        let s = c.bytes()?;
        if s.len() != self.sram.len() {
            return Err(StateError::Invalid("SRAM size"));
        }
        self.sram.copy_from_slice(s);
        Ok(())
    }
}

/// The SNES cartridge slot.
pub fn slot() -> CartSlot<SnesCart> {
    CartSlot::new(SnesCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image of `len` bytes with a valid header for `map` at `at`.
    fn image(len: usize, map: SnesMap, sram_kb_log: u8) -> Vec<u8> {
        let mut rom = vec![0u8; len];
        for (i, b) in rom.iter_mut().enumerate() {
            *b = (i >> 15) as u8;
        }
        let at = match map {
            SnesMap::LoRom => 0x7fc0,
            SnesMap::HiRom => 0xffc0,
            SnesMap::ExHiRom => 0x40_ffc0,
        };
        rom[at..at + 21].copy_from_slice(b"TEST CARTRIDGE       ");
        rom[at + 0x15] = 0x20
            | match map {
                SnesMap::LoRom => 0,
                SnesMap::HiRom => 1,
                SnesMap::ExHiRom => 5,
            };
        rom[at + 0x16] = if sram_kb_log > 0 { 2 } else { 0 };
        rom[at + 0x18] = sram_kb_log;
        rom[at + 0x1c..at + 0x20].copy_from_slice(&[0xff, 0xff, 0x00, 0x00]);
        rom[at + 0x3c..at + 0x3e].copy_from_slice(&[0x00, 0x80]);
        rom
    }

    #[test]
    fn lorom_with_sram() {
        let mut c = SnesCart::parse(&image(0x10_0000, SnesMap::LoRom, 3)).unwrap();
        assert_eq!(c.map, SnesMap::LoRom);
        assert_eq!(c.title, "TEST CARTRIDGE");
        assert!(c.battery);
        assert_eq!(c.peek(0x00_8000), Some(0));
        assert_eq!(c.peek(0x01_8000), Some(1), "32 KB banks");
        assert_eq!(c.peek(0x81_8000), Some(1), "mirror at $80");
        assert_eq!(c.peek(0x00_2000), None);
        c.write(0x70_0005, 0x42);
        assert_eq!(c.peek(0x70_0005), Some(0x42));
        assert_eq!(c.peek(0xf0_0005), Some(0x42));
        assert_eq!(c.peek(0x70_2005), Some(0x42), "8 KB repeat");
    }

    #[test]
    fn hirom_and_copier_header() {
        let mut img = vec![0u8; 0x200];
        img.extend(image(0x20_0000, SnesMap::HiRom, 1));
        let mut c = SnesCart::parse(&img).unwrap();
        assert_eq!(c.map, SnesMap::HiRom);
        assert_eq!(c.peek(0xc1_0000), Some(2), "64 KB banks");
        assert_eq!(c.peek(0x01_8000), Some(3), "upper half at $8000");
        assert_eq!(c.peek(0x01_0000), None);
        c.write(0x30_6001, 7);
        assert_eq!(c.peek(0xb0_6001), Some(7));
    }

    #[test]
    fn odd_sizes_mirror_the_last_part() {
        // 3 MB = 2 MB + 1 MB mirrored: offset 3 MB reads 2 MB again.
        assert_eq!(mirror(0x30_0000, 0x30_0000), 0x20_0000);
        assert_eq!(mirror(0x10_0000, 0x30_0000), 0x10_0000);
        assert_eq!(mirror(0x50_0000, 0x40_0000), 0x10_0000);
        assert!(SnesCart::parse(&[0; 100]).is_err());
    }

    #[test]
    fn sram_savestate() {
        let mut c = SnesCart::parse(&image(0x8_0000, SnesMap::LoRom, 1)).unwrap();
        c.write(0x70_0001, 9);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = SnesCart::parse(&image(0x8_0000, SnesMap::LoRom, 1)).unwrap();
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d.sram, c.sram);
    }
}
