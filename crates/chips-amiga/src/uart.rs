//! Paula's serial port (HRM chapter 8, "Serial port"; SERDAT, SERDATR,
//! SERPER and ADKCON UARTBRK in appendix A).
//!
//! - **SERPER**: bits 14–0 the bit period minus one in colour clocks
//!   (3.58 MHz / (N + 1) baud), bit 15 LONG: 9 data bits instead of 8.
//! - **SERDAT** (write): the word to send, data in the low bits with the
//!   stop bit(s) above them (software sets them). The transmitter sends a
//!   start bit (0) and then shifts the word out LSB first until it is
//!   empty, so the stop bits end the frame. SERDAT is a buffer in front of
//!   the shift register: a write moves to the shifter as soon as that is
//!   free, and TBE (INTREQ bit 0) is requested when the buffer empties.
//!   TSRE says the shifter is empty too.
//! - **SERDATR** (read): bit 15 OVRUN (a word arrived while RBF was still
//!   set), 14 RBF (a word arrived; cleared with INTREQ RBF), 13 TBE,
//!   12 TSRE, 11 RXD (the line as it is), 9–0 the received data with its
//!   stop bit(s) above.
//! - **Receiving**: a start bit's falling edge on RXD starts sampling at
//!   the middle of every bit for 8 or 9 data bits and one stop bit; the
//!   frame then lands in SERDATR and RBF (INTREQ bit 11) is requested.
//! - **UARTBRK** (ADKCON bit 11): TXD is held low.
//!
//! The other end of the line is the machine's business; [`Uart::loopback`]
//! ties TXD to RXD (tests, diagnostics software).

use emu_core::{StateError, StateReader, StateWriter};

use crate::int;

/// ADKCON UARTBRK.
pub const UARTBRK: u16 = 0x0800;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uart {
    pub serper: u16,
    /// TXD wired to RXD.
    pub loopback: bool,
    /// The level a device drives on RXD (idle high).
    rxd_in: bool,
    /// A word in the SERDAT buffer, not yet in the shifter.
    tx_buf: Option<u16>,
    tx_shift: u16,
    /// Bits of the frame still to send (start bit included); 0 = idle.
    tx_bits: u8,
    /// The start bit is the next bit out.
    tx_start: bool,
    tx_counter: u16,
    /// TXD as driven now (idle high).
    txd: bool,
    /// Received frame: data with stop bits.
    pub serdatr: u16,
    rbf: bool,
    ovrun: bool,
    rx_shift: u16,
    /// Bits still expected in the frame being received; 0 = waiting for
    /// a start bit.
    rx_bits: u8,
    rx_counter: u16,
    /// RXD at the previous colour clock (edge detection).
    rxd_prev: bool,
}

impl Default for Uart {
    fn default() -> Self {
        Self::new()
    }
}

impl Uart {
    /// Lines idle (high), nothing in flight.
    pub fn new() -> Self {
        Self {
            serper: 0,
            loopback: false,
            rxd_in: true,
            tx_buf: None,
            tx_shift: 0,
            tx_bits: 0,
            tx_start: false,
            tx_counter: 0,
            txd: true,
            serdatr: 0,
            rbf: false,
            ovrun: false,
            rx_shift: 0,
            rx_bits: 0,
            rx_counter: 0,
            rxd_prev: true,
        }
    }

    fn bit_period(&self) -> u16 {
        (self.serper & 0x7fff).wrapping_add(1).max(1)
    }

    fn data_bits(&self) -> u8 {
        if self.serper & 0x8000 != 0 {
            9
        } else {
            8
        }
    }

    /// The level on RXD now.
    pub fn rxd(&self) -> bool {
        if self.loopback {
            self.txd
        } else {
            self.rxd_in
        }
    }

    /// TXD as driven now (`adkcon`: UARTBRK forces it low).
    pub fn txd(&self, adkcon: u16) -> bool {
        self.txd && adkcon & UARTBRK == 0
    }

    /// A device drives RXD.
    pub fn set_rxd(&mut self, level: bool) {
        self.rxd_in = level;
    }

    /// The CPU wrote SERDAT. Returns interrupt requests.
    pub fn write_serdat(&mut self, v: u16) -> u16 {
        self.tx_buf = Some(v);
        self.load_shifter()
    }

    /// Move the buffer into an idle shifter; TBE when that happens.
    fn load_shifter(&mut self) -> u16 {
        if self.tx_bits != 0 {
            return 0;
        }
        let Some(w) = self.tx_buf.take() else {
            return 0;
        };
        self.tx_shift = w;
        // Start bit, then the word up to its highest set bit.
        self.tx_bits = 1 + (16 - w.leading_zeros() as u8).max(1);
        self.tx_start = true;
        self.tx_counter = 0;
        int::TBE
    }

    /// INTREQ RBF was cleared (reading the frame): OVRUN clears with it.
    pub fn clear_rbf(&mut self) {
        self.rbf = false;
        self.ovrun = false;
    }

    /// SERDATR.
    pub fn serdatr(&self) -> u16 {
        u16::from(self.ovrun) << 15
            | u16::from(self.rbf) << 14
            | u16::from(self.tx_buf.is_none()) << 13
            | u16::from(self.tx_bits == 0) << 12
            | u16::from(self.rxd()) << 11
            | (self.serdatr & 0x3ff)
    }

    /// One colour clock. Returns interrupt requests.
    pub fn tick(&mut self) -> u16 {
        let mut ints = 0;
        // Transmit.
        if self.tx_bits != 0 {
            if self.tx_counter == 0 {
                if std::mem::take(&mut self.tx_start) {
                    self.txd = false;
                } else {
                    self.txd = self.tx_shift & 1 != 0;
                    self.tx_shift >>= 1;
                }
                self.tx_bits -= 1;
                self.tx_counter = self.bit_period();
            }
            self.tx_counter -= 1;
            if self.tx_counter == 0 && self.tx_bits == 0 {
                self.txd = true;
                ints |= self.load_shifter();
            }
        }
        // Receive.
        let rxd = self.rxd();
        if self.rx_bits == 0 {
            if self.rxd_prev && !rxd {
                // Start bit: sample in the middle of each following bit.
                self.rx_bits = self.data_bits() + 1;
                self.rx_shift = 0;
                self.rx_counter = self.bit_period() + self.bit_period() / 2;
            }
        } else {
            self.rx_counter -= 1;
            if self.rx_counter == 0 {
                let total = self.data_bits() + 1;
                let i = total - self.rx_bits;
                self.rx_shift |= u16::from(rxd) << i;
                self.rx_bits -= 1;
                self.rx_counter = self.bit_period();
                if self.rx_bits == 0 {
                    self.serdatr = self.rx_shift;
                    if self.rbf {
                        self.ovrun = true;
                    }
                    self.rbf = true;
                    ints |= int::RBF;
                }
            }
        }
        self.rxd_prev = rxd;
        ints
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        w.u16(self.serper);
        w.bool(self.loopback);
        w.bool(self.rxd_in);
        w.bool(self.tx_buf.is_some());
        w.u16(self.tx_buf.unwrap_or(0));
        w.u16(self.tx_shift);
        w.u8(self.tx_bits);
        w.bool(self.tx_start);
        w.u16(self.tx_counter);
        w.bool(self.txd);
        w.u16(self.serdatr);
        w.bool(self.rbf);
        w.bool(self.ovrun);
        w.u16(self.rx_shift);
        w.u8(self.rx_bits);
        w.u16(self.rx_counter);
        w.bool(self.rxd_prev);
    }

    pub(crate) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut u = Self::new();
        u.serper = r.u16()?;
        u.loopback = r.bool()?;
        u.rxd_in = r.bool()?;
        let has_buf = r.bool()?;
        let buf = r.u16()?;
        u.tx_buf = has_buf.then_some(buf);
        u.tx_shift = r.u16()?;
        u.tx_bits = r.u8()?;
        u.tx_start = r.bool()?;
        u.tx_counter = r.u16()?;
        u.txd = r.bool()?;
        u.serdatr = r.u16()?;
        u.rbf = r.bool()?;
        u.ovrun = r.bool()?;
        u.rx_shift = r.u16()?;
        u.rx_bits = r.u8()?;
        u.rx_counter = r.u16()?;
        u.rxd_prev = r.bool()?;
        if u.tx_bits > 17 || u.rx_bits > 10 {
            return Err(StateError::Invalid("serial port"));
        }
        Ok(u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `n` clocks, collecting interrupt requests.
    fn run(u: &mut Uart, n: usize) -> u16 {
        (0..n).fold(0, |i, _| i | u.tick())
    }

    #[test]
    fn a_frame_goes_out_lsb_first_with_start_and_stop_bits() {
        let mut u = Uart::new();
        u.serper = 3; // 4 clocks per bit
        assert_eq!(u.serdatr() & 0x3800, 0x3800, "TBE, TSRE, RXD high");
        let ints = u.write_serdat(0x100 | 0xa5);
        assert_eq!(ints, int::TBE, "the buffer empties into the shifter");
        assert_eq!(u.serdatr() & 0x3000, 0x2000, "TBE set, TSRE clear");
        let mut line = Vec::new();
        for _ in 0..10 {
            u.tick();
            line.push(u.txd(0));
            run(&mut u, 3);
        }
        let expect: Vec<bool> = std::iter::once(false)
            .chain((0..8).map(|i| 0xa5 >> i & 1 != 0))
            .chain(std::iter::once(true))
            .collect();
        assert_eq!(line, expect);
        assert_eq!(u.serdatr() & 0x1000, 0x1000, "TSRE again");
        assert!(u.txd(0));
        assert!(!u.txd(UARTBRK), "break holds TXD low");
    }

    #[test]
    fn a_second_word_waits_in_the_buffer() {
        let mut u = Uart::new();
        u.serper = 0;
        u.write_serdat(0x101);
        assert_eq!(u.write_serdat(0x102), 0, "no TBE while the shifter is busy");
        assert_eq!(u.serdatr() & 0x2000, 0, "buffer full");
        let ints = run(&mut u, 10);
        assert_eq!(ints & int::TBE, int::TBE, "TBE when the shifter takes it");
    }

    #[test]
    fn loopback_receives_what_is_sent() {
        let mut u = Uart::new();
        u.loopback = true;
        u.serper = 7;
        u.write_serdat(0x100 | 0x5a);
        let ints = run(&mut u, 8 * 12);
        assert_eq!(ints & int::RBF, int::RBF);
        assert_eq!(u.serdatr() & 0x43ff, 0x415a, "RBF, data with stop bit");
        // A second frame before RBF is cleared: overrun.
        u.write_serdat(0x100 | 0x33);
        run(&mut u, 8 * 12);
        assert_eq!(u.serdatr() & 0xc3ff, 0xc133);
        u.clear_rbf();
        assert_eq!(u.serdatr() & 0xc000, 0);
    }

    #[test]
    fn nine_bit_frames() {
        let mut u = Uart::new();
        u.loopback = true;
        u.serper = 0x8000 | 1;
        u.write_serdat(0x200 | 0x1ff);
        run(&mut u, 2 * 12);
        assert_eq!(u.serdatr() & 0x3ff, 0x3ff);
    }

    #[test]
    fn savestate_round_trip() {
        let mut u = Uart::new();
        u.serper = 5;
        u.write_serdat(0x1aa);
        run(&mut u, 7);
        let mut w = StateWriter::new();
        u.save(&mut w);
        let bytes = w.into_bytes();
        let back = Uart::load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, u);
    }
}
