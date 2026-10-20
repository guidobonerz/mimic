//! EasyFlash: 64 banks of 8 KB ROML and 8 KB ROMH in two Am29F040 flash
//! chips, and 256 bytes of RAM at `$DF00`.
//!
//! `$DE00` (write): bank (bits 0–5). `$DE02` (write): control, bit 7 LED,
//! bit 2 M (1: /GAME follows bit 0; 0: the boot jumper, which holds /GAME
//! low), bit 1 X (/EXROM low), bit 0 G (/GAME low when M is set). Reset
//! leaves `$DE02` at 0: Ultimax with bank 0, the boot code in ROMH at
//! `$E000`. Writes to ROML/ROMH reach the flash chips; what is written goes
//! back into the CRT (for the chips the image has). Source: EasyFlash
//! programmer's documentation (skoe).

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};
use media::crt::{Chip, Crt};

use crate::expansion::{CartLines, ExpansionCart};
use crate::flash::Flash;

const BANK: usize = 0x2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EasyFlash {
    roml: Flash,
    romh: Flash,
    ram: [u8; 256],
    bank: u8,
    control: u8,
    /// The image the flash came from (its chip layout for writing back).
    crt: Crt,
}

impl EasyFlash {
    pub fn from_crt(crt: &Crt) -> Result<Self, MediaError> {
        let mut roml = vec![0xff; crate::flash::SIZE];
        let mut romh = vec![0xff; crate::flash::SIZE];
        for c in &crt.chips {
            let bank = usize::from(c.bank);
            if bank >= 64 || c.data.len() > BANK {
                return Err(MediaError::Format("EasyFlash: bad CHIP".into()));
            }
            let dst = match c.load {
                0x8000 => &mut roml,
                0xa000 | 0xe000 => &mut romh,
                other => {
                    return Err(MediaError::Format(format!(
                        "EasyFlash: chip at ${other:04X}"
                    )))
                }
            };
            dst[bank * BANK..bank * BANK + c.data.len()].copy_from_slice(&c.data);
        }
        Ok(Self {
            roml: Flash::new(roml),
            romh: Flash::new(romh),
            ram: [0; 256],
            bank: 0,
            control: 0,
            crt: crt.clone(),
        })
    }

    fn offset(&self, addr: u16) -> usize {
        usize::from(self.bank) * BANK + usize::from(addr & 0x1fff)
    }
}

impl ExpansionCart for EasyFlash {
    fn name(&self) -> &'static str {
        "EasyFlash"
    }

    fn lines(&self) -> CartLines {
        let game = if self.control & 4 != 0 {
            self.control & 1 != 0
        } else {
            true // boot jumper
        };
        CartLines {
            game,
            exrom: self.control & 2 != 0,
            ..CartLines::default()
        }
    }

    fn peek_roml(&self, addr: u16) -> Option<u8> {
        Some(self.roml.peek(self.offset(addr)))
    }

    fn peek_romh(&self, addr: u16) -> Option<u8> {
        Some(self.romh.peek(self.offset(addr)))
    }

    fn write_roml(&mut self, addr: u16, v: u8) {
        let o = self.offset(addr);
        self.roml.write(o, v);
    }

    fn write_romh(&mut self, addr: u16, v: u8) {
        let o = self.offset(addr);
        self.romh.write(o, v);
    }

    fn peek_io(&self, addr: u16) -> Option<u8> {
        (addr & 0xff00 == 0xdf00).then(|| self.ram[usize::from(addr & 0xff)])
    }

    fn write_io(&mut self, addr: u16, v: u8) {
        match addr {
            0xdf00..=0xdfff => self.ram[usize::from(addr & 0xff)] = v,
            _ => match addr & 0x0f {
                0 => self.bank = v & 0x3f,
                2 => self.control = v & 0x87,
                _ => {}
            },
        }
    }

    fn reset(&mut self) {
        self.bank = 0;
        self.control = 0;
        self.roml.reset();
        self.romh.reset();
    }

    fn written_image(&self) -> Option<Crt> {
        if !self.roml.modified && !self.romh.modified {
            return None;
        }
        let mut crt = self.crt.clone();
        for c in &mut crt.chips {
            let src = if c.load == 0x8000 {
                &self.roml
            } else {
                &self.romh
            };
            let start = usize::from(c.bank) * BANK;
            let n = c.data.len();
            c.data.copy_from_slice(&src.data[start..start + n]);
        }
        Some(crt)
    }

    fn save(&self, w: &mut StateWriter) {
        // The flash contents are media; RAM, registers and chip states are
        // state.
        w.chunk(*b"EZFL", 1, |w| {
            w.raw(&self.ram);
            w.u8(self.bank);
            w.u8(self.control);
            self.roml.save(w);
            self.romh.save(w);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"EZFL", 1)?;
        let mut n = self.clone();
        c.raw_into(&mut n.ram)?;
        n.bank = c.u8()? & 0x3f;
        n.control = c.u8()? & 0x87;
        n.roml.load(&mut c)?;
        n.romh.load(&mut c)?;
        *self = n;
        Ok(())
    }
}

/// A blank-but-bootable EasyFlash image: all 64 banks as chips (so every
/// flash write can go back into the file), bank 0 ROMH holding `boot` at
/// its end (the reset vector area `$FFxx`).
pub fn blank_image(boot: &[u8]) -> Crt {
    let mut chips = Vec::new();
    for bank in 0..64u16 {
        for load in [0x8000, 0xa000] {
            let mut data = vec![0xff; BANK];
            if bank == 0 && load == 0xa000 {
                data[BANK - boot.len()..].copy_from_slice(boot);
            }
            chips.push(Chip {
                kind: 2,
                bank,
                load,
                data,
            });
        }
    }
    Crt {
        machine: media::crt::CrtMachine::C64,
        version: 0x0100,
        hardware: media::crt::hw::EASYFLASH,
        exrom: true,
        game: false,
        subtype: 0,
        name: "EASYFLASH".into(),
        chips,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boots_in_ultimax_from_romh_bank_0() {
        let mut crt = blank_image(&[0x00, 0xe0]);
        for c in &mut crt.chips {
            if c.bank == 0 && c.load == 0xa000 {
                c.data[0] = 0x4c;
            }
        }
        let bytes = crt.to_bytes();
        let ef = EasyFlash::from_crt(&Crt::parse(&bytes).unwrap()).unwrap();
        let l = ef.lines();
        assert!(l.game && !l.exrom, "Ultimax");
        assert_eq!(ef.peek_romh(0xe000), Some(0x4c));
        assert_eq!(ef.peek_romh(0xfffe), Some(0x00));
        assert_eq!(ef.peek_romh(0xffff), Some(0xe0));
    }

    #[test]
    fn programs_flash_through_roml() {
        let mut ef = EasyFlash::from_crt(&blank_image(&[])).unwrap();
        for (v, a) in [(0xaa, 0x8555), (0x55, 0x82aa), (0xa0, 0x8555)] {
            ef.write_roml(a, v);
        }
        ef.write_io(0xde00, 1);
        ef.write_roml(0x8010, 0x5a);
        assert_eq!(ef.peek_roml(0x8010), Some(0x5a));
        assert!(ef.written_image().is_some());
    }
}
