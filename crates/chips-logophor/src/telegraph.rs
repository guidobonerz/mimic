//! Telegraph, the Logophor 16's I/O chip (SPEC.md section 10). Built so
//! far (step 4): the interrupt controller at FFFF:4280 (INTREQ, INTENA,
//! set/clear registers) with the fixed level of each source. Network,
//! timers, input and the SD card follow in later steps.

use emu_core::{StateError, StateReader, StateWriter};

pub const INTREQ: u16 = 0x4280;
pub const INTENA: u16 = 0x4282;

/// Interrupt sources (INTREQ/INTENA bits, SPEC.md 10.4).
pub const INT_VBLANK: u16 = 1 << 0;
pub const INT_COPPER: u16 = 1 << 1;
pub const INT_DMAERR: u16 = 1 << 2;
pub const INT_BLITTER: u16 = 1 << 3;
pub const INT_STEREOSCOPE: u16 = 1 << 4;
pub const INT_CALLIOPE: u16 = 1 << 5;
pub const INT_TIMER: u16 = 1 << 6;
pub const INT_NETWORK: u16 = 1 << 7;
pub const INT_KEYBOARD: u16 = 1 << 8;
pub const INT_PORTS: u16 = 1 << 9;
pub const INT_SD: u16 = 1 << 10;
pub const INT_RTC: u16 = 1 << 11;
pub const INT_NMI: u16 = 1 << 14;

/// The level of each source bit (0 = no source).
const LEVELS: [u8; 15] = [4, 4, 4, 3, 3, 6, 5, 2, 1, 1, 1, 1, 0, 0, 7];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interrupts {
    req: u16,
    ena: u16,
}

fn set_clear(reg: &mut u16, v: u16) {
    if v & 0x8000 != 0 {
        *reg |= v & 0x7fff;
    } else {
        *reg &= !v;
    }
}

impl Interrupts {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source requests an interrupt.
    pub fn raise(&mut self, bits: u16) {
        self.req |= bits & 0x7fff;
    }

    /// The highest pending level (0 = none).
    pub fn level(&self) -> u8 {
        let pending = self.req & self.ena;
        (0..15)
            .filter(|b| pending >> b & 1 != 0)
            .map(|b| LEVELS[b])
            .max()
            .unwrap_or(0)
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            INTREQ => self.req,
            INTENA => self.ena,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u16, v: u16) {
        match off & !1 {
            INTREQ => set_clear(&mut self.req, v),
            INTENA => set_clear(&mut self.ena, v),
            _ => {}
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"TINT", 1, |w| {
            w.u16(self.req);
            w.u16(self.ena);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"TINT", 1)?;
        *self = Self {
            req: c.u16()? & 0x7fff,
            ena: c.u16()? & 0x7fff,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_highest_enabled_source() {
        let mut i = Interrupts::new();
        i.raise(INT_VBLANK | INT_KEYBOARD);
        assert_eq!(i.level(), 0, "nothing enabled");
        i.write(INTENA, 0x8000 | INT_KEYBOARD);
        assert_eq!(i.level(), 1);
        i.write(INTENA, 0x8000 | INT_VBLANK | INT_CALLIOPE);
        assert_eq!(i.level(), 4);
        i.raise(INT_CALLIOPE);
        assert_eq!(i.level(), 6);
        i.write(INTREQ, INT_CALLIOPE | INT_VBLANK);
        assert_eq!(i.level(), 1, "cleared by a write without bit 15");
        i.write(INTREQ, 0x8000 | INT_NMI);
        i.write(INTENA, 0x8000 | INT_NMI);
        assert_eq!(i.level(), 7);
        assert_eq!(
            i.peek(INTENA),
            INT_KEYBOARD | INT_VBLANK | INT_CALLIOPE | INT_NMI
        );
    }
}
