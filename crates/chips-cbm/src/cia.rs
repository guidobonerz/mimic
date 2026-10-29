//! MOS 6526 / 6526A (8521) Complex Interface Adapter.
//!
//! Registers (offset from the chip base, e.g. `$DC00` for C64 CIA1):
//!
//! | Off | Name   | Function                                                    |
//! | --- | ------ | ----------------------------------------------------------- |
//! | 0   | PRA    | port A data                                                 |
//! | 1   | PRB    | port B data (PB6/PB7 may carry timer outputs)               |
//! | 2   | DDRA   | port A direction, 1 = output                                |
//! | 3   | DDRB   | port B direction                                            |
//! | 4/5 | TA     | timer A: read counter, write latch (hi write loads if stopped)|
//! | 6/7 | TB     | timer B                                                     |
//! | 8   | TOD10  | time of day, tenths (BCD); reading/writing it releases latch/stop |
//! | 9   | TODSEC | seconds (BCD)                                               |
//! | A   | TODMIN | minutes (BCD)                                               |
//! | B   | TODHR  | hours 1–12 (BCD), bit 7 = PM; reading latches, writing stops |
//! | C   | SDR    | serial data register                                        |
//! | D   | ICR    | read: flags (bit 7 = IRQ), clears; write: mask set/clear    |
//! | E   | CRA    | 0 start, 1 PB6 on, 2 toggle, 3 one-shot, 4 force load (strobe), 5 CNT input, 6 SP output, 7 TOD 50 Hz |
//! | F   | CRB    | 0 start, 1 PB7 on, 2 toggle, 3 one-shot, 4 force load, 5–6 input (φ2, CNT, TA, TA+CNT), 7 TOD alarm write |
//!
//! The 8520 ([`CiaModel::Mos8520`]) differs in the TOD (a 24-bit event
//! counter: reading A latches until 8 is read, writing A stops counting
//! until 8 is written) and in one-shot starts.
//!
//! Sources: MOS 6526 data sheet; Amiga Hardware Reference Manual,
//! appendix F (8520); Wolfgang Lorenz, "A Software Model of the
//! CIA6526" (timer pipeline, reload, one-shot and interrupt timing). The
//! implementation is this project's own.
//!
//! Timing model: [`Cia::tick`] runs once per φ2 cycle *before* the CPU's bus
//! access of that cycle, so a read in the cycle of an underflow sees the new
//! flag (and cancels the interrupt, as on the real chip).
//!
//! Each timer has a small delay pipeline. A count event enters at stage 1
//! (φ2 or cascade) or stage 0 (CNT edge, one extra cycle of input
//! synchronisation) and moves one stage per cycle. Stage 2 checks for
//! underflow (counter 0 with a clock waiting), stage 3 decrements. A reload,
//! by underflow or force load, removes the clock that was about to
//! decrement, which gives the period of latch + 1 cycles.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Chip revision. They differ in interrupt timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CiaModel {
    /// Original 6526 (C64 until 1986): IRQ output one cycle after the flag.
    #[default]
    Mos6526,
    /// 6526A / 8521 (C64C, C128): IRQ output in the same cycle as the flag.
    Mos6526A,
    /// MOS 8520 (Amiga): 6526A timing; the time-of-day clock is a 24-bit
    /// binary event counter (registers 8–A, low byte first; B unused) and a
    /// timer high-byte write in one-shot mode starts the timer
    /// (Amiga Hardware Reference Manual, appendix F "8520 CIA").
    Mos8520,
}

pub mod icr {
    pub const TA: u8 = 0x01;
    pub const TB: u8 = 0x02;
    pub const ALARM: u8 = 0x04;
    pub const SP: u8 = 0x08;
    pub const FLAG: u8 = 0x10;
    pub const IRQ: u8 = 0x80;
}

const CR_START: u8 = 0x01;
const CR_PBON: u8 = 0x02;
const CR_TOGGLE: u8 = 0x04;
const CR_ONESHOT: u8 = 0x08;
const CR_LOAD: u8 = 0x10;

// Timer pipeline bits.
const COUNT0: u8 = 0x01;
const COUNT1: u8 = 0x02;
const COUNT2: u8 = 0x04;
const COUNT3: u8 = 0x08;
/// Force load: written into LOAD0, performed when it reaches LOAD1 one
/// cycle later, i.e. two cycles after the write (Lorenz LoadA0/LoadA1).
const LOAD1: u8 = 0x10;
const LOAD0: u8 = 0x40;
/// One-shot bit as it was in the previous cycle (not shifted).
const ONESHOT0: u8 = 0x20;
/// Count stages that move on by the shift; inputs are OR-ed in afterwards.
const SHIFT_KEEP: u8 = COUNT1 | COUNT2 | COUNT3;

/// What feeds a timer's count input this cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Phi2,
    CntEdge,
    Cascade,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timer {
    pub counter: u16,
    pub latch: u16,
    /// Control register without the force-load strobe.
    pub cr: u8,
    delay: u8,
    /// Toggle-mode output flip-flop (PB6/PB7).
    toggle: bool,
    /// Pulse-mode output: high for the cycle after an underflow.
    pulse: bool,
    /// The counter was reloaded from the latch in this cycle; a latch
    /// write in the same cycle still reaches the counter (Lorenz cia1ta #08).
    reloaded: bool,
}

impl Timer {
    fn new() -> Self {
        Self {
            counter: 0xffff,
            latch: 0xffff,
            ..Self::default()
        }
    }

    /// Advance one cycle. `input` is this cycle's count event. Returns true
    /// on underflow.
    ///
    /// Order as in Lorenz' model: shift the pipeline and feed the new
    /// input first, then act on the stages.
    fn tick(&mut self, input: Input) -> bool {
        self.pulse = false;
        self.reloaded = false;
        let load = self.delay & LOAD1 != 0;
        let load_next = self.delay & LOAD0 != 0;
        let oneshot_before = self.delay & ONESHOT0 != 0;
        let mut d = (self.delay << 1) & SHIFT_KEEP;
        match input {
            Input::Phi2 | Input::Cascade => d |= COUNT1,
            Input::CntEdge => d |= COUNT0,
            Input::None => {}
        }
        if load_next {
            d |= LOAD1;
        }
        self.delay = d;

        if self.delay & COUNT3 != 0 {
            self.counter = self.counter.wrapping_sub(1);
        }
        let mut underflow = false;
        if self.counter == 0 && self.delay & COUNT2 != 0 {
            underflow = true;
            self.counter = self.latch;
            self.reloaded = true;
            // The clock behind the underflow never reaches the decrement stage.
            self.delay &= !COUNT2;
            self.toggle = !self.toggle;
            self.pulse = true;
            if self.cr & CR_ONESHOT != 0 || oneshot_before {
                self.cr &= !CR_START;
                self.delay &= !(COUNT0 | COUNT1);
            }
        }
        if load {
            // Like an underflow reload, a force load removes the next clock
            // (Lorenz cia1ta #05/#0d, irq).
            self.counter = self.latch;
            self.reloaded = true;
            self.delay &= !(COUNT2 | LOAD1);
        }
        if self.cr & CR_ONESHOT != 0 {
            self.delay |= ONESHOT0;
        }
        underflow
    }

    fn write_cr(&mut self, v: u8) {
        if v & CR_START != 0 && self.cr & CR_START == 0 {
            // A rising edge on START sets the toggle output high.
            self.toggle = true;
        }
        if v & CR_LOAD != 0 {
            self.delay |= LOAD0;
        }
        self.cr = v & !CR_LOAD;
    }

    fn write_hi(&mut self, v: u8) {
        self.latch = (self.latch & 0x00ff) | (u16::from(v) << 8);
        if self.reloaded {
            self.counter = self.latch;
        }
        if self.cr & CR_START == 0 {
            self.delay |= LOAD0;
        }
    }

    fn write_lo(&mut self, v: u8) {
        self.latch = (self.latch & 0xff00) | u16::from(v);
        if self.reloaded {
            self.counter = self.latch;
        }
    }

    /// Level of the PB6/PB7 timer output.
    fn output(&self) -> bool {
        if self.cr & CR_TOGGLE != 0 {
            self.toggle
        } else {
            self.pulse
        }
    }

    fn save(&self, w: &mut StateWriter) {
        w.u16(self.counter);
        w.u16(self.latch);
        w.u8(self.cr);
        w.u8(self.delay);
        w.bool(self.toggle);
        w.bool(self.pulse);
        w.bool(self.reloaded);
    }

    fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let t = Self {
            counter: r.u16()?,
            latch: r.u16()?,
            cr: r.u8()?,
            delay: r.u8()?,
            toggle: r.bool()?,
            pulse: r.bool()?,
            reloaded: r.bool()?,
        };
        if t.cr & CR_LOAD != 0 || t.delay & !(SHIFT_KEEP | COUNT0 | LOAD0 | LOAD1 | ONESHOT0) != 0 {
            return Err(StateError::Invalid("CIA timer state"));
        }
        Ok(t)
    }
}

/// Time-of-day clock: BCD tenths, seconds, minutes, hours with AM/PM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tod {
    /// [tenths, seconds, minutes, hours]
    pub time: [u8; 4],
    pub alarm: [u8; 4],
    /// Snapshot while the hours register has been read and tenths not yet.
    latch: Option<[u8; 4]>,
    /// Stopped by a write to hours until tenths is written.
    stopped: bool,
    /// Power-line ticks within the current tenth.
    divider: u8,
    /// Output of the time = alarm comparator; the alarm flag rises with it
    /// (VICE test programs CIA/tod/4tod, 5tod: rewriting the time it
    /// already has raises nothing).
    equal: bool,
}

impl Default for Tod {
    /// Power-up and reset: 1:00:00.0, alarm 0:00:00.0, the clock stopped
    /// until tenths are written (VICE test program CIA/tod/powerup).
    fn default() -> Self {
        Self {
            time: [0, 0, 0, 0x01],
            alarm: [0; 4],
            latch: None,
            stopped: true,
            divider: 0,
            equal: false,
        }
    }
}

/// One count of a seconds or minutes register. The digits are binary
/// counters; the carry to the next digit and register is a comparison by
/// value, so invalid values count on within the digit's bits (VICE test
/// programs CIA/tod/fix-sec, fix-min). Returns the new value and the carry.
fn sexagesimal_inc(v: u8) -> (u8, bool) {
    if v == 0x59 {
        (0, true)
    } else if v & 0x0f == 9 {
        ((v + 0x10) & 0x70, false)
    } else {
        ((v & 0x70) | ((v + 1) & 0x0f), false)
    }
}

/// One count of the hours register (bit 7 PM): 11 turns to 12 and flips
/// PM, 12 turns to 1; otherwise digits count like the others, the tens
/// digit only from 09 (VICE test programs CIA/tod/fix-hour, hour-test).
fn hours_inc(v: u8) -> u8 {
    let pm = v & 0x80;
    match v & 0x1f {
        0x11 => 0x12 | (pm ^ 0x80),
        0x12 => 0x01 | pm,
        0x09 => 0x10 | pm,
        h => (h & 0x10) | ((h + 1) & 0x0f) | pm,
    }
}

impl Tod {
    /// 8520: one count pulse of the 24-bit counter (`time[0..3]`, low byte
    /// first). Returns true if the alarm matched.
    fn count(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        let v = (u32::from(self.time[2]) << 16
            | u32::from(self.time[1]) << 8
            | u32::from(self.time[0]))
        .wrapping_add(1)
            & 0xff_ffff;
        self.time = [v as u8, (v >> 8) as u8, (v >> 16) as u8, 0];
        self.compare()
    }

    /// Update the comparator; true on its rising edge.
    fn compare(&mut self) -> bool {
        let equal = self.time == self.alarm;
        let rise = equal && !self.equal;
        self.equal = equal;
        rise
    }

    /// 8520 register read: reading the high byte (A) latches the counter
    /// until the low byte (8) is read.
    fn read_counter(&mut self, reg: u8) -> u8 {
        if reg == 0x0a {
            self.latch = Some(self.time);
        }
        let v = self.peek_counter(reg);
        if reg == 0x08 {
            self.latch = None;
        }
        v
    }

    fn peek_counter(&self, reg: u8) -> u8 {
        match reg {
            0x08..=0x0a => self.latch.unwrap_or(self.time)[usize::from(reg - 8)],
            _ => 0,
        }
    }

    /// 8520 register write: the high byte stops the counter until the low
    /// byte is written. Returns true if the write made time match alarm.
    fn write_counter(&mut self, reg: u8, v: u8, alarm: bool) -> bool {
        if reg == 0x0b {
            return false;
        }
        let i = usize::from(reg - 8);
        if alarm {
            self.alarm[i] = v;
        } else {
            self.time[i] = v;
            match reg {
                0x0a => self.stopped = true,
                0x08 => self.stopped = false,
                _ => {}
            }
        }
        self.compare()
    }

    /// One power-line period. Returns true if the alarm matched.
    fn tick(&mut self, fifty_hz: bool) -> bool {
        if self.stopped {
            return false;
        }
        // The divider counts 0–6 and wraps; it is compared with 5 (50 Hz)
        // or 6 (60 Hz) only on a tick, so switching to 50 Hz when it already
        // passed 5 makes one long tenth (VICE test program CIA/tod/hzsync6).
        self.divider = if self.divider >= 6 {
            0
        } else {
            self.divider + 1
        };
        if self.divider != if fifty_hz { 5 } else { 6 } {
            return false;
        }
        self.divider = 0;
        let t = &mut self.time;
        if t[0] == 9 {
            t[0] = 0;
            let (sec, carry) = sexagesimal_inc(t[1]);
            t[1] = sec;
            if carry {
                let (min, carry) = sexagesimal_inc(t[2]);
                t[2] = min;
                if carry {
                    t[3] = hours_inc(t[3]);
                }
            }
        } else {
            t[0] = (t[0] + 1) & 0x0f;
        }
        self.compare()
    }

    fn read(&mut self, reg: u8) -> u8 {
        let i = usize::from(reg - 8);
        // Reading the hours latches the time; further reads of the hours
        // do not latch again until tenths are read (CIA/tod/read-latch).
        if reg == 0x0b && self.latch.is_none() {
            self.latch = Some(self.time);
        }
        let v = self.latch.unwrap_or(self.time)[i];
        if reg == 0x08 {
            self.latch = None;
        }
        v
    }

    fn peek(&self, reg: u8) -> u8 {
        self.latch.unwrap_or(self.time)[usize::from(reg - 8)]
    }

    /// Returns true if the write made the time match the alarm.
    fn write(&mut self, reg: u8, v: u8, alarm: bool) -> bool {
        let i = usize::from(reg - 8);
        let v = match reg {
            0x08 => v & 0x0f,
            0x09 | 0x0a => v & 0x7f,
            // Writing 12 to the hours of the clock flips AM/PM
            // (CIA/tod/hour-test).
            _ if !alarm && v & 0x1f == 0x12 => (v & 0x9f) ^ 0x80,
            _ => v & 0x9f,
        };
        if alarm {
            self.alarm[i] = v;
        } else {
            self.time[i] = v;
            match reg {
                0x0b => self.stopped = true,
                // The divider restarts only with a stopped clock (CIA/tod/
                // hzsync0, hzsync1, hzsync3).
                0x08 if self.stopped => {
                    self.stopped = false;
                    self.divider = 0;
                }
                _ => {}
            }
        }
        self.compare()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Serial {
    sdr: u8,
    shift: u8,
    /// Remaining half-bits (output) or bits (input) of the current byte.
    count: u8,
    /// A byte written while shifting waits here.
    pending: bool,
    cnt_out: bool,
    sp_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cia {
    model: CiaModel,
    pub pra: u8,
    pub prb: u8,
    pub ddra: u8,
    pub ddrb: u8,
    pub ta: Timer,
    pub tb: Timer,
    pub tod: Tod,
    serial: Serial,
    /// Interrupt flags (ICR read value without bit 7).
    flags: u8,
    mask: u8,
    irq: bool,
    /// Old 6526: IRQ output rises at the next tick.
    irq_next: bool,
    /// An ICR mask write takes effect at the next tick (Lorenz imr).
    mask_written: bool,
    cnt_in: bool,
    cnt_edge: bool,
    flag_in: bool,
}

impl Cia {
    pub fn new(model: CiaModel) -> Self {
        let tod = if model == CiaModel::Mos8520 {
            Tod {
                time: [0; 4],
                stopped: false,
                ..Tod::default()
            }
        } else {
            Tod::default()
        };
        Self {
            model,
            pra: 0,
            prb: 0,
            ddra: 0,
            ddrb: 0,
            ta: Timer::new(),
            tb: Timer::new(),
            tod,
            serial: Serial::default(),
            flags: 0,
            mask: 0,
            irq: false,
            irq_next: false,
            mask_written: false,
            cnt_in: true,
            cnt_edge: false,
            flag_in: true,
        }
    }

    pub fn model(&self) -> CiaModel {
        self.model
    }

    /// /RES: registers to their power-on values.
    pub fn reset(&mut self) {
        *self = Self::new(self.model);
    }

    /// IRQ output (true = asserted).
    pub fn irq(&self) -> bool {
        self.irq
    }

    /// Port A pin levels driven by the CIA: outputs from PRA, inputs pulled up.
    pub fn port_a_out(&self) -> u8 {
        self.pra | !self.ddra
    }

    /// Port B pin levels driven by the CIA, including PB6/PB7 timer outputs.
    pub fn port_b_out(&self) -> u8 {
        let mut v = self.prb | !self.ddrb;
        if self.ta.cr & CR_PBON != 0 {
            v = (v & !0x40) | (u8::from(self.ta.output()) << 6);
        }
        if self.tb.cr & CR_PBON != 0 {
            v = (v & !0x80) | (u8::from(self.tb.output()) << 7);
        }
        v
    }

    /// CNT pin input level (pulled high when undriven).
    pub fn set_cnt(&mut self, level: bool) {
        if level && !self.cnt_in {
            self.cnt_edge = true;
        }
        self.cnt_in = level;
    }

    /// /FLAG input; a falling edge sets ICR bit 4.
    pub fn set_flag(&mut self, level: bool) {
        if !level && self.flag_in {
            self.raise(icr::FLAG);
        }
        self.flag_in = level;
    }

    /// CNT and SP outputs while the serial port shifts out.
    pub fn serial_out(&self) -> (bool, bool) {
        (self.serial.cnt_out, self.serial.sp_out)
    }

    /// One period of the TOD input (50 or 60 Hz power line; on the 8520 one
    /// count pulse, e.g. VSYNC or HSYNC on the Amiga).
    pub fn tod_tick(&mut self) {
        let matched = if self.model == CiaModel::Mos8520 {
            self.tod.count()
        } else {
            self.tod.tick(self.ta.cr & 0x80 != 0)
        };
        if matched {
            self.raise(icr::ALARM);
        }
    }

    fn raise(&mut self, bits: u8) {
        self.flags |= bits;
        if self.flags & self.mask != 0 && !self.irq {
            match self.model {
                CiaModel::Mos6526 => self.irq_next = true,
                CiaModel::Mos6526A | CiaModel::Mos8520 => self.irq = true,
            }
        }
    }

    /// One φ2 cycle, before the CPU's bus access in that cycle.
    pub fn tick(&mut self) {
        if std::mem::take(&mut self.irq_next) {
            self.irq = true;
        }
        if std::mem::take(&mut self.mask_written) {
            // Enabling a source whose flag is already set raises IRQ.
            self.raise(0);
        }
        let cnt_edge = std::mem::take(&mut self.cnt_edge);

        let ta_input = match (self.ta.cr & CR_START != 0, self.ta.cr & 0x20 != 0) {
            (false, _) => Input::None,
            (true, false) => Input::Phi2,
            (true, true) if cnt_edge => Input::CntEdge,
            (true, true) => Input::None,
        };
        let ta_under = self.ta.tick(ta_input);

        let tb_input = if self.tb.cr & CR_START == 0 {
            Input::None
        } else {
            match (self.tb.cr >> 5) & 3 {
                0 => Input::Phi2,
                1 if cnt_edge => Input::CntEdge,
                2 if ta_under => Input::Cascade,
                3 if ta_under && self.cnt_in => Input::Cascade,
                _ => Input::None,
            }
        };
        let tb_under = self.tb.tick(tb_input);

        if ta_under {
            self.serial_ta_underflow();
            self.raise(icr::TA);
        }
        if tb_under {
            self.raise(icr::TB);
        }
        if cnt_edge && self.ta.cr & 0x40 == 0 {
            self.serial_shift_in();
        }
    }

    fn serial_ta_underflow(&mut self) {
        if self.ta.cr & 0x40 == 0 {
            return;
        }
        let s = &mut self.serial;
        if s.count == 0 {
            if !s.pending {
                return;
            }
            s.pending = false;
            s.shift = s.sdr;
            s.count = 16;
        }
        // Two underflows per bit: CNT low with the data bit, then CNT high.
        s.count -= 1;
        if s.count % 2 == 1 {
            s.sp_out = s.shift & 0x80 != 0;
            s.cnt_out = false;
        } else {
            s.cnt_out = true;
            s.shift <<= 1;
        }
        if s.count == 0 {
            self.raise(icr::SP);
        }
    }

    fn serial_shift_in(&mut self) {
        let s = &mut self.serial;
        s.shift = (s.shift << 1) | u8::from(s.sp_out);
        s.count += 1;
        if s.count == 8 {
            s.count = 0;
            s.sdr = s.shift;
            self.raise(icr::SP);
        }
    }

    /// Serial input bit level (SP pin) for input mode.
    pub fn set_sp(&mut self, level: bool) {
        if self.ta.cr & 0x40 == 0 {
            self.serial.sp_out = level;
        }
    }

    /// Bus read. `pa_ext` / `pb_ext` are the levels external circuitry
    /// allows on the pins (1 = not pulled low): pin = driven & external.
    pub fn read(&mut self, reg: u8, pa_ext: u8, pb_ext: u8) -> u8 {
        match reg & 0x0f {
            0x08..=0x0b if self.model == CiaModel::Mos8520 => self.tod.read_counter(reg & 0x0f),
            0x08..=0x0b => self.tod.read(reg & 0x0f),
            0x0d => {
                let v = self.flags | if self.irq { icr::IRQ } else { 0 };
                self.flags = 0;
                self.irq = false;
                self.irq_next = false;
                v
            }
            r => self.peek(r, pa_ext, pb_ext),
        }
    }

    /// Side-effect-free read.
    pub fn peek(&self, reg: u8, pa_ext: u8, pb_ext: u8) -> u8 {
        match reg & 0x0f {
            0x0 => self.port_a_out() & pa_ext,
            0x1 => self.port_b_out() & pb_ext,
            0x2 => self.ddra,
            0x3 => self.ddrb,
            0x4 => self.ta.counter as u8,
            0x5 => (self.ta.counter >> 8) as u8,
            0x6 => self.tb.counter as u8,
            0x7 => (self.tb.counter >> 8) as u8,
            r @ 0x08..=0x0b if self.model == CiaModel::Mos8520 => self.tod.peek_counter(r),
            r @ 0x08..=0x0b => self.tod.peek(r),
            0x0c => self.serial.sdr,
            0x0d => self.flags | if self.irq { icr::IRQ } else { 0 },
            0x0e => self.ta.cr,
            _ => self.tb.cr,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 0x0f {
            0x0 => self.pra = v,
            0x1 => self.prb = v,
            0x2 => self.ddra = v,
            0x3 => self.ddrb = v,
            0x4 => self.ta.write_lo(v),
            0x5 => {
                self.ta.write_hi(v);
                self.oneshot_start(0);
            }
            0x6 => self.tb.write_lo(v),
            0x7 => {
                self.tb.write_hi(v);
                self.oneshot_start(1);
            }
            r @ 0x08..=0x0b => {
                let alarm = self.tb.cr & 0x80 != 0;
                let matched = if self.model == CiaModel::Mos8520 {
                    self.tod.write_counter(r, v, alarm)
                } else {
                    self.tod.write(r, v, alarm)
                };
                if matched {
                    self.raise(icr::ALARM);
                }
            }
            0x0c => {
                self.serial.sdr = v;
                if self.ta.cr & 0x40 != 0 {
                    self.serial.pending = true;
                }
            }
            0x0d => {
                if v & 0x80 != 0 {
                    self.mask |= v & 0x1f;
                } else {
                    self.mask &= !(v & 0x1f);
                }
                self.mask_written = true;
            }
            0x0e => {
                if (v ^ self.ta.cr) & 0x40 != 0 {
                    self.serial.count = 0;
                    self.serial.cnt_out = true;
                }
                self.ta.write_cr(v);
            }
            _ => self.tb.write_cr(v),
        }
    }

    /// 8520: a high-byte write in one-shot mode loads and starts the timer.
    fn oneshot_start(&mut self, timer: usize) {
        if self.model != CiaModel::Mos8520 {
            return;
        }
        let t = if timer == 0 {
            &mut self.ta
        } else {
            &mut self.tb
        };
        if t.cr & CR_ONESHOT != 0 {
            t.write_cr(t.cr | CR_START | CR_LOAD);
        }
    }

    /// Interrupt mask (not readable on the chip; for debugger views).
    pub fn mask(&self) -> u8 {
        self.mask
    }
}

const TAG: [u8; 4] = *b"6526";
/// 2: the TOD alarm comparator's output.
const VERSION: u16 = 2;

impl Savestate for Cia {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.pra);
            w.u8(self.prb);
            w.u8(self.ddra);
            w.u8(self.ddrb);
            self.ta.save(w);
            self.tb.save(w);
            w.raw(&self.tod.time);
            w.raw(&self.tod.alarm);
            match self.tod.latch {
                Some(l) => {
                    w.bool(true);
                    w.raw(&l);
                }
                None => {
                    w.bool(false);
                    w.raw(&[0; 4]);
                }
            }
            w.bool(self.tod.stopped);
            w.u8(self.tod.divider);
            let s = &self.serial;
            w.u8(s.sdr);
            w.u8(s.shift);
            w.u8(s.count);
            w.bool(s.pending);
            w.bool(s.cnt_out);
            w.bool(s.sp_out);
            w.u8(self.flags);
            w.u8(self.mask);
            w.bool(self.irq);
            w.bool(self.irq_next);
            w.bool(self.mask_written);
            w.bool(self.cnt_in);
            w.bool(self.cnt_edge);
            w.bool(self.flag_in);
            w.bool(self.tod.equal);
        });
    }

    /// Atomic: on error the chip is unchanged. The model is configuration
    /// and is kept.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = Cia::new(self.model);
        n.pra = c.u8()?;
        n.prb = c.u8()?;
        n.ddra = c.u8()?;
        n.ddrb = c.u8()?;
        n.ta = Timer::load(&mut c)?;
        n.tb = Timer::load(&mut c)?;
        c.raw_into(&mut n.tod.time)?;
        c.raw_into(&mut n.tod.alarm)?;
        let has_latch = c.bool()?;
        let mut latch = [0; 4];
        c.raw_into(&mut latch)?;
        n.tod.latch = has_latch.then_some(latch);
        n.tod.stopped = c.bool()?;
        n.tod.divider = c.u8()?;
        if n.tod.divider > 6 {
            return Err(StateError::Invalid("CIA TOD divider"));
        }
        n.serial = Serial {
            sdr: c.u8()?,
            shift: c.u8()?,
            count: c.u8()?,
            pending: c.bool()?,
            cnt_out: c.bool()?,
            sp_out: c.bool()?,
        };
        if n.serial.count > 16 {
            return Err(StateError::Invalid("CIA serial count"));
        }
        n.flags = c.u8()? & 0x1f;
        n.mask = c.u8()? & 0x1f;
        n.irq = c.bool()?;
        n.irq_next = c.bool()?;
        n.mask_written = c.bool()?;
        n.cnt_in = c.bool()?;
        n.cnt_edge = c.bool()?;
        n.flag_in = c.bool()?;
        n.tod.equal = if version >= 2 {
            c.bool()?
        } else {
            n.tod.time == n.tod.alarm
        };
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
