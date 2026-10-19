//! On-chip I/O port of the 6510, 8502 and 7501/8501 at addresses $00 (data
//! direction register) and $01 (data register).
//!
//! The CPU core does not intercept $00/$01; machines route those addresses to
//! an `IoPort` because what the port pins connect to (banking, datasette,
//! IEC) is machine wiring. Source: MOS 6510 data sheet.
//!
//! Not yet modelled: the slow fall-off of undriven input bits (C64 bits 6/7).

use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

const STATE_TAG: [u8; 4] = *b"IOPT";
const STATE_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoPort {
    /// Bits that exist on this chip variant.
    mask: u8,
    /// $00: 1 = output.
    pub ddr: u8,
    /// $01: output latch.
    pub data: u8,
}

impl IoPort {
    /// A port with the given pin mask, all pins inputs (reset state).
    pub fn new(mask: u8) -> Self {
        Self {
            mask,
            ddr: 0,
            data: 0,
        }
    }

    /// CPU read of $00 (`reg` = 0) or $01 (`reg` = 1). `pins` is what external
    /// circuitry drives onto input pins.
    pub fn read(&self, reg: u8, pins: u8) -> u8 {
        if reg & 1 == 0 {
            self.ddr
        } else {
            (self.data & self.ddr) | (pins & !self.ddr)
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        if reg & 1 == 0 {
            self.ddr = v;
        } else {
            self.data = v;
        }
    }

    /// Levels on the port pins as seen by external circuitry: outputs drive
    /// the latch value, inputs are pulled high. Bits beyond the variant's pins
    /// read as 0.
    pub fn outputs(&self) -> u8 {
        ((self.data & self.ddr) | !self.ddr) & self.mask
    }
}

impl Savestate for IoPort {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, STATE_VERSION, |w| {
            w.u8(self.ddr);
            w.u8(self.data);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, STATE_VERSION)?;
        let ddr = c.u8()?;
        let data = c.u8()?;
        self.ddr = ddr;
        self.data = data;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c64_reset_state_maps_all_roms() {
        // C64 KERNAL init: DDR=$2F, data=$37.
        let mut port = IoPort::new(0x3f);
        assert_eq!(port.outputs(), 0x3f, "inputs float high after reset");
        port.write(0, 0x2f);
        port.write(1, 0x37);
        assert_eq!(port.outputs() & 0x07, 0x07, "LORAM/HIRAM/CHAREN high");
        assert_eq!(
            port.read(1, 0x00) & 0x10,
            0x00,
            "bit 4 is an input (cassette sense)"
        );
        assert_eq!(port.read(1, 0x10) & 0x10, 0x10);
        assert_eq!(port.read(0, 0), 0x2f);
    }

    #[test]
    fn savestate_round_trip() {
        let mut port = IoPort::new(0x7f);
        port.write(0, 0x12);
        port.write(1, 0x34);
        let mut w = StateWriter::new();
        port.save(&mut w);
        let data = w.into_bytes();
        let mut back = IoPort::new(0x7f);
        back.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(back, port);
    }
}
