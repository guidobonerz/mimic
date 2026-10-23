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
//! every field the Copper restarts at COP1LC. After a restart the Copper
//! lets one of its cycles (an even slot) pass before it fetches the first
//! instruction word (observed on hardware: a Copper list starting with a
//! no-op MOVE at the line start needs it; see `docs/sources.md`). The
//! 68000 gets that slot, which code polling VHPOSR for line 0 relies on.
//!
//! After a satisfied WAIT the Copper wakes up in one even slot of its own
//! (it holds the bus; the CPU does not get it) before it fetches again.
//! The common way past line 255, `WAIT $FF,$DE` followed by a WAIT for a
//! line below $2C, depends on it: the second WAIT is fetched in line 256,
//! where the beam's low eight bits start over (Rule 30's kaleidoscope).
//! Measured against a cycle-exact A500 configuration with the bus test of
//! `tools/amiga_bustest.py` (`docs/sources.md`).

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

/// Disassemble a Copper instruction: mnemonic and operand.
///
/// `MOVE #$0F00,COLOR00`; `WAIT $2C,$40` (vertical, horizontal; masks
/// that are not all ones follow as `mask $vv,$hh`; `,blitter` when BFD is
/// clear, i.e. the WAIT also waits for the Blitter); `SKIP` likewise.
pub fn disassemble(ir1: u16, ir2: u16) -> (String, String) {
    if ir1 & 1 == 0 {
        return (
            "MOVE".into(),
            format!("#${ir2:04X},{}", crate::regnames::name(ir1)),
        );
    }
    let mnemonic = if ir2 & 1 == 0 { "WAIT" } else { "SKIP" };
    let mut operand = format!("${:02X},${:02X}", ir1 >> 8, ir1 & 0xfe);
    let (ve, he) = ((ir2 >> 8) & 0x7f, ir2 & 0xfe);
    if ve != 0x7f || he != 0xfe {
        operand += &format!(" mask ${:02X},${:02X}", ve | 0x80, he);
    }
    if ir2 & 0x8000 == 0 {
        operand += ",blitter";
    }
    (mnemonic.into(), operand)
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
    /// ECS Agnus: with CDANG every register is reachable (configuration).
    pub ecs: bool,
    /// Instruction addresses fetched since [`Copper::take_fetched`] while
    /// `watch` is set (debugger exec breakpoints); not state.
    pub watch: bool,
    fetched: Vec<u32>,
    /// Even slots to let pass before the next fetch (restart wake-up).
    wake: u8,
    /// Even slots the Copper takes without fetching after a satisfied
    /// WAIT (its wake-up cycle; it holds the bus, the CPU waits).
    pub busy_wake: u8,
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
            ecs: false,
            watch: false,
            fetched: Vec::new(),
            wake: 0,
            busy_wake: 0,
        }
    }

    /// Addresses of the instructions started since the last call.
    pub fn take_fetched(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.fetched)
    }

    pub fn jump(&mut self, second: bool) {
        self.pc = if second { self.cop2lc } else { self.cop1lc };
        self.state = State::Fetch1;
        // Counted down on the even slot of the restart (if any) and the
        // next one: the first fetch comes one Copper cycle later.
        self.wake = 2;
    }

    /// Every colour clock: a WAIT that is satisfied releases the Copper
    /// for its next fetch; the wake-up after a restart runs down.
    pub fn check_wait(&mut self, v: u16, h: u16, blitter_busy: bool) {
        if h & 1 == 0 && self.wake > 0 {
            self.wake -= 1;
        }
        if let State::Wait(ir1, ir2) = self.state {
            if beam_reached(ir1, ir2, v, h, blitter_busy) {
                self.state = State::Fetch1;
                // Waking up takes the Copper one even slot of its own.
                self.busy_wake = 1;
            }
        }
    }

    /// Does the Copper want this (even, otherwise free) slot?
    pub fn wants_slot(&self) -> bool {
        self.wake == 0
            && self.busy_wake == 0
            && matches!(self.state, State::Fetch1 | State::Fetch2(_))
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
                if self.watch {
                    self.fetched.push(self.pc);
                }
                self.pc = self.pc.wrapping_add(2);
                self.state = State::Fetch2(word);
                None
            }
            State::Fetch2(ir1) => {
                self.pc = self.pc.wrapping_add(2);
                self.state = State::Fetch1;
                if ir1 & 1 == 0 {
                    let reg = ir1 & 0x1fe;
                    let floor = if !self.danger {
                        0x80
                    } else if self.ecs {
                        0
                    } else {
                        0x40
                    };
                    if reg < floor {
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
        w.u8(self.wake);
        w.u8(self.busy_wake);
    }

    /// `version`: the enclosing chunk's; 1 had no wake-up count, before 5
    /// no wake-up after WAIT.
    pub(crate) fn load(r: &mut StateReader<'_>, version: u16) -> Result<Self, StateError> {
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
            ecs: false,
            watch: false,
            fetched: Vec::new(),
            wake: if version >= 2 { r.u8()?.min(2) } else { 0 },
            busy_wake: if version >= 5 { r.u8()?.min(1) } else { 0 },
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
    fn disassembles_moves_waits_and_skips() {
        assert_eq!(
            disassemble(0x0180, 0x0f00),
            ("MOVE".into(), "#$0F00,COLOR00".into())
        );
        assert_eq!(
            disassemble(0x2c41, 0xfffe),
            ("WAIT".into(), "$2C,$40".into())
        );
        assert_eq!(
            disassemble(0x0001, 0x7f01),
            ("SKIP".into(), "$00,$00 mask $FF,$00,blitter".into())
        );
    }

    #[test]
    fn a_restart_lets_one_copper_cycle_pass() {
        let mut c = Copper::new();
        c.jump(false);
        assert!(!c.wants_slot());
        c.check_wait(0, 0, false);
        assert!(!c.wants_slot(), "the restart's own slot passes");
        c.check_wait(0, 1, false);
        assert!(!c.wants_slot());
        c.check_wait(0, 2, false);
        assert!(c.wants_slot(), "fetching from the second even slot on");
    }

    #[test]
    fn a_satisfied_wait_takes_one_copper_cycle() {
        let mut c = Copper::new();
        c.state = State::Wait(0x2c41, 0xfffe);
        c.check_wait(0x2c, 0x40, false);
        assert_eq!(c.state, State::Fetch1);
        assert!(!c.wants_slot(), "the wake-up slot comes first");
        assert_eq!(c.busy_wake, 1);
        c.busy_wake = 0; // the custom chips take it on the next even slot
        assert!(c.wants_slot());
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
