//! The BBC Micro's Video ULA: it turns the bytes the 6845 addresses into
//! pixels through a 16-entry palette, and switches the 6845 between 2 and
//! 1 MHz.
//!
//! Registers (`&FE20`–`&FE2F`, write only):
//! - even address, control: bit 0 flash colour select, 1 teletext (the
//!   SAA5050's output instead of the pixels), 2–3 the pixel rate (`00` 2,
//!   `01` 4, `10` 8, `11` 16 MHz: 10, 20, 40, 80 characters a line), 4 the
//!   6845's clock (1 = 2 MHz), 5–7 the cursor segments (bit 7 the first,
//!   6 the second, 5 the third and fourth).
//! - odd address, palette: bits 7–4 the logical colour, 3–0 the physical
//!   colour exclusive-or 7 (bit 3 flashing).
//!
//! Pixels: the byte goes into a shift register; at each pixel clock bits
//! 7, 5, 3 and 1 form the logical colour and the register shifts left
//! (with 1s coming in). That one rule gives 1, 2 and 4 bits a pixel; the
//! MOS programs the palette so the unused bits do not matter. Physical
//! colours: bit 0 red, 1 green, 2 blue.
//!
//! Sources: Acorn, *BBC Microcomputer Advanced User Guide* (1983),
//! chapter 19 "The video ULA"; *BBC Microcomputer Service Manual*. The
//! implementation is this project's own.

use emu_core::{StateError, StateReader, StateWriter};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoUla {
    pub ctrl: u8,
    /// The palette as written (low nibble: physical colour ^ 7, bit 3 flash).
    pub palette: [u8; 16],
}

impl VideoUla {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        if addr & 1 == 0 {
            self.ctrl = v;
        } else {
            self.palette[usize::from(v >> 4)] = v & 0x0f;
        }
    }

    /// The 6845 runs at 2 MHz (else 1 MHz).
    pub fn fast_clock(&self) -> bool {
        self.ctrl & 0x10 != 0
    }

    pub fn teletext(&self) -> bool {
        self.ctrl & 0x02 != 0
    }

    /// The cursor's segments: how many character widths it covers (0–4).
    pub fn cursor_width(&self) -> u8 {
        u8::from(self.ctrl & 0x80 != 0)
            + u8::from(self.ctrl & 0x40 != 0)
            + 2 * u8::from(self.ctrl & 0x20 != 0)
    }

    /// The physical colour (0–7) of logical colour `logical`.
    pub fn colour(&self, logical: u8) -> u8 {
        let p = self.palette[usize::from(logical & 15)];
        let c = (p & 7) ^ 7;
        if p & 8 != 0 && self.ctrl & 1 != 0 {
            c ^ 7
        } else {
            c
        }
    }

    /// One character's pixels at 16 MHz: `out` is 8 pixels long at a 2 MHz
    /// 6845 clock, 16 at 1 MHz.
    pub fn pixels(&self, byte: u8, out: &mut [u8]) {
        let rate_mhz = 2usize << ((self.ctrl >> 2) & 3);
        let width = (16 / rate_mhz).max(1);
        let mut sr = byte;
        for chunk in out.chunks_mut(width) {
            let logical = (sr >> 4 & 8) | (sr >> 3 & 4) | (sr >> 2 & 2) | (sr >> 1 & 1);
            chunk.fill(self.colour(logical));
            sr = sr << 1 | 1;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.ctrl);
        w.raw(&self.palette);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let ctrl = r.u8()?;
        let mut palette = [0; 16];
        r.raw_into(&mut palette)?;
        self.ctrl = ctrl;
        self.palette = palette.map(|p| p & 0x0f);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_0_shows_one_bit_a_pixel() {
        let mut u = VideoUla::new();
        u.write(0, 0x9c);
        // MODE 0's palette: logical 0–7 black, 8–15 white (bit 7 decides).
        for l in 0..16u8 {
            u.write(1, l << 4 | if l >= 8 { 0 } else { 7 });
        }
        let mut px = [0u8; 8];
        u.pixels(0b1010_0001, &mut px);
        assert_eq!(px, [7, 0, 7, 0, 0, 0, 0, 7]);
        assert!(u.fast_clock());
        assert_eq!(u.cursor_width(), 1);
    }

    #[test]
    fn mode_2_takes_four_bits_a_pixel() {
        let mut u = VideoUla::new();
        u.write(0, 0xf4);
        for l in 0..16u8 {
            u.write(1, l << 4 | ((l & 7) ^ 7));
        }
        // Bits 7,5,3,1 = 1,0,0,1 → 9 (red); then 6,4,2,0 = 1,1,1,0 → 14
        // (cyan).
        let mut px = [0u8; 8];
        u.pixels(0b1101_0110, &mut px);
        assert_eq!(px, [1, 1, 1, 1, 6, 6, 6, 6]);
        assert_eq!(u.cursor_width(), 4);
    }

    #[test]
    fn mode_4_is_slow_and_doubles_pixels() {
        let mut u = VideoUla::new();
        u.write(0, 0x88);
        assert!(!u.fast_clock());
        for l in 0..16u8 {
            u.write(1, l << 4 | if l >= 8 { 0 } else { 7 });
        }
        let mut px = [0u8; 16];
        u.pixels(0b1000_0000, &mut px);
        assert_eq!(&px[..4], &[7, 7, 0, 0]);
    }
}
