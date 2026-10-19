//! MOS 6522 Versatile Interface Adapter (VIC-20, 1541, 1571, PET).
//!
//! | Reg | Read                         | Write                                  |
//! | --- | ---------------------------- | -------------------------------------- |
//! | 0   | IRB (clears CB1/CB2 flags)   | ORB (clears CB1/CB2 flags)             |
//! | 1   | IRA (clears CA1/CA2 flags)   | ORA (clears CA1/CA2 flags)             |
//! | 2/3 | DDRB / DDRA                  | DDRB / DDRA                            |
//! | 4   | T1 counter low (clears T1)   | T1 latch low                           |
//! | 5   | T1 counter high              | T1 latch high, load counter, clear T1, start |
//! | 6/7 | T1 latch low/high            | T1 latch low / high (high clears T1)   |
//! | 8   | T2 counter low (clears T2)   | T2 latch low                           |
//! | 9   | T2 counter high              | T2 counter high, load, clear T2, arm   |
//! | A   | shift register               | shift register                         |
//! | B   | ACR: 7 PB7 out, 6 T1 free-run, 5 T2 counts PB6, 4–2 SR mode, 1/0 latch PB/PA |
//! | C   | PCR: CB2 7–5, CB1 edge 4, CA2 3–1, CA1 edge 0 |
//! | D   | IFR: 7 any, 6 T1, 5 T2, 4 CB1, 3 CB2, 2 SR, 1 CA1, 0 CA2 (write 1 clears) |
//! | E   | IER (bit 7 reads 1)          | bit 7 = set/clear selected bits         |
//! | F   | IRA without handshake        | ORA without handshake                  |
//!
//! Timers: T1 counts down every φ2; the cycle after it passes 0 it sets its
//! flag and, in free-run mode, reloads the latch (one cycle at the latch
//! value), so it interrupts every N + 2 cycles. T2 counts down once and
//! interrupts once per write of its high byte. Source: MOS 6522 data sheet;
//! Rockwell R6522 data sheet (timing diagrams). The shift register stores
//! but does not shift yet (unused by the 1541).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub mod ifr {
    pub const CA2: u8 = 0x01;
    pub const CA1: u8 = 0x02;
    pub const SR: u8 = 0x04;
    pub const CB2: u8 = 0x08;
    pub const CB1: u8 = 0x10;
    pub const T2: u8 = 0x20;
    pub const T1: u8 = 0x40;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Via {
    pub orb: u8,
    pub ora: u8,
    pub ddrb: u8,
    pub ddra: u8,
    pub t1_counter: u16,
    pub t1_latch: u16,
    pub t2_counter: u16,
    pub t2_latch_lo: u8,
    pub sr: u8,
    pub acr: u8,
    pub pcr: u8,
    pub ifr: u8,
    pub ier: u8,
    /// T1 interrupts on its next underflow (one-shot armed / free-run).
    t1_armed: bool,
    t2_armed: bool,
    /// T1 reached zero; reload (free-run) or wrap happens next cycle.
    t1_reload: bool,
    /// The counter was just written; it starts counting next cycle.
    t1_loaded: bool,
    t2_loaded: bool,
    pb7: bool,
    ca1: bool,
    cb1: bool,
    /// Input latches (ACR bits 0/1) captured on the CA1/CB1 edge.
    ira_latch: u8,
    irb_latch: u8,
}

impl Via {
    pub fn new() -> Self {
        Self {
            t1_counter: 0xffff,
            t1_latch: 0xffff,
            t2_counter: 0xffff,
            ca1: true,
            cb1: true,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn irq(&self) -> bool {
        self.ifr & self.ier & 0x7f != 0
    }

    /// Port A pin levels driven by the VIA (inputs pulled up).
    pub fn port_a_out(&self) -> u8 {
        self.ora | !self.ddra
    }

    /// Port B pin levels driven by the VIA, with PB7 as T1 output if enabled.
    pub fn port_b_out(&self) -> u8 {
        let v = self.orb | !self.ddrb;
        if self.acr & 0x80 != 0 {
            (v & 0x7f) | (u8::from(self.pb7) << 7)
        } else {
            v
        }
    }

    /// CA2 output level in manual output mode (PCR 110 low, 111 high);
    /// other modes report high.
    pub fn ca2_out(&self) -> bool {
        self.pcr & 0x0e != 0x0c
    }

    /// CB2 output level in manual output mode.
    pub fn cb2_out(&self) -> bool {
        self.pcr & 0xe0 != 0xc0
    }

    /// CA1 input level; the edge selected by PCR bit 0 sets the CA1 flag.
    pub fn set_ca1(&mut self, level: bool, pa_pins: u8) {
        let positive = self.pcr & 0x01 != 0;
        if level != self.ca1 && level == positive {
            self.ifr |= ifr::CA1;
            self.ira_latch = pa_pins;
        }
        self.ca1 = level;
    }

    /// CB1 input level; the edge selected by PCR bit 4 sets the CB1 flag.
    pub fn set_cb1(&mut self, level: bool, pb_pins: u8) {
        let positive = self.pcr & 0x10 != 0;
        if level != self.cb1 && level == positive {
            self.ifr |= ifr::CB1;
            self.irb_latch = pb_pins;
        }
        self.cb1 = level;
    }

    /// One φ2 cycle, before the CPU access of that cycle.
    pub fn tick(&mut self) {
        // T1
        if self.t1_loaded {
            self.t1_loaded = false;
        } else if self.t1_reload {
            self.t1_reload = false;
            if self.acr & 0x40 != 0 {
                self.t1_counter = self.t1_latch;
            } else {
                self.t1_counter = self.t1_counter.wrapping_sub(1);
            }
        } else {
            if self.t1_counter == 0 {
                self.t1_reload = true;
                if self.t1_armed {
                    self.ifr |= ifr::T1;
                    self.pb7 = !self.pb7;
                    // One-shot mode fires once; free-run keeps going.
                    self.t1_armed = self.acr & 0x40 != 0;
                }
            }
            self.t1_counter = self.t1_counter.wrapping_sub(1);
            if self.t1_reload {
                // The counter showed 0 this cycle; next cycle it reloads.
                self.t1_counter = 0xffff;
            }
        }

        // T2 (timed mode; pulse counting on PB6 is not wired yet).
        if self.t2_loaded {
            self.t2_loaded = false;
        } else if self.acr & 0x20 == 0 {
            if self.t2_counter == 0 && self.t2_armed {
                self.ifr |= ifr::T2;
                self.t2_armed = false;
            }
            self.t2_counter = self.t2_counter.wrapping_sub(1);
        }
    }

    /// Bus read; `pa_pins` / `pb_pins` are the external levels on the pins.
    pub fn read(&mut self, reg: u8, pa_pins: u8, pb_pins: u8) -> u8 {
        let v = self.peek(reg, pa_pins, pb_pins);
        match reg & 0x0f {
            0x0 => self.ifr &= !(ifr::CB1 | ifr::CB2),
            0x1 => self.ifr &= !(ifr::CA1 | ifr::CA2),
            0x4 => self.ifr &= !ifr::T1,
            0x8 => self.ifr &= !ifr::T2,
            _ => {}
        }
        v
    }

    /// Side-effect-free read.
    pub fn peek(&self, reg: u8, pa_pins: u8, pb_pins: u8) -> u8 {
        match reg & 0x0f {
            0x0 => {
                let pins = if self.acr & 0x02 != 0 {
                    self.irb_latch
                } else {
                    pb_pins
                };
                (self.port_b_out() & self.ddrb) | (pins & !self.ddrb)
            }
            0x1 | 0xf => {
                if self.acr & 0x01 != 0 {
                    self.ira_latch
                } else {
                    pa_pins
                }
            }
            0x2 => self.ddrb,
            0x3 => self.ddra,
            0x4 => self.t1_counter as u8,
            0x5 => (self.t1_counter >> 8) as u8,
            0x6 => self.t1_latch as u8,
            0x7 => (self.t1_latch >> 8) as u8,
            0x8 => self.t2_counter as u8,
            0x9 => (self.t2_counter >> 8) as u8,
            0xa => self.sr,
            0xb => self.acr,
            0xc => self.pcr,
            0xd => {
                let any = if self.irq() { 0x80 } else { 0 };
                (self.ifr & 0x7f) | any
            }
            _ => self.ier | 0x80,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 0x0f {
            0x0 => {
                self.orb = v;
                self.ifr &= !(ifr::CB1 | ifr::CB2);
            }
            0x1 => {
                self.ora = v;
                self.ifr &= !(ifr::CA1 | ifr::CA2);
            }
            0x2 => self.ddrb = v,
            0x3 => self.ddra = v,
            0x4 | 0x6 => self.t1_latch = (self.t1_latch & 0xff00) | u16::from(v),
            0x5 => {
                self.t1_latch = (self.t1_latch & 0x00ff) | (u16::from(v) << 8);
                self.t1_counter = self.t1_latch;
                self.ifr &= !ifr::T1;
                self.t1_armed = true;
                self.t1_reload = false;
                self.t1_loaded = true;
                if self.acr & 0x80 != 0 {
                    self.pb7 = false;
                }
            }
            0x7 => {
                self.t1_latch = (self.t1_latch & 0x00ff) | (u16::from(v) << 8);
                self.ifr &= !ifr::T1;
            }
            0x8 => self.t2_latch_lo = v,
            0x9 => {
                self.t2_counter = (u16::from(v) << 8) | u16::from(self.t2_latch_lo);
                self.ifr &= !ifr::T2;
                self.t2_armed = true;
                self.t2_loaded = true;
            }
            0xa => {
                self.sr = v;
                self.ifr &= !ifr::SR;
            }
            0xb => self.acr = v,
            0xc => self.pcr = v,
            0xd => self.ifr &= !(v & 0x7f),
            0xe => {
                if v & 0x80 != 0 {
                    self.ier |= v & 0x7f;
                } else {
                    self.ier &= !(v & 0x7f);
                }
            }
            _ => self.ora = v,
        }
    }
}

const TAG: [u8; 4] = *b"6522";
const VERSION: u16 = 1;

impl Savestate for Via {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            for b in [
                self.orb,
                self.ora,
                self.ddrb,
                self.ddra,
                self.t2_latch_lo,
                self.sr,
                self.acr,
                self.pcr,
                self.ifr,
                self.ier,
                self.ira_latch,
                self.irb_latch,
            ] {
                w.u8(b);
            }
            w.u16(self.t1_counter);
            w.u16(self.t1_latch);
            w.u16(self.t2_counter);
            for b in [
                self.t1_armed,
                self.t2_armed,
                self.t1_reload,
                self.t1_loaded,
                self.t2_loaded,
                self.pb7,
                self.ca1,
                self.cb1,
            ] {
                w.bool(b);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut v = Via::new();
        let mut bytes = [0u8; 12];
        c.raw_into(&mut bytes)?;
        [
            v.orb,
            v.ora,
            v.ddrb,
            v.ddra,
            v.t2_latch_lo,
            v.sr,
            v.acr,
            v.pcr,
            v.ifr,
            v.ier,
            v.ira_latch,
            v.irb_latch,
        ] = bytes;
        v.ifr &= 0x7f;
        v.ier &= 0x7f;
        v.t1_counter = c.u16()?;
        v.t1_latch = c.u16()?;
        v.t2_counter = c.u16()?;
        v.t1_armed = c.bool()?;
        v.t2_armed = c.bool()?;
        v.t1_reload = c.bool()?;
        v.t1_loaded = c.bool()?;
        v.t2_loaded = c.bool()?;
        v.pb7 = c.bool()?;
        v.ca1 = c.bool()?;
        v.cb1 = c.bool()?;
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(v: &mut Via, n: usize) -> Vec<u16> {
        (0..n)
            .map(|_| {
                v.tick();
                v.t1_counter
            })
            .collect()
    }

    #[test]
    fn t1_free_run_period_is_latch_plus_two() {
        let mut v = Via::new();
        v.write(0xb, 0x40);
        v.write(0x4, 3);
        v.write(0x5, 0);
        let seq = run(&mut v, 11);
        assert_eq!(seq, [3, 2, 1, 0, 0xffff, 3, 2, 1, 0, 0xffff, 3]);
        assert_eq!(v.ifr & ifr::T1, ifr::T1);
    }

    #[test]
    fn t1_one_shot_interrupts_once() {
        let mut v = Via::new();
        v.write(0xe, 0xc0);
        v.write(0x4, 2);
        v.write(0x5, 0);
        run(&mut v, 4);
        assert!(v.irq());
        assert_eq!(v.read(0xd, 0xff, 0xff), 0xc0);
        assert_eq!(
            v.read(0x4, 0xff, 0xff),
            0xff,
            "wrapped to $FFFF, counting on"
        );
        assert!(!v.irq(), "reading T1 low clears the flag");
        run(&mut v, 70_000);
        assert!(!v.irq(), "no second interrupt without rewriting");
    }

    #[test]
    fn t2_one_shot() {
        let mut v = Via::new();
        v.write(0x8, 5);
        v.write(0x9, 0);
        run(&mut v, 6);
        assert_eq!(v.ifr & ifr::T2, 0);
        run(&mut v, 1);
        assert_eq!(v.ifr & ifr::T2, ifr::T2);
        v.read(0x8, 0xff, 0xff);
        assert_eq!(v.ifr & ifr::T2, 0);
    }

    #[test]
    fn ca1_edge_and_port_handshake_flags() {
        let mut v = Via::new();
        v.write(0xc, 0x01); // CA1 positive edge
        v.set_ca1(false, 0);
        v.set_ca1(true, 0x5a);
        assert_eq!(v.ifr, ifr::CA1);
        v.write(0xb, 0x01); // latch PA
        assert_eq!(v.peek(0x1, 0x00, 0xff), 0x5a);
        v.read(0x1, 0x00, 0xff);
        assert_eq!(v.ifr, 0, "reading ORA clears CA1");
        v.set_ca1(false, 0);
        assert_eq!(v.ifr, 0, "wrong edge");
    }

    #[test]
    fn ports_ier_and_manual_ca2() {
        let mut v = Via::new();
        v.write(0x2, 0x0f);
        v.write(0x0, 0x05);
        assert_eq!(v.read(0x0, 0xff, 0xa0), 0xa5);
        v.write(0xe, 0x82);
        assert_eq!(v.peek(0xe, 0, 0), 0x82);
        v.write(0xe, 0x02);
        assert_eq!(v.peek(0xe, 0, 0), 0x80);
        v.write(0xc, 0x0c);
        assert!(!v.ca2_out());
        v.write(0xc, 0x0e);
        assert!(v.ca2_out());
    }

    #[test]
    fn savestate_round_trip() {
        let mut v = Via::new();
        v.write(0xb, 0x40);
        v.write(0x4, 9);
        v.write(0x5, 1);
        run(&mut v, 17);
        let mut w = StateWriter::new();
        v.save(&mut w);
        let data = w.into_bytes();
        let mut u = Via::new();
        u.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(u, v);
        assert_eq!(run(&mut u, 600), run(&mut v, 600));
    }
}
