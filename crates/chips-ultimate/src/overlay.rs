//! The user-interface overlay of the Ultimate 64 Elite II: the character
//! generator that lays the board's menu over the HDMI picture, and the
//! registers of the HDMI output beside it. I/O `$140000`–`$14FFFF`.
//!
//! | address             | what                                              |
//! |---------------------|---------------------------------------------------|
//! | `$140000`–`$140FFF` | character generator registers (16, mirrored)      |
//! | `$141000`–`$141FFF` | screen RAM, a character code per cell             |
//! | `$142000`–`$142FFF` | colour RAM, a colour byte per cell                |
//! | `$144000`–`$14401D` | HDMI timing and scaler registers                  |
//! | `$145000`–`$14503F` | HDMI palette, 16 × {red, green, blue, unused}     |
//! | `$148000`–`$148003` | VIC cropper                                       |
//!
//! # Character generator registers (write only, a read gives 0)
//!
//! | offset  | name           | bits                                         |
//! |---------|----------------|----------------------------------------------|
//! | `0`,`1` | LINE_CLOCKS    | high, low; never written by the firmware     |
//! | `2`     | CHAR_WIDTH     | 0–3: pixels per cell across                  |
//! | `3`     | CHAR_HEIGHT    | 0–4: line count of a cell; 6: big font (12 × 24); 7: every font line twice |
//! | `4`     | CHARS_PER_LINE | cells per row                                |
//! | `5`     | ACTIVE_LINES   | 0–5: rows                                    |
//! | `6`,`7` | X_ON           | high (4 bits), low: first pixel of the window, counted from the line sync |
//! | `8`,`9` | Y_ON           | high (4 bits), low: first line, counted from the frame sync |
//! | `A`,`B` | POINTER        | high (7 bits), low: screen RAM index of the first cell |
//! | `C`     | PERFORM_SYNC   | the firmware writes 0                        |
//! | `D`     | TRANSPARENCY   | 0–3: the colour that lets the picture through; 6: the board owns the keyboard; 7: overlay shown |
//!
//! The firmware shows the menu by writing `$C0` to TRANSPARENCY and hides
//! it with `$00`; the screen is filled long before it is first shown.
//!
//! # A cell
//!
//! Screen RAM: bits 0–6 the character, bit 7 reverse (foreground and
//! background change places). The firmware's cursor is nothing but that
//! bit toggled in the RAM, which is why both RAMs read back. Colour RAM:
//! low nibble foreground, high nibble background, both indexes into the
//! HDMI palette. A pixel whose colour index equals the transparent colour
//! is not drawn at all — with the firmware's `$C0` every cell with
//! background 0 shows the C64's picture through, only the selected line
//! (background 6) is solid.
//!
//! Character codes are ASCII: `$20`–`$7E` are the ASCII characters, and
//! `$01`–`$13` are line graphics (`$01` ┌, `$02` ─, `$03` ┐, `$04` │,
//! `$05` └, `$06` ┘, `$07` ╭, `$08` ┬, `$09` ╮, `$0A` ├, `$0B` a block
//! without its top line, `$0C` ┤, `$0D` ┴, `$0E` ╰, `$0F` ╯, `$10` α,
//! `$11` β, `$12` a block without its bottom line, `$13` ♦). They are
//! neither C64 screen codes nor PETSCII.
//!
//! Lines of a cell, by CHAR_HEIGHT: with bit 7, font line `y / 2`; with
//! bit 6, the big font, where the line counter skips every fourth value
//! (so `$5E`, count 30, makes 23 lines, three per font row and two for
//! the last); otherwise font line `y` for `y` < 8 and for the lines after
//! it the last font line again, but only if that is `$18` (a vertical line
//! runs on), else blank. The firmware uses 8 × 9 (`$09`), 8 × 16 (`$90`)
//! and 12 × 23 (`$5E`) cells, always 40 × 25 of them.
//!
//! # The font
//!
//! The character ROMs are part of the FPGA's configuration; the firmware
//! does not load them, and mimic ships none. [`Overlay::render`]
//! therefore takes the 8 × 8 font from the caller: 8 bytes per character,
//! top line first, bit 7 the left pixel, characters `$00`–`$7F` in the
//! overlay's code (ASCII with the graphics below `$20`), 1024 bytes
//! ([`FONT_BYTES`]); anything after that is ignored and what is missing
//! is blank. Two ways to get one:
//!
//! - the file `roms/chars.bin` of the firmware's repository (2048 bytes,
//!   the first 1024 are what the FPGA holds), if the user supplies it,
//!   passed as it is;
//! - [`font_from_c64_chargen`], which puts one together from the C64's
//!   character generator ROM. Letters, digits and punctuation then have
//!   Commodore's shapes; the line graphics come from the C64's graphics
//!   characters, whose lines run through the same pixels; backslash,
//!   caret, underscore, grave accent, braces, bar and tilde, α, β, the
//!   two blocks and `$7F` are drawn by this module; the pieces
//!   `$14`–`$1F` (parts of a logo) are blank.
//!
//! The board's second ROM, the 12 × 24 font of the big cells, has no file
//! of that kind. Big cells are drawn from the same 8 × 8 font, one and a
//! half times as wide and each line three times — the cell's place and
//! size are right, the letters coarser than the board's.
//!
//! # Where the window is
//!
//! X_ON and Y_ON count output pixels and lines from the sync pulses, so
//! in the active picture the window starts at `X_ON - (sync + back
//! porch)`, and likewise down; the timing registers give those. It sits
//! right of the centre and below the middle in every mode the firmware
//! sets and never fills the screen.
//!
//! Timing registers used here (offset in `$144000`; the firmware writes
//! them shifted): `1` line sync / 2, `2` back porch / 4, `3` active
//! pixels / 8, `4` front porch / 4, `5` the bits lost by that (bit 1 sync
//! bit 0, bits 2–3 active bits 1–2, bits 4–5 back porch bits 0–1, bits
//! 6–7 front porch bits 0–1; bit 0 pixel repetition), `9` frame sync
//! lines, `A` back porch lines, `B` active lines / 8, `C` front porch
//! lines; further `10` x offset of the scaled picture, `13`/`14` the
//! scalers' ratio codes. All of them, the palette and the cropper
//! (`0` x, `1` y, `2` width / 2, `3` height / 2 of the part of the VIC-II
//! picture that is scaled up) are stored and read back as written.
//!
//! [`Overlay::render`] treats the frame it is given as the whole active
//! HDMI picture, stretched: a frame of exactly [`Overlay::canvas_size`]
//! gets the window pixel for pixel, any other size takes for each of its
//! pixels the nearest one of the HDMI picture (text stays readable only
//! when the frame is not much smaller than the canvas). Before the
//! firmware has set a timing the canvas is the frame itself (or the
//! window, where that is larger) with the window in its middle.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/05-ui-overlay-input.md` (register
//! table, rendering rules, window position, fonts, hazard 12),
//! `00-memory-map.md` (C32, C33), `03-board-init.md` (timing register
//! list), `docs/specs/S07-overlay-u64io-render.md` (canvas, cropper);
//! 1541 Ultimate `software/io/overlay/chargen.h` and `overlay.h`
//! (register order, show/hide), `software/system/u64.h` (addresses,
//! timing register order), `software/u64/hdmi_scan.cc` (how a mode is
//! packed into the timing registers), `software/io/c64/screen.h` (names
//! of the graphics characters), `fpga/ip/video/vhdl_source/
//! char_generator_slave12.vhd`, `char_generator_regs.vhd` and
//! `char_generator_timing.vhd` (line counting, pixel order, reverse,
//! transparency, register decode, counters from sync), `roms/chars.bin`
//! (looked at for which shape each graphics code has; no byte of it is
//! in this file).

use crate::io::{IoCtx, IoDevice};
use mimic_core::{StateError, StateReader, StateWriter};

/// First and last address of the block.
pub const BASE: u32 = 0x14_0000;
pub const END: u32 = 0x14_FFFF;

/// Register offsets of the character generator.
pub mod reg {
    pub const CHAR_WIDTH: usize = 0x2;
    pub const CHAR_HEIGHT: usize = 0x3;
    pub const CHARS_PER_LINE: usize = 0x4;
    pub const ACTIVE_LINES: usize = 0x5;
    pub const X_ON_HI: usize = 0x6;
    pub const X_ON_LO: usize = 0x7;
    pub const Y_ON_HI: usize = 0x8;
    pub const Y_ON_LO: usize = 0x9;
    pub const POINTER_HI: usize = 0xA;
    pub const POINTER_LO: usize = 0xB;
    pub const PERFORM_SYNC: usize = 0xC;
    pub const TRANSPARENCY: usize = 0xD;
}

const RAM_SIZE: usize = 0x1000;
/// Bytes of a font for [`Overlay::render`]: 128 characters of 8 lines.
pub const FONT_BYTES: usize = 128 * 8;

/// The HDMI mode the firmware programmed, in pixels and lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HdmiTiming {
    pub h_active: u16,
    pub h_front_porch: u16,
    pub h_sync: u16,
    pub h_back_porch: u16,
    pub v_active: u16,
    pub v_front_porch: u16,
    pub v_sync: u16,
    pub v_back_porch: u16,
}

/// The character window as the registers define it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub cols: usize,
    pub rows: usize,
    /// Pixels of a cell across.
    pub cell_width: usize,
    /// Lines of a cell as shown.
    pub cell_height: usize,
}

impl Window {
    pub fn width(&self) -> usize {
        self.cols * self.cell_width
    }

    pub fn height(&self) -> usize {
        self.rows * self.cell_height
    }
}

/// The part of the VIC-II picture the board scales up for HDMI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VicCrop {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    regs: [u8; 16],
    screen: Vec<u8>,
    colour: Vec<u8>,
    timing: [u8; 32],
    palette: [u8; 64],
    cropper: [u8; 4],
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    /// The block after configuration: registers 0, the screen full of
    /// spaces in colour `$0F` (the RAMs' initial contents in the FPGA).
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            screen: vec![0x20; RAM_SIZE],
            colour: vec![0x0F; RAM_SIZE],
            timing: [0; 32],
            palette: [0; 64],
            cropper: [0; 4],
        }
    }

    // ---- what the registers say -----------------------------------------

    /// The overlay is shown (TRANSPARENCY bit 7).
    pub fn active(&self) -> bool {
        self.regs[reg::TRANSPARENCY] & 0x80 != 0
    }

    /// The board has taken the C64's keyboard for its menu (TRANSPARENCY
    /// bit 6): keys should go to the board's scan, not to the C64.
    pub fn owns_keyboard(&self) -> bool {
        self.regs[reg::TRANSPARENCY] & 0x40 != 0
    }

    /// The colour index that is not drawn.
    pub fn transparent(&self) -> u8 {
        self.regs[reg::TRANSPARENCY] & 0x0F
    }

    /// A character generator register as written.
    pub fn reg(&self, offset: usize) -> u8 {
        self.regs[offset & 15]
    }

    /// X_ON and Y_ON: the window's first pixel and line, counted from the
    /// sync pulses.
    pub fn origin(&self) -> (u16, u16) {
        let word =
            |hi: usize, lo: usize| u16::from(self.regs[hi] & 0x0F) << 8 | u16::from(self.regs[lo]);
        (
            word(reg::X_ON_HI, reg::X_ON_LO),
            word(reg::Y_ON_HI, reg::Y_ON_LO),
        )
    }

    /// Screen RAM index of the first cell.
    pub fn pointer(&self) -> usize {
        let hi = usize::from(self.regs[reg::POINTER_HI] & 0x7F);
        (hi << 8 | usize::from(self.regs[reg::POINTER_LO])) & (RAM_SIZE - 1)
    }

    /// The values the cell's line counter takes, one per line shown.
    fn cell_lines(&self) -> Vec<u8> {
        let h = self.regs[reg::CHAR_HEIGHT];
        let big = h & 0x40 != 0;
        (0..h & 0x1F).filter(|y| !big || y & 3 != 3).collect()
    }

    /// The window, or `None` while the registers describe none (a size
    /// of 0 somewhere).
    pub fn window(&self) -> Option<Window> {
        let w = Window {
            cols: usize::from(self.regs[reg::CHARS_PER_LINE]).min(128),
            rows: usize::from(self.regs[reg::ACTIVE_LINES] & 0x3F),
            cell_width: usize::from(self.regs[reg::CHAR_WIDTH] & 0x0F),
            cell_height: self.cell_lines().len(),
        };
        (w.width() != 0 && w.height() != 0).then_some(w)
    }

    /// The HDMI mode, or `None` before the firmware has programmed one.
    pub fn timing(&self) -> Option<HdmiTiming> {
        let t = |i: usize| u16::from(self.timing[i]);
        let rep = t(5);
        let timing = HdmiTiming {
            h_sync: t(1) << 1 | (rep >> 1 & 1),
            h_back_porch: t(2) << 2 | (rep >> 4 & 3),
            h_active: t(3) << 3 | (rep >> 1 & 6),
            h_front_porch: t(4) << 2 | (rep >> 6 & 3),
            v_sync: t(9),
            v_back_porch: t(0xA),
            v_active: t(0xB) << 3,
            v_front_porch: t(0xC),
        };
        (timing.h_active != 0 && timing.v_active != 0).then_some(timing)
    }

    /// The size of the active HDMI picture: the frame size at which
    /// [`Overlay::render`] draws pixel for pixel. `None` before a mode is
    /// programmed.
    pub fn canvas_size(&self) -> Option<(usize, usize)> {
        self.timing()
            .map(|t| (usize::from(t.h_active), usize::from(t.v_active)))
    }

    /// Top left corner of the window in the active HDMI picture (may lie
    /// outside), `None` without a mode.
    pub fn window_position(&self) -> Option<(i32, i32)> {
        let t = self.timing()?;
        let (x, y) = self.origin();
        Some((
            i32::from(x) - i32::from(t.h_sync) - i32::from(t.h_back_porch),
            i32::from(y) - i32::from(t.v_sync) - i32::from(t.v_back_porch),
        ))
    }

    /// An HDMI timing register as written (offset 0–`$1F`).
    pub fn timing_reg(&self, offset: usize) -> u8 {
        self.timing[offset & 31]
    }

    /// The cropper's rectangle in the VIC-II picture.
    pub fn vic_crop(&self) -> VicCrop {
        VicCrop {
            x: u16::from(self.cropper[0]),
            y: u16::from(self.cropper[1]),
            width: u16::from(self.cropper[2]) * 2,
            height: u16::from(self.cropper[3]) * 2,
        }
    }

    /// Red, green, blue of HDMI palette entry `index` (0–15).
    pub fn palette_rgb(&self, index: u8) -> [u8; 3] {
        let i = usize::from(index & 15) * 4;
        [self.palette[i], self.palette[i + 1], self.palette[i + 2]]
    }

    /// The 4 KB of screen RAM.
    pub fn screen_ram(&self) -> &[u8] {
        &self.screen
    }

    /// The 4 KB of colour RAM.
    pub fn colour_ram(&self) -> &[u8] {
        &self.colour
    }

    /// Character and colour byte of the cell at `row`, `col` of the
    /// window.
    pub fn cell(&self, row: usize, col: usize) -> (u8, u8) {
        let cols = usize::from(self.regs[reg::CHARS_PER_LINE]);
        let i = (self.pointer() + row * cols + col) & (RAM_SIZE - 1);
        (self.screen[i], self.colour[i])
    }

    /// The window as text, a string per row, whether shown or not (the
    /// firmware keeps it filled while hidden). `$20`–`$7E` are themselves
    /// (the codes are ASCII); the graphics become `-` (`$02`), `|`
    /// (`$04`), `+` (corners and tees), `#` (blocks), `*` (diamond), `a`
    /// and `b` (α, β); code 0 is a space and everything else `.`. The
    /// reverse bit is dropped, so neither cursor nor selection shows
    /// (the selection is a colour anyway: see [`Overlay::cell`]); spaces
    /// at the end of a row are cut off.
    pub fn text(&self) -> Vec<String> {
        let rows = usize::from(self.regs[reg::ACTIVE_LINES] & 0x3F);
        let cols = usize::from(self.regs[reg::CHARS_PER_LINE]);
        (0..rows)
            .map(|r| {
                let line: String = (0..cols)
                    .map(|c| code_to_ascii(self.cell(r, c).0))
                    .collect();
                line.trim_end_matches(' ').to_string()
            })
            .collect()
    }

    /// The pixels of line `y` (a value of the cell's line counter) of
    /// character `code`: bit `width - 1` is the left pixel.
    fn glyph_line(&self, font: &[u8], code: u8, y: u8) -> u16 {
        let h = self.regs[reg::CHAR_HEIGHT];
        let glyph = usize::from(code & 0x7F) * 8;
        let row = |r: u8| font.get(glyph + usize::from(r & 7)).copied().unwrap_or(0);
        if h & 0x40 != 0 {
            // Big font: stands in for the board's 12 x 24 ROM.
            let bits = u16::from(row(y >> 2));
            (0..12u16).fold(0, |acc, x| acc << 1 | (bits >> (7 - x * 2 / 3) & 1))
        } else if h & 0x80 != 0 {
            u16::from(row(y >> 1))
        } else if y & 8 == 0 {
            u16::from(row(y))
        } else if row(7) == 0x18 {
            0x18
        } else {
            0
        }
    }

    /// Draw the overlay over a picture. `frame` is RGB, three bytes per
    /// pixel, `width` × `height` pixels, and stands for the whole active
    /// HDMI picture (see the module text for the mapping); the caller has
    /// put the C64's picture into it. `font` is the 8 × 8 font described
    /// in the module text. Nothing is drawn while the overlay is hidden,
    /// and a frame slice shorter than the size says is left alone.
    pub fn render(&self, font: &[u8], frame: &mut [u8], width: usize, height: usize) {
        let Some(win) = self.window() else {
            return;
        };
        if !self.active() || width == 0 || height == 0 {
            return;
        }
        match width.checked_mul(height).and_then(|n| n.checked_mul(3)) {
            Some(n) if n <= frame.len() => {}
            _ => return,
        }
        let (canvas_w, canvas_h, x0, y0) = match (self.canvas_size(), self.window_position()) {
            (Some((w, h)), Some((x, y))) => (w, h, i64::from(x), i64::from(y)),
            _ => {
                let w = width.max(win.width());
                let h = height.max(win.height());
                (
                    w,
                    h,
                    ((w - win.width()) / 2) as i64,
                    ((h - win.height()) / 2) as i64,
                )
            }
        };
        let lines = self.cell_lines();
        let transparent = self.transparent();
        let (win_w, win_h) = (win.width() as i64, win.height() as i64);
        // The line being drawn: a pattern and a colour byte per cell.
        let mut patterns = vec![(0u16, 0u8); win.cols];
        let mut have_line = None;
        for fy in 0..height {
            let wy = (fy * canvas_h / height) as i64 - y0;
            if !(0..win_h).contains(&wy) {
                continue;
            }
            let wy = wy as usize;
            if have_line != Some(wy) {
                let (row, y) = (wy / win.cell_height, lines[wy % win.cell_height]);
                for (col, p) in patterns.iter_mut().enumerate() {
                    let (code, attr) = self.cell(row, col);
                    let mut bits = self.glyph_line(font, code, y);
                    if code & 0x80 != 0 {
                        bits = !bits;
                    }
                    *p = (bits, attr);
                }
                have_line = Some(wy);
            }
            for fx in 0..width {
                let wx = (fx * canvas_w / width) as i64 - x0;
                if !(0..win_w).contains(&wx) {
                    continue;
                }
                let wx = wx as usize;
                let (bits, attr) = patterns[wx / win.cell_width];
                let bit = win.cell_width - 1 - wx % win.cell_width;
                let index = if bits >> bit & 1 != 0 {
                    attr & 0x0F
                } else {
                    attr >> 4
                };
                if index != transparent {
                    let at = (fy * width + fx) * 3;
                    frame[at..at + 3].copy_from_slice(&self.palette_rgb(index));
                }
            }
        }
    }
}

/// The ASCII stand-in of an overlay character code for [`Overlay::text`].
fn code_to_ascii(code: u8) -> char {
    match code & 0x7F {
        0x00 => ' ',
        c @ 0x20..=0x7E => char::from(c),
        0x02 => '-',
        0x04 => '|',
        0x01 | 0x03 | 0x05..=0x0A | 0x0C..=0x0F => '+',
        0x0B | 0x12 => '#',
        0x10 => 'a',
        0x11 => 'b',
        0x13 => '*',
        _ => '.',
    }
}

/// Characters this module draws itself for [`font_from_c64_chargen`]:
/// the C64 has no such character or another one in its place.
const OWN_GLYPHS: [(u8, [u8; 8]); 13] = [
    // Block without its top line, block without its bottom line.
    (0x0B, [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
    (0x12, [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]),
    // Alpha, beta.
    (0x10, [0x00, 0x00, 0x3A, 0x6C, 0x6C, 0x6C, 0x3A, 0x00]),
    (0x11, [0x38, 0x6C, 0x6C, 0x78, 0x6C, 0x66, 0x7C, 0x60]),
    (b'\\', [0x00, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x03, 0x00]),
    (b'^', [0x18, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'_', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF]),
    (b'`', [0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00]),
    (b'{', [0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00]),
    (b'|', [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00]),
    (b'}', [0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00]),
    (b'~', [0x00, 0x00, 0x76, 0xDC, 0x00, 0x00, 0x00, 0x00]),
    // A frame for the one code after the ASCII characters.
    (0x7F, [0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00]),
];

/// Line graphics of the overlay and the screen code of the same shape in
/// the C64's first character set (upper case and graphics).
const C64_GRAPHICS: [(u8, u8); 15] = [
    (0x01, 0x70), // ┌
    (0x02, 0x40), // ─
    (0x03, 0x6E), // ┐
    (0x04, 0x5D), // │
    (0x05, 0x6D), // └
    (0x06, 0x7D), // ┘
    (0x07, 0x55), // ╭
    (0x08, 0x72), // ┬
    (0x09, 0x49), // ╮
    (0x0A, 0x6B), // ├
    (0x0C, 0x73), // ┤
    (0x0D, 0x71), // ┴
    (0x0E, 0x4A), // ╰
    (0x0F, 0x4B), // ╯
    (0x13, 0x5A), // ♦
];

/// A font for [`Overlay::render`] made from the C64's character generator
/// ROM (4096 bytes: the upper case/graphics set, then the lower/upper
/// case set at `$800`, each indexed by screen code).
///
/// The overlay's codes are ASCII, the ROM is in screen-code order, so the
/// characters are picked: `$20`–`$3F` from the same screen codes, `@`
/// from 0, `A`–`Z` from `$41`–`$5A` and `a`–`z` from `$01`–`$1A` of the
/// second set, `[` and `]` from `$1B` and `$1D`; the line graphics
/// `$01`–`$0F` and the diamond from the first set's graphics, whose lines
/// run through the same pixels as the overlay's own. Characters the C64
/// lacks are drawn by this module, and `$14`–`$1F` stay blank (see the
/// module text). A ROM that is too short gives blanks for what is
/// missing; the result always has [`FONT_BYTES`] bytes.
pub fn font_from_c64_chargen(chargen: &[u8]) -> Vec<u8> {
    let mut font = vec![0u8; FONT_BYTES];
    let mut copy = |code: u8, set: usize, screen_code: u8| {
        let from = set * 0x800 + usize::from(screen_code) * 8;
        if let Some(glyph) = chargen.get(from..from + 8) {
            let to = usize::from(code) * 8;
            font[to..to + 8].copy_from_slice(glyph);
        }
    };
    for code in 0x20..=0x3F {
        copy(code, 1, code);
    }
    copy(b'@', 1, 0x00);
    for code in b'A'..=b'Z' {
        copy(code, 1, code);
    }
    for code in b'a'..=b'z' {
        copy(code, 1, code - 0x60);
    }
    copy(b'[', 1, 0x1B);
    copy(b']', 1, 0x1D);
    for (code, screen_code) in C64_GRAPHICS {
        copy(code, 0, screen_code);
    }
    for (code, glyph) in OWN_GLYPHS {
        let to = usize::from(code) * 8;
        font[to..to + 8].copy_from_slice(&glyph);
    }
    font
}

impl IoDevice for Overlay {
    fn read(&mut self, addr: u32, _ctx: &mut IoCtx<'_>) -> u8 {
        self.peek(addr)
    }

    fn peek(&self, addr: u32) -> u8 {
        if !(BASE..=END).contains(&addr) {
            return 0;
        }
        let offset = (addr & 0xFFF) as usize;
        let stored = |bytes: &[u8]| bytes.get(offset).copied().unwrap_or(0);
        match (addr >> 12) & 0xF {
            1 => self.screen[offset],
            2 => self.colour[offset],
            4 => stored(&self.timing),
            5 => stored(&self.palette),
            8 => stored(&self.cropper),
            // The character generator's registers cannot be read.
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, _ctx: &mut IoCtx<'_>) {
        if !(BASE..=END).contains(&addr) {
            return;
        }
        let offset = (addr & 0xFFF) as usize;
        let store = |bytes: &mut [u8]| {
            if let Some(b) = bytes.get_mut(offset) {
                *b = v;
            }
        };
        match (addr >> 12) & 0xF {
            0 => self.regs[offset & 15] = v,
            1 => self.screen[offset] = v,
            2 => self.colour[offset] = v,
            4 => store(&mut self.timing),
            5 => store(&mut self.palette),
            8 => store(&mut self.cropper),
            _ => {}
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn save(&self, w: &mut StateWriter) {
        w.raw(&self.regs);
        w.raw(&self.screen);
        w.raw(&self.colour);
        w.raw(&self.timing);
        w.raw(&self.palette);
        w.raw(&self.cropper);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut s = Self::new();
        r.raw_into(&mut s.regs)?;
        r.raw_into(&mut s.screen)?;
        r.raw_into(&mut s.colour)?;
        r.raw_into(&mut s.timing)?;
        r.raw_into(&mut s.palette)?;
        r.raw_into(&mut s.cropper)?;
        *self = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Irq, NullC64};

    const SCREEN: u32 = 0x14_1000;
    const COLOUR: u32 = 0x14_2000;
    const TIMING: u32 = 0x14_4000;
    const PALETTE: u32 = 0x14_5000;
    const CROPPER: u32 = 0x14_8000;

    const WHITE: [u8; 3] = [0xF7, 0xF7, 0xF7];
    const BLUE: [u8; 3] = [0x2C, 0x29, 0xB1];
    const GREY: [u8; 3] = [0x7B, 0x7B, 0x7B];
    /// What the test frames are filled with: the C64's picture.
    const PICTURE: [u8; 3] = [1, 2, 3];

    struct Rig {
        ov: Overlay,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                ov: Overlay::new(),
                ram: vec![0; 16],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
            }
        }

        fn wr(&mut self, addr: u32, v: u8) {
            let mut ctx = IoCtx {
                now: 0,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.ov.write(addr, v, &mut ctx);
        }

        fn rd(&mut self, addr: u32) -> u8 {
            let mut ctx = IoCtx {
                now: 0,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.ov.read(addr, &mut ctx)
        }

        /// The constructor's register writes of the firmware's overlay.
        fn geometry(&mut self, x: u16, y: u16, width: u8, height: u8) {
            self.wr(BASE + 4, 40);
            self.wr(BASE + 5, 25);
            self.wr(BASE + 6, (x >> 8) as u8);
            self.wr(BASE + 7, x as u8);
            self.wr(BASE + 8, (y >> 8) as u8);
            self.wr(BASE + 9, y as u8);
            self.wr(BASE + 2, width);
            self.wr(BASE + 3, height);
            self.wr(BASE + 0xA, 0);
            self.wr(BASE + 0xB, 0);
            self.wr(BASE + 0xC, 0);
        }

        /// A mode packed into the timing registers the firmware's way.
        #[allow(clippy::too_many_arguments)]
        fn mode(&mut self, ha: u16, hf: u16, hs: u16, hb: u16, va: u16, vf: u16, vs: u16, vb: u16) {
            let rep = (hs & 1) << 1 | (ha & 6) << 1 | (hb & 3) << 4 | (hf & 3) << 6;
            for (o, v) in [
                (1, hs >> 1),
                (2, hb >> 2),
                (3, ha >> 3),
                (4, hf >> 2),
                (5, rep),
                (9, vs),
                (0xA, vb),
                (0xB, va >> 3),
                (0xC, vf),
            ] {
                self.wr(TIMING + o, v as u8);
            }
        }

        fn pal_576p(&mut self) {
            self.mode(720, 12, 64, 68, 576, 5, 5, 39);
        }

        fn palette(&mut self) {
            for (i, rgb) in [(1, WHITE), (6, BLUE), (12, GREY)] {
                for (k, v) in rgb.into_iter().enumerate() {
                    self.wr(PALETTE + i * 4 + k as u32, v);
                }
            }
        }

        fn put(&mut self, row: u32, col: u32, text: &[u8], colour: u8) {
            for (i, &c) in text.iter().enumerate() {
                let at = row * 40 + col + i as u32;
                self.wr(SCREEN + at, c);
                self.wr(COLOUR + at, colour);
            }
        }

        fn frame(&self, width: usize, height: usize, font: &[u8]) -> Vec<u8> {
            let mut frame = PICTURE.repeat(width * height);
            self.ov.render(font, &mut frame, width, height);
            frame
        }
    }

    fn px(frame: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
        let at = (y * width + x) * 3;
        [frame[at], frame[at + 1], frame[at + 2]]
    }

    /// A font of the tests' own: 'A' is a mark in the top left corner and
    /// one at the bottom right, 'I' a vertical line, 'F' full.
    fn font() -> Vec<u8> {
        let mut f = vec![0u8; FONT_BYTES];
        f[0x41 * 8..0x41 * 8 + 8].copy_from_slice(&[0x80, 0, 0, 0, 0, 0, 0, 0x01]);
        f[0x49 * 8..0x49 * 8 + 8].copy_from_slice(&[0x18; 8]);
        f[0x46 * 8..0x46 * 8 + 8].copy_from_slice(&[0xFF; 8]);
        f
    }

    #[test]
    fn rams_read_back_and_registers_do_not() {
        let mut rig = Rig::new();
        // Power-on contents.
        assert_eq!(rig.rd(SCREEN), 0x20);
        assert_eq!(rig.rd(COLOUR + 0xFFF), 0x0F);
        for (i, a) in [0u32, 1, 999, 0xFFF].into_iter().enumerate() {
            rig.wr(SCREEN + a, 0x41 + i as u8);
            rig.wr(COLOUR + a, 0x61 + i as u8);
        }
        for (i, a) in [0u32, 1, 999, 0xFFF].into_iter().enumerate() {
            assert_eq!(rig.rd(SCREEN + a), 0x41 + i as u8);
            assert_eq!(rig.rd(COLOUR + a), 0x61 + i as u8);
            assert_eq!(rig.ov.peek(SCREEN + a), 0x41 + i as u8);
        }
        // The cursor: the firmware toggles bit 7 of what it reads.
        let c = rig.rd(SCREEN + 999);
        rig.wr(SCREEN + 999, c ^ 0x80);
        assert_eq!(rig.rd(SCREEN + 999), 0xC3);
        // Character generator registers: write only.
        rig.wr(BASE + 0xD, 0xC0);
        assert_eq!(rig.rd(BASE + 0xD), 0);
        assert_eq!(rig.ov.reg(0xD), 0xC0);
        // Palette, timing and cropper keep what is written.
        rig.wr(PALETTE + 0x3E, 0x99);
        assert_eq!(rig.rd(PALETTE + 0x3E), 0x99);
        assert_eq!(rig.ov.palette_rgb(15), [0, 0, 0x99]);
        rig.wr(TIMING + 0x13, 0x0C);
        assert_eq!(rig.rd(TIMING + 0x13), 0x0C);
        assert_eq!(rig.ov.timing_reg(0x13), 0x0C);
        // Around them: nothing, and nothing breaks.
        for a in [
            0x14_3000u32,
            0x14_4020,
            0x14_4FFF,
            0x14_5040,
            0x14_8004,
            0x14_F123,
            0x13_FFFF,
            0x15_0000,
        ] {
            rig.wr(a, 0xAA);
            assert_eq!(rig.rd(a), 0, "{a:#x}");
        }
        assert_eq!(rig.ov.next_event(), u64::MAX);
    }

    #[test]
    fn show_and_hide() {
        let mut rig = Rig::new();
        assert!(!rig.ov.active());
        rig.wr(BASE + 0xD, 0xC0);
        assert!(rig.ov.active());
        assert!(rig.ov.owns_keyboard());
        assert_eq!(rig.ov.transparent(), 0);
        rig.wr(BASE + 0xD, 0x00);
        assert!(!rig.ov.active());
        assert!(!rig.ov.owns_keyboard());
        // The registers repeat every sixteen bytes.
        rig.wr(BASE + 0x1D, 0x85);
        assert!(rig.ov.active());
        assert_eq!(rig.ov.transparent(), 5);
    }

    #[test]
    fn geometry_of_the_firmwares_modes() {
        let mut rig = Rig::new();
        assert_eq!(rig.ov.window(), None);
        rig.geometry(386, 307, 8, 0x09);
        assert_eq!(rig.ov.origin(), (386, 307));
        let w = rig.ov.window().unwrap();
        assert_eq!((w.cols, w.rows, w.width(), w.height()), (40, 25, 320, 225));
        rig.geometry(958, 160, 8, 0x90);
        let w = rig.ov.window().unwrap();
        assert_eq!((w.width(), w.height()), (320, 400));
        rig.geometry(1438, 240, 12, 0x5E);
        let w = rig.ov.window().unwrap();
        assert_eq!((w.cell_width, w.cell_height), (12, 23));
        assert_eq!((w.width(), w.height()), (480, 575));
        assert_eq!(rig.ov.origin(), (1438, 240));
    }

    #[test]
    fn timing_is_unpacked() {
        let mut rig = Rig::new();
        assert_eq!(rig.ov.timing(), None);
        assert_eq!(rig.ov.canvas_size(), None);
        assert_eq!(rig.ov.window_position(), None);
        rig.pal_576p();
        rig.geometry(386, 307, 8, 0x09);
        assert_eq!(rig.ov.canvas_size(), Some((720, 576)));
        assert_eq!(rig.ov.window_position(), Some((254, 263)));
        // 1080p.
        rig.mode(1920, 528, 44, 148, 1080, 4, 5, 36);
        rig.geometry(1438, 240, 12, 0x5E);
        assert_eq!(rig.ov.canvas_size(), Some((1920, 1080)));
        assert_eq!(rig.ov.window_position(), Some((1246, 199)));
        // The low bits that travel in the repetition register.
        rig.mode(726, 13, 63, 70, 480, 9, 6, 31);
        assert_eq!(
            rig.ov.timing(),
            Some(HdmiTiming {
                h_active: 726,
                h_front_porch: 13,
                h_sync: 63,
                h_back_porch: 70,
                v_active: 480,
                v_front_porch: 9,
                v_sync: 6,
                v_back_porch: 31,
            })
        );
    }

    #[test]
    fn cropper() {
        let mut rig = Rig::new();
        for (i, v) in [8u8, 9, 192, 135].into_iter().enumerate() {
            rig.wr(CROPPER + i as u32, v);
        }
        assert_eq!(
            rig.ov.vic_crop(),
            VicCrop {
                x: 8,
                y: 9,
                width: 384,
                height: 270
            }
        );
        assert_eq!(rig.rd(CROPPER + 2), 192);
    }

    #[test]
    fn text_of_a_written_screen() {
        let mut rig = Rig::new();
        rig.geometry(386, 307, 8, 0x09);
        rig.put(0, 12, b"*** Ultimate ***", 0x0F);
        rig.put(1, 0, &[0x02; 40], 0x0C);
        // A box, a reversed character (a cursor), a code without a
        // picture.
        rig.put(3, 2, &[0x01, 0x02, 0x03], 0x0C);
        rig.put(
            4,
            2,
            &[0x04, b'x' | 0x80, 0x04, 0x00, 0x13, 0x1F, 0x0B],
            0x0C,
        );
        rig.put(5, 2, &[0x05, 0x02, 0x06], 0x0C);
        rig.put(24, 37, b"end", 0x0C);
        let text = rig.ov.text();
        assert_eq!(text.len(), 25);
        assert_eq!(text[0], "            *** Ultimate ***");
        assert_eq!(text[1], "-".repeat(40));
        assert_eq!(text[2], "");
        assert_eq!(text[3], "  +-+");
        assert_eq!(text[4], "  |x| *.#");
        assert_eq!(text[5], "  +-+");
        assert_eq!(text[24], format!("{}end", " ".repeat(37)));
        // Hidden or shown makes no difference to the text.
        rig.wr(BASE + 0xD, 0xC0);
        assert_eq!(rig.ov.text(), text);
        // The pointer moves the window through the RAM.
        rig.wr(BASE + 0xB, 40);
        assert_eq!(rig.ov.text()[0], "-".repeat(40));
        // No geometry: no rows.
        assert!(Overlay::new().text().is_empty());
    }

    #[test]
    fn a_glyph_lands_where_the_registers_put_it() {
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.geometry(386, 307, 8, 0x09);
        // 'A' in white on blue at row 2, column 3; around it the cleared
        // screen: spaces, colour $0F, background 0 = transparent.
        rig.put(2, 3, b"A", 0x61);
        let font = font();

        // Hidden: the picture stays as it is.
        let f = rig.frame(720, 576, &font);
        assert!(f.chunks(3).all(|p| p == PICTURE));

        rig.wr(BASE + 0xD, 0xC0);
        let f = rig.frame(720, 576, &font);
        // The cell: x 254 + 3*8, y 263 + 2*9, 8 x 9 pixels.
        let (cx, cy) = (254 + 24, 263 + 18);
        assert_eq!(px(&f, 720, cx, cy), WHITE, "top left pixel of the glyph");
        assert_eq!(px(&f, 720, cx + 1, cy), BLUE);
        assert_eq!(px(&f, 720, cx, cy + 1), BLUE);
        assert_eq!(px(&f, 720, cx + 7, cy + 7), WHITE, "bottom right pixel");
        assert_eq!(px(&f, 720, cx + 6, cy + 7), BLUE);
        // The ninth line of the cell is background.
        for x in 0..8 {
            assert_eq!(px(&f, 720, cx + x, cy + 8), BLUE);
        }
        // Exactly 8 x 9 pixels were drawn: everything else shows through.
        let drawn = f.chunks(3).filter(|p| *p != PICTURE).count();
        assert_eq!(drawn, 72);
        assert_eq!(px(&f, 720, cx - 1, cy), PICTURE);
        assert_eq!(px(&f, 720, cx + 8, cy), PICTURE);
        assert_eq!(px(&f, 720, cx, cy - 1), PICTURE);
        assert_eq!(px(&f, 720, cx, cy + 9), PICTURE);

        // Reverse: foreground and background change places.
        rig.put(2, 3, &[b'A' | 0x80], 0x61);
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, cx, cy), BLUE);
        assert_eq!(px(&f, 720, cx + 1, cy), WHITE);
        assert_eq!(
            px(&f, 720, cx + 3, cy + 8),
            WHITE,
            "blank ninth line, reversed"
        );

        // Another transparent colour: blue goes, the black of colour 0
        // comes (the whole window is solid now).
        rig.put(2, 3, b"A", 0x61);
        rig.wr(BASE + 0xD, 0x86);
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, cx, cy), WHITE);
        assert_eq!(px(&f, 720, cx + 1, cy), PICTURE);
        assert_eq!(px(&f, 720, 254, 263), [0, 0, 0]);
        assert_eq!(px(&f, 720, 254 + 319, 263 + 224), [0, 0, 0]);
        assert_eq!(px(&f, 720, 253, 263), PICTURE);
        assert_eq!(px(&f, 720, 254 + 320, 263), PICTURE);
        assert_eq!(px(&f, 720, 254, 263 + 225), PICTURE);
    }

    #[test]
    fn a_vertical_line_runs_through_the_ninth_line() {
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.geometry(386, 307, 8, 0x09);
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(0, 0, b"IF", 0x01);
        let f = rig.frame(720, 576, &font());
        for y in 0..9 {
            assert_eq!(px(&f, 720, 254 + 3, 263 + y), WHITE, "line {y}");
            assert_eq!(px(&f, 720, 254 + 4, 263 + y), WHITE);
            assert_eq!(px(&f, 720, 254 + 2, 263 + y), PICTURE);
        }
        // A full block does not: its ninth line is blank.
        assert_eq!(px(&f, 720, 254 + 8, 263 + 7), WHITE);
        assert_eq!(px(&f, 720, 254 + 8, 263 + 8), PICTURE);
    }

    #[test]
    fn doubled_lines_and_big_cells() {
        let font = font();
        let mut rig = Rig::new();
        rig.palette();
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(1, 1, b"A", 0x61);

        // 720p: 8 x 16, every font line twice.
        rig.mode(1280, 440, 40, 220, 720, 5, 5, 20);
        rig.geometry(958, 160, 8, 0x90);
        assert_eq!(rig.ov.window_position(), Some((698, 135)));
        let f = rig.frame(1280, 720, &font);
        let (cx, cy) = (698 + 8, 135 + 16);
        assert_eq!(px(&f, 1280, cx, cy), WHITE);
        assert_eq!(px(&f, 1280, cx, cy + 1), WHITE);
        assert_eq!(px(&f, 1280, cx, cy + 2), BLUE);
        assert_eq!(px(&f, 1280, cx + 7, cy + 14), WHITE);
        assert_eq!(px(&f, 1280, cx + 7, cy + 15), WHITE);
        assert_eq!(px(&f, 1280, cx + 7, cy + 16), PICTURE);
        assert_eq!(f.chunks(3).filter(|p| *p != PICTURE).count(), 8 * 16);

        // 1080p: 12 x 23; the 8 x 8 font stands in, a pixel becomes one
        // or two across and three lines (two in the last font line).
        rig.mode(1920, 528, 44, 148, 1080, 4, 5, 36);
        rig.geometry(1438, 240, 12, 0x5E);
        let f = rig.frame(1920, 1080, &font);
        let (cx, cy) = (1246 + 12, 199 + 23);
        assert_eq!(f.chunks(3).filter(|p| *p != PICTURE).count(), 12 * 23);
        for y in 0..3 {
            assert_eq!(px(&f, 1920, cx, cy + y), WHITE);
            assert_eq!(px(&f, 1920, cx + 1, cy + y), WHITE);
            assert_eq!(px(&f, 1920, cx + 2, cy + y), BLUE);
        }
        assert_eq!(px(&f, 1920, cx, cy + 3), BLUE);
        assert_eq!(px(&f, 1920, cx + 11, cy + 20), BLUE);
        assert_eq!(px(&f, 1920, cx + 11, cy + 21), WHITE);
        assert_eq!(px(&f, 1920, cx + 11, cy + 22), WHITE);
        assert_eq!(px(&f, 1920, cx + 10, cy + 22), BLUE);
        assert_eq!(px(&f, 1920, cx + 11, cy + 23), PICTURE);
    }

    #[test]
    fn a_frame_of_another_size_is_the_stretched_picture() {
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.geometry(386, 307, 8, 0x09);
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(0, 0, b"F", 0x61);
        let font = font();
        // Half the size: frame pixel (x, y) is HDMI pixel (2x, 2y). The
        // cell covers 254..262 x 263..272, the full glyph lines 263..271.
        let f = rig.frame(360, 288, &font);
        assert_eq!(px(&f, 360, 126, 132), PICTURE);
        assert_eq!(px(&f, 360, 127, 132), WHITE);
        assert_eq!(px(&f, 360, 130, 132), WHITE);
        assert_eq!(px(&f, 360, 131, 132), PICTURE);
        assert_eq!(px(&f, 360, 127, 131), PICTURE);
        assert_eq!(px(&f, 360, 127, 135), WHITE);
        assert_eq!(px(&f, 360, 127, 136), PICTURE);
        // The C64's own frame size works as well (no panic, something
        // drawn inside the frame).
        let f = rig.frame(384, 272, &font);
        assert!(f.chunks(3).any(|p| p == WHITE));
        // Twice the size.
        let f = rig.frame(1440, 1152, &font);
        assert_eq!(px(&f, 1440, 508, 526), WHITE);
        assert_eq!(px(&f, 1440, 507, 526), PICTURE);
        assert_eq!(px(&f, 1440, 523, 541), WHITE);
        assert_eq!(px(&f, 1440, 524, 541), PICTURE);
    }

    #[test]
    fn without_a_timing_the_window_is_centred() {
        let mut rig = Rig::new();
        rig.palette();
        rig.geometry(300, 235, 8, 0x09);
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(0, 0, b"F", 0x61);
        rig.put(24, 39, b"F", 0x61);
        let font = font();
        // 384 x 272: the 320 x 225 window starts at (32, 23).
        let f = rig.frame(384, 272, &font);
        assert_eq!(px(&f, 384, 32, 23), WHITE);
        assert_eq!(px(&f, 384, 31, 23), PICTURE);
        assert_eq!(px(&f, 384, 32, 22), PICTURE);
        assert_eq!(px(&f, 384, 32 + 319, 23 + 223), WHITE);
        assert_eq!(px(&f, 384, 32 + 319, 23 + 224), BLUE);
        assert_eq!(px(&f, 384, 32 + 320, 23 + 223), PICTURE);
        // A frame smaller than the window: the window fills it.
        let f = rig.frame(160, 100, &font);
        assert_eq!(px(&f, 160, 0, 0), WHITE);
        assert_eq!(px(&f, 160, 159, 99), WHITE);
    }

    #[test]
    fn windows_off_the_picture_are_clipped() {
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.wr(BASE + 0xD, 0xC0);
        let font = font();
        for i in 0..1000 {
            rig.wr(SCREEN + i, b'F');
            rig.wr(COLOUR + i, 0x61);
        }
        // Past the right and bottom edges.
        rig.geometry(132 + 700, 44 + 570, 8, 0x09);
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, 700, 570), WHITE);
        assert_eq!(px(&f, 720, 699, 570), PICTURE);
        assert_eq!(px(&f, 720, 719, 575), WHITE);
        // Left of and above the picture (X_ON inside the sync).
        rig.geometry(100, 10, 8, 0x09);
        assert_eq!(rig.ov.window_position(), Some((-32, -34)));
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, 0, 0), WHITE);
        assert_eq!(px(&f, 720, 287, 0), WHITE);
        assert_eq!(px(&f, 720, 288, 0), PICTURE);
        // Wholly outside.
        rig.geometry(4000, 4000, 8, 0x09);
        let f = rig.frame(720, 576, &font);
        assert!(f.chunks(3).all(|p| p == PICTURE));
    }

    #[test]
    fn odd_register_values_and_arguments_do_not_panic() {
        let font = font();
        for (cols, rows, width, height, ptr) in [
            (0u8, 25u8, 8u8, 9u8, 0u8),
            (40, 0, 8, 9, 0),
            (40, 25, 0, 9, 0),
            (40, 25, 8, 0, 0),
            (255, 63, 15, 0x1F, 0x7F),
            (255, 63, 15, 0xDF, 0xFF),
            (200, 63, 1, 0x41, 0x0F),
            (40, 25, 12, 0x09, 0),
            (40, 25, 5, 0x1F, 0),
        ] {
            let mut rig = Rig::new();
            rig.palette();
            rig.geometry(0, 0, width, height);
            rig.wr(BASE + 4, cols);
            rig.wr(BASE + 5, rows);
            rig.wr(BASE + 0xA, ptr);
            rig.wr(BASE + 0xB, 0xF0);
            rig.wr(BASE + 0xD, 0xC1);
            let _ = rig.ov.text();
            for (w, h) in [(384, 272), (1, 1), (0, 0), (7, 300)] {
                let _ = rig.frame(w, h, &font);
            }
            rig.pal_576p();
            let _ = rig.frame(720, 576, &font);
            let _ = rig.frame(384, 272, &[]);
            // A slice that is too short is left alone.
            let mut short = vec![9u8; 100];
            rig.ov.render(&font, &mut short, 384, 272);
            rig.ov.render(&font, &mut short, usize::MAX, usize::MAX);
            assert!(short.iter().all(|&b| b == 9));
        }
    }

    #[test]
    fn narrow_and_wide_cells_take_the_low_bits() {
        // A cell wider than the font: the glyph sits at its right; a
        // narrower one shows the glyph's right part.
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(0, 0, b"A", 0x61);
        let font = font();
        rig.geometry(132, 44, 12, 0x09);
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, 0, 0), BLUE);
        assert_eq!(px(&f, 720, 4, 0), WHITE);
        assert_eq!(px(&f, 720, 11, 7), WHITE);
        rig.geometry(132, 44, 4, 0x09);
        let f = rig.frame(720, 576, &font);
        assert_eq!(px(&f, 720, 0, 0), BLUE);
        assert_eq!(px(&f, 720, 3, 7), WHITE);
    }

    /// A stand-in for the C64's character ROM: every character's eight
    /// bytes are its set (bit 7) and screen code, so the test sees which
    /// one was picked.
    fn marked_chargen() -> Vec<u8> {
        (0..4096u32)
            .map(|i| ((i / 0x800) << 7 | (i / 8) & 0x7F) as u8)
            .collect()
    }

    #[test]
    fn font_from_the_c64_rom() {
        let font = font_from_c64_chargen(&marked_chargen());
        assert_eq!(font.len(), FONT_BYTES);
        let glyph = |c: u8| &font[usize::from(c) * 8..usize::from(c) * 8 + 8];
        let from = |set: u8, code: u8| [set << 7 | code; 8];
        assert_eq!(glyph(b' '), from(1, 0x20));
        assert_eq!(glyph(b'0'), from(1, 0x30));
        assert_eq!(glyph(b'?'), from(1, 0x3F));
        assert_eq!(glyph(b'@'), from(1, 0x00));
        assert_eq!(glyph(b'A'), from(1, 0x41));
        assert_eq!(glyph(b'Z'), from(1, 0x5A));
        assert_eq!(glyph(b'a'), from(1, 0x01));
        assert_eq!(glyph(b'z'), from(1, 0x1A));
        assert_eq!(glyph(b'['), from(1, 0x1B));
        assert_eq!(glyph(b']'), from(1, 0x1D));
        // Line graphics from the first set.
        assert_eq!(glyph(0x02), from(0, 0x40));
        assert_eq!(glyph(0x04), from(0, 0x5D));
        assert_eq!(glyph(0x01), from(0, 0x70));
        assert_eq!(glyph(0x06), from(0, 0x7D));
        assert_eq!(glyph(0x07), from(0, 0x55));
        assert_eq!(glyph(0x13), from(0, 0x5A));
        // Drawn here.
        assert_eq!(glyph(0x0B), [0, 255, 255, 255, 255, 255, 255, 255]);
        assert_eq!(glyph(b'|')[..7], [0x18; 7]);
        assert_ne!(glyph(b'\\'), [0; 8]);
        assert_ne!(glyph(b'~'), [0; 8]);
        // Blank: code 0 and the logo pieces.
        assert_eq!(glyph(0x00), [0; 8]);
        for c in 0x14..0x20 {
            assert_eq!(glyph(c), [0; 8]);
        }
        // Every ASCII character but the space has a picture.
        for c in 0x21..0x7Fu8 {
            assert_ne!(glyph(c), [0; 8], "{c:#x}");
        }
        // A short or missing ROM: blanks, no panic.
        let half = font_from_c64_chargen(&marked_chargen()[..0x800]);
        assert_eq!(half.len(), FONT_BYTES);
        assert_eq!(half[0x41 * 8], 0);
        assert_eq!(half[0x02 * 8], 0x40);
        assert_eq!(font_from_c64_chargen(&[]).len(), FONT_BYTES);
    }

    fn busy() -> Rig {
        let mut rig = Rig::new();
        rig.pal_576p();
        rig.palette();
        rig.geometry(386, 307, 8, 0x09);
        rig.wr(BASE + 0xD, 0xC0);
        rig.put(3, 5, b"Savestate", 0x61);
        rig.wr(SCREEN + 0xFFF, 0x55);
        rig.wr(COLOUR + 0xFFF, 0xAA);
        rig.wr(CROPPER + 3, 135);
        rig.wr(TIMING + 0x1F, 0x77);
        rig
    }

    #[test]
    fn savestate_round_trip() {
        let rig = busy();
        let mut w = StateWriter::new();
        rig.ov.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = Overlay::new();
        let mut r = StateReader::new(&bytes);
        back.load(&mut r).unwrap();
        assert!(r.is_at_end());
        assert_eq!(back, rig.ov);
        assert!(back.active());
        assert_eq!(back.text()[3], "     Savestate");
        let font = font();
        let mut a = PICTURE.repeat(720 * 576);
        let mut b = a.clone();
        rig.ov.render(&font, &mut a, 720, 576);
        back.render(&font, &mut b, 720, 576);
        assert_eq!(a, b);
        // Reset: the power-on state.
        back.reset();
        assert_eq!(back, Overlay::new());
    }

    #[test]
    fn savestate_truncated() {
        let rig = busy();
        let mut w = StateWriter::new();
        rig.ov.save(&mut w);
        let bytes = w.into_bytes();
        for n in (0..bytes.len()).step_by(61).chain([bytes.len() - 1]) {
            let mut back = Overlay::new();
            assert!(
                back.load(&mut StateReader::new(&bytes[..n])).is_err(),
                "{n}"
            );
            assert_eq!(back, Overlay::new(), "a failed load leaves the block alone");
        }
    }
}
