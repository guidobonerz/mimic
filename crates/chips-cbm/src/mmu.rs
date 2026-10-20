//! MOS 8722 Memory Management Unit (C128).
//!
//! | Address        | Register                                                     |
//! | -------------- | ------------------------------------------------------------ |
//! | `$D500`/`$FF00` | CR: 7–6 RAM bank, 5–4 `$C000` (KERNAL/int./ext. ROM/RAM), 3–2 `$8000` (BASIC hi/int./ext./RAM), 1 `$4000` (BASIC lo/RAM), 0 `$D000` I/O (0) |
//! | `$D501–$D504`  | PCRA–PCRD: preset configurations                             |
//! | `$FF01–$FF04`  | LCRA–LCRD: a write loads CR from PCRA–PCRD                   |
//! | `$D505`        | MCR: 7 40/80 key (in), 6 C64 mode, 5 EXROM (in), 4 GAME (in), 3 FSDIR, 0 8502 (1) / Z80 (0) |
//! | `$D506`        | RCR: 7–6 VIC RAM bank, 3 top shared, 2 bottom shared, 1–0 shared size (1/4/8/16 KB) |
//! | `$D507–$D50A`  | P0L/P0H, P1L/P1H: zero page and stack relocation             |
//! | `$D50B`        | version: `$20` (two banks, version 0)                         |
//!
//! Source: Commodore 128 Programmer's Reference Guide, chapter 13; MOS
//! 8722 data sheet. [`Mmu::translate`] turns a CPU address in C128 mode
//! into what it selects; the machine supplies the ROMs and I/O.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// What a CPU address selects in C128 mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// RAM `bank` (0–3), physical address within the bank.
    Ram {
        bank: u8,
        addr: u16,
    },
    BasicLo(u16),
    BasicHi(u16),
    /// Function ROM: internal (`false`) or external (`true`), offset in
    /// the 32 KB (`$8000–$FFFF`).
    Function {
        external: bool,
        offset: u16,
    },
    /// KERNAL ROM offset (`$C000–$FFFF`, without `$D000–$DFFF`).
    Kernal(u16),
    /// Character ROM (the KERNAL's `$D000–$DFFF` window with I/O off).
    Chargen(u16),
    Io(u16),
    /// The MMU's load registers at `$FF00–$FF04`.
    Mmu(u16),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mmu {
    pub cr: u8,
    pub pcr: [u8; 4],
    pub mcr: u8,
    pub rcr: u8,
    /// Page pointers: (page, bank) of zero page and stack.
    pub p0: (u8, u8),
    pub p1: (u8, u8),
}

impl Mmu {
    /// Power-on/reset: CR 0 (all ROM, I/O), Z80 active, page pointers at
    /// pages 0 and 1, 1 KB bottom shared.
    pub fn new() -> Self {
        Self {
            cr: 0,
            pcr: [0; 4],
            mcr: 0,
            rcr: 0,
            p0: (0, 0),
            p1: (1, 0),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The 8502 runs (MCR bit 0); otherwise the Z80.
    pub fn m8502(&self) -> bool {
        self.mcr & 0x01 != 0
    }

    pub fn c64_mode(&self) -> bool {
        self.mcr & 0x40 != 0
    }

    /// RAM bank the VIC sees (RCR bits 6–7).
    pub fn vic_bank(&self) -> u8 {
        (self.rcr >> 6) & 3
    }

    /// `$D500–$D50B` read; `keys_80` is the 40/80 key (true = down),
    /// `game`/`exrom` the cartridge lines (true = low).
    pub fn peek(&self, reg: u8, key_4080_down: bool, game: bool, exrom: bool) -> u8 {
        match reg & 0x0f {
            0 => self.cr,
            1..=4 => self.pcr[usize::from(reg - 1)],
            5 => {
                let mut v = (self.mcr & 0x49) | 0x06;
                if !key_4080_down {
                    v |= 0x80;
                }
                if !exrom {
                    v |= 0x20;
                }
                if !game {
                    v |= 0x10;
                }
                v
            }
            6 => self.rcr | 0x30,
            7 => self.p0.0,
            8 => self.p0.1 | 0xf0,
            9 => self.p1.0,
            0xa => self.p1.1 | 0xf0,
            0xb => 0x20,
            _ => 0xff,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 0x0f {
            0 => self.cr = v,
            1..=4 => self.pcr[usize::from(reg - 1)] = v,
            5 => self.mcr = v & 0x49,
            6 => self.rcr = v & 0xcf,
            // The page pointers take effect when the low byte is written;
            // the high byte (bank) is latched first.
            7 => self.p0.0 = v,
            8 => self.p0.1 = v & 0x0f,
            9 => self.p1.0 = v,
            0xa => self.p1.1 = v & 0x0f,
            _ => {}
        }
    }

    /// `$FF00–$FF04`: CR, or load CR from a preset.
    pub fn write_lcr(&mut self, reg: u8, v: u8) {
        match reg {
            0 => self.cr = v,
            1..=4 => self.cr = self.pcr[usize::from(reg - 1)],
            _ => {}
        }
    }

    /// Size of the shared (common) RAM area.
    fn shared_size(&self) -> u16 {
        [0x0400, 0x1000, 0x2000, 0x4000][usize::from(self.rcr & 3)]
    }

    /// The RAM bank and address a CPU RAM access lands in: page
    /// relocation, then shared RAM (always bank 0).
    pub fn ram(&self, addr: u16) -> (u8, u16) {
        let bank = (self.cr >> 6) & 3;
        let page = (addr >> 8) as u8;
        let low = addr & 0xff;
        // Zero page and stack swap with their relocation targets.
        let (bank, addr) = if page == 0 {
            (self.p0.1 & 3, (u16::from(self.p0.0) << 8) | low)
        } else if page == 1 {
            (self.p1.1 & 3, (u16::from(self.p1.0) << 8) | low)
        } else if page == self.p0.0 && bank == self.p0.1 & 3 {
            (bank, low)
        } else if page == self.p1.0 && bank == self.p1.1 & 3 {
            (bank, 0x0100 | low)
        } else {
            (bank, addr)
        };
        let size = self.shared_size();
        let shared = (self.rcr & 0x04 != 0 && addr < size)
            || (self.rcr & 0x08 != 0 && addr >= 0u16.wrapping_sub(size));
        if shared {
            (0, addr)
        } else {
            (bank, addr)
        }
    }

    /// What a C128-mode CPU address selects (reads; writes to ROM areas go
    /// to the RAM underneath, see [`Mmu::ram`]).
    pub fn translate(&self, addr: u16) -> Target {
        let cr = self.cr;
        if (0xff00..=0xff04).contains(&addr) {
            return Target::Mmu(addr - 0xff00);
        }
        let ram = || {
            let (bank, addr) = self.ram(addr);
            Target::Ram { bank, addr }
        };
        let rom_select = |bits: u8, offset: u16, system: Target| match bits {
            0 => system,
            1 => Target::Function {
                external: false,
                offset,
            },
            2 => Target::Function {
                external: true,
                offset,
            },
            _ => ram(),
        };
        match addr {
            0x4000..=0x7fff if cr & 0x02 == 0 => Target::BasicLo(addr - 0x4000),
            0x8000..=0xbfff => {
                rom_select((cr >> 2) & 3, addr - 0x8000, Target::BasicHi(addr - 0x8000))
            }
            0xd000..=0xdfff if cr & 0x01 == 0 => Target::Io(addr),
            0xd000..=0xdfff if (cr >> 4) & 3 == 0 => Target::Chargen(addr - 0xd000),
            0xc000..=0xffff => {
                rom_select((cr >> 4) & 3, addr - 0x8000, Target::Kernal(addr - 0xc000))
            }
            _ => ram(),
        }
    }
}

const TAG: [u8; 4] = *b"8722";
const VERSION: u16 = 1;

impl Savestate for Mmu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.cr);
            w.raw(&self.pcr);
            w.u8(self.mcr);
            w.u8(self.rcr);
            w.u8(self.p0.0);
            w.u8(self.p0.1);
            w.u8(self.p1.0);
            w.u8(self.p1.1);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut m = Mmu::new();
        m.cr = c.u8()?;
        c.raw_into(&mut m.pcr)?;
        m.mcr = c.u8()? & 0x49;
        m.rcr = c.u8()? & 0xcf;
        m.p0 = (c.u8()?, c.u8()? & 0x0f);
        m.p1 = (c.u8()?, c.u8()? & 0x0f);
        *self = m;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_shows_roms_and_io() {
        let m = Mmu::new();
        assert!(!m.m8502(), "the Z80 starts");
        assert_eq!(m.translate(0x4000), Target::BasicLo(0));
        assert_eq!(m.translate(0x8000), Target::BasicHi(0));
        assert_eq!(m.translate(0xd020), Target::Io(0xd020));
        assert_eq!(m.translate(0xe000), Target::Kernal(0x2000));
        assert_eq!(m.translate(0xff00), Target::Mmu(0));
        assert_eq!(
            m.translate(0x1000),
            Target::Ram {
                bank: 0,
                addr: 0x1000
            }
        );
    }

    #[test]
    fn configuration_bits() {
        let mut m = Mmu::new();
        m.write(0, 0x7f); // bank 1, all RAM, no I/O
        assert_eq!(
            m.translate(0x4000),
            Target::Ram {
                bank: 1,
                addr: 0x4000
            }
        );
        assert_eq!(
            m.translate(0xd000),
            Target::Ram {
                bank: 1,
                addr: 0xd000
            }
        );
        m.write(0, 0x01 | 0x04); // I/O off, internal function ROM at $8000
        assert_eq!(
            m.translate(0x9000),
            Target::Function {
                external: false,
                offset: 0x1000
            }
        );
        assert_eq!(m.translate(0xd800), Target::Chargen(0x0800));
        // Presets: write PCRB, then any value to LCRB.
        m.write(2, 0x3f);
        m.write_lcr(2, 0);
        assert_eq!(m.cr, 0x3f);
    }

    #[test]
    fn shared_ram_and_page_relocation() {
        let mut m = Mmu::new();
        m.write(0, 0x40); // bank 1
        m.write(6, 0x04); // 1 KB shared at the bottom
        assert_eq!(m.ram(0x0300), (0, 0x0300));
        assert_eq!(m.ram(0x0400), (1, 0x0400));
        m.write(6, 0x0b); // 16 KB shared at the top
        assert_eq!(m.ram(0xc000), (0, 0xc000));
        // Zero page at $1300 of bank 1; $1300 then shows page 0.
        m.write(6, 0);
        m.write(8, 1);
        m.write(7, 0x13);
        assert_eq!(m.ram(0x0042), (1, 0x1342));
        assert_eq!(m.ram(0x1342), (1, 0x0042));
    }

    #[test]
    fn mcr_reads_inputs() {
        let mut m = Mmu::new();
        m.write(5, 0xff);
        assert_eq!(m.mcr, 0x49);
        assert_eq!(m.peek(5, false, false, false), 0xff);
        assert_eq!(m.peek(5, true, false, false) & 0x80, 0, "40/80 key down");
        assert_eq!(m.peek(0xb, false, false, false), 0x20);
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = Mmu::new();
        m.write(0, 0x3e);
        m.write(1, 0x12);
        m.write(5, 0x41);
        m.write(8, 1);
        m.write(7, 0x20);
        let mut w = StateWriter::new();
        m.save(&mut w);
        let data = w.into_bytes();
        let mut n = Mmu::new();
        n.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(m, n);
    }
}
