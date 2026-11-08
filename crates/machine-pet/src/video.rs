//! PET video: a CRTC (6545 on the 4032/8032, a fixed timing chain with the
//! same geometry on the 2001/3032) addresses video RAM; each character's
//! code selects 8 bytes of the 2 KB character ROM.
//!
//! - Character ROM: two sets of 128 characters (graphics / text), chosen
//!   by VIA CA2 (high = text). Code bit 7 inverts the character; scan
//!   lines 8 and up of a row (rows 8 and 9 of a 10-line character) are
//!   blanked, dark also under a reverse character or screen.
//! - CRTC MA12 low inverts the whole screen (the editors keep it high).
//! - 40 columns: one character per 1 MHz clock from 1 KB of video RAM.
//!   80 columns (8032): two characters per clock from 2 KB, at `2 × MA`
//!   and `2 × MA + 1`.
//!
//! The frame buffer is what the monitor shows: each scan line starts at
//! its horizontal sync, the field at the vertical sync, so a line whose
//! sync the program moves is drawn shifted, a line longer than 40 clocks
//! runs into the right margin, and rows outside the usual window show
//! (VICE test programs `CRTC/crtc3`-`crtc13`). Where the picture sits is
//! taken from the registers at each vertical sync (**[S]**): the normal
//! window lands 2 clocks right of the left edge and `MARGIN_Y` lines
//! below the top, in text and graphics mode alike, as a monitor adjusted
//! to the machine would show it.

use chips_cbm::crtc::{Crtc, CrtcOut};
use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use emu_core::{BeamPos, Savestate, StateError, StateReader, StateWriter};

/// Margin around the display window in pixels (per 40-column character
/// width: 2 character clocks) and lines.
const MARGIN_X: u32 = 16;
const MARGIN_Y: u32 = 20;
/// Clocks of left margin.
const MARGIN_CLOCKS: i32 = 2;

/// The 2001's timing chain as CRTC values: 64 clocks × 260 lines (60.1 Hz),
/// 25 rows of 8 lines, retrace after the last row.
pub const FIXED_TIMING: [u8; 18] = [63, 40, 50, 15, 31, 4, 25, 25, 0, 7, 0, 0, 16, 0, 0, 0, 0, 0];

/// The editors' graphics-mode set-ups, used as power-on values (the editor
/// reprograms the CRTC at once).
pub const CRTC_40_50HZ: [u8; 18] = [49, 40, 41, 15, 49, 0, 25, 37, 0, 7, 0, 0, 16, 0, 0, 0, 0, 0];
pub const CRTC_40_60HZ: [u8; 18] = [49, 40, 41, 15, 40, 5, 25, 33, 0, 7, 0, 0, 16, 0, 0, 0, 0, 0];
pub const CRTC_80_50HZ: [u8; 18] = [50, 40, 40, 8, 48, 0, 25, 37, 0, 7, 0, 0, 16, 0, 0, 0, 0, 0];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    pub crtc: Crtc,
    /// Characters per CRTC clock (1 or 2).
    cpc: u32,
    /// Lines per character row the window is sized for (8 or 10).
    char_lines: u32,
    standard: VideoStandard,
    pixels: Vec<u8>,
    frame: u64,
    /// The field each frame-buffer row was last drawn in (derived: a
    /// loaded state counts all rows as this field's).
    row_field: Vec<u32>,
    field: u32,
    /// The monitor: clocks since the last horizontal sync began, lines
    /// since the last vertical sync began, the sync levels of the last
    /// clock, and where the picture starts (latched at vertical sync).
    h: u16,
    v: u16,
    hsync: bool,
    vsync: bool,
    origin: (i32, i32),
}

impl Video {
    pub fn new(regs: [u8; 18], cpc: u32, char_lines: u32, standard: VideoStandard) -> Self {
        let mut v = Self {
            crtc: Crtc::new(regs),
            cpc,
            char_lines,
            standard,
            pixels: Vec::new(),
            frame: 0,
            row_field: Vec::new(),
            field: 0,
            h: 0,
            v: 0,
            hsync: false,
            vsync: false,
            origin: (0, 0),
        };
        v.origin = v.origin_now();
        // Power-on: as if the syncs had come where they would.
        v.h = (v.origin.0 + MARGIN_CLOCKS).max(0) as u16;
        v.v = (v.origin.1 + MARGIN_Y as i32).max(0) as u16;
        v.pixels = vec![0; (v.width() * v.height()) as usize];
        v.row_field = vec![0; v.height() as usize];
        v
    }

    pub fn width(&self) -> u32 {
        (40 * 8 + 2 * MARGIN_X) * self.cpc
    }

    pub fn height(&self) -> u32 {
        25 * self.char_lines + 2 * MARGIN_Y
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Where the picture starts, from the registers now: the first display
    /// clock lies `R0 + 1 - R2` clocks after horizontal sync, the first
    /// display line `frame lines - R7 (R9 + 1)` lines after vertical sync.
    fn origin_now(&self) -> (i32, i32) {
        let r = &self.crtc.regs;
        let h = i32::from(r[0]) + 1 - i32::from(r[2]) - MARGIN_CLOCKS;
        let v = self.crtc.frame_lines() as i32
            - i32::from(r[7]) * (i32::from(r[9]) + 1)
            - MARGIN_Y as i32;
        (h, v)
    }

    /// Frame-buffer row showing CRTC scan line `line` (counted from the
    /// first display line, [`Crtc::line`]): the window starts below the
    /// top margin; lines past the frame's last row are not shown.
    pub fn frame_row(&self, line: u16) -> Option<u32> {
        let y = MARGIN_Y + u32::from(line);
        (y < self.height()).then_some(y)
    }

    pub fn signal(&self) -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: 8_000_000 * self.cpc,
            active_pixels: self.width(),
            total_pixels: (u32::from(self.crtc.regs[0]) + 1) * 8 * self.cpc,
            lines_per_field: self.crtc.frame_lines(),
            standard: self.standard,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        }
    }

    pub fn beam(&self) -> BeamPos {
        let col = u32::from(self.crtc.column());
        BeamPos {
            frame: self.frame,
            line: self.crtc.line(),
            cycle: col as u16,
            // Frame column of this clock's first pixel, clamped to the
            // picture.
            hpos: ((MARGIN_X + col * 8) * self.cpc).min(self.width() - 1) as u16,
        }
    }

    /// One character clock. `vram` is the video RAM (1 or 2 KB), `chargen`
    /// the 2 KB character ROM, `text` the VIA CA2 level. Returns the CRTC
    /// output (sync) and draws the character.
    pub fn tick(&mut self, vram: &[u8], chargen: &[u8], text: bool) -> CrtcOut {
        let o = self.crtc.tick();
        // The monitor follows the syncs' leading edges.
        if o.vsync && !self.vsync {
            self.v = 0;
            self.origin = self.origin_now();
            // Rows the field now over did not draw go dark.
            let w = self.width() as usize;
            for (y, f) in self.row_field.iter().enumerate() {
                if *f != self.field {
                    self.pixels[y * w..(y + 1) * w].fill(0);
                }
            }
            self.field = self.field.wrapping_add(1);
        }
        if o.hsync && !self.hsync {
            self.h = 0;
            self.v = self.v.saturating_add(1);
        }
        self.vsync = o.vsync;
        self.hsync = o.hsync;
        let clock = i32::from(self.h) - self.origin.0;
        let y = i32::from(self.v) - self.origin.1;
        self.h = self.h.saturating_add(1);
        if o.display && clock >= 0 && y >= 0 {
            let (y, x0) = (y as u32, clock as u32 * 8 * self.cpc);
            if y < self.height() && x0 + 8 * self.cpc <= self.width() {
                // The row's first character this field: the rest of it
                // is dark until drawn.
                if self.row_field[y as usize] != self.field {
                    self.row_field[y as usize] = self.field;
                    let w = self.width() as usize;
                    self.pixels[y as usize * w..(y as usize + 1) * w].fill(0);
                }
                let invert = o.ma & 0x1000 == 0;
                let set = if text { 0x400 } else { 0 };
                for i in 0..self.cpc {
                    let addr = (u32::from(o.ma) * self.cpc + i) as usize % vram.len();
                    let code = vram[addr];
                    // Scan lines 8 and up of a row are blanked: dark,
                    // reverse or not (RA3/RA4; VICE `CRTC/crtc10` and its
                    // readme).
                    let pattern = if o.ra < 8 {
                        let p = chargen[set | (usize::from(code & 0x7f) << 3) | usize::from(o.ra)];
                        if (code & 0x80 != 0) != invert {
                            !p
                        } else {
                            p
                        }
                    } else {
                        0
                    };
                    let at = (y * self.width() + x0 + i * 8) as usize;
                    for (b, p) in self.pixels[at..at + 8].iter_mut().enumerate() {
                        *p = (pattern >> (7 - b)) & 1;
                    }
                }
            }
        }
        if o.frame_end {
            self.frame += 1;
        }
        o
    }
}

const TAG: [u8; 4] = *b"PETV";
const VERSION: u16 = 2;

impl Savestate for Video {
    fn save(&self, w: &mut StateWriter) {
        self.crtc.save(w);
        w.chunk(TAG, VERSION, |w| {
            w.u64(self.frame);
            w.u32(0); // version 1: the lowest line drawn
            w.raw(&self.pixels);
            // Version 2: the monitor.
            w.u16(self.h);
            w.u16(self.v);
            w.bool(self.hsync);
            w.bool(self.vsync);
            w.u32(self.origin.0 as u32);
            w.u32(self.origin.1 as u32);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut crtc = self.crtc.clone();
        crtc.load(r)?;
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let frame = c.u64()?;
        c.u32()?;
        let mut pixels = vec![0; self.pixels.len()];
        c.raw_into(&mut pixels)?;
        for p in &mut pixels {
            *p &= 1;
        }
        if version >= 2 {
            self.h = c.u16()?;
            self.v = c.u16()?;
            self.hsync = c.bool()?;
            self.vsync = c.bool()?;
            self.origin = (c.u32()? as i32, c.u32()? as i32);
        }
        self.crtc = crtc;
        self.frame = frame;
        self.row_field.fill(self.field);
        self.pixels = pixels;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hpos_is_the_frame_column_of_the_clock() {
        for (cpc, regs) in [(1, CRTC_40_50HZ), (2, CRTC_80_50HZ)] {
            let mut v = Video::new(regs, cpc, 10, VideoStandard::Pal);
            let mut vram = vec![0u8; 1024 * cpc as usize];
            vram[5 * cpc as usize] = 1; // MA 5 (first character of the clock)
            let mut chargen = vec![0u8; 2048];
            chargen[8] = 0x80;
            while v.crtc.column() != 5 {
                v.tick(&vram, &chargen, false);
            }
            assert_eq!(v.crtc.line(), 0);
            let x = u32::from(v.beam().hpos);
            v.tick(&vram, &chargen, false);
            let at = (v.frame_row(0).unwrap() * v.width()) as usize;
            assert_eq!(
                (v.pixels()[at + x as usize - 1], v.pixels()[at + x as usize]),
                (0, 1)
            );
            assert_eq!(x, (MARGIN_X + 5 * 8) * cpc);
        }
    }
}
