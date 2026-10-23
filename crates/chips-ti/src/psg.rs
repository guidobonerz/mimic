//! SN76489AN digital complex sound generator (TI datasheet; SMS Power,
//! "SN76489", for the noise shift register of TI's chip and the period-0
//! behaviour).
//!
//! **Writes.** A byte with bit 7 set latches a register (bits 6–5 the
//! channel, bit 4: 1 attenuation, 0 tone/noise) and writes its low four
//! bits; a byte with bit 7 clear writes bits 5–0 as the upper six bits of
//! a latched tone period, or the low four bits of another latched register.
//! Writing the noise register restarts the shift register.
//!
//! **Tones.** Three 10-bit periods: a channel's output flips every
//! `period` steps of the input clock ÷ 16 (period 0 counts as 1024).
//! **Noise:** bits 1–0 the rate (input ÷ 512, ÷ 1024, ÷ 2048, or the
//! third tone channel's flip rate), bit 2 white (1) or periodic (0) noise.
//! The 15-bit shift register shifts right on every rising edge of the
//! noise clock; the new bit 14 is bit 0 XOR bit 1 (white) or bit 0
//! (periodic); the output is bit 0.
//! **Attenuation:** 4 bits per channel, 2 dB a step, 15 off.
//!
//! [`Sn76489::tick`] is one input clock; the output level is the sum of
//! the four channels (0–4 × 8191).

use emu_core::{StateError, StateReader, StateWriter};

/// Levels for the attenuation values 0–15 (2 dB steps, 15 = off).
const VOLUME: [i32; 16] = [
    8191, 6507, 5168, 4105, 3261, 2590, 2057, 1634, 1298, 1031, 819, 650, 517, 410, 326, 0,
];

const NOISE_START: u16 = 0x4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sn76489 {
    /// Tone periods (10 bits) of channels 0–2.
    pub period: [u16; 3],
    /// Attenuation of channels 0–2 and noise.
    pub atten: [u8; 4],
    /// Noise control (3 bits).
    pub noise: u8,
    /// Latched register: channel × 2 + (1 for attenuation).
    latch: u8,
    counter: [u16; 4],
    output: [bool; 4],
    shift: u16,
    /// Input clocks within the ÷16 prescaler.
    prescale: u8,
}

impl Default for Sn76489 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sn76489 {
    pub fn new() -> Self {
        Self {
            period: [0; 3],
            atten: [15; 4],
            noise: 0,
            latch: 0,
            counter: [0; 4],
            output: [false; 4],
            shift: NOISE_START,
            prescale: 0,
        }
    }

    pub fn write(&mut self, v: u8) {
        if v & 0x80 != 0 {
            self.latch = (v >> 4) & 7;
            self.set_low(v & 15);
            return;
        }
        let ch = usize::from(self.latch >> 1);
        if self.latch & 1 == 0 && ch < 3 {
            self.period[ch] = (self.period[ch] & 0x0f) | u16::from(v & 0x3f) << 4;
        } else {
            self.set_low(v & 15);
        }
    }

    fn set_low(&mut self, v: u8) {
        let ch = usize::from(self.latch >> 1);
        if self.latch & 1 != 0 {
            self.atten[ch] = v;
        } else if ch < 3 {
            self.period[ch] = (self.period[ch] & 0x3f0) | u16::from(v);
        } else {
            self.noise = v & 7;
            self.shift = NOISE_START;
        }
    }

    fn noise_period(&self) -> Option<u16> {
        match self.noise & 3 {
            0 => Some(0x10),
            1 => Some(0x20),
            2 => Some(0x40),
            _ => None,
        }
    }

    /// One input clock; returns the output level.
    pub fn tick(&mut self) -> i32 {
        self.prescale += 1;
        if self.prescale == 16 {
            self.prescale = 0;
            self.step();
        }
        self.level()
    }

    fn step(&mut self) {
        let mut tone2_flipped = false;
        for ch in 0..3 {
            if self.counter[ch] == 0 {
                self.counter[ch] = match self.period[ch] {
                    0 => 0x400,
                    p => p,
                };
                self.output[ch] = !self.output[ch];
                tone2_flipped |= ch == 2;
            }
            self.counter[ch] -= 1;
        }
        let noise_edge = match self.noise_period() {
            Some(p) => {
                if self.counter[3] == 0 {
                    self.counter[3] = p;
                    true
                } else {
                    false
                }
            }
            None => tone2_flipped,
        };
        if self.noise_period().is_some() {
            self.counter[3] -= 1;
        }
        if noise_edge {
            self.output[3] = !self.output[3];
            if self.output[3] {
                let fb = if self.noise & 4 != 0 {
                    (self.shift ^ (self.shift >> 1)) & 1
                } else {
                    self.shift & 1
                };
                self.shift = (self.shift >> 1) | fb << 14;
            }
        }
    }

    /// The current output level (0 – 4 × 8191).
    pub fn level(&self) -> i32 {
        let mut sum = 0;
        for ch in 0..3 {
            if self.output[ch] {
                sum += VOLUME[usize::from(self.atten[ch])];
            }
        }
        if self.shift & 1 != 0 {
            sum += VOLUME[usize::from(self.atten[3])];
        }
        sum
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"S764", 1, |w| {
            for p in self.period {
                w.u16(p);
            }
            w.raw(&self.atten);
            w.u8(self.noise);
            w.u8(self.latch);
            for c in self.counter {
                w.u16(c);
            }
            for o in self.output {
                w.bool(o);
            }
            w.u16(self.shift);
            w.u8(self.prescale);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"S764", 1)?;
        let mut n = Self::new();
        for p in &mut n.period {
            *p = c.u16()? & 0x3ff;
        }
        c.raw_into(&mut n.atten)?;
        for a in &mut n.atten {
            *a &= 15;
        }
        n.noise = c.u8()? & 7;
        n.latch = c.u8()? & 7;
        for v in &mut n.counter {
            *v = c.u16()?.min(0x400);
        }
        for o in &mut n.output {
            *o = c.bool()?;
        }
        n.shift = c.u16()? & 0x7fff;
        n.prescale = c.u8()? % 16;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Input clocks until channel `ch`'s output changes.
    fn half_period(p: &mut Sn76489, ch: usize) -> u32 {
        let start = p.output[ch];
        for n in 1..100_000 {
            p.tick();
            if p.output[ch] != start {
                return n;
            }
        }
        panic!("no flip");
    }

    #[test]
    fn tone_periods_latch_and_data_bytes() {
        let mut p = Sn76489::new();
        // Channel 1 period $123: latch with the low nibble, then the rest.
        p.write(0xa3);
        p.write(0x12);
        assert_eq!(p.period[1], 0x123);
        // A second data byte keeps writing the upper bits.
        p.write(0x01);
        assert_eq!(p.period[1], 0x013);
        // Attenuation of channel 0 to 5, then to 7 with a data byte.
        p.write(0x95);
        assert_eq!(p.atten[0], 5);
        p.write(0x07);
        assert_eq!(p.atten[0], 7);
        // The flip rate: period × 16 input clocks.
        p.write(0x8a);
        p.write(0x00);
        half_period(&mut p, 0);
        assert_eq!(half_period(&mut p, 0), 10 * 16);
        // Period 0 counts as 1024.
        p.write(0x80);
        p.write(0x00);
        half_period(&mut p, 0);
        half_period(&mut p, 0);
        assert_eq!(half_period(&mut p, 0), 1024 * 16);
    }

    #[test]
    fn noise_shift_register_and_rates() {
        let mut p = Sn76489::new();
        // Periodic noise at ÷512: bit 14 cycles through in 15 shifts.
        p.write(0xe0);
        p.write(0xf0);
        let mut ones = 0;
        for _ in 0..15 * 2 * 0x10 * 16 {
            p.tick();
        }
        for _ in 0..15 {
            for _ in 0..2 * 0x10 * 16 {
                p.tick();
            }
            ones += u32::from(p.shift & 1);
        }
        assert_eq!(ones, 1, "periodic noise: one 1 in 15");
        // White noise (taps 0 and 1) repeats after 2^15 − 1 shifts.
        p.write(0xe4);
        let start = p.shift;
        let mut n = 0u32;
        loop {
            for _ in 0..2 * 0x10 * 16 {
                p.tick();
            }
            n += 1;
            if p.shift == start {
                break;
            }
            assert!(n < 40_000);
        }
        assert_eq!(n, 32_767);
        // Rate 3 follows channel 2.
        p.write(0xc4);
        p.write(0x00);
        p.write(0xe7);
        // The counter still runs out its old period (0 = 1024) first.
        let s = p.shift;
        for _ in 0..(0x400 + 2 * 4) * 16 * 2 {
            p.tick();
        }
        assert_ne!(p.shift, s, "noise clocked by tone 2");
    }

    #[test]
    fn levels_and_savestate() {
        let mut p = Sn76489::new();
        assert_eq!(p.level(), 0, "all off");
        p.write(0x90);
        p.write(0x81);
        p.write(0x00);
        let mut max = 0;
        for _ in 0..200 {
            max = max.max(p.tick());
        }
        assert_eq!(max, 8191);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let data = w.into_bytes();
        let mut q = Sn76489::new();
        q.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(q, p);
        assert!(q.load(&mut StateReader::new(&data[..4])).is_err());
    }
}
