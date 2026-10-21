//! POKEY (CO12294): sound, timers, keyboard scan, serial I/O and paddles
//! of the Atari 8-bit computers.
//!
//! Registers (`$D200`, mirrored every 16 bytes up to `$D2FF`):
//!
//! | Reg   | Write              | Read          |
//! | ----- | ------------------ | ------------- |
//! | 0,2,4,6 | AUDF1–4          | POT0–3        |
//! | 1,3,5,7 | AUDC1–4          | POT4–7        |
//! | 8     | AUDCTL             | ALLPOT        |
//! | 9     | STIMER             | KBCODE        |
//! | A     | SKRES              | RANDOM        |
//! | B     | POTGO              | –             |
//! | D     | SEROUT             | SERIN         |
//! | E     | IRQEN              | IRQST         |
//! | F     | SKCTL              | SKSTAT        |
//!
//! AUDCTL: 7 9-bit poly, 6 channel 1 at 1.79 MHz, 5 channel 3 at 1.79 MHz,
//! 4 join 1+2, 3 join 3+4, 2/1 high-pass filters, 0 15 kHz base clock
//! (else 64 kHz). Base clocks: 64 kHz = every 28 cycles, 15 kHz = every 114.
//! Timer periods: AUDF+1 base ticks; AUDF+4 cycles at 1.79 MHz; a joined
//! pair at 1.79 MHz AUDF16+7 cycles.
//!
//! IRQEN/IRQST (IRQST bits are 0 while pending): 7 BREAK, 6 key, 5 serial
//! input ready, 4 serial output needed, 3 serial output finished (a level,
//! not latched), 2 timer 4, 1 timer 2, 0 timer 1. Disabling a bit in IRQEN
//! clears it.
//!
//! SKSTAT: 7 framing error, 6 keyboard overrun, 5 serial overrun (all 0 =
//! set), 3 SHIFT down (0), 2 key down (0), 1 serial input busy (0).
//! SKCTL: 0 keyboard debounce, 1 keyboard scan, bits 0–1 = 0 hold POKEY in
//! its initial state (poly counters reset).
//!
//! Sources: POKEY data sheet (C012294); Altirra Hardware Reference Manual,
//! "POKEY". The keyboard scan visits one of 64 key codes every 114 cycles;
//! with debounce a key must be seen on two passes before KBCODE latches.
//! Serial output shifts one bit per two timer-4 periods (start bit, eight
//! data bits LSB first, stop bit).
//!
//! Sound: on each underflow a channel's output changes as AUDC bits 7–5
//! select — bit 7 = 0 gates the change with the 5-bit polynomial, bit 5 = 1
//! toggles (pure tone), else the output takes the 4-bit (bit 6 = 1) or
//! 17/9-bit polynomial. Polynomials x^4+x^3+1, x^5+x^3+1, x^9+x^4+1,
//! x^17+x^12+1 step every cycle. AUDC bit 4 outputs the volume constantly.
//! High-pass filters: channel 1 (2) is XORed with its own output sampled at
//! each channel 3 (4) underflow. Channels mix linearly.
//!
//! Pots: POTGO restarts a counter that steps once per line (every cycle
//! with SKCTL bit 2, fast scan) up to 228; each POT register latches the
//! count at which its line's capacitor charged, ALLPOT shows the lines
//! still counting (1).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

const IRQ_BREAK: u8 = 0x80;
const IRQ_KEY: u8 = 0x40;
const IRQ_SERIN: u8 = 0x20;
const IRQ_SEROUT_NEEDED: u8 = 0x10;
const IRQ_SEROUT_DONE: u8 = 0x08;
const IRQ_TIMER4: u8 = 0x04;
const IRQ_TIMER2: u8 = 0x02;
const IRQ_TIMER1: u8 = 0x01;

/// Serial bits per byte: start, 8 data, stop.
const SERIAL_BITS: u8 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pokey {
    pub audf: [u8; 4],
    pub audc: [u8; 4],
    pub audctl: u8,
    pub skctl: u8,
    pub irqen: u8,
    /// Latched interrupt requests (1 = pending).
    pending: u8,
    /// Ticks of each channel's clock until it underflows.
    counters: [u32; 4],
    /// Channel outputs before the high-pass filters.
    outputs: [bool; 4],
    /// High-pass filter latches of channels 1 and 2.
    highpass: [bool; 2],
    prescale64: u8,
    prescale15: u8,
    poly4: u8,
    poly5: u8,
    poly17: u32,
    poly9: u16,
    /// Pot positions 0–228 from the paddles (228 = open line).
    pot_input: [u8; 8],
    /// Latched pot counts and the scan counter.
    pots: [u8; 8],
    pot_counter: u8,
    /// Pots still counting (ALLPOT, 1 = counting).
    allpot: u8,
    /// Keys held down by key code 0–63.
    keys: u64,
    shift: bool,
    control: bool,
    pub kbcode: u8,
    key_down: bool,
    scan: u8,
    /// Key code seen on the last pass, waiting for debounce.
    compare: Option<u8>,
    /// SEROUT holding register and the byte being shifted out.
    serout_hold: Option<u8>,
    shift_reg: u16,
    /// The byte in the shift register.
    shifting: u8,
    shift_bits: u8,
    shift_phase: bool,
    /// Bytes sent completely (for SIO devices), oldest first.
    sent: Vec<u8>,
    pub serin: u8,
    serial_overrun: bool,
}

impl Default for Pokey {
    fn default() -> Self {
        Self::new()
    }
}

impl Pokey {
    pub fn new() -> Self {
        Self {
            audf: [0; 4],
            audc: [0; 4],
            audctl: 0,
            skctl: 0,
            irqen: 0,
            pending: 0,
            counters: [0; 4],
            outputs: [false; 4],
            highpass: [false; 2],
            prescale64: 0,
            prescale15: 0,
            poly4: 0x0f,
            poly5: 0x1f,
            poly17: 0x1ffff,
            poly9: 0x1ff,
            pot_input: [228; 8],
            pots: [228; 8],
            pot_counter: 228,
            allpot: 0,
            keys: 0,
            shift: false,
            control: false,
            kbcode: 0xff,
            key_down: false,
            scan: 0,
            compare: None,
            serout_hold: None,
            shift_reg: 0,
            shifting: 0,
            shift_bits: 0,
            shift_phase: false,
            sent: Vec::new(),
            serin: 0xff,
            serial_overrun: false,
        }
    }

    /// POKEY has no reset pin; the 800XL's RESET key leaves it alone.
    pub fn reset(&mut self) {}

    // --- pots ----------------------------------------------------------

    /// A paddle position on pot line `n` (0–7): 1 (turned fully) to 228
    /// (other end, or nothing connected).
    pub fn set_pot(&mut self, n: usize, value: u8) {
        if let Some(p) = self.pot_input.get_mut(n) {
            *p = value.min(228);
        }
    }

    fn pot_step(&mut self) {
        if self.allpot == 0 {
            return;
        }
        self.pot_counter = self.pot_counter.saturating_add(1);
        for n in 0..8 {
            let bit = 1 << n;
            if self.allpot & bit != 0
                && (self.pot_counter >= self.pot_input[n] || self.pot_counter >= 228)
            {
                self.pots[n] = self.pot_counter;
                self.allpot &= !bit;
            }
        }
    }

    // --- keyboard ------------------------------------------------------

    /// A key of the 64-key matrix (KBCODE bits 0–5) went down or up.
    pub fn set_key(&mut self, code: u8, down: bool) {
        let bit = 1u64 << (code & 0x3f);
        if down {
            self.keys |= bit;
        } else {
            self.keys &= !bit;
        }
    }

    pub fn set_shift(&mut self, down: bool) {
        self.shift = down;
    }

    pub fn set_control(&mut self, down: bool) {
        self.control = down;
    }

    /// The BREAK key has its own line and interrupt.
    pub fn press_break(&mut self) {
        self.request(IRQ_BREAK);
    }

    fn scan_step(&mut self) {
        let code = self.scan;
        self.scan = (self.scan + 1) & 0x3f;
        let pressed = self.keys & (1 << code) != 0;
        if self.key_down {
            if code == self.kbcode & 0x3f && !pressed {
                self.key_down = false;
            }
            return;
        }
        if !pressed {
            return;
        }
        let debounce = self.skctl & 0x01 != 0;
        if !debounce || self.compare == Some(code) {
            self.kbcode =
                code | if self.shift { 0x40 } else { 0 } | if self.control { 0x80 } else { 0 };
            self.key_down = true;
            self.compare = None;
            self.request(IRQ_KEY);
        } else {
            self.compare = Some(code);
        }
    }

    // --- interrupts ----------------------------------------------------

    fn request(&mut self, bit: u8) {
        if self.irqen & bit != 0 {
            self.pending |= bit;
        }
    }

    fn serout_done(&self) -> bool {
        self.shift_bits == 0 && self.serout_hold.is_none()
    }

    /// Active interrupt bits (1 = active), including the level of
    /// "serial output finished".
    fn active(&self) -> u8 {
        let done = if self.serout_done() {
            IRQ_SEROUT_DONE & self.irqen
        } else {
            0
        };
        self.pending | done
    }

    /// The /IRQ output.
    pub fn irq(&self) -> bool {
        self.active() != 0
    }

    // --- serial --------------------------------------------------------

    /// Bytes shifted out completely since the last call.
    pub fn take_sent(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.sent)
    }

    /// A byte arrived on the serial input (SIO device → computer).
    pub fn receive(&mut self, v: u8) {
        if self.pending & IRQ_SERIN != 0 {
            self.serial_overrun = true;
        }
        self.serin = v;
        self.request(IRQ_SERIN);
    }

    fn load_shift(&mut self) {
        if let Some(v) = self.serout_hold.take() {
            // Start bit 0, data LSB first, stop bit 1.
            self.shift_reg = (u16::from(v) << 1) | 0x200;
            self.shifting = v;
            self.shift_bits = SERIAL_BITS;
            self.shift_phase = false;
            self.request(IRQ_SEROUT_NEEDED);
        }
    }

    /// Serial output clock: one bit per two timer-4 underflows.
    fn serial_clock(&mut self) {
        if self.shift_bits == 0 {
            return;
        }
        self.shift_phase = !self.shift_phase;
        if self.shift_phase {
            return;
        }
        self.shift_reg >>= 1;
        self.shift_bits -= 1;
        if self.shift_bits == 0 {
            self.sent.push(self.shifting);
            self.load_shift();
        }
    }

    /// The serial output line (1 = mark/idle).
    pub fn serial_out(&self) -> bool {
        self.shift_bits == 0 || self.shift_reg & 1 != 0
    }

    // --- timers --------------------------------------------------------

    fn fast(&self, ch: usize) -> bool {
        match ch {
            0 => self.audctl & 0x40 != 0,
            2 => self.audctl & 0x20 != 0,
            _ => false,
        }
    }

    fn joined(&self, high: usize) -> bool {
        match high {
            1 => self.audctl & 0x10 != 0,
            3 => self.audctl & 0x08 != 0,
            _ => false,
        }
    }

    /// Ticks from reload to underflow minus one, for channel `ch`.
    fn reload(&self, ch: usize) -> u32 {
        let joined_high = (ch == 1 || ch == 3) && self.joined(ch);
        if joined_high {
            let v = u32::from(self.audf[ch - 1]) | (u32::from(self.audf[ch]) << 8);
            if self.fast(ch - 1) {
                v + 6
            } else {
                v
            }
        } else if self.fast(ch) {
            u32::from(self.audf[ch]) + 3
        } else {
            u32::from(self.audf[ch])
        }
    }

    /// Clock channel `ch` once; true on underflow.
    fn clock(&mut self, ch: usize) -> bool {
        if self.counters[ch] == 0 {
            self.counters[ch] = self.reload(ch);
            self.distort(ch);
            true
        } else {
            self.counters[ch] -= 1;
            false
        }
    }

    /// A channel's output change on underflow (AUDC bits 7–5).
    fn distort(&mut self, ch: usize) {
        let c = self.audc[ch];
        if c & 0x80 == 0 && self.poly5 & 1 == 0 {
            return;
        }
        self.outputs[ch] = if c & 0x20 != 0 {
            !self.outputs[ch]
        } else if c & 0x40 != 0 {
            self.poly4 & 1 != 0
        } else if self.audctl & 0x80 != 0 {
            self.poly9 & 1 != 0
        } else {
            self.poly17 & 1 != 0
        };
    }

    /// One machine cycle.
    pub fn tick(&mut self) {
        let init = self.skctl & 0x03 == 0;
        if !init {
            // Polynomial counters, one step per cycle.
            let b4 = ((self.poly4 >> 3) ^ (self.poly4 >> 2)) & 1;
            self.poly4 = ((self.poly4 << 1) | b4) & 0x0f;
            let b5 = ((self.poly5 >> 4) ^ (self.poly5 >> 2)) & 1;
            self.poly5 = ((self.poly5 << 1) | b5) & 0x1f;
            let b17 = ((self.poly17 >> 16) ^ (self.poly17 >> 11)) & 1;
            self.poly17 = ((self.poly17 << 1) | b17) & 0x1ffff;
            let b9 = ((self.poly9 >> 8) ^ (self.poly9 >> 3)) & 1;
            self.poly9 = ((self.poly9 << 1) | b9) & 0x1ff;
        }
        if self.skctl & 0x04 != 0 {
            self.pot_step();
        }
        let mut base = false;
        if !init {
            self.prescale64 += 1;
            if self.prescale64 >= 28 {
                self.prescale64 = 0;
                if self.audctl & 0x01 == 0 {
                    base = true;
                }
            }
            self.prescale15 += 1;
            if self.prescale15 >= 114 {
                self.prescale15 = 0;
                if self.audctl & 0x01 != 0 {
                    base = true;
                }
                if self.skctl & 0x04 == 0 {
                    self.pot_step();
                }
                if self.skctl & 0x02 != 0 {
                    self.scan_step();
                }
            }
        }
        // Channels 1 and 3 run on their own clock; 2 and 4 on it or, when
        // joined, on the underflows of 1 and 3.
        for low in [0usize, 2] {
            let high = low + 1;
            let low_clock = self.fast(low) || base;
            if self.joined(high) {
                if low_clock && self.clock(high) {
                    self.underflow(high);
                }
            } else {
                if low_clock && self.clock(low) {
                    self.underflow(low);
                }
                if base && self.clock(high) {
                    self.underflow(high);
                }
            }
        }
    }

    fn underflow(&mut self, ch: usize) {
        match ch {
            0 => self.request(IRQ_TIMER1),
            1 => self.request(IRQ_TIMER2),
            2 => self.highpass[0] = self.outputs[0],
            _ => {
                self.highpass[1] = self.outputs[1];
                self.request(IRQ_TIMER4);
                self.serial_clock();
            }
        }
    }

    /// Mixed output of the four channels, 0–60.
    pub fn output(&self) -> u8 {
        let mut sum = 0;
        for ch in 0..4 {
            let c = self.audc[ch];
            let mut out = self.outputs[ch];
            if ch == 0 && self.audctl & 0x04 != 0 {
                out ^= self.highpass[0];
            }
            if ch == 1 && self.audctl & 0x02 != 0 {
                out ^= self.highpass[1];
            }
            if c & 0x10 != 0 || out {
                sum += c & 0x0f;
            }
        }
        sum
    }

    // --- registers -----------------------------------------------------

    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 0x0f {
            r @ 0x00..=0x07 => {
                let n = usize::from(r);
                if self.allpot & (1 << n) != 0 {
                    self.pot_counter
                } else {
                    self.pots[n]
                }
            }
            0x08 => self.allpot,
            0x09 => self.kbcode,
            0x0a => {
                if self.audctl & 0x80 != 0 {
                    self.poly9 as u8
                } else {
                    (self.poly17 >> 9) as u8
                }
            }
            0x0d => self.serin,
            0x0e => !self.active(),
            0x0f => {
                let mut v = 0xff;
                if self.serial_overrun {
                    v &= !0x20;
                }
                if self.shift {
                    v &= !0x08;
                }
                if self.key_down {
                    v &= !0x04;
                }
                v
            }
            _ => 0xff,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 0x0f {
            r @ (0x00 | 0x02 | 0x04 | 0x06) => self.audf[usize::from(r / 2)] = v,
            r @ (0x01 | 0x03 | 0x05 | 0x07) => self.audc[usize::from(r / 2)] = v,
            0x08 => self.audctl = v,
            0x09 => {
                for ch in 0..4 {
                    self.counters[ch] = self.reload(ch);
                }
            }
            0x0a => self.serial_overrun = false,
            0x0b => {
                self.pot_counter = 0;
                self.allpot = 0xff;
            }
            0x0d => {
                self.serout_hold = Some(v);
                if self.shift_bits == 0 {
                    self.load_shift();
                }
            }
            0x0e => {
                self.irqen = v;
                self.pending &= v;
            }
            0x0f => {
                self.skctl = v;
                if v & 0x03 == 0 {
                    self.poly4 = 0x0f;
                    self.poly5 = 0x1f;
                    self.poly17 = 0x1ffff;
                    self.poly9 = 0x1ff;
                    self.prescale64 = 0;
                    self.prescale15 = 0;
                    self.key_down = false;
                    self.compare = None;
                }
            }
            _ => {}
        }
    }
}

const TAG: [u8; 4] = *b"POKY";
/// Version 2: sound polynomials, high-pass latches, pots.
const VERSION: u16 = 2;

impl Savestate for Pokey {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.audf);
            w.raw(&self.audc);
            for v in [
                self.audctl,
                self.skctl,
                self.irqen,
                self.pending,
                self.prescale64,
                self.prescale15,
                self.kbcode,
                self.scan,
                self.compare.unwrap_or(0xff),
                self.serout_hold.map_or(0, |_| 1),
                self.serout_hold.unwrap_or(0),
                self.shifting,
                self.shift_bits,
                self.serin,
            ] {
                w.u8(v);
            }
            for c in self.counters {
                w.u32(c);
            }
            for v in self.outputs.iter().chain(&[
                self.shift,
                self.control,
                self.key_down,
                self.shift_phase,
                self.serial_overrun,
            ]) {
                w.bool(*v);
            }
            w.u32(self.poly17);
            w.u16(self.poly9);
            w.u64(self.keys);
            w.u16(self.shift_reg);
            w.bytes(&self.sent);
            w.u8(self.poly4);
            w.u8(self.poly5);
            w.bool(self.highpass[0]);
            w.bool(self.highpass[1]);
            w.raw(&self.pot_input);
            w.raw(&self.pots);
            w.u8(self.pot_counter);
            w.u8(self.allpot);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        c.raw_into(&mut n.audf)?;
        c.raw_into(&mut n.audc)?;
        n.audctl = c.u8()?;
        n.skctl = c.u8()?;
        n.irqen = c.u8()?;
        n.pending = c.u8()?;
        n.prescale64 = c.u8()?;
        n.prescale15 = c.u8()?;
        n.kbcode = c.u8()?;
        n.scan = c.u8()? & 0x3f;
        n.compare = Some(c.u8()?).filter(|&v| v < 64);
        let has_hold = c.u8()? != 0;
        let hold = c.u8()?;
        n.serout_hold = has_hold.then_some(hold);
        n.shifting = c.u8()?;
        n.shift_bits = c.u8()?;
        n.serin = c.u8()?;
        for v in n.counters.iter_mut() {
            *v = c.u32()?;
        }
        for v in n.outputs.iter_mut() {
            *v = c.bool()?;
        }
        n.shift = c.bool()?;
        n.control = c.bool()?;
        n.key_down = c.bool()?;
        n.shift_phase = c.bool()?;
        n.serial_overrun = c.bool()?;
        n.poly17 = c.u32()? & 0x1ffff;
        n.poly9 = c.u16()? & 0x1ff;
        n.keys = c.u64()?;
        n.shift_reg = c.u16()?;
        n.sent = c.bytes()?.to_vec();
        if version >= 2 {
            n.poly4 = c.u8()? & 0x0f;
            n.poly5 = c.u8()? & 0x1f;
            n.highpass = [c.bool()?, c.bool()?];
            c.raw_into(&mut n.pot_input)?;
            c.raw_into(&mut n.pots)?;
            n.pot_counter = c.u8()?;
            n.allpot = c.u8()?;
        }
        if n.prescale64 >= 28 || n.prescale15 >= 114 || n.shift_bits > SERIAL_BITS {
            return Err(StateError::Invalid("POKEY counters"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
