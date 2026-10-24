//! Mockingboard (Sweet Micro Systems), a sound card for slot 4: two 6522
//! VIAs, each driving an AY-3-8910 sound generator. The card's ROM space
//! holds the VIAs: $Cn00–$Cn7F the first (register = address bits 3–0),
//! $Cn80–$CnFF the second. A VIA's port A is the AY's data bus, port B
//! bits 0–2 its BC1, BDIR and /RESET lines; the VIAs' interrupts go to
//! the CPU's IRQ. The AYs run at the CPU clock; the first one is the left
//! channel, the second the right. (Sweet Micro Systems, "Mockingboard"
//! manual, programming section; Apple II Mockingboard documentation in the
//! Apple II Documentation Project.)
//!
//! The AY-3-8910 is the 8912's 40-pin sibling with a second I/O port
//! (unused here), so `chips_amstrad::ay::Ay` serves.

use chips_amstrad::ay::Ay;
use chips_cbm::via::Via;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mockingboard {
    pub vias: [Via; 2],
    pub ays: [Ay; 2],
    /// Port B as the AYs last saw it.
    lines: [u8; 2],
}

impl Default for Mockingboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Mockingboard {
    pub fn new() -> Self {
        Self {
            vias: [Via::new(), Via::new()],
            ays: [Ay::new(), Ay::new()],
            lines: [0xff; 2],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn chip(offset: u8) -> usize {
        usize::from(offset >> 7)
    }

    pub fn read(&mut self, offset: u8) -> u8 {
        let n = Self::chip(offset);
        self.vias[n].read(offset & 0x0f, 0xff, 0xff)
    }

    pub fn peek(&self, offset: u8) -> u8 {
        let n = Self::chip(offset);
        self.vias[n].peek(offset & 0x0f, 0xff, 0xff)
    }

    pub fn write(&mut self, offset: u8, v: u8) {
        let n = Self::chip(offset);
        self.vias[n].write(offset & 0x0f, v);
        self.bus(n);
    }

    /// Port B drives the AY's control lines; an AY bus cycle happens on
    /// each change.
    fn bus(&mut self, n: usize) {
        let pb = self.vias[n].port_b_out();
        if pb == self.lines[n] {
            return;
        }
        self.lines[n] = pb;
        if pb & 0x04 == 0 {
            self.ays[n].reset();
            return;
        }
        let function = (pb >> 1 & 1) << 1 | (pb & 1);
        if function >= 2 {
            self.ays[n].bus_write(function, self.vias[n].port_a_out());
        }
    }

    /// One CPU cycle.
    pub fn tick(&mut self) {
        for n in 0..2 {
            self.vias[n].tick();
            self.ays[n].tick();
        }
    }

    pub fn irq(&self) -> bool {
        self.vias.iter().any(Via::irq)
    }

    /// Left and right levels (0–30 000 each).
    pub fn output(&self) -> [i32; 2] {
        let sum = |ay: &Ay| ay.output().iter().map(|&v| i32::from(v)).sum::<i32>();
        [sum(&self.ays[0]), sum(&self.ays[1])]
    }
}

impl Savestate for Mockingboard {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"A2MB", 1, |w| {
            w.u8(self.lines[0]);
            w.u8(self.lines[1]);
        });
        for n in 0..2 {
            self.vias[n].save(w);
            self.ays[n].save(w);
        }
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"A2MB", 1)?;
        let mut n = self.clone();
        n.lines = [c.u8()?, c.u8()?];
        for i in 0..2 {
            n.vias[i].load(r)?;
            n.ays[i].load(r)?;
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reach_the_ay_through_port_b() {
        let mut mb = Mockingboard::new();
        // DDRA/DDRB all outputs, /RESET high.
        mb.write(0x03, 0xff);
        mb.write(0x02, 0xff);
        mb.write(0x00, 0x04);
        // Latch register 8, then write 15 (channel A volume).
        mb.write(0x01, 8);
        mb.write(0x00, 0x07);
        mb.write(0x00, 0x04);
        mb.write(0x01, 15);
        mb.write(0x00, 0x06);
        mb.write(0x00, 0x04);
        assert_eq!(mb.ays[0].regs[8], 15);
        assert_eq!(mb.ays[1].regs[8], 0, "the second AY is untouched");
        // The second VIA answers at $80.
        mb.write(0x83, 0xff);
        assert_eq!(mb.peek(0x83), 0xff);
        assert_eq!(mb.peek(0x03), 0xff);
    }
}
