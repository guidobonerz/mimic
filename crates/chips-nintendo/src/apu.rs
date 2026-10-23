//! The 2A03/2A07 audio processing unit: two pulse channels, triangle,
//! noise, delta modulation channel (DMC) and the frame counter.
//!
//! **Registers** ($4000–$4017): $4000–$4003 pulse 1, $4004–$4007 pulse 2
//! (duty/halt/constant/volume, sweep, timer low, length/timer high),
//! $4008/$400A/$400B triangle (control/linear reload, timer), $400C/$400E/
//! $400F noise (volume, mode/period, length), $4010–$4013 DMC (IRQ/loop/
//! rate, direct load, sample address $C000 + 64·n, length 16·n + 1),
//! $4015 enable (write) / status (read: lengths, DMC active, frame and DMC
//! interrupt; the read clears the frame interrupt), $4017 frame counter
//! (bit 7 five-step mode, bit 6 interrupt inhibit).
//!
//! [`Apu::tick`] runs one CPU cycle. The pulse and noise timers and the
//! frame counter count CPU cycles as the NESdev wiki gives them; the
//! triangle's timer runs every CPU cycle, the pulses' every second.
//!
//! **DMC reads.** When its sample buffer empties and bytes remain, the DMC
//! asks for the next byte ([`Apu::dmc_request`]); the machine fetches it
//! (stealing CPU cycles) and hands it over with [`Apu::dmc_fill`].
//!
//! **Output.** [`Apu::output`] is the non-linear mix of the NESdev wiki
//! "APU Mixer" (pulse 95.88 / (8128 / (p1 + p2) + 100), the others
//! 159.79 / (1 / (t / 8227 + n / 12241 + d / 22638) + 100)), as a table in
//! units of 1/65536.

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

const LENGTHS: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

const DUTY: [u8; 4] = [0b0100_0000, 0b0110_0000, 0b0111_1000, 0b1001_1111];

const TRIANGLE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

const NOISE_NTSC: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];
const NOISE_PAL: [u16; 16] = [
    4, 8, 14, 30, 60, 88, 118, 148, 188, 236, 354, 472, 708, 944, 1890, 3778,
];
const DMC_NTSC: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];
const DMC_PAL: [u16; 16] = [
    398, 354, 316, 298, 276, 236, 210, 198, 176, 148, 132, 118, 98, 78, 66, 50,
];

/// Frame counter steps in CPU cycles: quarter frames at the first four
/// (the last one only in five-step mode at index 4), the interrupt at the
/// last cycles of the four-step sequence.
const FRAME_NTSC: [u32; 5] = [7457, 14913, 22371, 29829, 37281];
const FRAME_PAL: [u32; 5] = [8313, 16627, 24939, 33253, 41565];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Envelope {
    start: bool,
    divider: u8,
    decay: u8,
}

impl Envelope {
    fn clock(&mut self, reg: u8) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = reg & 15;
        } else if self.divider == 0 {
            self.divider = reg & 15;
            if self.decay > 0 {
                self.decay -= 1;
            } else if reg & 0x20 != 0 {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn volume(&self, reg: u8) -> u8 {
        if reg & 0x10 != 0 {
            reg & 15
        } else {
            self.decay
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Pulse {
    /// $4000/$4004.
    ctrl: u8,
    /// $4001/$4005.
    sweep: u8,
    period: u16,
    timer: u16,
    step: u8,
    length: u8,
    env: Envelope,
    sweep_div: u8,
    sweep_reload: bool,
    /// Pulse 1 negates in ones' complement.
    ones: bool,
}

impl Pulse {
    fn target(&self) -> u16 {
        let change = self.period >> (self.sweep & 7);
        if self.sweep & 8 != 0 {
            self.period
                .wrapping_sub(change)
                .wrapping_sub(u16::from(self.ones))
        } else {
            self.period + change
        }
    }

    fn muted(&self) -> bool {
        self.period < 8 || (self.sweep & 8 == 0 && self.target() > 0x7ff)
    }

    fn write(&mut self, reg: u16, v: u8, enabled: bool) {
        match reg & 3 {
            0 => self.ctrl = v,
            1 => {
                self.sweep = v;
                self.sweep_reload = true;
            }
            2 => self.period = (self.period & 0x700) | u16::from(v),
            _ => {
                self.period = (self.period & 0xff) | (u16::from(v & 7) << 8);
                if enabled {
                    self.length = LENGTHS[usize::from(v >> 3)];
                }
                self.step = 0;
                self.env.start = true;
            }
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.step = (self.step + 7) & 7;
        } else {
            self.timer -= 1;
        }
    }

    fn clock_half(&mut self) {
        if self.ctrl & 0x20 == 0 && self.length > 0 {
            self.length -= 1;
        }
        let shift = self.sweep & 7;
        if self.sweep_div == 0 && self.sweep & 0x80 != 0 && shift != 0 && !self.muted() {
            self.period = self.target();
        }
        if self.sweep_div == 0 || self.sweep_reload {
            self.sweep_div = (self.sweep >> 4) & 7;
            self.sweep_reload = false;
        } else {
            self.sweep_div -= 1;
        }
    }

    fn output(&self) -> u8 {
        let high = DUTY[usize::from(self.ctrl >> 6)] >> (7 - self.step) & 1 != 0;
        if self.length == 0 || self.muted() || !high {
            0
        } else {
            self.env.volume(self.ctrl)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Triangle {
    /// $4008.
    ctrl: u8,
    period: u16,
    timer: u16,
    step: u8,
    length: u8,
    linear: u8,
    linear_reload: bool,
}

impl Triangle {
    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            if self.length > 0 && self.linear > 0 {
                self.step = (self.step + 1) & 31;
            }
        } else {
            self.timer -= 1;
        }
    }

    fn clock_quarter(&mut self) {
        if self.linear_reload {
            self.linear = self.ctrl & 0x7f;
        } else if self.linear > 0 {
            self.linear -= 1;
        }
        if self.ctrl & 0x80 == 0 {
            self.linear_reload = false;
        }
    }

    fn clock_half(&mut self) {
        if self.ctrl & 0x80 == 0 && self.length > 0 {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        TRIANGLE[usize::from(self.step)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Noise {
    /// $400C.
    ctrl: u8,
    /// $400E.
    mode: u8,
    timer: u16,
    shift: u16,
    length: u8,
    env: Envelope,
}

impl Noise {
    fn clock_timer(&mut self, table: &[u16; 16]) {
        if self.timer == 0 {
            self.timer = table[usize::from(self.mode & 15)] - 1;
            let tap = if self.mode & 0x80 != 0 { 6 } else { 1 };
            let feedback = (self.shift ^ (self.shift >> tap)) & 1;
            self.shift = (self.shift >> 1) | (feedback << 14);
        } else {
            self.timer -= 1;
        }
    }

    fn clock_half(&mut self) {
        if self.ctrl & 0x20 == 0 && self.length > 0 {
            self.length -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.length == 0 || self.shift & 1 != 0 {
            0
        } else {
            self.env.volume(self.ctrl)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct Dmc {
    /// $4010.
    ctrl: u8,
    level: u8,
    /// $4012/$4013.
    start: u8,
    len: u8,
    addr: u16,
    remaining: u16,
    buffer: Option<u8>,
    shift: u8,
    bits: u8,
    silence: bool,
    timer: u16,
    irq: bool,
    /// The next fetch is a load (started by $4015, not by the buffer
    /// emptying), and the CPU cycles until it may begin.
    load: bool,
    load_delay: u8,
}

impl Dmc {
    fn restart(&mut self) {
        self.addr = 0xc000 | (u16::from(self.start) << 6);
        self.remaining = (u16::from(self.len) << 4) + 1;
    }

    fn clock_timer(&mut self, table: &[u16; 16]) {
        if self.timer > 0 {
            self.timer -= 1;
            return;
        }
        self.timer = table[usize::from(self.ctrl & 15)] - 1;
        if !self.silence {
            if self.shift & 1 != 0 {
                if self.level <= 125 {
                    self.level += 2;
                }
            } else if self.level >= 2 {
                self.level -= 2;
            }
        }
        self.shift >>= 1;
        if self.bits > 0 {
            self.bits -= 1;
        }
        if self.bits == 0 {
            self.bits = 8;
            match self.buffer.take() {
                Some(b) => {
                    self.silence = false;
                    self.shift = b;
                }
                None => self.silence = true,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApuModel {
    Ntsc,
    Pal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apu {
    pub model: ApuModel,
    pulse: [Pulse; 2],
    triangle: Triangle,
    noise: Noise,
    dmc: Dmc,
    /// $4015 bits 0–3: channels enabled (length counters run).
    enabled: u8,
    /// $4017 bits 6–7.
    pub frame_mode: u8,
    /// CPU cycles into the frame sequence.
    frame_cycle: u32,
    /// A $4017 write takes effect in this many cycles (0: none pending).
    frame_reset_in: u8,
    pub frame_irq: bool,
    /// CPU cycles since power-on (parity of the APU's half clock).
    cycle: u64,
}

impl Apu {
    pub fn new(model: ApuModel) -> Self {
        let mut pulse = [Pulse::default(); 2];
        pulse[0].ones = true;
        Self {
            model,
            pulse,
            triangle: Triangle::default(),
            noise: Noise {
                ctrl: 0,
                mode: 0,
                timer: 0,
                shift: 1,
                length: 0,
                env: Envelope::default(),
            },
            dmc: Dmc {
                bits: 8,
                silence: true,
                ..Dmc::default()
            },
            enabled: 0,
            frame_mode: 0,
            frame_cycle: 0,
            frame_reset_in: 0,
            frame_irq: false,
            cycle: 0,
        }
    }

    /// The reset line: channels silenced as by writing 0 to $4015, the
    /// frame counter restarts in its mode, the triangle and DMC keep their
    /// phase and level.
    pub fn reset(&mut self) {
        self.write(0x4015, 0);
        self.frame_irq = false;
        self.dmc.irq = false;
        let mode = self.frame_mode;
        self.write(0x4017, mode);
    }

    fn noise_table(&self) -> &'static [u16; 16] {
        match self.model {
            ApuModel::Ntsc => &NOISE_NTSC,
            ApuModel::Pal => &NOISE_PAL,
        }
    }

    fn dmc_table(&self) -> &'static [u16; 16] {
        match self.model {
            ApuModel::Ntsc => &DMC_NTSC,
            ApuModel::Pal => &DMC_PAL,
        }
    }

    fn frame_steps(&self) -> &'static [u32; 5] {
        match self.model {
            ApuModel::Ntsc => &FRAME_NTSC,
            ApuModel::Pal => &FRAME_PAL,
        }
    }

    /// The IRQ output: frame counter or DMC interrupt.
    pub fn irq(&self) -> bool {
        self.frame_irq || self.dmc.irq
    }

    /// A write to $4000–$4013, $4015 or $4017.
    pub fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0x4000..=0x4003 => {
                let en = self.enabled & 1 != 0;
                self.pulse[0].write(addr, v, en);
            }
            0x4004..=0x4007 => {
                let en = self.enabled & 2 != 0;
                self.pulse[1].write(addr, v, en);
            }
            0x4008 => self.triangle.ctrl = v,
            0x400a => self.triangle.period = (self.triangle.period & 0x700) | u16::from(v),
            0x400b => {
                let t = &mut self.triangle;
                t.period = (t.period & 0xff) | (u16::from(v & 7) << 8);
                if self.enabled & 4 != 0 {
                    t.length = LENGTHS[usize::from(v >> 3)];
                }
                t.linear_reload = true;
            }
            0x400c => self.noise.ctrl = v,
            0x400e => self.noise.mode = v,
            0x400f => {
                if self.enabled & 8 != 0 {
                    self.noise.length = LENGTHS[usize::from(v >> 3)];
                }
                self.noise.env.start = true;
            }
            0x4010 => {
                self.dmc.ctrl = v;
                if v & 0x80 == 0 {
                    self.dmc.irq = false;
                }
            }
            0x4011 => self.dmc.level = v & 0x7f,
            0x4012 => self.dmc.start = v,
            0x4013 => self.dmc.len = v,
            0x4015 => {
                self.enabled = v & 0x1f;
                if v & 1 == 0 {
                    self.pulse[0].length = 0;
                }
                if v & 2 == 0 {
                    self.pulse[1].length = 0;
                }
                if v & 4 == 0 {
                    self.triangle.length = 0;
                }
                if v & 8 == 0 {
                    self.noise.length = 0;
                }
                if v & 0x10 == 0 {
                    self.dmc.remaining = 0;
                } else if self.dmc.remaining == 0 {
                    self.dmc.restart();
                    if self.dmc.buffer.is_none() {
                        // A load fetch halts the CPU in the second APU cycle
                        // after the write (its 3rd or 4th CPU cycle).
                        self.dmc.load = true;
                        self.dmc.load_delay = 3;
                    }
                }
                self.dmc.irq = false;
            }
            0x4017 => {
                self.frame_mode = v & 0xc0;
                if v & 0x40 != 0 {
                    self.frame_irq = false;
                }
                // The sequence restarts 3 or 4 CPU cycles later, depending
                // on the APU cycle the write lands in.
                self.frame_reset_in = if self.cycle & 1 == 0 { 3 } else { 4 };
            }
            _ => {}
        }
    }

    /// $4015 read without side effects.
    pub fn peek_status(&self) -> u8 {
        let mut s = 0;
        if self.pulse[0].length > 0 {
            s |= 1;
        }
        if self.pulse[1].length > 0 {
            s |= 2;
        }
        if self.triangle.length > 0 {
            s |= 4;
        }
        if self.noise.length > 0 {
            s |= 8;
        }
        if self.dmc.remaining > 0 {
            s |= 0x10;
        }
        if self.frame_irq {
            s |= 0x40;
        }
        if self.dmc.irq {
            s |= 0x80;
        }
        s
    }

    /// $4015 read: clears the frame interrupt. Bit 5 is open bus (left to
    /// the caller).
    pub fn read_status(&mut self) -> u8 {
        let s = self.peek_status();
        self.frame_irq = false;
        s
    }

    /// The DMC for the debugger: bytes remaining, sample buffer full, bits
    /// left in the output shift register, output level.
    pub fn dmc_state(&self) -> (u16, bool, u8, u8) {
        let d = &self.dmc;
        (d.remaining, d.buffer.is_some(), d.bits, d.level)
    }

    /// The DMC wants the byte at this address (its buffer is empty and
    /// bytes remain).
    pub fn dmc_request(&self) -> Option<u16> {
        let d = &self.dmc;
        (d.buffer.is_none() && d.remaining > 0 && d.load_delay == 0).then_some(d.addr)
    }

    /// The pending fetch is a load (started by a $4015 write; the DMA
    /// unit halts the CPU on a get cycle) rather than a reload (the
    /// buffer emptied; it halts on a put cycle). NESdev wiki, "DMA".
    pub fn dmc_request_is_load(&self) -> bool {
        self.dmc.load
    }

    /// The byte the DMC asked for.
    pub fn dmc_fill(&mut self, v: u8) {
        let d = &mut self.dmc;
        if d.remaining == 0 {
            return;
        }
        d.load = false;
        d.buffer = Some(v);
        d.addr = if d.addr == 0xffff { 0x8000 } else { d.addr + 1 };
        d.remaining -= 1;
        if d.remaining == 0 {
            if d.ctrl & 0x40 != 0 {
                d.restart();
            } else if d.ctrl & 0x80 != 0 {
                d.irq = true;
            }
        }
    }

    fn quarter(&mut self) {
        let (c0, c1, cn) = (self.pulse[0].ctrl, self.pulse[1].ctrl, self.noise.ctrl);
        self.pulse[0].env.clock(c0);
        self.pulse[1].env.clock(c1);
        self.noise.env.clock(cn);
        self.triangle.clock_quarter();
    }

    fn half(&mut self) {
        self.pulse[0].clock_half();
        self.pulse[1].clock_half();
        self.triangle.clock_half();
        self.noise.clock_half();
    }

    fn clock_frame(&mut self) {
        if self.frame_reset_in > 0 {
            self.frame_reset_in -= 1;
            if self.frame_reset_in == 0 {
                self.frame_cycle = 0;
                if self.frame_mode & 0x80 != 0 {
                    self.quarter();
                    self.half();
                }
                return;
            }
        }
        let steps = self.frame_steps();
        self.frame_cycle += 1;
        let c = self.frame_cycle;
        let five = self.frame_mode & 0x80 != 0;
        let irq_ok = self.frame_mode & 0x40 == 0;
        if c == steps[0] || c == steps[2] {
            self.quarter();
        } else if c == steps[1] {
            self.quarter();
            self.half();
        } else if !five {
            if c == steps[3] - 1 && irq_ok {
                self.frame_irq = true;
            } else if c == steps[3] {
                self.quarter();
                self.half();
                if irq_ok {
                    self.frame_irq = true;
                }
            } else if c == steps[3] + 1 {
                if irq_ok {
                    self.frame_irq = true;
                }
                self.frame_cycle = 0;
            }
        } else if c == steps[4] {
            self.quarter();
            self.half();
        } else if c == steps[4] + 1 {
            self.frame_cycle = 0;
        }
    }

    /// One CPU cycle.
    pub fn tick(&mut self) {
        self.clock_frame();
        if self.dmc.load_delay > 0 {
            self.dmc.load_delay -= 1;
        }
        if self.cycle & 1 == 1 {
            self.pulse[0].clock_timer();
            self.pulse[1].clock_timer();
        }
        self.triangle.clock_timer();
        let nt = self.noise_table();
        self.noise.clock_timer(nt);
        let dt = self.dmc_table();
        self.dmc.clock_timer(dt);
        self.cycle += 1;
    }

    /// The mixed output, 0–65535.
    pub fn output(&self) -> i32 {
        let p = usize::from(self.pulse[0].output() + self.pulse[1].output());
        let t = usize::from(self.triangle.output());
        let n = usize::from(self.noise.output());
        let d = usize::from(self.dmc.level);
        let tables = mixer();
        tables.0[p] + tables.1[3 * t + 2 * n + d]
    }
}

/// Mixer tables in 1/65536: pulse sum 0–30, and 3t + 2n + d 0–202.
fn mixer() -> &'static ([i32; 31], [i32; 203]) {
    use std::sync::OnceLock;
    static T: OnceLock<([i32; 31], [i32; 203])> = OnceLock::new();
    T.get_or_init(|| {
        let pulse = std::array::from_fn(|n| {
            if n == 0 {
                0
            } else {
                (95.52 / (8128.0 / n as f64 + 100.0) * 65536.0) as i32
            }
        });
        let tnd = std::array::from_fn(|n| {
            if n == 0 {
                0
            } else {
                (163.67 / (24329.0 / n as f64 + 100.0) * 65536.0) as i32
            }
        });
        (pulse, tnd)
    })
}

const STATE_TAG: [u8; 4] = *b"2A03";

fn save_env(w: &mut StateWriter, e: &Envelope) {
    w.bool(e.start);
    w.u8(e.divider);
    w.u8(e.decay);
}

fn load_env(c: &mut StateReader<'_>) -> Result<Envelope, StateError> {
    Ok(Envelope {
        start: c.bool()?,
        divider: c.u8()? & 15,
        decay: c.u8()? & 15,
    })
}

impl Savestate for Apu {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            for p in &self.pulse {
                for v in [p.ctrl, p.sweep, p.step, p.length, p.sweep_div] {
                    w.u8(v);
                }
                w.u16(p.period);
                w.u16(p.timer);
                w.bool(p.sweep_reload);
                save_env(w, &p.env);
            }
            let t = &self.triangle;
            for v in [t.ctrl, t.step, t.length, t.linear] {
                w.u8(v);
            }
            w.u16(t.period);
            w.u16(t.timer);
            w.bool(t.linear_reload);
            let n = &self.noise;
            for v in [n.ctrl, n.mode, n.length] {
                w.u8(v);
            }
            w.u16(n.timer);
            w.u16(n.shift);
            save_env(w, &n.env);
            let d = &self.dmc;
            for v in [d.ctrl, d.level, d.start, d.len, d.shift, d.bits] {
                w.u8(v);
            }
            w.u16(d.addr);
            w.u16(d.remaining);
            w.bool(d.buffer.is_some());
            w.u8(d.buffer.unwrap_or(0));
            w.bool(d.silence);
            w.u16(d.timer);
            w.bool(d.irq);
            w.bool(d.load);
            w.u8(d.load_delay);
            w.u8(self.enabled);
            w.u8(self.frame_mode);
            w.u32(self.frame_cycle);
            w.u8(self.frame_reset_in);
            w.bool(self.frame_irq);
            w.u64(self.cycle);
        });
    }

    /// Atomic: a failed load leaves the APU unchanged.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let mut a = self.clone();
        for p in &mut a.pulse {
            p.ctrl = c.u8()?;
            p.sweep = c.u8()?;
            p.step = c.u8()? & 7;
            p.length = c.u8()?;
            p.sweep_div = c.u8()? & 7;
            p.period = c.u16()? & 0x7ff;
            p.timer = c.u16()? & 0x7ff;
            p.sweep_reload = c.bool()?;
            p.env = load_env(&mut c)?;
        }
        let t = &mut a.triangle;
        t.ctrl = c.u8()?;
        t.step = c.u8()? & 31;
        t.length = c.u8()?;
        t.linear = c.u8()? & 0x7f;
        t.period = c.u16()? & 0x7ff;
        t.timer = c.u16()? & 0x7ff;
        t.linear_reload = c.bool()?;
        let n = &mut a.noise;
        n.ctrl = c.u8()?;
        n.mode = c.u8()?;
        n.length = c.u8()?;
        n.timer = c.u16()?;
        n.shift = c.u16()? & 0x7fff;
        n.env = load_env(&mut c)?;
        let d = &mut a.dmc;
        d.ctrl = c.u8()?;
        d.level = c.u8()? & 0x7f;
        d.start = c.u8()?;
        d.len = c.u8()?;
        d.shift = c.u8()?;
        d.bits = c.u8()?;
        if d.bits > 8 {
            return Err(StateError::Invalid("DMC bit count"));
        }
        d.addr = c.u16()?;
        d.remaining = c.u16()?;
        let has = c.bool()?;
        let b = c.u8()?;
        d.buffer = has.then_some(b);
        d.silence = c.bool()?;
        d.timer = c.u16()?;
        d.irq = c.bool()?;
        d.load = c.bool()?;
        d.load_delay = c.u8()?.min(3);
        a.enabled = c.u8()? & 0x1f;
        a.frame_mode = c.u8()? & 0xc0;
        a.frame_cycle = c.u32()?;
        a.frame_reset_in = c.u8()?;
        a.frame_irq = c.bool()?;
        a.cycle = c.u64()?;
        if a.frame_cycle > 50_000 || a.frame_reset_in > 4 {
            return Err(StateError::Invalid("APU frame counter"));
        }
        *self = a;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(apu: &mut Apu, cycles: u32) {
        for _ in 0..cycles {
            apu.tick();
        }
    }

    #[test]
    fn four_step_mode_raises_the_frame_interrupt() {
        let mut apu = Apu::new(ApuModel::Ntsc);
        apu.write(0x4017, 0x00);
        run(&mut apu, 29_000);
        assert!(!apu.irq());
        run(&mut apu, 1_000);
        assert!(apu.irq());
        assert_eq!(apu.read_status() & 0x40, 0x40);
        assert!(!apu.irq(), "reading $4015 acknowledges");
        // Inhibited: no interrupt.
        apu.write(0x4017, 0x40);
        run(&mut apu, 40_000);
        assert!(!apu.irq());
        // Five-step mode never interrupts.
        apu.write(0x4017, 0x80);
        run(&mut apu, 80_000);
        assert!(!apu.irq());
    }

    #[test]
    fn length_counters_count_half_frames() {
        let mut apu = Apu::new(ApuModel::Ntsc);
        apu.write(0x4017, 0x40);
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0x00); // no halt
        apu.write(0x4003, 0x18); // length index 3 = 2
        assert_eq!(apu.peek_status() & 1, 1);
        run(&mut apu, 15_000); // first half frame
        assert_eq!(apu.peek_status() & 1, 1);
        run(&mut apu, 15_000); // second half frame
        assert_eq!(apu.peek_status() & 1, 0);
        // A disabled channel does not load its length.
        apu.write(0x4015, 0x00);
        apu.write(0x4003, 0x08);
        assert_eq!(apu.peek_status() & 1, 0);
    }

    #[test]
    fn pulse_produces_a_square_wave() {
        let mut apu = Apu::new(ApuModel::Ntsc);
        apu.write(0x4015, 0x01);
        apu.write(0x4000, 0xbf); // duty 2, halt, constant 15
        apu.write(0x4002, 0x40);
        apu.write(0x4003, 0x00);
        let mut levels = std::collections::BTreeSet::new();
        for _ in 0..4000 {
            apu.tick();
            levels.insert(apu.output());
        }
        assert_eq!(levels.len(), 2, "on and off: {levels:?}");
    }

    #[test]
    fn dmc_reads_its_sample_and_interrupts() {
        let mut apu = Apu::new(ApuModel::Ntsc);
        apu.write(0x4010, 0x8f); // IRQ, fastest rate
        apu.write(0x4012, 0x00); // $C000
        apu.write(0x4013, 0x00); // 1 byte
        apu.write(0x4015, 0x10);
        assert_eq!(apu.dmc_request(), None, "a load waits a few cycles");
        run(&mut apu, 3);
        assert!(apu.dmc_request_is_load());
        assert_eq!(apu.dmc_request(), Some(0xc000));
        apu.dmc_fill(0xff);
        assert_eq!(apu.dmc_request(), None);
        assert!(apu.irq(), "last byte read");
        assert_eq!(apu.peek_status() & 0x10, 0);
        run(&mut apu, 54 * 9);
        assert!(apu.output() > 0, "level rose");
    }

    #[test]
    fn savestate_round_trip() {
        let mut apu = Apu::new(ApuModel::Pal);
        apu.write(0x4015, 0x0f);
        apu.write(0x400e, 0x83);
        apu.write(0x400f, 0x10);
        run(&mut apu, 12_345);
        let mut w = StateWriter::new();
        apu.save(&mut w);
        let data = w.into_bytes();
        let mut copy = Apu::new(ApuModel::Pal);
        copy.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(copy, apu);
        run(&mut apu, 1000);
        run(&mut copy, 1000);
        assert_eq!(copy, apu);
    }
}
