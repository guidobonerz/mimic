//! TRS-80 Model I memory map (TRS-80 Model I Technical Reference Manual,
//! memory map and address decoding):
//!
//! | Range | Contents |
//! | --- | --- |
//! | `$0000–$2FFF` | ROM: Level II BASIC (12 KB); Level I (4 KB) at `$0000–$0FFF`; Model III 14 KB to `$37FF` |
//! | `$37E0–$37FF` | Expansion Interface registers (see the machine) |
//! | `$3800–$3BFF` | keyboard matrix (see [`crate::keyboard`]) |
//! | `$3C00–$3FFF` | video RAM, 64 × 16 characters |
//! | `$4000–` | RAM: 4, 16 or 48 KB |
//!
//! Unmapped addresses read `$FF` (the data bus floats high). The Model I's
//! video RAM has seven bits: bit 6 is not stored, it reads back as the NOR
//! of bits 5 and 7 (the "lowercase modification" adds the eighth chip).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const VRAM_BASE: u16 = 0x3c00;
pub const VRAM_SIZE: usize = 0x400;
pub const RAM_BASE: u16 = 0x4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
    pub vram: Vec<u8>,
    /// The video RAM keeps bit 6 (lowercase modification, Model III).
    pub lowercase: bool,
}

impl Memory {
    pub fn new(rom: Vec<u8>, ram_bytes: usize, lowercase: bool) -> Self {
        Self {
            rom,
            ram: vec![0; ram_bytes],
            vram: vec![0; VRAM_SIZE],
            lowercase,
        }
    }

    /// Power-on: RAM and video RAM cleared.
    pub fn clear(&mut self) {
        self.ram.fill(0);
        self.vram.fill(0);
    }

    /// A video RAM byte as the CPU reads it back.
    pub fn vram_read(&self, i: usize) -> u8 {
        let v = self.vram[i & (VRAM_SIZE - 1)];
        if self.lowercase {
            v
        } else {
            let nor = (v & 0xa0 == 0) as u8;
            (v & !0x40) | nor << 6
        }
    }

    /// ROM, video RAM or RAM; `None` for the keyboard and I/O areas the bus
    /// decodes itself.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            // Model III: the 14 KB ROM reaches $37FF (its printer is on a port).
            0..=0x37ff => self.rom.get(usize::from(addr)).copied().unwrap_or(0xff),
            0x3c00..=0x3fff => self.vram_read(usize::from(addr - VRAM_BASE)),
            RAM_BASE.. => self
                .ram
                .get(usize::from(addr - RAM_BASE))
                .copied()
                .unwrap_or(0xff),
            _ => 0xff,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0x3c00..=0x3fff => {
                let v = if self.lowercase { v } else { v & !0x40 };
                self.vram[usize::from(addr - VRAM_BASE)] = v;
            }
            RAM_BASE.. => {
                if let Some(b) = self.ram.get_mut(usize::from(addr - RAM_BASE)) {
                    *b = v;
                }
            }
            _ => {}
        }
    }
}

const TAG: [u8; 4] = *b"T80M";

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u32(self.ram.len() as u32);
            w.raw(&self.ram);
            w.raw(&self.vram);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        if c.u32()? as usize != self.ram.len() {
            return Err(StateError::Invalid("RAM size differs"));
        }
        c.raw_into(&mut self.ram)?;
        c.raw_into(&mut self.vram)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_i_video_ram_has_seven_bits() {
        let mut m = Memory::new(vec![0; 0x3000], 0x4000, false);
        for (written, read) in [
            (0x41, 0x41),
            (0x20, 0x20),
            (0x61, 0x21),
            (0x80, 0x80),
            (0xc5, 0x85),
        ] {
            m.write(0x3c00, written);
            assert_eq!(m.read(0x3c00), read, "{written:#04x}");
        }
        m.lowercase = true;
        m.write(0x3c00, 0x61);
        assert_eq!(m.read(0x3c00), 0x61);
    }

    #[test]
    fn map() {
        let mut rom = vec![0; 0x3000];
        rom[0x2fff] = 0x12;
        let mut m = Memory::new(rom, 0x4000, false);
        assert_eq!(m.read(0x2fff), 0x12);
        assert_eq!(m.read(0x3000), 0xff);
        m.write(0x7fff, 0x34);
        assert_eq!(m.read(0x7fff), 0x34);
        assert_eq!(m.read(0x8000), 0xff, "16 KB end");
        m.write(0x1000, 0x55);
        assert_eq!(m.read(0x1000), 0, "ROM ignores writes");
    }
}
