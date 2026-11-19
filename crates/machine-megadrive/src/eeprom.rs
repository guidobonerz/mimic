//! The Xicor X24C01: 128 bytes of serial EEPROM on two wires (Wonder Boy
//! in Monster World, Mega Man: The Wily Wars; option `save=eeprom`).
//!
//! **Protocol** (Xicor X24C01 data sheet): a START (SDA falls while SCL is
//! high), the 7-bit word address and the R/W bit, an acknowledge (the chip
//! pulls SDA low on the ninth clock); a write then takes up to four bytes
//! within the address's 4-byte page (the low two address bits count), each
//! acknowledged, until a STOP (SDA rises while SCL is high); a read sends
//! bytes from the address on, the address counting over all 128, as long
//! as the master acknowledges. Bits on SDA change while SCL is low and are
//! taken on SCL's rising edge.
//!
//! The order of the bits within the address and data bytes is taken as
//! least significant first (the data sheet's figures label LSB before
//! MSB; not checked against a real cartridge yet).
//!
//! The cartridges wire it to $200001: bit 0 SDA (in and out), bit 1 SCL
//! (MacDonald, *Sega Genesis hardware notes* 4.1, for the address).

use mimic_core::savestate::{StateError, StateReader, StateWriter};

pub const SIZE: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Receiving the address byte (bit count).
    Address(u8),
    /// The acknowledge clock after the address.
    AddressAck,
    WriteData(u8),
    WriteAck,
    /// Sending a data byte (bit count).
    ReadData(u8),
    /// The master's acknowledge clock.
    ReadAck,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X24c01 {
    pub mem: [u8; SIZE],
    phase: Phase,
    scl: bool,
    sda_in: bool,
    /// What the chip drives (false: pulls SDA low).
    sda_out: bool,
    shift: u8,
    addr: u8,
    read: bool,
    /// Bytes written since the last save.
    pub dirty: bool,
}

impl Default for X24c01 {
    fn default() -> Self {
        Self::new()
    }
}

impl X24c01 {
    pub fn new() -> Self {
        Self {
            mem: [0xff; SIZE],
            phase: Phase::Idle,
            scl: true,
            sda_in: true,
            sda_out: true,
            shift: 0,
            addr: 0,
            read: false,
            dirty: false,
        }
    }

    /// SDA as the bus sees it.
    pub fn sda(&self) -> bool {
        self.sda_in && self.sda_out
    }

    /// The master sets SCL and SDA.
    pub fn write(&mut self, scl: bool, sda: bool) {
        let (old_scl, old_sda) = (self.scl, self.sda_in);
        self.scl = scl;
        self.sda_in = sda;
        if old_scl && scl && old_sda != sda {
            if !sda {
                // START.
                self.phase = Phase::Address(0);
                self.shift = 0;
                self.sda_out = true;
            } else {
                // STOP.
                self.phase = Phase::Idle;
                self.sda_out = true;
            }
            return;
        }
        if !old_scl && scl {
            self.rise();
        } else if old_scl && !scl {
            self.fall();
        }
    }

    fn rise(&mut self) {
        match self.phase {
            Phase::Address(n) => {
                self.shift |= u8::from(self.sda_in) << n;
                if n == 7 {
                    self.addr = self.shift & 0x7f;
                    self.read = self.shift & 0x80 != 0;
                    self.phase = Phase::AddressAck;
                } else {
                    self.phase = Phase::Address(n + 1);
                }
            }
            Phase::WriteData(n) => {
                self.shift |= u8::from(self.sda_in) << n;
                if n == 7 {
                    self.mem[usize::from(self.addr)] = self.shift;
                    self.dirty = true;
                    self.addr = (self.addr & 0x7c) | ((self.addr + 1) & 3);
                    self.phase = Phase::WriteAck;
                } else {
                    self.phase = Phase::WriteData(n + 1);
                }
            }
            Phase::ReadData(n) => {
                if n == 7 {
                    self.phase = Phase::ReadAck;
                } else {
                    self.phase = Phase::ReadData(n + 1);
                }
            }
            Phase::ReadAck => {
                if self.sda_in {
                    // No acknowledge: the read ends.
                    self.phase = Phase::Idle;
                } else {
                    self.addr = (self.addr + 1) & 0x7f;
                    self.phase = Phase::ReadData(0);
                }
            }
            _ => {}
        }
    }

    /// SCL low: the chip sets SDA for the next clock.
    fn fall(&mut self) {
        match self.phase {
            Phase::AddressAck | Phase::WriteAck => {
                if self.sda_out {
                    // Acknowledge on this low phase and the next clock.
                    self.sda_out = false;
                } else {
                    self.sda_out = true;
                    self.shift = 0;
                    self.phase = if self.read {
                        self.put_bit(0);
                        Phase::ReadData(0)
                    } else {
                        Phase::WriteData(0)
                    };
                }
            }
            Phase::ReadData(n) => self.put_bit(n),
            Phase::ReadAck => self.sda_out = true,
            _ => {}
        }
    }

    fn put_bit(&mut self, n: u8) {
        self.sda_out = self.mem[usize::from(self.addr)] >> n & 1 != 0;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"X24C", 1, |w| {
            w.raw(&self.mem);
            let (tag, n) = match self.phase {
                Phase::Idle => (0, 0),
                Phase::Address(n) => (1, n),
                Phase::AddressAck => (2, 0),
                Phase::WriteData(n) => (3, n),
                Phase::WriteAck => (4, 0),
                Phase::ReadData(n) => (5, n),
                Phase::ReadAck => (6, 0),
            };
            w.u8(tag);
            w.u8(n);
            w.bool(self.scl);
            w.bool(self.sda_in);
            w.bool(self.sda_out);
            w.u8(self.shift);
            w.u8(self.addr);
            w.bool(self.read);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"X24C", 1)?;
        let mut n = Self::new();
        c.raw_into(&mut n.mem)?;
        let tag = c.u8()?;
        let bits = c.u8()? & 7;
        n.phase = match tag {
            0 => Phase::Idle,
            1 => Phase::Address(bits),
            2 => Phase::AddressAck,
            3 => Phase::WriteData(bits),
            4 => Phase::WriteAck,
            5 => Phase::ReadData(bits),
            6 => Phase::ReadAck,
            _ => return Err(StateError::Invalid("EEPROM phase")),
        };
        n.scl = c.bool()?;
        n.sda_in = c.bool()?;
        n.sda_out = c.bool()?;
        n.shift = c.u8()?;
        n.addr = c.u8()? & 0x7f;
        n.read = c.bool()?;
        n.dirty = self.dirty;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Master<'a>(&'a mut X24c01);

    impl Master<'_> {
        fn start(&mut self) {
            self.0.write(true, true);
            self.0.write(true, false);
            self.0.write(false, false);
        }

        fn stop(&mut self) {
            self.0.write(false, false);
            self.0.write(true, false);
            self.0.write(true, true);
        }

        fn bit(&mut self, b: bool) -> bool {
            self.0.write(false, b);
            self.0.write(true, b);
            let seen = self.0.sda();
            self.0.write(false, b);
            seen
        }

        /// Send a byte LSB first; returns the acknowledge.
        fn send(&mut self, v: u8) -> bool {
            for n in 0..8 {
                self.bit(v >> n & 1 != 0);
            }
            !self.bit(true)
        }

        fn receive(&mut self, ack: bool) -> u8 {
            let mut v = 0;
            for n in 0..8 {
                v |= u8::from(self.bit(true)) << n;
            }
            self.bit(!ack);
            v
        }
    }

    #[test]
    fn page_write_and_sequential_read() {
        let mut e = X24c01::new();
        let mut m = Master(&mut e);
        m.start();
        assert!(m.send(5), "address acknowledged");
        for v in [0x11, 0x22, 0x33] {
            assert!(m.send(v));
        }
        m.stop();
        assert_eq!(&e.mem[5..8], &[0x11, 0x22, 0x33]);
        let mut m = Master(&mut e);
        m.start();
        assert!(m.send(5 | 0x80));
        let a = m.receive(true);
        let b = m.receive(false);
        m.stop();
        assert_eq!((a, b), (0x11, 0x22));
        assert!(e.dirty);
    }
}
