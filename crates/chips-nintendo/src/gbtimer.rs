//! The Game Boy's timer and divider.
//!
//! A 16-bit counter runs with the machine's clock; its upper byte is DIV
//! (`$FF04`; any write clears the whole counter). TIMA (`$FF05`) counts
//! falling edges of one of the counter's bits ANDed with TAC's enable
//! (`$FF07`: bit 2 the enable, bits 1–0 the rate: bit 9, 3, 5 or 7 of the
//! counter, that is 4096, 262144, 65536 or 16384 Hz). Because the edge
//! is taken behind that AND, writing DIV or TAC can count once more. A
//! write to TAC takes effect before the last clock of its machine cycle:
//! switching the timer on in the very cycle its bit falls still counts
//! (mooneye's `rapid_toggle`).
//!
//! When TIMA overflows it reads 0 for one machine cycle; then it is
//! loaded from TMA (`$FF06`) and the interrupt is requested. A write to
//! TIMA in that cycle cancels both; in the cycle of the load a write to
//! TIMA is ignored and one to TMA goes to TIMA too.
//!
//! Bit 12 of the counter (bit 4 of DIV) clocks the sound unit's frame
//! sequencer: [`GbTimer::tick`] reports its falling edges.
//!
//! Sources: the Pan Docs (<https://gbdev.io/pandocs/>: "Timer and Divider
//! Registers", "Timer obscure behaviour"); Joonas Javanainen, *Game Boy:
//! Complete Technical Reference*. The implementation is this project's
//! own.

use mimic_core::{Savestate, StateError, StateReader, StateWriter};

/// The timer's interrupt, as a bit of IF.
pub const INT_TIMER: u8 = 0x04;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GbTimer {
    /// The 16-bit counter; DIV is its upper byte.
    pub counter: u16,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    /// TIMA overflowed in the last machine cycle: the load is due.
    overflow: bool,
    /// TIMA was loaded in this machine cycle.
    loaded: bool,
}

impl GbTimer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The signal TIMA counts the falling edges of.
    fn signal(&self) -> bool {
        let bit = match self.tac & 3 {
            0 => 9,
            1 => 3,
            2 => 5,
            _ => 7,
        };
        self.tac & 4 != 0 && self.counter >> bit & 1 != 0
    }

    fn count(&mut self) {
        self.tima = self.tima.wrapping_add(1);
        if self.tima == 0 {
            self.overflow = true;
        }
    }

    fn set_counter(&mut self, v: u16) -> bool {
        let before = self.signal();
        let frame = self.counter & 0x1000 != 0 && v & 0x1000 == 0;
        self.counter = v;
        if before && !self.signal() {
            self.count();
        }
        frame
    }

    /// One machine cycle (four clocks). Returns (the interrupt is
    /// requested, bit 12 of the counter fell).
    pub fn tick(&mut self) -> (bool, bool) {
        self.loaded = false;
        let mut irq = false;
        if self.overflow {
            self.overflow = false;
            self.tima = self.tma;
            self.loaded = true;
            irq = true;
        }
        let frame = self.set_counter(self.counter.wrapping_add(4));
        (irq, frame)
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr & 0xff {
            0x04 => (self.counter >> 8) as u8,
            0x05 => self.tima,
            0x06 => self.tma,
            0x07 => self.tac | 0xf8,
            _ => 0xff,
        }
    }

    /// A write; true if clearing the counter made its bit 12 fall.
    pub fn write(&mut self, addr: u16, v: u8) -> bool {
        match addr & 0xff {
            0x04 => return self.set_counter(0),
            0x05 => {
                if !self.loaded {
                    self.tima = v;
                    self.overflow = false;
                }
            }
            0x06 => {
                self.tma = v;
                if self.loaded {
                    self.tima = v;
                }
            }
            0x07 => {
                // The write lands before the cycle's last clock: the
                // signal goes from the old TAC to the new one with the
                // counter one behind, then the counter steps. [`tick`]
                // has already counted the old signal's fall on that step.
                let now = self.counter;
                let old = self.signal();
                self.counter = now.wrapping_sub(1);
                let a = self.signal();
                self.tac = v & 7;
                let b = self.signal();
                self.counter = now;
                let c = self.signal();
                let falls = u8::from(a && !b) + u8::from(b && !c);
                if falls > u8::from(a && !old) {
                    self.count();
                }
            }
            _ => {}
        }
        false
    }
}

const STATE_TAG: [u8; 4] = *b"GBTM";

impl Savestate for GbTimer {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.u16(self.counter);
            w.u8(self.tima);
            w.u8(self.tma);
            w.u8(self.tac);
            w.bool(self.overflow);
            w.bool(self.loaded);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        *self = Self {
            counter: c.u16()?,
            tima: c.u8()?,
            tma: c.u8()?,
            tac: c.u8()? & 7,
            overflow: c.bool()?,
            loaded: c.bool()?,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machine cycles until the interrupt.
    fn until_irq(t: &mut GbTimer) -> u32 {
        let mut n = 0;
        loop {
            n += 1;
            if t.tick().0 {
                return n;
            }
            assert!(n < 100_000);
        }
    }

    #[test]
    fn div_counts_and_any_write_clears_it() {
        let mut t = GbTimer::new();
        for _ in 0..64 {
            t.tick();
        }
        assert_eq!(t.read(0xff04), 1);
        t.write(0xff04, 0x55);
        assert_eq!((t.read(0xff04), t.counter), (0, 0));
        assert_eq!(t.read(0xff07), 0xf8);
    }

    #[test]
    fn tima_counts_at_the_four_rates_and_reloads_late() {
        for (tac, cycles) in [(4u8, 256u32), (5, 4), (6, 16), (7, 64)] {
            let mut t = GbTimer::new();
            // (A fresh counter is at 0, where every bit has just fallen:
            // switching on there counts once, so TIMA is set afterwards.)
            t.write(0xff06, 0xfe);
            t.write(0xff07, tac);
            t.write(0xff05, 0xfe);
            // Two counts to the overflow, one more cycle to the load.
            assert_eq!(until_irq(&mut t), 2 * cycles + 1, "TAC {tac}");
            assert_eq!(t.tima, 0xfe);
        }
        // Stopped: nothing counts.
        let mut t = GbTimer::new();
        t.write(0xff07, 1);
        for _ in 0..100 {
            assert!(!t.tick().0);
        }
        assert_eq!(t.tima, 0);
    }

    #[test]
    fn the_overflow_cycle_and_the_load_cycle() {
        // TAC 5 counts every four machine cycles.
        let four = |t: &mut GbTimer| (0..4).fold(false, |irq, _| irq | t.tick().0);
        let mut t = GbTimer::new();
        t.write(0xff06, 0x80);
        t.write(0xff07, 5);
        t.write(0xff05, 0xff);
        assert!(!four(&mut t));
        assert_eq!(t.tima, 0, "overflowed: 0 for a cycle");
        // A write now cancels the load and the interrupt.
        t.write(0xff05, 0x10);
        assert!(!four(&mut t));
        assert_eq!(t.tima, 0x11);
        // In the cycle of the load TIMA ignores writes, TMA goes through.
        t.write(0xff05, 0xff);
        assert!(!four(&mut t));
        assert!(t.tick().0);
        t.write(0xff05, 0x33);
        assert_eq!(t.tima, 0x80);
        t.write(0xff06, 0x44);
        assert_eq!(t.tima, 0x44);
    }

    #[test]
    fn writes_can_count_once_more() {
        // Bit 3 is set; clearing DIV makes the signal fall.
        let mut t = GbTimer::new();
        t.write(0xff07, 5);
        t.write(0xff05, 0);
        t.tick();
        t.tick();
        assert_eq!((t.counter, t.tima), (8, 0));
        t.write(0xff04, 0);
        assert_eq!(t.tima, 1);
        // So does switching the timer off while the bit is set,
        t.tick();
        t.tick();
        t.tick();
        t.write(0xff07, 1);
        assert_eq!(t.tima, 2);
        // but not in the cycle the bit rises: the write comes first.
        t.write(0xff07, 5);
        t.tick();
        t.write(0xff07, 1);
        assert_eq!((t.counter, t.tima), (16, 3), "the bit's own fall");
        t.write(0xff07, 5);
        assert_eq!(t.tima, 4, "switched on in the cycle of the fall");
        t.tick();
        t.tick();
        t.write(0xff07, 1);
        assert_eq!((t.counter, t.tima), (24, 4));
    }

    #[test]
    fn bit_12_clocks_the_frame_sequencer() {
        let mut t = GbTimer::new();
        let mut falls = 0;
        for _ in 0..(8192 / 4) * 3 {
            falls += u32::from(t.tick().1);
        }
        assert_eq!(falls, 3, "512 Hz");
        t.counter = 0x1000;
        assert!(t.write(0xff04, 0));
        let mut w = StateWriter::new();
        t.tima = 9;
        t.save(&mut w);
        let mut other = GbTimer::new();
        other.load(&mut StateReader::new(&w.into_bytes())).unwrap();
        assert_eq!(other, t);
    }
}
