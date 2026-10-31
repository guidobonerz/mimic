//! Pagefox (Scanntronik): 16 KB banks of two EPROMs and 32 KB of RAM at
//! `$8000–$BFFF`, in 16 KB game mode.
//!
//! `$DE80–$DEFF` (write): bit 1 16 KB bank within the chip, bits 2–3 chip
//! (0, 1 EPROM, 2 RAM, 3 empty: reads return what is on the bus), bit 4
//! 1 = cartridge off. Writes to the area go to the RAM when it is selected
//! and always to C64 RAM, whatever `$01` banks in. Source: `C64/carts/pagefox` of the VICE test
//! programs (`writeram.s`, `godot.asm`).

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::Crt;

use crate::expansion::{CartLines, ExpansionCart};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pagefox {
    rom: Vec<u8>,
    ram: Vec<u8>,
    control: u8,
}

impl Pagefox {
    pub fn from_crt(crt: &Crt) -> Result<Self, MediaError> {
        let mut rom = vec![0xff; 0x10000];
        for c in crt.chips.iter().filter(|c| c.kind != 1) {
            if c.bank > 3 || c.load != 0x8000 || c.data.len() != 0x4000 {
                return Err(MediaError::Format(
                    "CRT: chip does not fit a Pagefox".into(),
                ));
            }
            let at = usize::from(c.bank) * 0x4000;
            rom[at..at + 0x4000].copy_from_slice(&c.data);
        }
        Ok(Self {
            rom,
            ram: vec![0; 0x8000],
            control: 0,
        })
    }

    fn bank(&self) -> usize {
        usize::from(self.control >> 1 & 1)
    }

    fn chip(&self) -> u8 {
        self.control >> 2 & 3
    }

    fn at(&self, addr: u16) -> usize {
        self.bank() << 14 | usize::from(addr & 0x3fff)
    }

    fn byte(&self, addr: u16) -> Option<u8> {
        match self.chip() {
            2 => Some(self.ram[self.at(addr)]),
            3 => None,
            chip => Some(self.rom[usize::from(chip) << 15 | self.at(addr)]),
        }
    }
}

impl ExpansionCart for Pagefox {
    fn name(&self) -> &'static str {
        "Pagefox"
    }

    fn lines(&self) -> CartLines {
        let on = self.control & 0x10 == 0;
        CartLines {
            game: on,
            exrom: on,
            ..CartLines::default()
        }
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        self.byte(addr)
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        self.byte(addr)
    }

    fn write_roml(&mut self, addr: u16, v: u8) {
        if self.chip() == 2 {
            let at = self.at(addr);
            self.ram[at] = v;
        }
    }

    fn write_romh(&mut self, addr: u16, v: u8) {
        self.write_roml(addr, v);
    }

    fn write_under(&mut self, addr: u16, v: u8) {
        // The cartridge decodes the area itself, also with ROM banked out.
        if self.control & 0x10 == 0 && addr & 0xc000 == 0x8000 {
            self.write_roml(addr, v);
        }
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        if addr & 0xff80 == 0xde80 {
            self.control = v;
        }
    }

    fn reset(&mut self) {
        self.control = 0;
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"CPFX", 1, |w| {
            w.u8(self.control);
            w.raw(&self.ram);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"CPFX", 1)?;
        let control = c.u8()?;
        let ram = c.raw(self.ram.len())?.to_vec();
        self.control = control;
        self.ram = ram;
        Ok(())
    }
}
