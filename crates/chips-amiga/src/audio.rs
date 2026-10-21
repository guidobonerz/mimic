//! Paula's four audio channels (HRM chapter 5, "Audio hardware").
//!
//! Registers per channel x at $0A0 + $10·x: AUDxLC (location, high/low),
//! AUDxLEN (length in words, 0 = 65536), AUDxPER (period in colour
//! clocks), AUDxVOL (0–64), AUDxDAT (two signed 8-bit samples, high byte
//! first).
//!
//! **DMA** (DMACON AUDxEN): when a channel's DMA is switched on it copies
//! LC and LEN, fetches words through its DMA slot and plays them; at the
//! end of the block it starts over from LC/LEN. Each block start raises
//! the channel's interrupt (AUD0–AUD3, level 4) when its first word
//! arrives, so software can set up the next block meanwhile. Without DMA,
//! words the CPU writes to AUDxDAT play once each and raise the
//! interrupt when taken. Every period one byte is output.
//!
//! Channels 0 and 3 are the left output, 1 and 2 the right. ADKCON
//! attach modes (volume/period modulation) are not modelled yet.

use emu_core::{StateError, StateReader, StateWriter};

use crate::chipram::ChipRam;

/// Shortest period the chip can fetch for (DMA).
const MIN_PERIOD: u16 = 124;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub lc: u32,
    pub len: u16,
    pub per: u16,
    pub vol: u8,
    pub dat: u16,
    pt: u32,
    remaining: u32,
    dma: bool,
    /// A word for `dat` is wanted from DMA.
    want: bool,
    /// The next fetched word starts a block (interrupt).
    block_start: bool,
    /// `dat` holds a word not yet played.
    dat_full: bool,
    counter: u16,
    word: u16,
    /// Bytes of `word` left to play (2, 1, 0).
    bytes: u8,
    pub output: i8,
}

impl Channel {
    fn start_block(&mut self) {
        self.pt = self.lc;
        self.remaining = if self.len == 0 {
            0x1_0000
        } else {
            u32::from(self.len)
        };
        self.block_start = true;
    }

    /// DMACON AUDxEN changed.
    pub fn set_dma(&mut self, on: bool) {
        if on && !self.dma {
            self.start_block();
            self.want = true;
            self.dat_full = false;
            self.bytes = 0;
            self.counter = 0;
        }
        if !on {
            self.want = false;
        }
        self.dma = on;
    }

    /// The CPU wrote AUDxDAT.
    pub fn write_dat(&mut self, v: u16) {
        self.dat = v;
        self.dat_full = true;
        if !self.dma && self.bytes == 0 {
            self.counter = 0;
        }
    }

    pub fn wants_slot(&self) -> bool {
        self.dma && self.want
    }

    /// One audio DMA slot. Returns true if the channel's interrupt is due.
    pub fn fetch(&mut self, chip: &ChipRam, mask: u32) -> bool {
        self.dat = chip.word(self.pt);
        self.dat_full = true;
        self.want = false;
        self.pt = self.pt.wrapping_add(2) & mask;
        let irq = std::mem::take(&mut self.block_start);
        self.remaining -= 1;
        if self.remaining == 0 {
            self.start_block();
        }
        irq
    }

    /// One colour clock. Returns true if the channel's interrupt is due
    /// (without DMA: a CPU word was taken).
    pub fn tick(&mut self) -> bool {
        if self.counter > 1 {
            self.counter -= 1;
            return false;
        }
        let period = if self.dma {
            self.per.max(MIN_PERIOD)
        } else {
            self.per.max(1)
        };
        let mut irq = false;
        if self.bytes == 0 {
            if !self.dat_full {
                // Nothing to play: the output holds, the period waits.
                self.counter = 0;
                return false;
            }
            self.word = self.dat;
            self.dat_full = false;
            self.bytes = 2;
            if self.dma {
                self.want = true;
            } else {
                irq = true;
            }
        }
        self.output = if self.bytes == 2 {
            (self.word >> 8) as i8
        } else {
            self.word as i8
        };
        self.bytes -= 1;
        self.counter = period;
        irq
    }

    /// The channel's level: sample × volume (−8192 … 8128).
    pub fn level(&self) -> i32 {
        i32::from(self.output) * i32::from(self.vol.min(64))
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        w.u32(self.lc);
        w.u32(self.pt);
        w.u32(self.remaining);
        for v in [self.len, self.per, self.dat, self.counter, self.word] {
            w.u16(v);
        }
        w.u8(self.vol);
        w.u8(self.bytes);
        w.u8(self.output as u8);
        for b in [self.dma, self.want, self.block_start, self.dat_full] {
            w.bool(b);
        }
    }

    pub(crate) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut c = Self {
            lc: r.u32()?,
            pt: r.u32()?,
            remaining: r.u32()?,
            ..Self::default()
        };
        for v in [
            &mut c.len,
            &mut c.per,
            &mut c.dat,
            &mut c.counter,
            &mut c.word,
        ] {
            *v = r.u16()?;
        }
        c.vol = r.u8()?;
        c.bytes = r.u8()?;
        c.output = r.u8()? as i8;
        for b in [&mut c.dma, &mut c.want, &mut c.block_start, &mut c.dat_full] {
            *b = r.bool()?;
        }
        if c.bytes > 2 || c.remaining > 0x1_0000 {
            return Err(StateError::Invalid("audio channel"));
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dma_plays_a_block_and_loops_with_interrupts() {
        let mut chip = ChipRam::new(0x1_0000);
        chip.set_word(0x100, 0x7f80);
        chip.set_word(0x102, 0x0102);
        let mut c = Channel {
            lc: 0x100,
            len: 2,
            per: 124,
            vol: 64,
            ..Channel::default()
        };
        c.set_dma(true);
        let mut out = Vec::new();
        let mut irqs = 0;
        for _ in 0..124 * 8 {
            if c.wants_slot() && c.fetch(&chip, 0xfffe) {
                irqs += 1;
            }
            if c.tick() {
                irqs += 1;
            }
            if out.last() != Some(&c.output) {
                out.push(c.output);
            }
        }
        assert_eq!(&out[..5], &[127, -128, 1, 2, 127]);
        assert!(irqs >= 2, "one interrupt per block start");
        assert_eq!(c.level(), i32::from(c.output) * 64);
    }

    #[test]
    fn cpu_written_samples_interrupt_when_taken() {
        let mut c = Channel {
            per: 10,
            vol: 32,
            ..Channel::default()
        };
        c.write_dat(0x4020);
        assert!(c.tick(), "word taken");
        assert_eq!(c.output, 0x40);
        for _ in 0..10 {
            assert!(!c.tick());
        }
        assert_eq!(c.output, 0x20);
    }
}
