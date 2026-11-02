//! VIDC (VL86C310): the Archimedes' video and sound output.
//!
//! A write puts the register number in bits 31–26 and the data in the rest.
//! $00–$3C palette, $40 border, $44–$4C cursor colours, $60–$7C stereo image,
//! $80–$9C horizontal timing, $A0–$BC vertical timing, $C0 sound frequency,
//! $E0 control: bits 1–0 pixel rate (8, 12, 16, 24 MHz), 3–2 bits per pixel
//! (1, 2, 4, 8).
//!
//! The picture holds the area inside the border registers; a line is drawn
//! when the beam has passed it, so palette changes between lines show.
//!
//! Source: "Acorn RISC Machine Family Data Manual", VL86C310 chapter: table
//! 1, the display parameters and their offsets per pixel depth, the 8-bit
//! palette (figure 6), the sound frequency register.

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

pub const MAX_WIDTH: usize = 1280;
pub const MAX_HEIGHT: usize = 1024;

#[derive(Clone, Debug)]
pub struct Vidc {
    pub palette: [u16; 16],
    pub border: u16,
    pub cursor: [u16; 3],
    pub stereo: [u8; 8],
    pub hcr: u32,
    pub hswr: u32,
    pub hbsr: u32,
    pub hdsr: u32,
    pub hder: u32,
    pub hber: u32,
    pub hcsr: u32,
    pub vcr: u32,
    pub vswr: u32,
    pub vbsr: u32,
    pub vdsr: u32,
    pub vder: u32,
    pub vber: u32,
    pub vcsr: u32,
    pub vcer: u32,
    pub sfr: u32,
    pub control: u32,
    /// The finished picture (not state: redrawn by the next frame).
    pixels: Vec<Rgb8>,
    width: u32,
    height: u32,
}

fn colour(v: u16) -> Rgb8 {
    let expand = |n: u16| ((n & 15) * 17) as u8;
    Rgb8 {
        r: expand(v),
        g: expand(v >> 4),
        b: expand(v >> 8),
    }
}

impl Default for Vidc {
    fn default() -> Self {
        Self::new()
    }
}

impl Vidc {
    pub fn new() -> Self {
        Self {
            palette: [0; 16],
            border: 0,
            cursor: [0; 3],
            stereo: [4; 8],
            hcr: 0,
            hswr: 0,
            hbsr: 0,
            hdsr: 0,
            hder: 0,
            hber: 0,
            hcsr: 0,
            vcr: 0,
            vswr: 0,
            vbsr: 0,
            vdsr: 0,
            vder: 0,
            vber: 0,
            vcsr: 0,
            vcer: 0,
            sfr: 0,
            control: 0,
            // Black until the OS sets a mode: the size its modes have with
            // their borders, so a window opens at the right shape.
            pixels: vec![Rgb8::default(); 832 * 288],
            width: 832,
            height: 288,
        }
    }

    pub fn write(&mut self, v: u32) {
        let reg = v >> 24 & 0xfc;
        let colour_bits = (v & 0x1fff) as u16;
        let high = v >> 14 & 0x3ff;
        match reg {
            0x00..=0x3c => self.palette[(reg / 4) as usize] = colour_bits,
            0x40 => self.border = colour_bits,
            0x44..=0x4c => self.cursor[((reg - 0x44) / 4) as usize] = colour_bits,
            0x60..=0x7c => {
                // $60 is channel 7, $64 channel 0.
                self.stereo[(((reg - 0x60) / 4 + 7) % 8) as usize] = (v & 7) as u8;
            }
            0x80 => self.hcr = high,
            0x84 => self.hswr = high,
            0x88 => self.hbsr = high,
            0x8c => self.hdsr = high,
            0x90 => self.hder = high,
            0x94 => self.hber = high,
            0x98 => self.hcsr = v >> 13 & 0x7ff,
            0xa0 => self.vcr = high,
            0xa4 => self.vswr = high,
            0xa8 => self.vbsr = high,
            0xac => self.vdsr = high,
            0xb0 => self.vder = high,
            0xb4 => self.vber = high,
            0xb8 => self.vcsr = high,
            0xbc => self.vcer = high,
            0xc0 => self.sfr = v & 0xff,
            0xe0 => self.control = v & 0xffff,
            _ => {}
        }
    }

    pub fn programmed(&self) -> bool {
        self.hcr != 0 && self.vcr != 0
    }

    /// Pixel clock in MHz.
    pub fn pixel_mhz(&self) -> u32 {
        [8, 12, 16, 24][(self.control & 3) as usize]
    }

    pub fn bits_per_pixel(&self) -> u32 {
        1 << (self.control >> 2 & 3)
    }

    /// Clocks of the 8 MHz bus a line takes.
    pub fn line_clocks(&self) -> u64 {
        if !self.programmed() {
            return 512;
        }
        (2 * u64::from(self.hcr) + 2) * 8 / u64::from(self.pixel_mhz())
    }

    pub fn total_lines(&self) -> u32 {
        if self.programmed() {
            self.vcr + 1
        } else {
            312
        }
    }

    /// Sound sample period in 8 MHz clocks, if sound can run.
    pub fn sample_clocks(&self) -> u32 {
        (self.sfr + 1) * 8
    }

    fn border_start(&self) -> u32 {
        2 * self.hbsr + 1
    }

    fn border_end(&self) -> u32 {
        2 * self.hber + 1
    }

    pub fn display_lines(&self) -> (u32, u32) {
        (self.vdsr + 1, self.vder + 1)
    }

    /// Bytes of video memory a line takes.
    pub fn line_bytes(&self) -> u32 {
        let width = 2 * self.hder.saturating_sub(self.hdsr);
        width * self.bits_per_pixel() / 8
    }

    /// Start a frame: the picture takes the size the border registers give.
    pub fn frame_start(&mut self) {
        if !self.programmed() {
            return;
        }
        let w = self
            .border_end()
            .saturating_sub(self.border_start())
            .clamp(16, MAX_WIDTH as u32);
        let h = self
            .vber
            .saturating_sub(self.vbsr)
            .clamp(16, MAX_HEIGHT as u32);
        if (w, h) != (self.width, self.height) {
            self.width = w;
            self.height = h;
            self.pixels = vec![Rgb8::default(); (w * h) as usize];
        }
    }

    /// Draw raster line `line` (0 at vertical sync). `video` yields the next
    /// byte of screen memory, `cursor` the next of the cursor's (if the
    /// cursor shows on this line, 8 bytes are taken).
    pub fn draw_line(
        &mut self,
        line: u32,
        mut video: impl FnMut() -> u8,
        mut cursor: impl FnMut() -> u8,
    ) {
        if !self.programmed() || line <= self.vbsr || line > self.vber {
            return;
        }
        let row = (line - self.vbsr - 1) as usize;
        if row >= self.height as usize {
            return;
        }
        let w = self.width as usize;
        let start = row * w;
        let border = colour(self.border);
        let (first, last) = self.display_lines();
        let shows = (first..last).contains(&line);
        let target = &mut self.pixels[start..start + w];
        target.fill(border);
        let offset = match self.control >> 2 & 3 {
            3 => 5,
            2 => 7,
            1 => 11,
            _ => 19,
        };
        if shows {
            let hds = 2 * self.hdsr + offset;
            let hde = 2 * self.hder + offset;
            let hbs = 2 * self.hbsr + 1;
            let bpp = 1u32 << (self.control >> 2 & 3);
            let mut bit = 8u32;
            let mut byte = 0u8;
            for x in hds..hde {
                if bit >= 8 {
                    byte = video();
                    bit = 0;
                }
                let v = (u32::from(byte) >> bit) & ((1 << bpp) - 1);
                bit += bpp;
                if x < hbs {
                    continue;
                }
                let Some(p) = target.get_mut((x - hbs) as usize) else {
                    continue;
                };
                *p = if bpp == 8 {
                    let e = self.palette[(v & 15) as usize];
                    let l = v as u16;
                    colour(
                        (e & 0x7)
                            | (l & 0x10) >> 1
                            | (e & 0x30)
                            | (l & 0x60) << 1
                            | (e & 0x700)
                            | (l & 0x80) << 4,
                    )
                } else {
                    colour(self.palette[v as usize])
                };
            }
        }
        if (self.vcsr..self.vcer).contains(&line) {
            let data: [u8; 8] = std::array::from_fn(|_| cursor());
            let cx = self.hcsr;
            let hbs = 2 * self.hbsr + 1;
            for i in 0..32u32 {
                let v = data[(i / 4) as usize] >> (2 * (i & 3)) & 3;
                if v == 0 {
                    continue;
                }
                let x = cx + i + 6;
                if x >= hbs {
                    if let Some(p) = target.get_mut((x - hbs) as usize) {
                        *p = colour(self.cursor[usize::from(v) - 1]);
                    }
                }
            }
        }
    }

    pub fn picture(&self) -> (&[Rgb8], u32, u32) {
        (&self.pixels, self.width, self.height)
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"VIDC", 1, |w| {
            for v in self.palette {
                w.u16(v);
            }
            w.u16(self.border);
            for v in self.cursor {
                w.u16(v);
            }
            w.bytes(&self.stereo);
            for v in self.timing() {
                w.u32(v);
            }
        });
    }

    fn timing(&self) -> [u32; 17] {
        [
            self.hcr,
            self.hswr,
            self.hbsr,
            self.hdsr,
            self.hder,
            self.hber,
            self.hcsr,
            self.vcr,
            self.vswr,
            self.vbsr,
            self.vdsr,
            self.vder,
            self.vber,
            self.vcsr,
            self.vcer,
            self.sfr,
            self.control,
        ]
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"VIDC", 1)?;
        for v in &mut self.palette {
            *v = c.u16()?;
        }
        self.border = c.u16()?;
        for v in &mut self.cursor {
            *v = c.u16()?;
        }
        let stereo = c.bytes()?;
        if stereo.len() != 8 {
            return Err(StateError::Invalid("VIDC stereo"));
        }
        self.stereo.copy_from_slice(stereo);
        let mut t = [0u32; 17];
        for v in &mut t {
            *v = c.u32()?;
        }
        [
            self.hcr,
            self.hswr,
            self.hbsr,
            self.hdsr,
            self.hder,
            self.hber,
            self.hcsr,
            self.vcr,
            self.vswr,
            self.vbsr,
            self.vdsr,
            self.vder,
            self.vber,
            self.vcsr,
            self.vcer,
            self.sfr,
            self.control,
        ] = t;
        self.frame_start();
        Ok(())
    }
}

/// One sound byte through the exponential DAC: bit 0 the sign, bits 7–1
/// three bits of chord and four of step; 16-bit linear.
pub fn sound_level(b: u8) -> i32 {
    let m = i32::from(b >> 1);
    let (chord, step) = (m >> 4, m & 15);
    let level = ((2 * step + 33) << chord) - 33;
    let scaled = level * 4;
    if b & 1 != 0 {
        -scaled
    } else {
        scaled
    }
}

/// Left and right share (of 8) for a stereo image value (table 2).
pub fn stereo_gain(position: u8) -> (i32, i32) {
    match position {
        1 => (8, 0),
        2 => (7, 1),
        3 => (5, 3),
        5 => (3, 5),
        6 => (1, 7),
        7 => (0, 8),
        _ => (4, 4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode12() -> Vidc {
        // 640 × 256, 4 bits per pixel, 24 MHz off 16 MHz pixel rate.
        let mut v = Vidc::new();
        let reg = |r: u32, val: u32| r << 24 | val << 14;
        v.write(0xe0 << 24 | 0b1010);
        v.write(reg(0x80, 511));
        v.write(reg(0x88, 57));
        v.write(reg(0x8c, 57));
        v.write(reg(0x90, 377));
        v.write(reg(0x94, 377));
        v.write(reg(0xa0, 311));
        v.write(reg(0xa8, 38));
        v.write(reg(0xac, 38));
        v.write(reg(0xb0, 294));
        v.write(reg(0xb4, 294));
        v
    }

    #[test]
    fn register_encoding_and_timing() {
        let v = mode12();
        assert_eq!(v.bits_per_pixel(), 4);
        assert_eq!(v.pixel_mhz(), 16);
        assert_eq!(v.line_clocks(), 512, "1024 pixels at 16 MHz = 64 µs");
        assert_eq!(v.total_lines(), 312);
        assert_eq!(v.line_bytes(), 320);
    }

    #[test]
    fn a_line_of_four_bit_pixels() {
        let mut v = mode12();
        v.write(1 << 26 | 0x00f); // colour 1 red
        v.write(2 << 26 | 0x0f0); // colour 2 green
        v.frame_start();
        let (_, w, h) = v.picture();
        assert_eq!((w, h), (640, 256));
        let mut bytes = [0x21u8, 0x00].into_iter().cycle();
        v.draw_line(39, || bytes.next().unwrap_or(0), || 0);
        let (p, _, _) = v.picture();
        assert_eq!(p[6].r, 0xff, "low nibble first");
        assert_eq!(p[7].g, 0xff);
    }

    #[test]
    fn exponential_dac() {
        assert_eq!(sound_level(0), 0);
        assert!(sound_level(0xfe) > sound_level(0x7e));
        assert_eq!(sound_level(0xff), -sound_level(0xfe));
        assert_eq!(stereo_gain(1), (8, 0));
    }

    #[test]
    fn savestate_round_trip() {
        let v = mode12();
        let mut w = StateWriter::new();
        v.save(&mut w);
        let b = w.into_bytes();
        let mut n = Vidc::new();
        n.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(n.line_clocks(), 512);
    }
}
