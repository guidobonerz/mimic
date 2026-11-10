//! TMC0430 GROM ("graphics read-only memory"): serial-access ROMs of 6 KB
//! in 8 KB windows, eight of them sharing one auto-incrementing 16-bit
//! address (GROM 0–2 in the TI-99/4A console, 3–7 in a cartridge).
//!
//! The port is byte-wide:
//! - Writing the address: high byte, then low byte. After the low byte
//!   the GROM fetches the byte at the address into its buffer and counts
//!   on (prefetch).
//! - Reading data: the buffer; then the next byte is fetched.
//! - Reading the address: the high byte; the low byte moves up, so a
//!   second read gives the low byte. Reading also resets the address
//!   write sequence.
//! - The address counts within its GROM: the upper three bits (the GROM
//!   number) stay.
//!
//! A GROM holds 6 KB: the last 2 KB of its window read as the OR of the
//! bytes 2 KB and 4 KB lower, unless the chip's data fills the window.
//!
//! Sources: Texas Instruments, *TI-99/4A Console and Peripheral Expansion
//! System Technical Data* (1983), GROM interface; Thierry Nouspikel, "The
//! TI-99/4A Tech Pages", GROMs (the address read-back and the 6 KB
//! mirror). The implementation is this project's own.

use emu_core::{StateError, StateReader, StateWriter};

/// Bytes a GROM holds.
pub const GROM_SIZE: usize = 0x1800;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grom {
    /// The eight GROMs (empty: not fitted, reads 0).
    pub chips: [Vec<u8>; 8],
    addr: u16,
    buffer: u8,
    /// The high address byte written, waiting for the low one.
    high: Option<u8>,
}

fn next(addr: u16) -> u16 {
    (addr & 0xe000) | (addr.wrapping_add(1) & 0x1fff)
}

impl Grom {
    pub fn new() -> Self {
        Self::default()
    }

    /// The byte at `addr` of the GROM space (side-effect free).
    pub fn byte(&self, addr: u16) -> u8 {
        let chip = &self.chips[usize::from(addr >> 13)];
        let o = usize::from(addr & 0x1fff);
        let at = |i: usize| chip.get(i).copied().unwrap_or(0);
        // A dump of the whole 8 KB window (as GROM files usually are)
        // holds its last 2 KB itself.
        if o < GROM_SIZE.max(chip.len()) {
            at(o)
        } else {
            at(o - 0x800) | at(o - 0x1000)
        }
    }

    /// The current address (where the next fetch reads).
    pub fn address(&self) -> u16 {
        self.addr
    }

    pub fn write_address(&mut self, v: u8) {
        match self.high.take() {
            None => self.high = Some(v),
            Some(h) => {
                self.addr = u16::from_be_bytes([h, v]);
                self.buffer = self.byte(self.addr);
                self.addr = next(self.addr);
            }
        }
    }

    pub fn read_address(&mut self) -> u8 {
        self.high = None;
        let [h, l] = self.addr.to_be_bytes();
        self.addr = u16::from_be_bytes([l, l]);
        h
    }

    pub fn read_data(&mut self) -> u8 {
        self.high = None;
        let v = self.buffer;
        self.buffer = self.byte(self.addr);
        self.addr = next(self.addr);
        v
    }

    /// Writing data: GROMs are read-only; the address still counts.
    pub fn write_data(&mut self, _v: u8) {
        self.high = None;
        self.addr = next(self.addr);
    }

    /// What a data read would return, without reading.
    pub fn peek_data(&self) -> u8 {
        self.buffer
    }

    /// The address state, not the contents (they are ROM).
    pub fn save(&self, w: &mut StateWriter) {
        w.u16(self.addr);
        w.u8(self.buffer);
        match self.high {
            Some(h) => {
                w.bool(true);
                w.u8(h);
            }
            None => w.bool(false),
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let addr = r.u16()?;
        let buffer = r.u8()?;
        let high = if r.bool()? { Some(r.u8()?) } else { None };
        (self.addr, self.buffer, self.high) = (addr, buffer, high);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grom() -> Grom {
        let mut g = Grom::new();
        g.chips[0] = (0..GROM_SIZE).map(|i| i as u8).collect();
        g.chips[2] = vec![0x55; 0x2000];
        g.chips[1] = vec![0xaa; GROM_SIZE];
        g
    }

    #[test]
    fn address_write_prefetches_and_reads_count_on() {
        let mut g = grom();
        g.write_address(0x00);
        g.write_address(0x10);
        assert_eq!(g.read_data(), 0x10);
        assert_eq!(g.read_data(), 0x11);
        assert_eq!(g.address(), 0x0013, "one ahead: the prefetch");
    }

    #[test]
    fn the_address_stays_within_its_grom() {
        let mut g = grom();
        g.write_address(0x1f);
        g.write_address(0xff);
        assert_eq!(g.address(), 0x0000, "wraps to the start of GROM 0");
        g.write_address(0x3f);
        g.write_address(0xff);
        assert_eq!(g.address(), 0x2000);
    }

    #[test]
    fn the_last_two_kb_mirror_as_an_or() {
        let g = grom();
        assert_eq!(g.byte(0x1800), g.byte(0x1000) | g.byte(0x0800));
        assert_eq!(g.byte(0x2000), 0xaa);
        assert_eq!(g.byte(0x5800), 0x55, "a full 8 KB dump");
        assert_eq!(g.byte(0x6000), 0, "not fitted");
    }

    #[test]
    fn reading_the_address_gives_high_then_low() {
        let mut g = grom();
        g.write_address(0x12);
        g.write_address(0x34);
        // After the prefetch the address is >1235.
        assert_eq!(g.read_address(), 0x12);
        assert_eq!(g.read_address(), 0x35);
    }

    #[test]
    fn savestate_round_trip() {
        let mut g = grom();
        g.write_address(0x01);
        let mut w = StateWriter::new();
        g.save(&mut w);
        let bytes = w.into_bytes();
        let mut h = grom();
        h.load(&mut StateReader::new(&bytes)).unwrap();
        h.write_address(0x00);
        assert_eq!(h.read_data(), 0x00);
        assert_eq!(h.address(), 0x0102);
    }
}
