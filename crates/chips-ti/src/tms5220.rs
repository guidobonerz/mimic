//! TMS5220 voice synthesis processor with its TMS6100 voice synthesis
//! memory (VSM): the TI-99/4A Speech Synthesizer.
//!
//! **Interface** (data bus bit 7 = TI's D0): a write is a command, or
//! during Speak External a byte for the 16-byte FIFO. Commands (bits 6–4):
//! `001` Read Byte, `011` Read and Branch, `100` Load Address (bits 3–0 one
//! nibble of the VSM address, the low nibble first), `101` Speak, `110`
//! Speak External, `111` Reset; the rest do nothing. A read returns the
//! byte of a Read Byte, otherwise the status: bit 7 TS (talking), bit 6 BL
//! (FIFO under half full), bit 5 BE (FIFO empty).
//!
//! **VSM**: bits come out of each byte least significant first. Read Byte
//! takes the next eight bits, the first one into bit 7 of the result;
//! Read and Branch loads the address from the next two bytes. The address
//! counts on across bytes.
//!
//! **Speech data**: frames of 25 ms (200 samples at 8 kHz), parameters most
//! significant bit first: energy 4 bits (0 silence, 15 stop), repeat 1,
//! pitch 6 (0 unvoiced), then unless repeat K1–K4 (5, 5, 4, 4 bits) and,
//! voiced, K5–K10 (4, 4, 4, 3, 3, 3); unvoiced frames clear K5–K10. The
//! coded values index the chip's look-up tables ([`ENERGY`], [`PITCH`],
//! [`K1`] … [`K10`]). A frame is interpolated towards in eight steps.
//!
//! **Synthesis** (this project's model where the data manual gives no
//! detail): the excitation is a pulse every `pitch` samples (voiced) or
//! noise from a 13-bit shift register (unvoiced), scaled by the energy; a
//! ten-stage lattice filter with K1–K10 shapes it. The interpolation is
//! linear and does not happen across a change of voicing or out of
//! silence. The real chip's voiced excitation (a short chirp) and its
//! interpolation shifts are not in the data manual; the sound is close,
//! not identical.
//!
//! Source: Texas Instruments, *TMS 5220 Voice Synthesis Processor Data
//! Manual* (1981): the I/O structure, the commands (Table 2), the status
//! bits, the FIFO, the frame format (Table 3, Figure 5) and the TMS5220
//! coding table (energy, pitch, K1–K10). The implementation is this
//! project's own.

use emu_core::{StateError, StateReader, StateWriter};

/// Samples per second.
pub const SAMPLE_RATE: u32 = 8000;
/// Samples per frame and per interpolation step.
const FRAME: u32 = 200;
const STEPS: u32 = 8;

/// The coding table (data manual, appendix): energy (RMS).
pub const ENERGY: [u16; 16] = [
    0, 52, 87, 123, 174, 246, 348, 491, 694, 981, 1385, 1957, 2764, 3904, 5514, 7789,
];
/// Pitch periods in samples.
pub const PITCH: [u8; 64] = [
    0, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    38, 39, 40, 41, 42, 44, 46, 48, 50, 52, 53, 56, 58, 60, 62, 65, 68, 70, 72, 76, 78, 80, 84, 86,
    91, 94, 98, 101, 105, 109, 114, 118, 122, 127, 132, 137, 142, 148, 153, 159,
];
pub const K1: [f32; 32] = [
    -0.97850, -0.97270, -0.97070, -0.96680, -0.96290, -0.95900, -0.95310, -0.94140, -0.93360,
    -0.92580, -0.91600, -0.90620, -0.89650, -0.88280, -0.86910, -0.85350, -0.80420, -0.74058,
    -0.66019, -0.56116, -0.44296, -0.30706, -0.15735, -0.00005, 0.15725, 0.30696, 0.44288, 0.56109,
    0.66013, 0.74054, 0.80416, 0.85350,
];
pub const K2: [f32; 32] = [
    -0.64000, -0.58999, -0.53500, -0.47507, -0.41039, -0.34129, -0.26830, -0.19209, -0.11350,
    -0.03345, 0.04702, 0.12690, 0.20515, 0.28087, 0.35325, 0.42163, 0.48553, 0.54464, 0.59878,
    0.64796, 0.69227, 0.73190, 0.76714, 0.79828, 0.82567, 0.84965, 0.87057, 0.88875, 0.90451,
    0.91813, 0.92988, 0.98830,
];
pub const K3: [f32; 16] = [
    -0.86000, -0.75467, -0.64933, -0.54400, -0.43867, -0.33333, -0.22800, -0.12267, -0.01733,
    0.08800, 0.19333, 0.29867, 0.40400, 0.50933, 0.61467, 0.72000,
];
pub const K4: [f32; 16] = [
    -0.64000, -0.53145, -0.42289, -0.31434, -0.20579, -0.09723, 0.01132, 0.11987, 0.22843, 0.33698,
    0.44553, 0.55409, 0.66264, 0.77119, 0.87975, 0.98830,
];
pub const K5: [f32; 16] = [
    -0.64000, -0.54933, -0.45867, -0.36800, -0.27733, -0.18667, -0.09600, -0.00533, 0.08533,
    0.17600, 0.26667, 0.35733, 0.44800, 0.53867, 0.62933, 0.72000,
];
pub const K6: [f32; 16] = [
    -0.50000, -0.41333, -0.32667, -0.24000, -0.15333, -0.06667, 0.02000, 0.10667, 0.19333, 0.28000,
    0.36667, 0.45333, 0.54000, 0.62667, 0.71333, 0.80000,
];
pub const K7: [f32; 16] = [
    -0.60000, -0.50667, -0.41333, -0.32000, -0.22667, -0.13333, -0.04000, 0.05333, 0.14667,
    0.24000, 0.33333, 0.42667, 0.52000, 0.61333, 0.70667, 0.80000,
];
pub const K8: [f32; 8] = [
    -0.50000, -0.31429, -0.12857, 0.05714, 0.24286, 0.42857, 0.61429, 0.80000,
];
pub const K9: [f32; 8] = [
    -0.50000, -0.34286, -0.18571, -0.02857, 0.12857, 0.28571, 0.44286, 0.60000,
];
pub const K10: [f32; 8] = [
    -0.40000, -0.25714, -0.11429, 0.02857, 0.17143, 0.31429, 0.45714, 0.60000,
];

/// Bits of K1–K10.
const K_BITS: [u32; 10] = [5, 5, 4, 4, 4, 4, 4, 3, 3, 3];

fn k_value(i: usize, code: u8) -> f32 {
    let c = usize::from(code);
    match i {
        0 => K1[c & 31],
        1 => K2[c & 31],
        2 => K3[c & 15],
        3 => K4[c & 15],
        4 => K5[c & 15],
        5 => K6[c & 15],
        6 => K7[c & 15],
        7 => K8[c & 7],
        8 => K9[c & 7],
        _ => K10[c & 7],
    }
}

/// One frame's parameters (decoded values).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Params {
    energy: f32,
    pitch: u8,
    k: [f32; 10],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tms5220 {
    /// The VSM's contents (empty: none).
    vsm: Vec<u8>,
    vsm_addr: u32,
    vsm_bit: u8,
    /// Load Address nibbles taken since the last read.
    nibbles: u8,
    /// A Read Byte result waits for the CPU.
    data: Option<u8>,
    fifo: Vec<u8>,
    fifo_bit: u8,
    speak_external: bool,
    talking: bool,
    /// The stop code was read: this frame ends the speech.
    stopping: bool,
    buffer_empty: bool,
    /// The frame being interpolated from and towards.
    current: Params,
    target: Params,
    /// Samples into the current frame.
    sample: u32,
    /// Pitch counter and filter state.
    phase: u32,
    noise: u16,
    u: [f32; 11],
    b: [f32; 11],
    /// The previous frame's coded K values (for repeat frames).
    coded_k: [u8; 10],
}

impl Default for Tms5220 {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Tms5220 {
    pub fn new(vsm: Vec<u8>) -> Self {
        Self {
            vsm,
            vsm_addr: 0,
            vsm_bit: 0,
            nibbles: 0,
            data: None,
            fifo: Vec::new(),
            fifo_bit: 0,
            speak_external: false,
            talking: false,
            stopping: false,
            buffer_empty: true,
            current: Params::default(),
            target: Params::default(),
            sample: 0,
            phase: 0,
            noise: 0x1fff,
            u: [0.0; 11],
            b: [0.0; 11],
            coded_k: [0; 10],
        }
    }

    /// Reset (command or power-up clear): speech stops, the FIFO empties.
    pub fn reset(&mut self) {
        let vsm = std::mem::take(&mut self.vsm);
        *self = Self::new(vsm);
    }

    pub fn talking(&self) -> bool {
        self.talking
    }

    pub fn status(&self) -> u8 {
        let bl = self.fifo.len() < 8;
        let be = self.fifo.is_empty();
        u8::from(self.talking) << 7 | u8::from(bl) << 6 | u8::from(be && self.buffer_empty) << 5
    }

    /// The next bit of the VSM (bit 0 of a byte first).
    fn vsm_next(&mut self) -> u8 {
        let byte = self
            .vsm
            .get(self.vsm_addr as usize % self.vsm.len().max(1))
            .copied()
            .unwrap_or(0);
        let bit = byte >> self.vsm_bit & 1;
        self.vsm_bit += 1;
        if self.vsm_bit == 8 {
            self.vsm_bit = 0;
            self.vsm_addr = (self.vsm_addr + 1) & 0x3fff;
        }
        bit
    }

    fn vsm_byte(&mut self) -> u8 {
        let mut v = 0;
        for _ in 0..8 {
            v = v << 1 | self.vsm_next();
        }
        v
    }

    /// The next bit of the speech data (VSM or FIFO); `None`: the FIFO ran
    /// dry.
    fn next_bit(&mut self) -> Option<u8> {
        if !self.speak_external {
            return Some(self.vsm_next());
        }
        let byte = *self.fifo.first()?;
        let bit = byte >> self.fifo_bit & 1;
        self.fifo_bit += 1;
        if self.fifo_bit == 8 {
            self.fifo_bit = 0;
            self.fifo.remove(0);
        }
        Some(bit)
    }

    fn bits(&mut self, n: u32) -> Option<u8> {
        let mut v = 0;
        for _ in 0..n {
            v = v << 1 | self.next_bit()?;
        }
        Some(v)
    }

    /// A byte from the CPU.
    pub fn write(&mut self, v: u8) {
        if self.speak_external {
            if self.fifo.len() < 16 {
                self.fifo.push(v);
            }
            if !self.talking && self.fifo.len() >= 9 {
                self.start();
            }
            return;
        }
        match v >> 4 & 7 {
            0b001 => {
                self.nibbles = 0;
                self.data = Some(self.vsm_byte());
            }
            0b011 => {
                self.nibbles = 0;
                let lo = self.vsm_byte();
                let hi = self.vsm_byte();
                self.vsm_addr = u32::from(u16::from_be_bytes([hi, lo]) & 0x3fff);
                self.vsm_bit = 0;
            }
            0b100 => {
                let n = u32::from(self.nibbles.min(4));
                let mask = 0xf << (4 * n);
                self.vsm_addr = (self.vsm_addr & !mask) | (u32::from(v & 0xf) << (4 * n));
                self.vsm_addr &= 0xf_ffff;
                self.vsm_bit = 0;
                self.nibbles = self.nibbles.saturating_add(1);
            }
            0b101 => {
                self.nibbles = 0;
                self.speak_external = false;
                self.start();
            }
            0b110 => {
                self.nibbles = 0;
                self.speak_external = true;
                self.fifo.clear();
                self.fifo_bit = 0;
                self.buffer_empty = true;
                self.talking = false;
            }
            0b111 => self.reset(),
            _ => {}
        }
    }

    /// A read by the CPU.
    pub fn read(&mut self) -> u8 {
        match self.data.take() {
            Some(d) => d,
            None => self.status(),
        }
    }

    /// What a read would return.
    pub fn peek(&self) -> u8 {
        self.data.unwrap_or_else(|| self.status())
    }

    fn start(&mut self) {
        self.talking = true;
        self.stopping = false;
        self.buffer_empty = false;
        self.sample = FRAME;
        self.current = Params::default();
        self.target = Params::default();
    }

    /// Read the next frame into `target`.
    fn next_frame(&mut self) {
        if self.stopping {
            self.talking = false;
            self.speak_external = false;
            self.stopping = false;
            self.target = Params::default();
            return;
        }
        let Some(e) = self.bits(4) else {
            return self.ran_dry();
        };
        let prev = self.target;
        self.current = prev;
        if e == 0 {
            self.target = Params {
                energy: 0.0,
                ..prev
            };
            return;
        }
        if e == 15 {
            // Interpolate down to silence, then end.
            self.stopping = true;
            self.target = Params {
                energy: 0.0,
                ..prev
            };
            return;
        }
        let Some(repeat) = self.bits(1) else {
            return self.ran_dry();
        };
        let Some(pitch) = self.bits(6) else {
            return self.ran_dry();
        };
        if repeat == 0 {
            let n = if pitch == 0 { 4 } else { 10 };
            for (i, &width) in K_BITS.iter().enumerate() {
                self.coded_k[i] = if i < n {
                    let Some(c) = self.bits(width) else {
                        return self.ran_dry();
                    };
                    c
                } else {
                    0
                };
            }
        }
        let mut k = [0.0; 10];
        for (i, v) in k.iter_mut().enumerate() {
            *v = if pitch == 0 && i >= 4 {
                0.0
            } else {
                k_value(i, self.coded_k[i])
            };
        }
        self.target = Params {
            energy: f32::from(ENERGY[usize::from(e)]),
            pitch: PITCH[usize::from(pitch)],
            k,
        };
        // No interpolation across a change of voicing or out of silence.
        if prev.energy == 0.0 || (prev.pitch == 0) != (self.target.pitch == 0) {
            self.current = self.target;
        }
    }

    /// Speak External without data: speech ends at once (BE).
    fn ran_dry(&mut self) {
        self.buffer_empty = true;
        self.talking = false;
        self.speak_external = false;
        self.target = Params::default();
        self.current = Params::default();
    }

    /// The parameters at the current sample (interpolated).
    fn params(&self) -> Params {
        let step = (self.sample / (FRAME / STEPS)).min(STEPS - 1) + 1;
        let t = step as f32 / STEPS as f32;
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        let mut p = self.target;
        p.energy = lerp(self.current.energy, self.target.energy);
        for i in 0..10 {
            p.k[i] = lerp(self.current.k[i], self.target.k[i]);
        }
        p
    }

    /// One 8 kHz sample (16-bit).
    pub fn sample(&mut self) -> i16 {
        if !self.talking {
            return 0;
        }
        if self.sample >= FRAME {
            self.sample = 0;
            self.next_frame();
            if !self.talking {
                return 0;
            }
        }
        let p = self.params();
        self.sample += 1;
        let excitation = if p.pitch == 0 {
            // A 13-bit shift register.
            let bit = (self.noise ^ (self.noise >> 1)) & 1;
            self.noise = (self.noise >> 1) | (bit << 12);
            if self.noise & 1 != 0 {
                p.energy
            } else {
                -p.energy
            }
        } else {
            self.phase += 1;
            if self.phase >= u32::from(p.pitch) {
                self.phase = 0;
                p.energy * 4.0
            } else {
                0.0
            }
        };
        // The lattice: u10 = excitation; u(i-1) = u(i) - k(i) b(i-1);
        // b(i) = b(i-1) + k(i) u(i-1).
        self.u[10] = excitation;
        for i in (1..=10).rev() {
            self.u[i - 1] = self.u[i] - p.k[i - 1] * self.b[i - 1];
        }
        for i in (1..=10).rev() {
            self.b[i] = self.b[i - 1] + p.k[i - 1] * self.u[i - 1];
        }
        self.b[0] = self.u[0];
        (self.u[0] * 2.0).clamp(-32767.0, 32767.0) as i16
    }

    /// The registers, FIFO and synthesis state; the VSM's contents are ROM.
    pub fn save(&self, w: &mut StateWriter) {
        w.u32(self.vsm_addr);
        w.u8(self.vsm_bit);
        w.u8(self.nibbles);
        w.bool(self.data.is_some());
        w.u8(self.data.unwrap_or(0));
        w.bytes(&self.fifo);
        w.u8(self.fifo_bit);
        for b in [
            self.speak_external,
            self.talking,
            self.stopping,
            self.buffer_empty,
        ] {
            w.bool(b);
        }
        for p in [&self.current, &self.target] {
            w.u32(p.energy.to_bits());
            w.u8(p.pitch);
            for k in p.k {
                w.u32(k.to_bits());
            }
        }
        w.u32(self.sample);
        w.u32(self.phase);
        w.u16(self.noise);
        for v in self.u.iter().chain(&self.b) {
            w.u32(v.to_bits());
        }
        w.raw(&self.coded_k);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Self::new(std::mem::take(&mut self.vsm));
        let result = (|| {
            n.vsm_addr = r.u32()? & 0xf_ffff;
            n.vsm_bit = r.u8()? & 7;
            n.nibbles = r.u8()?;
            let has = r.bool()?;
            let d = r.u8()?;
            n.data = has.then_some(d);
            n.fifo = r.bytes()?.to_vec();
            if n.fifo.len() > 16 {
                return Err(StateError::Invalid("speech FIFO"));
            }
            n.fifo_bit = r.u8()? & 7;
            n.speak_external = r.bool()?;
            n.talking = r.bool()?;
            n.stopping = r.bool()?;
            n.buffer_empty = r.bool()?;
            for p in [&mut n.current, &mut n.target] {
                p.energy = f32::from_bits(r.u32()?);
                p.pitch = r.u8()?;
                for k in &mut p.k {
                    *k = f32::from_bits(r.u32()?);
                }
            }
            n.sample = r.u32()?.min(FRAME);
            n.phase = r.u32()?;
            n.noise = r.u16()? | 1;
            for v in n.u.iter_mut().chain(n.b.iter_mut()) {
                *v = f32::from_bits(r.u32()?);
            }
            r.raw_into(&mut n.coded_k)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                *self = n;
                Ok(())
            }
            Err(e) => {
                self.vsm = n.vsm;
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits written least significant first into bytes, from parameters
    /// given most significant first.
    fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
        let mut bits = Vec::new();
        for &(v, n) in fields {
            for i in (0..n).rev() {
                bits.push((v >> i & 1) as u8);
            }
        }
        bits.chunks(8)
            .map(|c| c.iter().enumerate().fold(0, |b, (i, &x)| b | x << i))
            .collect()
    }

    #[test]
    fn the_tables_have_their_sizes_and_ends() {
        assert_eq!(
            (ENERGY[15], PITCH[63], K1[0], K2[31]),
            (7789, 159, -0.9785, 0.9883)
        );
        assert_eq!((K10[0], K10[7]), (-0.4, 0.6));
    }

    #[test]
    fn read_byte_and_load_address() {
        let mut vsm = vec![0u8; 0x4000];
        vsm[0] = 0xaa;
        vsm[0x1234] = 0b0000_0001;
        let mut s = Tms5220::new(vsm);
        s.write(0x10);
        assert_eq!(s.read(), 0x55, "first bit into bit 7: 0xAA reversed");
        assert_eq!(s.read() & 0x80, 0, "then the status: not talking");
        for nibble in [0x4, 0x3, 0x2, 0x1, 0x0] {
            s.write(0x40 | nibble);
        }
        s.write(0x10);
        assert_eq!(s.read(), 0x80);
    }

    #[test]
    fn speak_plays_frames_until_the_stop_code() {
        // A voiced frame (energy 10, pitch code 20, K codes 16/8), a
        // repeat frame, then stop.
        let mut f = vec![(10, 4), (0, 1), (20, 6)];
        f.extend([
            (16, 5),
            (16, 5),
            (8, 4),
            (8, 4),
            (8, 4),
            (8, 4),
            (8, 4),
            (4, 3),
            (4, 3),
            (4, 3),
        ]);
        f.extend([(10, 4), (1, 1), (20, 6), (15, 4)]);
        let mut vsm = pack(&f);
        vsm.resize(0x4000, 0);
        let mut s = Tms5220::new(vsm);
        s.write(0x50);
        assert!(s.talking());
        assert_eq!(s.status() & 0x80, 0x80);
        let samples: Vec<i16> = (0..1200).map(|_| s.sample()).collect();
        assert!(!s.talking(), "ended after the stop frame");
        let loud = samples[200..600].iter().filter(|&&v| v.abs() > 500).count();
        assert!(loud > 10, "voiced pulses come out: {loud}");
        assert!(samples[1000..].iter().all(|&v| v == 0));
    }

    #[test]
    fn speak_external_waits_for_nine_bytes() {
        let mut s = Tms5220::new(Vec::new());
        s.write(0x60);
        assert_eq!(s.status() & 0x60, 0x60, "BL and BE");
        for _ in 0..8 {
            s.write(0x00);
        }
        assert!(!s.talking());
        s.write(0x00);
        assert!(s.talking());
        // Zero-energy frames until the FIFO runs dry.
        for _ in 0..20_000 {
            s.sample();
        }
        assert!(!s.talking(), "BE ends it");
        assert_eq!(s.status() & 0x20, 0x20);
    }

    #[test]
    fn savestate_round_trip() {
        let mut s = Tms5220::new(vec![0x11; 0x4000]);
        s.write(0x50);
        for _ in 0..300 {
            s.sample();
        }
        let mut w = StateWriter::new();
        s.save(&mut w);
        let bytes = w.into_bytes();
        let mut t = Tms5220::new(vec![0x11; 0x4000]);
        t.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(s, t);
    }
}
