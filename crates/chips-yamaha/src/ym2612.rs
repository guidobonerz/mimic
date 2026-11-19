//! Yamaha YM2612 (OPN2): six four-operator FM channels, an 8-bit DAC in
//! place of channel 6, an LFO, two timers, stereo.
//!
//! **Interface.** Four ports: address and data of part I (channels 1–3 and
//! the common registers), address and data of part II (channels 4–6).
//! Reading any port gives the status: bit 7 busy (always 0 here), bit 1
//! timer B overflowed, bit 0 timer A. [`Ym2612::tick`] is one clock of the
//! 68000's 7.67 MHz input; a sample of all channels comes every 144
//! clocks.
//!
//! **Registers.** $22 LFO (bit 3 on, bits 2–0 the rate: 3.98, 5.56, 6.02,
//! 6.37, 6.88, 9.63, 48.1, 72.2 Hz at 8 MHz); $24/$25 timer A (10 bits,
//! a sample a step), $26 timer B (8 bits, 16 samples a step); $27 bits
//! 7–6 channel 3 mode (01: one frequency per operator), bits 5–4 reset the
//! flags, 3–2 enable them, 1–0 run the timers; $28 key on (bits 7–4 the
//! operators 4, 3, 2, 1; bits 2–0 the channel: 0–2, 4–6); $2A the DAC
//! sample, $2B bit 7 the DAC on. Per operator ($30 + channel + 4 ×
//! operator in the register order 1, 3, 2, 4): $30 detune and multiple,
//! $40 total level (0.75 dB steps), $50 rate scaling and attack, $60 AM
//! and the first decay, $70 the second decay, $80 the first decay level
//! and release, $90 SSG-EG. Per channel: $A4/$A0 block and F-number (the
//! high byte is latched until the low one is written), $AC–$AE/$A8–$AA
//! channel 3's operators 3, 1, 2 in the special mode, $B0 feedback and
//! algorithm, $B4 left, right, AMS, FMS.
//!
//! **Model.** An operator's 20-bit phase advances by `(fnum << block) / 2`
//! plus the detune (YM2608 manual, table 2-6, 0.053 Hz a unit), times the
//! multiple (0 = ½); its top 10 bits index a log-sine table, the
//! attenuation (10-bit envelope in 0.094 dB steps, total level, tremolo)
//! is added in the log domain. The envelope runs every third sample with
//! the OPN family's rate pattern: rate = 2 × R + key code >> (3 − RS),
//! attack exponential, decays and release linear; the release rate is
//! 2 × RR + 1. Algorithms 0–7 connect operators 1–4 as in the manual;
//! operator 1 feeds back into itself.
//!
//! Simplifications: SSG-EG's repeat, alternate, hold and invert are there
//! without the chip's exact timing; the DAC is not reduced to 9 bits; the
//! channels are summed (the chip time-multiplexes them on its DAC); the
//! busy flag is never set.
//!
//! Sources: Sega, *Genesis Software Manual* ("Sound": the YM2612's
//! registers, timers, the DAC, rates and levels); Yamaha, *YM2608 OPNA
//! Application Manual* (translated; detune, LFO, PMS/AMS, SSG-EG,
//! envelope). The implementation is this project's own.

use std::sync::OnceLock;

use mimic_core::savestate::{StateError, StateReader, StateWriter};

/// Input clocks per sample.
pub const CLOCKS_PER_SAMPLE: u8 = 144;

/// Detune in phase units by key code (0–31) for FD 1–3 (YM2608 manual
/// table 2-6 in Hz / 0.053).
const DETUNE: [[u8; 32]; 3] = [
    [
        0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8,
        8, 8,
    ],
    [
        1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 11, 12, 13, 14,
        16, 16, 16, 16,
    ],
    [
        2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19,
        20, 22, 22, 22, 22,
    ],
];

/// Samples per LFO step (128 steps a cycle) for the eight rates.
const LFO_DIV: [u32; 8] = [108, 77, 71, 67, 62, 44, 8, 5];
/// Vibrato depth by FMS, × F-number / 65536 at full swing.
const PM_DEPTH: [i32; 8] = [0, 129, 254, 379, 531, 760, 1534, 3100];
/// Tremolo depth shift by AMS (0: none).
const AM_SHIFT: [u32; 4] = [8, 3, 1, 0];
/// Envelope increments for rates 0–47 by the rate's low two bits.
const EG_PATTERN: [[u8; 8]; 4] = [
    [0, 1, 0, 1, 0, 1, 0, 1],
    [0, 1, 0, 1, 1, 1, 0, 1],
    [0, 1, 1, 1, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 1],
];
const ENV_MAX: u16 = 0x3ff;

struct Tables {
    logsin: [u16; 256],
    exp: [u16; 256],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Tables {
            logsin: [0; 256],
            exp: [0; 256],
        };
        for i in 0..256 {
            let s = ((i as f64 + 0.5) * std::f64::consts::PI / 512.0).sin();
            t.logsin[i] = (-s.log2() * 256.0).round() as u16;
            t.exp[i] = ((i as f64 / 256.0).exp2() * 1024.0).round() as u16 - 1024;
        }
        t
    })
}

/// A sine lookup: `idx` 10 bits a cycle, `att` in 0.094 dB steps; ±8168.
fn wave(idx: u32, att: u32) -> i32 {
    let t = tables();
    let i = idx & 1023;
    let q = if i & 256 != 0 {
        255 - (i & 255)
    } else {
        i & 255
    };
    let a = u32::from(t.logsin[q as usize]) + (att << 2);
    let shift = a >> 8;
    if shift > 13 {
        return 0;
    }
    let v = (((u32::from(t.exp[(!a & 255) as usize]) + 1024) << 2) >> shift) as i32;
    if i & 512 != 0 {
        -v
    } else {
        v
    }
}

/// The envelope increment for `rate` at `counter`.
fn eg_inc(rate: u32, counter: u32) -> u32 {
    if rate < 2 {
        return 0;
    }
    if rate < 48 {
        let shift = 11 - rate / 4;
        if counter & ((1 << shift) - 1) != 0 {
            return 0;
        }
        return u32::from(EG_PATTERN[(rate & 3) as usize][((counter >> shift) & 7) as usize]);
    }
    let step = (counter & 7) as usize;
    let base = 1u32 << ((rate - 48) / 4);
    let extra = u32::from(
        EG_PATTERN[(rate & 3) as usize][step] == 1 && (rate & 3) != 0 && pattern_hi(rate, step),
    );
    base + base * extra
}

/// Rates 48–63: which of the eight steps take the doubled increment.
fn pattern_hi(rate: u32, step: usize) -> bool {
    match rate & 3 {
        1 => step == 3 || step == 7,
        2 => step & 1 == 1,
        3 => step & 3 != 0,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum Eg {
    Attack,
    Decay,
    Sustain,
    #[default]
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Op {
    phase: u32,
    env: u16,
    eg: Eg,
    key: bool,
    /// SSG-EG: the output is inverted.
    ssg_inv: bool,
}

impl Default for Op {
    fn default() -> Self {
        Self {
            phase: 0,
            env: ENV_MAX,
            eg: Eg::Release,
            key: false,
            ssg_inv: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ym2612 {
    /// Registers of part I ($00–$FF) and part II ($100–$1FF).
    pub regs: Vec<u8>,
    addr: [u8; 2],
    /// The latched $A4–$A6 / $AC–$AE bytes per part.
    latch: [u8; 2],
    /// Block/F-number per channel (14 bits), and channel 3's three extra.
    freq: [u16; 6],
    freq3: [u16; 3],
    ops: [Op; 24],
    /// Operator 1's last two outputs per channel.
    fb: [[i32; 2]; 6],
    timer_a: u16,
    timer_b: u16,
    timer_b_sub: u8,
    status: u8,
    eg_counter: u32,
    eg_sub: u8,
    lfo_step: u8,
    lfo_count: u32,
    clocks: u8,
    out: (i32, i32),
}

impl Default for Ym2612 {
    fn default() -> Self {
        Self::new()
    }
}

/// Register offset → operator slot (S1, S3, S2, S4 order).
const SLOT_OF_OFFSET: [usize; 4] = [0, 2, 1, 3];

impl Ym2612 {
    pub fn new() -> Self {
        let mut regs = vec![0u8; 0x200];
        for part in [0usize, 0x100] {
            for ch in 0..3 {
                regs[part + 0xb4 + ch] = 0xc0;
            }
        }
        Self {
            regs,
            addr: [0; 2],
            latch: [0; 2],
            freq: [0; 6],
            freq3: [0; 3],
            ops: [Op::default(); 24],
            fb: [[0; 2]; 6],
            timer_a: 0,
            timer_b: 0,
            timer_b_sub: 0,
            status: 0,
            eg_counter: 0,
            eg_sub: 0,
            lfo_step: 0,
            lfo_count: 0,
            clocks: 0,
            out: (0, 0),
        }
    }

    pub fn read_status(&self) -> u8 {
        self.status
    }

    /// A write to port 0–3.
    pub fn write(&mut self, port: u8, v: u8) {
        let part = usize::from(port >> 1 & 1);
        if port & 1 == 0 {
            self.addr[part] = v;
            return;
        }
        let a = self.addr[part];
        if part == 0 && a < 0x30 {
            self.write_common(a, v);
            return;
        }
        if a < 0x30 {
            return;
        }
        self.regs[part * 0x100 + usize::from(a)] = v;
        let ch = usize::from(a & 3);
        if ch == 3 {
            return;
        }
        let channel = part * 3 + ch;
        match a & 0xfc {
            0xa4 | 0xac => self.latch[part] = v & 0x3f,
            0xa0 => self.freq[channel] = u16::from(self.latch[part]) << 8 | u16::from(v),
            0xa8 if part == 0 => self.freq3[ch] = u16::from(self.latch[part]) << 8 | u16::from(v),
            _ => {}
        }
    }

    fn write_common(&mut self, a: u8, v: u8) {
        self.regs[usize::from(a)] = v;
        match a {
            0x27 => {
                if v & 0x10 != 0 {
                    self.status &= !1;
                }
                if v & 0x20 != 0 {
                    self.status &= !2;
                }
                if v & 1 != 0 && self.regs[0x27] & 1 == 0 {
                    self.timer_a = self.timer_a_period();
                }
            }
            0x28 => {
                let c = usize::from(v & 7);
                let channel = match c {
                    0..=2 => c,
                    4..=6 => c - 1,
                    _ => return,
                };
                for slot in 0..4 {
                    let on = v & (0x10 << slot) != 0;
                    let o = &mut self.ops[channel * 4 + slot];
                    if on && !o.key {
                        o.phase = 0;
                        o.eg = Eg::Attack;
                        o.ssg_inv = false;
                    } else if !on && o.key {
                        o.eg = Eg::Release;
                    }
                    o.key = on;
                }
            }
            _ => {}
        }
    }

    fn timer_a_period(&self) -> u16 {
        1024 - (u16::from(self.regs[0x24]) << 2 | u16::from(self.regs[0x25] & 3))
    }

    /// The last sample (left, right), each about ±50000.
    pub fn output(&self) -> (i32, i32) {
        self.out
    }

    pub fn tick(&mut self) {
        self.clocks += 1;
        if self.clocks == CLOCKS_PER_SAMPLE {
            self.clocks = 0;
            self.sample();
        }
    }

    fn timers(&mut self) {
        let ctl = self.regs[0x27];
        if ctl & 1 != 0 {
            self.timer_a = self.timer_a.saturating_sub(1);
            if self.timer_a == 0 {
                if ctl & 4 != 0 {
                    self.status |= 1;
                }
                self.timer_a = self.timer_a_period();
            }
        }
        self.timer_b_sub += 1;
        if self.timer_b_sub == 16 {
            self.timer_b_sub = 0;
            if ctl & 2 != 0 {
                self.timer_b = self.timer_b.saturating_sub(1);
                if self.timer_b == 0 {
                    if ctl & 8 != 0 {
                        self.status |= 2;
                    }
                    self.timer_b = 256 - u16::from(self.regs[0x26]);
                }
            } else {
                self.timer_b = 256 - u16::from(self.regs[0x26]);
            }
        }
    }

    /// The LFO: (tremolo 0–126, vibrato −127–127).
    fn lfo(&mut self) -> (u32, i32) {
        let r = self.regs[0x22];
        if r & 8 == 0 {
            self.lfo_step = 0;
            self.lfo_count = 0;
            return (0, 0);
        }
        self.lfo_count += 1;
        if self.lfo_count >= LFO_DIV[usize::from(r & 7)] {
            self.lfo_count = 0;
            self.lfo_step = (self.lfo_step + 1) & 127;
        }
        let s = u32::from(self.lfo_step);
        let am = if s < 64 { s * 2 } else { (127 - s) * 2 };
        // A triangle for the vibrato, 0 at steps 0 and 64.
        let q = s as i32 & 63;
        let pm = if q < 32 { q * 4 } else { (64 - q) * 4 }.min(127);
        (am, if s < 64 { pm } else { -pm })
    }

    fn sample(&mut self) {
        self.timers();
        let (am, pm) = self.lfo();
        self.eg_sub += 1;
        let eg_tick = self.eg_sub == 3;
        if eg_tick {
            self.eg_sub = 0;
            self.eg_counter = self.eg_counter.wrapping_add(1);
        }
        let special3 = self.regs[0x27] & 0xc0 == 0x40;
        let dac_on = self.regs[0x2b] & 0x80 != 0;
        let (mut left, mut right) = (0i32, 0i32);
        for ch in 0..6 {
            let part = (ch / 3) * 0x100;
            let c = ch % 3;
            let pan = self.regs[part + 0xb4 + c];
            let fms = usize::from(pan & 7);
            let ams = usize::from(pan >> 4 & 3);
            let alg = self.regs[part + 0xb0 + c] & 7;
            let fb = u32::from(self.regs[part + 0xb0 + c] >> 3 & 7);
            let mut outs = [0i32; 4];
            let mut atts = [0u32; 4];
            let mut phases = [0u32; 4];
            for (off, &slot) in SLOT_OF_OFFSET.iter().enumerate() {
                let r = |base: usize| self.regs[part + base + off * 4 + c];
                let freq = if special3 && ch == 2 && slot != 3 {
                    // Operators 1, 2, 3 from $A9, $AA, $A8.
                    self.freq3[[1, 2, 0][slot]]
                } else {
                    self.freq[ch]
                };
                let block = u32::from(freq >> 11 & 7);
                let mut fnum = i32::from(freq & 0x7ff);
                if fms != 0 && pm != 0 {
                    fnum += (fnum * PM_DEPTH[fms] * pm / 127) >> 16;
                }
                let fnum = fnum.max(0) as u32;
                let kc = key_code(freq);
                let dt_mul = r(0x30);
                let dt = dt_mul >> 4 & 7;
                let mut inc = (fnum << block) >> 1;
                if dt & 3 != 0 {
                    let d = u32::from(DETUNE[usize::from(dt & 3) - 1][kc as usize]);
                    inc = if dt & 4 != 0 {
                        inc.wrapping_sub(d)
                    } else {
                        inc + d
                    };
                }
                inc &= 0x1ffff;
                let mul = u32::from(dt_mul & 15);
                let inc = if mul == 0 { inc >> 1 } else { inc * mul };
                let o = &mut self.ops[ch * 4 + slot];
                phases[slot] = o.phase >> 10;
                o.phase = (o.phase + inc) & 0xf_ffff;
                let ks = kc >> (3 - (r(0x50) >> 6));
                if eg_tick {
                    step_env(
                        o,
                        &EgRegs::from(&[r(0x50), r(0x60), r(0x70), r(0x80), r(0x90)]),
                        ks,
                        self.eg_counter,
                    );
                }
                let mut env = u32::from(o.env);
                if r(0x90) & 8 != 0 && o.ssg_inv && o.eg != Eg::Release {
                    env = (0x200u32.wrapping_sub(env)) & 0x3ff;
                }
                let tl = u32::from(r(0x40) & 0x7f) << 3;
                let trem = if r(0x60) & 0x80 != 0 && ams != 0 {
                    am >> AM_SHIFT[ams]
                } else {
                    0
                };
                atts[slot] = (env + tl + trem).min(0x3ff);
            }
            // Operator 1 with feedback.
            let f = if fb == 0 {
                0
            } else {
                (self.fb[ch][0] + self.fb[ch][1]) >> (10 - fb)
            };
            let w =
                |slot: usize, m: i32| wave(phases[slot].wrapping_add((m >> 1) as u32), atts[slot]);
            outs[0] = wave(phases[0].wrapping_add(f as u32), atts[0]);
            self.fb[ch] = [self.fb[ch][1], outs[0]];
            let o1 = outs[0];
            let sum = match alg {
                0 => {
                    let o2 = w(1, o1);
                    let o3 = w(2, o2);
                    w(3, o3)
                }
                1 => {
                    let o2 = w(1, 0);
                    let o3 = w(2, o1 + o2);
                    w(3, o3)
                }
                2 => {
                    let o2 = w(1, 0);
                    let o3 = w(2, o2);
                    w(3, o1 + o3)
                }
                3 => {
                    let o2 = w(1, o1);
                    let o3 = w(2, 0);
                    w(3, o2 + o3)
                }
                4 => {
                    let o2 = w(1, o1);
                    let o3 = w(2, 0);
                    o2 + w(3, o3)
                }
                5 => w(1, o1) + w(2, o1) + w(3, o1),
                6 => w(1, o1) + w(2, 0) + w(3, 0),
                _ => o1 + w(1, 0) + w(2, 0) + w(3, 0),
            };
            let mut v = sum.clamp(-8192, 8191);
            if ch == 5 && dac_on {
                v = (i32::from(self.regs[0x2a]) - 128) << 6;
            }
            if pan & 0x80 != 0 {
                left += v;
            }
            if pan & 0x40 != 0 {
                right += v;
            }
        }
        self.out = (left, right);
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"OPN2", 1, |w| {
            w.raw(&self.regs);
            w.raw(&self.addr);
            w.raw(&self.latch);
            for f in self.freq {
                w.u16(f);
            }
            for f in self.freq3 {
                w.u16(f);
            }
            for o in &self.ops {
                w.u32(o.phase);
                w.u16(o.env);
                w.u8(o.eg as u8);
                w.bool(o.key);
                w.bool(o.ssg_inv);
            }
            for f in self.fb {
                w.u32(f[0] as u32);
                w.u32(f[1] as u32);
            }
            w.u16(self.timer_a);
            w.u16(self.timer_b);
            w.u8(self.timer_b_sub);
            w.u8(self.status);
            w.u32(self.eg_counter);
            w.u8(self.eg_sub);
            w.u8(self.lfo_step);
            w.u32(self.lfo_count);
            w.u8(self.clocks);
            w.u32(self.out.0 as u32);
            w.u32(self.out.1 as u32);
        });
    }

    /// Atomic: on an error nothing changed.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"OPN2", 1)?;
        let mut n = Self::new();
        c.raw_into(&mut n.regs)?;
        c.raw_into(&mut n.addr)?;
        c.raw_into(&mut n.latch)?;
        for f in &mut n.freq {
            *f = c.u16()? & 0x3fff;
        }
        for f in &mut n.freq3 {
            *f = c.u16()? & 0x3fff;
        }
        for o in &mut n.ops {
            o.phase = c.u32()? & 0xf_ffff;
            o.env = c.u16()?.min(ENV_MAX);
            o.eg = match c.u8()? {
                0 => Eg::Attack,
                1 => Eg::Decay,
                2 => Eg::Sustain,
                3 => Eg::Release,
                _ => return Err(StateError::Invalid("OPN2 envelope state")),
            };
            o.key = c.bool()?;
            o.ssg_inv = c.bool()?;
        }
        for f in &mut n.fb {
            *f = [c.u32()? as i32, c.u32()? as i32];
        }
        n.timer_a = c.u16()?.min(1024);
        n.timer_b = c.u16()?.min(256);
        n.timer_b_sub = c.u8()? % 16;
        n.status = c.u8()? & 3;
        n.eg_counter = c.u32()?;
        n.eg_sub = c.u8()? % 3;
        n.lfo_step = c.u8()? & 127;
        n.lfo_count = c.u32()?.min(108);
        n.clocks = c.u8()? % CLOCKS_PER_SAMPLE;
        n.out = (c.u32()? as i32, c.u32()? as i32);
        *self = n;
        Ok(())
    }
}

/// The key code: block and the F-number's top bits (N4 N3).
fn key_code(freq: u16) -> u32 {
    let block = u32::from(freq >> 11 & 7);
    let f = u32::from(freq & 0x7ff);
    let n4 = f >> 10 & 1;
    let (f9, f8, f7) = (f >> 9 & 1, f >> 8 & 1, f >> 7 & 1);
    let n3 = (n4 & (f9 | f8 | f7)) | ((n4 ^ 1) & f9 & f8 & f7);
    block << 2 | n4 << 1 | n3
}

struct EgRegs {
    ar: u32,
    d1r: u32,
    d2r: u32,
    rr: u32,
    sl: u16,
    ssg: u8,
}

impl EgRegs {
    fn from(r: &[u8; 5]) -> Self {
        let sl = u16::from(r[3] >> 4);
        Self {
            ar: u32::from(r[0] & 0x1f),
            d1r: u32::from(r[1] & 0x1f),
            d2r: u32::from(r[2] & 0x1f),
            rr: u32::from(r[3] & 15) * 2 + 1,
            sl: if sl == 15 { ENV_MAX } else { sl << 5 },
            ssg: r[4] & 15,
        }
    }
}

fn rate(r: u32, ks: u32) -> u32 {
    if r == 0 {
        0
    } else {
        (2 * r + ks).min(63)
    }
}

fn step_env(o: &mut Op, e: &EgRegs, ks: u32, counter: u32) {
    let ssg = e.ssg & 8 != 0;
    match o.eg {
        Eg::Attack => {
            let r = rate(e.ar, ks);
            if r >= 62 {
                o.env = 0;
            } else {
                let inc = eg_inc(r, counter);
                if inc > 0 {
                    let env = i32::from(o.env);
                    let next = env + (((-env - 1) * inc as i32) >> 4);
                    o.env = next.max(0) as u16;
                }
            }
            if o.env == 0 {
                o.eg = Eg::Decay;
            }
        }
        Eg::Decay | Eg::Sustain => {
            let r = if o.eg == Eg::Decay { e.d1r } else { e.d2r };
            let mut inc = eg_inc(rate(r, ks), counter) as u16;
            if ssg {
                inc *= 4;
            }
            o.env = (o.env + inc).min(ENV_MAX);
            if o.eg == Eg::Decay && o.env >= e.sl {
                o.eg = Eg::Sustain;
            }
            if ssg && o.env >= 0x200 {
                // SSG-EG: bit 0 hold, bit 1 alternate, bit 2 attack.
                if e.ssg & 1 != 0 {
                    o.env = 0x200;
                    if e.ssg & 2 != 0 {
                        o.ssg_inv = true;
                    }
                } else {
                    if e.ssg & 2 != 0 {
                        o.ssg_inv = !o.ssg_inv;
                    }
                    o.phase = 0;
                    o.eg = Eg::Attack;
                }
            }
        }
        Eg::Release => {
            let mut inc = eg_inc(rate(e.rr, ks), counter) as u16;
            if ssg {
                inc *= 4;
            }
            o.env = (o.env + inc).min(ENV_MAX);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(c: &mut Ym2612, part: u8, r: u8, v: u8) {
        c.write(part * 2, r);
        c.write(part * 2 + 1, v);
    }

    fn samples(c: &mut Ym2612, n: usize) -> Vec<(i32, i32)> {
        (0..n)
            .map(|_| {
                for _ in 0..CLOCKS_PER_SAMPLE {
                    c.tick();
                }
                c.output()
            })
            .collect()
    }

    /// The manual's "Grand Piano" test note on channel 1.
    fn piano(c: &mut Ym2612) {
        for (r, v) in [
            (0x30, 0x71),
            (0x34, 0x0d),
            (0x38, 0x33),
            (0x3c, 0x01),
            (0x40, 0x23),
            (0x44, 0x2d),
            (0x48, 0x26),
            (0x4c, 0x00),
            (0x50, 0x5f),
            (0x54, 0x99),
            (0x58, 0x5f),
            (0x5c, 0x94),
            (0x60, 5),
            (0x64, 5),
            (0x68, 5),
            (0x6c, 7),
            (0x70, 2),
            (0x74, 2),
            (0x78, 2),
            (0x7c, 2),
            (0x80, 0x11),
            (0x84, 0x11),
            (0x88, 0x11),
            (0x8c, 0xa6),
            (0xb0, 0x32),
            (0xb4, 0xc0),
            (0x28, 0x00),
            (0xa4, 0x22),
            (0xa0, 0x69),
            (0x28, 0xf0),
        ] {
            write(c, 0, r, v);
        }
    }

    #[test]
    fn the_sine_peaks_and_the_key_code() {
        assert_eq!(wave(256, 0), 8168);
        assert_eq!(wave(768, 0), -8168);
        assert_eq!(key_code(4 << 11 | 0x469), 4 << 2 | 0b10);
        assert_eq!(eg_inc(63, 0), 8);
        assert_eq!(eg_inc(0, 0), 0);
    }

    #[test]
    fn the_manuals_piano_note_sounds_and_releases() {
        let mut c = Ym2612::new();
        piano(&mut c);
        let s = samples(&mut c, 20_000);
        assert!(s.iter().any(|&(l, _)| l.abs() > 2000), "it sounds");
        assert!(s.iter().all(|&(l, r)| l == r), "both speakers");
        write(&mut c, 0, 0x28, 0x00);
        let s = samples(&mut c, 53_267);
        assert!(
            s[s.len() - 500..].iter().all(|&(l, _)| l.abs() < 40),
            "released"
        );
    }

    #[test]
    fn the_frequency_follows_block_and_f_number() {
        // Algorithm 7, only operator 1, multiple 1: a sine.
        let mut c = Ym2612::new();
        write(&mut c, 0, 0xb0, 0x07);
        write(&mut c, 0, 0x30, 0x01);
        write(&mut c, 0, 0x50, 0x1f);
        for r in [0x44, 0x48, 0x4c] {
            write(&mut c, 0, r, 0x7f);
        }
        // F-number 1037, block 4: 1037 × 2^4 × 53267 / 2^21 ≈ 421 Hz.
        write(&mut c, 0, 0xa4, 4 << 3 | (1037 >> 8) as u8);
        write(&mut c, 0, 0xa0, 1037u16 as u8);
        write(&mut c, 0, 0x28, 0x10);
        let s = samples(&mut c, 53_267);
        let n = s.windows(2).filter(|w| w[0].0 < 0 && w[1].0 >= 0).count();
        assert!((415..=427).contains(&n), "{n} Hz");
    }

    #[test]
    fn timers_dac_and_pan() {
        let mut c = Ym2612::new();
        write(&mut c, 0, 0x24, 0xff);
        write(&mut c, 0, 0x25, 0x03);
        write(&mut c, 0, 0x27, 0x05);
        samples(&mut c, 2);
        assert_eq!(c.read_status() & 1, 1, "timer A: one sample");
        write(&mut c, 0, 0x27, 0x15);
        assert_eq!(c.read_status() & 1, 0);
        write(&mut c, 0, 0x26, 0xff);
        write(&mut c, 0, 0x27, 0x0a);
        samples(&mut c, 40);
        assert_eq!(c.read_status() & 2, 2, "timer B: 16 samples");
        // The DAC on channel 6, left only.
        write(&mut c, 1, 0xb6, 0x80);
        write(&mut c, 0, 0x2b, 0x80);
        write(&mut c, 0, 0x2a, 0xff);
        let s = samples(&mut c, 2);
        assert_eq!(s[1], (127 << 6, 0));
    }

    #[test]
    fn savestates_round_trip() {
        let mut a = Ym2612::new();
        piano(&mut a);
        samples(&mut a, 700);
        let mut w = StateWriter::new();
        a.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Ym2612::new();
        b.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(samples(&mut a, 300), samples(&mut b, 300));
        assert!(b.load(&mut StateReader::new(&bytes[..40])).is_err());
    }
}
