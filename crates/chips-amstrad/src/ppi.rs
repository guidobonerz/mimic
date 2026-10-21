//! 8255 PPI, mode 0 as the CPC uses it (cpcwiki, "8255"; Intel 8255A data
//! sheet).
//!
//! | Port  | CPC use                                                         |
//! | ----- | --------------------------------------------------------------- |
//! | &F4xx | A: the AY-3-8912's data bus (output, input to read the AY)     |
//! | &F5xx | B (input): 7 cassette in, 6 printer busy, 5 /EXP, 4 50 Hz link, 3–1 distributor links, 0 VSYNC |
//! | &F6xx | C (output): 7–6 AY BDIR/BC1, 5 cassette out, 4 motor, 3–0 keyboard line |
//! | &F7xx | control: bit 7 = 1 mode set (bit 4 port A input, 1 port B input, 3/0 port C upper/lower input; resets the output latches), bit 7 = 0 sets (bit 0) port C bit 3–1 |
//!
//! Reading an output port returns its latch. Port C pins that are inputs
//! are not driven by the PPI: they read (and the CPC's chips see) the
//! levels the board's pull-ups and pull-downs give them, [`Ppi::c_pulls`];
//! on the CPC &2F (keyboard line 15, cassette out high, motor off, AY
//! inactive; Arnold acid test `ppi`, `port_c_input`).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ppi {
    /// Output latches.
    pub a: u8,
    pub b: u8,
    pub c: u8,
    /// Last mode-set control word.
    pub control: u8,
    /// Levels of undriven port C pins (wiring, not state).
    pub c_pulls: u8,
}

impl Default for Ppi {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppi {
    /// After reset all ports are inputs; undriven port C pins float high.
    pub fn new() -> Self {
        Self::with_c_pulls(0xff)
    }

    /// A PPI whose undriven port C pins read `c_pulls`.
    pub fn with_c_pulls(c_pulls: u8) -> Self {
        Self {
            a: 0,
            b: 0,
            c: 0,
            control: 0x9b,
            c_pulls,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::with_c_pulls(self.c_pulls);
    }

    pub fn a_is_input(&self) -> bool {
        self.control & 0x10 != 0
    }

    /// Port C as the pins see it: outputs drive, inputs take the pulls.
    pub fn c_pins(&self) -> u8 {
        let mut inputs = 0;
        if self.control & 0x08 != 0 {
            inputs |= 0xf0;
        }
        if self.control & 0x01 != 0 {
            inputs |= 0x0f;
        }
        self.c & !inputs | self.c_pulls & inputs
    }

    /// Port A's pins when it is an output.
    pub fn a_out(&self) -> u8 {
        if self.a_is_input() {
            0xff
        } else {
            self.a
        }
    }

    /// Write port `reg` (0–3).
    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 3 {
            0 => self.a = v,
            1 => self.b = v,
            2 => self.c = v,
            _ => {
                if v & 0x80 != 0 {
                    self.control = v;
                    self.a = 0;
                    self.b = 0;
                    self.c = 0;
                } else {
                    let bit = 1 << ((v >> 1) & 7);
                    if v & 1 != 0 {
                        self.c |= bit;
                    } else {
                        self.c &= !bit;
                    }
                }
            }
        }
    }

    /// Read port `reg` with what the outside drives onto ports A and B.
    pub fn read(&self, reg: u8, a_in: u8, b_in: u8) -> u8 {
        match reg & 3 {
            0 => {
                if self.a_is_input() {
                    a_in
                } else {
                    self.a
                }
            }
            1 => {
                if self.control & 0x02 != 0 {
                    b_in
                } else {
                    self.b
                }
            }
            2 => self.c_pins(),
            _ => 0xff,
        }
    }
}

const TAG: [u8; 4] = *b"PPI8";
const VERSION: u16 = 1;

impl Savestate for Ppi {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for v in [self.a, self.b, self.c, self.control] {
                w.u8(v);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let n = Self {
            a: c.u8()?,
            b: c.u8()?,
            c: c.u8()?,
            control: c.u8()?,
            c_pulls: self.c_pulls,
        };
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_set_and_port_c_bits() {
        let mut p = Ppi::new();
        assert!(p.a_is_input());
        p.write(3, 0x82); // A out, B in, C out
        assert!(!p.a_is_input());
        p.write(0, 0x12);
        assert_eq!(p.read(0, 0xff, 0x5e), 0x12);
        assert_eq!(p.read(1, 0xff, 0x5e), 0x5e);
        p.write(3, 0x09); // set C bit 4
        assert_eq!(p.read(2, 0, 0), 0x10);
        p.write(3, 0x08); // clear it
        assert_eq!(p.c, 0);
        p.write(2, 0x45);
        p.write(3, 0x92); // A in: latches reset
        assert_eq!((p.a, p.c), (0, 0));
        assert_eq!(p.read(0, 0xfe, 0), 0xfe);
    }

    #[test]
    fn port_c_inputs_read_the_pulls() {
        let mut p = Ppi::with_c_pulls(0x2f);
        p.write(3, 0x89); // C upper and lower input
        p.write(2, 0xd0);
        assert_eq!(p.read(2, 0, 0), 0x2f);
        p.write(3, 0x88); // C lower output
        p.write(2, 0xd5);
        assert_eq!(p.read(2, 0, 0), 0x25);
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Ppi::new();
        p.write(3, 0x82);
        p.write(2, 0xc0);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut q = Ppi::new();
        q.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(p, q);
    }
}
