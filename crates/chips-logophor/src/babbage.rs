//! Babbage, the Logophor 16's bus arbiter and DMA chip (SPEC.md sections
//! 5 and 6): DMA control at FFFF:0000 and the Jacquard at FFFF:0020.
//! Built so far (steps 4 and 5): DMACON, DMAERR, BUSCON, the Jacquard and
//! the Lithograph ([`crate::lithograph`], registers at FFFF:0100) and the
//! Stereoscope ([`crate::stereoscope`], FFFF:0200).
//!
//! The Jacquard runs one instruction (two words, one chip-bus slot) per free
//! slot it gets: [`Babbage::jacquard_slot`] is called by the machine for
//! each slot the fixed DMA leaves, with the beam there, and says what the
//! instruction does outside the Jacquard (a register write, an interrupt,
//! an error). It restarts at list 1 when a field begins (line 0, x 0).

use emu_core::{StateError, StateReader, StateWriter};

use crate::lithograph::{ChipRamMut, Lithograph};
use crate::lumiere::ChipRam;
use crate::stereoscope::Stereoscope;

pub const BABID: u16 = 0x0000;
pub const DMACON: u16 = 0x0002;
pub const DMAERR: u16 = 0x0004;
pub const BUSCON: u16 = 0x0006;
pub const JAQ1SEG: u16 = 0x0020;
pub const JAQ1OFS: u16 = 0x0022;
pub const JAQ2SEG: u16 = 0x0024;
pub const JAQ2OFS: u16 = 0x0026;
pub const JAQJMP1: u16 = 0x0028;
pub const JAQJMP2: u16 = 0x002a;
pub const JAQCON: u16 = 0x002c;
pub const JAQIRQ: u16 = 0x002e;

/// DMACON bits.
pub const DMA_FRAMEBUFFER: u16 = 1 << 0;
pub const DMA_SPRITES: u16 = 1 << 1;
pub const DMA_CALLIOPE: u16 = 1 << 2;
pub const DMA_TELEGRAPH: u16 = 1 << 3;
pub const DMA_JACQUARD: u16 = 1 << 4;
pub const DMA_LITHOGRAPH: u16 = 1 << 5;
pub const DMA_STEREOSCOPE: u16 = 1 << 6;
pub const DMA_PHONOGRAPH: u16 = 1 << 7;
pub const DMA_MASTER: u16 = 1 << 14;

/// The end of chip RAM (segments 0–$3F).
const CHIP_END: u32 = 0x0040_0000;

/// What a Jacquard slot did that the machine must carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JacquardAction {
    None,
    /// Write `value` to the I/O register at FFFF:`offset`.
    Move {
        offset: u16,
        value: u16,
    },
    /// Raise the Jacquard interrupt (JAQIRQ holds the value).
    Irq,
    /// Start the Lithograph job (no Lithograph yet).
    Lith,
    /// The Jacquard stopped on an error (DMAERR bit 4 is set): raise the
    /// DMA-error interrupt.
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Fetching instructions.
    #[default]
    Run,
    /// WAIT for the beam at or past (y, x), and with `lithograph` the Lithograph
    /// idle.
    Wait { y: u16, x: u16, lithograph: bool },
    /// STOP, an error, or Jacquard DMA off: idle until the next field.
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Babbage {
    dmacon: u16,
    dmaerr: u16,
    buscon: u16,
    jaq1: (u16, u16),
    jaq2: (u16, u16),
    jaqcon: u16,
    jaqirq: u16,
    /// The Jacquard: next instruction (linear) and state.
    pc: u32,
    state: State,
    pub lithograph: Lithograph,
    pub stereo: Stereoscope,
    /// Lithograph and Stereoscope take turns when both want a slot.
    turn: bool,
    /// A start the CPU wrote failed on a pointer (DMAERR is set): the
    /// machine raises the DMA-error interrupt ([`Babbage::take_error`]).
    error_pending: bool,
}

impl Babbage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// DMA enabled for a unit (its DMACON bit and the master bit).
    pub fn dma(&self, bit: u16) -> bool {
        self.dmacon & DMA_MASTER != 0 && self.dmacon & bit != 0
    }

    /// The CPU before Lithograph/Stereoscope (BUSCON bit 0).
    pub fn cpu_first(&self) -> bool {
        self.buscon & 1 != 0
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            BABID => 0xba01,
            DMACON => self.dmacon,
            DMAERR => self.dmaerr,
            BUSCON => self.buscon,
            JAQ1SEG => self.jaq1.0,
            JAQ1OFS => self.jaq1.1,
            JAQ2SEG => self.jaq2.0,
            JAQ2OFS => self.jaq2.1,
            JAQCON => self.jaqcon,
            JAQIRQ => self.jaqirq,
            0x0100..=0x017f => self.lithograph.peek(off),
            0x0200..=0x02ff => self.stereo.peek(off),
            _ => 0,
        }
    }

    /// Register read (the Lithograph's done bit clears).
    pub fn read(&mut self, off: u16) -> u16 {
        match off & !1 {
            0x0100..=0x017f => self.lithograph.read(off),
            0x0200..=0x02ff => self.stereo.read(off),
            _ => self.peek(off),
        }
    }

    /// A Lithograph start failed since the last call.
    pub fn take_error(&mut self) -> bool {
        std::mem::take(&mut self.error_pending)
    }

    fn start_lith(&mut self) -> bool {
        let ok = self.lithograph.start();
        if !ok {
            self.dmaerr |= DMA_LITHOGRAPH;
        }
        ok
    }

    /// The Lithograph has a job and its DMA is on.
    pub fn lithograph_wants(&self) -> bool {
        self.lithograph.busy() && self.dma(DMA_LITHOGRAPH)
    }

    /// One free slot for the Lithograph; `true` when its job finished.
    pub fn lithograph_slot(&mut self, mem: &mut dyn ChipRamMut) -> bool {
        self.lithograph.slot(mem)
    }

    /// The Stereoscope has work and its DMA is on.
    pub fn stereo_wants(&self) -> bool {
        self.stereo.busy() && self.dma(DMA_STEREOSCOPE)
    }

    /// Who gets a free slot both want: they take turns (SPEC.md 5);
    /// `true` for the Stereoscope.
    pub fn stereo_turn(&mut self) -> bool {
        self.turn = !self.turn;
        self.turn
    }

    /// A display list failed (S3DSTAT bit 2): DMAERR bit 6.
    pub fn stereo_error(&mut self) {
        self.dmaerr |= DMA_STEREOSCOPE;
    }

    /// The Phonograph's ring lies outside chip RAM: DMAERR bit 7.
    pub fn phonograph_error(&mut self) {
        self.dmaerr |= DMA_PHONOGRAPH;
    }

    pub fn write(&mut self, off: u16, v: u16) {
        match off & !1 {
            DMACON => {
                // Set/clear: bit 15 says which (SPEC.md 4.1).
                if v & 0x8000 != 0 {
                    self.dmacon |= v & 0x7fff;
                } else {
                    self.dmacon &= !v;
                }
                if !self.dma(DMA_JACQUARD) {
                    self.state = State::Stopped;
                }
            }
            DMAERR => self.dmaerr &= !v,
            BUSCON => self.buscon = v & 1,
            JAQ1SEG => self.jaq1.0 = v,
            JAQ1OFS => self.jaq1.1 = v,
            JAQ2SEG => self.jaq2.0 = v,
            JAQ2OFS => self.jaq2.1 = v,
            JAQJMP1 => self.jump(self.jaq1),
            JAQJMP2 => self.jump(self.jaq2),
            JAQCON => self.jaqcon = v & 1,
            0x0100..=0x017f if self.lithograph.write(off, v) && !self.start_lith() => {
                self.error_pending = true;
            }
            0x0200..=0x02ff => self.stereo.write(off, v),
            _ => {}
        }
    }

    fn jump(&mut self, to: (u16, u16)) {
        self.pc = u32::from(to.0) << 16 | u32::from(to.1);
        self.state = if self.dma(DMA_JACQUARD) {
            State::Run
        } else {
            State::Stopped
        };
    }

    /// A field begins: the Jacquard restarts at list 1.
    pub fn field_start(&mut self) {
        self.jump(self.jaq1);
    }

    /// The Jacquard wants this free slot (it would fetch or check a WAIT).
    pub fn jacquard_active(&self) -> bool {
        self.dma(DMA_JACQUARD) && self.state != State::Stopped
    }

    /// The Jacquard is waiting (it takes no slot, it only compares).
    pub fn jacquard_waiting(&self) -> bool {
        matches!(self.state, State::Wait { .. })
    }

    /// The beam position a waiting Jacquard compares with, and whether it
    /// also waits for the Lithograph (for the machine's look-ahead: no slot
    /// before that position needs the Jacquard).
    pub fn jacquard_wait(&self) -> Option<(u16, u16, bool)> {
        match self.state {
            State::Wait { y, x, lithograph } => Some((y, x, lithograph)),
            _ => None,
        }
    }

    /// The Jacquard's next instruction (linear address).
    pub fn jacquard_pc(&self) -> u32 {
        self.pc
    }

    fn stop_with_error(&mut self) -> JacquardAction {
        self.dmaerr |= DMA_JACQUARD;
        self.state = State::Stopped;
        JacquardAction::Error
    }

    /// Check a waiting Jacquard against the beam at (`y`, `x`); true when it
    /// runs again. Costs no slot.
    pub fn jacquard_compare(&mut self, y: u16, x: u16) -> bool {
        if let State::Wait {
            y: wy,
            x: wx,
            lithograph,
        } = self.state
        {
            if (y, x) >= (wy, wx) && !(lithograph && self.lithograph.busy()) {
                self.state = State::Run;
                return true;
            }
        }
        false
    }

    /// One free chip-bus slot for a running Jacquard, the beam at (`y`, `x`):
    /// fetch and run one instruction.
    pub fn jacquard_slot(&mut self, y: u16, x: u16, mem: &dyn ChipRam) -> JacquardAction {
        if self.state != State::Run || !self.dma(DMA_JACQUARD) {
            return JacquardAction::None;
        }
        if self.pc >= CHIP_END - 3 {
            return self.stop_with_error();
        }
        let word = |a: u32| u16::from_le_bytes([mem.chip_byte(a), mem.chip_byte(a + 1)]);
        let (w0, w1) = (word(self.pc), word(self.pc + 2));
        self.pc += 4;
        match w0 >> 13 {
            0 => {
                let offset = (w0 & 0x1fff) << 1;
                if offset < 0x0800 && self.jaqcon & 1 == 0 {
                    return self.stop_with_error();
                }
                JacquardAction::Move { offset, value: w1 }
            }
            1 => {
                self.state = State::Wait {
                    y: w0 & 0x3ff,
                    x: w1 & 0x7ff,
                    lithograph: w1 & 0x8000 != 0,
                };
                // A WAIT that already holds goes on with the next slot.
                self.jacquard_compare(y, x);
                JacquardAction::None
            }
            2 => {
                if (y, x) >= (w0 & 0x3ff, w1 & 0x7ff) {
                    self.pc += 4;
                }
                JacquardAction::None
            }
            3 => {
                self.pc = if w0 & 1 == 0 {
                    u32::from(self.jaq1.0) << 16 | u32::from(self.jaq1.1)
                } else {
                    u32::from(self.jaq2.0) << 16 | u32::from(self.jaq2.1)
                };
                JacquardAction::None
            }
            4 => {
                if self.start_lith() {
                    JacquardAction::Lith
                } else {
                    self.state = State::Stopped;
                    JacquardAction::Error
                }
            }
            5 => {
                self.jaqirq = w1;
                JacquardAction::Irq
            }
            _ => {
                self.state = State::Stopped;
                JacquardAction::None
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"BABB", 3, |w| {
            for v in [
                self.dmacon,
                self.dmaerr,
                self.buscon,
                self.jaq1.0,
                self.jaq1.1,
                self.jaq2.0,
                self.jaq2.1,
                self.jaqcon,
                self.jaqirq,
            ] {
                w.u16(v);
            }
            w.u32(self.pc);
            match self.state {
                State::Run => w.u8(0),
                State::Wait { y, x, lithograph } => {
                    w.u8(1);
                    w.u16(y);
                    w.u16(x);
                    w.bool(lithograph);
                }
                State::Stopped => w.u8(2),
            }
            self.lithograph.save(w);
            w.bool(self.error_pending);
            self.stereo.save(w);
            w.bool(self.turn);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"BABB", 3)?;
        let mut b = Self {
            dmacon: c.u16()? & 0x7fff,
            dmaerr: c.u16()?,
            buscon: c.u16()? & 1,
            jaq1: (c.u16()?, c.u16()?),
            jaq2: (c.u16()?, c.u16()?),
            jaqcon: c.u16()? & 1,
            jaqirq: c.u16()?,
            pc: c.u32()?,
            state: State::Run,
            lithograph: Lithograph::default(),
            stereo: Stereoscope::default(),
            turn: false,
            error_pending: false,
        };
        b.state = match c.u8()? {
            0 => State::Run,
            1 => State::Wait {
                y: c.u16()? & 0x3ff,
                x: c.u16()? & 0x7ff,
                lithograph: c.bool()?,
            },
            2 => State::Stopped,
            _ => return Err(StateError::Invalid("Jacquard state")),
        };
        b.lithograph.load(&mut c)?;
        b.error_pending = c.bool()?;
        b.stereo.load(&mut c)?;
        b.turn = c.bool()?;
        *self = b;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    /// A Jacquard list at 0001:0000.
    fn list(words: &[u16]) -> Ram {
        let mut r = vec![0; 0x2_0000];
        for (i, w) in words.iter().enumerate() {
            r[0x1_0000 + 2 * i..0x1_0000 + 2 * i + 2].copy_from_slice(&w.to_le_bytes());
        }
        Ram(r)
    }

    fn jacquard(on: bool) -> Babbage {
        let mut b = Babbage::new();
        b.write(JAQ1SEG, 1);
        b.write(
            DMACON,
            0x8000 | DMA_MASTER | if on { DMA_JACQUARD } else { 0 },
        );
        b.field_start();
        b
    }

    #[test]
    fn dmacon_sets_and_clears() {
        let mut b = Babbage::new();
        b.write(DMACON, 0x8000 | DMA_MASTER | DMA_FRAMEBUFFER | DMA_JACQUARD);
        assert!(b.dma(DMA_FRAMEBUFFER) && b.dma(DMA_JACQUARD));
        b.write(DMACON, DMA_JACQUARD);
        assert!(b.dma(DMA_FRAMEBUFFER) && !b.dma(DMA_JACQUARD));
        b.write(DMACON, DMA_MASTER);
        assert!(!b.dma(DMA_FRAMEBUFFER), "master off");
        assert_eq!(b.peek(BABID), 0xba01);
    }

    #[test]
    fn jacquard_instructions() {
        let ram = list(&[
            0x0800 >> 1,
            0x8004, // MOVE VMODE
            0x2000 | 10,
            100, // WAIT 10, 100
            0x4000 | 10,
            200, // SKIP 10, 200 (not yet)
            0xa000,
            0x55aa, // IRQ
            0x8000,
            0, // LITH
            0xc000,
            0, // STOP
        ]);
        let mut b = jacquard(true);
        assert_eq!(
            b.jacquard_slot(0, 0, &ram),
            JacquardAction::Move {
                offset: 0x0800,
                value: 0x8004
            }
        );
        assert_eq!(b.jacquard_slot(0, 1, &ram), JacquardAction::None);
        assert!(b.jacquard_waiting());
        assert!(!b.jacquard_compare(10, 99));
        assert!(b.jacquard_compare(10, 100));
        assert_eq!(
            b.jacquard_slot(10, 101, &ram),
            JacquardAction::None,
            "SKIP not yet"
        );
        assert_eq!(b.jacquard_slot(10, 102, &ram), JacquardAction::Irq);
        assert_eq!(b.peek(JAQIRQ), 0x55aa);
        assert_eq!(b.jacquard_slot(10, 103, &ram), JacquardAction::Lith);
        assert_eq!(b.jacquard_slot(10, 104, &ram), JacquardAction::None);
        assert!(!b.jacquard_active(), "STOP");
        b.field_start();
        assert!(b.jacquard_active(), "again from list 1");
        assert!(!jacquard(false).jacquard_active(), "no Jacquard DMA");
    }

    #[test]
    fn jacquard_errors_stop_it() {
        let ram = list(&[0x0002 >> 1, 0x8000]);
        let mut b = jacquard(true);
        assert_eq!(
            b.jacquard_slot(0, 0, &ram),
            JacquardAction::Error,
            "Babbage without JAQCON"
        );
        assert_eq!(b.peek(DMAERR), DMA_JACQUARD);
        b.write(DMAERR, DMA_JACQUARD);
        b.write(JAQCON, 1);
        b.field_start();
        assert_eq!(
            b.jacquard_slot(0, 0, &ram),
            JacquardAction::Move {
                offset: 2,
                value: 0x8000
            }
        );
        b.write(JAQ1SEG, 0x40);
        b.field_start();
        assert_eq!(
            b.jacquard_slot(0, 0, &ram),
            JacquardAction::Error,
            "outside chip RAM"
        );
    }

    #[test]
    fn savestate_round_trip() {
        let ram = list(&[0x2000 | 300, 5]);
        let mut b = jacquard(true);
        b.jacquard_slot(0, 0, &ram);
        let mut w = StateWriter::new();
        b.save(&mut w);
        let bytes = w.into_bytes();
        let mut c = Babbage::new();
        c.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, b);
    }
}
