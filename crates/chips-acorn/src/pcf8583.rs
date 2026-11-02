//! PCF8583: clock and 240 bytes of battery-backed RAM on the I²C bus.
//!
//! Bit level: the machine calls [`Pcf8583::lines`] with SDA and SCL as the
//! bus master drives them and reads SDA back. Device address $A0 (write),
//! $A1 (read). Registers 0–15 clock and control, the rest RAM. The clock
//! does not run from the host's time (golden rule 3): it starts at a fixed
//! date and advances with the machine's seconds.
//!
//! Source: Philips PCF8583 data sheet; I²C bus specification.

use emu_core::{StateError, StateReader, StateWriter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Shifting in a byte, then the acknowledge.
    Receive,
    /// Shifting out a byte.
    Send,
    /// Our acknowledge bit on the bus.
    Ack,
    /// The master's acknowledge after a sent byte.
    MasterAck,
}

#[derive(Clone, Debug)]
pub struct Pcf8583 {
    pub ram: Vec<u8>,
    addr: u8,
    state: State,
    shift: u8,
    bits: u8,
    /// The next received byte is the device address / the word address.
    address_phase: u8,
    reading: bool,
    sda: bool,
    scl: bool,
    out: bool,
}

impl Default for Pcf8583 {
    fn default() -> Self {
        Self::new()
    }
}

impl Pcf8583 {
    pub fn new() -> Self {
        Self {
            ram: vec![0; 256],
            addr: 0,
            state: State::Idle,
            shift: 0,
            bits: 0,
            address_phase: 0,
            reading: false,
            sda: true,
            scl: true,
            out: true,
        }
    }

    /// What this device drives on SDA (high: released).
    pub fn sda_out(&self) -> bool {
        self.out
    }

    /// A second passed (only seconds, enough to see a running clock).
    pub fn second(&mut self) {
        let bcd = self.ram[2];
        let n = (bcd >> 4) * 10 + (bcd & 15) + 1;
        self.ram[2] = if n >= 60 { 0 } else { (n / 10) << 4 | (n % 10) };
    }

    fn load_out(&mut self) {
        self.shift = self.ram[usize::from(self.addr)];
        self.addr = self.addr.wrapping_add(1);
        self.bits = 0;
        self.out = self.shift & 0x80 != 0;
    }

    fn received(&mut self) {
        match self.address_phase {
            0 => {
                if self.shift & 0xfe != 0xa0 {
                    self.state = State::Idle;
                    self.out = true;
                    return;
                }
                self.reading = self.shift & 1 != 0;
                self.address_phase = 1;
            }
            1 => {
                self.addr = self.shift;
                self.address_phase = 2;
            }
            _ => {
                self.ram[usize::from(self.addr)] = self.shift;
                self.addr = self.addr.wrapping_add(1);
            }
        }
        self.state = State::Ack;
        self.out = false;
    }

    /// New levels on the bus.
    pub fn lines(&mut self, sda: bool, scl: bool) {
        let (old_sda, old_scl) = (self.sda, self.scl);
        self.sda = sda;
        self.scl = scl;
        if old_scl && scl {
            if old_sda && !sda {
                // Start.
                self.state = State::Receive;
                self.address_phase = 0;
                self.bits = 0;
                self.shift = 0;
                self.out = true;
            } else if !old_sda && sda {
                // Stop.
                self.state = State::Idle;
                self.out = true;
            }
            return;
        }
        if !old_scl && scl {
            // Rising clock: data is valid.
            match self.state {
                State::Receive => {
                    self.shift = self.shift << 1 | u8::from(sda);
                    self.bits += 1;
                }
                State::MasterAck if sda => self.state = State::Idle,
                _ => {}
            }
        } else if old_scl && !scl {
            // Falling clock: the bus may change.
            match self.state {
                State::Receive if self.bits == 8 => self.received(),
                State::Ack => {
                    if self.reading && self.address_phase == 1 {
                        self.state = State::Send;
                        self.load_out();
                    } else {
                        self.state = State::Receive;
                        self.bits = 0;
                        self.shift = 0;
                        self.out = true;
                    }
                }
                State::Send => {
                    self.bits += 1;
                    if self.bits == 8 {
                        self.state = State::MasterAck;
                        self.out = true;
                    } else {
                        self.out = self.shift >> (7 - self.bits) & 1 != 0;
                    }
                }
                State::MasterAck => {
                    self.state = State::Send;
                    self.load_out();
                }
                _ => {}
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"PCF8", 1, |w| {
            w.bytes(&self.ram);
            w.u8(self.addr);
            w.u8(self.state as u8);
            w.u8(self.shift);
            w.u8(self.bits);
            w.u8(self.address_phase);
            w.bool(self.reading);
            w.bool(self.sda);
            w.bool(self.scl);
            w.bool(self.out);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"PCF8", 1)?;
        let ram = c.bytes()?;
        if ram.len() != 256 {
            return Err(StateError::Invalid("RTC RAM"));
        }
        self.ram = ram.to_vec();
        self.addr = c.u8()?;
        self.state = match c.u8()? {
            0 => State::Idle,
            1 => State::Receive,
            2 => State::Send,
            3 => State::Ack,
            _ => State::MasterAck,
        };
        self.shift = c.u8()?;
        self.bits = c.u8()?;
        self.address_phase = c.u8()?;
        self.reading = c.bool()?;
        self.sda = c.bool()?;
        self.scl = c.bool()?;
        self.out = c.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small I²C master.
    struct Master<'a>(&'a mut Pcf8583);

    impl Master<'_> {
        fn start(&mut self) {
            self.0.lines(true, true);
            self.0.lines(false, true);
            self.0.lines(false, false);
        }
        fn stop(&mut self) {
            self.0.lines(false, false);
            self.0.lines(false, true);
            self.0.lines(true, true);
        }
        fn send(&mut self, b: u8) -> bool {
            for i in (0..8).rev() {
                let bit = b >> i & 1 != 0;
                self.0.lines(bit, false);
                self.0.lines(bit, true);
                self.0.lines(bit, false);
            }
            self.0.lines(true, false);
            self.0.lines(true, true);
            let ack = !self.0.sda_out();
            self.0.lines(true, false);
            ack
        }
        fn receive(&mut self, ack: bool) -> u8 {
            let mut v = 0;
            for _ in 0..8 {
                self.0.lines(true, false);
                self.0.lines(true, true);
                v = v << 1 | u8::from(self.0.sda_out());
                self.0.lines(true, false);
            }
            self.0.lines(!ack, false);
            self.0.lines(!ack, true);
            self.0.lines(!ack, false);
            v
        }
    }

    #[test]
    fn writes_and_reads_ram() {
        let mut c = Pcf8583::new();
        let mut m = Master(&mut c);
        m.start();
        assert!(m.send(0xa0));
        assert!(m.send(0x20));
        assert!(m.send(0x12));
        assert!(m.send(0x34));
        m.stop();
        m.start();
        assert!(m.send(0xa0));
        assert!(m.send(0x20));
        m.start();
        assert!(m.send(0xa1));
        assert_eq!(m.receive(true), 0x12);
        assert_eq!(m.receive(false), 0x34);
        m.stop();
    }

    #[test]
    fn other_addresses_get_no_ack() {
        let mut c = Pcf8583::new();
        let mut m = Master(&mut c);
        m.start();
        assert!(!m.send(0x70));
    }

    #[test]
    fn seconds_count_in_bcd() {
        let mut c = Pcf8583::new();
        c.ram[2] = 0x09;
        c.second();
        assert_eq!(c.ram[2], 0x10);
        c.ram[2] = 0x59;
        c.second();
        assert_eq!(c.ram[2], 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = Pcf8583::new();
        c.ram[9] = 5;
        let mut w = StateWriter::new();
        c.save(&mut w);
        let b = w.into_bytes();
        let mut d = Pcf8583::new();
        d.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(d.ram[9], 5);
    }
}
