//! C16/Plus/4 memory map and ROM banking.
//!
//! | Range         | Contents                                                |
//! | ------------- | ------------------------------------------------------- |
//! | `$0000/$0001` | 7501/8501 I/O port                                       |
//! | `$0002–$7FFF` | RAM                                                     |
//! | `$8000–$BFFF` | low ROM bank (BASIC, function ROM, cartridges) or RAM    |
//! | `$C000–$FCFF` | high ROM bank (KERNAL, function ROM, cartridges) or RAM |
//! | `$FC00–$FCFF` | KERNAL whenever ROM is on (bank switching code)         |
//! | `$FD00–$FF3F` | I/O (see [`crate`]), always                             |
//! | `$FF40–$FFFF` | high ROM bank or RAM                                     |
//!
//! Writing any value to TED `$FF3E` turns ROM on, `$FF3F` turns RAM on;
//! writes always go to RAM. A write to `$FDD0–$FDDF` selects the banks by
//! its address: bits 0–1 the low bank, bits 2–3 the high bank (0 BASIC /
//! KERNAL, 1 function ROM, 2 cartridge 1, 3 cartridge 2). The C16 has
//! 16 KB of RAM mirrored over the whole 64 KB. Source: Plus/4
//! Programmer's Reference, memory map. Empty ROM sockets read `$FF`.

use chips_cbm::ted::TedMemory;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const ROM_SIZE: usize = 0x4000;

/// One 16 KB ROM.
pub type Rom = Box<[u8; ROM_SIZE]>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub kernal: Rom,
    pub basic: Rom,
    /// The Plus/4's 3-plus-1 function ROM (low and high half).
    pub function: Option<(Rom, Rom)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    /// 64 KB backing store; the C16 uses its first 16 KB.
    pub ram: Vec<u8>,
    /// RAM address mask: `$3FFF` (16 KB) or `$FFFF` (64 KB).
    pub ram_mask: u16,
    pub roms: Roms,
    /// `$8000–$FFFF` read from ROM.
    pub rom_on: bool,
    /// Bank select: bits 0–1 low bank, bits 2–3 high bank.
    pub banks: u8,
}

/// What a CPU address selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Port,
    Ram,
    Rom,
    Io,
}

impl Memory {
    pub fn new(roms: Roms, ram_mask: u16) -> Self {
        Self {
            ram: vec![0; 0x10000],
            ram_mask,
            roms,
            rom_on: true,
            banks: 0,
        }
    }

    /// Power-on clears RAM; every reset returns to ROM, banks 0.
    pub fn reset(&mut self, hard: bool) {
        if hard {
            self.ram.fill(0);
        }
        self.rom_on = true;
        self.banks = 0;
    }

    pub fn area(&self, addr: u16) -> Area {
        match addr {
            0x0000 | 0x0001 => Area::Port,
            0xfd00..=0xff3f => Area::Io,
            0x8000..=0xffff if self.rom_on => Area::Rom,
            _ => Area::Ram,
        }
    }

    pub fn ram_read(&self, addr: u16) -> u8 {
        self.ram[usize::from(addr & self.ram_mask)]
    }

    pub fn ram_write(&mut self, addr: u16, v: u8) {
        self.ram[usize::from(addr & self.ram_mask)] = v;
    }

    /// ROM contents at `addr` (`$8000–$FFFF`) with the current banks.
    pub fn rom_read(&self, addr: u16) -> u8 {
        let i = usize::from(addr) & (ROM_SIZE - 1);
        if (0xfc00..=0xfcff).contains(&addr) {
            return self.roms.kernal[i];
        }
        let (bank, high) = if addr < 0xc000 {
            (self.banks & 3, false)
        } else {
            ((self.banks >> 2) & 3, true)
        };
        match (bank, &self.roms.function) {
            (0, _) if high => self.roms.kernal[i],
            (0, _) => self.roms.basic[i],
            (1, Some((lo, hi))) => {
                if high {
                    hi[i]
                } else {
                    lo[i]
                }
            }
            _ => 0xff,
        }
    }
}

/// The TED's view: RAM, or ROM for character data when it asks for it.
pub struct TedView<'a>(pub &'a Memory);

impl TedMemory for TedView<'_> {
    fn fetch(&self, addr: u16, rom: bool) -> u8 {
        if rom && addr >= 0x8000 {
            self.0.rom_read(addr)
        } else {
            self.0.ram_read(addr)
        }
    }
}

const TAG: [u8; 4] = *b"TEDM";
const VERSION: u16 = 1;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u16(self.ram_mask);
            w.bool(self.rom_on);
            w.u8(self.banks);
            w.raw(&self.ram);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        if c.u16()? != self.ram_mask {
            return Err(StateError::Invalid("RAM size differs"));
        }
        let rom_on = c.bool()?;
        let banks = c.u8()? & 0x0f;
        let mut ram = vec![0; 0x10000];
        c.raw_into(&mut ram)?;
        self.rom_on = rom_on;
        self.banks = banks;
        self.ram = ram;
        Ok(())
    }
}
