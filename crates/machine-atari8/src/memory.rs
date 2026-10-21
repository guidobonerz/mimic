//! 800XL memory map: 64 KB RAM with ROMs banked in by PIA port B.
//!
//! | Range         | Content                                            |
//! | ------------- | -------------------------------------------------- |
//! | `$0000–$4FFF` | RAM                                                |
//! | `$5000–$57FF` | self-test ROM if PORTB bit 7 = 0 and bit 0 = 1, else RAM |
//! | `$5800–$7FFF` | RAM                                                |
//! | `$8000–$9FFF` | cartridge (RD4), else RAM                           |
//! | `$A000–$BFFF` | cartridge (RD5), else BASIC ROM if PORTB bit 1 = 0, else RAM |
//! | `$C000–$CFFF` | OS ROM if PORTB bit 0 = 1, else RAM                |
//! | `$D000–$D7FF` | I/O: GTIA, PBI, POKEY, PIA, ANTIC, cartridge control |
//! | `$D800–$FFFF` | OS ROM (floating point, OS) if PORTB bit 0 = 1, else RAM |
//!
//! The 16 KB OS ROM holds `$C000–$CFFF`, then the self-test (`$D000–$D7FF`
//! in the ROM, shown at `$5000`), then `$D800–$FFFF`. Writes to a ROM area
//! go nowhere. ANTIC's DMA sees the same map (800XL; the 130XE differs).
//! Source: Atari 800XL Field Service Manual; Altirra Hardware Reference
//! Manual, "Memory map".

use devices_cartridge::atari::AtariCart;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const OS_SIZE: usize = 0x4000;
pub const BASIC_SIZE: usize = 0x2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub os: Box<[u8; OS_SIZE]>,
    /// The built-in BASIC; without it an enabled BASIC area floats ($FF).
    pub basic: Option<Box<[u8; BASIC_SIZE]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Ram,
    /// Offset into the OS ROM image.
    Os(u16),
    Basic,
    Cart,
    Io,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub ram: Box<[u8; 0x10000]>,
    pub roms: Roms,
    /// PIA port B pin levels (inputs pulled up): the banking bits.
    pub portb: u8,
    /// The cartridge in the slot, with its banking state.
    pub cart: Option<AtariCart>,
}

impl Memory {
    pub fn new(roms: Roms) -> Self {
        Self {
            ram: Box::new([0; 0x10000]),
            roms,
            portb: 0xff,
            cart: None,
        }
    }

    /// Power-up clears RAM (real DRAM starts with a pattern; zeros keep runs
    /// deterministic).
    pub fn reset(&mut self, hard: bool) {
        if hard {
            self.ram.fill(0);
        }
        self.portb = 0xff;
        if hard {
            if let Some(c) = &mut self.cart {
                c.reset();
            }
        }
    }

    fn os_on(&self) -> bool {
        self.portb & 0x01 != 0
    }

    pub fn area(&self, addr: u16) -> Area {
        match addr {
            0x5000..=0x57ff if self.os_on() && self.portb & 0x80 == 0 => Area::Os(addr - 0x4000),
            0x8000..=0x9fff if self.cart.as_ref().is_some_and(AtariCart::rd4) => Area::Cart,
            0xa000..=0xbfff if self.cart.as_ref().is_some_and(AtariCart::rd5) => Area::Cart,
            0xa000..=0xbfff if self.portb & 0x02 == 0 => Area::Basic,
            0xc000..=0xcfff if self.os_on() => Area::Os(addr - 0xc000),
            0xd000..=0xd7ff => Area::Io,
            0xd800..=0xffff if self.os_on() => Area::Os(addr - 0xc000),
            _ => Area::Ram,
        }
    }

    /// Read outside I/O.
    pub fn read_plain(&self, addr: u16) -> u8 {
        match self.area(addr) {
            Area::Ram => self.ram[usize::from(addr)],
            Area::Os(off) => self.roms.os[usize::from(off)],
            Area::Basic => self
                .roms
                .basic
                .as_ref()
                .map_or(0xff, |b| b[usize::from(addr - 0xa000)]),
            Area::Cart => self
                .cart
                .as_ref()
                .and_then(|c| c.read(addr))
                .unwrap_or(0xff),
            Area::Io => 0xff,
        }
    }

    /// Write outside I/O; ROM areas ignore it.
    pub fn write_plain(&mut self, addr: u16, v: u8) {
        if self.area(addr) == Area::Ram {
            self.ram[usize::from(addr)] = v;
        }
    }
}

const TAG: [u8; 4] = *b"A8MM";
const VERSION: u16 = 1;

/// RAM and the banking state; ROMs are configuration.
impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.portb);
            w.raw(&self.ram[..]);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let portb = c.u8()?;
        let mut ram = Box::new([0u8; 0x10000]);
        c.raw_into(&mut ram[..])?;
        self.portb = portb;
        self.ram = ram;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Memory {
        let mut os = Box::new([0u8; OS_SIZE]);
        os[0x0000] = 0xc0; // $C000
        os[0x1000] = 0x50; // self-test at $5000
        os[0x1800] = 0xd8; // $D800
        let mut basic = Box::new([0u8; BASIC_SIZE]);
        basic[0] = 0xba;
        Memory::new(Roms {
            os,
            basic: Some(basic),
        })
    }

    #[test]
    fn portb_banking() {
        let mut m = mem();
        m.ram[0xc000] = 0x11;
        m.ram[0xa000] = 0x22;
        m.ram[0x5000] = 0x33;
        // Power-up: inputs pulled up — OS on, BASIC and self-test off.
        assert_eq!(m.read_plain(0xc000), 0xc0);
        assert_eq!(m.read_plain(0xd800), 0xd8);
        assert_eq!(m.read_plain(0xa000), 0x22);
        assert_eq!(m.read_plain(0x5000), 0x33);
        m.portb = 0xfd; // BASIC on
        assert_eq!(m.read_plain(0xa000), 0xba);
        m.portb = 0x7f; // self-test on
        assert_eq!(m.read_plain(0x5000), 0x50);
        m.portb = 0xfe; // OS off: RAM everywhere but I/O
        assert_eq!(m.read_plain(0xc000), 0x11);
        assert_eq!(m.area(0xd000), Area::Io);
        m.write_plain(0xc000, 0x44);
        assert_eq!(m.read_plain(0xc000), 0x44);
        m.portb = 0xff;
        m.write_plain(0xc000, 0x55);
        assert_eq!(m.ram[0xc000], 0x44, "ROM area ignores writes");
    }

    #[test]
    fn missing_basic_floats() {
        let mut m = mem();
        m.roms.basic = None;
        m.portb = 0xfd;
        assert_eq!(m.read_plain(0xbffc), 0xff);
    }
}
