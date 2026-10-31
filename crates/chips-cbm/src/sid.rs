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
//! Oscillator, noise and envelope follow the behaviour the VICE test programs
//! (`SID/`, readmes and real-chip references) pin down: the shift register's
//! two-phase clock with the selector writing combined noise back, the 6581's
//! adder MSB pulled by the selector, the 8580's output register behind
//! triangle and sawtooth, the 15-bit rate counter, the floating output.
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

/// Rate counter period in cycles for rate values 0–15: the data sheet's
/// attack times over 255 steps, as measured on real chips (`SID/resid-test`
/// `envrate`, `envtime`).
const RATE_PERIOD: [u16; 16] = [
    9, 32, 63, 95, 149, 220, 267, 313, 392, 977, 1954, 3126, 3907, 11720, 19532, 31251,
];

/// Period of the rate counter, a 15-bit shift register: a match missed
/// after a rate change takes a full turn (the "ADSR delay bug").
const RATE_WRAP: u16 = 0x7fff;

/// Noise output d11…d4 taken from these shift register bits.
const NOISE_BITS: [u8; 8] = [20, 18, 14, 11, 9, 5, 2, 0];
const NOISE_MASK: u32 = 1 << 20 | 1 << 18 | 1 << 14 | 1 << 11 | 1 << 9 | 1 << 5 | 1 << 2 | 1;
const LFSR_ONES: u32 = 0x7f_ffff;

/// Cycles from a rate match to the level's step in attack, and from a
/// match to the divider's carry and from there to a decay or release step;
/// gate on, gate off, gate off with a step under way until the state
/// changes. Found by scanning them against `SID/env_test`.
const ATTACK_STEP: u8 = 3;
const EXP_CARRY: u8 = 2;
const DOWN_STEP: u8 = 2;
const GATE_ON: u8 = 1;
const GATE_OFF: u8 = 2;
const GATE_OFF_STEPPING: u8 = 4;

/// Cycles a value stays on the internal bus. Not measured: leakage takes
/// far less on the 6581 than on the 8580.
const fn bus_hold(model: SidModel) -> u32 {
    match model {
        SidModel::Mos6581 => 8_000,
        SidModel::Mos8580 => 700_000,
    }
}

/// Cycles the output of "no waveform" floats before its bits start to leak
/// (not measured; `SID/osc3-wave0`: gone after about 3 s).
const fn float_hold(model: SidModel) -> u32 {
    match model {
        SidModel::Mos6581 => 50_000,
        SidModel::Mos8580 => 1_000_000,
    }
}

/// Cycles with the test bit on until the shift register has faded to ones
/// (on the 6581 `SID/resid-test` `oscsample` needs more than $8000 cycles,
/// the 8580 takes much longer: `SID/wb_testsuite` readme).
const fn lfsr_fade(model: SidModel) -> u32 {
    match model {
        SidModel::Mos6581 => 35_000,
        SidModel::Mos8580 => 1_000_000,
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
    /// Divider of decay/release steps, set as the level passes its marks.
    exp_period: u8,
    /// The level reached 0: it stays there until the next attack.
    frozen: bool,
    /// The rate counter matched: it restarts in the next cycle.
    rate_hit: bool,
    /// Cycles until the exponential divider's carry restarts it.
    exp_wait: u8,
    /// Cycles until the level moves.
    step_wait: u8,
    /// A state change under way: 1 attack, 2 release, 3 decay.
    gate_edge: u8,
    /// Cycles until it happens.
    gate_delay: u8,
    /// Gate on: one rate compare still uses the decay rate (`env_test_ra_0100`).
    rate_decay: bool,
    /// The step under way goes up; attack must still hold when it lands.
    step_up: bool,
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
    /// Shift in progress: 2 = bit 19 rose, 1 = latched, 0 = idle.
    shift: u8,
    /// The next shift register value, loaded by the second clock phase.
    latch: u32,
    /// Pulse comparator output, 12 bits, one cycle behind the accumulator.
    pulse: u16,
    /// Waveform selector output (held and leaking with no waveform).
    out: u16,
    /// OSC3 register.
    osc: u8,
    /// The 8580's triangle/sawtooth output register.
    tri_saw: u16,
    /// Cycles left of the floating output, then of the shift register fade.
    float_ttl: u32,
    fade_ttl: u32,
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
            // All bits 1, the odd ones stored inverted (`SID/oscinit`).
            acc: 0x55_5555,
            // Bit 0 comes up low (`SID/oscinit`).
            lfsr: LFSR_ONES & !1,
            msb_rising: false,
            shift: 0,
            latch: LFSR_ONES,
            pulse: 0,
            out: 0,
            osc: 0,
            tri_saw: 0,
            float_ttl: 0,
            fade_ttl: 0,
            env: Envelope {
                exp_period: 1,
                ..Envelope::default()
            },
        }
    }
}

impl Voice {
    /// One phase-1 of the shift: every bit's output is latched into the
    /// next bit, `keep` is what the selector leaves of it. With the test
    /// bit the feedback's bit 22 input is held high.
    fn latch_shift(&mut self, keep: u32) {
        let test = self.ctrl & 0x08 != 0;
        let r = self.lfsr & keep;
        let bit = (u32::from(test) | (r >> 22)) ^ (r >> 17);
        self.latch = ((r << 1) | (bit & 1)) & LFSR_ONES;
    }

    /// What the selector leaves of the shift register's output bits.
    fn writeback_mask(&self) -> u32 {
        if self.ctrl >> 4 <= 8 || self.ctrl & 0x80 == 0 {
            return LFSR_ONES;
        }
        let mut keep = !NOISE_MASK & LFSR_ONES;
        for (i, b) in NOISE_BITS.iter().enumerate() {
            keep |= u32::from(self.out >> (11 - i) & 1) << b;
        }
        keep
    }

    fn noise(&self) -> u16 {
        let mut n = 0;
        for (i, b) in NOISE_BITS.iter().enumerate() {
            n |= ((self.lfsr >> b & 1) as u16) << (11 - i);
        }
        n
    }

    fn clock_osc(&mut self, model: SidModel) {
        if self.ctrl & 0x08 != 0 {
            self.msb_rising = false;
            if self.fade_ttl > 0 {
                self.fade_ttl -= 1;
                if self.fade_ttl == 0 {
                    self.lfsr = LFSR_ONES;
                }
            }
            self.latch_shift(LFSR_ONES);
            return;
        }
        let old = self.acc;
        self.acc = (self.acc + u32::from(self.freq)) & 0xff_ffff;
        self.msb_rising = old & 0x80_0000 == 0 && self.acc & 0x80_0000 != 0;
        match self.shift {
            2 => {
                self.latch_shift(self.writeback_mask());
                self.shift = 1;
            }
            1 => {
                self.lfsr = self.latch;
                self.shift = 0;
            }
            _ => {}
        }
        if old & 0x08_0000 == 0 && self.acc & 0x08_0000 != 0 {
            self.shift = 2;
        }
        // The 6581's sawtooth switch connects the adder's MSB directly to
        // the selector: combined waveforms pull it low (`SID/osc_topbit`).
        let sel = self.ctrl >> 4;
        if model == SidModel::Mos6581 && sel & 2 != 0 && sel != 2 && self.out & 0x800 == 0 {
            self.acc &= 0x7f_ffff;
        }
    }

    /// Waveform output for this cycle. `ring_msb` is the source's MSB.
    fn clock_wave(&mut self, ring_msb: bool, model: SidModel) {
        let test = self.ctrl & 0x08 != 0;
        let sel = self.ctrl >> 4;
        let saw = (self.acc >> 12) as u16;
        let pulse = if test || saw >= self.pw { 0xfff } else { 0 };
        if sel == 0 {
            if self.float_ttl > 0 {
                self.float_ttl -= 1;
                if self.float_ttl == 0 {
                    self.out &= self.out >> 1;
                    if self.out != 0 {
                        self.float_ttl = 1000;
                    }
                }
            }
            self.pulse = pulse;
            self.osc = (self.out >> 4) as u8;
            return;
        }
        // TriXOR = !Saw & ((!V3 & Ring) ^ bit 23), `SID/ringmod`.
        let msb = self.acc & 0x80_0000 != 0;
        let flip = sel & 2 == 0 && (msb ^ (self.ctrl & 0x04 != 0 && !ring_msb));
        let tri = (((if flip { self.acc ^ 0x7f_ffff } else { self.acc }) >> 11) & 0xfff) as u16;
        let mut out = 0xfff;
        if sel & 3 != 0 {
            let mut t = 0xfff;
            if sel & 1 != 0 {
                t &= tri;
            }
            if sel & 2 != 0 {
                t &= saw;
            }
            if model == SidModel::Mos8580 {
                out &= self.tri_saw;
                self.tri_saw = t;
            } else {
                out &= t;
            }
        }
        if sel & 4 != 0 {
            out &= self.pulse;
        }
        if sel & 8 != 0 {
            out &= self.noise();
        }
        self.out = out;
        self.osc = (out >> 4) as u8;
        self.float_ttl = float_hold(model);
        self.pulse = pulse;
        // Combined noise overwrites the register at every cycle but the
        // latching one, not only as it shifts (`SID/wb_testsuite`).
        if !test && self.shift != 1 {
            self.lfsr &= self.writeback_mask();
        }
    }

    fn rate_period(&self) -> u16 {
        RATE_PERIOD[usize::from(match self.env.state {
            _ if self.env.rate_decay => self.ad & 15,
            EnvState::Attack => self.ad >> 4,
            EnvState::DecaySustain => self.ad & 15,
            EnvState::Release => self.sr & 15,
        })]
    }

    fn clock_env(&mut self) {
        if self.env.gate_delay > 0 {
            self.env.gate_delay -= 1;
            let d = self.env.gate_delay;
            match self.env.gate_edge {
                1 if d == 0 => {
                    self.env.state = EnvState::Attack;
                    self.env.frozen = false;
                    self.env.gate_edge = 0;
                }
                2 if d == 0 => {
                    self.env.state = EnvState::Release;
                    self.env.gate_edge = 0;
                }
                3 if d == 0 => {
                    self.env.state = EnvState::DecaySustain;
                    self.env.gate_edge = 0;
                }
                _ => {}
            }
        }
        let period = self.rate_period();
        let sustain = (self.sr >> 4) * 17;
        let e = &mut self.env;
        if e.step_wait > 0 {
            e.step_wait -= 1;
            if e.step_wait == 0 && !e.frozen {
                if e.step_up && e.state == EnvState::Attack {
                    e.level = e.level.wrapping_add(1);
                    if e.level == 0xff {
                        e.gate_edge = 3;
                        e.gate_delay = 1;
                    }
                } else {
                    e.level = e.level.wrapping_sub(1);
                }
                e.exp_period = match e.level {
                    0xff => 1,
                    0x5d => 2,
                    0x36 => 4,
                    0x1a => 8,
                    0x0e => 16,
                    0x06 => 30,
                    0x00 => {
                        e.frozen = true;
                        1
                    }
                    _ => e.exp_period,
                };
            }
        } else if e.exp_wait > 0 {
            e.exp_wait -= 1;
            if e.exp_wait == 0 {
                e.exp_counter = 0;
                if e.state == EnvState::Attack {
                    e.step_wait = DOWN_STEP;
                    e.step_up = true;
                } else if e.state == EnvState::Release || e.level != sustain {
                    e.step_wait = DOWN_STEP;
                    e.step_up = false;
                }
            }
        } else if e.rate_hit {
            e.rate_hit = false;
            e.rate_counter = 0;
            if e.state == EnvState::Attack {
                e.exp_counter = 0;
                e.step_wait = ATTACK_STEP;
                e.step_up = true;
            } else if !e.frozen {
                e.exp_counter = e.exp_counter.wrapping_add(1) & 0x1f;
                if e.exp_counter == e.exp_period {
                    e.exp_wait = EXP_CARRY;
                }
            }
        }
        e.rate_counter = (e.rate_counter + 1) & RATE_WRAP;
        if e.rate_counter == period {
            e.rate_hit = true;
        }
        e.rate_decay = false;
    }

    fn write_ctrl(&mut self, v: u8, model: SidModel) {
        let was = self.ctrl;
        if was & 0x08 != 0 && v & 0x08 == 0 {
            // The second phase of the shift. The selector's output reaches
            // the latch only through a waveform both settings use; on the
            // 8580 through its register, whichever of the two they use
            // (`SID/wb_testsuite`).
            let shared = match model {
                SidModel::Mos6581 => was >> 4 & v >> 4 & 3 != 0,
                SidModel::Mos8580 => was >> 4 & 3 != 0 && v >> 4 & 3 != 0,
            };
            if shared {
                self.latch_shift(self.writeback_mask());
            }
            self.lfsr = self.latch;
        }
        if v & 1 != 0 && was & 1 == 0 {
            self.env.gate_edge = 1;
            self.env.gate_delay = GATE_ON;
            self.env.rate_decay = true;
        } else if v & 1 == 0 && was & 1 != 0 {
            self.env.gate_edge = 2;
            self.env.gate_delay = if self.env.step_wait > 0 {
                GATE_OFF_STEPPING
            } else {
                GATE_OFF
            };
        }
        self.ctrl = v;
        if v & 0x08 != 0 && was & 0x08 == 0 {
            self.acc = 0;
            self.shift = 0;
            self.fade_ttl = lfsr_fade(model);
            // Running into the test bit, triangle or sawtooth (now 0) on
            // the selector pull the outputs low; with the test bit already
            // on they do not (`SID/noiselfsrinit` against `wb_testsuite`).
            if v & 0x80 != 0 && v & 0x30 != 0 {
                self.lfsr &= !NOISE_MASK;
            }
            self.latch_shift(LFSR_ONES);
        }
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
    /// Cycles until that value has leaked away.
    bus_ttl: u32,
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
            bus_ttl: 0,
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
        if self.bus_ttl > 0 {
            self.bus_ttl -= 1;
            if self.bus_ttl == 0 {
                self.bus = 0;
            }
        }
        let model = self.model;
        for v in &mut self.voices {
            v.clock_osc(model);
        }
        for i in 0..3 {
            let src = (i + 2) % 3;
            if self.voices[i].ctrl & 0x02 != 0 && self.voices[src].msb_rising {
                self.voices[i].acc = 0;
            }
        }
        for i in 0..3 {
            let ring = self.voices[(i + 2) % 3].acc & 0x80_0000 != 0;
            self.voices[i].clock_wave(ring, model);
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
            let v = &self.voices[i];
            let out = (i32::from(v.out) - 2048) * i32::from(v.env.level);
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
            0x1b => self.voices[2].osc,
            0x1c => self.voices[2].env.level,
            _ => self.bus,
        }
    }

    /// A read: the read-only registers refresh the internal data bus
    /// (`SID/busvalue`).
    pub fn read(&mut self, reg: u8) -> u8 {
        let v = self.peek(reg);
        if matches!(reg & 0x1f, 0x19..=0x1c) {
            self.bus = v;
            self.bus_ttl = bus_hold(self.model);
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        self.bus = v;
        self.bus_ttl = bus_hold(self.model);
        let reg = reg & 0x1f;
        if reg < 0x15 {
            let voice = &mut self.voices[usize::from(reg / 7)];
            match reg % 7 {
                0 => voice.freq = (voice.freq & 0xff00) | u16::from(v),
                1 => voice.freq = (voice.freq & 0x00ff) | (u16::from(v) << 8),
                2 => voice.pw = (voice.pw & 0x0f00) | u16::from(v),
                3 => voice.pw = (voice.pw & 0x00ff) | (u16::from(v & 15) << 8),
                4 => voice.write_ctrl(v, self.model),
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
const VERSION: u16 = 3;

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
            w.u32(self.bus_ttl);
            for v in &self.voices {
                w.u8(v.shift);
                w.u32(v.latch);
                w.u16(v.pulse);
                w.u16(v.out);
                w.u8(v.osc);
                w.u16(v.tri_saw);
                w.u32(v.float_ttl);
                w.u32(v.fade_ttl);
                let e = &v.env;
                w.u8(e.exp_period);
                w.u8(e.gate_delay);
                w.bool(e.rate_decay);
                w.bool(e.step_up);
                w.bool(e.frozen);
                w.bool(e.rate_hit);
                w.u8(e.exp_wait);
                w.u8(e.step_wait);
                w.u8(e.gate_edge);
            }
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
            v.env.rate_counter = c.u16()? & RATE_WRAP;
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
        // Version 2 predates the pipelines: they start idle.
        if version >= 3 {
            s.bus_ttl = c.u32()?;
            for v in s.voices.iter_mut() {
                v.shift = c.u8()? & 3;
                v.latch = c.u32()? & LFSR_ONES;
                v.pulse = c.u16()? & 0xfff;
                v.out = c.u16()? & 0xfff;
                v.osc = c.u8()?;
                v.tri_saw = c.u16()? & 0xfff;
                v.float_ttl = c.u32()?;
                v.fade_ttl = c.u32()?;
                let e = &mut v.env;
                e.exp_period = c.u8()?.max(1);
                e.gate_delay = c.u8()?;
                e.rate_decay = c.bool()?;
                e.step_up = c.bool()?;
                e.frozen = c.bool()?;
                e.rate_hit = c.bool()?;
                e.exp_wait = c.u8()?;
                e.step_wait = c.u8()?;
                e.gate_edge = c.u8()?;
            }
        }
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
