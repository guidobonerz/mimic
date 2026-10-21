//! General Instrument AY-3-8912 PSG (General Instrument AY-3-8910/8912
//! data sheet): three square-wave tone channels, one noise generator, one
//! envelope generator, a mixer and logarithmic volume per channel, plus
//! I/O port A.
//!
//! | Reg   | Function                         | Reg   | Function                    |
//! | ----- | -------------------------------- | ----- | --------------------------- |
//! | 0–5   | tone periods A/B/C (fine, coarse 4 bits) | 8–10 | amplitudes (bit 4 envelope) |
//! | 6     | noise period (5 bits)            | 11–12 | envelope period          |
//! | 7     | mixer: bits 0–2 tone off, 3–5 noise off; bit 6 port A output | 13 | envelope shape (a write restarts it) |
//! |       |                                  | 14    | I/O port A                  |
//!
//! The bus is driven by BDIR/BC1 (BC2 tied high on the CPC): 11 latches
//! the register number, 10 writes, 01 reads, 00 is inactive. On the CPC
//! port A is the keyboard's column input and the chip is clocked at 1 MHz.
//!
//! Sound: the tone and noise counters run at clock / 8; a tone output
//! flips when its counter reaches the period (frequency clock / 16 /
//! period), the noise generator (17-bit LFSR, taps 0 and 3) shifts at half
//! that rate per period step. The envelope steps through 16 levels at
//! clock / 16 / period, its shape from register 13 (continue, attack,
//! alternate, hold). A channel sounds where its enabled sources are high
//! (a disabled source counts as high).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Writable bits per register.
const MASKS: [u8; 16] = [
    0xff, 0x0f, 0xff, 0x0f, 0xff, 0x0f, 0x1f, 0xff, 0x1f, 0x1f, 0x1f, 0xff, 0xff, 0x0f, 0xff, 0xff,
];

/// DAC levels of the 16 volumes (about 3 dB apart), full scale 10 000.
const VOLUME: [u16; 16] = [
    0, 137, 205, 291, 423, 618, 847, 1369, 1691, 2647, 3527, 4499, 5704, 6873, 8482, 10000,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ay {
    pub regs: [u8; 16],
    /// Latched register number.
    pub addr: u8,
    /// Clock / 8 prescaler.
    prescale: u8,
    tone_count: [u16; 3],
    tone_out: [bool; 3],
    noise_count: u8,
    /// Noise runs at half the tone rate: this toggles each count.
    noise_half: bool,
    lfsr: u32,
    env_count: u16,
    /// Envelope step 0–15 within the current ramp, and whether it holds.
    env_step: u8,
    env_holding: bool,
    /// Ramp direction (true = attack, rising).
    env_up: bool,
}

impl Default for Ay {
    fn default() -> Self {
        Self::new()
    }
}

impl Ay {
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            addr: 0,
            prescale: 0,
            tone_count: [0; 3],
            tone_out: [false; 3],
            noise_count: 0,
            noise_half: false,
            lfsr: 1,
            env_count: 0,
            env_step: 0,
            env_holding: false,
            env_up: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// A bus cycle with BDIR/BC1 = `function` (bits 1–0) and `data` on the
    /// bus (writes and latches).
    pub fn bus_write(&mut self, function: u8, data: u8) {
        match function & 3 {
            3 => self.addr = data & 0x0f,
            2 => {
                let a = usize::from(self.addr);
                self.regs[a] = data & MASKS[a];
                if a == 13 {
                    self.restart_envelope();
                }
            }
            _ => {}
        }
    }

    /// What a read cycle puts on the bus, with `port_a` the pins of I/O
    /// port A (an input unless register 7 bit 6 makes it an output).
    pub fn read(&self, port_a: u8) -> u8 {
        match self.addr {
            14 if self.regs[7] & 0x40 == 0 => port_a,
            14 => self.regs[14] & port_a,
            a => self.regs[usize::from(a)],
        }
    }

    fn restart_envelope(&mut self) {
        self.env_count = 0;
        self.env_step = 0;
        self.env_holding = false;
        self.env_up = self.regs[13] & 0x04 != 0;
    }

    fn tone_period(&self, ch: usize) -> u16 {
        (u16::from(self.regs[2 * ch + 1]) << 8 | u16::from(self.regs[2 * ch])).max(1)
    }

    /// The envelope's current level 0–15.
    fn envelope(&self) -> u8 {
        if self.env_up {
            self.env_step
        } else {
            15 - self.env_step
        }
    }

    fn step_envelope(&mut self) {
        if self.env_holding {
            return;
        }
        if self.env_step < 15 {
            self.env_step += 1;
            return;
        }
        // End of a ramp: shape bits C (3), At (2), Al (1), H (0).
        let shape = self.regs[13];
        if shape & 0x08 == 0 {
            // Shapes 0–7: one ramp, then 0.
            self.env_holding = true;
            self.env_up = false;
            self.env_step = 15;
            return;
        }
        if shape & 0x01 != 0 {
            // Hold: the last level, or its opposite with alternate.
            self.env_holding = true;
            if shape & 0x02 != 0 {
                self.env_up = !self.env_up;
            }
            return;
        }
        if shape & 0x02 != 0 {
            self.env_up = !self.env_up;
        }
        self.env_step = 0;
    }

    /// One clock (1 MHz on the CPC).
    pub fn tick(&mut self) {
        self.prescale += 1;
        if self.prescale < 8 {
            return;
        }
        self.prescale = 0;
        for ch in 0..3 {
            self.tone_count[ch] += 1;
            if self.tone_count[ch] >= self.tone_period(ch) {
                self.tone_count[ch] = 0;
                self.tone_out[ch] = !self.tone_out[ch];
            }
        }
        self.noise_half = !self.noise_half;
        if self.noise_half {
            self.noise_count += 1;
            if self.noise_count >= (self.regs[6] & 0x1f).max(1) {
                self.noise_count = 0;
                let bit = (self.lfsr ^ (self.lfsr >> 3)) & 1;
                self.lfsr = (self.lfsr >> 1) | (bit << 16);
            }
            self.env_count += 1;
            let period = (u16::from(self.regs[12]) << 8 | u16::from(self.regs[11])).max(1);
            if self.env_count >= period {
                self.env_count = 0;
                self.step_envelope();
            }
        }
    }

    /// The three channels' output levels (0–10 000 each).
    pub fn output(&self) -> [u16; 3] {
        let noise = self.lfsr & 1 != 0;
        let mixer = self.regs[7];
        let mut out = [0; 3];
        for (ch, o) in out.iter_mut().enumerate() {
            let tone_on = mixer & (1 << ch) == 0;
            let noise_on = mixer & (8 << ch) == 0;
            let high = (self.tone_out[ch] || !tone_on) && (noise || !noise_on);
            if !high {
                continue;
            }
            let amp = self.regs[8 + ch];
            let level = if amp & 0x10 != 0 {
                self.envelope()
            } else {
                amp & 0x0f
            };
            *o = VOLUME[usize::from(level)];
        }
        out
    }
}

const TAG: [u8; 4] = *b"AY38";
const VERSION: u16 = 1;

impl Savestate for Ay {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.addr);
            w.u8(self.prescale);
            for c in self.tone_count {
                w.u16(c);
            }
            for v in self.tone_out {
                w.bool(v);
            }
            w.u8(self.noise_count);
            w.bool(self.noise_half);
            w.u32(self.lfsr);
            w.u16(self.env_count);
            w.u8(self.env_step);
            w.bool(self.env_holding);
            w.bool(self.env_up);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = Self::new();
        c.raw_into(&mut n.regs)?;
        n.addr = c.u8()? & 0x0f;
        for (v, m) in n.regs.iter_mut().zip(MASKS) {
            *v &= m;
        }
        n.prescale = c.u8()? & 7;
        for v in n.tone_count.iter_mut() {
            *v = c.u16()?;
        }
        for v in n.tone_out.iter_mut() {
            *v = c.bool()?;
        }
        n.noise_count = c.u8()?;
        n.noise_half = c.bool()?;
        n.lfsr = c.u32()? & 0x1ffff;
        n.env_count = c.u16()?;
        n.env_step = c.u8()?.min(15);
        n.env_holding = c.bool()?;
        n.env_up = c.bool()?;
        if n.lfsr == 0 {
            return Err(StateError::Invalid("AY noise generator"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(ay: &mut Ay, reg: u8, v: u8) {
        ay.bus_write(3, reg);
        ay.bus_write(2, v);
    }

    #[test]
    fn latch_write_read_and_port() {
        let mut ay = Ay::new();
        ay.bus_write(3, 1);
        ay.bus_write(2, 0xff);
        assert_eq!(ay.read(0xff), 0x0f, "coarse tone is 4 bits");
        ay.bus_write(3, 14);
        assert_eq!(ay.read(0xbf), 0xbf, "port A input");
        ay.bus_write(0, 0x55);
        assert_eq!(ay.addr, 14, "inactive cycle changes nothing");
    }

    #[test]
    fn tone_frequency() {
        let mut ay = Ay::new();
        // Channel A at period 100: 1 MHz / 16 / 100 = 625 Hz.
        write(&mut ay, 0, 100);
        write(&mut ay, 7, 0x3e);
        write(&mut ay, 8, 15);
        let mut edges = 0;
        let mut prev = ay.output()[0];
        for _ in 0..1_000_000 {
            ay.tick();
            let o = ay.output()[0];
            if o != prev {
                edges += 1;
                prev = o;
            }
        }
        assert_eq!(edges, 2 * 625);
        assert_eq!(ay.output()[1], 0, "B off");
    }

    #[test]
    fn envelope_decay_holds_at_zero() {
        let mut ay = Ay::new();
        write(&mut ay, 7, 0x3f); // all sources off: level only
        write(&mut ay, 8, 0x10); // envelope
        write(&mut ay, 11, 1);
        write(&mut ay, 13, 0x00); // \___
        assert_eq!(ay.output()[0], VOLUME[15]);
        for _ in 0..16 * 16 * 2 {
            ay.tick();
        }
        assert_eq!(ay.output()[0], 0);
        // /|/| (shape 12) keeps going.
        write(&mut ay, 13, 0x0c);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..16 * 16 * 3 {
            ay.tick();
            seen.insert(ay.output()[0]);
        }
        assert_eq!(seen.len(), 16);
    }

    #[test]
    fn noise_is_not_constant() {
        let mut ay = Ay::new();
        write(&mut ay, 6, 1);
        write(&mut ay, 7, 0x37); // noise on A only
        write(&mut ay, 8, 15);
        let mut ones = 0;
        for _ in 0..100_000 {
            ay.tick();
            ones += u32::from(ay.output()[0] > 0);
        }
        assert!((30_000..70_000).contains(&ones), "{ones}");
    }

    #[test]
    fn savestate_round_trip() {
        let mut ay = Ay::new();
        write(&mut ay, 7, 0x38);
        write(&mut ay, 0, 50);
        for _ in 0..12_345 {
            ay.tick();
        }
        let mut w = StateWriter::new();
        ay.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Ay::new();
        b.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(ay, b);
    }
}
