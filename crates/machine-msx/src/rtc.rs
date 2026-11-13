//! Ricoh RP5C01 real-time clock of the MSX2 (Ricoh RP5C01 data sheet;
//! MSX2 Technical Handbook, chapter 5, "Clock and battery-powered
//! memory").
//!
//! Sixteen 4-bit registers behind an address latch (port $B4) and a data
//! port ($B5; the high nibble reads 1). Registers 0–12 belong to one of
//! four blocks, chosen by the mode register:
//!
//! | Register | Block 0 (time)      | Block 1 (alarm)        | Blocks 2, 3 |
//! | -------- | ------------------- | ---------------------- | ----------- |
//! | 0, 1     | seconds, tens       | —                      | RAM         |
//! | 2, 3     | minutes, tens       | alarm minutes          | RAM         |
//! | 4, 5     | hours, tens         | alarm hours            | RAM         |
//! | 6        | day of the week     | alarm day of the week  | RAM         |
//! | 7, 8     | day, tens           | alarm day              | RAM         |
//! | 9, 10    | month, tens         | —, 12/24 (bit 0: 24 h) | RAM         |
//! | 11, 12   | year, tens          | leap counter (0: leap) | RAM         |
//!
//! Register 13, the mode: bit 3 the clock runs, bit 2 alarm on, bits 1–0
//! the block. Register 14 (test) and 15 (reset: bit 0 clears the alarm,
//! bit 1 the fraction of the second) are write-only and read 0. The MSX2
//! counts years from 1980 and keeps its screen settings in blocks 2 and 3.
//!
//! Golden rule 3: the clock never reads the host's. It starts at
//! 1990-01-01 00:00:00 (a Monday) in 24-hour mode and is advanced by the
//! machine, one [`Rp5c01::second`] per emulated second. In 12-hour mode
//! the hours still count 0–23 (the MSX2 BIOS sets 24 hours). The alarm
//! output and the 16 Hz/1 Hz outputs are not connected on an MSX2 and not
//! modelled.

use emu_core::{StateError, StateReader, StateWriter};

/// The bits each register of a block has.
const MASKS: [[u8; 13]; 4] = [
    [15, 7, 15, 7, 15, 3, 7, 15, 3, 15, 1, 15, 15],
    [0, 0, 15, 7, 15, 3, 7, 15, 3, 0, 1, 3, 0],
    [15; 13],
    [15; 13],
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rp5c01 {
    /// The address latch.
    pub addr: u8,
    /// Register 13.
    pub mode: u8,
    pub blocks: [[u8; 13]; 4],
}

impl Default for Rp5c01 {
    fn default() -> Self {
        Self::new()
    }
}

impl Rp5c01 {
    pub fn new() -> Self {
        let mut blocks = [[0; 13]; 4];
        // 1990-01-01, a Monday; years count from 1980.
        blocks[0][6] = 1;
        blocks[0][7] = 1;
        blocks[0][9] = 1;
        blocks[0][12] = 1;
        blocks[1][10] = 1;
        blocks[1][11] = 2;
        Self {
            addr: 0,
            mode: 8,
            blocks,
        }
    }

    pub fn write_addr(&mut self, v: u8) {
        self.addr = v & 15;
    }

    /// The data port (side-effect free).
    pub fn read(&self) -> u8 {
        let block = usize::from(self.mode & 3);
        let v = match self.addr {
            13 => self.mode,
            14 | 15 => 0,
            r => self.blocks[block][usize::from(r)],
        };
        0xf0 | v
    }

    pub fn write(&mut self, v: u8) {
        let block = usize::from(self.mode & 3);
        match self.addr {
            13 => self.mode = v & 15,
            14 => {}
            15 => {
                if v & 1 != 0 {
                    self.blocks[1][2..9].fill(0);
                }
            }
            r => {
                let r = usize::from(r);
                self.blocks[block][r] = v & MASKS[block][r];
            }
        }
    }

    /// Two registers as a number.
    fn get(&self, r: usize) -> u8 {
        self.blocks[0][r] + 10 * self.blocks[0][r + 1]
    }

    fn set(&mut self, r: usize, v: u8) {
        self.blocks[0][r] = v % 10;
        self.blocks[0][r + 1] = (v / 10) & MASKS[0][r + 1];
    }

    /// Count `r`/`r + 1` up; at `limit` back to `first`, true for a carry.
    fn count(&mut self, r: usize, first: u8, limit: u8) -> bool {
        let v = self.get(r) + 1;
        let carry = v >= limit;
        self.set(r, if carry { first } else { v });
        carry
    }

    /// One second has passed.
    pub fn second(&mut self) {
        if self.mode & 8 == 0 {
            return;
        }
        if !(self.count(0, 0, 60) && self.count(2, 0, 60) && self.count(4, 0, 24)) {
            return;
        }
        self.blocks[0][6] = (self.blocks[0][6] + 1) % 7;
        let days = match self.get(9) {
            2 if self.blocks[1][11] == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if self.count(7, 1, days + 1) && self.count(9, 1, 13) {
            self.count(11, 0, 100);
            self.blocks[1][11] = (self.blocks[1][11] + 1) & 3;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"5C01", 1, |w| {
            w.u8(self.addr);
            w.u8(self.mode);
            for b in &self.blocks {
                w.raw(b);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"5C01", 1)?;
        let addr = c.u8()? & 15;
        let mode = c.u8()? & 15;
        let mut blocks = [[0u8; 13]; 4];
        for (b, masks) in blocks.iter_mut().zip(MASKS) {
            c.raw_into(b)?;
            for (v, m) in b.iter_mut().zip(masks) {
                *v &= m;
            }
        }
        // Digits the counting relies on.
        for r in [0, 2, 4, 7, 9, 11] {
            if blocks[0][r] > 9 {
                return Err(StateError::Invalid("clock digit"));
            }
        }
        *self = Self { addr, mode, blocks };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(c: &Rp5c01) -> [u8; 6] {
        [c.get(11), c.get(9), c.get(7), c.get(4), c.get(2), c.get(0)]
    }

    #[test]
    fn blocks_masks_and_the_mode_register() {
        let mut c = Rp5c01::new();
        c.write_addr(13);
        assert_eq!(c.read(), 0xf8);
        c.write(0x0a); // clock on, block 2
        c.write_addr(0);
        c.write(0xfa);
        assert_eq!(c.read(), 0xfa);
        assert_eq!(c.blocks[2][0], 10);
        c.write_addr(13);
        c.write(8);
        c.write_addr(1);
        c.write(0xff);
        assert_eq!(c.read(), 0xf7, "tens of seconds have three bits");
        c.write_addr(15);
        assert_eq!(c.read(), 0xf0);
    }

    #[test]
    fn the_clock_counts_through_months_and_leap_years() {
        let mut c = Rp5c01::new();
        assert_eq!(date(&c), [10, 1, 1, 0, 0, 0]);
        for _ in 0..86_400 * 31 {
            c.second();
        }
        assert_eq!(date(&c), [10, 2, 1, 0, 0, 0]);
        assert_eq!(c.blocks[0][6], (1 + 31) % 7);
        // 1990 has no 29 February; 1992 has.
        for _ in 0..86_400 * 28 {
            c.second();
        }
        assert_eq!(date(&c), [10, 3, 1, 0, 0, 0]);
        for _ in 0..86_400 * (306 + 365 + 31 + 28) {
            c.second();
        }
        assert_eq!(date(&c), [12, 2, 29, 0, 0, 0]);
        assert_eq!(c.blocks[1][11], 0);
        // Stopped, it stays.
        c.write_addr(13);
        c.write(0);
        c.second();
        assert_eq!(date(&c), [12, 2, 29, 0, 0, 0]);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = Rp5c01::new();
        c.blocks[3][5] = 9;
        for _ in 0..100_000 {
            c.second();
        }
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Rp5c01::new();
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
        assert!(d.load(&mut StateReader::new(&bytes[..9])).is_err());
    }
}
