//! Yamaha YM2413 (OPLL): nine two-operator FM channels, or six plus five
//! rhythm instruments; 15 fixed instruments in ROM and one user tone.
//!
//! **Interface.** Two write-only ports: address (A0 = 0) and data (A0 = 1).
//! [`Ym2413::tick`] is one clock of the 3.579545 MHz input; a sample of
//! all channels comes every 72 clocks (49.7 kHz), [`Ym2413::output`] holds
//! the last one.
//!
//! **Registers** (datasheet, "Register map"): $00–$07 the user tone
//! (modulator, carrier): $00/$01 AM, VIB, EG-TYP (1 sustained), KSR,
//! MULTI; $02 KSL (mod) and TL; $03 KSL (car), DC/DM (half-sine
//! carrier/modulator), FB; $04/$05 AR, DR; $06/$07 SL, RR. $0E bit 5
//! rhythm mode, bits 4–0 the keys of BD, SD, TOM, T-CT, HH. $10–$18
//! F-number low 8 bits; $20–$28 SUS, KEY, BLOCK, F-number bit 8; $30–$38
//! instrument (bits 7–4) and volume (3 dB a step; in rhythm mode $36 BD,
//! $37 HH/SD, $38 TOM/T-CT volumes).
//!
//! **Model.** The usual log-sine / exponent table synthesis of Yamaha's
//! OPL family: an operator's phase (19 bits) advances by
//! `fnum << block × MULTI / 2` a sample, its top 10 bits index a sine
//! (half-sine with DC/DM), the attenuation (envelope 0.375 dB steps, TL
//! 0.75 dB, volume 3 dB, key scale level, tremolo) is added in the log
//! domain. The carrier is phase-modulated by the modulator; the
//! modulator by itself (feedback). Envelopes: attack, decay to SL,
//! sustain (EG-TYP 1) or decay on at RR (EG-TYP 0), release at RR, at 7
//! without EG-TYP or at 5 with SUS; the rates scale with KSR. Vibrato
//! ±13.75 cents at 6.1 Hz, tremolo 4.8 dB at 3.7 Hz. The rhythm
//! instruments use channels 6–8: BD both operators, HH and SD the
//! operators of channel 7, TOM and T-CT those of channel 8, with the
//! cymbal/hat phase mixing of the OPL family and a noise generator.
//!
//! Simplifications of this project: no damp phase before an attack, the
//! envelope rates are the OPL family's pattern (not measured on a YM2413),
//! the output is the sum of the channels (the real chip time-multiplexes
//! them on its DAC).
//!
//! **Instrument ROM.** No exact dump of the YM2413's ROM exists; the
//! values are Mitsutaka Okazaki's estimates as published in emu2413
//! (MIT licence, © 2001-2019 Mitsutaka Okazaki), the rhythm tones from
//! the VRC7's debug-mode dump (NESdev Wiki, "VRC7 audio").
//!
//! Sources: Yamaha, *YM2413 FM Operator Type-LL (OPLL)* datasheet (1986);
//! SMS Power, "YM2413"; github.com/andete/ym2413 (openMSX reverse
//! engineering notes); emu2413's instrument table (see above). The
//! implementation is this project's own.

use std::sync::OnceLock;

use mimic_core::savestate::{StateError, StateReader, StateWriter};

/// Input clocks per sample.
pub const CLOCKS_PER_SAMPLE: u8 = 72;

/// The fixed instruments 1–15 and the rhythm tones (BD, HH/SD, TOM/T-CT),
/// eight bytes each as the user tone's registers (see the module doc).
const ROM: [[u8; 8]; 19] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x71, 0x61, 0x1e, 0x17, 0xd0, 0x78, 0x00, 0x17], // violin
    [0x13, 0x41, 0x1a, 0x0d, 0xd8, 0xf7, 0x23, 0x13], // guitar
    [0x13, 0x01, 0x99, 0x00, 0xf2, 0xc4, 0x21, 0x23], // piano
    [0x11, 0x61, 0x0e, 0x07, 0x8d, 0x64, 0x70, 0x27], // flute
    [0x32, 0x21, 0x1e, 0x06, 0xe1, 0x76, 0x01, 0x28], // clarinet
    [0x31, 0x22, 0x16, 0x05, 0xe0, 0x71, 0x00, 0x18], // oboe
    [0x21, 0x61, 0x1d, 0x07, 0x82, 0x81, 0x11, 0x07], // trumpet
    [0x33, 0x21, 0x2d, 0x13, 0xb0, 0x70, 0x00, 0x07], // organ
    [0x61, 0x61, 0x1b, 0x06, 0x64, 0x65, 0x10, 0x17], // horn
    [0x41, 0x61, 0x0b, 0x18, 0x85, 0xf0, 0x81, 0x07], // synthesizer
    [0x33, 0x01, 0x83, 0x11, 0xea, 0xef, 0x10, 0x04], // harpsichord
    [0x17, 0xc1, 0x24, 0x07, 0xf8, 0xf8, 0x22, 0x12], // vibraphone
    [0x61, 0x50, 0x0c, 0x05, 0xd2, 0xf5, 0x40, 0x42], // synthesizer bass
    [0x01, 0x01, 0x55, 0x03, 0xe9, 0x90, 0x03, 0x02], // acoustic bass
    [0x41, 0x41, 0x89, 0x03, 0xf1, 0xe4, 0xc0, 0x13], // electric guitar
    [0x01, 0x01, 0x18, 0x0f, 0xdf, 0xf8, 0x6a, 0x6d], // BD
    [0x01, 0x01, 0x00, 0x00, 0xc8, 0xd8, 0xa7, 0x68], // HH / SD
    [0x05, 0x01, 0x00, 0x00, 0xf8, 0xaa, 0x59, 0x55], // TOM / T-CT
];

/// MULTI in half steps (½, 1, 2, … 15).
const MULTI2: [u32; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30];
/// Key scale level base by the F-number's top 4 bits (0.75 dB steps).
const KSL_BASE: [u32; 16] = [
    0, 24, 32, 37, 40, 43, 45, 47, 48, 50, 51, 52, 53, 54, 55, 56,
];
/// Envelope increments by the rate's low two bits, over eight steps.
const EG_INC: [[u32; 8]; 4] = [
    [0, 1, 0, 1, 0, 1, 0, 1],
    [0, 1, 0, 1, 1, 1, 0, 1],
    [0, 1, 1, 1, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 1],
];
/// Vibrato steps (× F-number / 256).
const PM: [i32; 8] = [0, 1, 2, 1, 0, -1, -2, -1];
const ENV_MAX: u16 = 127;

struct Tables {
    /// −log2(sin) of the first quarter wave, × 256.
    logsin: [u16; 256],
    /// 2^(i/256) × 1024 − 1024.
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

/// One sine lookup: `idx` 10 bits a cycle, `att` in 0.375 dB steps.
fn wave(idx: u32, att: u32, half: bool) -> i32 {
    let t = tables();
    let i = idx & 1023;
    let neg = i & 512 != 0;
    if neg && half {
        return 0;
    }
    let q = if i & 256 != 0 {
        255 - (i & 255)
    } else {
        i & 255
    };
    let a = u32::from(t.logsin[q as usize]) + (att << 4);
    let shift = a >> 8;
    if shift > 12 {
        return 0;
    }
    let v = ((u32::from(t.exp[(!a & 255) as usize]) + 1024) << 1) >> shift;
    if neg {
        -(v as i32)
    } else {
        v as i32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum Eg {
    Attack,
    Decay,
    Sustain,
    Release,
    #[default]
    Off,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Op {
    phase: u32,
    env: u16,
    eg: Eg,
    /// The modulator's last two outputs (feedback).
    out: [i32; 2],
}

/// One operator's tone parameters, decoded from a patch.
#[derive(Clone, Copy)]
struct Tone {
    am: bool,
    vib: bool,
    sustained: bool,
    ksr: bool,
    multi: u8,
    ksl: u8,
    ar: u8,
    dr: u8,
    sl: u8,
    rr: u8,
}

fn tone(p: &[u8; 8], car: bool) -> Tone {
    let c = usize::from(car);
    Tone {
        am: p[c] & 0x80 != 0,
        vib: p[c] & 0x40 != 0,
        sustained: p[c] & 0x20 != 0,
        ksr: p[c] & 0x10 != 0,
        multi: p[c] & 15,
        ksl: p[2 + c] >> 6,
        ar: p[4 + c] >> 4,
        dr: p[4 + c] & 15,
        sl: p[6 + c] >> 4,
        rr: p[6 + c] & 15,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ym2413 {
    pub regs: [u8; 0x40],
    addr: u8,
    ops: [Op; 18],
    noise: u32,
    eg_counter: u32,
    lfo_counter: u32,
    clocks: u8,
    out: i32,
}

impl Default for Ym2413 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym2413 {
    pub fn new() -> Self {
        Self {
            regs: [0; 0x40],
            addr: 0,
            ops: [Op::default(); 18],
            noise: 1,
            eg_counter: 0,
            lfo_counter: 0,
            clocks: 0,
            out: 0,
        }
    }

    pub fn write_addr(&mut self, v: u8) {
        self.addr = v & 0x3f;
    }

    pub fn write_data(&mut self, v: u8) {
        let r = usize::from(self.addr);
        let keys_before = self.keys();
        self.regs[r] = v;
        let keys = self.keys();
        for (ch, (&was, &now)) in keys_before.iter().zip(&keys).enumerate() {
            for (op, (w, n)) in [(0, (was & 1, now & 1)), (1, (was & 2, now & 2))] {
                let o = &mut self.ops[ch * 2 + op];
                if n != 0 && w == 0 {
                    o.phase = 0;
                    o.eg = Eg::Attack;
                    o.out = [0; 2];
                } else if n == 0 && w != 0 && o.eg != Eg::Off {
                    o.eg = Eg::Release;
                }
            }
        }
    }

    fn rhythm(&self) -> bool {
        self.regs[0x0e] & 0x20 != 0
    }

    /// Per channel: bit 0 the modulator keyed, bit 1 the carrier.
    fn keys(&self) -> [u8; 9] {
        let mut k = [0u8; 9];
        for (ch, v) in k.iter_mut().enumerate() {
            if self.regs[0x20 + ch] & 0x10 != 0 {
                *v = 3;
            }
        }
        if self.rhythm() {
            let r = self.regs[0x0e];
            if r & 0x10 != 0 {
                k[6] = 3;
            }
            k[7] |= u8::from(r & 0x01 != 0) | u8::from(r & 0x08 != 0) << 1;
            k[8] |= u8::from(r & 0x04 != 0) | u8::from(r & 0x02 != 0) << 1;
        }
        k
    }

    fn patch(&self, ch: usize) -> [u8; 8] {
        if self.rhythm() && ch >= 6 {
            return ROM[16 + ch - 6];
        }
        match self.regs[0x30 + ch] >> 4 {
            0 => self.regs[..8].try_into().expect("8 bytes"),
            n => ROM[usize::from(n)],
        }
    }

    /// The last sample: the sum of the channels (about ±40000).
    pub fn output(&self) -> i32 {
        self.out
    }

    /// One input clock.
    pub fn tick(&mut self) {
        self.clocks += 1;
        if self.clocks == CLOCKS_PER_SAMPLE {
            self.clocks = 0;
            self.sample();
        }
    }

    fn sample(&mut self) {
        self.eg_counter = self.eg_counter.wrapping_add(1);
        self.lfo_counter = self.lfo_counter.wrapping_add(1);
        let fb = ((self.noise >> 22) ^ (self.noise >> 17)) & 1;
        self.noise = ((self.noise << 1) | fb) & 0x7f_ffff;
        // Tremolo: a triangle 0–13–0 (0.375 dB steps), 26 steps of 512
        // samples; vibrato: eight steps of 1024 samples.
        let am_step = (self.lfo_counter >> 9) % 26;
        let am = if am_step < 13 { am_step } else { 26 - am_step };
        let pm = PM[((self.lfo_counter >> 10) & 7) as usize];
        let rhythm = self.rhythm();
        let mut out = 0;
        let mut ph = [0u32; 18];
        for ch in 0..9 {
            let p = self.patch(ch);
            let fnum = u32::from(self.regs[0x10 + ch]) | u32::from(self.regs[0x20 + ch] & 1) << 8;
            let block = u32::from(self.regs[0x20 + ch] >> 1 & 7);
            let sus = self.regs[0x20 + ch] & 0x20 != 0;
            for op in 0..2 {
                let t = tone(&p, op == 1);
                let o = &mut self.ops[ch * 2 + op];
                let f = if t.vib {
                    (fnum as i32 + ((fnum as i32 * pm) >> 8)).max(0) as u32
                } else {
                    fnum
                };
                ph[ch * 2 + op] = o.phase >> 9;
                o.phase =
                    (o.phase + (((f << block) * MULTI2[usize::from(t.multi)]) >> 1)) & 0x7_ffff;
                let rks = ((block << 1) | (fnum >> 8)) >> if t.ksr { 0 } else { 2 };
                step_env(o, &t, sus, rks, self.eg_counter);
            }
        }
        for ch in 0..9 {
            if rhythm && ch >= 6 {
                continue;
            }
            out += self.channel(ch, &ph, am);
        }
        if rhythm {
            out += 2 * self.channel(6, &ph, am);
            out += 2 * self.rhythm_ops(&ph, am);
        }
        self.out = out;
    }

    /// The attenuation of operator `op` (0 modulator, 1 carrier) of `ch`
    /// in 0.375 dB steps; `level` is TL × 2 or volume × 8.
    fn attenuation(&self, ch: usize, op: usize, level: u32, am: u32) -> u32 {
        let p = self.patch(ch);
        let t = tone(&p, op == 1);
        let fnum = u32::from(self.regs[0x10 + ch]) | u32::from(self.regs[0x20 + ch] & 1) << 8;
        let block = u32::from(self.regs[0x20 + ch] >> 1 & 7);
        let base = (KSL_BASE[(fnum >> 5) as usize] * 2).saturating_sub(16 * (7 - block));
        let ksl = match t.ksl {
            0 => 0,
            1 => base / 4,
            2 => base / 2,
            _ => base,
        };
        u32::from(self.ops[ch * 2 + op].env) + level + ksl + if t.am { am } else { 0 }
    }

    /// A melody channel: the modulator with feedback into the carrier.
    fn channel(&mut self, ch: usize, ph: &[u32; 18], am: u32) -> i32 {
        let p = self.patch(ch);
        let fb = u32::from(p[3] & 7);
        let vol = u32::from(self.regs[0x30 + ch] & 15);
        let tl = u32::from(p[2] & 0x3f);
        let m_att = self.attenuation(ch, 0, tl * 2, am);
        let c_att = self.attenuation(ch, 1, vol * 8, am);
        let m = &mut self.ops[ch * 2];
        let feedback = if fb == 0 {
            0
        } else {
            (m.out[0] + m.out[1]) >> (9 - fb)
        };
        let mv = if m.eg == Eg::Off {
            0
        } else {
            wave(
                ph[ch * 2].wrapping_add(feedback as u32),
                m_att,
                p[3] & 0x08 != 0,
            )
        };
        m.out = [m.out[1], mv];
        if self.ops[ch * 2 + 1].eg == Eg::Off {
            return 0;
        }
        wave(
            ph[ch * 2 + 1].wrapping_add((mv >> 1) as u32),
            c_att,
            p[3] & 0x10 != 0,
        )
    }

    /// HH, SD, TOM, T-CT: single operators of channels 7 and 8.
    fn rhythm_ops(&mut self, ph: &[u32; 18], am: u32) -> i32 {
        let p7 = ph[14];
        let p8 = ph[17];
        let noise = self.noise & 1 != 0;
        let res = ((((p7 >> 2) ^ (p7 >> 7)) | (p7 >> 3)) & 1) | (((p8 >> 3) ^ (p8 >> 5)) & 1);
        let hh = {
            let mut phase = if res != 0 { 0x200 | (0xd0 >> 2) } else { 0xd0 };
            if noise {
                phase = if phase & 0x200 != 0 {
                    0x200 | 0xd0
                } else {
                    0xd0 >> 2
                };
            }
            phase
        };
        let sd = {
            let phase = if p7 & 0x100 != 0 { 0x200 } else { 0x100 };
            if noise {
                phase ^ 0x100
            } else {
                phase
            }
        };
        let tcy = if res != 0 { 0x300 } else { 0x100 };
        let vols = [
            self.regs[0x37] >> 4,
            self.regs[0x37] & 15,
            self.regs[0x38] >> 4,
            self.regs[0x38] & 15,
        ];
        let mut out = 0;
        for (i, (op, phase)) in [(14usize, hh), (15, sd), (16, ph[16]), (17, tcy)]
            .into_iter()
            .enumerate()
        {
            if self.ops[op].eg == Eg::Off {
                continue;
            }
            let att = self.attenuation(op / 2, op & 1, u32::from(vols[i]) * 8, am);
            out += wave(phase, att, false);
        }
        out
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"OPLL", 1, |w| {
            w.raw(&self.regs);
            w.u8(self.addr);
            for o in &self.ops {
                w.u32(o.phase);
                w.u16(o.env);
                w.u8(o.eg as u8);
                w.u32(o.out[0] as u32);
                w.u32(o.out[1] as u32);
            }
            w.u32(self.noise);
            w.u32(self.eg_counter);
            w.u32(self.lfo_counter);
            w.u8(self.clocks);
            w.u32(self.out as u32);
        });
    }

    /// Atomic: on an error nothing changed.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"OPLL", 1)?;
        let mut n = Self::new();
        c.raw_into(&mut n.regs)?;
        n.addr = c.u8()? & 0x3f;
        for o in &mut n.ops {
            o.phase = c.u32()? & 0x7_ffff;
            o.env = c.u16()?.min(ENV_MAX);
            o.eg = match c.u8()? {
                0 => Eg::Attack,
                1 => Eg::Decay,
                2 => Eg::Sustain,
                3 => Eg::Release,
                4 => Eg::Off,
                _ => return Err(StateError::Invalid("OPLL envelope state")),
            };
            o.out = [c.u32()? as i32, c.u32()? as i32];
        }
        n.noise = c.u32()? & 0x7f_ffff;
        n.eg_counter = c.u32()?;
        n.lfo_counter = c.u32()?;
        n.clocks = c.u8()? % CLOCKS_PER_SAMPLE;
        n.out = c.u32()? as i32;
        *self = n;
        Ok(())
    }
}

/// The envelope increment for `rate` (0–63) at `counter`.
fn eg_inc(rate: u32, counter: u32) -> u32 {
    if rate == 0 {
        return 0;
    }
    let (hi, lo) = (rate >> 2, (rate & 3) as usize);
    if hi < 13 {
        let shift = 13 - hi;
        if counter & ((1 << shift) - 1) != 0 {
            0
        } else {
            EG_INC[lo][((counter >> shift) & 7) as usize]
        }
    } else {
        EG_INC[lo][(counter & 7) as usize] << (hi - 12)
    }
}

fn rate(r: u8, rks: u32) -> u32 {
    if r == 0 {
        0
    } else {
        (u32::from(r) * 4 + rks).min(63)
    }
}

fn step_env(o: &mut Op, t: &Tone, sus: bool, rks: u32, counter: u32) {
    match o.eg {
        Eg::Attack => {
            if t.ar == 15 {
                o.env = 0;
            } else {
                let inc = eg_inc(rate(t.ar, rks), counter);
                if inc > 0 {
                    let d = ((u32::from(o.env) * inc) >> 2) + 1;
                    o.env = (u32::from(o.env).saturating_sub(d)) as u16;
                }
            }
            if o.env == 0 {
                o.eg = Eg::Decay;
            }
        }
        Eg::Decay => {
            let sl = u16::from(t.sl) * 8;
            o.env = (o.env + eg_inc(rate(t.dr, rks), counter) as u16).min(ENV_MAX);
            if o.env >= sl {
                o.env = sl.min(ENV_MAX);
                o.eg = Eg::Sustain;
            }
        }
        Eg::Sustain => {
            if !t.sustained {
                o.env = (o.env + eg_inc(rate(t.rr, rks), counter) as u16).min(ENV_MAX);
            }
        }
        Eg::Release => {
            let r = if sus {
                5
            } else if t.sustained {
                t.rr
            } else {
                7
            };
            o.env = (o.env + eg_inc(rate(r, rks), counter) as u16).min(ENV_MAX);
            if o.env >= ENV_MAX {
                o.eg = Eg::Off;
            }
        }
        Eg::Off => o.env = ENV_MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(c: &mut Ym2413, r: u8, v: u8) {
        c.write_addr(r);
        c.write_data(v);
    }

    fn samples(c: &mut Ym2413, n: usize) -> Vec<i32> {
        (0..n)
            .map(|_| {
                for _ in 0..CLOCKS_PER_SAMPLE {
                    c.tick();
                }
                c.output()
            })
            .collect()
    }

    /// Zero crossings upward in `s`.
    fn crossings(s: &[i32]) -> usize {
        s.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count()
    }

    #[test]
    fn the_sine_table_peaks_at_full_scale() {
        assert_eq!(wave(256, 0, false), 4084);
        assert_eq!(wave(768, 0, false), -4084);
        assert_eq!(wave(768, 0, true), 0, "half sine");
        assert!(wave(256, 16, false) < 2100, "6 dB = 16 steps halves it");
    }

    #[test]
    fn a_keyed_flute_sounds_at_its_frequency_and_releases() {
        let mut c = Ym2413::new();
        // A440: fnum 288, block 4 (49716 × 288 × 2^3 / 2^18 ≈ 437 Hz).
        write(&mut c, 0x10, 288u16 as u8);
        write(&mut c, 0x30, 0x40);
        write(&mut c, 0x20, 0x10 | 4 << 1 | 1);
        let s = samples(&mut c, 49716 / 2);
        let n = crossings(&s[s.len() / 2..]);
        assert!((100..=120).contains(&n), "≈437 Hz over 0.25 s: {n}");
        assert!(s.iter().map(|v| v.abs()).max().unwrap() > 1000);
        write(&mut c, 0x20, 4 << 1 | 1);
        let s = samples(&mut c, 49716 * 3);
        assert!(
            s[s.len() - 1000..].iter().all(|&v| v.abs() < 50),
            "released"
        );
    }

    #[test]
    fn rhythm_mode_plays_the_bass_drum_and_hats() {
        let mut c = Ym2413::new();
        write(&mut c, 0x16, 0x20);
        write(&mut c, 0x26, 0x05);
        write(&mut c, 0x17, 0x50);
        write(&mut c, 0x27, 0x05);
        write(&mut c, 0x18, 0xc0);
        write(&mut c, 0x28, 0x01);
        write(&mut c, 0x0e, 0x20);
        assert!(samples(&mut c, 1000).iter().all(|&v| v == 0), "quiet");
        write(&mut c, 0x0e, 0x31);
        let s = samples(&mut c, 4000);
        assert!(s.iter().any(|v| v.abs() > 1000), "BD and HH sound");
    }

    #[test]
    fn savestates_round_trip() {
        let mut a = Ym2413::new();
        write(&mut a, 0x10, 0x80);
        write(&mut a, 0x30, 0x13);
        write(&mut a, 0x20, 0x18);
        samples(&mut a, 500);
        let mut w = StateWriter::new();
        a.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Ym2413::new();
        b.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(samples(&mut a, 300), samples(&mut b, 300));
        assert!(b.load(&mut StateReader::new(&bytes[..20])).is_err());
    }
}
