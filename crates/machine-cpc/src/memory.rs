//! CPC memory: 64 KB RAM (464, 664) or 128 KB (6128), the 16 KB lower ROM
//! (OS) at &0000 and upper ROMs at &C000, enabled by the Gate Array's RMR.
//! Writes always go to RAM. Video reads the first 64 KB.
//!
//! Upper ROMs are selected by a write to &DFxx: 7 is the disc ROM (AMSDOS
//! or ParaDOS) where fitted; every other number, like an empty slot on the
//! real machine, gives BASIC.
//!
//! The 6128's extra 64 KB is mapped in by its PAL (port &7Fxx, bits 7–6 =
//! 11, bits 2–0 the configuration; cpcwiki, "Gate Array"):
//!
//! | Config | &0000 | &4000 | &8000 | &C000 |
//! | ------ | ----- | ----- | ----- | ----- |
//! | 0      | 0     | 1     | 2     | 3     |
//! | 1      | 0     | 1     | 2     | 7     |
//! | 2      | 4     | 5     | 6     | 7     |
//! | 3      | 0     | 3     | 2     | 7     |
//! | 4–7    | 0     | 4–7   | 2     | 3     |

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const ROM_SIZE: usize = 0x4000;

const CONFIGS: [[u8; 4]; 8] = [
    [0, 1, 2, 3],
    [0, 1, 2, 7],
    [4, 5, 6, 7],
    [0, 3, 2, 7],
    [0, 4, 2, 3],
    [0, 5, 2, 3],
    [0, 6, 2, 3],
    [0, 7, 2, 3],
];

pub type Rom = Box<[u8; ROM_SIZE]>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub os: Rom,
    pub basic: Rom,
    /// Upper ROM 7, the disc ROM.
    pub dos: Option<Rom>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    ram: Vec<u8>,
    roms: Roms,
    /// Selected upper ROM.
    pub upper: u8,
    /// RAM configuration (6128).
    pub config: u8,
}

impl Memory {
    pub fn new(roms: Roms, ram_kb: usize) -> Self {
        Self {
            ram: vec![0; ram_kb * 1024],
            roms,
            upper: 0,
            config: 0,
        }
    }

    /// Power-up: RAM cleared, configuration 0, upper ROM 0.
    pub fn reset(&mut self, hard: bool) {
        self.upper = 0;
        self.config = 0;
        if hard {
            self.ram.fill(0);
        }
    }

    pub fn extra(&self) -> bool {
        self.ram.len() > 0x10000
    }

    fn ram_index(&self, addr: u16) -> usize {
        let page = usize::from(addr >> 14);
        let bank = if self.extra() {
            usize::from(CONFIGS[usize::from(self.config & 7)][page])
        } else {
            page
        };
        bank * 0x4000 + usize::from(addr & 0x3fff)
    }

    /// The selected upper ROM.
    fn upper_rom(&self) -> &[u8; ROM_SIZE] {
        match (&self.roms.dos, self.upper) {
            (Some(dos), 7) => dos,
            _ => &self.roms.basic,
        }
    }

    /// A CPU read with the Gate Array's ROM enables.
    pub fn read(&self, addr: u16, lower: bool, upper: bool) -> u8 {
        match addr >> 14 {
            0 if lower => self.roms.os[usize::from(addr)],
            3 if upper => self.upper_rom()[usize::from(addr & 0x3fff)],
            _ => self.ram[self.ram_index(addr)],
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        let i = self.ram_index(addr);
        self.ram[i] = v;
    }

    /// The 64 KB the Gate Array shows.
    pub fn video(&self) -> &[u8] {
        &self.ram[..0x10000]
    }
}

const TAG: [u8; 4] = *b"CMEM";
const VERSION: u16 = 1;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.upper);
            w.u8(self.config);
            w.bytes(&self.ram);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let upper = c.u8()?;
        let config = c.u8()? & 7;
        let ram = c.bytes()?;
        if ram.len() != self.ram.len() {
            return Err(StateError::Invalid("CPC RAM size differs"));
        }
        self.upper = upper;
        self.config = config;
        self.ram.copy_from_slice(ram);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roms() -> Roms {
        Roms {
            os: Box::new([1; ROM_SIZE]),
            basic: Box::new([2; ROM_SIZE]),
            dos: Some(Box::new([3; ROM_SIZE])),
        }
    }

    #[test]
    fn roms_over_ram_and_upper_selection() {
        let mut m = Memory::new(roms(), 64);
        m.write(0x0000, 9);
        m.write(0xc000, 8);
        assert_eq!(m.read(0x0000, true, true), 1);
        assert_eq!(m.read(0x0000, false, true), 9);
        assert_eq!(m.read(0xc000, true, true), 2, "BASIC");
        m.upper = 7;
        assert_eq!(m.read(0xc000, true, true), 3, "AMSDOS");
        m.upper = 5;
        assert_eq!(m.read(0xc000, true, true), 2, "empty slot: BASIC");
        assert_eq!(m.read(0xc000, true, false), 8);
    }

    #[test]
    fn ram_configurations() {
        let mut m = Memory::new(roms(), 128);
        m.write(0x4000, 0x11); // bank 1
        m.config = 2;
        m.write(0x4000, 0x55); // bank 5
        assert_eq!(m.video()[0x4000], 0x11, "video sees bank 1");
        m.config = 5;
        assert_eq!(m.read(0x4000, false, false), 0x55);
        m.config = 0;
        assert_eq!(m.read(0x4000, false, false), 0x11);
    }
}
