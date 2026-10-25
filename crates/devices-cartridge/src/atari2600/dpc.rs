//! DPC, David Crane's Display Processor Chip (Activision, Pitfall II).
//!
//! Next to an 8 KB program ROM switched like F8 (`$1FF8`/`$1FF9`), the
//! chip holds 2 KB of display data, eight data fetchers that read it, an
//! 8-bit random number generator and a three-voice music generator.
//!
//! Registers (in every bank, A12 = 1):
//!
//! | Address        | Access | Function                                                    |
//! | -------------- | ------ | ----------------------------------------------------------- |
//! | `$1000–$1003`  | read   | random number (the generator steps first)                   |
//! | `$1004–$1007`  | read   | music amplitude (0–15, for AUDVx)                           |
//! | `$1008–$100F`  | read   | DF0–DF7 data                                                |
//! | `$1010–$1017`  | read   | DF0–DF7 data AND flag                                       |
//! | `$1018–$101F`  | read   | DF0–DF7 data AND flag, nybbles swapped                      |
//! | `$1020–$1027`  | read   | DF0–DF7 data AND flag, bit order reversed                   |
//! | `$1028–$102F`  | read   | DF0–DF7 data AND flag, rotated right one bit                |
//! | `$1030–$1037`  | read   | DF0–DF7 data AND flag, rotated left one bit                 |
//! | `$1038–$103F`  | read   | DF0–DF7 flag                                                |
//! | `$1040–$1047`  | write  | DF0–DF7 top (also clears the flag)                          |
//! | `$1048–$104F`  | write  | DF0–DF7 bottom                                              |
//! | `$1050–$1057`  | write  | DF0–DF7 counter low byte (DF5–7 in music mode: loads top)   |
//! | `$1058–$105F`  | write  | DF0–DF7 counter high bits (D0–D2); DF5–7: D4 = music mode   |
//! | `$1060–$106F`  | write  | unused                                                      |
//! | `$1070–$1077`  | write  | random number reset (to 0)                                  |
//!
//! A data fetcher is an 11-bit down counter into the display data (the
//! dump stores it backwards: counter `n` reads byte `2047 − n`) with a top
//! and a bottom compare on its low byte: the flag sets when the low byte
//! equals top and clears when it equals bottom, checked on each read
//! before the data is taken; every read of a DF register then counts the
//! fetcher down (not the music fetchers).
//!
//! Music: in music mode DF5–DF7 are clocked by an oscillator on the
//! board instead of reads, about 20 kHz ([`OSC_HZ`]). Each clock counts
//! the low byte down, reloading top after 0, so the flag is a square wave
//! of period top + 1 clocks, high while the low byte is above bottom. The
//! amplitude register mixes the three flags with weights 4, 5 and 6
//! (sum 15); Pitfall II writes it to AUDV0 every scan line.
//!
//! Random numbers: an 8-bit shift register shifting left, the new bit 0
//! the XNOR of bits 7, 5, 4 and 3; from 0 it runs through all 255 other
//! states (the 255 bytes appended to the common 10 495-byte dump are this
//! sequence from 0).
//!
//! Sources: Kevin Horton's DPC description (register map, fetchers,
//! flags, music mode and mixing, oscillator "about 20 kHz"); Kevin
//! Horton, "Cart Information" v6
//! (<https://www.qotile.net/minidig/docs/sizes.txt>); the LFSR taps were
//! checked against the generator dump in the 10 495-byte Pitfall II
//! image. The swapped/reversed/rotated reads are from the description
//! only (Pitfall II does not use them); the oscillator's exact frequency
//! is an RC value that varies between boards.

use emu_core::{StateError, StateReader, StateWriter};

/// The music oscillator's frequency (nominal, the board's RC oscillator).
pub const OSC_HZ: u32 = 20_000;

/// Amplitude for the flags of DF5 (bit 0), DF6 (bit 1) and DF7 (bit 2).
const MIX: [u8; 8] = [0, 4, 5, 9, 6, 10, 11, 15];

/// The display data's size.
pub const DISPLAY_LEN: usize = 0x800;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dpc {
    tops: [u8; 8],
    bottoms: [u8; 8],
    /// 11-bit counters.
    counters: [u16; 8],
    flags: [bool; 8],
    /// Music mode of DF5–DF7.
    music: [bool; 3],
    random: u8,
    /// Oscillator phase: CPU cycles × [`OSC_HZ`] modulo the CPU clock.
    osc_phase: u32,
}

impl Dpc {
    pub fn new() -> Self {
        Self::default()
    }

    /// The random number generator's next state.
    pub fn lfsr_next(r: u8) -> u8 {
        let fb = !((r >> 7) ^ (r >> 5) ^ (r >> 4) ^ (r >> 3)) & 1;
        (r << 1) | fb
    }

    fn in_music_mode(&self, i: usize) -> bool {
        i >= 5 && self.music[i - 5]
    }

    /// The flag of fetcher `i` as the compare leaves it before a read.
    fn compared_flag(&self, i: usize) -> bool {
        let low = (self.counters[i] & 0xff) as u8;
        if low == self.tops[i] {
            true
        } else if low == self.bottoms[i] {
            false
        } else {
            self.flags[i]
        }
    }

    /// The music amplitude (0–15).
    pub fn amplitude(&self) -> u8 {
        let bits = (5..8).fold(0, |acc, i| acc | (usize::from(self.flags[i]) << (i - 5)));
        MIX[bits]
    }

    /// The value a read of register `reg` (`$00–$3F`) returns, with the
    /// flag `flag` and data byte `data` of its fetcher.
    fn value(&self, reg: usize, flag: bool, data: u8) -> u8 {
        let f = if flag { 0xff } else { 0 };
        match reg >> 3 {
            0 if reg < 4 => self.random,
            0 => self.amplitude(),
            1 => data,
            2 => data & f,
            3 => (data & f).rotate_left(4),
            4 => (data & f).reverse_bits(),
            5 => (data & f) >> 1,
            6 => (data & f) << 1,
            _ => f,
        }
    }

    fn data(&self, i: usize, display: &[u8]) -> u8 {
        let n = usize::from(self.counters[i] & 0x7ff);
        display.get(DISPLAY_LEN - 1 - n).copied().unwrap_or(0)
    }

    /// A read of register `reg` (`$00–$3F`) without side effects.
    pub fn peek(&self, reg: usize, display: &[u8]) -> u8 {
        let i = reg & 7;
        if reg < 8 {
            return self.value(reg, false, 0);
        }
        self.value(reg, self.compared_flag(i), self.data(i, display))
    }

    /// A read of register `reg` (`$00–$3F`).
    pub fn read(&mut self, reg: usize, display: &[u8]) -> u8 {
        if reg < 4 {
            self.random = Self::lfsr_next(self.random);
            return self.random;
        }
        if reg < 8 {
            return self.amplitude();
        }
        let i = reg & 7;
        self.flags[i] = self.compared_flag(i);
        let v = self.value(reg, self.flags[i], self.data(i, display));
        if !self.in_music_mode(i) {
            self.counters[i] = self.counters[i].wrapping_sub(1) & 0x7ff;
        }
        v
    }

    /// A write to register `reg` (`$40–$7F`).
    pub fn write(&mut self, reg: usize, v: u8) {
        let i = reg & 7;
        match (reg >> 3) & 7 {
            0 => {
                self.tops[i] = v;
                self.flags[i] = false;
            }
            1 => self.bottoms[i] = v,
            2 => {
                let low = if self.in_music_mode(i) {
                    self.tops[i]
                } else {
                    v
                };
                self.counters[i] = (self.counters[i] & 0x700) | u16::from(low);
            }
            3 => {
                self.counters[i] = (u16::from(v & 7) << 8) | (self.counters[i] & 0xff);
                if i >= 5 {
                    self.music[i - 5] = v & 0x10 != 0;
                }
            }
            6 => self.random = 0,
            _ => {}
        }
    }

    /// One oscillator clock: the music fetchers count.
    fn clock_music(&mut self) {
        for i in 5..8 {
            if !self.music[i - 5] {
                continue;
            }
            let low = (self.counters[i] & 0xff) as u8;
            let low = if low == 0 { self.tops[i] } else { low - 1 };
            self.counters[i] = (self.counters[i] & 0x700) | u16::from(low);
            if low == self.tops[i] {
                self.flags[i] = true;
            } else if low == self.bottoms[i] {
                self.flags[i] = false;
            }
        }
    }

    /// One CPU cycle of a CPU clocked at `cpu_hz`: the oscillator advances
    /// by [`OSC_HZ`] / `cpu_hz` of a clock (fixed-point, emulated time).
    pub fn tick(&mut self, cpu_hz: u32) {
        self.osc_phase += OSC_HZ;
        if self.osc_phase >= cpu_hz {
            self.osc_phase -= cpu_hz;
            self.clock_music();
        }
    }

    /// Debugger registers: random number, amplitude, then per fetcher
    /// top, bottom, counter low/high and flag.
    pub fn registers(&self) -> Vec<(&'static str, u32, u8)> {
        const NAMES: [[&str; 5]; 8] = [
            ["DF0TOP", "DF0BOT", "DF0LOW", "DF0HI", "DF0FLAG"],
            ["DF1TOP", "DF1BOT", "DF1LOW", "DF1HI", "DF1FLAG"],
            ["DF2TOP", "DF2BOT", "DF2LOW", "DF2HI", "DF2FLAG"],
            ["DF3TOP", "DF3BOT", "DF3LOW", "DF3HI", "DF3FLAG"],
            ["DF4TOP", "DF4BOT", "DF4LOW", "DF4HI", "DF4FLAG"],
            ["DF5TOP", "DF5BOT", "DF5LOW", "DF5HI", "DF5FLAG"],
            ["DF6TOP", "DF6BOT", "DF6LOW", "DF6HI", "DF6FLAG"],
            ["DF7TOP", "DF7BOT", "DF7LOW", "DF7HI", "DF7FLAG"],
        ];
        let mut regs = vec![
            ("RANDOM", 0x1000, self.random),
            ("MUSIC", 0x1004, self.amplitude()),
        ];
        for (i, n) in NAMES.iter().enumerate() {
            let a = 0x1040 + i as u32;
            let hi = (self.counters[i] >> 8) as u8 | if self.in_music_mode(i) { 0x10 } else { 0 };
            regs.extend([
                (n[0], a, self.tops[i]),
                (n[1], a + 8, self.bottoms[i]),
                (n[2], a + 0x10, self.counters[i] as u8),
                (n[3], a + 0x18, hi),
                (
                    n[4],
                    0x1038 + i as u32,
                    if self.flags[i] { 0xff } else { 0 },
                ),
            ]);
        }
        regs
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"DPC ", 1, |w| {
            w.raw(&self.tops);
            w.raw(&self.bottoms);
            for c in self.counters {
                w.u16(c);
            }
            for f in self.flags {
                w.bool(f);
            }
            for m in self.music {
                w.bool(m);
            }
            w.u8(self.random);
            w.u32(self.osc_phase);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"DPC ", 1)?;
        let mut d = Dpc::new();
        c.raw_into(&mut d.tops)?;
        c.raw_into(&mut d.bottoms)?;
        for x in &mut d.counters {
            *x = c.u16()?;
            if *x > 0x7ff {
                return Err(StateError::Invalid("DPC counter"));
            }
        }
        for f in &mut d.flags {
            *f = c.bool()?;
        }
        for m in &mut d.music {
            *m = c.bool()?;
        }
        d.random = c.u8()?;
        d.osc_phase = c.u32()?;
        // Any CPU clock is far below 100 MHz; a larger phase is corrupt.
        if d.osc_phase >= 100_000_000 {
            return Err(StateError::Invalid("DPC oscillator"));
        }
        *self = d;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display data whose byte at counter `n` is `n & 0xff`.
    fn display() -> Vec<u8> {
        (0..DISPLAY_LEN)
            .map(|i| ((DISPLAY_LEN - 1 - i) & 0xff) as u8)
            .collect()
    }

    #[test]
    fn fetcher_counts_down_and_wraps() {
        let d = display();
        let mut p = Dpc::new();
        p.write(0x50 + 2, 0x01); // DF2 low
        p.write(0x58 + 2, 0x03); // DF2 high
        assert_eq!(p.peek(0x0a, &d), 0x01, "peek does not count");
        assert_eq!(p.read(0x0a, &d), 0x01);
        assert_eq!(p.read(0x0a, &d), 0x00);
        assert_eq!(p.counters[2], 0x2ff);
        p.write(0x58 + 2, 0);
        p.write(0x50 + 2, 0);
        p.read(0x0a, &d);
        assert_eq!(p.counters[2], 0x7ff, "11-bit wrap");
        assert_eq!(p.counters[3], 0, "other fetchers untouched");
    }

    #[test]
    fn flag_window_between_top_and_bottom() {
        let d = display();
        let mut p = Dpc::new();
        p.write(0x40, 0x05); // top
        p.write(0x48, 0x02); // bottom
        p.write(0x50, 0x07);
        let masked: Vec<u8> = (0..8).map(|_| p.read(0x10, &d)).collect();
        // Counter 7, 6: flag clear; 5 sets it; 4, 3 inside; 2 clears it.
        assert_eq!(masked, [0, 0, 5, 4, 3, 0, 0, 0]);
        p.write(0x50, 0x05);
        assert_eq!(p.read(0x38, &d), 0xff, "flag register");
        assert_eq!(p.read(0x18, &d), 0x40, "nybbles swapped (4)");
        assert_eq!(p.read(0x20, &d), 0xc0, "reversed (3)");
        assert_eq!(p.read(0x28, &d), 0x00, "ROR, flag cleared at bottom (2)");
        p.write(0x40, 0x01);
        assert!(!p.flags[0], "writing top clears the flag");
        assert_eq!(p.read(0x30, &d), 0x02, "ROL (1), flag set at top");
    }

    #[test]
    fn random_numbers_run_through_255_states() {
        let mut p = Dpc::new();
        let first: Vec<u8> = (0..12).map(|_| p.read(0, &[])).collect();
        assert_eq!(
            first,
            [0x01, 0x03, 0x07, 0x0f, 0x1e, 0x3d, 0x7a, 0xf4, 0xe8, 0xd0, 0xa1, 0x43]
        );
        let mut r = 0u8;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..255 {
            seen.insert(r);
            r = Dpc::lfsr_next(r);
        }
        assert_eq!(r, 0, "period 255");
        assert_eq!(seen.len(), 255);
        p.write(0x70, 0);
        assert_eq!(p.peek(0, &[]), 0, "reset");
    }

    #[test]
    fn music_square_waves_and_amplitude() {
        let cpu_hz = 1_193_182;
        let mut p = Dpc::new();
        for (i, top) in [(5usize, 9u8), (6, 19), (7, 39)] {
            p.write(0x40 + i, top);
            p.write(0x48 + i, top / 2);
            p.write(0x58 + i, 0x10);
            p.write(0x50 + i, 0);
        }
        assert_eq!(p.counters[5] & 0xff, 9, "low byte loads top in music mode");
        let d = display();
        p.read(0x08 + 5, &d);
        assert_eq!(p.counters[5] & 0xff, 9, "reads do not clock music fetchers");
        // One second of CPU cycles: 20 000 oscillator clocks.
        let mut rises = [0u32; 3];
        let mut last = [false; 3];
        let mut levels = std::collections::BTreeSet::new();
        for _ in 0..cpu_hz {
            p.tick(cpu_hz);
            for (k, i) in (5..8).enumerate() {
                rises[k] += u32::from(p.flags[i] && !last[k]);
                last[k] = p.flags[i];
            }
            levels.insert(p.amplitude());
        }
        // Periods 10, 20 and 40 clocks: 2000, 1000 and 500 Hz.
        for (r, want) in rises.iter().zip([2000, 1000, 500]) {
            assert!(r.abs_diff(want) <= 1, "{rises:?}");
        }
        let mut all = MIX.to_vec();
        all.sort_unstable();
        assert_eq!(
            levels.into_iter().collect::<Vec<_>>(),
            all,
            "all eight mixes"
        );
        p.write(0x58 + 5, 0);
        let c = p.counters[5];
        for _ in 0..1000 {
            p.tick(cpu_hz);
        }
        assert_eq!(p.counters[5], c, "music mode off: no clocking");
    }

    #[test]
    fn savestate_round_trip() {
        let mut p = Dpc::new();
        p.write(0x47, 30);
        p.write(0x5f, 0x15);
        for _ in 0..777 {
            p.tick(1_193_182);
        }
        p.read(1, &[]);
        let mut w = StateWriter::new();
        p.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = Dpc::new();
        back.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, p);
        assert!(back.load(&mut StateReader::new(&bytes[..10])).is_err());
    }
}
