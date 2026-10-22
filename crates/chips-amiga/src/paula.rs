//! Paula (8364): interrupt control (INTENA, INTREQ → IPL0–2), ADKCON,
//! POTGO, the serial port ([`Uart`]), the disk controller ([`DiskCtl`])
//! and the four audio channels ([`Channel`]).
//!
//! ADKCON (appendix A): bits 0–7 the audio attach modes (`audio`), 8 FAST
//! and 9 MSBSYNC and 10 WORDSYNC for the disk (`disk`; MSBSYNC is for GCR
//! and ignored), 11 UARTBRK (`uart`), 12 MFMPREC and 13–14 PRECOMP: write
//! precompensation (0, 140, 280, 560 ns). Precompensation shifts the
//! analogue flux transitions a drive writes; the drives here keep whole
//! bit cells, so the bits are stored as written and the setting only
//! reads back.
//!
//! Interrupt levels (HRM chapter 7, "Interrupts", table 7-1):
//!
//! | Level | INTREQ bits |
//! | --- | --- |
//! | 1 | TBE (0), DSKBLK (1), SOFT (2) |
//! | 2 | PORTS (3): CIA-A /INT, the expansion bus |
//! | 3 | COPER (4), VERTB (5), BLIT (6) |
//! | 4 | AUD0–AUD3 (7–10) |
//! | 5 | RBF (11), DSKSYN (12) |
//! | 6 | EXTER (13): CIA-B /INT |
//!
//! INTEN (bit 14 of INTENA) is the master enable. Writes to INTENA,
//! INTREQ and ADKCON are SET/CLR: bit 15 set → the 1 bits are set, else
//! cleared.

use emu_core::{StateError, StateReader, StateWriter};

use crate::audio::Channel;
use crate::disk::DiskCtl;
use crate::int;
use crate::uart::Uart;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paula {
    pub intena: u16,
    pub intreq: u16,
    pub adkcon: u16,
    pub potgo: u16,
    /// Interrupt inputs from the CIAs (level, held while asserted).
    pub ports_line: bool,
    pub exter_line: bool,
    pub dsklen: u16,
    pub dsksync: u16,
    pub disk: DiskCtl,
    pub audio: [Channel; 4],
    pub uart: Uart,
}

pub(crate) fn set_clr(reg: &mut u16, v: u16) {
    if v & 0x8000 != 0 {
        *reg |= v & 0x7fff;
    } else {
        *reg &= !v;
    }
}

impl Paula {
    pub fn new() -> Self {
        Self::default()
    }

    /// The interrupt level on IPL0–2 (0 = none).
    pub fn ipl(&self) -> u8 {
        if self.intena & int::INTEN == 0 {
            return 0;
        }
        let active = self.intena & self.requests() & 0x3fff;
        const LEVELS: [u8; 14] = [1, 1, 1, 2, 3, 3, 3, 4, 4, 4, 4, 5, 5, 6];
        (0..14)
            .rev()
            .find(|&b| active & 1 << b != 0)
            .map_or(0, |b| LEVELS[b])
    }

    /// INTREQ as read: the latched requests plus the CIA lines.
    pub fn requests(&self) -> u16 {
        self.intreq
            | if self.ports_line { int::PORTS } else { 0 }
            | if self.exter_line { int::EXTER } else { 0 }
    }

    pub fn request(&mut self, bits: u16) {
        self.intreq |= bits;
    }

    pub fn write_intena(&mut self, v: u16) {
        set_clr(&mut self.intena, v);
    }

    pub fn write_intreq(&mut self, v: u16) {
        set_clr(&mut self.intreq, v);
        if v & 0x8000 == 0 && v & int::RBF != 0 {
            self.uart.clear_rbf();
        }
    }

    pub fn write_adkcon(&mut self, v: u16) {
        set_clr(&mut self.adkcon, v);
    }

    /// POTGOR: a pin set as output reads its data bit; inputs read high
    /// (pulled up; mouse buttons 2/3 pull them low, see `pins_low`).
    pub fn potgor(&self, pins_low: u16) -> u16 {
        let mut v = 0;
        for pin in 0..4 {
            let out = 0x0200 << (2 * pin);
            let dat = 0x0100 << (2 * pin);
            let level = if self.potgo & out != 0 {
                self.potgo & dat != 0
            } else {
                pins_low & dat == 0
            };
            if level {
                v |= dat;
            }
        }
        v
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        for v in [
            self.intena,
            self.intreq,
            self.adkcon,
            self.potgo,
            self.dsklen,
            self.dsksync,
            self.uart.serper,
        ] {
            w.u16(v);
        }
        w.bool(self.ports_line);
        w.bool(self.exter_line);
        self.disk.save(w);
        for c in &self.audio {
            c.save(w);
        }
        self.uart.save(w);
    }

    /// `version`: the enclosing chunk's; before 6 the serial port had
    /// only SERPER.
    pub(crate) fn load(r: &mut StateReader<'_>, version: u16) -> Result<Self, StateError> {
        let mut p = Self::new();
        for v in [&mut p.intena, &mut p.intreq, &mut p.adkcon] {
            *v = r.u16()? & 0x7fff;
        }
        for v in [&mut p.potgo, &mut p.dsklen, &mut p.dsksync] {
            *v = r.u16()?;
        }
        let serper = r.u16()?;
        p.ports_line = r.bool()?;
        p.exter_line = r.bool()?;
        p.disk = DiskCtl::load(r)?;
        for c in &mut p.audio {
            *c = Channel::load(r, version)?;
        }
        if version >= 6 {
            p.uart = Uart::load(r)?;
        } else {
            p.uart.serper = serper;
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_highest_enabled_request() {
        let mut p = Paula::new();
        p.write_intena(0x8000 | int::INTEN | int::VERTB | int::PORTS);
        assert_eq!(p.ipl(), 0);
        p.ports_line = true;
        assert_eq!(p.ipl(), 2);
        p.request(int::VERTB);
        assert_eq!(p.ipl(), 3);
        p.write_intena(int::INTEN);
        assert_eq!(p.ipl(), 0, "master enable off");
        p.write_intena(0x8000 | int::INTEN);
        p.write_intreq(int::VERTB);
        assert_eq!(p.requests() & int::VERTB, 0);
        assert_eq!(p.ipl(), 2);
    }

    #[test]
    fn potgor_reads_pulled_up_inputs_and_outputs() {
        let mut p = Paula::new();
        assert_eq!(p.potgor(0), 0x5500);
        assert_eq!(p.potgor(0x0400), 0x5100, "right button, port 0");
        p.potgo = 0x0200; // pin 0 output low
        assert_eq!(p.potgor(0), 0x5400);
    }
}
