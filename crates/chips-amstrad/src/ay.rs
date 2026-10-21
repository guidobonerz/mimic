//! General Instrument AY-3-8912 PSG: the register file and its I/O port
//! (General Instrument AY-3-8910/8912 data sheet). Sound generation follows
//! with the CPC's audio (Phase 6 M4).
//!
//! | Reg   | Function                         | Reg | Function                    |
//! | ----- | -------------------------------- | --- | --------------------------- |
//! | 0–5   | tone periods A/B/C (fine, coarse 4 bits) | 8–10 | amplitudes (bit 4 envelope) |
//! | 6     | noise period (5 bits)            | 11–12 | envelope period          |
//! | 7     | mixer; bit 6 port A output       | 13  | envelope shape              |
//! |       |                                  | 14  | I/O port A                  |
//!
//! The bus is driven by BDIR/BC1 (BC2 tied high on the CPC): 11 latches
//! the register number, 10 writes, 01 reads, 00 is inactive. On the CPC
//! port A is the keyboard's column input.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Writable bits per register.
const MASKS: [u8; 16] = [
    0xff, 0x0f, 0xff, 0x0f, 0xff, 0x0f, 0x1f, 0xff, 0x1f, 0x1f, 0x1f, 0xff, 0xff, 0x0f, 0xff, 0xff,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ay {
    pub regs: [u8; 16],
    /// Latched register number.
    pub addr: u8,
}

impl Default for Ay {
    fn default() -> Self {
        Self::new()
    }
}

impl Ay {
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            addr: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// A bus cycle with BDIR/BC1 = `function` (bits 1–0) and `data` on the
    /// bus (writes and latches).
    pub fn bus_write(&mut self, function: u8, data: u8) {
        match function & 3 {
            3 => self.addr = data & 0x0f,
            2 => self.regs[usize::from(self.addr)] = data & MASKS[usize::from(self.addr)],
            _ => {}
        }
    }

    /// What a read cycle puts on the bus, with `port_a` the pins of I/O
    /// port A (an input unless register 7 bit 6 makes it an output).
    pub fn read(&self, port_a: u8) -> u8 {
        match self.addr {
            14 if self.regs[7] & 0x40 == 0 => port_a,
            14 => self.regs[14] & port_a,
            a => self.regs[usize::from(a)],
        }
    }
}

const TAG: [u8; 4] = *b"AY38";
const VERSION: u16 = 1;

impl Savestate for Ay {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.addr);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = Self::new();
        c.raw_into(&mut n.regs)?;
        n.addr = c.u8()? & 0x0f;
        for (v, m) in n.regs.iter_mut().zip(MASKS) {
            *v &= m;
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latch_write_read_and_port() {
        let mut ay = Ay::new();
        ay.bus_write(3, 1);
        ay.bus_write(2, 0xff);
        assert_eq!(ay.read(0xff), 0x0f, "coarse tone is 4 bits");
        ay.bus_write(3, 14);
        assert_eq!(ay.read(0xbf), 0xbf, "port A input");
        ay.bus_write(0, 0x55);
        assert_eq!(ay.addr, 14, "inactive cycle changes nothing");
    }

    #[test]
    fn savestate_round_trip() {
        let mut ay = Ay::new();
        ay.bus_write(3, 7);
        ay.bus_write(2, 0x38);
        let mut w = StateWriter::new();
        ay.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Ay::new();
        b.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(ay, b);
    }
}
