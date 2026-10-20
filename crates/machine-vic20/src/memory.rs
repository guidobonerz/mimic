//! VIC-20 memory map and RAM expansions.
//!
//! | Range         | Contents                                              |
//! | ------------- | ----------------------------------------------------- |
//! | `$0000–$03FF` | RAM (zero page, stack, system)                        |
//! | `$0400–$0FFF` | 3 KB expansion RAM (RAM1–3), if fitted                |
//! | `$1000–$1FFF` | RAM (BASIC and screen on an unexpanded machine)       |
//! | `$2000–$7FFF` | 8 KB expansion blocks 1–3, if fitted                  |
//! | `$8000–$8FFF` | character ROM                                         |
//! | `$9000–$93FF` | I/O: VIC, VIA1, VIA2 (see [`crate::io`])              |
//! | `$9400–$97FF` | colour RAM, 4 bits                                    |
//! | `$9800–$9FFF` | I/O2, I/O3 (expansion port)                           |
//! | `$A000–$BFFF` | block 5: cartridge ROM or expansion RAM               |
//! | `$C000–$DFFF` | BASIC ROM                                             |
//! | `$E000–$FFFF` | KERNAL ROM                                            |
//!
//! A cartridge's ROM in blocks 1–3 or 5 takes the place of expansion RAM
//! there (the VIC never sees it).
//!
//! Source: VIC-20 Programmer's Reference Guide, memory map. Unpopulated
//! addresses read the last byte on the data bus (open bus).

use chips_cbm::vic::VicMemory;
use devices_cartridge::vic20::VicCart;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const KERNAL_SIZE: usize = 0x2000;
pub const BASIC_SIZE: usize = 0x2000;
pub const CHARGEN_SIZE: usize = 0x1000;

/// Fitted RAM expansions.
pub mod expansion {
    /// 3 KB at `$0400–$0FFF`.
    pub const RAM123: u8 = 1 << 0;
    pub const BLK1: u8 = 1 << 1;
    pub const BLK2: u8 = 1 << 2;
    pub const BLK3: u8 = 1 << 3;
    pub const BLK5: u8 = 1 << 5;
}

/// The `ram` option: which expansions a configuration fits.
pub fn expansion_for(option: &str) -> Option<u8> {
    use expansion::*;
    Some(match option {
        "none" => 0,
        "3k" => RAM123,
        "8k" => BLK1,
        "16k" => BLK1 | BLK2,
        "24k" => BLK1 | BLK2 | BLK3,
        "all" => RAM123 | BLK1 | BLK2 | BLK3 | BLK5,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub kernal: Box<[u8; KERNAL_SIZE]>,
    pub basic: Box<[u8; BASIC_SIZE]>,
    pub chargen: Box<[u8; CHARGEN_SIZE]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    /// 64 KB backing store; only fitted ranges are visible.
    pub ram: Vec<u8>,
    pub color: Vec<u8>,
    pub roms: Roms,
    pub expansion: u8,
    /// The plugged-in cartridge (a copy of the slot's, set by the machine
    /// on power-up; ROM only, so not part of savestates).
    pub cart: Option<VicCart>,
}

/// What a CPU address selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Ram,
    Chargen,
    Io,
    Color,
    Basic,
    Kernal,
    /// Cartridge ROM.
    Cart,
    /// Nothing fitted: open bus.
    None,
}

impl Memory {
    pub fn new(roms: Roms, expansion: u8) -> Self {
        Self {
            ram: vec![0; 0x10000],
            color: vec![0; 0x400],
            roms,
            expansion,
            cart: None,
        }
    }

    /// Power-on clears RAM; a reset (the KERNAL's RAM test) keeps it.
    pub fn reset(&mut self, hard: bool) {
        if hard {
            self.ram.fill(0);
            self.color.fill(0);
        }
    }

    pub fn area(&self, addr: u16) -> Area {
        use expansion::*;
        let fitted = |bit: u8| {
            if self.expansion & bit != 0 {
                Area::Ram
            } else {
                Area::None
            }
        };
        let cart_rom = matches!(addr, 0x2000..=0x7fff | 0xa000..=0xbfff)
            && self.cart.as_ref().is_some_and(|c| c.peek(addr).is_some());
        if cart_rom {
            return Area::Cart;
        }
        match addr {
            0x0000..=0x03ff | 0x1000..=0x1fff => Area::Ram,
            0x0400..=0x0fff => fitted(RAM123),
            0x2000..=0x3fff => fitted(BLK1),
            0x4000..=0x5fff => fitted(BLK2),
            0x6000..=0x7fff => fitted(BLK3),
            0x8000..=0x8fff => Area::Chargen,
            0x9000..=0x93ff => Area::Io,
            0x9400..=0x97ff => Area::Color,
            0x9800..=0x9fff => Area::None,
            0xa000..=0xbfff => fitted(BLK5),
            0xc000..=0xdfff => Area::Basic,
            _ => Area::Kernal,
        }
    }
}

/// The VIC's view: its `$2000–$3FFF` are CPU `$0000–$1FFF`, its
/// `$0000–$1FFF` are CPU `$8000–$9FFF`.
pub struct VicView<'a>(pub &'a Memory);

impl VicMemory for VicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        let cpu = if addr & 0x2000 != 0 {
            addr & 0x1fff
        } else {
            0x8000 | (addr & 0x1fff)
        };
        match self.0.area(cpu) {
            Area::Ram => self.0.ram[usize::from(cpu)],
            Area::Chargen => self.0.roms.chargen[usize::from(cpu & 0x0fff)],
            // I/O and empty sockets: the pull-ups on the VIC's data bus.
            _ => 0xff,
        }
    }

    fn color(&self, index: u16) -> u8 {
        self.0.color[usize::from(index & 0x3ff)]
    }
}

const TAG: [u8; 4] = *b"V20M";
const VERSION: u16 = 1;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.expansion);
            w.raw(&self.ram);
            w.raw(&self.color);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        if c.u8()? != self.expansion {
            return Err(StateError::Invalid("RAM expansion differs"));
        }
        let mut ram = vec![0; 0x10000];
        c.raw_into(&mut ram)?;
        let mut color = vec![0; 0x400];
        c.raw_into(&mut color)?;
        for v in &mut color {
            *v &= 0x0f;
        }
        self.ram = ram;
        self.color = color;
        Ok(())
    }
}
