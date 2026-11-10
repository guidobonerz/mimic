//! TMS9901 programmable systems interface: 32 CRU bits with interrupt
//! inputs, a 14-bit interval timer and 16 I/O pins (the TI-99/4A's keyboard,
//! joysticks, cassette and VDP interrupt).
//!
//! CRU bits relative to the chip's base:
//! - bit 0: control. Written 1: clock mode, 0: I/O mode. Read: the mode.
//! - bits 1–15, I/O mode: written, the interrupt mask for INT1–INT15
//!   (writing bit 3 also clears a pending timer interrupt); read, the
//!   interrupt inputs (pin levels: a line pulled low reads 0).
//! - bits 1–14, clock mode: written, the clock register (the decrementer's
//!   reload value; 0 stops the timer); read, the decrementer as it was
//!   when clock mode was entered. Bit 15 read: INTREQ; bit 15 written 0:
//!   software reset (all I/O pins become inputs).
//! - bits 16–31: P0–P15. Written: the pin becomes an output with that
//!   level. Read: the output level, or the input level of an input pin.
//!
//! The decrementer counts at φ/64 ([`Tms9901::tick`] gets φ cycles); at
//! zero it reloads and latches the timer interrupt on INT3. INTREQ is set
//! while an enabled interrupt is active ([`Tms9901::intreq`]).
//!
//! Source: Texas Instruments, *TMS 9901 Programmable Systems Interface*
//! data manual (1977): CRU bit assignments, clock mode, the decrementer
//! and interrupt logic. The implementation is this project's own.

use emu_core::{StateError, StateReader, StateWriter};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tms9901 {
    clock_mode: bool,
    /// Interrupt mask, bits 1–15.
    mask: u16,
    /// The clock register (14 bits).
    clock: u16,
    decrementer: u16,
    /// The decrementer as read in clock mode.
    read_latch: u16,
    timer_int: bool,
    /// φ cycles toward the next decrement.
    prescale: u32,
    /// P0–P15: the output latches and which pins are outputs.
    out: u16,
    is_output: u16,
    /// Interrupt inputs INT1–INT15 asserted (low), bit n = INTn.
    int_in: u16,
    /// Input levels of P0–P15.
    p_in: u16,
}

impl Tms9901 {
    pub fn new() -> Self {
        Self {
            p_in: 0xffff,
            ..Self::default()
        }
    }

    /// RESET: I/O mode, mask clear, all pins inputs, timer stopped.
    pub fn reset(&mut self) {
        let (int_in, p_in) = (self.int_in, self.p_in);
        *self = Self::new();
        self.int_in = int_in;
        self.p_in = p_in;
    }

    /// The interrupt inputs (bit n set: INTn is pulled low, active).
    pub fn set_interrupts(&mut self, active: u16) {
        self.int_in = active & 0xfffe;
    }

    /// The input levels of P0–P15 (bit set: high).
    pub fn set_inputs(&mut self, levels: u16) {
        self.p_in = levels;
    }

    /// The level of pin Pn as the outside sees it (an input floats high).
    pub fn pin(&self, n: u8) -> bool {
        let b = 1u16 << (n & 15);
        if self.is_output & b != 0 {
            self.out & b != 0
        } else {
            true
        }
    }

    /// The pins driven as outputs and their levels.
    pub fn outputs(&self) -> (u16, u16) {
        (self.is_output, self.out)
    }

    /// An enabled interrupt is active.
    pub fn intreq(&self) -> bool {
        let timer = if self.timer_int { 1 << 3 } else { 0 };
        (self.int_in | timer) & self.mask != 0
    }

    pub fn read_bit(&self, bit: u8) -> bool {
        match bit & 31 {
            0 => self.clock_mode,
            b @ 1..=14 if self.clock_mode => self.read_latch >> (b - 1) & 1 != 0,
            15 if self.clock_mode => self.intreq(),
            b @ 1..=15 => self.int_in & (1 << b) == 0,
            b => {
                let n = b - 16;
                if self.is_output & (1 << n) != 0 {
                    self.out & (1 << n) != 0
                } else {
                    self.p_in & (1 << n) != 0
                }
            }
        }
    }

    pub fn write_bit(&mut self, bit: u8, v: bool) {
        match bit & 31 {
            0 => {
                if v && !self.clock_mode {
                    self.read_latch = self.decrementer;
                }
                if !v && self.clock_mode {
                    // Leaving clock mode starts the timer with the register.
                    self.decrementer = self.clock;
                    self.prescale = 0;
                }
                self.clock_mode = v;
            }
            b @ 1..=14 if self.clock_mode => {
                if v {
                    self.clock |= 1 << (b - 1);
                } else {
                    self.clock &= !(1 << (b - 1));
                }
                self.decrementer = self.clock;
            }
            15 if self.clock_mode => {
                if !v {
                    self.is_output = 0;
                }
            }
            b @ 1..=15 => {
                if v {
                    self.mask |= 1 << b;
                } else {
                    self.mask &= !(1 << b);
                }
                if b == 3 {
                    self.timer_int = false;
                }
            }
            b => {
                let n = b - 16;
                self.is_output |= 1 << n;
                if v {
                    self.out |= 1 << n;
                } else {
                    self.out &= !(1 << n);
                }
            }
        }
    }

    /// `cycles` φ cycles of the system clock.
    pub fn tick(&mut self, cycles: u32) {
        if self.clock == 0 {
            return;
        }
        self.prescale += cycles;
        while self.prescale >= 64 {
            self.prescale -= 64;
            if self.decrementer <= 1 {
                self.decrementer = self.clock;
                self.timer_int = true;
            } else {
                self.decrementer -= 1;
            }
            if !self.clock_mode {
                self.read_latch = self.decrementer;
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bool(self.clock_mode);
        for v in [
            self.mask,
            self.clock,
            self.decrementer,
            self.read_latch,
            self.out,
            self.is_output,
            self.int_in,
            self.p_in,
        ] {
            w.u16(v);
        }
        w.bool(self.timer_int);
        w.u32(self.prescale);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Self::new();
        n.clock_mode = r.bool()?;
        n.mask = r.u16()?;
        n.clock = r.u16()? & 0x3fff;
        n.decrementer = r.u16()? & 0x3fff;
        n.read_latch = r.u16()? & 0x3fff;
        n.out = r.u16()?;
        n.is_output = r.u16()?;
        n.int_in = r.u16()?;
        n.p_in = r.u16()?;
        n.timer_int = r.bool()?;
        n.prescale = r.u32()? % 64;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupts_need_their_mask_bit() {
        let mut p = Tms9901::new();
        p.set_interrupts(1 << 2);
        assert!(!p.intreq());
        assert!(!p.read_bit(2), "the line reads low");
        p.write_bit(2, true);
        assert!(p.intreq());
        p.set_interrupts(0);
        assert!(!p.intreq());
        assert!(p.read_bit(2));
    }

    #[test]
    fn pins_become_outputs_when_written() {
        let mut p = Tms9901::new();
        p.set_inputs(0xfffb);
        assert!(!p.read_bit(18), "P2 is an input, low");
        p.write_bit(18, true);
        assert!(p.read_bit(18) && p.pin(2));
        p.write_bit(20, false);
        assert_eq!(p.outputs(), (0b10100, 0b00100));
        // Software reset in clock mode: inputs again.
        p.write_bit(0, true);
        p.write_bit(15, false);
        assert_eq!(p.outputs().0, 0);
    }

    #[test]
    fn the_timer_counts_at_a_64th_and_latches_int3() {
        let mut p = Tms9901::new();
        p.write_bit(0, true);
        // Clock register = 3.
        p.write_bit(1, true);
        p.write_bit(2, true);
        p.write_bit(0, false);
        p.write_bit(3, true);
        p.tick(64 * 2);
        assert!(!p.intreq());
        p.tick(64);
        assert!(p.intreq(), "reached zero after three decrements");
        // Writing mask bit 3 clears it.
        p.write_bit(3, true);
        assert!(!p.intreq());
        // Read the decrementer in clock mode: frozen.
        p.tick(64);
        p.write_bit(0, true);
        let v: u16 = (1..=14).map(|b| u16::from(p.read_bit(b)) << (b - 1)).sum();
        assert_eq!(v, 2);
        p.tick(64 * 5);
        let w: u16 = (1..=14).map(|b| u16::from(p.read_bit(b)) << (b - 1)).sum();
        assert_eq!(w, 2);
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Tms9901::new();
        p.write_bit(18, true);
        p.write_bit(2, true);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut q = Tms9901::new();
        q.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(p, q);
    }
}
