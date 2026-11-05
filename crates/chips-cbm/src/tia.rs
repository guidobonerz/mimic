//! MOS 6523 Tri-Port Interface Adapter (TIA), as used by the 1551 disk
//! drive (in the drive and, as the 28-pin 6523A-28, in its interface
//! cartridge for the C16/Plus/4).
//!
//! Three 8-bit ports, each with a data direction register; no timers,
//! no interrupts. Registers (RS2–RS0):
//!
//! | Reg | Function                              |
//! | --- | ------------------------------------- |
//! | 0   | port A (read: pins/latch, write: latch) |
//! | 1   | port B                                |
//! | 2   | port C                                |
//! | 3   | DDR A (1 = output)                    |
//! | 4   | DDR B                                 |
//! | 5   | DDR C                                 |
//! | 6–7 | not used by the 1551 DOS or the KERNAL; this model ignores writes and reads 0 |
//!
//! A port reads its output latch where the DDR bit is set and the pin
//! where it is clear. [`Tia6523::outputs`] gives the level each port
//! drives (inputs released, high through the pull-ups); the 28-pin
//! version bonds out only PA0–7, PB0–1 and PC6–7, so its other input pins
//! read 0 (VICE test program `Plus4/tcbm/test.asm`: an input port bit
//! without a pin reads 0, an output bit its latch).
//!
//! Sources: Commodore 1551 schematic 251860 and the 1551 interface
//! ("paddle") schematic 251925 (zimmers.net), the register use in the
//! 1551 DOS (318008-01) and the Plus/4 KERNAL (318004-05).

use emu_core::{Savestate, StateError, StateReader, StateWriter};

const TAG: [u8; 4] = *b"TIA1";
const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tia6523 {
    /// Output latches A, B, C.
    pub latch: [u8; 3],
    /// Data direction A, B, C (1 = output).
    pub ddr: [u8; 3],
}

impl Tia6523 {
    pub fn new() -> Self {
        Self::default()
    }

    /// RESET: all ports inputs, latches cleared.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Register `reg` (0–7) as the CPU reads it; `pins` are the levels on
    /// ports A, B and C. Reading has no side effects.
    pub fn read(&self, reg: u8, pins: [u8; 3]) -> u8 {
        match reg & 7 {
            r @ 0..=2 => {
                let i = usize::from(r);
                (self.latch[i] & self.ddr[i]) | (pins[i] & !self.ddr[i])
            }
            r @ 3..=5 => self.ddr[usize::from(r - 3)],
            _ => 0,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 7 {
            r @ 0..=2 => self.latch[usize::from(r)] = v,
            r @ 3..=5 => self.ddr[usize::from(r - 3)] = v,
            _ => {}
        }
    }

    /// Register `reg` as read when the ports' pins are wired lines: a
    /// port reads the level of its lines, outputs included (`lines` must
    /// hold the chip's own output ANDed with whatever else drives them).
    pub fn read_lines(&self, reg: u8, lines: [u8; 3]) -> u8 {
        match reg & 7 {
            r @ 0..=2 => lines[usize::from(r)],
            r @ 3..=5 => self.ddr[usize::from(r - 3)],
            _ => 0,
        }
    }

    /// What port `port` (0 = A, 1 = B, 2 = C) drives: the latch on output
    /// bits, high (released) on inputs.
    pub fn outputs(&self, port: usize) -> u8 {
        (self.latch[port] & self.ddr[port]) | !self.ddr[port]
    }
}

impl Savestate for Tia6523 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.latch);
            w.raw(&self.ddr);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut t = Self::default();
        c.raw_into(&mut t.latch)?;
        c.raw_into(&mut t.ddr)?;
        *self = t;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_read_latch_on_outputs_and_pins_on_inputs() {
        let mut t = Tia6523::new();
        t.write(4, 0xfc); // DDR B
        t.write(1, 0xfc);
        assert_eq!(t.read(1, [0, 0x03, 0]), 0xff);
        assert_eq!(t.read(1, [0, 0x00, 0]), 0xfc);
        assert_eq!(t.read(4, [0; 3]), 0xfc);
        assert_eq!(t.outputs(1), 0xfc | 0x03);
        t.write(4, 0);
        assert_eq!(t.read(1, [0, 0x02, 0]), 0x02);
        assert_eq!(t.outputs(1), 0xff);
        assert_eq!(t.read(6, [0xff; 3]), 0);
    }

    #[test]
    fn savestate_round_trip() {
        let mut t = Tia6523::new();
        t.write(0, 0x55);
        t.write(3, 0xff);
        t.write(5, 0x40);
        let mut w = StateWriter::new();
        t.save(&mut w);
        let bytes = w.into_bytes();
        let mut u = Tia6523::new();
        u.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(t, u);
    }
}
