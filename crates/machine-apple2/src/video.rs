//! The Apple II video scanner and display modes.
//!
//! **Scanner.** The video counters run with the CPU: every CPU cycle the
//! video circuit reads one byte of display memory in the first half of the
//! cycle, the CPU uses the bus in the second. A line is 65 cycles (the
//! horizontal counter takes the states $00, $40–$7F), a frame 262 lines
//! (the vertical counter runs $0FA–$1FF); the last 40 states of a line
//! ($58–$7F) and the vertical states $100–$1BF are the visible 40 × 192
//! bytes. The address of every cycle — visible or not — comes from the
//! counters (Sather, "Understanding the Apple II", chapter 5, "The Video
//! Scanner", figures 5.9 and 5.10):
//!
//! - A0–A2 = H0–H2; A3–A6 = H5 H4 H3 + V4 V3 V4 V3 + 1101 (four bits);
//! - text and lo-res: A7–A9 = V0–V2, A10 = page 1, A11 = page 2;
//! - hi-res: A7–A9 = V0–V2, A10–A12 = VA–VC, A13 = page 1, A14 = page 2,
//!
//! with VA–VC the line within a character row and V0–V4 the row. The byte
//! read in a cycle is also what the CPU sees when it reads an address no
//! device drives (the "floating bus").
//!
//! **Dots.** Each visible cycle sends 14 dots at 14.318 MHz (four per
//! colour subcarrier cycle) as a luma level, 0 or 1; colour comes from the
//! monitor decoding the pattern ([`crate::Apple2::framebuffer`]):
//!
//! - text: seven dots per character from the character generator, each
//!   sent twice; II/II+ codes $00–$3F inverse, $40–$7F flashing (IIe: the
//!   alternate set has MouseText or inverse lower case there);
//! - lo-res: the byte's low nibble in the upper four lines of a character
//!   row, the high nibble in the lower four, sent as a 4-dot pattern that
//!   keeps its phase to the subcarrier (bit `x mod 4` at dot `x`);
//! - hi-res: bits 0–6 LSB first, each sent twice; bit 7 delays the byte by
//!   one dot, the first dot repeating the last of the byte before (Sather
//!   chapter 8, "Hi-res graphics");
//! - IIe with 80COL: text with 80 columns and, with AN3 off, double lo-res
//!   and double hi-res: an auxiliary-memory byte first, then the main
//!   memory byte at the same address, 7 dots each (Apple IIe Technical
//!   Reference Manual, chapter 2, "The Video Display"; Sather,
//!   "Understanding the Apple IIe", chapter 8).
//!
//! The text window of mixed mode is the bottom four rows (V4 and V2 set).
//! The colour burst is off in text mode, so monitors show a frame without
//! graphics in black and white.
//!
//! **Character generators** (user-supplied; this project's font
//! [`crate::font`] without one). II/II+: the 2 KB 341-0036, eight bytes a
//! code (row 0 on top), the dots of a row in bits 6–0 from left to right;
//! inverse and flashing come from the video circuit. IIe: the 4 KB
//! 342-0133 (enhanced 342-0265), of which the first 2 KB are used: eight
//! bytes a code, bit 0 the leftmost dot, stored so that a 0 lights the
//! dot; $00–$3F hold the inverse characters, $40–$7F the alternate set's
//! (MouseText or inverse upper case, inverse lower case), $80–$FF the
//! normal ones; the primary set shows $40–$7F as the inverse codes
//! $00–$3F, inverted again while the flash is on. Layouts read from the
//! ROM images (glyph shapes of '@', 'L' and '/').

use crate::font;

/// CPU cycles per line.
pub const CYCLES_PER_LINE: u16 = 65;
/// Lines per frame (NTSC).
pub const LINES: u16 = 262;
/// CPU cycles per frame.
pub const FRAME_CYCLES: u64 = CYCLES_PER_LINE as u64 * LINES as u64;
/// The first visible cycle of a line (horizontal state $58).
pub const FIRST_VISIBLE: u16 = 25;
/// Visible lines.
pub const ACTIVE_LINES: u16 = 192;
/// Dots (14.318 MHz) per CPU cycle.
pub const DOTS: usize = 14;
/// Dot clock: four times the NTSC colour subcarrier.
pub const DOT_HZ: u32 = 14_318_182;
/// Frame pixels per dot: the frame is sampled at twice the dot clock, so
/// the picture can sit half a dot from the colour reference.
pub const PX_PER_DOT: usize = 2;
/// Pixel clock of the frame.
pub const PIXEL_HZ: u32 = 2 * DOT_HZ;
/// Pixels per line including blanking (912 dots: the 65th cycle is two
/// dots longer).
pub const TOTAL_PIXELS: u32 = 912 * PX_PER_DOT as u32;
/// Black border around the picture: pixels left and right, lines above
/// and below.
pub const BORDER_X: u32 = 80;
pub const BORDER_Y: u32 = 24;
/// Where the first dot of a line starts, in pixels after the border: the
/// monitor's colour reference (see `video_crt::signal`, column 0 at
/// subcarrier phase 180°) against the Apple's dots. Three pixels (one and
/// a half dots, 135° of the subcarrier) put the four hi-res colours where an NTSC
/// monitor shows them: green, violet, orange and blue at about 105°,
/// 285°, 15° and 195° (checked with the composite stage in
/// `tests/tests/apple2.rs`).
pub const PHASE_PX: usize = 3;
/// The frame: 560 dots (1120 pixels) and 192 lines with the borders.
pub const WIDTH: u32 = 560 * PX_PER_DOT as u32 + 2 * BORDER_X;
pub const HEIGHT: u32 = ACTIVE_LINES as u32 + 2 * BORDER_Y;
/// Frames between flashing-character changes (about 1.9 Hz).
const FLASH_FRAMES: u8 = 16;

/// The soft switches the display depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub text: bool,
    pub mixed: bool,
    pub page2: bool,
    pub hires: bool,
    /// IIe only.
    pub col80: bool,
    pub store80: bool,
    pub altchar: bool,
    /// Double hi-res/lo-res: AN3 off (IIe only).
    pub dhires: bool,
}

impl Modes {
    pub fn bits(&self) -> u8 {
        [
            self.text,
            self.mixed,
            self.page2,
            self.hires,
            self.col80,
            self.store80,
            self.altchar,
            self.dhires,
        ]
        .iter()
        .enumerate()
        .fold(0, |v, (i, &b)| v | (u8::from(b) << i))
    }

    pub fn from_bits(v: u8) -> Self {
        let b = |i: u8| v & (1 << i) != 0;
        Self {
            text: b(0),
            mixed: b(1),
            page2: b(2),
            hires: b(3),
            col80: b(4),
            store80: b(5),
            altchar: b(6),
            dhires: b(7),
        }
    }
}

/// Which character set the text modes use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charset {
    /// II/II+: upper case only, $40–$7F flashing.
    Ii,
    /// IIe: lower case; the alternate set has inverse lower case at
    /// $60–$7F ($40–$5F stay flashing upper case on the original IIe).
    IIe,
    /// Enhanced IIe: the alternate set has MouseText at $40–$5F.
    IIeEnhanced,
}

/// Video counters, the picture and the flash timer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    /// Cycle in the line (0–64) and line in the frame (0–261; 0–191 visible).
    pub h: u16,
    pub line: u16,
    pub frame: u64,
    pub charset: Charset,
    /// Characters $40–$7F show inverted now.
    pub flash: bool,
    flash_count: u8,
    /// The last dot sent (a delayed hi-res byte starts with it).
    last_dot: u8,
    /// Visible lines of this frame that started in a graphics mode, and
    /// whether the last complete frame had any (the colour burst).
    graphics_lines: u16,
    pub burst: bool,
    /// The character generator ROM (see the module docs).
    pub char_rom: Option<Vec<u8>>,
    pixels: Vec<u8>,
}

/// Bits of the vertical counter for line `line` (VA = bit 0 … V5 = bit 8).
fn vcount(line: u16) -> u16 {
    if line < 256 {
        0x100 + line
    } else {
        0xfa + (line - 256)
    }
}

impl Video {
    pub fn new(charset: Charset) -> Self {
        Self {
            h: 0,
            line: 0,
            frame: 0,
            charset,
            flash: false,
            flash_count: 0,
            last_dot: 0,
            graphics_lines: 0,
            burst: false,
            char_rom: None,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The vertical blanking interval (lines 192–261).
    pub fn vbl(&self) -> bool {
        self.line >= ACTIVE_LINES
    }

    /// The mixed-mode text window: rows 20–23 (V4 and V2 set).
    fn text_window(v: u16) -> bool {
        v & 0x80 != 0 && v & 0x20 != 0
    }

    /// The display memory address of the current cycle.
    pub fn address(&self, m: &Modes) -> u16 {
        let hstate = if self.h == 0 { 0 } else { 0x40 + self.h - 1 };
        let v = vcount(self.line);
        let (v3, v4) = ((v >> 6) & 1, (v >> 7) & 1);
        let sum = (0b1101 + ((hstate >> 3) & 7) + (v4 << 3 | v3 << 2 | v4 << 1 | v3)) & 0x0f;
        let low = (hstate & 7) | sum << 3 | ((v >> 3) & 7) << 7;
        let page2 = m.page2 && !m.store80;
        let text = m.text || (m.mixed && Self::text_window(v));
        if m.hires && !text {
            low | (v & 7) << 10 | if page2 { 0x4000 } else { 0x2000 }
        } else {
            low | if page2 { 0x0800 } else { 0x0400 }
        }
    }

    /// One CPU cycle: the scanner reads display memory (`main`, and `aux`
    /// for the 80-column modes), visible bytes become dots. Returns the
    /// byte read (the floating bus). Advances the counters; `true` at the
    /// end of a frame.
    pub fn cycle(&mut self, main: &[u8], aux: Option<&[u8]>, m: &Modes) -> (u8, bool) {
        let addr = usize::from(self.address(m));
        let byte = main.get(addr).copied().unwrap_or(0);
        if self.line < ACTIVE_LINES {
            if self.h == 0 && (!m.text || m.mixed) {
                self.graphics_lines += 1;
            }
            if self.h >= FIRST_VISIBLE {
                let auxb = aux.and_then(|a| a.get(addr).copied());
                self.render(byte, auxb, m);
            }
        }
        self.h += 1;
        let mut frame_done = false;
        if self.h == CYCLES_PER_LINE {
            self.h = 0;
            self.line += 1;
            if self.line == LINES {
                self.line = 0;
                self.frame += 1;
                self.burst = self.graphics_lines > 0;
                self.graphics_lines = 0;
                self.flash_count += 1;
                if self.flash_count >= FLASH_FRAMES {
                    self.flash_count = 0;
                    self.flash = !self.flash;
                }
                frame_done = true;
            }
        }
        (byte, frame_done)
    }

    /// Seven dots of character `code` in cell row `row`, bit 0 first,
    /// inverse and flashing applied.
    pub(crate) fn char_dots(&self, code: u8, row: u8, altchar: bool) -> u8 {
        if let Some(rom) = &self.char_rom {
            return self.rom_dots(rom, code, row, altchar);
        }
        // ASCII of the glyph, and whether it shows inverted.
        let upper = |c: u8| {
            if c & 0x3f < 0x20 {
                (c & 0x3f) + 0x40
            } else {
                c & 0x3f
            }
        };
        let (ascii, inverse) = match (self.charset, code) {
            (_, 0x00..=0x3f) => (upper(code), true),
            (Charset::Ii, 0x40..=0x7f) => (upper(code), self.flash),
            (Charset::Ii, _) => (upper(code), false),
            (Charset::IIeEnhanced, 0x40..=0x5f) if altchar => (0x7f, false),
            (_, 0x60..=0x7f) if altchar => (code, true),
            (_, 0x40..=0x7f) => (upper(code), self.flash),
            (_, 0x80..=0x9f) => (upper(code), false),
            (_, _) => (code & 0x7f, false),
        };
        let dots = font::row(ascii, row);
        if inverse {
            dots ^ 0x7f
        } else {
            dots
        }
    }

    fn rom_dots(&self, rom: &[u8], code: u8, row: u8, altchar: bool) -> u8 {
        let byte = |c: u8| {
            rom.get(usize::from(c) * 8 + usize::from(row))
                .copied()
                .unwrap_or(0)
        };
        match self.charset {
            Charset::Ii => {
                let b = byte(code);
                let dots = (0..7).fold(0, |acc, i| acc | (((b >> (6 - i)) & 1) << i));
                let inverse = code < 0x40 || (code < 0x80 && self.flash);
                if inverse {
                    dots ^ 0x7f
                } else {
                    dots
                }
            }
            _ => {
                if !altchar && (0x40..0x80).contains(&code) {
                    let dots = !byte(code & 0x3f) & 0x7f;
                    if self.flash {
                        dots ^ 0x7f
                    } else {
                        dots
                    }
                } else {
                    !byte(code) & 0x7f
                }
            }
        }
    }

    fn render(&mut self, byte: u8, aux: Option<u8>, m: &Modes) {
        let col = usize::from(self.h - FIRST_VISIBLE);
        let y = usize::from(self.line);
        let v = vcount(self.line);
        let row = (y & 7) as u8;
        let text = m.text || (m.mixed && Self::text_window(v));
        let double = m.col80 && aux.is_some();
        let aux = aux.unwrap_or(0);
        let mut dots = [0u8; DOTS];
        if text && double {
            let a = self.char_dots(aux, row, m.altchar);
            let b = self.char_dots(byte, row, m.altchar);
            for i in 0..7 {
                dots[i] = (a >> i) & 1;
                dots[7 + i] = (b >> i) & 1;
            }
        } else if text {
            let c = self.char_dots(byte, row, m.altchar);
            for (i, d) in dots.iter_mut().enumerate() {
                *d = (c >> (i / 2)) & 1;
            }
        } else if m.hires && double && m.dhires {
            for i in 0..7 {
                dots[i] = (aux >> i) & 1;
                dots[7 + i] = (byte >> i) & 1;
            }
        } else if m.hires {
            let delayed = byte & 0x80 != 0;
            for (i, d) in dots.iter_mut().enumerate() {
                *d = match (delayed, i) {
                    (true, 0) => self.last_dot,
                    (true, _) => (byte >> ((i - 1) / 2)) & 1,
                    (false, _) => (byte >> (i / 2)) & 1,
                };
            }
            if delayed {
                // The second half of bit 6 falls into the next byte's time.
                self.last_dot = (byte >> 6) & 1;
                self.put(col, &dots);
                return;
            }
        } else {
            let shift = if y & 4 == 0 { 0 } else { 4 };
            let main_nibble = (byte >> shift) & 0x0f;
            let aux_nibble = (aux >> shift) & 0x0f;
            let dlores = double && m.dhires;
            for (i, d) in dots.iter_mut().enumerate() {
                let x = col * DOTS + i;
                let nibble = if dlores && i < 7 {
                    aux_nibble
                } else {
                    main_nibble
                };
                *d = (nibble >> (x & 3)) & 1;
            }
        }
        self.last_dot = dots[DOTS - 1];
        self.put(col, &dots);
    }

    fn base(&self, col: usize) -> usize {
        (usize::from(self.line) + BORDER_Y as usize) * WIDTH as usize
            + BORDER_X as usize
            + PHASE_PX
            + col * DOTS * PX_PER_DOT
    }

    /// The 14 dots of column `col` into the frame.
    fn put(&mut self, col: usize, dots: &[u8; DOTS]) {
        let base = self.base(col);
        let (chunks, _) = self.pixels[base..base + DOTS * PX_PER_DOT].as_chunks_mut::<PX_PER_DOT>();
        for (px, &d) in chunks.iter_mut().zip(dots) {
            px.fill(d);
        }
    }

    pub fn save(&self, w: &mut emu_core::StateWriter) {
        w.chunk(*b"A2VD", 1, |w| {
            w.u16(self.h);
            w.u16(self.line);
            w.u64(self.frame);
            w.bool(self.flash);
            w.u8(self.flash_count);
            w.u8(self.last_dot);
            w.u16(self.graphics_lines);
            w.bool(self.burst);
            w.bytes(&self.pixels);
        });
    }

    /// Atomic: on error the video is unchanged.
    pub fn load(&mut self, r: &mut emu_core::StateReader<'_>) -> Result<(), emu_core::StateError> {
        let (_, mut c) = r.chunk(*b"A2VD", 1)?;
        let mut n = self.clone();
        n.h = c.u16()?;
        n.line = c.u16()?;
        n.frame = c.u64()?;
        n.flash = c.bool()?;
        n.flash_count = c.u8()?;
        n.last_dot = c.u8()? & 1;
        n.graphics_lines = c.u16()?;
        n.burst = c.bool()?;
        let px = c.bytes()?;
        if n.h >= CYCLES_PER_LINE || n.line >= LINES || px.len() != n.pixels.len() {
            return Err(emu_core::StateError::Invalid("apple2 video"));
        }
        n.pixels = px.iter().map(|&p| p & 1).collect();
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u16, line: u16) -> Video {
        let mut v = Video::new(Charset::Ii);
        v.h = h;
        v.line = line;
        v
    }

    #[test]
    fn text_addresses_follow_the_row_layout() {
        let text = Modes {
            text: true,
            ..Modes::default()
        };
        // Row r, column c: $400 + (r mod 8) * $80 + (r / 8) * 40 + c.
        for (row, col) in [(0, 0), (1, 5), (8, 0), (16, 39), (23, 20)] {
            let v = at(FIRST_VISIBLE + col, row * 8 + 3);
            let want = 0x400 + (row % 8) * 0x80 + (row / 8) * 40 + col;
            assert_eq!(v.address(&text), want, "row {row} col {col}");
        }
        let page2 = Modes {
            page2: true,
            ..text
        };
        assert_eq!(at(FIRST_VISIBLE, 0).address(&page2), 0x800);
    }

    #[test]
    fn hires_addresses_follow_the_line_layout() {
        let hires = Modes {
            hires: true,
            ..Modes::default()
        };
        for (y, col) in [(0u16, 0u16), (1, 1), (8, 2), (63, 39), (64, 0), (191, 39)] {
            let v = at(FIRST_VISIBLE + col, y);
            let want = 0x2000 + (y % 8) * 0x400 + ((y / 8) % 8) * 0x80 + (y / 64) * 40 + col;
            assert_eq!(v.address(&hires), want, "line {y} col {col}");
        }
        // Mixed mode: the bottom four rows are text.
        let mixed = Modes {
            mixed: true,
            ..hires
        };
        assert_eq!(at(FIRST_VISIBLE, 160).address(&mixed), 0x650);
        assert_eq!(at(FIRST_VISIBLE, 159).address(&mixed) & 0xe000, 0x2000);
    }

    #[test]
    fn horizontal_blanking_reads_the_bytes_before_the_row() {
        // State $40 (cycle 1) is 24 bytes before the row start, wrapping
        // within the 128-byte block: row 0 reads $468 ($400 + 128 - 24).
        let text = Modes {
            text: true,
            ..Modes::default()
        };
        assert_eq!(at(1, 0).address(&text), 0x468);
        assert_eq!(at(0, 0).address(&text), 0x468, "state $00 like $40");
    }

    #[test]
    fn a_frame_is_17030_cycles() {
        let mut v = Video::new(Charset::Ii);
        let mem = vec![0u8; 0x10000];
        let m = Modes::default();
        let mut n = 0u64;
        loop {
            n += 1;
            if v.cycle(&mem, None, &m).1 {
                break;
            }
        }
        assert_eq!(n, FRAME_CYCLES);
        assert_eq!(n, 17_030);
    }

    #[test]
    fn hires_bit7_delays_the_byte_by_one_dot() {
        let mut mem = vec![0u8; 0x10000];
        mem[0x2000] = 0x01;
        mem[0x2001] = 0x81;
        let m = Modes {
            hires: true,
            ..Modes::default()
        };
        let mut v = Video::new(Charset::Ii);
        while !(v.line == 0 && v.h == FIRST_VISIBLE + 2) {
            v.cycle(&mem, None, &m);
        }
        let start = (BORDER_Y * WIDTH + BORDER_X) as usize + PHASE_PX;
        let row: Vec<u8> = v.pixels()[start..][..56]
            .iter()
            .step_by(2)
            .copied()
            .collect();
        assert_eq!(&row[..3], &[1, 1, 0], "undelayed: bit 0 on dots 0-1");
        assert_eq!(&row[14..18], &[0, 1, 1, 0], "delayed: bit 0 on dots 1-2");
    }

    #[test]
    fn lores_patterns_keep_their_phase() {
        let mut mem = vec![0u8; 0x10000];
        mem[0x400] = 0x01;
        mem[0x401] = 0x01;
        let m = Modes::default();
        let mut v = Video::new(Charset::Ii);
        while !(v.line == 0 && v.h == FIRST_VISIBLE + 2) {
            v.cycle(&mem, None, &m);
        }
        let start = (BORDER_Y * WIDTH + BORDER_X) as usize + PHASE_PX;
        let row: Vec<u8> = v.pixels()[start..][..56]
            .iter()
            .step_by(2)
            .copied()
            .collect();
        // Colour 1 lights every fourth dot from dot 0, also in column 1.
        for (x, &d) in row.iter().enumerate() {
            assert_eq!(d, u8::from(x % 4 == 0), "dot {x}");
        }
    }
}
