//! The Game Boy's sound unit: two pulse channels (the first with a
//! frequency sweep), a channel playing 32 four-bit samples from wave RAM,
//! and a noise channel, mixed to two outputs.
//!
//! **Registers** (`$FF10`–`$FF26`, wave RAM `$FF30`–`$FF3F`):
//!
//! | Address | Name | Bits |
//! | --- | --- | --- |
//! | `$FF10` | NR10 | 6–4 sweep period, 3 down, 2–0 shift |
//! | `$FF11`, `$FF16` | NR11, NR21 | 7–6 duty (12.5, 25, 50, 75 %), 5–0 length (64 − n) |
//! | `$FF12`, `$FF17`, `$FF21` | NR12, NR22, NR42 | 7–4 volume, 3 envelope up, 2–0 its period; the upper five bits 0 switch the channel's converter off |
//! | `$FF13`/`$FF14`, `$FF18`/`$FF19`, `$FF1D`/`$FF1E` | NRx3, NRx4 | the 11-bit frequency; NRx4 bit 7 trigger, bit 6 length on |
//! | `$FF1A` | NR30 | 7 the wave channel's converter on |
//! | `$FF1B` | NR31 | length (256 − n) |
//! | `$FF1C` | NR32 | 6–5 volume: mute, 100, 50, 25 % |
//! | `$FF20` | NR41 | 5–0 length |
//! | `$FF22` | NR43 | 7–4 shift, 3 seven-bit noise, 2–0 divider |
//! | `$FF23` | NR44 | 7 trigger, 6 length on |
//! | `$FF24` | NR50 | 6–4 left volume, 2–0 right volume |
//! | `$FF25` | NR51 | 7–4 channels 4–1 to the left, 3–0 to the right |
//! | `$FF26` | NR52 | 7 the unit on; 3–0 read the channels' activity |
//!
//! A pulse channel steps through its eight-step duty every (2048 −
//! frequency) × 4 clocks, the wave channel through its samples every
//! (2048 − frequency) × 2, the noise's shift register every divider
//! (8, 16, 32 … 112) shifted left by the shift. The frame sequencer,
//! clocked at 512 Hz by the divider register ([`GbApu::frame_step`]),
//! clocks the lengths on its steps 0, 2, 4, 6, the sweep on 2 and 6 and
//! the envelopes on 7.
//!
//! Quirks kept, because blargg's tests and some games rely on them: the
//! extra length clock when a length is switched on in the first half of
//! the sequencer's period; the sweep's overflow check on trigger and
//! after each step; switching the sweep from down to up after a
//! calculation stops the channel; with the unit off the registers read as
//! cleared and take no writes but the lengths (DMG) and NR52.
//!
//! Each channel's converter turns its four bits into a level around zero
//! (off: zero); the mixer adds the channels of each side and scales by
//! the side's volume. The machine's capacitor is a first-order high pass
//! in [`GbApu::tick`]'s output.
//!
//! Sources: the Pan Docs (<https://gbdev.io/pandocs/>: "Audio", "Audio
//! Registers", "Audio details"); the gbdev wiki, "Gameboy sound hardware"
//! (the frame sequencer, length, sweep and trigger behaviour). The
//! implementation is this project's own.

use mimic_core::{Savestate, StateError, StateReader, StateWriter};

const DUTY: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

/// What the four channels share: the length counter, the converter and
/// the envelope (the wave channel has none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Common {
    on: bool,
    dac: bool,
    length: u16,
    length_on: bool,
    /// NRx2.
    envelope: u8,
    volume: u8,
    env_timer: u8,
    /// The 11-bit frequency (NR43 for the noise).
    freq: u16,
    /// Machine cycles (half cycles for the wave) to the next step.
    timer: u16,
}

impl Common {
    fn clock_length(&mut self) {
        if self.length_on && self.length > 0 {
            self.length -= 1;
            if self.length == 0 {
                self.on = false;
            }
        }
    }

    fn clock_envelope(&mut self) {
        let period = self.envelope & 7;
        if period == 0 {
            return;
        }
        self.env_timer = self.env_timer.saturating_sub(1);
        if self.env_timer == 0 {
            self.env_timer = period;
            if self.envelope & 8 != 0 {
                self.volume = (self.volume + 1).min(15);
            } else {
                self.volume = self.volume.saturating_sub(1);
            }
        }
    }

    /// NRx2: the converter is on with any of the upper five bits.
    fn set_envelope(&mut self, v: u8) {
        self.envelope = v;
        self.dac = v & 0xf8 != 0;
        if !self.dac {
            self.on = false;
        }
    }

    /// NRx4's length bit and trigger. `first_half`: the sequencer's next
    /// step does not clock the lengths.
    fn control(&mut self, v: u8, max: u16, first_half: bool) -> bool {
        let was_on = self.length_on;
        self.length_on = v & 0x40 != 0;
        if !was_on && self.length_on && first_half && self.length > 0 {
            self.length -= 1;
            if self.length == 0 && v & 0x80 == 0 {
                self.on = false;
            }
        }
        let trigger = v & 0x80 != 0;
        if trigger {
            self.on = self.dac;
            if self.length == 0 {
                self.length = max;
                if self.length_on && first_half {
                    self.length -= 1;
                }
            }
            self.volume = self.envelope >> 4;
            self.env_timer = match self.envelope & 7 {
                0 => 8,
                p => p,
            };
        }
        trigger
    }

    /// The converter's output around zero for the four-bit `level`.
    fn out(&self, level: u8) -> i32 {
        if !self.dac {
            0
        } else if self.on {
            2 * i32::from(level) - 15
        } else {
            -15
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GbApu {
    on: bool,
    ch: [Common; 4],
    /// The pulse channels' duty (NRx1's upper bits) and step.
    duty: [u8; 2],
    duty_step: [u8; 2],
    /// The sweep: NR10, its shadow frequency, timer, whether it runs and
    /// whether a calculation went down since the trigger.
    nr10: u8,
    shadow: u16,
    sweep_timer: u8,
    sweep_on: bool,
    sweep_down_used: bool,
    /// The wave channel: NR32's volume code, the sample position (0–31)
    /// and the sample being played.
    wave_volume: u8,
    wave_at: u8,
    wave_sample: u8,
    pub wave_ram: [u8; 16],
    /// The noise's shift register.
    lfsr: u16,
    nr50: u8,
    nr51: u8,
    /// The frame sequencer's next step.
    step: u8,
    /// The high pass's state for each side: last input and output (16.16).
    dc: [[i32; 2]; 2],
}

impl Default for GbApu {
    fn default() -> Self {
        Self::new()
    }
}

impl GbApu {
    pub fn new() -> Self {
        Self {
            on: false,
            ch: [Common::default(); 4],
            duty: [0; 2],
            duty_step: [0; 2],
            nr10: 0,
            shadow: 0,
            sweep_timer: 0,
            sweep_on: false,
            sweep_down_used: false,
            wave_volume: 0,
            wave_at: 0,
            wave_sample: 0,
            wave_ram: [0; 16],
            lfsr: 0x7fff,
            nr50: 0,
            nr51: 0,
            step: 0,
            dc: [[0; 2]; 2],
        }
    }

    /// The registers as the boot ROM leaves them.
    pub fn after_boot(&mut self) {
        self.write(0xff26, 0x80);
        for (addr, v) in [
            (0xff10u16, 0x80u8),
            (0xff11, 0xbf),
            (0xff12, 0xf3),
            (0xff14, 0x87),
            (0xff16, 0x3f),
            (0xff19, 0x3f),
            (0xff1a, 0x7f),
            (0xff1b, 0xff),
            (0xff1c, 0x9f),
            (0xff1e, 0x3f),
            (0xff20, 0xff),
            (0xff23, 0x3f),
            (0xff24, 0x77),
            (0xff25, 0xf3),
        ] {
            self.write(addr, v);
        }
    }

    /// The sequencer's next step does not clock the lengths.
    fn first_half(&self) -> bool {
        self.step & 1 == 1
    }

    /// The sweep's next frequency; above 2047 the channel stops.
    fn sweep_calc(&mut self) -> u16 {
        let delta = self.shadow >> (self.nr10 & 7);
        let new = if self.nr10 & 8 != 0 {
            self.sweep_down_used = true;
            self.shadow.wrapping_sub(delta)
        } else {
            self.shadow + delta
        };
        if new > 2047 {
            self.ch[0].on = false;
        }
        new
    }

    fn clock_sweep(&mut self) {
        self.sweep_timer = self.sweep_timer.saturating_sub(1);
        if self.sweep_timer != 0 {
            return;
        }
        let period = self.nr10 >> 4 & 7;
        self.sweep_timer = if period == 0 { 8 } else { period };
        if self.sweep_on && period != 0 {
            let new = self.sweep_calc();
            if new <= 2047 && self.nr10 & 7 != 0 {
                self.shadow = new;
                self.ch[0].freq = new;
                self.sweep_calc();
            }
        }
    }

    /// The frame sequencer's clock: 512 Hz, from the divider register.
    pub fn frame_step(&mut self) {
        if !self.on {
            return;
        }
        if self.step & 1 == 0 {
            for c in &mut self.ch {
                c.clock_length();
            }
        }
        if self.step == 2 || self.step == 6 {
            self.clock_sweep();
        }
        if self.step == 7 {
            for i in [0, 1, 3] {
                self.ch[i].clock_envelope();
            }
        }
        self.step = (self.step + 1) & 7;
    }

    /// The noise's period in machine cycles.
    fn noise_period(&self) -> u16 {
        let nr43 = self.ch[3].freq;
        let divider = match nr43 & 7 {
            0 => 2,
            n => 4 * n,
        };
        divider << (nr43 >> 4).min(13)
    }

    fn wave_step(&mut self) {
        let c = &mut self.ch[2];
        c.timer = c.timer.saturating_sub(1);
        if c.timer == 0 {
            c.timer = 2048 - c.freq;
            self.wave_at = (self.wave_at + 1) & 31;
            let byte = self.wave_ram[usize::from(self.wave_at / 2)];
            self.wave_sample = if self.wave_at & 1 == 0 {
                byte >> 4
            } else {
                byte & 15
            };
        }
    }

    /// One machine cycle; the two outputs (left, right).
    pub fn tick(&mut self) -> (i16, i16) {
        if !self.on {
            return (self.filter(0, 0), self.filter(1, 0));
        }
        for i in 0..2 {
            let c = &mut self.ch[i];
            c.timer = c.timer.saturating_sub(1);
            if c.timer == 0 {
                c.timer = 2048 - c.freq;
                self.duty_step[i] = (self.duty_step[i] + 1) & 7;
            }
        }
        if self.ch[2].on {
            self.wave_step();
            self.wave_step();
        }
        let c = &mut self.ch[3];
        c.timer = c.timer.saturating_sub(1);
        if c.timer == 0 {
            let period = self.noise_period();
            let c = &mut self.ch[3];
            c.timer = period;
            // Shifts above 13 give the register no clock.
            if c.freq >> 4 < 14 {
                let bit = (self.lfsr ^ self.lfsr >> 1) & 1;
                self.lfsr = self.lfsr >> 1 | bit << 14;
                if c.freq & 8 != 0 {
                    self.lfsr = self.lfsr & !0x40 | bit << 6;
                }
            }
        }
        let mut levels = [0i32; 4];
        for i in 0..2 {
            let high = DUTY[usize::from(self.duty[i])] >> self.duty_step[i] & 1;
            levels[i] = self.ch[i].out(high * self.ch[i].volume);
        }
        let wave = match self.wave_volume {
            0 => 0,
            n => self.wave_sample >> (n - 1),
        };
        levels[2] = self.ch[2].out(wave);
        levels[3] = self.ch[3].out(u8::from(self.lfsr & 1 == 0) * self.ch[3].volume);
        let mut sides = [0i32; 2];
        for (i, level) in levels.iter().enumerate() {
            if self.nr51 & 0x10 << i != 0 {
                sides[0] += level;
            }
            if self.nr51 & 1 << i != 0 {
                sides[1] += level;
            }
        }
        let left = sides[0] * (i32::from(self.nr50 >> 4 & 7) + 1) * 64;
        let right = sides[1] * (i32::from(self.nr50 & 7) + 1) * 64;
        (self.filter(0, left), self.filter(1, right))
    }

    /// The output capacitor: a first-order high pass (about 20 Hz at the
    /// machine cycle's rate).
    fn filter(&mut self, side: usize, v: i32) -> i16 {
        let x = v << 16;
        let [x1, y1] = self.dc[side];
        let y = x - x1 + ((i64::from(y1) * 65_528) >> 16) as i32;
        self.dc[side] = [x, y];
        (y >> 16).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }

    pub fn read(&self, addr: u16) -> u8 {
        let n = addr & 0xff;
        let c = &self.ch;
        let nrx4 = |i: usize| u8::from(c[i].length_on) << 6 | 0xbf;
        match n {
            0x10 => self.nr10 | 0x80,
            0x11 => self.duty[0] << 6 | 0x3f,
            0x12 => c[0].envelope,
            0x14 => nrx4(0),
            0x16 => self.duty[1] << 6 | 0x3f,
            0x17 => c[1].envelope,
            0x19 => nrx4(1),
            0x1a => u8::from(c[2].dac) << 7 | 0x7f,
            0x1c => self.wave_volume << 5 | 0x9f,
            0x1e => nrx4(2),
            0x21 => c[3].envelope,
            0x22 => c[3].freq as u8,
            0x23 => nrx4(3),
            0x24 => self.nr50,
            0x25 => self.nr51,
            0x26 => {
                let active = (0..4).fold(0, |v, i| v | u8::from(c[i].on) << i);
                u8::from(self.on) << 7 | 0x70 | active
            }
            // While the wave channel plays, its RAM shows the byte played.
            0x30..=0x3f if c[2].on => self.wave_ram[usize::from(self.wave_at / 2)],
            0x30..=0x3f => self.wave_ram[usize::from(n - 0x30)],
            _ => 0xff,
        }
    }

    fn power_off(&mut self) {
        let wave_ram = self.wave_ram;
        let lengths = self.ch.map(|c| c.length);
        let dc = self.dc;
        *self = Self::new();
        self.wave_ram = wave_ram;
        self.dc = dc;
        // The DMG's length counters survive.
        for (c, length) in self.ch.iter_mut().zip(lengths) {
            c.length = length;
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        let n = addr & 0xff;
        if let 0x30..=0x3f = n {
            let at = if self.ch[2].on {
                usize::from(self.wave_at / 2)
            } else {
                usize::from(n - 0x30)
            };
            self.wave_ram[at] = v;
            return;
        }
        if n == 0x26 {
            let on = v & 0x80 != 0;
            if self.on && !on {
                self.power_off();
            } else if !self.on && on {
                self.on = true;
                self.step = 0;
            }
            return;
        }
        if !self.on {
            // Only the lengths take writes (DMG).
            match n {
                0x11 => self.ch[0].length = 64 - u16::from(v & 0x3f),
                0x16 => self.ch[1].length = 64 - u16::from(v & 0x3f),
                0x1b => self.ch[2].length = 256 - u16::from(v),
                0x20 => self.ch[3].length = 64 - u16::from(v & 0x3f),
                _ => {}
            }
            return;
        }
        let first_half = self.first_half();
        match n {
            0x10 => {
                // From down to up after a calculation: the channel stops.
                if self.nr10 & 8 != 0 && v & 8 == 0 && self.sweep_down_used {
                    self.ch[0].on = false;
                }
                self.nr10 = v & 0x7f;
            }
            0x11 | 0x16 => {
                let i = usize::from(n == 0x16);
                self.duty[i] = v >> 6;
                self.ch[i].length = 64 - u16::from(v & 0x3f);
            }
            0x12 => self.ch[0].set_envelope(v),
            0x17 => self.ch[1].set_envelope(v),
            0x21 => self.ch[3].set_envelope(v),
            0x13 | 0x18 | 0x1d => {
                let c = &mut self.ch[usize::from((n - 0x13) / 5)];
                c.freq = c.freq & 0x700 | u16::from(v);
            }
            0x14 | 0x19 => {
                let i = usize::from(n == 0x19);
                let c = &mut self.ch[i];
                c.freq = c.freq & 0xff | u16::from(v & 7) << 8;
                if c.control(v, 64, first_half) {
                    c.timer = 2048 - c.freq;
                    if i == 0 {
                        self.shadow = self.ch[0].freq;
                        let period = self.nr10 >> 4 & 7;
                        self.sweep_timer = if period == 0 { 8 } else { period };
                        self.sweep_on = period != 0 || self.nr10 & 7 != 0;
                        self.sweep_down_used = false;
                        if self.nr10 & 7 != 0 {
                            self.sweep_calc();
                        }
                    }
                }
            }
            0x1a => {
                self.ch[2].dac = v & 0x80 != 0;
                if !self.ch[2].dac {
                    self.ch[2].on = false;
                }
            }
            0x1b => self.ch[2].length = 256 - u16::from(v),
            0x1c => self.wave_volume = v >> 5 & 3,
            0x1e => {
                let c = &mut self.ch[2];
                c.freq = c.freq & 0xff | u16::from(v & 7) << 8;
                if c.control(v, 256, first_half) {
                    // The first sample comes a little late.
                    c.timer = 2048 - c.freq + 3;
                    self.wave_at = 0;
                }
            }
            0x20 => self.ch[3].length = 64 - u16::from(v & 0x3f),
            0x22 => self.ch[3].freq = u16::from(v),
            0x23 => {
                if self.ch[3].control(v, 64, first_half) {
                    self.lfsr = 0x7fff;
                    self.ch[3].timer = self.noise_period();
                }
            }
            0x24 => self.nr50 = v,
            0x25 => self.nr51 = v,
            _ => {}
        }
    }
}

const STATE_TAG: [u8; 4] = *b"GBAP";

impl Savestate for GbApu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.bool(self.on);
            for c in &self.ch {
                w.bool(c.on);
                w.bool(c.dac);
                w.u16(c.length);
                w.bool(c.length_on);
                w.u8(c.envelope);
                w.u8(c.volume);
                w.u8(c.env_timer);
                w.u16(c.freq);
                w.u16(c.timer);
            }
            for v in [
                self.duty[0],
                self.duty[1],
                self.duty_step[0],
                self.duty_step[1],
                self.nr10,
                self.sweep_timer,
                self.wave_volume,
                self.wave_at,
                self.wave_sample,
                self.nr50,
                self.nr51,
                self.step,
            ] {
                w.u8(v);
            }
            w.u16(self.shadow);
            w.bool(self.sweep_on);
            w.bool(self.sweep_down_used);
            w.bytes(&self.wave_ram);
            w.u16(self.lfsr);
            for side in self.dc {
                w.u32(side[0] as u32);
                w.u32(side[1] as u32);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut n = self.clone();
        n.on = c.bool()?;
        for ch in &mut n.ch {
            ch.on = c.bool()?;
            ch.dac = c.bool()?;
            ch.length = c.u16()?.min(256);
            ch.length_on = c.bool()?;
            ch.envelope = c.u8()?;
            ch.volume = c.u8()? & 15;
            ch.env_timer = c.u8()?;
            ch.freq = c.u16()? & 0x7ff;
            ch.timer = c.u16()?;
        }
        n.duty = [c.u8()? & 3, c.u8()? & 3];
        n.duty_step = [c.u8()? & 7, c.u8()? & 7];
        n.nr10 = c.u8()? & 0x7f;
        n.sweep_timer = c.u8()?;
        n.wave_volume = c.u8()? & 3;
        n.wave_at = c.u8()? & 31;
        n.wave_sample = c.u8()? & 15;
        n.nr50 = c.u8()?;
        n.nr51 = c.u8()?;
        n.step = c.u8()? & 7;
        n.shadow = c.u16()?;
        n.sweep_on = c.bool()?;
        n.sweep_down_used = c.bool()?;
        let wave = c.bytes()?;
        if wave.len() != 16 {
            return Err(StateError::Invalid("wave RAM size"));
        }
        n.wave_ram.copy_from_slice(wave);
        n.lfsr = c.u16()? & 0x7fff;
        for side in &mut n.dc {
            *side = [c.u32()? as i32, c.u32()? as i32];
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> GbApu {
        let mut a = GbApu::new();
        a.write(0xff26, 0x80);
        a.write(0xff24, 0x77);
        a.write(0xff25, 0xff);
        a
    }

    /// Machine cycles between two rising edges of the left output.
    fn period(a: &mut GbApu) -> u32 {
        let mut last = a.tick().0;
        let mut edges = Vec::new();
        for n in 0..200_000u32 {
            let v = a.tick().0;
            if v > last + 500 {
                edges.push(n);
                if edges.len() == 3 {
                    return edges[2] - edges[1];
                }
            }
            last = v;
        }
        panic!("no tone: {edges:?}");
    }

    #[test]
    fn the_registers_read_with_their_unused_bits_set() {
        let mut a = on();
        for addr in 0xff10..=0xff25u16 {
            a.write(addr, 0);
        }
        let read: Vec<u8> = (0xff10..=0xff26u16).map(|n| a.read(n)).collect();
        assert_eq!(
            read,
            [
                0x80, 0x3f, 0x00, 0xff, 0xbf, 0xff, 0x3f, 0x00, 0xff, 0xbf, 0x7f, 0xff, 0x9f, 0xff,
                0xbf, 0xff, 0xff, 0x00, 0x00, 0xbf, 0x00, 0x00, 0xf0
            ]
        );
        assert_eq!(a.read(0xff27), 0xff);
        // Off: cleared, and writes do not arrive (but the wave RAM's).
        a.write(0xff25, 0x12);
        a.write(0xff26, 0);
        assert_eq!((a.read(0xff25), a.read(0xff26)), (0, 0x70));
        a.write(0xff25, 0x34);
        a.write(0xff30, 0xab);
        assert_eq!((a.read(0xff25), a.read(0xff30)), (0, 0xab));
    }

    #[test]
    fn a_pulse_channel_plays_its_frequency() {
        let mut a = on();
        a.write(0xff11, 0x80);
        a.write(0xff12, 0xf0);
        // Frequency 1792: (2048 - 1792) x 8 steps = 2048 machine cycles.
        a.write(0xff13, 0x00);
        a.write(0xff14, 0x87);
        assert_eq!(a.read(0xff26) & 0x0f, 1);
        assert_eq!(period(&mut a), 2048);
        // The second channel at 2047: 8 machine cycles.
        let mut a = on();
        a.write(0xff17, 0xf0);
        a.write(0xff18, 0xff);
        a.write(0xff19, 0x87);
        assert_eq!(period(&mut a), 8);
        // The converter off: the channel stops.
        a.write(0xff17, 0x07);
        assert_eq!(a.read(0xff26) & 0x0f, 0);
    }

    #[test]
    fn the_length_counter_stops_a_channel() {
        let mut a = on();
        a.write(0xff11, 0x3e);
        a.write(0xff12, 0xf0);
        a.write(0xff14, 0xc0);
        // Two length clocks (steps 0 and 2).
        for _ in 0..2 {
            assert_eq!(a.read(0xff26) & 1, 1);
            a.frame_step();
        }
        a.frame_step();
        assert_eq!(a.read(0xff26) & 1, 0);
        // A trigger with the length at 0 loads the maximum.
        a.write(0xff14, 0xc0);
        assert_eq!(a.ch[0].length, 63, "the next step is not a length step");
        // Switching the length on in the first half clocks it once.
        let mut a = on();
        a.frame_step();
        a.write(0xff16, 0x3e);
        a.write(0xff17, 0xf0);
        a.write(0xff19, 0x80);
        a.write(0xff19, 0x40);
        assert_eq!(a.ch[1].length, 1);
        a.write(0xff19, 0x00);
        a.write(0xff19, 0x40);
        assert_eq!((a.ch[1].length, a.read(0xff26) & 2), (0, 0));
    }

    #[test]
    fn the_envelope_and_the_sweep() {
        let mut a = on();
        a.write(0xff12, 0x31);
        a.write(0xff14, 0x80);
        assert_eq!(a.ch[0].volume, 3);
        // One envelope clock every eighth step.
        for _ in 0..8 * 3 {
            a.frame_step();
        }
        assert_eq!(a.ch[0].volume, 0);
        // Sweep up by 1/2 every period: 1024 -> 1536 -> out of range.
        let mut a = on();
        a.write(0xff10, 0x11);
        a.write(0xff12, 0xf0);
        a.write(0xff13, 0x00);
        a.write(0xff14, 0x84);
        for _ in 0..3 {
            a.frame_step();
        }
        assert_eq!((a.ch[0].freq, a.read(0xff26) & 1), (1536, 0));
        // The check at the trigger alone stops it.
        let mut a = on();
        a.write(0xff10, 0x11);
        a.write(0xff12, 0xf0);
        a.write(0xff13, 0xff);
        a.write(0xff14, 0x87);
        assert_eq!(a.read(0xff26) & 1, 0);
        // Down, then switched to up: stopped.
        let mut a = on();
        a.write(0xff10, 0x19);
        a.write(0xff12, 0xf0);
        a.write(0xff14, 0x84);
        assert_eq!(a.read(0xff26) & 1, 1);
        a.write(0xff10, 0x11);
        assert_eq!(a.read(0xff26) & 1, 0);
    }

    #[test]
    fn the_wave_and_the_noise_channels() {
        let mut a = on();
        for i in 0..16 {
            a.write(0xff30 + i, if i < 8 { 0xff } else { 0x00 });
        }
        a.write(0xff1a, 0x80);
        a.write(0xff1c, 0x20);
        // Frequency 2040: 32 samples x 8 x 2 clocks = 128 machine cycles.
        a.write(0xff1d, 0xf8);
        a.write(0xff1e, 0x87);
        assert_eq!(a.read(0xff26) & 0x0f, 4);
        assert_eq!(period(&mut a), 128);
        // Playing, its RAM shows the byte played.
        assert!(matches!(a.read(0xff30), 0x00 | 0xff));
        // Noise: the register's fifteen bits repeat after 32767 steps of
        // 2 machine cycles; seven bits after 127.
        for (nr43, steps) in [(0x00u8, 32_767u32), (0x08, 127)] {
            let mut a = on();
            a.write(0xff21, 0xf0);
            a.write(0xff22, nr43);
            a.write(0xff23, 0x80);
            let start = a.lfsr;
            let mut n = 0;
            loop {
                a.tick();
                a.tick();
                n += 1;
                if a.lfsr & 0x7f == start & 0x7f && (nr43 != 0 || a.lfsr == start) {
                    break;
                }
            }
            assert_eq!(n, steps);
        }
    }

    #[test]
    fn mixing_and_the_savestate() {
        // Channel 2 to the left only, at full and at the lowest volume.
        let mut a = on();
        a.write(0xff25, 0x20);
        a.write(0xff17, 0xf0);
        a.write(0xff18, 0x00);
        a.write(0xff19, 0x87);
        let peak = |a: &mut GbApu| {
            (0..4096).fold((0i16, 0i16), |m, _| {
                let (l, r) = a.tick();
                (m.0.max(l.abs()), m.1.max(r.abs()))
            })
        };
        let (left, right) = peak(&mut a);
        assert!(left > 5000 && right == 0, "{left} {right}");
        a.write(0xff24, 0x07);
        // The step of the change dies away in the high pass first.
        for _ in 0..200_000 {
            a.tick();
        }
        let (quiet, _) = peak(&mut a);
        assert!(quiet * 5 < left, "{quiet} {left}");
        let mut w = StateWriter::new();
        a.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = GbApu::new();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, a);
        assert_eq!(other.tick(), a.tick());
        assert!(GbApu::new()
            .load(&mut StateReader::new(&bytes[..bytes.len() - 5]))
            .is_err());
        // The state after the boot ROM: the unit on, channel 1 active.
        let mut a = GbApu::new();
        a.after_boot();
        assert_eq!(a.read(0xff26), 0xf1);
        assert_eq!((a.read(0xff24), a.read(0xff25)), (0x77, 0xf3));
    }
}
