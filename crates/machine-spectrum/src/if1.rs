//! Sinclair ZX Interface 1: the shadow ROM (paged in [`crate::memory`]),
//! up to eight Microdrives and the RS-232 / network port.
//!
//! I/O ports (low byte; the IF1 decodes A3 and A4):
//!
//! | Port | Read | Write |
//! | --- | --- | --- |
//! | `$E7` | the next Microdrive byte | a Microdrive byte |
//! | `$EF` | bit 0 write protect (0 = protected), 1 SYNC (0 = found), 2 GAP (1 = in a gap), 3 DTR (1 = the RS-232 device is ready), 4 BUSY | bit 0 COMMS DATA, 1 COMMS CLK, 2 R/W (0 = write), 3 ERASE (0 = on), 4 CTS, 5 WAIT |
//! | `$F7` | bit 0 network in, bit 7 RS-232 TXD from the device (1 = space) | bit 0 RS-232 RXD to the device / network out |
//!
//! **Drive select:** the drives' motor flip-flops form a shift register:
//! each falling edge of COMMS CLK shifts COMMS DATA in at drive 1 (0 =
//! motor on) and every drive passes its state on to the next. The ROM
//! clocks eight bits, the selected drive's 0 first for drive 8.
//!
//! The RS-232 port has no host device yet: DTR stays low (a sender waits
//! until BREAK) and the line is idle.
//!
//! Sources: Sinclair ZX Interface 1 and Interface 2 Service Manual; the
//! Interface 1 ROM (edition 2) read for the port bits it uses (SEL-DRIVE
//! $1532, the GAP/SYNC search at $15E2, RS-232 at $0B88 and $0D0C).

use drives_other::microdrive::Microdrive;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub struct Interface1 {
    pub drives: Vec<Microdrive>,
    /// Motor flip-flops, bit n = drive n + 1.
    select: u8,
    /// The last write to port $EF.
    control: u8,
}

impl Interface1 {
    /// `drives`: Microdrives connected (1–8).
    pub fn new(drives: usize) -> Self {
        Self {
            drives: (0..drives.clamp(1, 8)).map(|_| Microdrive::new()).collect(),
            select: 0,
            control: 0xff,
        }
    }

    pub fn reset(&mut self) {
        self.select = 0;
        self.control = 0xff;
        self.motors();
    }

    /// Whether this port is the IF1's.
    #[inline]
    pub fn claims(port: u16) -> bool {
        matches!(port & 0xff, 0xe7 | 0xef | 0xf7)
    }

    fn motors(&mut self) {
        for (n, d) in self.drives.iter_mut().enumerate() {
            d.set_motor(self.select & (1 << n) != 0);
        }
    }

    /// The running drive (the first, if several are on).
    fn drive(&mut self) -> Option<&mut Microdrive> {
        let n = self.select.trailing_zeros() as usize;
        self.drives.get_mut(n)
    }

    pub fn read(&mut self, port: u16) -> u8 {
        match port & 0xff {
            0xe7 => self.drive().map_or(0xff, |d| d.read()),
            0xef => {
                let s = self.drive().map(|d| d.poll()).unwrap_or_default();
                0xe0 | u8::from(!s.write_protected) | u8::from(!s.sync) << 1 | u8::from(s.gap) << 2
            }
            // RS-232 idle, no network traffic.
            _ => 0x7e,
        }
    }

    pub fn write(&mut self, port: u16, v: u8) {
        match port & 0xff {
            0xe7 => {
                if let Some(d) = self.drive() {
                    d.write(v);
                }
            }
            0xef => {
                let old = self.control;
                self.control = v;
                if old & 2 != 0 && v & 2 == 0 {
                    self.select = self.select << 1 | u8::from(v & 1 == 0);
                    self.motors();
                }
                let writing = v & 4 == 0;
                if let Some(d) = self.drive() {
                    d.set_write(writing);
                }
            }
            _ => {}
        }
    }

    /// Drive `n` (0-based) runs.
    pub fn motor(&self, n: usize) -> bool {
        self.select & (1 << n) != 0
    }
}

const TAG: [u8; 4] = *b"ZIF1";

impl Savestate for Interface1 {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, 1, |w| {
            w.u8(self.select);
            w.u8(self.control);
            w.u8(self.drives.len() as u8);
        });
        for d in &self.drives {
            d.save(w);
        }
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, 1)?;
        let (select, control) = (c.u8()?, c.u8()?);
        if usize::from(c.u8()?) != self.drives.len() {
            return Err(StateError::Invalid("Microdrive count differs"));
        }
        for d in &mut self.drives {
            d.load(r)?;
        }
        self.select = select;
        self.control = control;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SEL-DRIVE as the ROM does it: eight clocks, drive `n`'s 0 at the
    /// (9 − n)th.
    fn select(i: &mut Interface1, n: u8) {
        for k in 1..=8u8 {
            let data = u8::from(k != 9 - n);
            i.write(0xef, 0xee | data);
            i.write(0xef, 0xec | data);
        }
    }

    #[test]
    fn drive_select_shift_register() {
        let mut i = Interface1::new(8);
        select(&mut i, 2);
        assert!(i.motor(1));
        assert_eq!((0..8).filter(|&n| i.motor(n)).count(), 1);
        select(&mut i, 0);
        assert!((0..8).all(|n| !i.motor(n)), "all off");
        // No cartridge: no gap, no SYNC.
        select(&mut i, 1);
        assert_eq!(i.read(0xef) & 6, 0b010);
        assert!(Interface1::claims(0x00ef) && !Interface1::claims(0x00fe));
    }
}
