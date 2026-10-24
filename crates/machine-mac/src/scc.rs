//! Zilog Z8530 SCC (serial communications controller): the Mac's two
//! serial ports (A modem, B printer) and the mouse's X1/Y1 lines on the
//! DCD inputs (DCDA horizontal, DCDB vertical).
//!
//! **Registers** (Zilog Z8530 Technical Manual): each channel has write
//! registers WR0–WR15 and read registers RR0–RR15, reached through the
//! control port: a write to WR0 sets the pointer for the next access
//! (bits 2–0, bits 5–3 = 001 adds 8) and gives commands; after that
//! access the pointer returns to 0. The data port reaches WR8/RR8.
//!
//! | Reg  | Use here                                                     |
//! | ---- | ------------------------------------------------------------ |
//! | WR0  | pointer; commands 010 reset ext/status interrupts, 111 reset highest IUS; 101 reset Tx int pending |
//! | WR1  | bit 0 ext/status int enable, bit 1 Tx int enable, bits 4–3 Rx int mode |
//! | WR2  | interrupt vector (shared)                                    |
//! | WR9  | bits 7–6 reset (11 hardware, 10 A, 01 B), 4 status high, 3 MIE, 0 VIS |
//! | WR14 | bit 4 local loopback                                         |
//! | WR15 | bit 3 DCD interrupt enable, bit 5 CTS, bit 7 break           |
//! | RR0  | 0 Rx character available, 2 Tx buffer empty, 3 DCD, 5 CTS   |
//! | RR2  | channel B: the vector modified by the highest pending interrupt |
//! | RR3  | channel A: pending bits (A Rx 5, A Tx 4, A ext 3, B Rx 2, B Tx 1, B ext 0) |
//!
//! External/status interrupts latch RR0's status bits until "reset
//! ext/status interrupts"; a DCD change while latched raises another one
//! after the reset. RR2 of channel B carries the status in bits 3–1 (or
//! 6–4 with "status high"): B Tx 000, B ext 001, B Rx 010, B special
//! 011, A Tx 100, A ext 101, A Rx 110, A special 111, and 011 when
//! nothing is pending. The transmitter takes [`BYTE_CLOCKS`] a character
//! (LocalTalk's 230.4 kbit/s, whatever the baud rate registers say): a
//! written character moves into the shift register at once if it is free
//! (the buffer is empty again: a Tx interrupt with WR1 bit 1), else waits
//! in the buffer; when the shift register runs dry the Tx underrun/EOM bit
//! (RR0 bit 6) is set, an external/status interrupt with WR15 bit 6
//! (AppleTalk's driver waits for it), and WR0's "reset Tx underrun/EOM
//! latch" (bits 7-6 = 11) clears it. With local loopback
//! (or the `loopback` option wiring a port to itself) they arrive in the
//! channel's own receiver. Baud rates, sync modes and DMA requests are
//! not modelled. The Mac reads the SCC at $9FFFF8 + 2·n and writes at
//! $BFFFF9 + 2·n: A1 picks channel A, A2 the data port.

use emu_core::{StateError, StateReader, StateWriter};
use std::collections::VecDeque;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub wr: [u8; 16],
    /// RR0 status bits as latched for an ext/status interrupt.
    latched: Option<u8>,
    pub dcd: bool,
    pub cts: bool,
    rx: VecDeque<u8>,
    ext_pending: bool,
    tx_pending: bool,
    /// The shift register sends until this clock; the buffered character.
    tx_busy: Option<u64>,
    tx_buf: Option<u8>,
    /// RR0 bit 6.
    underrun: bool,
    /// The port is wired back to itself.
    pub loopback: bool,
    /// Bytes sent, for tools (cleared when read).
    pub sent: Vec<u8>,
}

impl Channel {
    fn status(&self) -> u8 {
        u8::from(!self.rx.is_empty())
            | u8::from(self.tx_buf.is_none()) << 2
            | u8::from(self.dcd) << 3
            | 0x10 // sync/hunt: nothing on the line
            | u8::from(self.cts) << 5
            | u8::from(self.underrun) << 6
    }

    /// An external/status condition changed: latch RR0 and interrupt if
    /// enabled (`ie` = its WR15 bit).
    fn ext_event(&mut self, ie: u8) {
        if self.wr[15] & ie != 0 && self.wr[1] & 0x01 != 0 && !self.ext_pending {
            self.latched = Some(self.status());
            self.ext_pending = true;
        }
    }

    /// A character goes into the shift register: the buffer is empty.
    fn send(&mut self, v: u8) {
        self.sent.push(v);
        if self.loopback || self.wr[14] & 0x10 != 0 {
            self.rx.push_back(v);
        }
        if self.wr[1] & 0x02 != 0 {
            self.tx_pending = true;
        }
    }

    fn rr0(&self) -> u8 {
        self.latched.unwrap_or_else(|| self.status())
    }

    fn rx_int(&self) -> bool {
        !self.rx.is_empty() && self.wr[1] & 0x18 != 0
    }

    fn reset(&mut self) {
        let (dcd, cts, loopback) = (self.dcd, self.cts, self.loopback);
        *self = Self {
            dcd,
            cts,
            loopback,
            underrun: true,
            ..Self::default()
        };
        self.wr[15] = 0xf8;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scc {
    /// Channel A, channel B.
    pub ch: [Channel; 2],
    pointer: u8,
    /// CPU clock as of the last [`Scc::run_to`].
    clock: u64,
}

/// CPU clocks (7.8336 MHz) a character takes to send: 8 bits at 230.4
/// kbit/s.
pub const BYTE_CLOCKS: u64 = 272;

impl Default for Scc {
    fn default() -> Self {
        Self::new()
    }
}

impl Scc {
    pub fn new() -> Self {
        let mut s = Self {
            ch: [Channel::default(), Channel::default()],
            pointer: 0,
            clock: 0,
        };
        s.reset();
        s
    }

    pub fn reset(&mut self) {
        for c in &mut self.ch {
            c.reset();
        }
        self.pointer = 0;
    }

    fn mie(&self) -> bool {
        self.ch[0].wr[9] & 0x08 != 0
    }

    /// Pending interrupts, RR3 layout.
    pub fn pending(&self) -> u8 {
        let mut ip = 0;
        for (i, c) in self.ch.iter().enumerate() {
            let shift = if i == 0 { 3 } else { 0 };
            if c.ext_pending {
                ip |= 1 << shift;
            }
            if c.tx_pending {
                ip |= 2 << shift;
            }
            if c.rx_int() {
                ip |= 4 << shift;
            }
        }
        ip
    }

    /// The interrupt line (level 2 on the Mac).
    pub fn irq(&self) -> bool {
        self.mie() && self.pending() != 0
    }

    /// Status code of the highest pending interrupt (Zilog order: A Rx,
    /// A Tx, A ext, B Rx, B Tx, B ext).
    fn status_code(&self) -> u8 {
        let ip = self.pending();
        for (bit, code) in [(5, 6), (4, 4), (3, 5), (2, 2), (1, 0), (0, 1)] {
            if ip & (1 << bit) != 0 {
                return code;
            }
        }
        3
    }

    fn rr2_b(&self) -> u8 {
        let v = self.ch[0].wr[2];
        let code = self.status_code();
        if self.ch[0].wr[9] & 0x10 != 0 {
            (v & 0x8f) | code << 4
        } else {
            (v & 0xf1) | code << 1
        }
    }

    /// A DCD input level (mouse X1 on A, Y1 on B).
    pub fn set_dcd(&mut self, ch: usize, level: bool) {
        let c = &mut self.ch[ch];
        if c.dcd == level {
            return;
        }
        c.dcd = level;
        c.ext_event(0x08);
    }

    /// The next clock a transmitter finishes a character.
    pub fn next_event(&self) -> u64 {
        self.ch
            .iter()
            .filter_map(|c| c.tx_busy)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Run the transmitters to `now`.
    pub fn run_to(&mut self, now: u64) {
        self.clock = now;
        for c in &mut self.ch {
            while let Some(t) = c.tx_busy {
                if t > now {
                    break;
                }
                match c.tx_buf.take() {
                    Some(v) => {
                        c.tx_busy = Some(t + BYTE_CLOCKS);
                        c.send(v);
                    }
                    None => {
                        c.tx_busy = None;
                        if !c.underrun {
                            c.underrun = true;
                            c.ext_event(0x40);
                        }
                    }
                }
            }
        }
    }

    /// Bytes arriving at a port from outside.
    pub fn receive(&mut self, ch: usize, data: &[u8]) {
        self.ch[ch].rx.extend(data);
    }

    fn reg_read(&mut self, ch: usize, reg: u8, peek: bool) -> u8 {
        let c = &self.ch[ch];
        match reg {
            0 | 4 => c.rr0(),
            1 | 5 => 0x07,
            2 | 6 => {
                if ch == 1 {
                    self.rr2_b()
                } else {
                    self.ch[0].wr[2]
                }
            }
            3 | 7 => {
                if ch == 0 {
                    self.pending()
                } else {
                    0
                }
            }
            8 => {
                if peek {
                    c.rx.front().copied().unwrap_or(0)
                } else {
                    self.ch[ch].rx.pop_front().unwrap_or(0)
                }
            }
            10 | 14 => 0,
            12 => c.wr[12],
            13 | 9 => c.wr[13],
            15 | 11 => c.wr[15],
            _ => 0,
        }
    }

    fn reg_write(&mut self, ch: usize, reg: u8, v: u8) {
        match reg {
            0 => {
                self.pointer = v & 7 | if v >> 3 & 7 == 1 { 8 } else { 0 };
                let c = &mut self.ch[ch];
                match v >> 3 & 7 {
                    2 => {
                        c.ext_pending = false;
                        c.latched = None;
                    }
                    5 => c.tx_pending = false,
                    _ => {}
                }
                if v >> 6 == 3 {
                    c.underrun = false;
                }
                c.wr[0] = v;
                return;
            }
            2 => {
                self.ch[0].wr[2] = v;
                self.ch[1].wr[2] = v;
            }
            8 => {
                let now = self.clock;
                let c = &mut self.ch[ch];
                if c.tx_busy.is_none() {
                    c.tx_busy = Some(now + BYTE_CLOCKS);
                    c.send(v);
                } else {
                    c.tx_buf = Some(v);
                }
            }
            9 => {
                match v >> 6 {
                    3 => self.reset(),
                    2 => self.ch[0].reset(),
                    1 => self.ch[1].reset(),
                    _ => {}
                }
                self.ch[0].wr[9] = v & 0x3f;
                self.ch[1].wr[9] = v & 0x3f;
            }
            _ => self.ch[ch].wr[usize::from(reg)] = v,
        }
        self.pointer = 0;
    }

    /// A control (`data` false) or data port access of channel `ch`
    /// (0 = A, 1 = B).
    pub fn read(&mut self, ch: usize, data: bool) -> u8 {
        let reg = if data { 8 } else { self.pointer };
        let v = self.reg_read(ch, reg, false);
        if !data {
            self.pointer = 0;
        }
        v
    }

    pub fn peek(&self, ch: usize, data: bool) -> u8 {
        let reg = if data { 8 } else { self.pointer };
        self.clone().reg_read(ch, reg, true)
    }

    pub fn write(&mut self, ch: usize, data: bool, v: u8) {
        let reg = if data { 8 } else { self.pointer };
        self.reg_write(ch, reg, v);
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"Z853", 2, |w| {
            w.u8(self.pointer);
            for c in &self.ch {
                w.raw(&c.wr);
                w.bool(c.latched.is_some());
                w.u8(c.latched.unwrap_or(0));
                w.bool(c.dcd);
                w.bool(c.cts);
                w.bytes(&c.rx.iter().copied().collect::<Vec<u8>>());
                w.bool(c.ext_pending);
                w.bool(c.tx_pending);
                w.u64(c.tx_busy.unwrap_or(u64::MAX));
                w.u16(c.tx_buf.map_or(0xffff, u16::from));
                w.bool(c.underrun);
            }
            w.u64(self.clock);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"Z853", 2)?;
        let mut n = self.clone();
        n.pointer = c.u8()? & 15;
        for ch in &mut n.ch {
            c.raw_into(&mut ch.wr)?;
            let has = c.bool()?;
            let l = c.u8()?;
            ch.latched = has.then_some(l);
            ch.dcd = c.bool()?;
            ch.cts = c.bool()?;
            ch.rx = c.bytes()?.iter().copied().collect();
            ch.ext_pending = c.bool()?;
            ch.tx_pending = c.bool()?;
            let busy = c.u64()?;
            ch.tx_busy = (busy != u64::MAX).then_some(busy);
            let buf = c.u16()?;
            ch.tx_buf = (buf != 0xffff).then_some(buf as u8);
            ch.underrun = c.bool()?;
        }
        n.clock = c.u64()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wr(s: &mut Scc, ch: usize, reg: u8, v: u8) {
        if reg != 0 {
            s.write(ch, false, reg);
        }
        s.write(ch, false, v);
    }

    #[test]
    fn dcd_interrupt_and_modified_vector() {
        let mut s = Scc::new();
        wr(&mut s, 0, 9, 0xc0); // hardware reset
        wr(&mut s, 0, 15, 0x08);
        wr(&mut s, 0, 1, 0x01);
        wr(&mut s, 1, 2, 0x00);
        wr(&mut s, 0, 9, 0x08); // MIE, status low
        assert!(!s.irq());
        s.set_dcd(0, true);
        assert!(s.irq());
        // RR2 B: A ext/status = 101 in bits 3-1.
        s.write(1, false, 2);
        assert_eq!(s.read(1, false), 0x0a);
        // RR0 latched DCD high even after it falls.
        s.set_dcd(0, false);
        assert_eq!(s.read(0, false) & 0x08, 0x08);
        // Reset ext/status: unlatched, no interrupt.
        wr(&mut s, 0, 0, 0x10);
        assert!(!s.irq());
        assert_eq!(s.read(0, false) & 0x08, 0);
        s.write(1, false, 2);
        assert_eq!(s.read(1, false), 0x06);
    }

    #[test]
    fn loopback_and_savestate() {
        let mut s = Scc::new();
        wr(&mut s, 1, 14, 0x10);
        s.write(1, true, 0x42);
        s.run_to(1000);
        assert_eq!(s.read(1, false) & 1, 1);
        assert_eq!(s.read(1, true), 0x42);
        assert_eq!(s.read(1, false) & 1, 0);
        let mut w = StateWriter::new();
        s.save(&mut w);
        let b = w.into_bytes();
        let mut t = Scc::new();
        t.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(t.ch[1].wr[14], 0x10);
    }

    #[test]
    fn transmitter_underrun_interrupt() {
        let mut s = Scc::new();
        wr(&mut s, 0, 9, 0xc0);
        wr(&mut s, 1, 15, 0x40); // Tx underrun/EOM IE
        wr(&mut s, 1, 1, 0x03); // ext/status + Tx int
        wr(&mut s, 0, 9, 0x08);
        wr(&mut s, 1, 0, 0xc0); // reset Tx underrun/EOM latch
        s.run_to(100);
        s.write(1, true, 0x7e);
        s.write(1, true, 0x55);
        assert_eq!(s.read(1, false) & 0x44, 0, "buffer full, sending");
        s.run_to(100 + BYTE_CLOCKS);
        assert_eq!(
            s.read(1, false) & 0x04,
            0x04,
            "second byte in the shift register"
        );
        assert!(s.irq());
        wr(&mut s, 1, 0, 0x28); // reset Tx IP
        s.run_to(100 + 3 * BYTE_CLOCKS);
        assert_eq!(s.pending() & 0x01, 0x01, "B ext/status: underrun");
        assert_eq!(s.ch[1].sent, [0x7e, 0x55]);
    }
}
