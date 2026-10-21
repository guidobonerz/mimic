//! Amiga 3.5" floppy drive mechanics: select, motor, head stepping, the
//! status lines and the drive identification, on the CIA lines the Amiga
//! wires them to (HRM appendix F; chapter 8 "Disk"):
//!
//! - CIA-B port B (outputs): 7 /MTR, 6–3 /SEL3–/SEL0, 2 /SIDE, 1 DIR
//!   (1 = outward, towards cylinder 0), 0 /STEP.
//! - CIA-A port A (inputs, low active): 5 /RDY, 4 /TK0, 3 /WPRO, 2 /CHNG.
//!
//! A drive latches /MTR when it is selected. /STEP pulses move the head on
//! the falling edge. /CHNG goes low when the disk is removed and stays low
//! until a step pulse with a disk inside. With the motor off, /RDY
//! carries the drive's 32-bit identification, one bit per select: turning
//! the motor on and off restarts it (3.5" double-density drives send all
//! ones). The disk itself (MFM tracks, DMA) follows in milestone M3; until
//! then a drive is always empty.

use emu_core::{StateError, StateReader, StateWriter};

/// Identification of a 3.5" double-density drive.
pub const ID_35_DD: u32 = 0xffff_ffff;
/// Highest cylinder the head reaches.
pub const LAST_CYLINDER: u8 = 81;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmigaDrive {
    pub id: u32,
    pub motor: bool,
    pub selected: bool,
    pub cylinder: u8,
    /// /SIDE low: the upper head.
    pub upper: bool,
    pub has_disk: bool,
    pub write_protected: bool,
    /// /CHNG asserted (low): the disk was removed.
    pub changed: bool,
    id_shift: u32,
    id_bit: bool,
    step_level: bool,
}

impl Default for AmigaDrive {
    fn default() -> Self {
        Self::new()
    }
}

impl AmigaDrive {
    pub fn new() -> Self {
        Self {
            id: ID_35_DD,
            motor: false,
            selected: false,
            cylinder: 0,
            upper: false,
            has_disk: false,
            write_protected: false,
            changed: true,
            id_shift: ID_35_DD,
            id_bit: false,
            step_level: true,
        }
    }

    /// CIA-B port B changed; `sel` is this drive's /SELx bit (3–6).
    pub fn write_prb(&mut self, prb: u8, sel: u8) {
        let selected = prb & (1 << sel) == 0;
        if selected && !self.selected {
            let motor = prb & 0x80 == 0;
            if self.motor && !motor {
                // Motor turned off: the identification starts again.
                self.id_shift = self.id;
            }
            if !motor && !self.motor {
                self.id_bit = self.id_shift & 0x8000_0000 != 0;
                self.id_shift <<= 1;
            }
            self.motor = motor;
        }
        self.selected = selected;
        if selected {
            self.upper = prb & 0x04 == 0;
            let step = prb & 0x01 != 0;
            if self.step_level && !step {
                if prb & 0x02 != 0 {
                    self.cylinder = self.cylinder.saturating_sub(1);
                } else if self.cylinder < LAST_CYLINDER {
                    self.cylinder += 1;
                }
                if self.has_disk {
                    self.changed = false;
                }
            }
            self.step_level = step;
        }
    }

    /// CIA-A port A bits 5–2 this drive pulls low while selected (1 = low).
    pub fn pulled_low(&self) -> u8 {
        if !self.selected {
            return 0;
        }
        let rdy = if self.motor { true } else { self.id_bit };
        u8::from(rdy) << 5
            | u8::from(self.cylinder == 0) << 4
            | u8::from(self.has_disk && self.write_protected) << 3
            | u8::from(self.changed) << 2
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"AFDD", 1, |w| {
            w.u32(self.id);
            w.u32(self.id_shift);
            w.u8(self.cylinder);
            for b in [
                self.motor,
                self.selected,
                self.upper,
                self.has_disk,
                self.write_protected,
                self.changed,
                self.id_bit,
                self.step_level,
            ] {
                w.bool(b);
            }
        });
    }

    /// Atomic: on error the drive is unchanged.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"AFDD", 1)?;
        let mut n = Self::new();
        n.id = c.u32()?;
        n.id_shift = c.u32()?;
        n.cylinder = c.u8()?;
        if n.cylinder > LAST_CYLINDER {
            return Err(StateError::Invalid("floppy cylinder"));
        }
        for b in [
            &mut n.motor,
            &mut n.selected,
            &mut n.upper,
            &mut n.has_disk,
            &mut n.write_protected,
            &mut n.changed,
            &mut n.id_bit,
            &mut n.step_level,
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

    /// /SEL0 low with the other lines as given.
    fn sel(d: &mut AmigaDrive, motor: bool, step: bool, outward: bool) {
        let prb = 0x78 & !0x08
            | if motor { 0 } else { 0x80 }
            | if step { 1 } else { 0 }
            | if outward { 2 } else { 0 }
            | 0x04;
        d.write_prb(prb, 3);
    }

    fn deselect(d: &mut AmigaDrive) {
        d.write_prb(0xff, 3);
    }

    #[test]
    fn steps_and_reports_track_zero() {
        let mut d = AmigaDrive::new();
        sel(&mut d, true, true, false);
        assert!(d.motor);
        assert_eq!(d.pulled_low() & 0x30, 0x30, "ready, track 0");
        for _ in 0..3 {
            sel(&mut d, true, false, false);
            sel(&mut d, true, true, false);
        }
        assert_eq!(d.cylinder, 3);
        assert_eq!(d.pulled_low() & 0x10, 0);
        assert_eq!(d.pulled_low() & 0x04, 0x04, "no disk: /CHNG stays low");
        deselect(&mut d);
        assert_eq!(d.pulled_low(), 0, "deselected");
    }

    #[test]
    fn identification_sends_all_ones() {
        let mut d = AmigaDrive::new();
        sel(&mut d, true, true, false);
        deselect(&mut d);
        sel(&mut d, false, true, false);
        deselect(&mut d);
        for _ in 0..32 {
            sel(&mut d, false, true, false);
            assert_eq!(d.pulled_low() & 0x20, 0x20);
            deselect(&mut d);
        }
    }

    #[test]
    fn savestate_round_trip() {
        let mut d = AmigaDrive::new();
        sel(&mut d, true, false, false);
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        let mut e = AmigaDrive::new();
        e.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(e, d);
        assert!(e.load(&mut StateReader::new(&data[..4])).is_err());
    }
}
