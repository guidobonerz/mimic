//! The serial terminal on the console port: 80 × 24 characters with the
//! ADM-3A's control codes. The IMSAI has no screen of its own; this is
//! what stood next to it, shown as the machine's second output.
//!
//! Codes: BS, LF, CR, ^K cursor up, ^L right, ^Z clear, ^^ home, ESC = row
//! column (each + $20). Bit 7 of a byte is ignored (Altair and IMSAI BASIC
//! use it in their strings). Source: Lear Siegler, "ADM-3A Operator's
//! Manual".

use emu_core::font;
use emu_core::frame::{FieldParity, OutputType, Rgb8, SignalInfo, VideoStandard};
use emu_core::{StateError, StateReader, StateWriter};

pub const COLS: usize = 80;
pub const ROWS: usize = 24;
const CELL_W: usize = 8;
const CELL_H: usize = 16;
pub const WIDTH: u32 = (COLS * CELL_W) as u32;
pub const HEIGHT: u32 = (ROWS * CELL_H) as u32;

pub const PALETTE: [Rgb8; 2] = [
    Rgb8 {
        r: 0x08,
        g: 0x0c,
        b: 0x0a,
    },
    Rgb8 {
        r: 0xd8,
        g: 0xe8,
        b: 0xdc,
    },
];

/// The picture's signal metadata (progressive, `fps` fields a second).
pub fn signal(fps: u32) -> SignalInfo {
    SignalInfo {
        pixel_clock_hz: WIDTH * HEIGHT * fps,
        active_pixels: WIDTH,
        total_pixels: WIDTH,
        lines_per_field: HEIGHT,
        standard: VideoStandard::Ntsc,
        output: OutputType::Rgb,
        field: FieldParity::Progressive,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Esc {
    None,
    /// ESC seen.
    Start,
    /// ESC = seen.
    Row,
    /// ESC = row seen.
    Col(u8),
}

#[derive(Clone, Debug)]
pub struct Terminal {
    text: Vec<u8>,
    pub col: usize,
    pub row: usize,
    esc: Esc,
    /// The picture; derived from the text.
    pixels: Vec<u8>,
    dirty: bool,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            text: vec![b' '; COLS * ROWS],
            col: 0,
            row: 0,
            esc: Esc::None,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
            dirty: true,
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Row `row` of the screen as text, without trailing blanks.
    pub fn line(&self, row: usize) -> String {
        let s = &self.text[row * COLS..(row + 1) * COLS];
        String::from_utf8_lossy(s).trim_end().to_string()
    }

    fn scroll(&mut self) {
        self.text.copy_within(COLS.., 0);
        self.text[COLS * (ROWS - 1)..].fill(b' ');
    }

    fn line_feed(&mut self) {
        if self.row + 1 < ROWS {
            self.row += 1;
        } else {
            self.scroll();
        }
    }

    /// A byte from the computer.
    pub fn put(&mut self, byte: u8) {
        let b = byte & 0x7f;
        self.dirty = true;
        match self.esc {
            Esc::Start => {
                self.esc = if b == b'=' { Esc::Row } else { Esc::None };
                return;
            }
            Esc::Row => {
                self.esc = Esc::Col(b);
                return;
            }
            Esc::Col(r) => {
                self.esc = Esc::None;
                self.row = usize::from(r.saturating_sub(0x20)).min(ROWS - 1);
                self.col = usize::from(b.saturating_sub(0x20)).min(COLS - 1);
                return;
            }
            Esc::None => {}
        }
        match b {
            0x08 => self.col = self.col.saturating_sub(1),
            0x0a => self.line_feed(),
            0x0b => self.row = self.row.saturating_sub(1),
            0x0c => self.col = (self.col + 1).min(COLS - 1),
            0x0d => self.col = 0,
            0x1a => {
                self.text.fill(b' ');
                self.col = 0;
                self.row = 0;
            }
            0x1b => self.esc = Esc::Start,
            0x1e => {
                self.col = 0;
                self.row = 0;
            }
            0x20..=0x7e => {
                self.text[self.row * COLS + self.col] = b;
                self.col += 1;
                if self.col == COLS {
                    self.col = 0;
                    self.line_feed();
                }
            }
            _ => {}
        }
    }

    /// Redraw if something changed; `cursor` shows the cursor block.
    pub fn render(&mut self, cursor: bool) {
        let at = self.row * COLS + self.col;
        if !self.dirty && self.pixels_cursor(at) == cursor {
            return;
        }
        self.dirty = false;
        let w = WIDTH as usize;
        for row in 0..ROWS {
            for col in 0..COLS {
                let ch = self.text[row * COLS + col];
                let inverse = cursor && row == self.row && col == self.col;
                for y in 0..CELL_H {
                    let g = font::row(ch, (y.saturating_sub(1) / 2) as u8);
                    let dots = if y == 0 || y >= 15 { 0 } else { g };
                    let at = (row * CELL_H + y) * w + col * CELL_W;
                    for x in 0..CELL_W {
                        let on = (1..=5).contains(&x) && dots & (1 << (6 - x)) != 0;
                        self.pixels[at + x] = u8::from(on != inverse);
                    }
                }
            }
        }
    }

    /// The cursor cell's corner pixel is lit only by the cursor block.
    fn pixels_cursor(&self, at: usize) -> bool {
        let (row, col) = (at / COLS, at % COLS);
        self.pixels[row * CELL_H * WIDTH as usize + col * CELL_W] != 0
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.bytes(&self.text);
        w.u16(self.col as u16);
        w.u16(self.row as u16);
        let (kind, arg) = match self.esc {
            Esc::None => (0, 0),
            Esc::Start => (1, 0),
            Esc::Row => (2, 0),
            Esc::Col(r) => (3, r),
        };
        w.u8(kind);
        w.u8(arg);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let text = r.bytes()?;
        if text.len() != COLS * ROWS {
            return Err(StateError::Invalid("terminal size"));
        }
        let (col, row) = (usize::from(r.u16()?), usize::from(r.u16()?));
        if col >= COLS || row >= ROWS {
            return Err(StateError::Invalid("terminal cursor"));
        }
        let kind = r.u8()?;
        let arg = r.u8()?;
        self.esc = match kind {
            1 => Esc::Start,
            2 => Esc::Row,
            3 => Esc::Col(arg),
            _ => Esc::None,
        };
        self.text = text.to_vec();
        self.col = col;
        self.row = row;
        self.dirty = true;
        Ok(())
    }
}
