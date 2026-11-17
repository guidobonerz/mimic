//! Z80 CTC: four counter/timer channels.
//!
//! One port per channel. A write is, in this order:
//! - the **time constant** if the last control word announced one (0
//!   stands for 256);
//! - a **control word** if bit 0 is set:
//!
//!   | Bit | Meaning |
//!   | --- | --- |
//!   | 7 | interrupt enable |
//!   | 6 | 1 = counter mode (counts edges on CLK/TRG), 0 = timer mode (counts the system clock through the prescaler) |
//!   | 5 | prescaler: 1 = 256, 0 = 16 (timer mode) |
//!   | 4 | active edge of CLK/TRG: 1 = rising |
//!   | 3 | timer start: 1 = on the edge of CLK/TRG, 0 = when the time constant is loaded |
//!   | 2 | a time constant follows |
//!   | 1 | reset: the channel stops until a time constant is loaded |
//!   | 0 | 1 |
//!
//! - else the **interrupt vector** (channel 0's port only): bits 7–3; the
//!   chip puts the channel number into bits 2–1, bit 0 is 0.
//!
//! A read gives the down counter. At zero the counter reloads the time
//! constant, pulses ZC/TO and requests an interrupt if enabled. Channel 0
//! has the highest priority.
//!
//! The edges on CLK/TRG arrive through [`Ctc::trigger`], whatever their
//! polarity: the machine calls it on the edge the channel is set to
//! ([`Ctc::rising`]).
//!
//! Source: Zilog, "Z80 CTC Counter/Timer Circuit" product specification;
//! Z80 Family CPU Peripherals User Manual (UM0081).

use mimic_core::{StateError, StateReader, StateWriter};

use crate::Chain;

pub const INT_ENABLE: u8 = 0x80;
pub const COUNTER_MODE: u8 = 0x40;
pub const PRESCALE_256: u8 = 0x20;
pub const RISING: u8 = 0x10;
pub const TRIGGER_START: u8 = 0x08;
pub const CONSTANT_FOLLOWS: u8 = 0x04;
pub const RESET: u8 = 0x02;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Channel {
    control: u8,
    /// Time constant, 1–256.
    constant: u16,
    /// Down counter, 1–256.
    counter: u16,
    /// System clocks until the prescaler's next output, 1 to its divisor.
    prescale: u16,
    constant_follows: bool,
    /// Counting (a time constant is loaded and, in timer mode with a
    /// trigger start, the edge came).
    running: bool,
    /// A time constant is loaded; the timer waits for its trigger.
    armed: bool,
}

impl Channel {
    fn divisor(&self) -> u16 {
        if self.control & PRESCALE_256 != 0 {
            256
        } else {
            16
        }
    }

    fn timer(&self) -> bool {
        self.control & COUNTER_MODE == 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ctc {
    ch: [Channel; 4],
    vector: u8,
    pub chain: Chain<4>,
}

impl Ctc {
    pub fn new() -> Self {
        let mut c = Self::default();
        c.reset();
        c
    }

    /// /RESET: all channels stop, interrupts off. The vector stays.
    pub fn reset(&mut self) {
        let vector = self.vector;
        *self = Self::default();
        self.vector = vector;
        for c in &mut self.ch {
            c.constant = 256;
            c.counter = 256;
            c.prescale = 16;
        }
    }

    pub fn write(&mut self, channel: usize, v: u8) {
        let n = channel & 3;
        let c = &mut self.ch[n];
        if c.constant_follows {
            c.constant_follows = false;
            c.constant = if v == 0 { 256 } else { u16::from(v) };
            if !c.running && !c.armed {
                // A stopped channel starts with the new constant; a running
                // one takes it at its next zero.
                c.counter = c.constant;
                c.prescale = c.divisor();
                if c.timer() && c.control & TRIGGER_START != 0 {
                    c.armed = true;
                } else {
                    c.running = true;
                }
            }
        } else if v & 1 != 0 {
            c.control = v & 0xf8;
            c.constant_follows = v & CONSTANT_FOLLOWS != 0;
            if v & RESET != 0 {
                c.running = false;
                c.armed = false;
            }
            if v & INT_ENABLE == 0 {
                self.chain.request[n] = false;
            }
        } else if n == 0 {
            self.vector = v & 0xf8;
        }
    }

    /// The channel's down counter. No side effects.
    pub fn read(&self, channel: usize) -> u8 {
        self.ch[channel & 3].counter as u8
    }

    /// The channel counts rising edges of CLK/TRG (else falling ones).
    pub fn rising(&self, channel: usize) -> bool {
        self.ch[channel & 3].control & RISING != 0
    }

    /// An active edge on the channel's CLK/TRG. True if the counter
    /// reached zero (a ZC/TO pulse).
    pub fn trigger(&mut self, channel: usize) -> bool {
        let n = channel & 3;
        let c = &mut self.ch[n];
        if c.timer() {
            if c.armed {
                c.armed = false;
                c.running = true;
            }
            return false;
        }
        if !c.running {
            return false;
        }
        c.counter -= 1;
        if c.counter > 0 {
            return false;
        }
        c.counter = c.constant;
        if c.control & INT_ENABLE != 0 {
            self.chain.request[n] = true;
        }
        true
    }

    /// Run `t` system clocks. `zero(channel, offset)` is called for every
    /// ZC/TO pulse of a timer, `offset` clocks into the span (1 to `t`),
    /// in rising order per channel.
    pub fn advance(&mut self, t: u32, mut zero: impl FnMut(usize, u32)) {
        for n in 0..4 {
            let c = &mut self.ch[n];
            if !c.running || !c.timer() {
                continue;
            }
            let div = u32::from(c.divisor());
            let mut left = t;
            let mut at = 0;
            loop {
                // Clocks until the counter reaches zero.
                let total = u32::from(c.prescale) + (u32::from(c.counter) - 1) * div;
                if left < total {
                    let rest = total - left;
                    c.counter = ((rest - 1) / div + 1) as u16;
                    c.prescale = ((rest - 1) % div + 1) as u16;
                    break;
                }
                left -= total;
                at += total;
                c.counter = c.constant;
                c.prescale = div as u16;
                if c.control & INT_ENABLE != 0 {
                    self.chain.request[n] = true;
                }
                zero(n, at);
            }
        }
    }

    /// The vector of the pending interrupt (given IEI high).
    pub fn vector(&self) -> Option<u8> {
        self.chain.pending().map(|n| self.vector | (n as u8) << 1)
    }

    /// Register values for debugger views: control, time constant and
    /// counter per channel.
    pub fn channel(&self, n: usize) -> (u8, u16, u16) {
        let c = &self.ch[n & 3];
        (
            c.control | u8::from(c.running) | u8::from(c.armed) << 1,
            c.constant,
            c.counter,
        )
    }

    pub fn vector_base(&self) -> u8 {
        self.vector
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"ZCTC", 1, |w| {
            w.u8(self.vector);
            self.chain.save(w);
            for c in &self.ch {
                w.u8(c.control);
                w.u16(c.constant);
                w.u16(c.counter);
                w.u16(c.prescale);
                w.bool(c.constant_follows);
                w.bool(c.running);
                w.bool(c.armed);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"ZCTC", 1)?;
        let mut n = Self {
            vector: r.u8()? & 0xf8,
            ..Self::default()
        };
        n.chain.load(&mut r)?;
        for c in &mut n.ch {
            c.control = r.u8()? & 0xf8;
            c.constant = r.u16()?;
            c.counter = r.u16()?;
            c.prescale = r.u16()?;
            c.constant_follows = r.bool()?;
            c.running = r.bool()?;
            c.armed = r.bool()?;
            let in_range = |v: u16, max: u16| (1..=max).contains(&v);
            if !in_range(c.constant, 256)
                || !in_range(c.counter, 256)
                || !in_range(c.prescale, c.divisor())
            {
                return Err(StateError::Invalid("CTC counter"));
            }
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timer_counts_the_clock_through_its_prescaler() {
        let mut c = Ctc::new();
        c.write(0, 0xf8);
        // Timer, prescaler 16, interrupt, constant 10.
        c.write(1, INT_ENABLE | CONSTANT_FOLLOWS | RESET | 1);
        c.write(1, 10);
        let mut zeros = Vec::new();
        c.advance(159, |n, at| zeros.push((n, at)));
        assert!(zeros.is_empty());
        assert_eq!(c.read(1), 1);
        c.advance(1 + 320, |n, at| zeros.push((n, at)));
        assert_eq!(zeros, [(1, 1), (1, 161), (1, 321)]);
        assert_eq!(c.read(1), 10);
        assert_eq!(c.vector(), Some(0xfa));
        assert_eq!(c.chain.acknowledge(), Some(1));
        assert_eq!(c.vector(), None);
    }

    #[test]
    fn the_same_count_in_one_span_or_in_many() {
        let mut a = Ctc::new();
        let mut b = Ctc::new();
        for c in [&mut a, &mut b] {
            c.write(2, PRESCALE_256 | CONSTANT_FOLLOWS | RESET | 1);
            c.write(2, 3);
        }
        let (mut za, mut zb) = (0, 0);
        a.advance(5000, |_, _| za += 1);
        for step in [7, 23, 4, 11].iter().cycle().take(1000) {
            b.advance(*step, |_, _| zb += 1);
        }
        let done: u32 = [7, 23, 4, 11].iter().cycle().take(1000).sum();
        a.advance(done - 5000, |_, _| za += 1);
        assert_eq!(za, zb);
        assert_eq!(a, b);
    }

    #[test]
    fn a_counter_counts_edges_and_a_reset_stops_it() {
        let mut c = Ctc::new();
        c.write(3, COUNTER_MODE | CONSTANT_FOLLOWS | RESET | 1);
        assert!(!c.trigger(3), "stopped until the constant is there");
        c.write(3, 2);
        assert!(!c.trigger(3));
        assert!(c.trigger(3));
        assert_eq!(c.read(3), 2);
        assert_eq!(c.vector(), None, "interrupt not enabled");
        c.advance(100_000, |_, _| panic!("a counter ignores the clock"));
        c.write(3, COUNTER_MODE | RESET | 1);
        assert!(!c.trigger(3));
        assert!(!c.trigger(3));
    }

    #[test]
    fn a_running_channel_takes_a_new_constant_at_zero() {
        let mut c = Ctc::new();
        c.write(0, CONSTANT_FOLLOWS | RESET | 1);
        c.write(0, 4);
        c.advance(16, |_, _| {});
        c.write(0, CONSTANT_FOLLOWS | 1);
        c.write(0, 9);
        assert_eq!(c.read(0), 3);
        c.advance(48, |_, _| {});
        assert_eq!(c.read(0), 9);
    }

    #[test]
    fn a_timer_with_trigger_start_waits_for_the_edge() {
        let mut c = Ctc::new();
        c.write(0, TRIGGER_START | CONSTANT_FOLLOWS | RESET | 1);
        c.write(0, 1);
        let mut n = 0;
        c.advance(64, |_, _| n += 1);
        assert_eq!(n, 0);
        c.trigger(0);
        c.advance(64, |_, _| n += 1);
        assert_eq!(n, 4);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = Ctc::new();
        c.write(0, 0x40);
        c.write(2, INT_ENABLE | CONSTANT_FOLLOWS | RESET | 1);
        c.write(2, 100);
        c.advance(777, |_, _| {});
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Ctc::new();
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
        for cut in 0..bytes.len() {
            let _ = d.load(&mut StateReader::new(&bytes[..cut]));
        }
    }
}
