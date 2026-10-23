//! The SNES's S-DSP: 8 BRR sample voices with Gaussian interpolation,
//! ADSR/GAIN envelopes, noise and pitch modulation, and the echo unit
//! (ring buffer in sound RAM, 8-tap FIR filter, feedback), mixed to a
//! stereo 32 kHz output.
//!
//! **Registers** (128, through the S-SMP's $F2/$F3): per voice x = 0–7 at
//! x0 VOLL, x1 VOLR, x2/x3 PITCH (14 bits, $1000 = 32 kHz), x4 SRCN, x5/x6
//! ADSR, x7 GAIN, x8 ENVX, x9 OUTX (read), xF FIRx; globals $0C/$1C MVOL,
//! $2C/$3C EVOL, $4C KON, $5C KOFF, $6C FLG (soft reset, mute, echo write
//! off, noise rate), $7C ENDX (any write clears it), $0D EFB, $2D PMON, $3D
//! NON, $4D EON, $5D DIR, $6D ESA, $7D EDL. $80–$FF read as $00–$7F.
//!
//! [`SDsp::sample`] produces one stereo sample from the sound RAM (it runs
//! every 32 S-SMP cycles). KON and KOFF are acted on every second sample;
//! a keyed-on voice starts 5 samples later. Rates (envelopes, noise) are
//! periods in samples from a free-running counter.
//!
//! Not emulated: the per-cycle order of the DSP's RAM and register
//! accesses within a sample (a voice reads its registers once a sample),
//! the delayed take-over of ESA/EDL changes.
//!
//! Sources: Martin Korth, fullsnes, "SNES APU DSP BRR Samples", "… BRR
//! Pitch" (with the Gauss table, a hardware ROM table reproduced from it),
//! "… ADSR/Gain Envelope", "… Volume Registers", "… Control Registers",
//! "… Echo Registers", "SNES APU Low Level Timings" (read 2026-10-23). The
//! implementation is this project's own.

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

/// Gaussian interpolation weights (fullsnes, "SNES APU DSP BRR Pitch").
#[rustfmt::skip]
const GAUSS: [i32; 512] = [
    0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000, 0x000,
    0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x001, 0x002, 0x002, 0x002, 0x002, 0x002,
    0x002, 0x002, 0x003, 0x003, 0x003, 0x003, 0x003, 0x004, 0x004, 0x004, 0x004, 0x004, 0x005, 0x005, 0x005, 0x005,
    0x006, 0x006, 0x006, 0x006, 0x007, 0x007, 0x007, 0x008, 0x008, 0x008, 0x009, 0x009, 0x009, 0x00a, 0x00a, 0x00a,
    0x00b, 0x00b, 0x00b, 0x00c, 0x00c, 0x00d, 0x00d, 0x00e, 0x00e, 0x00f, 0x00f, 0x00f, 0x010, 0x010, 0x011, 0x011,
    0x012, 0x013, 0x013, 0x014, 0x014, 0x015, 0x015, 0x016, 0x017, 0x017, 0x018, 0x018, 0x019, 0x01a, 0x01b, 0x01b,
    0x01c, 0x01d, 0x01d, 0x01e, 0x01f, 0x020, 0x020, 0x021, 0x022, 0x023, 0x024, 0x024, 0x025, 0x026, 0x027, 0x028,
    0x029, 0x02a, 0x02b, 0x02c, 0x02d, 0x02e, 0x02f, 0x030, 0x031, 0x032, 0x033, 0x034, 0x035, 0x036, 0x037, 0x038,
    0x03a, 0x03b, 0x03c, 0x03d, 0x03e, 0x040, 0x041, 0x042, 0x043, 0x045, 0x046, 0x047, 0x049, 0x04a, 0x04c, 0x04d,
    0x04e, 0x050, 0x051, 0x053, 0x054, 0x056, 0x057, 0x059, 0x05a, 0x05c, 0x05e, 0x05f, 0x061, 0x063, 0x064, 0x066,
    0x068, 0x06a, 0x06b, 0x06d, 0x06f, 0x071, 0x073, 0x075, 0x076, 0x078, 0x07a, 0x07c, 0x07e, 0x080, 0x082, 0x084,
    0x086, 0x089, 0x08b, 0x08d, 0x08f, 0x091, 0x093, 0x096, 0x098, 0x09a, 0x09c, 0x09f, 0x0a1, 0x0a3, 0x0a6, 0x0a8,
    0x0ab, 0x0ad, 0x0af, 0x0b2, 0x0b4, 0x0b7, 0x0ba, 0x0bc, 0x0bf, 0x0c1, 0x0c4, 0x0c7, 0x0c9, 0x0cc, 0x0cf, 0x0d2,
    0x0d4, 0x0d7, 0x0da, 0x0dd, 0x0e0, 0x0e3, 0x0e6, 0x0e9, 0x0ec, 0x0ef, 0x0f2, 0x0f5, 0x0f8, 0x0fb, 0x0fe, 0x101,
    0x104, 0x107, 0x10b, 0x10e, 0x111, 0x114, 0x118, 0x11b, 0x11e, 0x122, 0x125, 0x129, 0x12c, 0x130, 0x133, 0x137,
    0x13a, 0x13e, 0x141, 0x145, 0x148, 0x14c, 0x150, 0x153, 0x157, 0x15b, 0x15f, 0x162, 0x166, 0x16a, 0x16e, 0x172,
    0x176, 0x17a, 0x17d, 0x181, 0x185, 0x189, 0x18d, 0x191, 0x195, 0x19a, 0x19e, 0x1a2, 0x1a6, 0x1aa, 0x1ae, 0x1b2,
    0x1b7, 0x1bb, 0x1bf, 0x1c3, 0x1c8, 0x1cc, 0x1d0, 0x1d5, 0x1d9, 0x1dd, 0x1e2, 0x1e6, 0x1eb, 0x1ef, 0x1f3, 0x1f8,
    0x1fc, 0x201, 0x205, 0x20a, 0x20f, 0x213, 0x218, 0x21c, 0x221, 0x226, 0x22a, 0x22f, 0x233, 0x238, 0x23d, 0x241,
    0x246, 0x24b, 0x250, 0x254, 0x259, 0x25e, 0x263, 0x267, 0x26c, 0x271, 0x276, 0x27b, 0x280, 0x284, 0x289, 0x28e,
    0x293, 0x298, 0x29d, 0x2a2, 0x2a6, 0x2ab, 0x2b0, 0x2b5, 0x2ba, 0x2bf, 0x2c4, 0x2c9, 0x2ce, 0x2d3, 0x2d8, 0x2dc,
    0x2e1, 0x2e6, 0x2eb, 0x2f0, 0x2f5, 0x2fa, 0x2ff, 0x304, 0x309, 0x30e, 0x313, 0x318, 0x31d, 0x322, 0x326, 0x32b,
    0x330, 0x335, 0x33a, 0x33f, 0x344, 0x349, 0x34e, 0x353, 0x357, 0x35c, 0x361, 0x366, 0x36b, 0x370, 0x374, 0x379,
    0x37e, 0x383, 0x388, 0x38c, 0x391, 0x396, 0x39b, 0x39f, 0x3a4, 0x3a9, 0x3ad, 0x3b2, 0x3b7, 0x3bb, 0x3c0, 0x3c5,
    0x3c9, 0x3ce, 0x3d2, 0x3d7, 0x3dc, 0x3e0, 0x3e5, 0x3e9, 0x3ed, 0x3f2, 0x3f6, 0x3fb, 0x3ff, 0x403, 0x408, 0x40c,
    0x410, 0x415, 0x419, 0x41d, 0x421, 0x425, 0x42a, 0x42e, 0x432, 0x436, 0x43a, 0x43e, 0x442, 0x446, 0x44a, 0x44e,
    0x452, 0x455, 0x459, 0x45d, 0x461, 0x465, 0x468, 0x46c, 0x470, 0x473, 0x477, 0x47a, 0x47e, 0x481, 0x485, 0x488,
    0x48c, 0x48f, 0x492, 0x496, 0x499, 0x49c, 0x49f, 0x4a2, 0x4a6, 0x4a9, 0x4ac, 0x4af, 0x4b2, 0x4b5, 0x4b7, 0x4ba,
    0x4bd, 0x4c0, 0x4c3, 0x4c5, 0x4c8, 0x4cb, 0x4cd, 0x4d0, 0x4d2, 0x4d5, 0x4d7, 0x4d9, 0x4dc, 0x4de, 0x4e0, 0x4e3,
    0x4e5, 0x4e7, 0x4e9, 0x4eb, 0x4ed, 0x4ef, 0x4f1, 0x4f3, 0x4f5, 0x4f6, 0x4f8, 0x4fa, 0x4fb, 0x4fd, 0x4ff, 0x500,
    0x502, 0x503, 0x504, 0x506, 0x507, 0x508, 0x50a, 0x50b, 0x50c, 0x50d, 0x50e, 0x50f, 0x510, 0x511, 0x511, 0x512,
    0x513, 0x514, 0x514, 0x515, 0x516, 0x516, 0x517, 0x517, 0x517, 0x518, 0x518, 0x518, 0x518, 0x518, 0x519, 0x519,
];

/// Samples between envelope/noise steps for the 32 rates (0 = never).
const RATES: [u16; 32] = [
    0, 2048, 1536, 1280, 1024, 768, 640, 512, 384, 320, 256, 192, 160, 128, 96, 80, 64, 48, 40, 32,
    24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2, 1,
];

/// The rate counter wraps at a common multiple of all periods.
const COUNTER_WRAP: u16 = 30720;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Env {
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Voice {
    /// Decoded samples: 3 of the previous block, then this block's 16.
    buf: [i16; 19],
    /// Address of the current BRR block.
    addr: u16,
    /// The current block's header.
    header: u8,
    /// Pitch counter: bits 12–15 the sample in the block, 4–11 the
    /// interpolation index.
    counter: u16,
    env: Env,
    level: i32,
    /// Samples until a keyed-on voice starts (5 … 1), 0 = running.
    kon_delay: u8,
    /// Last output (15 bits) for OUTX and pitch modulation.
    out: i32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            buf: [0; 19],
            addr: 0,
            header: 0,
            counter: 0,
            env: Env::Release,
            level: 0,
            kon_delay: 0,
            out: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SDsp {
    pub regs: [u8; 128],
    voices: [Voice; 8],
    /// KON written since the last poll.
    kon: u8,
    endx: u8,
    /// KON/KOFF are polled every second sample.
    even: bool,
    counter: u16,
    noise: i32,
    /// Echo: FIR history per channel, position in the ring buffer.
    fir: [[i32; 8]; 2],
    fir_pos: usize,
    echo_pos: u16,
    echo_len: u16,
}

impl Default for SDsp {
    fn default() -> Self {
        Self::new()
    }
}

fn clamp16(v: i32) -> i32 {
    v.clamp(-0x8000, 0x7fff)
}

impl SDsp {
    pub fn new() -> Self {
        let mut regs = [0; 128];
        regs[0x6c] = 0xe0;
        Self {
            regs,
            voices: Default::default(),
            kon: 0,
            endx: 0,
            even: false,
            counter: 0,
            noise: 0x4000,
            fir: [[0; 8]; 2],
            fir_pos: 0,
            echo_pos: 0,
            echo_len: 0,
        }
    }

    /// The reset line: FLG = $E0 (all voices off, muted, echo writes off).
    pub fn reset(&mut self) {
        self.regs[0x6c] = 0xe0;
        self.endx = 0;
        self.kon = 0;
        for v in &mut self.voices {
            v.env = Env::Release;
            v.level = 0;
            v.out = 0;
        }
    }

    pub fn read(&self, reg: u8) -> u8 {
        let r = usize::from(reg & 0x7f);
        match r {
            0x7c => self.endx,
            _ if r & 0x0f == 0x08 => (self.voices[r >> 4].level >> 4) as u8,
            _ if r & 0x0f == 0x09 => (self.voices[r >> 4].out >> 7) as u8,
            _ => self.regs[r],
        }
    }

    /// A write; registers $80–$FF are read-only.
    pub fn write(&mut self, reg: u8, v: u8) {
        if reg & 0x80 != 0 {
            return;
        }
        match reg {
            0x4c => self.kon = v,
            0x7c => self.endx = 0,
            _ => {}
        }
        self.regs[usize::from(reg)] = v;
    }

    fn vreg(&self, voice: usize, r: usize) -> u8 {
        self.regs[voice << 4 | r]
    }

    fn rate_due(&self, rate: u8) -> bool {
        let period = RATES[usize::from(rate & 31)];
        period != 0 && self.counter.is_multiple_of(period)
    }

    /// Decode a BRR block at `addr` into the voice's buffer (keeping the
    /// last three samples as history).
    fn decode(v: &mut Voice, ram: &[u8; 0x10000]) {
        let a = usize::from(v.addr);
        let header = ram[a];
        v.header = header;
        let shift = header >> 4;
        let filter = (header >> 2) & 3;
        let hist = [v.buf[16], v.buf[17], v.buf[18]];
        v.buf[..3].copy_from_slice(&hist);
        for i in 0..16 {
            let byte = ram[(a + 1 + i / 2) & 0xffff];
            let nibble = if i & 1 == 0 { byte >> 4 } else { byte & 0x0f };
            let n = i32::from((nibble << 4) as i8 >> 4);
            let mut s = if shift <= 12 {
                (n << shift) >> 1
            } else {
                (n >> 3) << 11
            };
            let old = i32::from(v.buf[i + 2]);
            let older = i32::from(v.buf[i + 1]);
            s += match filter {
                0 => 0,
                1 => old + ((-old) >> 4),
                2 => old * 2 + ((-old * 3) >> 5) - older + (older >> 4),
                _ => old * 2 + ((-old * 13) >> 6) - older + ((older * 3) >> 4),
            };
            let s = clamp16(s);
            // Keep 15 bits (the sign of bit 14 wins).
            v.buf[i + 3] = ((s as i16) << 1) >> 1;
        }
    }

    fn interpolate(v: &Voice) -> i32 {
        let pos = usize::from(v.counter >> 12) + 3;
        let i = usize::from((v.counter >> 4) & 0xff);
        let s = |k: usize| i32::from(v.buf[pos - k]);
        let mut out = (GAUSS[0xff - i] * s(3)) >> 10;
        out += (GAUSS[0x1ff - i] * s(2)) >> 10;
        out += (GAUSS[0x100 + i] * s(1)) >> 10;
        let out = i32::from(out as i16);
        let out = clamp16(out + ((GAUSS[i] * s(0)) >> 10));
        out >> 1
    }

    fn envelope(&mut self, n: usize) {
        let adsr1 = self.vreg(n, 5);
        let adsr2 = self.vreg(n, 6);
        let gain = self.vreg(n, 7);
        let v = &self.voices[n];
        let mut level = v.level;
        let mut env = v.env;
        if env == Env::Release {
            level -= 8;
        } else if adsr1 & 0x80 != 0 {
            let (rate, step) = match env {
                Env::Attack => {
                    let rate = (adsr1 & 0x0f) * 2 + 1;
                    (rate, if rate == 31 { 1024 } else { 32 })
                }
                Env::Decay => (((adsr1 >> 4) & 7) * 2 + 16, -(((level - 1) >> 8) + 1)),
                _ => (adsr2 & 0x1f, -(((level - 1) >> 8) + 1)),
            };
            if self.rate_due(rate) {
                level += step;
            }
        } else if gain & 0x80 == 0 {
            level = i32::from(gain & 0x7f) * 16;
        } else if self.rate_due(gain & 0x1f) {
            level += match (gain >> 5) & 3 {
                0 => -32,
                1 => -(((level - 1) >> 8) + 1),
                2 => 32,
                _ => {
                    if level < 0x600 {
                        32
                    } else {
                        8
                    }
                }
            };
        }
        if env == Env::Attack && level >= 0x7e0 {
            env = Env::Decay;
        }
        level = level.clamp(0, 0x7ff);
        if env == Env::Decay && level <= (i32::from(adsr2 >> 5) + 1) * 0x100 {
            env = Env::Sustain;
        }
        let v = &mut self.voices[n];
        v.level = level;
        v.env = env;
    }

    /// One 32 kHz stereo sample; `ram` is the sound RAM (the echo buffer
    /// is written into it).
    pub fn sample(&mut self, ram: &mut [u8; 0x10000]) -> (i16, i16) {
        self.counter = if self.counter == 0 {
            COUNTER_WRAP - 1
        } else {
            self.counter - 1
        };
        let flg = self.regs[0x6c];
        self.even = !self.even;
        if self.even {
            let koff = self.regs[0x5c];
            let kon = std::mem::take(&mut self.kon);
            for (n, v) in self.voices.iter_mut().enumerate() {
                let bit = 1 << n;
                if koff & bit != 0 {
                    v.env = Env::Release;
                }
                if kon & bit != 0 {
                    v.kon_delay = 5;
                    v.env = Env::Attack;
                    v.level = 0;
                    self.endx &= !bit;
                }
            }
        }
        if flg & 0x80 != 0 {
            for v in &mut self.voices {
                v.env = Env::Release;
                v.level = 0;
            }
        }
        if self.rate_due(flg & 0x1f) {
            let n = self.noise;
            self.noise = ((n >> 1) & 0x3fff) | (((n ^ (n >> 1)) & 1) << 14);
        }
        let noise = (((self.noise << 1) as i16) >> 1) as i32;

        let dir = u16::from(self.regs[0x5d]) << 8;
        let (pmon, non, eon) = (self.regs[0x2d], self.regs[0x3d], self.regs[0x4d]);
        let (mut left, mut right, mut echo_l, mut echo_r) = (0i32, 0i32, 0i32, 0i32);
        let mut prev_out = 0;
        for n in 0..8 {
            let bit = 1u8 << n;
            let srcn = u16::from(self.vreg(n, 4));
            let entry = usize::from(dir.wrapping_add(srcn * 4));
            if self.voices[n].kon_delay > 0 {
                let v = &mut self.voices[n];
                v.kon_delay -= 1;
                if v.kon_delay == 4 {
                    v.addr = u16::from_le_bytes([ram[entry], ram[(entry + 1) & 0xffff]]);
                    v.counter = 0;
                    v.buf = [0; 19];
                    Self::decode(v, ram);
                    if v.header & 1 != 0 {
                        self.endx |= bit;
                    }
                }
                v.out = 0;
                prev_out = 0;
                continue;
            }
            self.envelope(n);
            let pitch = i32::from(u16::from_le_bytes([
                self.vreg(n, 2),
                self.vreg(n, 3) & 0x3f,
            ]));
            let step = if pmon & bit != 0 && n > 0 {
                (pitch * ((prev_out >> 4) + 0x400)) >> 10
            } else {
                pitch
            };
            let sample = if non & bit != 0 {
                noise
            } else {
                Self::interpolate(&self.voices[n])
            };
            let v = &mut self.voices[n];
            let out = ((sample * v.level) >> 11) & !1;
            v.out = out;
            prev_out = out;
            let next = u32::from(v.counter) + step.clamp(0, 0x7fff) as u32;
            v.counter = next as u16;
            if next > 0xffff {
                // The block ended: on to the next one, or the loop.
                if v.header & 1 != 0 {
                    v.addr =
                        u16::from_le_bytes([ram[(entry + 2) & 0xffff], ram[(entry + 3) & 0xffff]]);
                    if v.header & 2 == 0 {
                        v.env = Env::Release;
                        v.level = 0;
                    }
                } else {
                    v.addr = v.addr.wrapping_add(9);
                }
                Self::decode(v, ram);
                if v.header & 1 != 0 {
                    self.endx |= bit;
                }
            }
            let vol = |r: u8| i32::from(r as i8);
            let l = (out * vol(self.vreg(n, 0))) >> 6;
            let r = (out * vol(self.vreg(n, 1))) >> 6;
            left = clamp16(left + l);
            right = clamp16(right + r);
            if eon & bit != 0 {
                echo_l = clamp16(echo_l + l);
                echo_r = clamp16(echo_r + r);
            }
        }

        // Echo: read the oldest entry, filter, feed back.
        let base = u16::from(self.regs[0x6d]) << 8;
        let addr = base.wrapping_add(self.echo_pos.wrapping_mul(4));
        let rd = |a: u16| -> i32 {
            let lo = ram[usize::from(a)];
            let hi = ram[usize::from(a.wrapping_add(1))];
            i32::from(i16::from_le_bytes([lo, hi])) >> 1
        };
        self.fir[0][self.fir_pos] = rd(addr);
        self.fir[1][self.fir_pos] = rd(addr.wrapping_add(2));
        let mut fir_out = [0i32; 2];
        for (ch, out) in fir_out.iter_mut().enumerate() {
            let mut sum = 0i32;
            for tap in 0..7 {
                let idx = (self.fir_pos + 1 + tap) & 7;
                let c = i32::from(self.regs[tap << 4 | 0x0f] as i8);
                sum += (self.fir[ch][idx] * c) >> 6;
            }
            let c7 = i32::from(self.regs[0x7f] as i8);
            sum = i32::from(sum as i16) + ((self.fir[ch][self.fir_pos] * c7) >> 6);
            *out = clamp16(sum);
        }
        self.fir_pos = (self.fir_pos + 1) & 7;
        let vol = |r: usize| i32::from(self.regs[r] as i8);
        let out_l = clamp16(((left * vol(0x0c)) >> 7) + ((fir_out[0] * vol(0x2c)) >> 7));
        let out_r = clamp16(((right * vol(0x1c)) >> 7) + ((fir_out[1] * vol(0x3c)) >> 7));
        if flg & 0x20 == 0 {
            let efb = vol(0x0d);
            for (ch, input) in [echo_l, echo_r].into_iter().enumerate() {
                let e = (clamp16(input + ((fir_out[ch] * efb) >> 7)) & !1) as i16;
                let a = usize::from(addr.wrapping_add(2 * ch as u16));
                let [lo, hi] = e.to_le_bytes();
                ram[a] = lo;
                ram[(a + 1) & 0xffff] = hi;
            }
        }
        if self.echo_pos == 0 {
            self.echo_len = u16::from(self.regs[0x7d] & 0x0f) << 9;
        }
        self.echo_pos += 1;
        if self.echo_pos >= self.echo_len.max(1) {
            self.echo_pos = 0;
        }

        if flg & 0x40 != 0 {
            (0, 0)
        } else {
            (out_l as i16, out_r as i16)
        }
    }
}

const STATE_TAG: [u8; 4] = *b"SDSP";

impl Savestate for SDsp {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.raw(&self.regs);
            for v in &self.voices {
                for s in v.buf {
                    w.u16(s as u16);
                }
                w.u16(v.addr);
                w.u8(v.header);
                w.u16(v.counter);
                w.u8(match v.env {
                    Env::Attack => 0,
                    Env::Decay => 1,
                    Env::Sustain => 2,
                    Env::Release => 3,
                });
                w.u16(v.level as u16);
                w.u8(v.kon_delay);
                w.u32(v.out as u32);
            }
            w.u8(self.kon);
            w.u8(self.endx);
            w.bool(self.even);
            w.u16(self.counter);
            w.u16(self.noise as u16);
            for ch in &self.fir {
                for s in ch {
                    w.u32(*s as u32);
                }
            }
            w.u8(self.fir_pos as u8);
            w.u16(self.echo_pos);
            w.u16(self.echo_len);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut d = self.clone();
        c.raw_into(&mut d.regs)?;
        for v in &mut d.voices {
            for s in &mut v.buf {
                *s = c.u16()? as i16;
            }
            v.addr = c.u16()?;
            v.header = c.u8()?;
            v.counter = c.u16()?;
            v.env = match c.u8()? {
                0 => Env::Attack,
                1 => Env::Decay,
                2 => Env::Sustain,
                3 => Env::Release,
                _ => return Err(StateError::Invalid("S-DSP envelope")),
            };
            v.level = i32::from(c.u16()?).min(0x7ff);
            v.kon_delay = c.u8()?.min(5);
            v.out = c.u32()? as i32;
        }
        d.kon = c.u8()?;
        d.endx = c.u8()?;
        d.even = c.bool()?;
        d.counter = c.u16()? % COUNTER_WRAP;
        d.noise = i32::from(c.u16()? & 0x7fff);
        for ch in &mut d.fir {
            for s in ch.iter_mut() {
                *s = c.u32()? as i32;
            }
        }
        d.fir_pos = usize::from(c.u8()? & 7);
        d.echo_pos = c.u16()?;
        d.echo_len = c.u16()?;
        *self = d;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A looping BRR square wave at $1000, the DIR table at $0200.
    fn setup() -> (SDsp, Box<[u8; 0x10000]>) {
        let mut ram = Box::new([0u8; 0x10000]);
        ram[0x200..0x204].copy_from_slice(&[0x00, 0x10, 0x00, 0x10]);
        // One block: shift 12, filter 0, end+loop; nibbles +7 then -7.
        ram[0x1000] = 0xc3;
        for i in 0..8 {
            ram[0x1001 + i] = if i < 4 { 0x77 } else { 0x99 };
        }
        let mut d = SDsp::new();
        for (r, v) in [
            (0x5d, 0x02),
            (0x00, 0x7f),
            (0x01, 0x7f),
            (0x02, 0x00),
            (0x03, 0x10),
            (0x04, 0x00),
            (0x05, 0x00),
            (0x07, 0x7f),
            (0x0c, 0x7f),
            (0x1c, 0x7f),
            (0x6c, 0x20),
            (0x6d, 0x80),
        ] {
            d.write(r, v);
        }
        (d, ram)
    }

    #[test]
    fn a_keyed_on_voice_plays_its_sample() {
        let (mut d, mut ram) = setup();
        for _ in 0..20 {
            assert_eq!(d.sample(&mut ram), (0, 0), "silent before KON");
        }
        d.write(0x4c, 0x01);
        let out: Vec<(i16, i16)> = (0..200).map(|_| d.sample(&mut ram)).collect();
        let max = out.iter().map(|s| s.0).max().unwrap();
        let min = out.iter().map(|s| s.0).min().unwrap();
        assert!(max > 4000 && min < -4000, "square wave: {min}..{max}");
        assert_eq!(d.read(0x7c) & 1, 1, "ENDX on the end+loop block");
        assert_eq!(d.read(0x08), 0x7f, "direct gain $7F");
        d.write(0x7c, 0);
        assert_eq!(d.read(0x7c), 0, "a write clears ENDX");
        // Key off: release to silence.
        d.write(0x5c, 0x01);
        for _ in 0..400 {
            d.sample(&mut ram);
        }
        assert_eq!(d.read(0x08), 0);
        assert!(d.sample(&mut ram).0.abs() < 16);
    }

    #[test]
    fn mute_and_soft_reset() {
        let (mut d, mut ram) = setup();
        d.write(0x4c, 0x01);
        for _ in 0..50 {
            d.sample(&mut ram);
        }
        d.write(0x6c, 0x60);
        assert_eq!(d.sample(&mut ram), (0, 0), "muted");
        d.write(0x6c, 0xa0);
        d.sample(&mut ram);
        assert_eq!(d.read(0x08), 0, "soft reset: envelope 0");
    }

    #[test]
    fn savestate_round_trip() {
        let (mut d, mut ram) = setup();
        d.write(0x4c, 0x01);
        for _ in 0..77 {
            d.sample(&mut ram);
        }
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        let mut e = SDsp::new();
        e.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(e, d);
        let mut ram2 = ram.clone();
        for _ in 0..100 {
            assert_eq!(d.sample(&mut ram), e.sample(&mut ram2));
        }
    }
}
