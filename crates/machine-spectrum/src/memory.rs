//! ZX Spectrum memory.
//!
//! - **48K:** 16 KB ROM at `$0000`, RAM `$4000–$FFFF` (the lower 16 KB,
//!   with the screen, contended).
//! - **128K / +2:** two 16 KB ROMs (0: the 128 editor, 1: 48 BASIC) and
//!   eight 16 KB RAM banks. `$4000` is always bank 5, `$8000` bank 2,
//!   `$C000` the bank port `$7FFD` selects. Port `$7FFD` (decoded by A15 =
//!   0 and A1 = 0, write only): bits 0–2 the bank at `$C000`, bit 3 the
//!   screen (bank 5 or 7), bit 4 the ROM, bit 5 locks the port until reset.
//!   Banks 1, 3, 5 and 7 are contended.
//!
//! The 48K uses the same banks: 5 at `$4000`, 2 at `$8000`, 0 at `$C000`.
//!
//! Expansions page their own memory over the ROM: the Multiface 128 (8 KB
//! ROM at `$0000`, 8 KB RAM at `$2000`) and the Interface 1's shadow ROM
//! (8 KB, repeated at `$2000`). The Multiface wins when both are paged.
//!
//! **Multiface 128** (Romantic Robot manual; its ROM's own port use): the
//! red button pulls /NMI and the interface pages itself in at the
//! acknowledge's opcode fetch from `$0066`, which also makes it
//! "visible". While visible, `IN ($BF)` pages it in (bit 7 of the byte
//! read: the screen bit of the last `$7FFD` write) and `IN ($3F)` pages it
//! out; `OUT ($3F)` hides it (the menu's "off"), `OUT ($BF)` shows it.
//! After power-on it is hidden.
//!
//! **Interface 1:** its ROM pages in on an opcode fetch from `$0008`
//! (the error restart, through which it extends BASIC) or `$1708` (CLOSE #),
//! that fetch already reading the shadow ROM, and pages out after the
//! fetch from `$0700` (a RET in the shadow ROM). Source: the Interface 1
//! ROM's own code at those addresses; Sinclair ZX Interface 1 Service
//! Manual.
//!
//! Sources: Sinclair ZX Spectrum 128 Service Manual (memory map, port
//! `$7FFD`); World of Spectrum FAQ "128K memory".

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const ROM_SIZE: usize = 0x4000;
pub const BANK: usize = 0x4000;
/// Bytes of the screen: bitmap and attributes.
pub const SCREEN_LEN: usize = 6912;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    /// One ROM (48K) or two (128K, +2).
    pub roms: Vec<Vec<u8>>,
    /// Eight banks of 16 KB.
    pub ram: Vec<u8>,
    /// Port $7FFD (always 0 on the 48K).
    pub paging: u8,
    pub banked: bool,
    /// Multiface 128: ROM, RAM and whether they are paged in.
    pub mf_rom: Option<Vec<u8>>,
    pub mf_ram: Vec<u8>,
    pub mf_paged: bool,
    /// The Multiface answers its ports; its button was pressed and the
    /// NMI's fetch from $0066 is still to come.
    pub mf_visible: bool,
    pub mf_armed: bool,
    /// The last value written to port $7FFD (the Multiface reads its
    /// screen bit; also on the 48K, where nothing else listens).
    pub last_7ffd: u8,
    /// Interface 1 shadow ROM and whether it is paged in.
    pub if1_rom: Option<Vec<u8>>,
    pub if1_paged: bool,
    /// The fetch from $0700 happened: page out at the next fetch.
    if1_leaving: bool,
}

impl Memory {
    pub fn new(roms: Vec<Vec<u8>>, banked: bool) -> Self {
        Self {
            roms,
            ram: vec![0; 8 * BANK],
            paging: 0,
            banked,
            mf_rom: None,
            mf_ram: vec![0; 0x2000],
            mf_paged: false,
            mf_visible: false,
            mf_armed: false,
            last_7ffd: 0,
            if1_rom: None,
            if1_paged: false,
            if1_leaving: false,
        }
    }

    /// Power-on: RAM cleared, paging reset.
    pub fn clear(&mut self) {
        self.ram.fill(0);
        self.mf_ram.fill(0);
        self.mf_visible = false;
        self.reset();
    }

    /// The RESET line: paging back to bank 0, ROM 0, unlocked.
    pub fn reset(&mut self) {
        self.paging = 0;
        self.mf_paged = false;
        self.mf_armed = false;
        self.if1_paged = false;
        self.if1_leaving = false;
    }

    /// An opcode fetch from `addr`: the Multiface pages in at its NMI's
    /// fetch from $0066.
    #[inline]
    pub fn m1(&mut self, addr: u16) {
        if self.if1_rom.is_some() {
            if std::mem::take(&mut self.if1_leaving) {
                self.if1_paged = false;
            }
            match addr {
                0x0008 | 0x1708 => self.if1_paged = true,
                0x0700 if self.if1_paged => self.if1_leaving = true,
                _ => {}
            }
        }
        if self.mf_armed && addr == 0x0066 {
            self.mf_armed = false;
            self.mf_paged = true;
            self.mf_visible = true;
        }
    }

    /// The Multiface's port reads ($BF in, $3F out), `None` when it does
    /// not answer.
    pub fn mf_in(&mut self, port: u16) -> Option<u8> {
        if self.mf_rom.is_none() || !self.mf_visible {
            return None;
        }
        match port & 0xff {
            0xbf => {
                self.mf_paged = true;
                Some(0x7f | (self.last_7ffd & 8) << 4)
            }
            0x3f => {
                self.mf_paged = false;
                Some(0xff)
            }
            _ => None,
        }
    }

    /// The Multiface's port writes: $3F hides it, $BF shows it.
    pub fn mf_out(&mut self, port: u16) {
        if self.mf_rom.is_some() {
            match port & 0xff {
                0x3f => self.mf_visible = false,
                0xbf => self.mf_visible = true,
                _ => {}
            }
        }
    }

    /// The RAM bank in 16 KB page `page` (1–3).
    #[inline]
    pub fn bank_of(&self, page: u16) -> usize {
        match page {
            1 => 5,
            2 => 2,
            _ => usize::from(self.paging & 7),
        }
    }

    /// The ROM in use (0, or 1 on the 128K with bit 4).
    #[inline]
    pub fn rom_index(&self) -> usize {
        usize::from(self.paging >> 4 & 1).min(self.roms.len() - 1)
    }

    /// The 48K BASIC ROM is paged in (with no expansion over it).
    pub fn basic_rom(&self) -> bool {
        !self.mf_paged && !self.if1_paged && self.rom_index() == self.roms.len() - 1
    }

    /// The CPU's address `addr` is in contended RAM.
    #[inline]
    pub fn contended(&self, addr: u16) -> bool {
        match addr >> 14 {
            1 => true,
            3 => self.banked && self.paging & 1 != 0,
            _ => false,
        }
    }

    #[inline]
    pub fn read(&self, addr: u16) -> u8 {
        let page = addr >> 14;
        let off = usize::from(addr & 0x3fff);
        if page == 0 {
            if self.mf_paged {
                if let Some(rom) = &self.mf_rom {
                    return if off < 0x2000 {
                        rom[off]
                    } else {
                        self.mf_ram[off - 0x2000]
                    };
                }
            }
            if self.if1_paged {
                if let Some(rom) = &self.if1_rom {
                    return rom[off & 0x1fff];
                }
            }
            return self.roms[self.rom_index()][off];
        }
        self.ram[self.bank_of(page) * BANK + off]
    }

    /// A write; returns false where nothing is written (ROM).
    #[inline]
    pub fn write(&mut self, addr: u16, v: u8) -> bool {
        let page = addr >> 14;
        let off = usize::from(addr & 0x3fff);
        if page == 0 {
            if self.mf_paged && self.mf_rom.is_some() && off >= 0x2000 {
                self.mf_ram[off - 0x2000] = v;
                return true;
            }
            return false;
        }
        let i = self.bank_of(page) * BANK + off;
        self.ram[i] = v;
        true
    }

    /// Port $7FFD (ignored while locked or on the 48K).
    pub fn write_7ffd(&mut self, v: u8) {
        self.last_7ffd = v;
        if self.banked && self.paging & 0x20 == 0 {
            self.paging = v & 0x3f;
        }
    }

    /// The bank the ULA shows (5, or 7 with bit 3).
    #[inline]
    pub fn screen_bank(&self) -> usize {
        if self.banked && self.paging & 8 != 0 {
            7
        } else {
            5
        }
    }

    /// The screen the ULA shows: bitmap and attributes.
    #[inline]
    pub fn screen(&self) -> &[u8] {
        let at = self.screen_bank() * BANK;
        &self.ram[at..at + SCREEN_LEN]
    }

    /// A CPU write to `addr` changes the shown screen.
    #[inline]
    pub fn hits_screen(&self, addr: u16) -> bool {
        let page = addr >> 14;
        page != 0
            && usize::from(addr & 0x3fff) < SCREEN_LEN
            && self.bank_of(page) == self.screen_bank()
    }

    /// A RAM bank.
    pub fn bank(&self, n: usize) -> &[u8] {
        &self.ram[n * BANK..(n + 1) * BANK]
    }

    pub fn bank_mut(&mut self, n: usize) -> &mut [u8] {
        &mut self.ram[n * BANK..(n + 1) * BANK]
    }
}

const TAG: [u8; 4] = *b"SPMM";

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u8(self.paging);
            w.bool(self.mf_paged);
            w.bool(self.mf_visible);
            w.bool(self.mf_armed);
            w.u8(self.last_7ffd);
            w.bool(self.if1_paged);
            w.bool(self.if1_leaving);
            w.raw(&self.ram);
            w.raw(&self.mf_ram);
        });
    }

    /// ROMs and the model are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let paging = c.u8()?;
        let (mf_paged, mf_visible, mf_armed) = (c.bool()?, c.bool()?, c.bool()?);
        let last_7ffd = c.u8()?;
        let if1_paged = c.bool()?;
        let if1_leaving = c.bool()?;
        if paging & 0xc0 != 0 || (!self.banked && paging != 0) {
            return Err(StateError::Invalid("paging"));
        }
        let mut ram = vec![0; self.ram.len()];
        c.raw_into(&mut ram)?;
        let mut mf_ram = vec![0; self.mf_ram.len()];
        c.raw_into(&mut mf_ram)?;
        self.paging = paging;
        self.mf_paged = mf_paged && self.mf_rom.is_some();
        self.mf_visible = mf_visible;
        self.mf_armed = mf_armed;
        self.last_7ffd = last_7ffd;
        self.if1_paged = if1_paged && self.if1_rom.is_some();
        self.if1_leaving = if1_leaving && self.if1_rom.is_some();
        self.ram = ram;
        self.mf_ram = mf_ram;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roms() -> Vec<Vec<u8>> {
        vec![vec![0x11; ROM_SIZE], vec![0x22; ROM_SIZE]]
    }

    #[test]
    fn banking_and_contention() {
        let mut m = Memory::new(roms(), true);
        assert_eq!(m.read(0x0000), 0x11);
        m.write_7ffd(0x13); // bank 3, ROM 1
        assert_eq!(m.read(0x0000), 0x22);
        assert!(m.basic_rom());
        assert!(m.write(0xc000, 0xaa));
        assert_eq!(m.bank(3)[0], 0xaa);
        assert!(m.contended(0xc000), "odd bank");
        assert!(m.contended(0x4000));
        assert!(!m.contended(0x8000));
        m.write_7ffd(0x2f); // bank 7, screen 7, lock
        m.write_7ffd(0x01);
        assert_eq!(m.paging, 0x2f, "locked");
        assert_eq!(m.screen_bank(), 7);
        assert!(m.hits_screen(0xc000), "bank 7 shown and paged");
        assert!(!m.hits_screen(0x4000));
        assert!(!m.write(0x1000, 1), "ROM");
        m.reset();
        assert_eq!(m.paging, 0);
    }

    #[test]
    fn the_48k_does_not_page() {
        let mut m = Memory::new(vec![vec![0x33; ROM_SIZE]], false);
        m.write_7ffd(0x07);
        assert_eq!(m.paging, 0);
        assert!(!m.contended(0xc000));
        assert!(m.basic_rom());
        m.write(0x4000, 5);
        assert_eq!(m.screen()[0], 5);
    }

    #[test]
    fn expansions_over_the_rom() {
        let mut m = Memory::new(roms(), true);
        m.if1_rom = Some(vec![0x44; 0x2000]);
        m.mf_rom = Some(vec![0x55; 0x2000]);
        m.if1_paged = true;
        assert_eq!(m.read(0x2001), 0x44, "IF1 ROM repeats");
        m.mf_paged = true;
        assert_eq!(m.read(0x0001), 0x55);
        assert!(m.write(0x2000, 9));
        assert_eq!(m.read(0x2000), 9, "Multiface RAM");
        assert!(!m.basic_rom());
    }
}
