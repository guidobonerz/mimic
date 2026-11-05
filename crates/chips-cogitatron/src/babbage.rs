//! Babbage, the Cogitatron 16's bus arbiter and DMA chip (SPEC.md sections
//! 5 and 6): DMA control at FFFF:0000 and the Copper at FFFF:0020.
//! Built so far (steps 4 and 5): DMACON, DMAERR, BUSCON, the Copper and
//! the Blitter ([`crate::blitter`], registers at FFFF:0100); the
//! Stereoscope follows in a later step.
//!
//! The Copper runs one instruction (two words, one chip-bus slot) per free
//! slot it gets: [`Babbage::copper_slot`] is called by the machine for
//! each slot the fixed DMA leaves, with the beam there, and says what the
//! instruction does outside the Copper (a register write, an interrupt,
//! an error). It restarts at list 1 when a field begins (line 0, x 0).

use emu_core::{StateError, StateReader, StateWriter};

use crate::blitter::{Blitter, ChipRamMut};
use crate::lumiere::ChipRam;

pub const BABID: u16 = 0x0000;
pub const DMACON: u16 = 0x0002;
pub const DMAERR: u16 = 0x0004;
pub const BUSCON: u16 = 0x0006;
pub const COP1SEG: u16 = 0x0020;
pub const COP1OFS: u16 = 0x0022;
pub const COP2SEG: u16 = 0x0024;
pub const COP2OFS: u16 = 0x0026;
pub const COPJMP1: u16 = 0x0028;
pub const COPJMP2: u16 = 0x002a;
pub const COPCON: u16 = 0x002c;
pub const COPIRQ: u16 = 0x002e;

/// DMACON bits.
pub const DMA_FRAMEBUFFER: u16 = 1 << 0;
pub const DMA_SPRITES: u16 = 1 << 1;
pub const DMA_CALLIOPE: u16 = 1 << 2;
pub const DMA_TELEGRAPH: u16 = 1 << 3;
pub const DMA_COPPER: u16 = 1 << 4;
pub const DMA_BLITTER: u16 = 1 << 5;
pub const DMA_STEREOSCOPE: u16 = 1 << 6;
pub const DMA_MASTER: u16 = 1 << 14;

/// The end of chip RAM (segments 0–$3F).
const CHIP_END: u32 = 0x0040_0000;

/// What a Copper slot did that the machine must carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopperAction {
    None,
    /// Write `value` to the I/O register at FFFF:`offset`.
    Move {
        offset: u16,
        value: u16,
    },
    /// Raise the Copper interrupt (COPIRQ holds the value).
    Irq,
    /// Start the Blitter job (no Blitter yet).
    Blit,
    /// The Copper stopped on an error (DMAERR bit 4 is set): raise the
    /// DMA-error interrupt.
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Fetching instructions.
    #[default]
    Run,
    /// WAIT for the beam at or past (y, x), and with `blitter` the Blitter
    /// idle.
    Wait { y: u16, x: u16, blitter: bool },
    /// STOP, an error, or Copper DMA off: idle until the next field.
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Babbage {
    dmacon: u16,
    dmaerr: u16,
    buscon: u16,
    cop1: (u16, u16),
    cop2: (u16, u16),
    copcon: u16,
    copirq: u16,
    /// The Copper: next instruction (linear) and state.
    pc: u32,
    state: State,
    pub blitter: Blitter,
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

    /// The CPU before Blitter/Stereoscope (BUSCON bit 0).
    pub fn cpu_first(&self) -> bool {
        self.buscon & 1 != 0
    }

    pub fn peek(&self, off: u16) -> u16 {
        match off & !1 {
            BABID => 0xba01,
            DMACON => self.dmacon,
            DMAERR => self.dmaerr,
            BUSCON => self.buscon,
            COP1SEG => self.cop1.0,
            COP1OFS => self.cop1.1,
            COP2SEG => self.cop2.0,
            COP2OFS => self.cop2.1,
            COPCON => self.copcon,
            COPIRQ => self.copirq,
            0x0100..=0x017f => self.blitter.peek(off),
            _ => 0,
        }
    }

    /// Register read (the Blitter's done bit clears).
    pub fn read(&mut self, off: u16) -> u16 {
        match off & !1 {
            0x0100..=0x017f => self.blitter.read(off),
            _ => self.peek(off),
        }
    }

    /// A Blitter start failed since the last call.
    pub fn take_error(&mut self) -> bool {
        std::mem::take(&mut self.error_pending)
    }

    fn start_blit(&mut self) -> bool {
        let ok = self.blitter.start();
        if !ok {
            self.dmaerr |= DMA_BLITTER;
        }
        ok
    }

    /// The Blitter has a job and its DMA is on.
    pub fn blitter_wants(&self) -> bool {
        self.blitter.busy() && self.dma(DMA_BLITTER)
    }

    /// One free slot for the Blitter; `true` when its job finished.
    pub fn blitter_slot(&mut self, mem: &mut dyn ChipRamMut) -> bool {
        self.blitter.slot(mem)
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
                if !self.dma(DMA_COPPER) {
                    self.state = State::Stopped;
                }
            }
            DMAERR => self.dmaerr &= !v,
            BUSCON => self.buscon = v & 1,
            COP1SEG => self.cop1.0 = v,
            COP1OFS => self.cop1.1 = v,
            COP2SEG => self.cop2.0 = v,
            COP2OFS => self.cop2.1 = v,
            COPJMP1 => self.jump(self.cop1),
            COPJMP2 => self.jump(self.cop2),
            COPCON => self.copcon = v & 1,
            0x0100..=0x017f if self.blitter.write(off, v) && !self.start_blit() => {
                self.error_pending = true;
            }
            _ => {}
        }
    }

    fn jump(&mut self, to: (u16, u16)) {
        self.pc = u32::from(to.0) << 16 | u32::from(to.1);
        self.state = if self.dma(DMA_COPPER) {
            State::Run
        } else {
            State::Stopped
        };
    }

    /// A field begins: the Copper restarts at list 1.
    pub fn field_start(&mut self) {
        self.jump(self.cop1);
    }

    /// The Copper wants this free slot (it would fetch or check a WAIT).
    pub fn copper_active(&self) -> bool {
        self.dma(DMA_COPPER) && self.state != State::Stopped
    }

    /// The Copper is waiting (it takes no slot, it only compares).
    pub fn copper_waiting(&self) -> bool {
        matches!(self.state, State::Wait { .. })
    }

    /// The Copper's next instruction (linear address).
    pub fn copper_pc(&self) -> u32 {
        self.pc
    }

    fn stop_with_error(&mut self) -> CopperAction {
        self.dmaerr |= DMA_COPPER;
        self.state = State::Stopped;
        CopperAction::Error
    }

    /// Check a waiting Copper against the beam at (`y`, `x`); true when it
    /// runs again. Costs no slot.
    pub fn copper_compare(&mut self, y: u16, x: u16) -> bool {
        if let State::Wait {
            y: wy,
            x: wx,
            blitter,
        } = self.state
        {
            if (y, x) >= (wy, wx) && !(blitter && self.blitter.busy()) {
                self.state = State::Run;
                return true;
            }
        }
        false
    }

    /// One free chip-bus slot for a running Copper, the beam at (`y`, `x`):
    /// fetch and run one instruction.
    pub fn copper_slot(&mut self, y: u16, x: u16, mem: &dyn ChipRam) -> CopperAction {
        if self.state != State::Run || !self.dma(DMA_COPPER) {
            return CopperAction::None;
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
                if offset < 0x0800 && self.copcon & 1 == 0 {
                    return self.stop_with_error();
                }
                CopperAction::Move { offset, value: w1 }
            }
            1 => {
                self.state = State::Wait {
                    y: w0 & 0x3ff,
                    x: w1 & 0x7ff,
                    blitter: w1 & 0x8000 != 0,
                };
                // A WAIT that already holds goes on with the next slot.
                self.copper_compare(y, x);
                CopperAction::None
            }
            2 => {
                if (y, x) >= (w0 & 0x3ff, w1 & 0x7ff) {
                    self.pc += 4;
                }
                CopperAction::None
            }
            3 => {
                self.pc = if w0 & 1 == 0 {
                    u32::from(self.cop1.0) << 16 | u32::from(self.cop1.1)
                } else {
                    u32::from(self.cop2.0) << 16 | u32::from(self.cop2.1)
                };
                CopperAction::None
            }
            4 => {
                if self.start_blit() {
                    CopperAction::Blit
                } else {
                    self.state = State::Stopped;
                    CopperAction::Error
                }
            }
            5 => {
                self.copirq = w1;
                CopperAction::Irq
            }
            _ => {
                self.state = State::Stopped;
                CopperAction::None
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"BABB", 2, |w| {
            for v in [
                self.dmacon,
                self.dmaerr,
                self.buscon,
                self.cop1.0,
                self.cop1.1,
                self.cop2.0,
                self.cop2.1,
                self.copcon,
                self.copirq,
            ] {
                w.u16(v);
            }
            w.u32(self.pc);
            match self.state {
                State::Run => w.u8(0),
                State::Wait { y, x, blitter } => {
                    w.u8(1);
                    w.u16(y);
                    w.u16(x);
                    w.bool(blitter);
                }
                State::Stopped => w.u8(2),
            }
            self.blitter.save(w);
            w.bool(self.error_pending);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"BABB", 2)?;
        let mut b = Self {
            dmacon: c.u16()? & 0x7fff,
            dmaerr: c.u16()?,
            buscon: c.u16()? & 1,
            cop1: (c.u16()?, c.u16()?),
            cop2: (c.u16()?, c.u16()?),
            copcon: c.u16()? & 1,
            copirq: c.u16()?,
            pc: c.u32()?,
            state: State::Run,
            blitter: Blitter::default(),
            error_pending: false,
        };
        b.state = match c.u8()? {
            0 => State::Run,
            1 => State::Wait {
                y: c.u16()? & 0x3ff,
                x: c.u16()? & 0x7ff,
                blitter: c.bool()?,
            },
            2 => State::Stopped,
            _ => return Err(StateError::Invalid("Copper state")),
        };
        b.blitter.load(&mut c)?;
        b.error_pending = c.bool()?;
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

    /// A Copper list at 0001:0000.
    fn list(words: &[u16]) -> Ram {
        let mut r = vec![0; 0x2_0000];
        for (i, w) in words.iter().enumerate() {
            r[0x1_0000 + 2 * i..0x1_0000 + 2 * i + 2].copy_from_slice(&w.to_le_bytes());
        }
        Ram(r)
    }

    fn copper(on: bool) -> Babbage {
        let mut b = Babbage::new();
        b.write(COP1SEG, 1);
        b.write(
            DMACON,
            0x8000 | DMA_MASTER | if on { DMA_COPPER } else { 0 },
        );
        b.field_start();
        b
    }

    #[test]
    fn dmacon_sets_and_clears() {
        let mut b = Babbage::new();
        b.write(DMACON, 0x8000 | DMA_MASTER | DMA_FRAMEBUFFER | DMA_COPPER);
        assert!(b.dma(DMA_FRAMEBUFFER) && b.dma(DMA_COPPER));
        b.write(DMACON, DMA_COPPER);
        assert!(b.dma(DMA_FRAMEBUFFER) && !b.dma(DMA_COPPER));
        b.write(DMACON, DMA_MASTER);
        assert!(!b.dma(DMA_FRAMEBUFFER), "master off");
        assert_eq!(b.peek(BABID), 0xba01);
    }

    #[test]
    fn copper_instructions() {
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
            0, // BLIT
            0xc000,
            0, // STOP
        ]);
        let mut b = copper(true);
        assert_eq!(
            b.copper_slot(0, 0, &ram),
            CopperAction::Move {
                offset: 0x0800,
                value: 0x8004
            }
        );
        assert_eq!(b.copper_slot(0, 1, &ram), CopperAction::None);
        assert!(b.copper_waiting());
        assert!(!b.copper_compare(10, 99));
        assert!(b.copper_compare(10, 100));
        assert_eq!(
            b.copper_slot(10, 101, &ram),
            CopperAction::None,
            "SKIP not yet"
        );
        assert_eq!(b.copper_slot(10, 102, &ram), CopperAction::Irq);
        assert_eq!(b.peek(COPIRQ), 0x55aa);
        assert_eq!(b.copper_slot(10, 103, &ram), CopperAction::Blit);
        assert_eq!(b.copper_slot(10, 104, &ram), CopperAction::None);
        assert!(!b.copper_active(), "STOP");
        b.field_start();
        assert!(b.copper_active(), "again from list 1");
        assert!(!copper(false).copper_active(), "no Copper DMA");
    }

    #[test]
    fn copper_errors_stop_it() {
        let ram = list(&[0x0002 >> 1, 0x8000]);
        let mut b = copper(true);
        assert_eq!(
            b.copper_slot(0, 0, &ram),
            CopperAction::Error,
            "Babbage without COPCON"
        );
        assert_eq!(b.peek(DMAERR), DMA_COPPER);
        b.write(DMAERR, DMA_COPPER);
        b.write(COPCON, 1);
        b.field_start();
        assert_eq!(
            b.copper_slot(0, 0, &ram),
            CopperAction::Move {
                offset: 2,
                value: 0x8000
            }
        );
        b.write(COP1SEG, 0x40);
        b.field_start();
        assert_eq!(
            b.copper_slot(0, 0, &ram),
            CopperAction::Error,
            "outside chip RAM"
        );
    }

    #[test]
    fn savestate_round_trip() {
        let ram = list(&[0x2000 | 300, 5]);
        let mut b = copper(true);
        b.copper_slot(0, 0, &ram);
        let mut w = StateWriter::new();
        b.save(&mut w);
        let bytes = w.into_bytes();
        let mut c = Babbage::new();
        c.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(c, b);
    }
}
