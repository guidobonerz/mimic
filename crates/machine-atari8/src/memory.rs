//! Memory maps of the Atari 8-bit models.
//!
//! **400/800** (`Kind::Old`): 8–48 KB RAM from `$0000`, nothing (`$FF`) up
//! to the I/O area, the 10 KB OS-A/OS-B ROM at `$D800–$FFFF`. No banking:
//! PIA port B is joystick ports 3 and 4. Cartridges sit in the left slot
//! (`$A000–$BFFF`, RD5) and the right slot (`$8000–$9FFF`, RD4); a built-in
//! BASIC does not exist, the BASIC cartridge goes into the left slot.
//!
//! **800XL / 65XE** (`Kind::Xl`): 64 KB RAM with ROMs banked in by PIA
//! port B.
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
//! go nowhere.
//!
//! **130XE** (`Kind::Xe130`): the XL map plus 64 KB of extended RAM in four
//! 16 KB banks that appear at `$4000–$7FFF`: PORTB bit 4 = 0 switches the
//! CPU's accesses there to the bank in bits 2–3, bit 5 = 0 does the same
//! for ANTIC's DMA, independently (so ANTIC can show a bank the CPU
//! does not see). With both bits 1 the main RAM is there for both.
//!
//! **320 KB expansions** of an XL/XE (`--option ram=`), 256 KB in 16 banks
//! at the same place. `Kind::Rambo320`: bank in PORTB bits 2, 3, 5, 6; bit 4
//! switches the bank in for the CPU and ANTIC alike. `Kind::Compy320`
//! (Compy Shop): bank in bits 2, 3, 6, 7; bits 4 and 5 as on the 130XE, and
//! bit 7 selects the self-test only while the bank is off.
//!
//! Sources: Atari 800XL Field Service Manual; Atari 130XE Field Service
//! Manual (PORTB bit assignment); Altirra Hardware Reference Manual,
//! "Memory map" and "PORTB"; Atari 400/800 Operating System User's Manual
//! (OS ROM placement, cartridge slots); the 320 KB bit assignments from the
//! srambo project (https://github.com/piotr-wiszowaty/srambo_1) and AtariAge
//! (topic 210856), which agree; the bit 7/self-test rule is this project's
//! assumption.

use devices_cartridge::atari::AtariCart;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// XL/XE OS ROM size.
pub const OS_SIZE: usize = 0x4000;
/// 400/800 OS-A/OS-B ROM size.
pub const OS_AB_SIZE: usize = 0x2800;
pub const BASIC_SIZE: usize = 0x2000;
/// Extended RAM of the 130XE.
pub const XE_BANKS: usize = 4;
pub const XE_BANK_SIZE: usize = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 400/800 with this much RAM (8, 16, 32 or 48 KB).
    Old {
        ram: usize,
    },
    Xl,
    Xe130,
    Rambo320,
    Compy320,
}

impl Kind {
    /// Banks of 16 KB of extended RAM.
    fn banks(self) -> usize {
        match self {
            Kind::Old { .. } | Kind::Xl => 0,
            Kind::Xe130 => XE_BANKS,
            Kind::Rambo320 | Kind::Compy320 => 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    /// The OS ROM: 16 KB (XL/XE) or 10 KB (400/800).
    pub os: Vec<u8>,
    /// The built-in BASIC (XL/XE); without it an enabled BASIC area
    /// floats ($FF). On the 400/800 BASIC is a cartridge (see `Memory::new`).
    pub basic: Option<Box<[u8; BASIC_SIZE]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Ram,
    /// Extended RAM: bank, offset.
    Extended(u8, u16),
    /// Offset into the OS ROM image.
    Os(u16),
    Basic,
    Cart,
    Io,
    /// Nothing answers (400/800 above the fitted RAM, `$C000–$CFFF`).
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub kind: Kind,
    pub ram: Box<[u8; 0x10000]>,
    /// Extended RAM (empty without any).
    pub extended: Vec<u8>,
    pub roms: Roms,
    /// PIA port B pin levels (inputs pulled up): the banking bits on the
    /// XL/XE, joystick ports 3 and 4 on the 400/800.
    pub portb: u8,
    /// The cartridge in the slot, with its banking state.
    pub cart: Option<AtariCart>,
}

impl Memory {
    pub fn new(kind: Kind, roms: Roms) -> Self {
        Self {
            kind,
            ram: Box::new([0; 0x10000]),
            extended: vec![0; kind.banks() * XE_BANK_SIZE],
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
            self.extended.fill(0);
        }
        self.portb = 0xff;
        if hard {
            if let Some(c) = &mut self.cart {
                c.reset();
            }
        }
    }

    /// Does PIA port B bank memory on this model?
    pub fn banks_with_portb(&self) -> bool {
        !matches!(self.kind, Kind::Old { .. })
    }

    fn os_on(&self) -> bool {
        self.portb & 0x01 != 0
    }

    /// The bank CPU (`antic = false`) or ANTIC accesses see at
    /// `$4000–$7FFF`, if the extended RAM is switched in for it.
    fn xe_bank(&self, antic: bool) -> Option<u8> {
        let b = self.portb;
        let low = (b >> 2) & 3;
        let enable = if antic { 0x20 } else { 0x10 };
        match self.kind {
            Kind::Old { .. } | Kind::Xl => None,
            Kind::Xe130 => (b & enable == 0).then_some(low),
            Kind::Rambo320 => (b & 0x10 == 0).then_some(low | ((b >> 3) & 0x0c)),
            Kind::Compy320 => (b & enable == 0).then_some(low | ((b >> 4) & 0x0c)),
        }
    }

    /// The area an access by the CPU (`antic = false`) or ANTIC hits.
    pub fn area_for(&self, addr: u16, antic: bool) -> Area {
        if let Kind::Old { ram } = self.kind {
            return match addr {
                0x8000..=0x9fff if self.cart.as_ref().is_some_and(AtariCart::rd4) => Area::Cart,
                0xa000..=0xbfff if self.cart.as_ref().is_some_and(AtariCart::rd5) => Area::Cart,
                0xd000..=0xd7ff => Area::Io,
                0xd800..=0xffff => Area::Os(addr - 0xd800),
                a if usize::from(a) < ram => Area::Ram,
                _ => Area::Open,
            };
        }
        match addr {
            0x5000..=0x57ff
                if self.os_on()
                    && self.portb & 0x80 == 0
                    && !(self.kind == Kind::Compy320 && self.xe_bank(antic).is_some()) =>
            {
                Area::Os(addr - 0x4000)
            }
            0x4000..=0x7fff => match self.xe_bank(antic) {
                Some(bank) => Area::Extended(bank, addr - 0x4000),
                None => Area::Ram,
            },
            0x8000..=0x9fff if self.cart.as_ref().is_some_and(AtariCart::rd4) => Area::Cart,
            0xa000..=0xbfff if self.cart.as_ref().is_some_and(AtariCart::rd5) => Area::Cart,
            0xa000..=0xbfff if self.portb & 0x02 == 0 => Area::Basic,
            0xc000..=0xcfff if self.os_on() => Area::Os(addr - 0xc000),
            0xd000..=0xd7ff => Area::Io,
            0xd800..=0xffff if self.os_on() => Area::Os(addr - 0xc000),
            _ => Area::Ram,
        }
    }

    /// The CPU's area.
    pub fn area(&self, addr: u16) -> Area {
        self.area_for(addr, false)
    }

    fn read_area(&self, area: Area, addr: u16) -> u8 {
        match area {
            Area::Ram => self.ram[usize::from(addr)],
            Area::Extended(bank, off) => {
                self.extended[usize::from(bank) * XE_BANK_SIZE + usize::from(off)]
            }
            Area::Os(off) => self.roms.os.get(usize::from(off)).copied().unwrap_or(0xff),
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
            Area::Io | Area::Open => 0xff,
        }
    }

    /// CPU read outside I/O.
    pub fn read_plain(&self, addr: u16) -> u8 {
        self.read_area(self.area(addr), addr)
    }

    /// ANTIC's DMA read.
    pub fn read_dma(&self, addr: u16) -> u8 {
        self.read_area(self.area_for(addr, true), addr)
    }

    /// CPU write outside I/O; ROM and open areas ignore it.
    pub fn write_plain(&mut self, addr: u16, v: u8) {
        match self.area(addr) {
            Area::Ram => self.ram[usize::from(addr)] = v,
            Area::Extended(bank, off) => {
                self.extended[usize::from(bank) * XE_BANK_SIZE + usize::from(off)] = v;
            }
            _ => {}
        }
    }
}

const TAG: [u8; 4] = *b"A8MM";
/// Version 2: the 130XE's extended RAM follows the main RAM.
const VERSION: u16 = 2;

/// RAM and the banking state; ROMs and the model are configuration.
impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.portb);
            w.raw(&self.ram[..]);
            w.u32(self.extended.len() as u32);
            w.raw(&self.extended);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let portb = c.u8()?;
        let mut ram = Box::new([0u8; 0x10000]);
        c.raw_into(&mut ram[..])?;
        let mut extended = vec![0; self.extended.len()];
        if version >= 2 {
            if c.u32()? as usize != extended.len() {
                return Err(StateError::Invalid("extended RAM size differs"));
            }
            c.raw_into(&mut extended)?;
        } else if !extended.is_empty() {
            return Err(StateError::Invalid("state without extended RAM"));
        }
        self.portb = portb;
        self.ram = ram;
        self.extended = extended;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xl_roms() -> Roms {
        let mut os = vec![0u8; OS_SIZE];
        os[0x0000] = 0xc0; // $C000
        os[0x1000] = 0x50; // self-test at $5000
        os[0x1800] = 0xd8; // $D800
        let mut basic = Box::new([0u8; BASIC_SIZE]);
        basic[0] = 0xba;
        Roms {
            os,
            basic: Some(basic),
        }
    }

    fn mem() -> Memory {
        Memory::new(Kind::Xl, xl_roms())
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

    #[test]
    fn the_400_800_map_has_no_banking() {
        let mut os = vec![0u8; OS_AB_SIZE];
        os[0] = 0xd8;
        os[OS_AB_SIZE - 1] = 0xee;
        let mut m = Memory::new(Kind::Old { ram: 0x4000 }, Roms { os, basic: None });
        assert!(!m.banks_with_portb());
        m.write_plain(0x3fff, 0x12);
        assert_eq!(m.read_plain(0x3fff), 0x12);
        m.write_plain(0x4000, 0x34);
        assert_eq!(m.read_plain(0x4000), 0xff, "above the fitted RAM");
        assert_eq!(m.area(0x4000), Area::Open);
        assert_eq!(m.read_plain(0xc000), 0xff, "nothing at $C000");
        assert_eq!(m.read_plain(0xd800), 0xd8);
        assert_eq!(m.read_plain(0xffff), 0xee);
        assert_eq!(m.area(0xd301), Area::Io);
        // PORTB bits mean nothing here: the OS stays, no self-test, no BASIC.
        m.portb = 0x00;
        assert_eq!(m.read_plain(0xd800), 0xd8);
        assert_eq!(m.read_plain(0x5000), 0xff);
        assert_eq!(m.read_plain(0xa000), 0xff);
    }

    #[test]
    fn the_130xe_banks_cpu_and_antic_separately() {
        let mut m = Memory::new(Kind::Xe130, xl_roms());
        m.ram[0x4000] = 0x11;
        // Both bits 4 and 5 set: main RAM for both.
        assert_eq!(m.read_plain(0x4000), 0x11);
        assert_eq!(m.read_dma(0x4000), 0x11);
        // CPU into bank 2 (bits 2-3 = 10), ANTIC stays on main RAM.
        m.portb = 0xeb;
        m.write_plain(0x4000, 0x22);
        assert_eq!(m.area(0x4000), Area::Extended(2, 0));
        assert_eq!(m.read_plain(0x4000), 0x22);
        assert_eq!(m.read_dma(0x4000), 0x11, "ANTIC still sees main RAM");
        assert_eq!(m.extended[2 * XE_BANK_SIZE], 0x22);
        // ANTIC into bank 2 too, CPU back to main RAM.
        m.portb = 0xdb;
        assert_eq!(m.read_plain(0x4000), 0x11);
        assert_eq!(m.read_dma(0x4000), 0x22);
        // Other banks are separate; the window ends at $7FFF.
        m.portb = 0xe3;
        assert_eq!(m.read_plain(0x4000), 0, "bank 0");
        assert_eq!(m.area(0x8000), Area::Ram);
        // Self-test and OS banking still work on the XE.
        m.portb = 0x7f;
        assert_eq!(m.read_plain(0x5000), 0x50);
        // Savestates carry the extended RAM and refuse other sizes.
        let mut w = StateWriter::new();
        m.save(&mut w);
        let s = w.into_bytes();
        let mut back = Memory::new(Kind::Xe130, xl_roms());
        back.load(&mut StateReader::new(&s)).unwrap();
        assert_eq!(back.extended, m.extended);
        let mut xl = mem();
        assert!(xl.load(&mut StateReader::new(&s)).is_err());
    }

    #[test]
    fn the_320k_expansions_decode_their_banks() {
        // PORTB value with the CPU bank on: a distinct bank per bit pattern.
        let distinct = |kind: Kind, bits: [u8; 4]| {
            let mut m = Memory::new(kind, xl_roms());
            let mut seen = Vec::new();
            for n in 0..16u8 {
                let mut portb = 0xef;
                for (i, bit) in bits.iter().enumerate() {
                    if n >> i & 1 == 0 {
                        portb &= !bit;
                    }
                }
                m.portb = portb;
                seen.push(m.area(0x4000));
            }
            seen.sort_by_key(|a| match a {
                Area::Extended(b, _) => *b,
                _ => 255,
            });
            seen.dedup();
            assert_eq!(seen.len(), 16, "{kind:?}");
            assert!(!seen.contains(&Area::Ram));
        };
        distinct(Kind::Rambo320, [0x04, 0x08, 0x20, 0x40]);
        distinct(Kind::Compy320, [0x04, 0x08, 0x40, 0x80]);

        // RAMBO: bit 4 is ANTIC's too, bit 5 only a bank bit.
        let mut m = Memory::new(Kind::Rambo320, xl_roms());
        m.portb = 0xef;
        assert!(matches!(m.area_for(0x4000, true), Area::Extended(..)));
        m.portb = 0xdf | 0x10;
        assert_eq!(m.area_for(0x4000, true), Area::Ram);
        // Compy Shop: separate as on the 130XE, bit 7 is a bank bit while on.
        let mut m = Memory::new(Kind::Compy320, xl_roms());
        m.portb = 0xdf;
        assert!(matches!(m.area_for(0x4000, true), Area::Extended(..)));
        assert_eq!(m.area_for(0x4000, false), Area::Ram);
        m.portb = 0x6f;
        assert!(matches!(m.area_for(0x5000, false), Area::Extended(..)));
        m.portb = 0x7f;
        assert!(matches!(m.area_for(0x5000, false), Area::Os(_)));
    }
}
