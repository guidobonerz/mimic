//! The compact Macintosh's display: 512 × 342 pixels, one bit each (1 =
//! black), read from RAM by the video circuit, and the sound buffer it
//! reads alongside.
//!
//! **Timing** (Inside Macintosh III, "Hardware"; Wikipedia, "Macintosh
//! 128K/512K technical details"): pixels at 15.6672 MHz, two per CPU
//! clock; a line is 704 pixel periods (512 shown, 192 horizontal
//! blanking) = 352 CPU clocks, 44.9 µs; a frame 370 lines (342 shown, 28
//! vertical blanking) = 130 240 clocks, 60.15 Hz. The VIA's CA1 sees the
//! vertical blanking (a falling edge as it starts: the VBL interrupt) and
//! PB6 the horizontal blanking (H4).
//!
//! **Buffers**: the main screen starts $5900 below the top of RAM, the
//! alternate one $D900 below it (VIA PA6 = 0 selects it); one word of the
//! sound buffer ($300 below the top, alternate $5F00, VIA PA3 = 0) is read
//! at the end of each line, its high byte the sound sample and its low
//! byte the 400K drive's speed (pulse width).
//!
//! **Memory contention**: while a line is being shown the video circuit
//! takes every other 4-clock RAM slot (128K/512K/Plus), so a CPU RAM
//! cycle waits for the next of its slots; the SE's video takes one slot
//! in four (Big Mess o' Wires, "Mac Plus and SE memory timing", citing
//! MacTech). Outside the shown part of a line the CPU has RAM to itself.

pub const WIDTH: u32 = 512;
pub const HEIGHT: u32 = 342;
pub const LINE_CLOCKS: u64 = 352;
pub const LINES: u64 = 370;
pub const FRAME_CLOCKS: u64 = LINE_CLOCKS * LINES;
/// Clocks of a line during which pixels are shown.
pub const ACTIVE_CLOCKS: u64 = 256;
pub const PIXEL_HZ: u32 = 15_667_200;
pub const MAIN_SCREEN: u32 = 0x5900;
pub const ALT_SCREEN: u32 = 0xd900;
pub const MAIN_SOUND: u32 = 0x300;
pub const ALT_SOUND: u32 = 0x5f00;

/// Wait clocks for a CPU RAM cycle starting `in_line` clocks into line
/// `line`: `se` for the SE's one-in-four sharing.
pub fn ram_waits(line: u64, in_line: u64, se: bool) -> u32 {
    if line >= u64::from(HEIGHT) || in_line >= ACTIVE_CLOCKS {
        return 0;
    }
    if se {
        // Video owns clocks 12–15 of every 16.
        let p = in_line % 16;
        if p > 8 {
            (16 - p) as u32
        } else {
            0
        }
    } else {
        let p = in_line % 8;
        if p == 0 {
            0
        } else {
            (8 - p) as u32
        }
    }
}

/// The frame as indexed pixels (0 white, 1 black).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub pixels: Vec<u8>,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Self {
        Self {
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }

    /// Draw line `line` from the 64 bytes at `row`.
    pub fn draw_line(&mut self, line: u32, row: &[u8]) {
        let out = &mut self.pixels[(line * WIDTH) as usize..((line + 1) * WIDTH) as usize];
        for (i, b) in row.iter().take(64).enumerate() {
            for k in 0..8 {
                out[i * 8 + k] = b >> (7 - k) & 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_slots() {
        assert_eq!(ram_waits(0, 0, false), 0);
        assert_eq!(ram_waits(0, 2, false), 6);
        assert_eq!(ram_waits(0, 8, false), 0);
        assert_eq!(ram_waits(0, 300, false), 0, "horizontal blanking");
        assert_eq!(ram_waits(350, 2, false), 0, "vertical blanking");
        assert_eq!(ram_waits(0, 10, true), 6);
        assert_eq!(ram_waits(0, 4, true), 0);
    }

    #[test]
    fn line_bits_become_pixels() {
        let mut s = Screen::new();
        let mut row = [0u8; 64];
        row[0] = 0x81;
        s.draw_line(1, &row);
        assert_eq!(&s.pixels[512..520], &[1, 0, 0, 0, 0, 0, 0, 1]);
    }
}
