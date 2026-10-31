//! Freezer cartridges with the Action Replay register: Action Replay,
//! Atomic/Nordic Power, Retro Replay and Nordic Replay.
//!
//! `$DE00` (write; all of /IO1 on the Action Replay and Nordic Power):
//!
//! | Bit | Meaning                                                      |
//! | --- | ------------------------------------------------------------ |
//! | 0   | 1 = /GAME low                                                |
//! | 1   | 1 = /EXROM high                                              |
//! | 2   | 1 = cartridge off, register locked until reset               |
//! | 3–4 | bank A13, A14                                                |
//! | 5   | 1 = RAM instead of ROM (ROML, and the I/O window)            |
//! | 6   | 1 = leave the freeze state                                   |
//! | 7   | bank A15 (Retro and Nordic Replay)                           |
//!
//! `$DE01` (Retro and Nordic Replay): bit 0 flash mode, bit 1 AllowBank
//! (RAM of the I/O window follows the bank), bit 2 NoFreeze, bits 3, 4, 7
//! the bank as in `$DE00`, bit 6 REU mapping (the I/O window moves from
//! `$DF00` to `$DE02–$DEFF`). Bits 1, 2 and 6 can be written once until
//! reset, unless in flash mode; this model has no flash jumper, so flash
//! mode stays off and the ROM cannot be written. The window shows the
//! bank's `$1E00`/`$1F00` page.
//!
//! Mode `$22` differs by cartridge (`C64/carts/nordicpower/readme.txt`,
//! measured on the hardware): on the Action Replay cartridge RAM sits
//! beside C64 RAM at `$8000` and both drive the bus, so a read gives the
//! OR of the two (the test's `$08 or $43`); on the Nordic Power and Nordic Replay ROM
//! reads at `$8000` and cartridge RAM at `$A000`, writes there missing C64
//! RAM.
//!
//! The freeze button pulls /NMI and selects Ultimax mode with bank 0 until
//! bit 6 is written. The real logic waits for the CPU's stack writes first;
//! this model switches at once.
//!
//! The Retro Replay writes its RAM through ROML in Ultimax mode only; the
//! Nordic Replay and the older cartridges in every mode (`aracidtest`).
//!
//! Sources: `C64/carts/nordicpower`, `aracidtest`, `retroreplay` of the
//! VICE test programs (readmes, table of tested hardware, programs).

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::Crt;

use crate::expansion::{CartLines, ExpansionCart};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    ActionReplay,
    NordicPower,
    RetroReplay,
    NordicReplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freezer {
    kind: Kind,
    rom: Vec<u8>,
    ram: Vec<u8>,
    /// `$DE00` as written.
    control: u8,
    /// `$DE01`: bits 1, 2 and 6.
    extended: u8,
    /// `$DE01` took its one write.
    written: bool,
    /// Bit 2 of `$DE00`: until reset.
    off: bool,
    frozen: bool,
}

const ALLOW_BANK: u8 = 0x02;
const NO_FREEZE: u8 = 0x04;
const REU_MAP: u8 = 0x40;
const RAM: u8 = 0x20;

impl Freezer {
    pub fn from_crt(crt: &Crt, kind: Kind) -> Result<Self, MediaError> {
        let mut rom = Vec::new();
        for c in crt.chips.iter().filter(|c| c.kind != 1) {
            if c.load != 0x8000 || c.data.len() != 0x2000 {
                return Err(MediaError::Format(
                    "CRT: chip does not fit a freezer".into(),
                ));
            }
            let at = usize::from(c.bank) * 0x2000;
            if rom.len() < at + 0x2000 {
                rom.resize(at + 0x2000, 0xff);
            }
            rom[at..at + 0x2000].copy_from_slice(&c.data);
        }
        if rom.is_empty() {
            return Err(MediaError::Format("CRT: no ROM".into()));
        }
        let replay = matches!(kind, Kind::RetroReplay | Kind::NordicReplay);
        Ok(Self {
            kind,
            rom,
            ram: vec![0; if replay { 0x8000 } else { 0x2000 }],
            control: 0,
            extended: 0,
            written: false,
            off: false,
            frozen: false,
        })
    }

    fn replay(&self) -> bool {
        matches!(self.kind, Kind::RetroReplay | Kind::NordicReplay)
    }

    /// The Atomic Power's mode: ROM at `$8000`, RAM at `$A000`.
    fn nordic_mode(&self) -> bool {
        !self.frozen
            && !self.off
            && matches!(self.kind, Kind::NordicPower | Kind::NordicReplay)
            && self.control & 0x27 == 0x22
    }

    fn bank(&self) -> usize {
        let low = usize::from(self.control >> 3 & 3);
        if self.replay() {
            low | usize::from(self.control >> 7) << 2
        } else {
            low
        }
    }

    fn ram_at(&self, addr: u16, bank: usize) -> usize {
        (bank << 13 | usize::from(addr & 0x1fff)) % self.ram.len()
    }

    fn rom_byte(&self, addr: u16) -> u8 {
        let bank = if self.frozen { 0 } else { self.bank() };
        self.rom[(bank << 13 | usize::from(addr & 0x1fff)) % self.rom.len()]
    }

    fn ram_on(&self) -> bool {
        !self.off && self.control & RAM != 0
    }

    /// The bank the I/O window's RAM uses: without AllowBank always 0.
    fn window_bank(&self) -> usize {
        if !self.replay() || self.extended & ALLOW_BANK != 0 {
            self.bank()
        } else {
            0
        }
    }

    /// The window's offset in the 8 KB bank, if `addr` lies in it.
    fn window(&self, addr: u16) -> Option<u16> {
        if self.off {
            return None;
        }
        let in_io1 = self.replay() && self.extended & REU_MAP != 0;
        match (addr & 0xff00, in_io1) {
            (0xdf00, false) => Some(0x1f00 | (addr & 0xff)),
            (0xde00, true) if addr & 0xff >= 2 => Some(0x1e00 | (addr & 0xff)),
            _ => None,
        }
    }

    fn status(&self) -> u8 {
        self.control & 0x98 | self.extended & (ALLOW_BANK | REU_MAP) | u8::from(self.frozen) << 2
    }
}

impl ExpansionCart for Freezer {
    fn name(&self) -> &'static str {
        match self.kind {
            Kind::ActionReplay => "Action Replay",
            Kind::NordicPower => "Nordic Power",
            Kind::RetroReplay => "Retro Replay",
            Kind::NordicReplay => "Nordic Replay",
        }
    }

    fn lines(&self) -> CartLines {
        let (game, exrom) = if self.frozen {
            (true, false)
        } else if self.off {
            (false, false)
        } else if self.nordic_mode() {
            (true, true)
        } else {
            (self.control & 1 != 0, self.control & 2 == 0)
        };
        CartLines {
            game,
            exrom,
            nmi: self.frozen,
            irq: false,
        }
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        if self.ram_on() && !self.frozen && !self.nordic_mode() {
            Some(self.ram[self.ram_at(addr, self.bank())])
        } else {
            Some(self.rom_byte(addr))
        }
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        if self.nordic_mode() {
            Some(self.ram[self.ram_at(addr, self.bank())])
        } else {
            Some(self.rom_byte(addr))
        }
    }

    fn write_roml(&mut self, addr: u16, v: u8) {
        // The Retro Replay takes RAM writes in Ultimax mode only.
        let writable = self.kind != Kind::RetroReplay || self.control & 3 == 3;
        if writable && self.ram_on() && !self.frozen && !self.nordic_mode() {
            let at = self.ram_at(addr, self.bank());
            self.ram[at] = v;
        }
    }

    fn write_romh(&mut self, addr: u16, v: u8) {
        if self.nordic_mode() {
            let at = self.ram_at(addr, self.bank());
            self.ram[at] = v;
        }
    }

    fn write_under(&mut self, addr: u16, v: u8) {
        // Without /ROML the cartridge still decodes $8000: its RAM takes
        // the write beside C64 RAM.
        if self.kind == Kind::ActionReplay && addr & 0xe000 == 0x8000 {
            self.write_roml(addr, v);
        }
    }

    fn blocks_ram_write(&self, addr: u16) -> bool {
        self.nordic_mode() && addr & 0xe000 == 0xa000
    }

    fn vic_sees_ultimax(&self) -> bool {
        self.kind != Kind::RetroReplay
    }

    fn drives_over_ram(&self, addr: u16) -> u8 {
        if self.kind == Kind::ActionReplay && self.ram_on() && addr & 0xe000 == 0x8000 {
            self.ram[self.ram_at(addr, self.bank())]
        } else {
            0
        }
    }

    fn peek_io(&self, addr: u16) -> Option<u8> {
        if self.replay() && !self.off && addr & 0xfffe == 0xde00 {
            return Some(self.status());
        }
        let at = self.window(addr)?;
        Some(if self.ram_on() {
            self.ram[self.ram_at(at, self.window_bank())]
        } else {
            self.rom_byte(at)
        })
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        if self.off {
            return;
        }
        let register = if self.replay() {
            addr & 0xfffe == 0xde00
        } else {
            addr & 0xff00 == 0xde00
        };
        if register {
            if self.replay() && addr == 0xde01 {
                if !self.written {
                    self.extended = v & (ALLOW_BANK | NO_FREEZE | REU_MAP);
                    self.written = true;
                }
                self.control = self.control & !0x98 | v & 0x98;
                return;
            }
            self.control = v;
            self.off = v & 4 != 0;
            if v & 0x40 != 0 {
                self.frozen = false;
            }
        } else if self.ram_on() {
            if let Some(at) = self.window(addr) {
                let at = self.ram_at(at, self.window_bank());
                self.ram[at] = v;
            }
        }
    }

    fn reset(&mut self) {
        self.control = 0;
        self.extended = 0;
        self.written = false;
        self.off = false;
        self.frozen = false;
    }

    fn press_button(&mut self) -> bool {
        if self.extended & NO_FREEZE == 0 {
            self.frozen = true;
            self.off = false;
        }
        true
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"CFRZ", 1, |w| {
            w.u8(self.control);
            w.u8(self.extended);
            w.bool(self.written);
            w.bool(self.off);
            w.bool(self.frozen);
            w.raw(&self.ram);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"CFRZ", 1)?;
        let (control, extended) = (c.u8()?, c.u8()?);
        let (written, off, frozen) = (c.bool()?, c.bool()?, c.bool()?);
        let ram = c.raw(self.ram.len())?.to_vec();
        self.control = control;
        self.extended = extended;
        self.written = written;
        self.off = off;
        self.frozen = frozen;
        self.ram = ram;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
