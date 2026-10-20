//! MOS 6545-1 CRT controller (PET 4032/8032), compatible with the Motorola
//! MC6845 timing registers.
//!
//! | Reg | Function                         | Reg | Function                       |
//! | --- | -------------------------------- | --- | ------------------------------ |
//! | R0  | horizontal total − 1             | R7  | vertical sync position (rows)  |
//! | R1  | horizontal displayed             | R8  | interlace mode                 |
//! | R2  | horizontal sync position         | R9  | scan lines per row − 1         |
//! | R3  | sync widths                      | R10/R11 | cursor start / end         |
//! | R4  | vertical total − 1 (rows)        | R12/R13 | start address high / low   |
//! | R5  | vertical total adjust (lines)    | R14/R15 | cursor address (read/write)|
//! | R6  | vertical displayed (rows)        | R16/R17 | light pen (read only)      |
//!
//! The address register is at the even address, the selected register at
//! the odd one. On the 6545-1 vertical sync lasts 16 scan lines. Source:
//! MOS 6545 data sheet; Motorola MC6845 data sheet.
//!
//! [`Crtc::tick`] is one character clock: it reports whether the display is
//! enabled, the memory address (MA) and row address (RA) of that character
//! and the sync level, then advances the counters.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Writable bits per register (the address registers are 14 bits wide).
const MASKS: [u8; 18] = [
    0xff, 0xff, 0xff, 0xff, 0x7f, 0x1f, 0x7f, 0x7f, 0x03, 0x1f, 0x7f, 0x1f, 0x3f, 0xff, 0x3f, 0xff,
    0x3f, 0xff,
];

/// VSYNC length of the 6545-1 in scan lines.
const VSYNC_LINES: u8 = 16;

/// What one character clock shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtcOut {
    pub display: bool,
    /// Memory address of the character (14 bits).
    pub ma: u16,
    /// Scan line within the character row.
    pub ra: u8,
    /// Vertical sync active.
    pub vsync: bool,
    /// A new frame starts with the next clock.
    pub frame_end: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crtc {
    pub regs: [u8; 18],
    pub addr: u8,
    hc: u8,
    sc: u8,
    row: u8,
    adjusting: bool,
    adjust: u8,
    row_start: u16,
    vsync: u8,
    /// Scan line within the frame.
    line: u16,
}

impl Crtc {
    /// A CRTC with the given power-on register values.
    pub fn new(regs: [u8; 18]) -> Self {
        let mut c = Self {
            regs: [0; 18],
            addr: 0,
            hc: 0,
            sc: 0,
            row: 0,
            adjusting: false,
            adjust: 0,
            row_start: 0,
            vsync: 0,
            line: 0,
        };
        for (i, v) in regs.into_iter().enumerate() {
            c.regs[i] = v & MASKS[i];
        }
        c.row_start = c.start_address();
        c
    }

    fn start_address(&self) -> u16 {
        (u16::from(self.regs[12]) << 8) | u16::from(self.regs[13])
    }

    /// Character clock within the line.
    pub fn column(&self) -> u8 {
        self.hc
    }

    /// Scan line within the frame.
    pub fn line(&self) -> u16 {
        self.line
    }

    /// Total scan lines of a frame with the current registers.
    pub fn frame_lines(&self) -> u32 {
        (u32::from(self.regs[4]) + 1) * (u32::from(self.regs[9]) + 1) + u32::from(self.regs[5])
    }

    /// Side-effect-free read: the light pen and cursor registers read back,
    /// everything else reads 0.
    pub fn peek(&self, reg: u8) -> u8 {
        if reg & 1 == 0 {
            return 0;
        }
        match self.addr {
            14..=17 => self.regs[usize::from(self.addr)],
            _ => 0,
        }
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        if reg & 1 == 0 {
            self.addr = v & 0x1f;
        } else if let Some(mask) = MASKS.get(usize::from(self.addr)) {
            // R16/R17 (light pen) are read only.
            if self.addr < 16 {
                self.regs[usize::from(self.addr)] = v & mask;
            }
        }
    }

    pub fn tick(&mut self) -> CrtcOut {
        let display = !self.adjusting && self.hc < self.regs[1] && self.row < self.regs[6];
        let mut out = CrtcOut {
            display,
            ma: (self.row_start + u16::from(self.hc)) & 0x3fff,
            ra: self.sc,
            vsync: self.vsync > 0,
            frame_end: false,
        };
        if self.hc >= self.regs[0] {
            self.hc = 0;
            out.frame_end = self.end_line();
        } else {
            self.hc += 1;
        }
        out
    }

    /// Returns true when the frame ends.
    fn end_line(&mut self) -> bool {
        self.vsync = self.vsync.saturating_sub(1);
        self.line += 1;
        if self.adjusting {
            self.adjust += 1;
            if self.adjust >= self.regs[5] {
                self.new_frame();
                return true;
            }
            return false;
        }
        if self.sc >= self.regs[9] {
            self.sc = 0;
            self.row_start = (self.row_start + u16::from(self.regs[1])) & 0x3fff;
            self.row = self.row.wrapping_add(1);
            if self.row > self.regs[4] {
                if self.regs[5] > 0 {
                    self.adjusting = true;
                    self.adjust = 0;
                } else {
                    self.new_frame();
                    return true;
                }
            } else if self.row == self.regs[7] {
                self.vsync = VSYNC_LINES;
            }
        } else {
            self.sc += 1;
        }
        false
    }

    fn new_frame(&mut self) {
        self.row = 0;
        self.sc = 0;
        self.adjusting = false;
        self.line = 0;
        self.row_start = self.start_address() & 0x3fff;
        if self.regs[7] == 0 {
            self.vsync = VSYNC_LINES;
        }
    }
}

const TAG: [u8; 4] = *b"6545";
const VERSION: u16 = 1;

impl Savestate for Crtc {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.addr);
            w.u8(self.hc);
            w.u8(self.sc);
            w.u8(self.row);
            w.bool(self.adjusting);
            w.u8(self.adjust);
            w.u16(self.row_start);
            w.u8(self.vsync);
            w.u16(self.line);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut regs = [0u8; 18];
        c.raw_into(&mut regs)?;
        let mut n = Crtc::new(regs);
        n.addr = c.u8()? & 0x1f;
        n.hc = c.u8()?;
        n.sc = c.u8()?;
        n.row = c.u8()?;
        n.adjusting = c.bool()?;
        n.adjust = c.u8()?;
        n.row_start = c.u16()? & 0x3fff;
        n.vsync = c.u8()?.min(VSYNC_LINES);
        n.line = c.u16()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PET 4032 50 Hz editor's text-mode set-up.
    const PET_TEXT_50HZ: [u8; 18] = [49, 40, 41, 15, 39, 0, 25, 32, 0, 9, 0, 0, 16, 0, 0, 0, 0, 0];

    fn frame(c: &mut Crtc) -> Vec<CrtcOut> {
        let mut v = Vec::new();
        loop {
            let o = c.tick();
            v.push(o);
            if o.frame_end {
                return v;
            }
        }
    }

    #[test]
    fn frame_geometry_of_the_pet_text_mode() {
        let mut c = Crtc::new(PET_TEXT_50HZ);
        frame(&mut c);
        let f = frame(&mut c);
        assert_eq!(f.len(), 50 * 400, "400 lines of 50 clocks: 50 Hz at 1 MHz");
        assert_eq!(c.frame_lines(), 400);
        let shown = f.iter().filter(|o| o.display).count();
        assert_eq!(shown, 40 * 25 * 10);
        // Row 1 starts 40 characters further.
        let first_of_row1 = f[50 * 10];
        assert_eq!((first_of_row1.ma, first_of_row1.ra), (0x1000 + 40, 0));
        assert_eq!(f[50 * 3 + 5].ra, 3);
        // VSYNC from row 32 for 16 lines.
        let vsync: Vec<usize> = (0..400).filter(|l| f[l * 50].vsync).collect();
        assert_eq!(vsync.first(), Some(&320));
        assert_eq!(vsync.len(), 16);
    }

    #[test]
    fn vertical_adjust_adds_lines() {
        let mut regs = PET_TEXT_50HZ;
        regs[4] = 32;
        regs[5] = 3;
        regs[9] = 7;
        let mut c = Crtc::new(regs);
        frame(&mut c);
        assert_eq!(frame(&mut c).len(), 50 * (33 * 8 + 3));
    }

    #[test]
    fn register_access() {
        let mut c = Crtc::new([0; 18]);
        c.write(0, 1);
        c.write(1, 40);
        assert_eq!(c.regs[1], 40);
        c.write(0, 4);
        c.write(1, 0xff);
        assert_eq!(c.regs[4], 0x7f, "masked");
        c.write(0, 1);
        assert_eq!(c.peek(1), 0, "write-only");
        c.write(0, 14);
        c.write(1, 0x12);
        assert_eq!(c.peek(1), 0x12, "cursor reads back");
        c.write(0, 16);
        c.write(1, 0x55);
        assert_eq!(c.regs[16], 0, "light pen is read only");
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = Crtc::new(PET_TEXT_50HZ);
        for _ in 0..12_345 {
            c.tick();
        }
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = Crtc::new([0; 18]);
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(c, d);
        assert_eq!(frame(&mut c), frame(&mut d));
    }
}
