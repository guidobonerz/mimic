//! Calliope, the Logophor 16's sound chip (SPEC.md 9): eight sample
//! channels fed by DMA from chip RAM, stereo output at 48 kHz.
//!
//! Registers of channel n at FFFF:3000 + $20 × n, AUDMASTER at 3100,
//! AUDIRQ at 3102. Each channel that plays samples fetches one aligned
//! 32-bit word a line in the fixed DMA (the machine asks
//! [`Calliope::fetch_line`] at slot 640) into an 8-byte FIFO; playback
//! takes 1 or 2 bytes a sample from it at 4 MHz / AUDPER.
//!
//! **[S]** (decided here, recorded in SPEC.md 9): writing AUDCTL bit 0
//! from 0 to 1 starts a channel from AUDSEG/AUDOFS with AUDLEN (latched),
//! from 1 to 0 stops it; period, volume, pan, the loop registers and the
//! envelope registers act at once. A channel whose FIFO runs dry keeps its
//! last sample and goes on when data arrives (it plays slower than asked).
//! After AUDLEN samples it continues at AUDLOOP for AUDLLEN samples, again
//! and again, or stops when AUDLLEN is 0; AUDCTL bit 6 raises its
//! interrupt at each such end. Envelope: a level 0–255 (8 fraction bits)
//! stepped once a line, attack up by its rate to 255, decay down to the
//! sustain level, release to 0 when the gate falls; a rate is in 1/256
//! level steps a line, 0 means at once; the gate's rising edge starts the
//! attack from 0. Without the envelope bit the level is 255. Noise: a
//! 17-bit LFSR (taps 17 and 14) clocked at the channel's rate, its low
//! byte as the sample; no DMA. Pan: 0–16 keeps the left side full and
//! raises the right from 0, 16–31 lowers the left to 0 (31) with the right
//! full. Mix: Σ sample × volume/127 × level/255 × pan, × AUDMASTER/127,
//! clipped to 16 bits. A modulating channel is silent; its new sample sets
//! channel n + 1's volume (bits 15–9) or period (the sample, at least 16).
//! Channel 7 cannot modulate. The channels advance in batches of one
//! output sample (250 CPU cycles), the FIFO is filled at slot 640.

use emu_core::{StateError, StateReader, StateWriter};

use crate::lumiere::ChipRam;

pub const AUD0: u16 = 0x3000;
pub const AUDSEG: u16 = 0x00;
pub const AUDOFS: u16 = 0x02;
pub const AUDLENH: u16 = 0x04;
pub const AUDLENL: u16 = 0x06;
pub const AUDPER: u16 = 0x08;
pub const AUDVOL: u16 = 0x0a;
pub const AUDPAN: u16 = 0x0c;
pub const AUDLOOPH: u16 = 0x0e;
pub const AUDLOOPL: u16 = 0x10;
pub const AUDLLENH: u16 = 0x12;
pub const AUDLLENL: u16 = 0x14;
pub const AUDCTL: u16 = 0x16;
pub const AUDENV1: u16 = 0x18;
pub const AUDENV2: u16 = 0x1a;
pub const AUDGATE: u16 = 0x1c;
pub const AUDSTAT: u16 = 0x1e;
pub const AUDMASTER: u16 = 0x3100;
pub const AUDIRQ: u16 = 0x3102;

pub const CTL_PLAY: u16 = 1 << 0;
pub const CTL_16BIT: u16 = 1 << 1;
pub const CTL_MOD_VOLUME: u16 = 1 << 2;
pub const CTL_MOD_PERIOD: u16 = 2 << 2;
pub const CTL_ENVELOPE: u16 = 1 << 4;
pub const CTL_NOISE: u16 = 1 << 5;
pub const CTL_IRQ: u16 = 1 << 6;

/// CPU cycles per output sample (12 MHz / 48 kHz).
pub const CYCLES_PER_SAMPLE: u32 = 250;
/// 4 MHz ticks per output sample, in thirds (83⅓).
const THIRDS_PER_SAMPLE: u32 = 250;
const ENV_MAX: u32 = 255 << 8;
/// Output kept when nobody takes it: one second of stereo.
const OUT_KEEP: usize = 2 * 48_000;
const CHIP_END: u32 = 0x0040_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Env {
    #[default]
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Channel {
    regs: [u16; 16],
    playing: bool,
    irq: bool,
    /// Fetch: next byte address, bytes left in this pass, in the loop.
    fetch_at: u32,
    fetch_left: u32,
    fetch_loop: bool,
    fifo: [u8; 8],
    fifo_len: u8,
    /// Playback: samples played in this pass, in the loop.
    pos: u32,
    play_loop: bool,
    /// Time to the next sample, in thirds of a 4 MHz tick.
    phase: u32,
    sample: i16,
    env: Env,
    level: u32,
    lfsr: u32,
    /// Set by the channel before it (modulation).
    mod_vol: Option<u16>,
    mod_per: Option<u16>,
}

impl Channel {
    fn reg(&self, r: u16) -> u16 {
        self.regs[usize::from(r >> 1)]
    }

    fn ctl(&self) -> u16 {
        self.reg(AUDCTL)
    }

    fn bytes_per_sample(&self) -> u32 {
        if self.ctl() & CTL_16BIT != 0 {
            2
        } else {
            1
        }
    }

    fn pair(&self, hi: u16, lo: u16) -> u32 {
        u32::from(self.reg(hi)) << 16 | u32::from(self.reg(lo))
    }

    fn start(&mut self) {
        self.playing = true;
        self.fetch_at = u32::from(self.reg(AUDSEG)) << 16 | u32::from(self.reg(AUDOFS));
        self.fetch_left = self.pair(AUDLENH, AUDLENL) * self.bytes_per_sample();
        self.fetch_loop = false;
        self.fifo_len = 0;
        self.pos = 0;
        self.play_loop = false;
        self.phase = 0;
        self.sample = 0;
        self.lfsr = 1;
    }

    fn period(&self) -> u32 {
        u32::from(self.mod_per.unwrap_or(self.reg(AUDPER)).max(16))
    }

    fn volume(&self) -> u32 {
        u32::from(self.mod_vol.unwrap_or(self.reg(AUDVOL)).min(127))
    }

    /// Wants this line's DMA slot.
    fn wants_fetch(&self) -> bool {
        self.playing && self.ctl() & CTL_NOISE == 0 && self.fifo_len <= 4 && self.fetch_left > 0
    }

    fn fetch(&mut self, mem: &dyn ChipRam) {
        let word = self.fetch_at & !3;
        let mut a = self.fetch_at;
        while a < word + 4 && self.fetch_left > 0 && self.fifo_len < 8 {
            self.fifo[usize::from(self.fifo_len)] = if a < CHIP_END { mem.chip_byte(a) } else { 0 };
            self.fifo_len += 1;
            a += 1;
            self.fetch_left -= 1;
        }
        self.fetch_at = a;
        if self.fetch_left == 0 {
            let llen = self.pair(AUDLLENH, AUDLLENL);
            if llen > 0 {
                let bps = self.bytes_per_sample();
                let start = u32::from(self.reg(AUDSEG)) << 16 | u32::from(self.reg(AUDOFS));
                self.fetch_at = start.wrapping_add(self.pair(AUDLOOPH, AUDLOOPL) * bps);
                self.fetch_left = llen * bps;
                self.fetch_loop = true;
            }
        }
    }

    /// The next sample; `true` at the end of a pass (for the interrupt).
    fn next_sample(&mut self) -> bool {
        if self.ctl() & CTL_NOISE != 0 {
            let bit = (self.lfsr ^ self.lfsr >> 3) & 1;
            self.lfsr = self.lfsr >> 1 | bit << 16;
            self.sample = i16::from(self.lfsr as u8 as i8) << 8;
            return false;
        }
        // The pass is over once its last sample has had its time.
        let end = if self.play_loop {
            self.pair(AUDLLENH, AUDLLENL)
        } else {
            self.pair(AUDLENH, AUDLENL)
        };
        let mut ended = false;
        if self.pos >= end {
            self.pos = 0;
            ended = true;
            if self.pair(AUDLLENH, AUDLLENL) > 0 {
                self.play_loop = true;
            } else {
                self.playing = false;
                self.sample = 0;
                self.regs[usize::from(AUDCTL >> 1)] &= !CTL_PLAY;
                return true;
            }
        }
        let bps = self.bytes_per_sample() as u8;
        if self.fifo_len < bps {
            return ended; // dry: hold the last sample
        }
        self.sample = if bps == 2 {
            i16::from_le_bytes([self.fifo[0], self.fifo[1]])
        } else {
            i16::from(self.fifo[0] as i8) << 8
        };
        self.fifo.copy_within(usize::from(bps).., 0);
        self.fifo_len -= bps;
        self.pos += 1;
        ended
    }

    fn envelope(&mut self) {
        let rate = |r: u16| if r == 0 { ENV_MAX } else { u32::from(r) };
        let (e1, e2) = (self.reg(AUDENV1), self.reg(AUDENV2));
        let sustain = u32::from(e2 & 0xff) << 8;
        match self.env {
            Env::Off | Env::Sustain => {}
            Env::Attack => {
                self.level = (self.level + rate(e1 & 0xff)).min(ENV_MAX);
                if self.level == ENV_MAX {
                    self.env = Env::Decay;
                }
            }
            Env::Decay => {
                self.level = self.level.saturating_sub(rate(e1 >> 8)).max(sustain);
                if self.level == sustain {
                    self.env = Env::Sustain;
                }
            }
            Env::Release => {
                self.level = self.level.saturating_sub(rate(e2 >> 8));
                if self.level == 0 {
                    self.env = Env::Off;
                }
            }
        }
    }

    fn gate(&mut self, on: bool) {
        let was = self.reg(AUDGATE) & 1 != 0;
        if on && !was {
            self.env = Env::Attack;
            self.level = 0;
        } else if !on && was {
            self.env = Env::Release;
        }
    }

    /// Envelope level 0–255.
    fn level(&self) -> u32 {
        if self.ctl() & CTL_ENVELOPE != 0 {
            self.level >> 8
        } else {
            255
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calliope {
    ch: [Channel; 8],
    master: u16,
    /// CPU cycles towards the next output sample.
    cycles: u32,
    /// Output samples, interleaved left/right (not machine state).
    out: Vec<i16>,
}

impl Default for Calliope {
    fn default() -> Self {
        Self::new()
    }
}

impl Calliope {
    pub fn new() -> Self {
        Self {
            ch: Default::default(),
            master: 0,
            cycles: 0,
            out: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        let out = std::mem::take(&mut self.out);
        *self = Self::new();
        self.out = out;
    }

    pub fn peek(&self, off: u16) -> u16 {
        let off = off & !1;
        match off {
            AUDMASTER => self.master,
            AUDIRQ => self.irq_bits(),
            AUD0..=0x30ff => {
                let c = &self.ch[usize::from((off - AUD0) >> 5)];
                match off & 0x1f {
                    AUDSTAT => u16::from(c.playing) | u16::from(c.irq) << 1,
                    r => c.reg(r),
                }
            }
            _ => 0,
        }
    }

    /// Register read: AUDSTAT clears the channel's interrupt.
    pub fn read(&mut self, off: u16) -> u16 {
        let v = self.peek(off);
        let off = off & !1;
        if (AUD0..=0x30ff).contains(&off) && off & 0x1f == AUDSTAT {
            self.ch[usize::from((off - AUD0) >> 5)].irq = false;
        }
        v
    }

    pub fn write(&mut self, off: u16, v: u16) {
        let off = off & !1;
        match off {
            AUDMASTER => self.master = v & 0x7f,
            AUDIRQ => {
                for (n, c) in self.ch.iter_mut().enumerate() {
                    if v >> n & 1 != 0 {
                        c.irq = false;
                    }
                }
            }
            AUD0..=0x30ff => {
                let n = usize::from((off - AUD0) >> 5);
                let c = &mut self.ch[n];
                let r = off & 0x1f;
                match r {
                    AUDCTL => {
                        let was = c.ctl() & CTL_PLAY != 0;
                        c.regs[usize::from(r >> 1)] = v & 0x7f;
                        if v & CTL_PLAY != 0 && !was {
                            c.start();
                        } else if v & CTL_PLAY == 0 {
                            c.playing = false;
                            c.sample = 0;
                        }
                        if v & 0x0c == 0 && n < 7 {
                            // No longer modulating: the next channel is free.
                            let next = &mut self.ch[n + 1];
                            next.mod_vol = None;
                            next.mod_per = None;
                        }
                    }
                    AUDGATE => {
                        c.gate(v & 1 != 0);
                        c.regs[usize::from(r >> 1)] = v & 1;
                    }
                    AUDSTAT => {}
                    AUDVOL => c.regs[usize::from(r >> 1)] = v & 0x7f,
                    AUDPAN => c.regs[usize::from(r >> 1)] = v & 0x1f,
                    _ => c.regs[usize::from(r >> 1)] = v,
                }
            }
            _ => {}
        }
    }

    fn irq_bits(&self) -> u16 {
        self.ch
            .iter()
            .enumerate()
            .map(|(n, c)| u16::from(c.irq) << n)
            .fold(0, |a, b| a | b)
    }

    /// A channel's interrupt is pending (Telegraph's Calliope source).
    pub fn irq(&self) -> bool {
        self.irq_bits() != 0
    }

    /// Slot 640: every channel that wants data takes its slot (when the
    /// DMA is on) and fills its FIFO; the slots taken.
    pub fn fetch_line(&mut self, dma: bool, mem: &dyn ChipRam) -> u16 {
        if !dma {
            return 0;
        }
        let mut slots = 0;
        for c in &mut self.ch {
            if c.wants_fetch() {
                c.fetch(mem);
                slots += 1;
            }
        }
        slots
    }

    /// The end of a line: the envelopes step.
    pub fn end_line(&mut self) {
        for c in &mut self.ch {
            if c.env != Env::Off && c.env != Env::Sustain {
                c.envelope();
            }
        }
    }

    /// One CPU cycle; every [`CYCLES_PER_SAMPLE`] the channels advance and
    /// a stereo sample is mixed. `true` when a channel interrupt is new.
    pub fn cycle(&mut self) -> bool {
        self.cycles += 1;
        if self.cycles < CYCLES_PER_SAMPLE {
            return false;
        }
        self.cycles = 0;
        self.output_sample()
    }

    fn output_sample(&mut self) -> bool {
        let mut new_irq = false;
        for n in 0..8 {
            if !self.ch[n].playing {
                continue;
            }
            self.ch[n].phase += THIRDS_PER_SAMPLE;
            loop {
                let step = 3 * self.ch[n].period();
                if self.ch[n].phase < step || !self.ch[n].playing {
                    break;
                }
                self.ch[n].phase -= step;
                let c = &mut self.ch[n];
                if c.next_sample() && c.ctl() & CTL_IRQ != 0 && !c.irq {
                    c.irq = true;
                    new_irq = true;
                }
                let (mode, s) = (self.ch[n].ctl() >> 2 & 3, self.ch[n].sample as u16);
                if n < 7 && mode != 0 {
                    let next = &mut self.ch[n + 1];
                    match mode {
                        1 => next.mod_vol = Some(s >> 9),
                        2 => next.mod_per = Some(s.max(16)),
                        _ => {}
                    }
                }
            }
        }
        let (mut l, mut r) = (0i64, 0i64);
        for c in &self.ch {
            if !c.playing || c.ctl() >> 2 & 3 != 0 {
                continue;
            }
            let pan = u32::from(c.reg(AUDPAN));
            let gl = if pan <= 16 { 240 } else { (31 - pan) * 16 };
            let gr = if pan >= 16 { 240 } else { pan * 15 };
            let v = i64::from(c.sample) * i64::from(c.volume() * c.level());
            l += v * i64::from(gl);
            r += v * i64::from(gr);
        }
        let scale = 127 * 255 * 240 * 127;
        let m = i64::from(self.master);
        let clip = |x: i64| (x * m / scale).clamp(-32768, 32767) as i16;
        if self.out.len() >= OUT_KEEP {
            self.out.drain(..OUT_KEEP / 2);
        }
        self.out.push(clip(l));
        self.out.push(clip(r));
        new_irq
    }

    /// The samples mixed since the last call, interleaved left/right.
    pub fn take_output(&mut self, out: &mut Vec<i16>) {
        out.append(&mut self.out);
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u16(self.master);
        w.u32(self.cycles);
        for c in &self.ch {
            for r in c.regs {
                w.u16(r);
            }
            w.bool(c.playing);
            w.bool(c.irq);
            w.u32(c.fetch_at);
            w.u32(c.fetch_left);
            w.bool(c.fetch_loop);
            w.raw(&c.fifo);
            w.u8(c.fifo_len);
            w.u32(c.pos);
            w.bool(c.play_loop);
            w.u32(c.phase);
            w.u16(c.sample as u16);
            w.u8(c.env as u8);
            w.u32(c.level);
            w.u32(c.lfsr);
            w.u32(c.mod_vol.map_or(0x1_0000, u32::from));
            w.u32(c.mod_per.map_or(0x1_0000, u32::from));
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Calliope::new();
        s.master = r.u16()? & 0x7f;
        s.cycles = r.u32()? % CYCLES_PER_SAMPLE;
        for c in &mut s.ch {
            for reg in &mut c.regs {
                *reg = r.u16()?;
            }
            c.playing = r.bool()?;
            c.irq = r.bool()?;
            c.fetch_at = r.u32()?;
            c.fetch_left = r.u32()?;
            c.fetch_loop = r.bool()?;
            r.raw_into(&mut c.fifo)?;
            c.fifo_len = r.u8()?.min(8);
            c.pos = r.u32()?;
            c.play_loop = r.bool()?;
            c.phase = r.u32()?;
            c.sample = r.u16()? as i16;
            c.env = match r.u8()? {
                0 => Env::Off,
                1 => Env::Attack,
                2 => Env::Decay,
                3 => Env::Sustain,
                4 => Env::Release,
                _ => return Err(StateError::Invalid("Calliope envelope")),
            };
            c.level = r.u32()?.min(ENV_MAX);
            c.lfsr = r.u32()? & 0x1_ffff;
            let opt = |v: u32| (v <= 0xffff).then_some(v as u16);
            c.mod_vol = opt(r.u32()?);
            c.mod_per = opt(r.u32()?);
            // A phase beyond any period would loop for long: bound it.
            c.phase = c.phase.min(3 * 0xffff);
        }
        s.out = std::mem::take(&mut self.out);
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    const LINE: u32 = 318;

    /// Channel `n`'s registers.
    fn set(c: &mut Calliope, n: u16, regs: &[(u16, u16)]) {
        for &(r, v) in regs {
            c.write(AUD0 + 0x20 * n + r, v);
        }
    }

    /// Run `samples` output samples with a fetch every line; the output
    /// and the DMA slots taken.
    fn run(c: &mut Calliope, ram: &Ram, samples: u32, dma: bool) -> (Vec<i16>, u32) {
        let mut slots = 0;
        let mut out = Vec::new();
        c.take_output(&mut Vec::new());
        for k in 0..samples * CYCLES_PER_SAMPLE {
            if k % LINE == 0 {
                slots += u32::from(c.fetch_line(dma, ram));
                c.end_line();
            }
            c.cycle();
        }
        c.take_output(&mut out);
        (out, slots)
    }

    /// A channel playing `data` (8 bit) from 0001:0000 at 16 kHz (three
    /// output samples each), volume 127, centre, master 127.
    fn channel(data: &[u8], len: u16, loop_len: u16, ctl: u16) -> (Calliope, Ram) {
        let mut ram = vec![0; 0x2_0000];
        ram[0x1_0000..0x1_0000 + data.len()].copy_from_slice(data);
        let mut c = Calliope::new();
        c.write(AUDMASTER, 127);
        set(
            &mut c,
            0,
            &[
                (AUDSEG, 1),
                (AUDLENL, len),
                (AUDPER, 250),
                (AUDVOL, 127),
                (AUDPAN, 16),
                (AUDLLENL, loop_len),
                (AUDCTL, ctl | CTL_PLAY),
            ],
        );
        (c, Ram(ram))
    }

    fn left(out: &[i16]) -> Vec<i16> {
        out.iter().step_by(2).copied().collect()
    }

    #[test]
    fn samples_play_through_the_fifo_at_their_rate() {
        let (mut c, ram) = channel(&[0x40, 0xc0, 0x20, 0x10], 4, 0, CTL_IRQ);
        let (out, slots) = run(&mut c, &ram, 20, true);
        let l = left(&out);
        // A sample every 3 output samples (16 kHz of 48), full scale.
        assert_eq!(
            &l[..12],
            &[
                0, 0, 0x4000, 0x4000, 0x4000, -0x4000, -0x4000, -0x4000, 0x2000, 0x2000, 0x2000,
                0x1000
            ]
        );
        assert_eq!(out[4], out[5], "centre: both sides");
        assert_eq!(l[14], 0, "stopped after 4 samples");
        assert_eq!(slots, 1, "one word for 4 bytes");
        assert_eq!(c.peek(AUD0 + AUDSTAT), 2, "stopped, interrupt");
        assert_eq!(c.peek(AUD0 + AUDCTL) & CTL_PLAY, 0);
        assert!(c.irq());
        assert_eq!(c.read(AUDIRQ), 1);
        assert_eq!(c.read(AUD0 + AUDSTAT), 2);
        assert!(!c.irq(), "cleared by reading AUDSTAT");
    }

    #[test]
    fn loops_sixteen_bits_and_a_slow_dma() {
        // 2 samples, then samples 1-2 again and again.
        let (mut c, ram) = channel(&[0x10, 0x20, 0x30], 2, 2, 0);
        set(&mut c, 0, &[(AUDLOOPL, 1)]);
        let l = left(&run(&mut c, &ram, 24, true).0);
        let seq: Vec<i16> = l.chunks(3).map(|s| s[2] >> 8).collect();
        assert_eq!(seq, [0x10, 0x20, 0x20, 0x30, 0x20, 0x30, 0x20, 0x30]);
        // 16-bit samples, little-endian.
        let (mut c, ram) = channel(&[0x34, 0x12, 0xcc, 0xed], 2, 0, CTL_16BIT);
        let l = left(&run(&mut c, &ram, 9, true).0);
        assert_eq!((l[2], l[5]), (0x1234, -0x1234));
        // Without its DMA a channel holds its last sample.
        let (mut c, ram) = channel(&[0x40; 8], 8, 0, 0);
        let l = left(&run(&mut c, &ram, 30, false).0);
        assert!(l.iter().all(|s| *s == 0));
        assert!(c.peek(AUD0 + AUDSTAT) & 1 != 0, "still playing");
    }

    #[test]
    fn pan_volume_and_master() {
        let (mut c, ram) = channel(&[0x40; 4], 4, 4, 0);
        set(&mut c, 0, &[(AUDPAN, 0), (AUDVOL, 64)]);
        c.write(AUDMASTER, 127);
        let out = run(&mut c, &ram, 6, true).0;
        assert_eq!((out[4], out[5]), ((0x4000i32 * 64 / 127) as i16, 0));
        set(&mut c, 0, &[(AUDPAN, 31), (AUDVOL, 127)]);
        c.write(AUDMASTER, 64);
        let out = run(&mut c, &ram, 3, true).0;
        assert_eq!((out[4], out[5]), (0, (0x4000i32 * 64 / 127) as i16));
        set(&mut c, 0, &[(AUDPAN, 8)]);
        let out = run(&mut c, &ram, 3, true).0;
        assert_eq!(out[5], out[4] / 2, "right at half");
    }

    #[test]
    fn the_envelope_follows_the_gate() {
        let (mut c, ram) = channel(&[0x40; 4], 4, 4, CTL_ENVELOPE);
        // Attack at once, decay 64 (127 levels in 508 lines), sustain 128,
        // release at once.
        set(&mut c, 0, &[(AUDENV1, 64 << 8), (AUDENV2, 128)]);
        assert!(
            left(&run(&mut c, &ram, 6, true).0).iter().all(|s| *s == 0),
            "gate closed"
        );
        set(&mut c, 0, &[(AUDGATE, 1)]);
        let l = left(&run(&mut c, &ram, 12, true).0);
        assert!(l[11] > 0x3e00, "attack done: {:x}", l[11]);
        assert!(l[11] < 0x4000, "decaying");
        let l = left(&run(&mut c, &ram, 48_00, true).0);
        let sustain = (0x4000i32 * 128 / 255) as i16;
        assert_eq!(*l.last().unwrap(), sustain, "decayed to the sustain level");
        set(&mut c, 0, &[(AUDENV2, 128), (AUDGATE, 0)]);
        let l = left(&run(&mut c, &ram, 12, true).0);
        assert_eq!(l[11], 0, "released at once");
    }

    #[test]
    fn noise_needs_no_dma_and_modulation_drives_the_next_channel() {
        let (mut c, ram) = channel(&[], 0, 0, CTL_NOISE);
        let (out, slots) = run(&mut c, &ram, 100, true);
        assert_eq!(slots, 0);
        let l = left(&out);
        assert!(l.iter().any(|s| *s > 0) && l.iter().any(|s| *s < 0));
        // Channel 0 modulates channel 1's volume: its samples $7F and $3F
        // (bits 15-9: 63 and 31); channel 0 itself is silent.
        let (mut c, mut ram) = channel(&[0x7f, 0x3f], 2, 2, CTL_MOD_VOLUME);
        ram.0[0x1_1000..0x1_1004].fill(0x40);
        set(
            &mut c,
            1,
            &[
                (AUDSEG, 1),
                (AUDOFS, 0x1000),
                (AUDLENL, 4),
                (AUDLLENL, 4),
                (AUDPER, 250),
                (AUDVOL, 127),
                (AUDPAN, 16),
                (AUDCTL, CTL_PLAY),
            ],
        );
        let l = left(&run(&mut c, &ram, 12, true).0);
        assert_eq!(l[2], (0x4000i32 * 63 / 127) as i16);
        assert_eq!(l[5], (0x4000i32 * 31 / 127) as i16);
    }

    #[test]
    fn savestate_round_trip() {
        let (mut c, ram) = channel(&[0x40, 0x20, 0x10], 3, 3, CTL_ENVELOPE | CTL_IRQ);
        set(&mut c, 0, &[(AUDENV1, 3), (AUDGATE, 1)]);
        run(&mut c, &ram, 7, true);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut d = Calliope::new();
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, d);
        assert_eq!(run(&mut c, &ram, 50, true), run(&mut d, &ram, 50, true));
        assert!(d.load(&mut StateReader::new(&bytes[..10])).is_err());
    }
}
