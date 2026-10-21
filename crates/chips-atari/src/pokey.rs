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
//!
//! Timers (fitted to Acid800 `pokey_timertiming`, which measures them to
//! the cycle): a counter that reaches 0 commits its underflow; 3 cycles
//! later it takes effect — IRQ, output change, reload from AUDF as it was
//! the cycle before. A STIMER in between does not stop it. Periods: AUDF+1
//! base ticks; AUDF+4 cycles at 1.79 MHz (first period after STIMER the
//! same). Joined pairs: the low channel keeps its own IRQ; each of its
//! effects counts the high channel, the low counter running through 256
//! values until the high channel commits; the pair reloads when that takes
//! effect (16-bit period at 1.79 MHz: AUDF16+7).
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
//! Polynomial counters (fitted to Acid800 `pokey_noise`, which measures
//! RANDOM at exact cycles): all shift right one step per cycle with the
//! XOR of bit 0 and a tap fed into the top bit — 9-bit tap 5, 17-bit tap 5,
//! 5-bit tap 2, 4-bit tap 1. RANDOM reads the 9-bit counter's bits 0–7 or
//! the 17-bit counter's bits 8–15. Initialization mode (SKCTL bits 0–1 = 0)
//! does not reset them: they keep shifting with 1s fed in.
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
    /// AUDF as the counters see it (one cycle behind the register).
    audf_latched: [u8; 4],
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
    /// Cycles until a joined high channel's underflow takes effect.
    delayed: [u8; 4],
    /// A joined low channel waiting for the pair's reload.
    hold: [bool; 4],
    /// A committed underflow from before the last STIMER.
    after_stimer: [bool; 4],
    /// Cycles until a late reload (two-tone channel 1).
    reload_wait: [u8; 4],
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
    /// Cycles until an idle shifter takes the holding register.
    load_wait: u8,
    /// Bytes sent completely (for SIO devices), oldest first.
    sent: Vec<u8>,
    pub serin: u8,
    serial_overrun: bool,
    /// Serial input line (true = mark) and a byte being received.
    serial_line: bool,
    serial_busy: bool,
    /// SKCTL was written while a byte came in: its receive logic restarted
    /// and that byte is lost.
    rx_discard: bool,
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
            audf_latched: [0; 4],
            audc: [0; 4],
            audctl: 0,
            skctl: 0,
            irqen: 0,
            pending: 0,
            counters: [0; 4],
            outputs: [false; 4],
            delayed: [0; 4],
            hold: [false; 4],
            after_stimer: [false; 4],
            reload_wait: [0; 4],
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
            load_wait: 0,
            sent: Vec::new(),
            serin: 0xff,
            serial_overrun: false,
            serial_line: true,
            serial_busy: false,
            rx_discard: false,
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

    /// "Serial output finished": the shift register is empty (a byte
    /// waiting in SEROUT with a stopped clock does not count, Acid800
    /// `pokey_serclock`).
    fn serout_done(&self) -> bool {
        self.shift_bits == 0
    }

    /// IRQST bits (1 = active): the latched requests and the level of
    /// "serial output finished", which shows even while disabled
    /// (Acid800 `pokey_seroc`).
    fn active(&self) -> u8 {
        let done = if self.serout_done() {
            IRQ_SEROUT_DONE
        } else {
            0
        };
        self.pending | done
    }

    /// The /IRQ output: enabled requests only.
    pub fn irq(&self) -> bool {
        self.active() & (self.irqen | !IRQ_SEROUT_DONE) != 0
    }

    // --- serial --------------------------------------------------------

    /// Bytes shifted out completely since the last call.
    pub fn take_sent(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.sent)
    }

    /// The serial input line as the sender drives it (SKSTAT bit 4) and
    /// whether a byte is being shifted in (SKSTAT bit 1 low). The cassette
    /// handler measures the baud rate from the line's edges.
    pub fn set_serial_line(&mut self, level: bool, busy: bool) {
        self.serial_line = level;
        self.serial_busy = busy;
    }

    /// A byte arrived on the serial input (SIO device → computer).
    pub fn receive(&mut self, v: u8) {
        if std::mem::take(&mut self.rx_discard) {
            return;
        }
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
        // An idle shifter takes the holding register on a clock edge, not
        // when SEROUT is written (Acid800 `pokey_serclock`), 2 cycles after
        // the edge (`pokey_sertiming`).
        if self.shift_bits == 0 {
            if self.serout_hold.is_some() {
                self.load_wait = 2;
            }
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
        // SKCTL bit 7 forces a break (0).
        self.skctl & 0x80 == 0 && (self.shift_bits == 0 || self.shift_reg & 1 != 0)
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

    /// Restart a counter (STIMER): a 1.79 MHz channel's first period is
    /// AUDF+4 cycles, like the others.
    fn restart(&mut self, ch: usize) {
        let joined_high = (ch == 1 || ch == 3) && self.joined(ch);
        self.counters[ch] = self.audf_of(ch) + if !joined_high && self.fast(ch) { 4 } else { 0 };
        self.hold[ch] = false;
        self.after_stimer[ch] = self.delayed[ch] > 0;
        self.reload_wait[ch] = 0;
    }

    /// AUDF of `ch` as the counter loads it (one cycle behind writes: a
    /// write in the reload cycle itself is too late).
    fn audf_of(&self, ch: usize) -> u32 {
        u32::from(self.audf_latched[ch])
    }

    /// Count channel `ch` once. A counter at 0 commits its underflow; the
    /// underflow takes effect 3 cycles later ([`Pokey::effect`]).
    fn count(&mut self, ch: usize) {
        // A counter waits at 0 for its reload, unless a STIMER restarted it
        // meanwhile.
        let waiting = self.delayed[ch] > 0 && !self.after_stimer[ch];
        if waiting || self.hold[ch] || self.reload_wait[ch] > 0 {
            return;
        }
        if self.counters[ch] > 0 {
            self.counters[ch] -= 1;
        } else if self.delayed[ch] == 0 {
            self.delayed[ch] = 3;
        }
    }

    /// An underflow takes effect: output, IRQ, reload (and, for a joined
    /// low channel, the high channel's count). Returns whether it reloaded
    /// a joined pair.
    fn effect(&mut self, ch: usize) -> bool {
        self.distort(ch);
        self.underflow(ch);
        // Two-tone mode (SKCTL bit 3, channels 1+2 unjoined): timer 2
        // resynchronises timer 1 — reloads it and drops an underflow it
        // committed in the cycle before; timer 1 resets timer 2 while the
        // serial output is 1 (Acid800 `pokey_timertiming` two-tone
        // cancellation, `pokey_twotone`).
        if self.skctl & 0x08 != 0 && !self.joined(1) {
            let other = 1 - ch.min(1);
            if ch == 1 || (ch == 0 && self.serial_out()) {
                if self.delayed[other] == 3 {
                    self.delayed[other] = 0;
                }
                self.counters[other] = self.audf_of(other);
                self.reload_wait[other] = 0;
            }
        }
        // Committed before a STIMER: the IRQ still happens, the counters
        // keep their restart.
        if std::mem::take(&mut self.after_stimer[ch]) {
            return false;
        }
        let joined_high = (ch == 1 || ch == 3) && self.joined(ch);
        let joined_low = (ch == 0 || ch == 2) && self.joined(ch + 1);
        if joined_high {
            if ch == 1 && self.skctl & 0x08 != 0 {
                // Two-tone: the 1+2 pair reloads 2 cycles later.
                self.reload_wait[0] = 2;
                self.reload_wait[1] = 2;
                return false;
            }
            // The pair reloads together; the low channel was waiting.
            self.counters[ch] = self.audf_of(ch);
            self.counters[ch - 1] = self.audf_of(ch - 1);
            self.hold[ch - 1] = false;
            return true;
        } else if joined_low {
            let high = ch + 1;
            if self.counters[high] == 0 {
                // The high channel underflows; the low one waits for the
                // pair's reload.
                self.delayed[high] = 3;
                self.hold[ch] = true;
            } else {
                self.counters[high] -= 1;
                // Without a reload the low counter runs through all 256
                // values.
                self.counters[ch] = if self.fast(ch) { 252 } else { 255 };
            }
        } else if ch == 0 && self.skctl & 0x08 != 0 {
            // Two-tone mode: channel 1 reloads 2 cycles later (periods 2
            // cycles longer after the first, Acid800 `pokey_timertiming`).
            self.reload_wait[0] = 2;
        } else {
            self.counters[ch] = self.audf_of(ch);
        }
        false
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
        self.tick_counters();
        self.audf_latched = self.audf;
    }

    fn tick_counters(&mut self) {
        let init = self.skctl & 0x03 == 0;
        // Polynomial counters, one step per cycle, shifting right with the
        // feedback into the top bit. In initialization mode they keep
        // shifting with 1s fed in, so they fill with ones (RANDOM reads $FF
        // once settled, but not right after entering it).
        let fb = |taps: u32| if init { 1 } else { taps & 1 };
        self.poly4 =
            (self.poly4 >> 1) | ((fb(u32::from(self.poly4 ^ (self.poly4 >> 1))) as u8) << 3);
        self.poly5 =
            (self.poly5 >> 1) | ((fb(u32::from(self.poly5 ^ (self.poly5 >> 2))) as u8) << 4);
        self.poly9 =
            (self.poly9 >> 1) | ((fb(u32::from(self.poly9 ^ (self.poly9 >> 5))) as u16) << 8);
        self.poly17 = (self.poly17 >> 1) | (fb(self.poly17 ^ (self.poly17 >> 5)) << 16);
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
        // A clock edge loads the idle shifter after a delay.
        if self.load_wait > 0 {
            self.load_wait -= 1;
            if self.load_wait == 0 {
                self.load_shift();
            }
        }
        // Pending underflows take effect after their 3-cycle delay; a
        // channel that reloads in this cycle does not count in it too.
        let mut fired = [false; 4];
        for (ch, fired) in fired.iter_mut().enumerate() {
            if self.reload_wait[ch] > 0 {
                self.reload_wait[ch] -= 1;
                if self.reload_wait[ch] == 0 {
                    self.counters[ch] = self.audf_of(ch);
                    self.hold[ch] = false;
                    *fired = true;
                }
            }
        }
        // Within a pair the high channel first (a low channel's effect
        // starts its high channel's delay, which must not count down in the
        // same cycle); channels 1/2 before 3/4 (the high-pass latches sample
        // their new outputs).
        for ch in [1, 0, 3, 2] {
            if self.delayed[ch] > 0 {
                self.delayed[ch] -= 1;
                if self.delayed[ch] == 0 {
                    // After a STIMER the counter keeps counting.
                    let restarted = self.after_stimer[ch];
                    let pair = self.effect(ch);
                    fired[ch] = !restarted;
                    // A joined pair's reload also restarts the low channel.
                    if pair {
                        fired[ch - 1] = true;
                    }
                }
            }
        }
        // Asynchronous receive mode (SKCTL bit 4) waiting for a start bit
        // keeps channels 3 and 4 at their reload values (Acid800
        // `pokey_asyncrecv`).
        if self.skctl & 0x10 != 0 && !self.serial_busy {
            for ch in [2, 3] {
                self.restart(ch);
            }
        }
        // Channels 1 and 3 run on their own clock (1.79 MHz or the base
        // clock); 2 and 4 on the base clock or, joined, on the effects of 1
        // and 3.
        for low in [0usize, 2] {
            if (self.fast(low) || base) && !fired[low] {
                self.count(low);
            }
            if !self.joined(low + 1) && base && !fired[low + 1] {
                self.count(low + 1);
            }
        }
    }

    fn underflow(&mut self, ch: usize) {
        match ch {
            0 => self.request(IRQ_TIMER1),
            1 => {
                self.request(IRQ_TIMER2);
                if self.output_clock() == Some(1) {
                    self.serial_clock();
                }
            }
            2 => self.highpass[0] = self.outputs[0],
            _ => {
                self.highpass[1] = self.outputs[1];
                self.request(IRQ_TIMER4);
                if self.output_clock() == Some(3) {
                    self.serial_clock();
                }
            }
        }
    }

    /// The channel clocking the serial output (SKCTL bits 6–5): none
    /// (external clock), channel 4, or channel 2 (Acid800 `pokey_serclock`).
    fn output_clock(&self) -> Option<usize> {
        match (self.skctl >> 5) & 3 {
            0 => None,
            3 => Some(1),
            _ => Some(3),
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
                // 9-bit: its low 8 bits; 17-bit: bits 8–15.
                if self.audctl & 0x80 != 0 {
                    self.poly9 as u8
                } else {
                    (self.poly17 >> 8) as u8
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
                if !self.serial_line {
                    v &= !0x10;
                }
                if self.serial_busy {
                    v &= !0x02;
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
                // STIMER restarts the counters; a committed underflow still
                // takes effect.
                for ch in 0..4 {
                    self.restart(ch);
                }
            }
            0x0a => self.serial_overrun = false,
            0x0b => {
                self.pot_counter = 0;
                self.allpot = 0xff;
            }
            0x0d => {
                self.serout_hold = Some(v);
            }
            0x0e => {
                self.irqen = v;
                self.pending &= v;
            }
            0x0f => {
                // A new serial mode (bits 4–6) restarts the receiver: a
                // byte in progress is lost (the cassette handler switches
                // modes after measuring the baud rate on the sync bytes).
                if self.serial_busy && (self.skctl ^ v) & 0x70 != 0 {
                    self.rx_discard = true;
                }
                self.skctl = v;
                if v & 0x03 == 0 {
                    // Initialization holds the clock dividers at a phase
                    // that gives the first 64 kHz tick 21 cycles, the first
                    // 15 kHz tick 80 cycles after leaving it (Acid800
                    // `pokey_inittiming`, `pokey_timertiming`).
                    self.prescale64 = 6;
                    self.prescale15 = 33;
                    // and stops a transmission in progress (Acid800
                    // `pokey_serclock` expects SEROC afterwards).
                    self.shift_bits = 0;
                    self.key_down = false;
                    self.compare = None;
                }
            }
            _ => {}
        }
    }
}

const TAG: [u8; 4] = *b"POKY";
/// Version 2: sound polynomials, high-pass latches, pots; 3: serial input
/// line; 4: receiver restart; 5: joined-timer delay.
const VERSION: u16 = 5;

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
            w.bool(self.serial_line);
            w.bool(self.serial_busy);
            w.bool(self.rx_discard);
            w.raw(&self.delayed);
            w.raw(&self.audf_latched);
            for h in self.hold.iter().chain(&self.after_stimer) {
                w.bool(*h);
            }
            w.raw(&self.reload_wait);
            w.u8(self.load_wait);
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
        if version >= 3 {
            n.serial_line = c.bool()?;
            n.serial_busy = c.bool()?;
        }
        if version >= 4 {
            n.rx_discard = c.bool()?;
        }
        if version >= 5 {
            c.raw_into(&mut n.delayed)?;
            c.raw_into(&mut n.audf_latched)?;
            for h in n.hold.iter_mut().chain(n.after_stimer.iter_mut()) {
                *h = c.bool()?;
            }
            c.raw_into(&mut n.reload_wait)?;
            n.load_wait = c.u8()?;
        } else {
            n.audf_latched = n.audf;
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
