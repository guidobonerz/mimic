//! ZX81 memory: the 8 KB ROM at `$0000–$1FFF` (shadowed at `$2000–$3FFF`)
//! and the RAM from `$4000`: 1 KB on board, repeated through
//! `$4000–$7FFF`, or the 16 KB RAM pack. Address line A15 is not decoded,
//! so `$8000–$FFFF` repeats the lower half — which is how the ROM
//! "executes" the display file at its address + `$8000` (see the ULA).
//! ROM areas ignore writes.
//!
//! Source: Sinclair ZX81 Service Manual (memory decoding).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const ROM_SIZE: usize = 0x2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub rom: Vec<u8>,
    pub ram: Vec<u8>,
}

impl Memory {
    /// `ram_bytes`: a power of two between 1 KB and 16 KB.
    pub fn new(rom: Vec<u8>, ram_bytes: usize) -> Self {
        Self {
            rom,
            ram: vec![0; ram_bytes],
        }
    }

    /// Power-on: RAM cleared (the ROM initialises it anyway).
    pub fn clear(&mut self) {
        self.ram.fill(0);
    }

    fn ram_index(&self, addr: u16) -> usize {
        usize::from(addr & 0x3fff) & (self.ram.len() - 1)
    }

    pub fn read(&self, addr: u16) -> u8 {
        if addr & 0x4000 == 0 {
            self.rom[usize::from(addr) & (ROM_SIZE - 1)]
        } else {
            self.ram[self.ram_index(addr)]
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        if addr & 0x4000 != 0 {
            let i = self.ram_index(addr);
            self.ram[i] = v;
        }
    }
}

const TAG: [u8; 4] = *b"Z81M";

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u32(self.ram.len() as u32);
            w.raw(&self.ram);
        });
    }

    /// The ROM and the RAM size are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        if c.u32()? as usize != self.ram.len() {
            return Err(StateError::Invalid("RAM size differs"));
        }
        let mut ram = vec![0; self.ram.len()];
        c.raw_into(&mut ram)?;
        self.ram = ram;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rom_shadow_ram_mirrors_and_a15() {
        let mut rom = vec![0u8; ROM_SIZE];
        rom[0x1234] = 0xaa;
        let mut m = Memory::new(rom, 0x400);
        assert_eq!(m.read(0x1234), 0xaa);
        assert_eq!(m.read(0x3234), 0xaa, "ROM shadow");
        assert_eq!(m.read(0x9234), 0xaa, "A15 ignored");
        m.write(0x1234, 0);
        assert_eq!(m.read(0x1234), 0xaa, "ROM ignores writes");
        m.write(0x4009, 0x55);
        assert_eq!(m.read(0x4409), 0x55, "1 KB repeats");
        assert_eq!(m.read(0xc009), 0x55, "display file at +$8000");
        let m16 = Memory::new(vec![0; ROM_SIZE], 0x4000);
        assert_eq!(m16.ram.len(), 0x4000);
    }
}
