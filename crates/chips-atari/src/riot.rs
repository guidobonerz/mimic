//! MOS 6532 RIOT (RAM, I/O, timer) as wired in the Atari 2600: 128 bytes
//! of RAM, two 8-bit ports (A: controllers, B: console switches) and an
//! interval timer. The 2600 leaves the /IRQ output unconnected; the flags
//! are still readable.
//!
//! Register decoding (address lines as the chip sees them; the machine
//! selects the chip with A7 = 1, A12 = 0):
//!
//! | A9 | A2 | Access | Function                                              |
//! | -- | -- | ------ | ----------------------------------------------------- |
//! | 0  | –  | r/w    | RAM, A0–A6                                            |
//! | 1  | 0  | r/w    | A1 A0: 0 SWCHA (port A), 1 SWACNT (DDR A), 2 SWCHB, 3 SWBCNT |
//! | 1  | 1  | write  | A4 = 1: timer, A1 A0 = interval 1/8/64/1024 (TIM1T…T1024T), A3 = timer interrupt enable; A4 = 0: PA7 edge detect, A0 = positive edge, A1 = PA7 interrupt enable |
//! | 1  | 1  | read   | A0 = 0: timer (INTIM), A3 = timer interrupt enable; A0 = 1: interrupt flags (bit 7 timer, bit 6 PA7) |
//!
//! Timer: a write loads the counter and restarts the prescaler; the
//! counter steps down once in the next cycle and then once per interval.
//! When it passes zero it sets the timer flag and counts down every cycle
//! until the next write. Reading the timer clears the timer flag; reading
//! the flags clears the PA7 flag.
//!
//! Sources: MOS MCS6532 data sheet ("Interval timer", "Interrupt flag
//! register", "Edge detect control"); Steve Wright, "Stella Programmer's
//! Guide" (1979), section 12 ("PIA 6532").

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

/// Prescaler intervals selected by A1 A0 of a timer write.
const INTERVALS: [u16; 4] = [1, 8, 64, 1024];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Riot {
    pub ram: [u8; 128],
    /// Output registers and data direction (1 = output) of ports A and B.
    pub ora: u8,
    pub ddra: u8,
    pub orb: u8,
    pub ddrb: u8,
    /// What the outside world drives onto the port pins (inputs pulled up
    /// read 1).
    pa_in: u8,
    pb_in: u8,
    pub timer: u8,
    /// Prescaler interval of the last timer write.
    interval: u16,
    /// Cycles until the next decrement.
    countdown: u16,
    /// Passed zero: counting every cycle until the next write.
    expired: bool,
    timer_flag: bool,
    pa7_flag: bool,
    timer_irq: bool,
    pa7_irq: bool,
    pa7_positive: bool,
}

impl Default for Riot {
    fn default() -> Self {
        Self::new()
    }
}

impl Riot {
    pub fn new() -> Self {
        Self {
            ram: [0; 128],
            ora: 0,
            ddra: 0,
            orb: 0,
            ddrb: 0,
            pa_in: 0xff,
            pb_in: 0xff,
            timer: 0,
            interval: 1024,
            countdown: 1024,
            expired: false,
            timer_flag: false,
            pa7_flag: false,
            timer_irq: false,
            pa7_irq: false,
            pa7_positive: false,
        }
    }

    /// One cycle (φ2) of the timer.
    pub fn tick(&mut self) {
        if !self.expired {
            self.countdown -= 1;
            if self.countdown > 0 {
                return;
            }
            self.countdown = self.interval;
        }
        if self.timer == 0 {
            self.timer_flag = true;
            self.expired = true;
        }
        self.timer = self.timer.wrapping_sub(1);
    }

    /// The levels the controllers drive onto port A (1 = high). A PA7
    /// edge in the selected direction sets the PA7 flag.
    pub fn set_port_a_input(&mut self, pins: u8) {
        let before = self.port_a() & 0x80 != 0;
        self.pa_in = pins;
        let after = self.port_a() & 0x80 != 0;
        if before != after && after == self.pa7_positive {
            self.pa7_flag = true;
        }
    }

    /// The levels the console switches drive onto port B.
    pub fn set_port_b_input(&mut self, pins: u8) {
        self.pb_in = pins;
    }

    /// Port A pin levels: outputs as driven (an output pulled low by a
    /// device reads low), inputs as driven from outside.
    pub fn port_a(&self) -> u8 {
        (self.ora | !self.ddra) & self.pa_in
    }

    /// Port B pin levels.
    pub fn port_b(&self) -> u8 {
        (self.orb & self.ddrb) | (self.pb_in & !self.ddrb)
    }

    /// Interrupt flags as read at A0 = 1: bit 7 timer, bit 6 PA7 edge.
    pub fn flags(&self) -> u8 {
        (u8::from(self.timer_flag) << 7) | (u8::from(self.pa7_flag) << 6)
    }

    /// The /IRQ output (unconnected on the 2600).
    pub fn irq(&self) -> bool {
        (self.timer_flag && self.timer_irq) || (self.pa7_flag && self.pa7_irq)
    }

    /// Side-effect-free read of `addr` (chip-relative address lines).
    pub fn peek(&self, addr: u16) -> u8 {
        if addr & 0x200 == 0 {
            return self.ram[usize::from(addr & 0x7f)];
        }
        if addr & 0x04 == 0 {
            return match addr & 3 {
                0 => self.port_a(),
                1 => self.ddra,
                2 => self.port_b(),
                _ => self.ddrb,
            };
        }
        if addr & 1 == 0 {
            self.timer
        } else {
            self.flags()
        }
    }

    pub fn read(&mut self, addr: u16) -> u8 {
        let v = self.peek(addr);
        if addr & 0x204 == 0x204 {
            if addr & 1 == 0 {
                self.timer_flag = false;
                self.timer_irq = addr & 0x08 != 0;
            } else {
                self.pa7_flag = false;
            }
        }
        v
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        if addr & 0x200 == 0 {
            self.ram[usize::from(addr & 0x7f)] = v;
            return;
        }
        if addr & 0x04 == 0 {
            match addr & 3 {
                0 => self.ora = v,
                1 => self.ddra = v,
                2 => self.orb = v,
                _ => self.ddrb = v,
            }
            return;
        }
        if addr & 0x10 != 0 {
            self.timer = v;
            self.interval = INTERVALS[usize::from(addr & 3)];
            self.countdown = 1;
            self.expired = false;
            self.timer_flag = false;
            self.timer_irq = addr & 0x08 != 0;
        } else {
            self.pa7_positive = addr & 1 != 0;
            self.pa7_irq = addr & 2 != 0;
        }
    }

    /// Power-up: RAM cleared (deterministic; real RAM comes up random),
    /// ports to inputs. The timer runs from power-up at its slowest rate.
    pub fn reset(&mut self) {
        let (pa_in, pb_in) = (self.pa_in, self.pb_in);
        *self = Self::new();
        self.pa_in = pa_in;
        self.pb_in = pb_in;
    }
}

const TAG: [u8; 4] = *b"RIOT";
const VERSION: u16 = 1;

impl Savestate for Riot {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram);
            for v in [
                self.ora, self.ddra, self.orb, self.ddrb, self.pa_in, self.pb_in, self.timer,
            ] {
                w.u8(v);
            }
            w.u16(self.interval);
            w.u16(self.countdown);
            for b in [
                self.expired,
                self.timer_flag,
                self.pa7_flag,
                self.timer_irq,
                self.pa7_irq,
                self.pa7_positive,
            ] {
                w.bool(b);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        c.raw_into(&mut n.ram)?;
        for v in [
            &mut n.ora,
            &mut n.ddra,
            &mut n.orb,
            &mut n.ddrb,
            &mut n.pa_in,
            &mut n.pb_in,
            &mut n.timer,
        ] {
            *v = c.u8()?;
        }
        n.interval = c.u16()?;
        n.countdown = c.u16()?;
        if !INTERVALS.contains(&n.interval) || n.countdown == 0 || n.countdown > n.interval {
            return Err(StateError::Invalid("RIOT timer"));
        }
        for b in [
            &mut n.expired,
            &mut n.timer_flag,
            &mut n.pa7_flag,
            &mut n.timer_irq,
            &mut n.pa7_irq,
            &mut n.pa7_positive,
        ] {
            *b = c.bool()?;
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWCHA: u16 = 0x280;
    const SWACNT: u16 = 0x281;
    const SWCHB: u16 = 0x282;
    const INTIM: u16 = 0x284;
    const TIMINT: u16 = 0x285;
    const TIM8T: u16 = 0x295;
    const TIM1T: u16 = 0x294;

    #[test]
    fn ram_mirrors_on_a0_a6() {
        let mut r = Riot::new();
        r.write(0x80, 0x12);
        assert_eq!(r.read(0x180), 0x12, "A8 is not decoded");
        assert_eq!(r.peek(0x00), 0x12);
    }

    #[test]
    fn ports_combine_outputs_and_inputs() {
        let mut r = Riot::new();
        r.set_port_a_input(0b1110_1111);
        assert_eq!(r.read(SWCHA), 0b1110_1111, "all inputs");
        r.write(SWACNT, 0x0f);
        r.write(SWCHA, 0x05);
        assert_eq!(r.read(SWCHA), 0b1110_0101, "low nibble driven");
        r.set_port_b_input(0x0b);
        assert_eq!(r.read(SWCHB), 0x0b);
    }

    #[test]
    fn timer_counts_per_interval_then_every_cycle() {
        let mut r = Riot::new();
        r.write(TIM8T, 2);
        r.tick();
        assert_eq!(r.read(INTIM), 1, "first step in the next cycle");
        for _ in 0..7 {
            r.tick();
        }
        assert_eq!(r.peek(INTIM), 1);
        r.tick();
        assert_eq!(r.peek(INTIM), 0);
        assert_eq!(r.peek(TIMINT) & 0x80, 0);
        for _ in 0..8 {
            r.tick();
        }
        assert_eq!(r.peek(INTIM), 0xff, "passed zero");
        assert_eq!(r.peek(TIMINT) & 0x80, 0x80);
        r.tick();
        assert_eq!(r.peek(INTIM), 0xfe, "then every cycle");
        r.read(INTIM);
        assert_eq!(
            r.peek(TIMINT) & 0x80,
            0,
            "reading the timer clears the flag"
        );
        r.write(TIM1T, 3);
        assert_eq!(r.peek(INTIM), 3);
    }

    #[test]
    fn pa7_edge_flag() {
        let mut r = Riot::new();
        r.write(0x284, 0); // negative edge
        r.set_port_a_input(0x7f);
        assert_eq!(r.peek(TIMINT) & 0x40, 0x40);
        assert_eq!(r.read(TIMINT) & 0x40, 0x40);
        assert_eq!(r.peek(TIMINT) & 0x40, 0, "cleared by reading the flags");
        r.write(0x285, 0); // positive edge
        r.set_port_a_input(0xff);
        assert_eq!(r.peek(TIMINT) & 0x40, 0x40);
    }

    #[test]
    fn savestate_round_trip() {
        let mut r = Riot::new();
        r.write(TIM8T, 9);
        r.write(0x99, 7);
        r.tick();
        let mut w = StateWriter::new();
        r.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = Riot::new();
        back.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, r);
    }
}
