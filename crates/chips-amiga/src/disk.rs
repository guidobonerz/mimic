//! Paula's disk controller (HRM chapter 8, "Disk"; DSKLEN, DSKBYTR,
//! DSKSYNC, ADKCON in appendix A).
//!
//! - **DSKLEN**: bit 15 DMAEN, 14 WRITE, 13–0 length in words. DMA starts
//!   when DSKLEN is written twice with DMAEN set; a write with DMAEN clear
//!   stops it. The end of the transfer raises DSKBLK (level 1).
//! - **Reading**: the drive's bits shift into a 16-bit register. A match
//!   with DSKSYNC raises DSKSYN (level 5); with ADKCON WORDSYNC (bit 10)
//!   reading DMA waits for the first match and every match realigns the
//!   word boundary. The first sync word is not transferred, later ones
//!   are. Words go to a small FIFO that the disk DMA slots empty into chip
//!   RAM.
//! - **Writing**: the disk DMA slots fill the same FIFO with words that are
//!   shifted out MSB first; DSKBLK follows the last bit.
//! - **DSKBYTR**: 15 DSKBYT (a byte arrived, cleared by reading), 14 DMAON,
//!   13 DSKWRITE, 12 WORDEQUAL, 7–0 the last byte.

use emu_core::{StateError, StateReader, StateWriter};

use crate::chipram::ChipRam;
use crate::int;

/// ADKCON WORDSYNC.
pub const WORDSYNC: u16 = 0x0400;
/// ADKCON FAST: 2 µs (MFM) bit cells, else 4 µs.
pub const FAST: u16 = 0x0100;

const FIFO: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskCtl {
    pub pt: u32,
    armed: bool,
    pub active: bool,
    pub write: bool,
    pub remaining: u16,
    shift: u16,
    bits: u8,
    synced: bool,
    fifo: [u16; FIFO],
    fifo_len: u8,
    out_word: u16,
    out_bits: u8,
    byte: u8,
    byte_ready: bool,
    word_equal: bool,
}

impl DiskCtl {
    pub fn new() -> Self {
        Self::default()
    }

    /// DSKLEN. Returns interrupt requests (an empty transfer ends at once).
    pub fn write_dsklen(&mut self, v: u16) -> u16 {
        if v & 0x8000 == 0 {
            self.active = false;
            self.armed = false;
            return 0;
        }
        if !self.armed {
            self.armed = true;
            return 0;
        }
        self.active = true;
        self.write = v & 0x4000 != 0;
        self.remaining = v & 0x3fff;
        self.synced = false;
        self.fifo_len = 0;
        self.out_bits = 0;
        if self.remaining == 0 {
            self.active = false;
            return int::DSKBLK;
        }
        0
    }

    fn push(&mut self, w: u16) {
        if usize::from(self.fifo_len) < FIFO {
            self.fifo[usize::from(self.fifo_len)] = w;
            self.fifo_len += 1;
        }
    }

    fn pop(&mut self) -> u16 {
        let w = self.fifo[0];
        self.fifo.copy_within(1.., 0);
        self.fifo_len -= 1;
        w
    }

    /// One bit cell read from the drives. Returns interrupt requests.
    pub fn bit_in(&mut self, bit: bool, sync: u16, adkcon: u16) -> u16 {
        self.shift = self.shift << 1 | u16::from(bit);
        self.bits += 1;
        let reading = self.active && !self.write;
        self.word_equal = self.shift == sync;
        let mut ints = 0;
        if self.word_equal {
            ints = int::DSKSYN;
            if adkcon & WORDSYNC != 0 {
                if reading {
                    if self.synced {
                        self.push(self.shift);
                    }
                    self.synced = true;
                }
                self.bits = 0;
                return ints;
            }
        }
        if self.bits.is_multiple_of(8) {
            self.byte = self.shift as u8;
            self.byte_ready = true;
        }
        if self.bits >= 16 {
            self.bits = 0;
            if reading && (self.synced || adkcon & WORDSYNC == 0) {
                self.push(self.shift);
            }
        }
        ints
    }

    /// One bit cell while writing: the bit to write (`None`: not writing)
    /// and interrupt requests.
    pub fn bit_out(&mut self) -> (Option<bool>, u16) {
        if !self.active || !self.write {
            return (None, 0);
        }
        if self.out_bits == 0 {
            if self.fifo_len > 0 {
                self.out_word = self.pop();
                self.out_bits = 16;
            } else if self.remaining == 0 {
                self.active = false;
                return (None, int::DSKBLK);
            } else {
                return (None, 0);
            }
        }
        let bit = self.out_word & 0x8000 != 0;
        self.out_word <<= 1;
        self.out_bits -= 1;
        (Some(bit), 0)
    }

    /// Does the controller want a disk DMA slot?
    pub fn wants_slot(&self) -> bool {
        self.active
            && if self.write {
                usize::from(self.fifo_len) < FIFO && self.remaining > 0
            } else {
                self.fifo_len > 0
            }
    }

    /// One disk DMA slot. Returns interrupt requests.
    pub fn dma(&mut self, chip: &mut ChipRam, mask: u32) -> u16 {
        if self.write {
            self.push(chip.word(self.pt));
            self.pt = self.pt.wrapping_add(2) & mask;
            self.remaining -= 1;
            return 0;
        }
        let w = self.pop();
        chip.set_word(self.pt, w);
        self.pt = self.pt.wrapping_add(2) & mask;
        self.remaining -= 1;
        if self.remaining == 0 {
            self.active = false;
            return int::DSKBLK;
        }
        0
    }

    pub fn dskbytr(&self) -> u16 {
        u16::from(self.byte_ready) << 15
            | u16::from(self.active) << 14
            | u16::from(self.active && self.write) << 13
            | u16::from(self.word_equal) << 12
            | u16::from(self.byte)
    }

    /// Reading DSKBYTR clears DSKBYT.
    pub fn read_dskbytr(&mut self) -> u16 {
        let v = self.dskbytr();
        self.byte_ready = false;
        v
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        w.u32(self.pt);
        for b in [
            self.armed,
            self.active,
            self.write,
            self.synced,
            self.byte_ready,
            self.word_equal,
        ] {
            w.bool(b);
        }
        for v in [self.remaining, self.shift, self.out_word] {
            w.u16(v);
        }
        for v in self.fifo {
            w.u16(v);
        }
        w.u8(self.fifo_len);
        w.u8(self.bits);
        w.u8(self.out_bits);
        w.u8(self.byte);
    }

    pub(crate) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut d = Self::new();
        d.pt = r.u32()?;
        let mut flags = [false; 6];
        for f in &mut flags {
            *f = r.bool()?;
        }
        [
            d.armed,
            d.active,
            d.write,
            d.synced,
            d.byte_ready,
            d.word_equal,
        ] = [flags[0], flags[1], flags[2], flags[3], flags[4], flags[5]];
        d.remaining = r.u16()?;
        d.shift = r.u16()?;
        d.out_word = r.u16()?;
        for v in &mut d.fifo {
            *v = r.u16()?;
        }
        d.fifo_len = r.u8()?;
        d.bits = r.u8()?;
        d.out_bits = r.u8()?;
        d.byte = r.u8()?;
        if usize::from(d.fifo_len) > FIFO || d.bits > 16 || d.out_bits > 16 {
            return Err(StateError::Invalid("disk controller"));
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(d: &mut DiskCtl, words: &[u16], adkcon: u16) -> u16 {
        let mut ints = 0;
        for w in words {
            for i in (0..16).rev() {
                ints |= d.bit_in(w >> i & 1 != 0, 0x4489, adkcon);
            }
        }
        ints
    }

    #[test]
    fn word_sync_starts_and_aligns_the_transfer() {
        let mut chip = ChipRam::new(0x1_0000);
        let mut d = DiskCtl::new();
        d.pt = 0x100;
        d.write_dsklen(0x8000 | 3);
        d.write_dsklen(0x8000 | 3);
        assert!(d.active);
        // Noise, then $4489 $4489, then data.
        let ints = feed(&mut d, &[0xaaaa, 0x1234], WORDSYNC);
        assert_eq!(d.fifo_len, 0, "nothing before the sync");
        assert_eq!(ints, 0);
        let ints = feed(&mut d, &[0x4489, 0x4489, 0x1111, 0x2222], WORDSYNC);
        assert_ne!(ints & int::DSKSYN, 0);
        let mut ints = 0;
        while d.wants_slot() {
            ints |= d.dma(&mut chip, 0xfffe);
        }
        assert_eq!(
            [chip.word(0x100), chip.word(0x102), chip.word(0x104)],
            [0x4489, 0x1111, 0x2222],
            "the second sync word is data"
        );
        assert_ne!(ints & int::DSKBLK, 0);
        assert!(!d.active);
    }

    #[test]
    fn writes_shift_out_and_end_with_dskblk() {
        let mut chip = ChipRam::new(0x1_0000);
        chip.set_word(0x200, 0xa5f0);
        let mut d = DiskCtl::new();
        d.pt = 0x200;
        d.write_dsklen(0xc001);
        d.write_dsklen(0xc001);
        assert!(d.wants_slot());
        d.dma(&mut chip, 0xfffe);
        let mut bits = 0u16;
        let mut ints = 0;
        for _ in 0..17 {
            let (b, i) = d.bit_out();
            ints |= i;
            if let Some(b) = b {
                bits = bits << 1 | u16::from(b);
            }
        }
        assert_eq!(bits, 0xa5f0);
        assert_eq!(ints, int::DSKBLK);
        assert_eq!(d.dskbytr() & 0x4000, 0);
    }

    #[test]
    fn dskbytr_reports_bytes() {
        let mut d = DiskCtl::new();
        feed(&mut d, &[0x00c3], 0);
        let v = d.read_dskbytr();
        assert_eq!(v & 0x80ff, 0x80c3);
        assert_eq!(d.dskbytr() & 0x8000, 0, "cleared by reading");
    }
}
