//! TIA (Television Interface Adaptor), the Atari 2600's video and sound
//! chip. It has no frame buffer: the CPU races the beam and sets the
//! registers line by line.
//!
//! Timing: 228 colour clocks per line, the first 68 are horizontal
//! blanking, 160 are picture. The CPU runs at a third of the colour clock
//! (76 cycles per line). A write to WSYNC holds the CPU (RDY low) until
//! the next line starts. Lines and fields come from the program: VSYNC
//! (bit 1) is the vertical sync pulse, VBLANK (bit 1) blanks the picture.
//!
//! This model starts a field when VSYNC ends: line 0 is the first line
//! after the sync pulse, as a TV locks to it. Without a sync pulse the
//! field ends after [`free_run_lines`], like a TV that loses sync.
//!
//! Output: [`WIDTH`] × [`height`] pixels of colour register values ÷ 2
//! (the TIA ignores bit 0), index into [`crate::palette::tia`].
//!
//! Registers (write `$00–$2C`, read `$00–$0D`; the machine passes A0–A5):
//! see [`W`] and [`R`]. Sources: Steve Wright, "Stella Programmer's Guide"
//! (1979); Andrew Towers, "Atari 2600 TIA Hardware Notes".

use emu_core::frame::VideoStandard;
use emu_core::savestate::{Savestate, StateError, StateReader, StateWriter};

/// Colour clocks per line and of horizontal blanking.
pub const CLOCKS_PER_LINE: u8 = 228;
pub const HBLANK: u8 = 68;
/// Picture width in colour clocks (one pixel each).
pub const WIDTH: u32 = 160;

/// Write register names, by address.
pub const W: [&str; 0x2d] = [
    "VSYNC", "VBLANK", "WSYNC", "RSYNC", "NUSIZ0", "NUSIZ1", "COLUP0", "COLUP1", "COLUPF",
    "COLUBK", "CTRLPF", "REFP0", "REFP1", "PF0", "PF1", "PF2", "RESP0", "RESP1", "RESM0", "RESM1",
    "RESBL", "AUDC0", "AUDC1", "AUDF0", "AUDF1", "AUDV0", "AUDV1", "GRP0", "GRP1", "ENAM0",
    "ENAM1", "ENABL", "HMP0", "HMP1", "HMM0", "HMM1", "HMBL", "VDELP0", "VDELP1", "VDELBL",
    "RESMP0", "RESMP1", "HMOVE", "HMCLR", "CXCLR",
];

/// Read register names, by address.
pub const R: [&str; 14] = [
    "CXM0P", "CXM1P", "CXP0FB", "CXP1FB", "CXM0FB", "CXM1FB", "CXBLPF", "CXPPMM", "INPT0", "INPT1",
    "INPT2", "INPT3", "INPT4", "INPT5",
];

pub const VSYNC: u8 = 0x00;
pub const VBLANK: u8 = 0x01;
pub const WSYNC: u8 = 0x02;
pub const RSYNC: u8 = 0x03;
pub const COLUBK: u8 = 0x09;

/// Lines of a field as the TV standard defines them (the height of the
/// output picture).
pub fn height(standard: VideoStandard) -> u32 {
    match standard {
        VideoStandard::Ntsc => 262,
        VideoStandard::Pal => 312,
    }
}

/// Lines after which a field without VSYNC ends.
pub fn free_run_lines(standard: VideoStandard) -> u16 {
    height(standard) as u16 + 40
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tia {
    standard: VideoStandard,
    /// Last value written to each register.
    pub regs: [u8; 0x2d],
    /// Colour clock within the line (0–227) and line within the field.
    cc: u8,
    line: u16,
    frame: u64,
    /// The CPU waits for the next line.
    wsync: bool,
    /// A field ended since the last [`Tia::tick`] reported it.
    field_ended: bool,
    /// Levels of the input pins I0–I5 (bit n = INPTn high).
    inputs: u8,
    pixels: Vec<u8>,
}

impl Tia {
    pub fn new(standard: VideoStandard) -> Self {
        Self {
            standard,
            regs: [0; 0x2d],
            cc: 0,
            line: 0,
            frame: 0,
            wsync: false,
            field_ended: false,
            inputs: 0x3f,
            pixels: vec![0; (WIDTH * height(standard)) as usize],
        }
    }

    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    pub fn clock(&self) -> u8 {
        self.cc
    }

    pub fn line(&self) -> u16 {
        self.line
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// RDY for the CPU: low while it waits for WSYNC.
    pub fn rdy(&self) -> bool {
        !self.wsync
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        let reg = reg & 0x3f;
        let Some(slot) = self.regs.get_mut(usize::from(reg)) else {
            return;
        };
        let before = *slot;
        *slot = v;
        match reg {
            VSYNC if before & 2 != 0 && v & 2 == 0 => self.end_field(),
            WSYNC => self.wsync = true,
            // Resets the horizontal counter: the line starts over.
            RSYNC => self.cc = CLOCKS_PER_LINE - 3,
            _ => {}
        }
    }

    /// Read registers drive D7 and D6 only; the machine fills in the rest
    /// from the data bus. No side effects.
    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 0x0f {
            r @ 0x08..=0x0d => (self.inputs >> (r - 8) & 1) << 7,
            _ => 0,
        }
    }

    /// Level of input pin `n` (0–5): paddles on I0–I3, fire buttons on
    /// I4 (left port) and I5 (right port), low = pressed.
    pub fn set_input(&mut self, n: u8, high: bool) {
        let bit = 1 << n;
        if high {
            self.inputs |= bit;
        } else {
            self.inputs &= !bit;
        }
    }

    /// One colour clock. Returns true when a field ended (on VSYNC or
    /// free-running).
    pub fn tick(&mut self) -> bool {
        if self.cc >= HBLANK {
            let x = u32::from(self.cc - HBLANK);
            let y = u32::from(self.line);
            if y < height(self.standard) {
                let colour = if self.regs[usize::from(VBLANK)] & 2 != 0 {
                    0
                } else {
                    self.regs[usize::from(COLUBK)] >> 1
                };
                self.pixels[(y * WIDTH + x) as usize] = colour;
            }
        }
        self.cc += 1;
        if self.cc == CLOCKS_PER_LINE {
            self.cc = 0;
            self.wsync = false;
            self.line += 1;
            if self.line >= free_run_lines(self.standard) {
                self.end_field();
            }
        }
        std::mem::take(&mut self.field_ended)
    }

    fn end_field(&mut self) {
        // Lines the field did not reach show black.
        let from = (u32::from(self.line) + 1).min(height(self.standard)) * WIDTH;
        self.pixels[from as usize..].fill(0);
        self.line = 0;
        self.frame += 1;
        self.field_ended = true;
    }

    /// Power-up (the input pins stay as the controllers drive them).
    pub fn reset(&mut self) {
        let inputs = self.inputs;
        *self = Self::new(self.standard);
        self.inputs = inputs;
    }
}

const TAG: [u8; 4] = *b"TIA ";
const VERSION: u16 = 1;

impl Savestate for Tia {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.cc);
            w.u16(self.line);
            w.u64(self.frame);
            w.bool(self.wsync);
            w.bool(self.field_ended);
            w.u8(self.inputs);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = self.clone();
        c.raw_into(&mut n.regs)?;
        n.cc = c.u8()?;
        n.line = c.u16()?;
        n.frame = c.u64()?;
        n.wsync = c.bool()?;
        n.field_ended = c.bool()?;
        n.inputs = c.u8()?;
        c.raw_into(&mut n.pixels)?;
        if n.cc >= CLOCKS_PER_LINE || n.line >= free_run_lines(n.standard) {
            return Err(StateError::Invalid("TIA beam"));
        }
        *self = n;
        Ok(())
    }
}
