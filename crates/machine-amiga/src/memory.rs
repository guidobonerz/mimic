//! The A500 address map (HRM appendix "Memory map"; Gary's decoding):
//!
//! | Range | Contents |
//! | --- | --- |
//! | $000000–$1FFFFF | chip RAM (mirrored); while OVL is high the Kickstart ROM reads at $000000–$07FFFF (writes still reach chip RAM) |
//! | $A00000–$BFFFFF | CIAs: A12 = 0 selects CIA-A (odd bytes, D0–D7), A13 = 0 CIA-B (even bytes, D8–D15); register = A11–A8 |
//! | $C00000–$D7FFFF | slow ("trapdoor") RAM; above it the custom registers appear mirrored (Kickstart finds the end of slow RAM that way) |
//! | $DFF000–$DFFFFF | custom chip registers |
//! | $F80000–$FFFFFF | Kickstart ROM (256 KB images repeat) |
//!
//! Everything else reads 0 and ignores writes.

use chips_amiga::ChipRam;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Kickstart image size in the map.
pub const ROM_SIZE: usize = 0x8_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Chip,
    Slow,
    Custom,
    Cia,
    Rom,
    Open,
}

impl Region {
    /// Accesses that go over the chip bus and wait for a free DMA slot.
    pub fn chip_bus(self) -> bool {
        matches!(self, Region::Chip | Region::Slow | Region::Custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub chip: ChipRam,
    /// Slow RAM words (empty: none fitted).
    pub slow: Vec<u16>,
    /// 512 KB of ROM words.
    pub rom: Vec<u16>,
    /// OVL: the ROM overlays address 0.
    pub overlay: bool,
}

impl Memory {
    /// `rom`: a 256 or 512 KB Kickstart image.
    pub fn new(chip_bytes: usize, slow_bytes: usize, rom: &[u8]) -> Self {
        let mut words: Vec<u16> = rom
            .as_chunks::<2>()
            .0
            .iter()
            .map(|w| u16::from_be_bytes(*w))
            .collect();
        while words.len() < ROM_SIZE / 2 {
            let again = words.clone();
            words.extend(again);
        }
        words.truncate(ROM_SIZE / 2);
        Self {
            chip: ChipRam::new(chip_bytes),
            slow: vec![0; slow_bytes / 2],
            rom: words,
            overlay: true,
        }
    }

    pub fn region(&self, addr: u32, write: bool) -> Region {
        let a = addr & 0xff_ffff;
        match a {
            0x00_0000..=0x07_ffff if self.overlay && !write => Region::Rom,
            0x00_0000..=0x1f_ffff => Region::Chip,
            0xa0_0000..=0xbf_ffff => Region::Cia,
            0xc0_0000..=0xd7_ffff => {
                if ((a - 0xc0_0000) as usize) < self.slow.len() * 2 {
                    Region::Slow
                } else {
                    Region::Custom
                }
            }
            0xdf_f000..=0xdf_ffff => Region::Custom,
            0xf8_0000..=0xff_ffff => Region::Rom,
            _ => Region::Open,
        }
    }

    pub fn rom_word(&self, addr: u32) -> u16 {
        self.rom[(addr as usize >> 1) & (ROM_SIZE / 2 - 1)]
    }

    pub fn slow_word(&self, addr: u32) -> u16 {
        let i = ((addr & 0xff_ffff) - 0xc0_0000) as usize >> 1;
        self.slow.get(i).copied().unwrap_or(0)
    }

    pub fn set_slow_word(&mut self, addr: u32, v: u16) {
        let i = ((addr & 0xff_ffff) - 0xc0_0000) as usize >> 1;
        if let Some(w) = self.slow.get_mut(i) {
            *w = v;
        }
    }

    pub fn set_slow_byte(&mut self, addr: u32, v: u8) {
        let w = self.slow_word(addr & !1);
        let w = if addr & 1 == 0 {
            (w & 0x00ff) | u16::from(v) << 8
        } else {
            (w & 0xff00) | u16::from(v)
        };
        self.set_slow_word(addr & !1, w);
    }

    /// Power-on: RAM cleared.
    pub fn clear(&mut self) {
        self.chip.clear();
        self.slow.fill(0);
    }
}

const TAG: [u8; 4] = *b"AMEM";

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.bool(self.overlay);
            w.u32(self.slow.len() as u32);
            for v in &self.slow {
                w.u16(*v);
            }
        });
        self.chip.save(w);
    }

    /// Atomic; the ROM is configuration and not saved.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let overlay = c.bool()?;
        if c.u32()? as usize != self.slow.len() {
            return Err(StateError::Invalid("slow RAM size differs"));
        }
        let mut slow = vec![0; self.slow.len()];
        for v in &mut slow {
            *v = c.u16()?;
        }
        let mut chip = self.chip.clone();
        chip.load(r)?;
        self.chip = chip;
        self.slow = slow;
        self.overlay = overlay;
        Ok(())
    }
}
