//! PET video: a CRTC (6545 on the 4032/8032, a fixed timing chain with the
//! same geometry on the 2001/3032) addresses video RAM; each character's
//! code selects 8 bytes of the 2 KB character ROM.
//!
//! - Character ROM: two sets of 128 characters (graphics / text), chosen
//!   by VIA CA2 (high = text). Code bit 7 inverts the character; rows 8
//!   and 9 of a 10-line character are blank.
//! - CRTC MA12 low inverts the whole screen (the editors keep it high).
//! - 40 columns: one character per 1 MHz clock from 1 KB of video RAM.
//!   80 columns (8032): two characters per clock from 2 KB, at `2 × MA`
//!   and `2 × MA + 1`.
//!
//! The frame buffer shows the display window with a fixed black margin
//! (the PET has no border colour); the picture is aligned to the window,
//! not to sync, so CRTC sync position changes do not move it.

use chips_cbm::crtc::{Crtc, CrtcOut};
use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use emu_core::{BeamPos, Savestate, StateError, StateReader, StateWriter};

/// Margin around the display window in pixels (per 40-column character
/// width) and lines.
const MARGIN_X: u32 = 32;
const MARGIN_Y: u32 = 20;

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
    /// Lowest frame-buffer line drawn this frame.
    drawn_to: u32,
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
            drawn_to: 0,
        };
        v.pixels = vec![0; (v.width() * v.height()) as usize];
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
        let line = u32::from(self.crtc.line());
        let col = u32::from(self.crtc.column());
        let o = self.crtc.tick();
        if o.display {
            let y = MARGIN_Y + line;
            let x0 = (MARGIN_X + col * 8) * self.cpc;
            if y < self.height() && x0 + 8 * self.cpc <= self.width() {
                let invert = o.ma & 0x1000 == 0;
                let set = if text { 0x400 } else { 0 };
                for i in 0..self.cpc {
                    let addr = (u32::from(o.ma) * self.cpc + i) as usize % vram.len();
                    let code = vram[addr];
                    let mut pattern = if o.ra < 8 {
                        chargen[set | (usize::from(code & 0x7f) << 3) | usize::from(o.ra)]
                    } else {
                        0
                    };
                    if (code & 0x80 != 0) != invert {
                        pattern = !pattern;
                    }
                    let at = (y * self.width() + x0 + i * 8) as usize;
                    for (b, p) in self.pixels[at..at + 8].iter_mut().enumerate() {
                        *p = (pattern >> (7 - b)) & 1;
                    }
                }
                self.drawn_to = self.drawn_to.max(y + 1);
            }
        }
        if o.frame_end {
            // Lines below the window (after a switch to fewer lines) go dark.
            let from = (self.drawn_to.max(MARGIN_Y) * self.width()) as usize;
            let to = ((self.height() - MARGIN_Y) * self.width()) as usize;
            if from < to {
                self.pixels[from..to].fill(0);
            }
            self.drawn_to = 0;
            self.frame += 1;
        }
        o
    }
}

const TAG: [u8; 4] = *b"PETV";
const VERSION: u16 = 1;

impl Savestate for Video {
    fn save(&self, w: &mut StateWriter) {
        self.crtc.save(w);
        w.chunk(TAG, VERSION, |w| {
            w.u64(self.frame);
            w.u32(self.drawn_to);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut crtc = self.crtc.clone();
        crtc.load(r)?;
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let frame = c.u64()?;
        let drawn_to = c.u32()?.min(self.height());
        let mut pixels = vec![0; self.pixels.len()];
        c.raw_into(&mut pixels)?;
        for p in &mut pixels {
            *p &= 1;
        }
        self.crtc = crtc;
        self.frame = frame;
        self.drawn_to = drawn_to;
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
