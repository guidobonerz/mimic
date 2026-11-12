//! Konami SCC (051649, "Sound Creative Chip"): the wavetable sound chip in
//! Konami's MSX MegaROM cartridges. Five channels, each playing a waveform
//! of 32 signed bytes at a 12-bit period with a 4-bit volume; channels 4
//! and 5 share one waveform.
//!
//! Registers (the low address byte; the cartridge shows them at
//! $9800–$9FFF, repeating every 256 bytes):
//!
//! | Address   | Function                                                   |
//! | --------- | ---------------------------------------------------------- |
//! | $00–$7F   | waveforms of channels 1, 2, 3 and 4/5, 32 bytes each (RAM) |
//! | $80–$89   | periods of channels 1–5: low byte, then bits 11–8          |
//! | $8A–$8E   | volumes of channels 1–5 (bits 3–0)                         |
//! | $8F       | channel switches (bit 0 channel 1 … bit 4 channel 5)       |
//! | $90–$9F   | the same as $80–$8F                                        |
//! | $A0–$DF   | nothing                                                    |
//! | $E0–$FF   | test register (one byte)                                   |
//!
//! $80–$FF are write-only and read $FF.
//!
//! A channel steps through its waveform once per `period + 1` clocks of
//! the 3.58 MHz bus clock: tone frequency = clock / (32 × (period + 1)).
//! A channel's output is its waveform byte times its volume, divided by 16
//! (8 bits); the five add up to the chip's 11-bit output.
//!
//! Not in the documents and this project's reading: a period below 9
//! stops the channel (as commonly reported), a period write restarts the
//! channel's counter but not its place in the waveform, a switched-off
//! channel keeps running silently. The test register is kept but has no
//! effect (its pitch and noise modes are not built; Konami's games do not
//! use it).
//!
//! Sources: BiFi, "Konami SCC Sound Chip" (register map, period formula,
//! read behaviour, test register); MSX Resource Center wiki, "MegaROM
//! Mappers" and "Konami 051649" (register map, the 11-bit output). See
//! `docs/sources.md`.

use emu_core::{StateError, StateReader, StateWriter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scc {
    /// Waveforms of channels 1, 2, 3 and 4/5.
    pub wave: [[i8; 32]; 4],
    pub period: [u16; 5],
    pub volume: [u8; 5],
    /// Channel switches (bits 4–0).
    pub enable: u8,
    /// The test register (no effect).
    pub test: u8,
    /// Clocks until each channel's next step.
    count: [u16; 5],
    /// Each channel's place in its waveform.
    pos: [u8; 5],
}

impl Default for Scc {
    fn default() -> Self {
        Self::new()
    }
}

impl Scc {
    pub fn new() -> Self {
        Self {
            wave: [[0; 32]; 4],
            period: [0; 5],
            volume: [0; 5],
            enable: 0,
            test: 0,
            count: [0; 5],
            pos: [0; 5],
        }
    }

    /// Side-effect-free read of register `reg`.
    pub fn read(&self, reg: u8) -> u8 {
        match reg {
            0x00..=0x7f => self.wave[usize::from(reg >> 5)][usize::from(reg & 31)] as u8,
            _ => 0xff,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg {
            0x00..=0x7f => self.wave[usize::from(reg >> 5)][usize::from(reg & 31)] = v as i8,
            0x80..=0x9f => match reg & 0x0f {
                r @ 0..=9 => {
                    let ch = usize::from(r >> 1);
                    let p = &mut self.period[ch];
                    *p = if r & 1 == 0 {
                        *p & 0xf00 | u16::from(v)
                    } else {
                        *p & 0x0ff | u16::from(v & 0x0f) << 8
                    };
                    self.count[ch] = *p;
                }
                r @ 0x0a..=0x0e => self.volume[usize::from(r - 0x0a)] = v & 0x0f,
                _ => self.enable = v & 0x1f,
            },
            0xa0..=0xdf => {}
            0xe0..=0xff => self.test = v,
        }
    }

    /// `n` clocks of the bus clock.
    pub fn clock(&mut self, n: u16) {
        for ch in 0..5 {
            let period = self.period[ch];
            if period < 9 {
                continue;
            }
            let mut left = n;
            while left > self.count[ch] {
                left -= self.count[ch] + 1;
                self.count[ch] = period;
                self.pos[ch] = (self.pos[ch] + 1) & 31;
            }
            self.count[ch] -= left;
        }
    }

    /// The chip's output (11 bits, signed: −640 … 635).
    pub fn output(&self) -> i16 {
        let mut sum = 0;
        for ch in 0..5 {
            if self.enable & (1 << ch) == 0 {
                continue;
            }
            let sample = self.wave[ch.min(3)][usize::from(self.pos[ch])];
            sum += (i16::from(sample) * i16::from(self.volume[ch])) >> 4;
        }
        sum
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"KSCC", 1, |w| {
            for wave in &self.wave {
                for &s in wave {
                    w.u8(s as u8);
                }
            }
            for ch in 0..5 {
                w.u16(self.period[ch]);
                w.u8(self.volume[ch]);
                w.u16(self.count[ch]);
                w.u8(self.pos[ch]);
            }
            w.u8(self.enable);
            w.u8(self.test);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"KSCC", 1)?;
        let mut n = Self::new();
        for wave in n.wave.iter_mut() {
            for s in wave.iter_mut() {
                *s = c.u8()? as i8;
            }
        }
        for ch in 0..5 {
            n.period[ch] = c.u16()? & 0xfff;
            n.volume[ch] = c.u8()? & 0x0f;
            n.count[ch] = c.u16()?.min(n.period[ch]);
            n.pos[ch] = c.u8()? & 31;
        }
        n.enable = c.u8()? & 0x1f;
        n.test = c.u8()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_read_and_mirror() {
        let mut s = Scc::new();
        s.write(0x00, 0x7f);
        s.write(0x7f, 0x80);
        assert_eq!((s.read(0x00), s.read(0x7f)), (0x7f, 0x80));
        assert_eq!(s.wave[3][31], -128);
        s.write(0x82, 0x34);
        s.write(0x93, 0xf2);
        assert_eq!(s.period[1], 0x234, "high nibble ignored, $90 mirrors $80");
        s.write(0x8e, 0xff);
        s.write(0x8f, 0xff);
        assert_eq!((s.volume[4], s.enable), (15, 0x1f));
        s.write(0xe5, 0x42);
        assert_eq!(s.test, 0x42);
        assert_eq!(
            (s.read(0x82), s.read(0xa0), s.read(0xe0)),
            (0xff, 0xff, 0xff)
        );
    }

    #[test]
    fn a_channel_steps_once_per_period_plus_one() {
        let mut s = Scc::new();
        // A sawtooth on channel 1: the output shows the place in it.
        for i in 0..32 {
            s.write(i, i * 4);
        }
        s.write(0x80, 99);
        s.write(0x8a, 15);
        s.write(0x8f, 0x01);
        assert_eq!(s.output(), 0);
        s.clock(99);
        assert_eq!(s.output(), 0, "not yet");
        s.clock(1);
        assert_eq!(s.output(), (4 * 15) >> 4);
        // 32 steps of 100 clocks: once around, in uneven pieces.
        for n in [1, 7, 250, 1000, 1942] {
            s.clock(n);
        }
        assert_eq!(s.pos[0], 1);
        // Channel 5 plays channel 4's waveform.
        s.write(0x60, 0x40);
        s.write(0x8e, 8);
        s.write(0x8f, 0x10);
        assert_eq!(s.output(), 0x20);
        // Negative samples.
        s.write(0x60, 0x80);
        assert_eq!(s.output(), -64);
    }

    #[test]
    fn short_periods_stop_a_channel_and_savestates_round_trip() {
        let mut s = Scc::new();
        s.write(0x80, 8);
        s.clock(10_000);
        assert_eq!(s.pos[0], 0);
        s.write(0x80, 9);
        s.clock(25);
        assert_eq!(s.pos[0], 2);
        s.write(0x21, 0x55);
        let mut w = StateWriter::new();
        s.save(&mut w);
        let data = w.into_bytes();
        let mut t = Scc::new();
        t.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(t, s);
        assert!(t.load(&mut StateReader::new(&data[..10])).is_err());
    }
}
