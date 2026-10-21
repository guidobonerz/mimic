//! The Copper: Agnus' coprocessor that writes custom registers in step
//! with the beam (HRM chapter 2, "Coprocessor hardware").
//!
//! Instructions are two words, fetched in two even-numbered DMA slots:
//!
//! - **MOVE** (first word bit 0 = 0): register address (bits 8–1) and data.
//!   Registers below $40 are never written, $40–$7F only with COPCON CDANG
//!   (bit 1); a forbidden MOVE stops the Copper until the next restart.
//! - **WAIT** (bit 0 = 1, second word bit 0 = 0) and **SKIP** (both bit 0
//!   = 1): vertical position bits 15–8 and horizontal bits 7–1 of the
//!   first word, compare-enable masks in the second (vertical bit 7 is
//!   always compared); bit 15 of the second word (BFD) = 0 also waits for
//!   the Blitter to finish. The condition is beam ≥ position on the masked
//!   bits. SKIP jumps over the next instruction if the condition holds.
//!
//! COPJMP1/COPJMP2 (strobes) restart at COP1LC/COP2LC; at the start of
//! every field the Copper restarts at COP1LC.

use emu_core::{StateError, StateReader, StateWriter};

use crate::chipram::ChipRam;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Fetch the first instruction word.
    Fetch1,
    /// Fetch the second word; the first is held.
    Fetch2(u16),
    /// WAIT for the beam (first word, second word).
    Wait(u16, u16),
    /// After a forbidden MOVE.
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copper {
    pub cop1lc: u32,
    pub cop2lc: u32,
    /// Address of the next instruction word.
    pub pc: u32,
    pub state: State,
    /// COPCON CDANG: MOVEs to $40–$7F allowed.
    pub danger: bool,
}

impl Default for Copper {
    fn default() -> Self {
        Self::new()
    }
}

/// Does the beam (`v` line, `h` colour clock) satisfy a WAIT/SKIP?
pub fn beam_reached(ir1: u16, ir2: u16, v: u16, h: u16, blitter_busy: bool) -> bool {
    if ir2 & 0x8000 == 0 && blitter_busy {
        return false;
    }
    let ve = ((ir2 >> 8) & 0x7f) | 0x80;
    let he = ir2 & 0xfe;
    let beam = ((v & 0xff & ve) << 8) | (h & 0xfe & he);
    let pos = (((ir1 >> 8) & ve) << 8) | (ir1 & 0xfe & he);
    beam >= pos
}

impl Copper {
    pub fn new() -> Self {
        Self {
            cop1lc: 0,
            cop2lc: 0,
            pc: 0,
            state: State::Fetch1,
            danger: false,
        }
    }

    pub fn jump(&mut self, second: bool) {
        self.pc = if second { self.cop2lc } else { self.cop1lc };
        self.state = State::Fetch1;
    }

    /// Every colour clock: a WAIT that is satisfied releases the Copper
    /// for its next fetch.
    pub fn check_wait(&mut self, v: u16, h: u16, blitter_busy: bool) {
        if let State::Wait(ir1, ir2) = self.state {
            if beam_reached(ir1, ir2, v, h, blitter_busy) {
                self.state = State::Fetch1;
            }
        }
    }

    /// Does the Copper want this (even, otherwise free) slot?
    pub fn wants_slot(&self) -> bool {
        matches!(self.state, State::Fetch1 | State::Fetch2(_))
    }

    /// Use one DMA slot. Returns a register MOVE (offset, value) to perform.
    pub fn cycle(
        &mut self,
        chip: &ChipRam,
        v: u16,
        h: u16,
        blitter_busy: bool,
    ) -> Option<(u16, u16)> {
        let word = chip.word(self.pc);
        match self.state {
            State::Fetch1 => {
                self.pc = self.pc.wrapping_add(2);
                self.state = State::Fetch2(word);
                None
            }
            State::Fetch2(ir1) => {
                self.pc = self.pc.wrapping_add(2);
                self.state = State::Fetch1;
                if ir1 & 1 == 0 {
                    let reg = ir1 & 0x1fe;
                    if reg < 0x40 || (reg < 0x80 && !self.danger) {
                        self.state = State::Stopped;
                        return None;
                    }
                    return Some((reg, word));
                }
                if word & 1 == 0 {
                    // WAIT: checked from the next colour clock on.
                    self.state = State::Wait(ir1, word);
                } else if beam_reached(ir1, word, v, h, blitter_busy) {
                    // SKIP
                    self.pc = self.pc.wrapping_add(4);
                }
                None
            }
            State::Wait(..) | State::Stopped => None,
        }
    }

    pub(crate) fn save(&self, w: &mut StateWriter) {
        w.u32(self.cop1lc);
        w.u32(self.cop2lc);
        w.u32(self.pc);
        let (kind, a, b) = match self.state {
            State::Fetch1 => (0, 0, 0),
            State::Fetch2(a) => (1, a, 0),
            State::Wait(a, b) => (2, a, b),
            State::Stopped => (3, 0, 0),
        };
        w.u8(kind);
        w.u16(a);
        w.u16(b);
        w.bool(self.danger);
    }

    pub(crate) fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let cop1lc = r.u32()?;
        let cop2lc = r.u32()?;
        let pc = r.u32()?;
        let kind = r.u8()?;
        let a = r.u16()?;
        let b = r.u16()?;
        let state = match kind {
            0 => State::Fetch1,
            1 => State::Fetch2(a),
            2 => State::Wait(a, b),
            3 => State::Stopped,
            _ => return Err(StateError::Invalid("Copper state")),
        };
        Ok(Self {
            cop1lc,
            cop2lc,
            pc,
            state,
            danger: r.bool()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_compares_masked_beam_positions() {
        // WAIT line $2C, h $40, all bits.
        assert!(!beam_reached(0x2c41, 0xfffe, 0x2b, 0xe0, false));
        assert!(!beam_reached(0x2c41, 0xfffe, 0x2c, 0x3e, false));
        assert!(beam_reached(0x2c41, 0xfffe, 0x2c, 0x40, false));
        assert!(beam_reached(0x2c41, 0xfffe, 0x2d, 0x00, false));
        // Vertical bit 8 is not compared: line 256 reads as 0.
        assert!(!beam_reached(0x2c41, 0xfffe, 0x100, 0x00, false));
        // Horizontal compare disabled.
        assert!(beam_reached(0x2c01, 0xff00, 0x2c, 0x00, false));
        // BFD clear waits for the Blitter.
        assert!(!beam_reached(0x2c01, 0x7f00, 0x2c, 0x00, true));
    }

    #[test]
    fn move_wait_and_danger() {
        let mut chip = ChipRam::new(0x1_0000);
        for (i, w) in [0x0180u16, 0x0f00, 0x0101, 0xff00, 0x0040, 0x0000]
            .iter()
            .enumerate()
        {
            chip.set_word(0x100 + 2 * i as u32, *w);
        }
        let mut c = Copper::new();
        c.cop1lc = 0x100;
        c.jump(false);
        assert_eq!(c.cycle(&chip, 0, 0, false), None);
        assert_eq!(c.cycle(&chip, 0, 2, false), Some((0x180, 0x0f00)));
        c.cycle(&chip, 0, 4, false);
        c.cycle(&chip, 0, 6, false);
        assert_eq!(c.state, State::Wait(0x0101, 0xff00));
        c.check_wait(0, 8, false);
        assert_eq!(c.state, State::Wait(0x0101, 0xff00));
        c.check_wait(1, 0, false);
        assert_eq!(c.state, State::Fetch1);
        c.cycle(&chip, 1, 2, false);
        assert_eq!(c.cycle(&chip, 1, 4, false), None, "BLTCON0 needs CDANG");
        assert_eq!(c.state, State::Stopped);
    }
}
