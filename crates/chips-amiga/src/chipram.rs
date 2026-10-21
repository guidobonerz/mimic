//! Chip RAM: the memory Agnus' DMA channels and the CPU share. Big-endian
//! 16-bit words; addresses wrap at its size (a 512 KB A500 sees its chip
//! RAM mirrored up to $1FFFFF).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipRam {
    words: Vec<u16>,
}

impl ChipRam {
    /// `bytes`: a power of two, at least 64 KB.
    pub fn new(bytes: usize) -> Self {
        debug_assert!(bytes.is_power_of_two() && bytes >= 0x1_0000);
        Self {
            words: vec![0; bytes / 2],
        }
    }

    pub fn len_bytes(&self) -> usize {
        self.words.len() * 2
    }

    fn index(&self, addr: u32) -> usize {
        (addr as usize >> 1) & (self.words.len() - 1)
    }

    pub fn word(&self, addr: u32) -> u16 {
        self.words[self.index(addr)]
    }

    pub fn set_word(&mut self, addr: u32, v: u16) {
        let i = self.index(addr);
        self.words[i] = v;
    }

    pub fn byte(&self, addr: u32) -> u8 {
        let w = self.word(addr);
        if addr & 1 == 0 {
            (w >> 8) as u8
        } else {
            w as u8
        }
    }

    pub fn set_byte(&mut self, addr: u32, v: u8) {
        let i = self.index(addr);
        let w = &mut self.words[i];
        *w = if addr & 1 == 0 {
            (*w & 0x00ff) | u16::from(v) << 8
        } else {
            (*w & 0xff00) | u16::from(v)
        };
    }

    /// Power-on contents.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }
}

const TAG: [u8; 4] = *b"CHIP";

impl Savestate for ChipRam {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u32(self.words.len() as u32);
            for v in &self.words {
                w.u16(*v);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        if c.u32()? as usize != self.words.len() {
            return Err(StateError::Invalid("chip RAM size differs"));
        }
        let mut words = vec![0; self.words.len()];
        for v in &mut words {
            *v = c.u16()?;
        }
        self.words = words;
        Ok(())
    }
}
