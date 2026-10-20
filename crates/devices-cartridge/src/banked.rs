//! ROM cartridges with 8 KB banks: normal 8 KB / 16 KB / Ultimax and the
//! bank-switching schemes of Ocean, C64GS/System 3, Dinamic, Magic Desk,
//! Fun Play/Power Play and Super Games.
//!
//! | Scheme      | Mode   | Bank register                                         |
//! | ----------- | ------ | ----------------------------------------------------- |
//! | Normal      | header | none                                                  |
//! | Ocean       | header | write `$DE00`: bank (bits 0–5)                         |
//! | System 3    | 8 KB   | write `$DE00+n`: bank n                                |
//! | Dinamic     | 8 KB   | read `$DE00+n`: bank n                                 |
//! | Magic Desk  | 8 KB   | write `$DE00`: bank (bits 0–6), bit 7 = cartridge off  |
//! | Fun Play    | 8 KB   | write `$DE00`: bank = bits 3–5 + bit 0 × 8; `$86` = off |
//! | Super Games | 16 KB  | write `$DF00`: bank (bits 0–1), bit 2 off, bit 3 locks |
//!
//! A bank holds ROML (chip at `$8000`) and ROMH (chip at `$A000`/`$E000`,
//! or the upper half of a 16 KB chip at `$8000`). Sources: the cartridge
//! descriptions of the CRT format (VICE documentation) and the C64-Wiki.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::Crt;

use crate::expansion::{CartLines, ExpansionCart};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Normal,
    Ocean,
    System3,
    Dinamic,
    MagicDesk,
    FunPlay,
    SuperGames,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Bank {
    roml: Option<Vec<u8>>,
    romh: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banked {
    scheme: Scheme,
    banks: Vec<Bank>,
    /// The mode lines the header gives (true = asserted).
    game: bool,
    exrom: bool,
    bank: usize,
    enabled: bool,
    /// Super Games: the register is locked until reset.
    locked: bool,
}

impl Banked {
    pub fn from_crt(crt: &Crt, scheme: Scheme) -> Result<Self, MediaError> {
        let mut banks: Vec<Bank> = Vec::new();
        for c in crt.chips.iter().filter(|c| c.kind != 1) {
            let i = usize::from(c.bank);
            if i >= 256 {
                return Err(MediaError::Format("CRT: bank beyond 255".into()));
            }
            if banks.len() <= i {
                banks.resize(i + 1, Bank::default());
            }
            let b = &mut banks[i];
            match (c.load, c.data.len()) {
                (0x8000, n) if n > 0x2000 => {
                    b.roml = Some(c.data[..0x2000].to_vec());
                    b.romh = Some(c.data[0x2000..].to_vec());
                }
                (0x8000, _) => b.roml = Some(c.data.clone()),
                (0xa000 | 0xe000, _) => b.romh = Some(c.data.clone()),
                (other, _) => {
                    return Err(MediaError::Format(format!(
                        "CRT: chip at ${other:04X} does not fit this cartridge"
                    )))
                }
            }
        }
        if banks.is_empty() {
            return Err(MediaError::Format("CRT: no ROM".into()));
        }
        let (game, exrom) = match scheme {
            Scheme::Normal | Scheme::Ocean => (!crt.game, !crt.exrom),
            Scheme::SuperGames => (true, true),
            _ => (false, true),
        };
        Ok(Self {
            scheme,
            banks,
            game,
            exrom,
            bank: 0,
            enabled: true,
            locked: false,
        })
    }

    fn select(&mut self, bank: usize) {
        self.bank = bank % self.banks.len().max(1);
    }

    fn byte(rom: &Option<Vec<u8>>, addr: u16) -> Option<u8> {
        let rom = rom.as_ref()?;
        rom.get(usize::from(addr & 0x1fff) % rom.len().max(1))
            .copied()
    }

    pub fn bank(&self) -> usize {
        self.bank
    }
}

impl ExpansionCart for Banked {
    fn name(&self) -> &'static str {
        match self.scheme {
            Scheme::Normal => "normal",
            Scheme::Ocean => "Ocean",
            Scheme::System3 => "C64GS/System 3",
            Scheme::Dinamic => "Dinamic",
            Scheme::MagicDesk => "Magic Desk",
            Scheme::FunPlay => "Fun Play",
            Scheme::SuperGames => "Super Games",
        }
    }

    fn lines(&self) -> CartLines {
        CartLines {
            game: self.enabled && self.game,
            exrom: self.enabled && self.exrom,
            ..CartLines::default()
        }
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        Self::byte(&self.banks[self.bank].roml, addr)
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        Self::byte(&self.banks[self.bank].romh, addr)
    }

    fn read_io(&mut self, addr: u16) -> Option<u8> {
        if self.scheme == Scheme::Dinamic && addr & 0xff00 == 0xde00 {
            self.select(usize::from(addr & 0xff));
        }
        None
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        let io1 = addr & 0xff00 == 0xde00;
        match self.scheme {
            Scheme::Ocean if io1 => self.select(usize::from(v & 0x3f)),
            Scheme::System3 if io1 => self.select(usize::from(addr & 0xff)),
            Scheme::MagicDesk if io1 => {
                self.select(usize::from(v & 0x7f));
                self.enabled = v & 0x80 == 0;
            }
            Scheme::FunPlay if io1 => {
                if v == 0x86 {
                    self.enabled = false;
                } else {
                    self.enabled = true;
                    self.select(usize::from(((v >> 3) & 7) | ((v & 1) << 3)));
                }
            }
            Scheme::SuperGames if !io1 && !self.locked => {
                self.select(usize::from(v & 3));
                self.enabled = v & 4 == 0;
                self.locked = v & 8 != 0;
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.bank = 0;
        self.enabled = true;
        self.locked = false;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"CBNK", 1, |w| {
            w.u8(self.bank as u8);
            w.bool(self.enabled);
            w.bool(self.locked);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"CBNK", 1)?;
        let bank = usize::from(c.u8()?);
        let (enabled, locked) = (c.bool()?, c.bool()?);
        if bank >= self.banks.len() {
            return Err(StateError::Invalid("cartridge bank"));
        }
        self.bank = bank;
        self.enabled = enabled;
        self.locked = locked;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
