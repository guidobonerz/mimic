//! MOS 6581 / 8580 Sound Interface Device.
//!
//! | Reg     | Function                                                         |
//! | ------- | ---------------------------------------------------------------- |
//! | $00–$06 | voice 1: freq lo/hi, pulse width lo/hi (12 bit), control, AD, SR |
//! | $07–$0D | voice 2                                                          |
//! | $0E–$14 | voice 3                                                          |
//! | $15/$16 | filter cutoff (11 bit: $15 bits 0–2, $16)                        |
//! | $17     | 7–4 resonance, 3 ext in, 2–0 voices through the filter           |
//! | $18     | 7 voice 3 off, 6 HP, 5 BP, 4 LP, 3–0 volume                      |
//! | $19/$1A | POTX / POTY (read)                                               |
//! | $1B/$1C | OSC3 (voice 3 waveform, upper 8 bits) / ENV3 (read)              |
//!
//! POTX/POTY: the SID discharges the pot capacitors for 256 cycles, then
//! counts the cycles until they charge past the threshold; the count of the
//! last 512-cycle period is the register value, so it grows with the
//! resistance (255: nothing connected). The analog inputs are set with
//! [`Sid::set_pots`] as resistance on a 0–65535 scale (65535 = open).
//!
//! Control register: 7 noise, 6 pulse, 5 saw, 4 triangle, 3 test, 2 ring
//! modulation, 1 sync, 0 gate. Source: MOS 6581 data sheet; envelope rate
//! periods derived from the data sheet attack times (2 ms … 8 s for 255
//! steps at 1 MHz); the exponential decay steps follow published analyses
//! of the chip's envelope counter. Implementation is this project's own.
//!
//! Approximations (to refine with measurements): combined waveforms are the
//! AND of their parts; the filter is a digital state-variable filter with a
//! cutoff curve per model, not a model of the analog circuit.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidModel {
    /// Original SID (C64 until 1986): non-linear cutoff, DC offset that
    /// makes volume-register samples audible.
    #[default]
    Mos6581,
    /// C64C / C128DCR: linear cutoff, no DC offset.
    Mos8580,
}

/// Attack times in ms from the data sheet, for rate values 0–15.
const ATTACK_MS: [u32; 16] = [
    2, 8, 16, 24, 38, 56, 68, 80, 100, 250, 500, 800, 1000, 3000, 5000, 8000,
];

/// Rate counter period in cycles: attack time spread over 255 steps.
const fn rate_period(rate: u8) -> u16 {
    (ATTACK_MS[rate as usize & 15] * 1000 / 255) as u16
}

/// Extra divider of decay/release steps by envelope level (exponential
/// fall-off).
fn exp_period(level: u8) -> u8 {
    match level {
        0..=6 => 30,
        7..=14 => 16,
        15..=26 => 8,
        27..=54 => 4,
        55..=93 => 2,
        _ => 1,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EnvState {
    Attack,
    DecaySustain,
    #[default]
    Release,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Envelope {
    state: EnvState,
    level: u8,
    rate_counter: u16,
    exp_counter: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Voice {
    freq: u16,
    pw: u16,
    ctrl: u8,
    ad: u8,
    sr: u8,
    acc: u32,
    lfsr: u32,
    /// Accumulator MSB rose in this cycle (sync source).
    msb_rising: bool,
    env: Envelope,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            freq: 0,
            pw: 0,
            ctrl: 0,
            ad: 0,
            sr: 0,
            acc: 0,
            lfsr: 0x7f_fff8,
            msb_rising: false,
            env: Envelope::default(),
        }
    }
}

impl Voice {
    fn clock_osc(&mut self) {
        if self.ctrl & 0x08 != 0 {
            self.acc = 0;
            self.msb_rising = false;
            return;
        }
        let old = self.acc;
        self.acc = (self.acc + u32::from(self.freq)) & 0xff_ffff;
        self.msb_rising = old & 0x80_0000 == 0 && self.acc & 0x80_0000 != 0;
        if old & 0x08_0000 == 0 && self.acc & 0x08_0000 != 0 {
            let bit = ((self.lfsr >> 22) ^ (self.lfsr >> 17)) & 1;
            self.lfsr = ((self.lfsr << 1) | bit) & 0x7f_ffff;
        }
    }

    fn clock_env(&mut self) {
        let e = &mut self.env;
        let rate = match e.state {
            EnvState::Attack => self.ad >> 4,
            EnvState::DecaySustain => self.ad & 15,
            EnvState::Release => self.sr & 15,
        };
        e.rate_counter += 1;
        if e.rate_counter < rate_period(rate).max(1) {
            return;
        }
        e.rate_counter = 0;
        match e.state {
            EnvState::Attack => {
                e.level = e.level.saturating_add(1);
                if e.level == 255 {
                    e.state = EnvState::DecaySustain;
                }
            }
            EnvState::DecaySustain | EnvState::Release => {
                e.exp_counter += 1;
                if e.exp_counter < exp_period(e.level) {
                    return;
                }
                e.exp_counter = 0;
                let floor = if e.state == EnvState::DecaySustain {
                    (self.sr >> 4) * 17
                } else {
                    0
                };
                if e.level > floor {
                    e.level -= 1;
                }
            }
        }
    }

    fn write_ctrl(&mut self, v: u8) {
        let gate_on = v & 1 != 0 && self.ctrl & 1 == 0;
        let gate_off = v & 1 == 0 && self.ctrl & 1 != 0;
        if gate_on {
            self.env.state = EnvState::Attack;
        } else if gate_off {
            self.env.state = EnvState::Release;
        }
        if v & 0x08 != 0 {
            self.lfsr = 0x7f_fff8;
        }
        self.ctrl = v;
    }

    /// 12-bit waveform output; `ring_msb` is the ring-modulation source's MSB.
    fn wave(&self, ring_msb: bool) -> u16 {
        let sel = self.ctrl >> 4;
        if sel == 0 {
            return 0;
        }
        let mut out = 0xfff;
        if sel & 1 != 0 {
            let mut msb = self.acc & 0x80_0000 != 0;
            if self.ctrl & 0x04 != 0 {
                msb ^= ring_msb;
            }
            let t = if msb { self.acc ^ 0x7f_ffff } else { self.acc };
            out &= ((t >> 11) & 0xfff) as u16;
        }
        if sel & 2 != 0 {
            out &= (self.acc >> 12) as u16;
        }
        if sel & 4 != 0 {
            let high = self.ctrl & 0x08 != 0 || (self.acc >> 12) as u16 >= self.pw;
            out &= if high { 0xfff } else { 0 };
        }
        if sel & 8 != 0 {
            let r = self.lfsr;
            let noise = ((r >> 22) & 1) << 7
                | ((r >> 20) & 1) << 6
                | ((r >> 16) & 1) << 5
                | ((r >> 13) & 1) << 4
                | ((r >> 11) & 1) << 3
                | ((r >> 7) & 1) << 2
                | ((r >> 4) & 1) << 1
                | ((r >> 2) & 1);
            out &= (noise << 4) as u16;
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sid {
    model: SidModel,
    clock_hz: u32,
    voices: [Voice; 3],
    cutoff: u16,
    res_filt: u8,
    mode_vol: u8,
    /// Last value on the data bus; write-only registers read it back.
    bus: u8,
    pub pot: [u8; 2],
    /// Resistance on the POTX/POTY pins (0 = none, 65535 = open).
    pot_in: [u16; 2],
    /// Cycle in the 512-cycle pot measurement period.
    pot_cycle: u16,
    // Filter state.
    lp: f32,
    bp: f32,
}

impl Sid {
    /// A SID clocked at `clock_hz` (the machine's φ2).
    pub fn new(model: SidModel, clock_hz: u32) -> Self {
        Self {
            model,
            clock_hz,
            voices: [Voice::default(); 3],
            cutoff: 0,
            res_filt: 0,
            mode_vol: 0,
            bus: 0,
            pot: [0xff; 2],
            pot_in: [0xffff; 2],
            pot_cycle: 0,
            lp: 0.0,
            bp: 0.0,
        }
    }

    pub fn model(&self) -> SidModel {
        self.model
    }

    /// Filter cutoff in Hz for the current register value.
    fn cutoff_hz(&self) -> f32 {
        let x = f32::from(self.cutoff) / 2047.0;
        match self.model {
            // Steep, non-linear curve between ~220 Hz and ~18 kHz.
            SidModel::Mos6581 => 220.0 + 17_800.0 * x * x,
            // Linear, ~30 Hz to ~12 kHz as in the data sheet.
            SidModel::Mos8580 => 30.0 + 11_970.0 * x,
        }
    }

    /// The resistances on POTX/POTY (0 = none, 65535 = open).
    pub fn set_pots(&mut self, pots: [u16; 2]) {
        self.pot_in = pots;
    }

    /// One φ2 cycle; returns the mixed output sample (≈ 16-bit range).
    pub fn clock(&mut self) -> i32 {
        self.pot_cycle += 1;
        if self.pot_cycle == 512 {
            self.pot_cycle = 0;
            self.pot = self.pot_in.map(|r| (r >> 8) as u8);
        }
        for v in &mut self.voices {
            v.clock_osc();
        }
        for i in 0..3 {
            let src = (i + 2) % 3;
            if self.voices[i].ctrl & 0x02 != 0 && self.voices[src].msb_rising {
                self.voices[i].acc = 0;
            }
        }
        for v in &mut self.voices {
            v.clock_env();
        }
        self.mix()
    }

    fn mix(&mut self) -> i32 {
        let mut direct = 0i32;
        let mut filtered = 0i32;
        for i in 0..3 {
            let src = (i + 2) % 3;
            let ring_msb = self.voices[src].acc & 0x80_0000 != 0;
            let v = &self.voices[i];
            let out = (i32::from(v.wave(ring_msb)) - 2048) * i32::from(v.env.level);
            if self.res_filt & (1 << i) != 0 {
                filtered += out;
            } else if !(i == 2 && self.mode_vol & 0x80 != 0) {
                direct += out;
            }
        }
        let mode = self.mode_vol >> 4 & 7;
        let mut filter_out = 0.0f32;
        if mode != 0 || self.res_filt & 7 != 0 {
            // Chamberlin state-variable filter; small-angle coefficient
            // (cutoff ≪ clock) keeps it free of library calls.
            let f = 2.0 * std::f32::consts::PI * self.cutoff_hz() / self.clock_hz as f32;
            let q = 1.0 / (0.707 + f32::from(self.res_filt >> 4) / 15.0 * 1.5);
            let input = filtered as f32;
            let hp = input - self.lp - q * self.bp;
            self.bp += f * hp;
            self.lp += f * self.bp;
            if mode & 1 != 0 {
                filter_out += self.lp;
            }
            if mode & 2 != 0 {
                filter_out += self.bp;
            }
            if mode & 4 != 0 {
                filter_out += hp;
            }
        }
        let volume = i32::from(self.mode_vol & 15);
        let mut sum = (direct + filter_out as i32) / 15 * volume;
        if self.model == SidModel::Mos6581 {
            // The 6581's output stage carries a DC level proportional to the
            // volume: writing $D418 alone produces sound ("digis").
            sum += volume * 0x6000;
        }
        sum / 48
    }

    /// Side-effect-free read.
    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 0x1f {
            0x19 => self.pot[0],
            0x1a => self.pot[1],
            0x1b => {
                let ring = self.voices[1].acc & 0x80_0000 != 0;
                (self.voices[2].wave(ring) >> 4) as u8
            }
            0x1c => self.voices[2].env.level,
            _ => self.bus,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        self.bus = v;
        let reg = reg & 0x1f;
        if reg < 0x15 {
            let voice = &mut self.voices[usize::from(reg / 7)];
            match reg % 7 {
                0 => voice.freq = (voice.freq & 0xff00) | u16::from(v),
                1 => voice.freq = (voice.freq & 0x00ff) | (u16::from(v) << 8),
                2 => voice.pw = (voice.pw & 0x0f00) | u16::from(v),
                3 => voice.pw = (voice.pw & 0x00ff) | (u16::from(v & 15) << 8),
                4 => voice.write_ctrl(v),
                5 => voice.ad = v,
                _ => voice.sr = v,
            }
            return;
        }
        match reg {
            0x15 => self.cutoff = (self.cutoff & 0x7f8) | u16::from(v & 7),
            0x16 => self.cutoff = (self.cutoff & 7) | (u16::from(v) << 3),
            0x17 => self.res_filt = v,
            0x18 => self.mode_vol = v,
            _ => {}
        }
    }
}

const TAG: [u8; 4] = *b"SID0";
const VERSION: u16 = 2;

impl Savestate for Sid {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for v in &self.voices {
                w.u16(v.freq);
                w.u16(v.pw);
                w.u8(v.ctrl);
                w.u8(v.ad);
                w.u8(v.sr);
                w.u32(v.acc);
                w.u32(v.lfsr);
                w.bool(v.msb_rising);
                w.u8(match v.env.state {
                    EnvState::Attack => 0,
                    EnvState::DecaySustain => 1,
                    EnvState::Release => 2,
                });
                w.u8(v.env.level);
                w.u16(v.env.rate_counter);
                w.u8(v.env.exp_counter);
            }
            w.u16(self.cutoff);
            w.u8(self.res_filt);
            w.u8(self.mode_vol);
            w.u8(self.bus);
            w.raw(&self.pot);
            w.u32(self.lp.to_bits());
            w.u32(self.bp.to_bits());
            w.u16(self.pot_in[0]);
            w.u16(self.pot_in[1]);
            w.u16(self.pot_cycle);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut s = Sid::new(self.model, self.clock_hz);
        for v in s.voices.iter_mut() {
            v.freq = c.u16()?;
            v.pw = c.u16()? & 0xfff;
            v.ctrl = c.u8()?;
            v.ad = c.u8()?;
            v.sr = c.u8()?;
            v.acc = c.u32()? & 0xff_ffff;
            v.lfsr = c.u32()? & 0x7f_ffff;
            v.msb_rising = c.bool()?;
            v.env.state = match c.u8()? {
                0 => EnvState::Attack,
                1 => EnvState::DecaySustain,
                2 => EnvState::Release,
                _ => return Err(StateError::Invalid("SID envelope state")),
            };
            v.env.level = c.u8()?;
            v.env.rate_counter = c.u16()?;
            v.env.exp_counter = c.u8()?;
        }
        s.cutoff = c.u16()? & 0x7ff;
        s.res_filt = c.u8()?;
        s.mode_vol = c.u8()?;
        s.bus = c.u8()?;
        c.raw_into(&mut s.pot)?;
        s.lp = f32::from_bits(c.u32()?);
        s.bp = f32::from_bits(c.u32()?);
        if !s.lp.is_finite() || !s.bp.is_finite() {
            return Err(StateError::Invalid("SID filter state"));
        }
        // Version 1 predates the pot inputs.
        if version >= 2 {
            s.pot_in = [c.u16()?, c.u16()?];
            s.pot_cycle = c.u16()? % 512;
        }
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
