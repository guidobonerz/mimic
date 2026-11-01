//! The dot matrix printers: the print head on a page of 8 × 11 inches and
//! the control codes of each [`Model`].
//!
//! - **Epson FX-80** and its many compatibles, 9 pins: the common part of
//!   ESC/P (Epson, "FX Series Printer User's Manual", 1983): text in pica,
//!   elite and condensed, wide, bold and underlined, line spacing in
//!   1/216 inch, bit-image graphics in columns of eight pins 1/72 inch
//!   apart, top pin in bit 7.
//! - **NEC Pinwriter P6+/P7+**, 24 pins, 360 dpi: the same codes with the
//!   24-pin steps (`ESC 3` in 1/180, `ESC A` in 1/60 inch), `ESC *` with
//!   the three-byte column modes, `ESC +` and `FS 3` in 1/360 inch. No code
//!   table of the printer was at hand: the codes are the 24-pin extension of
//!   ESC/P as on Epson's LQ series, `FS 3` is from the notes of a P6+
//!   printer driver (RISC OS 3.6, `Printers/NEC/Read_Me`). Other `FS`
//!   codes are ignored, without parameters. The real head cannot print
//!   adjacent dots at 360 dpi across; this one can.
//! - **Apple ImageWriter**, the Macintosh's serial printer, 9 pins with
//!   Apple's own codes (Apple, "ImageWriter User's Manual"; written down
//!   from memory, then seen in use from the System 6 driver): pitches `ESC n N E e q Q p P`, graphics
//!   `ESC G nnnn` with the count in decimal digits and the top pin in
//!   bit 0, `ESC T nn` line spacing in 1/144 inch.
//!
//! The picture has the printer's finest steps. The characters come from
//! Noto Sans Mono, not from a printer's ROM. Codes it does not know are
//! skipped with their parameters where those are known, so a stream still
//! prints.

use emu_core::{StateError, StateReader, StateWriter};
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

const ESC: u8 = 0x1b;
const FS: u8 = 0x1c;
const SOURCE: RasterHeight = RasterHeight::Size24;
/// Page size in tenths of an inch.
const PAGE: (usize, usize) = (80, 110);

/// A printer model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Model {
    #[default]
    Fx80,
    P6,
    ImageWriter,
}

impl Model {
    pub fn from_option(v: Option<&str>) -> Option<Self> {
        match v {
            Some("fx80" | "epson") => Some(Self::Fx80),
            Some("p6" | "p6+" | "p6plus" | "p7" | "nec") => Some(Self::P6),
            Some("imagewriter") => Some(Self::ImageWriter),
            _ => None,
        }
    }

    /// Dots an inch across and down.
    fn dpi(self) -> (usize, usize) {
        match self {
            Self::Fx80 => (240, 216),
            Self::P6 => (360, 360),
            // Across, a multiple of the pitches' dots an inch.
            Self::ImageWriter => (720, 144),
        }
    }

    /// Dots a unit of `ESC 3`/`ESC J` covers, those of a unit of `ESC A`,
    /// and the distance of the pins in an eight-pin column.
    fn steps(self) -> (usize, usize, usize) {
        match self {
            Self::Fx80 => (1, 3, 3),
            Self::P6 => (2, 6, 6),
            Self::ImageWriter => (1, 1, 2),
        }
    }

    /// The bytes that print a 8 × 8 picture (`rows` top first, MSB left)
    /// in a character cell of ten to the inch: 80 columns an inch on the
    /// Epson, 90 with a blank one on the NEC, ESC N's pitch on the
    /// ImageWriter.
    pub fn glyph_bytes(self, rows: &[u8]) -> Vec<u8> {
        let column = |c: usize, top_is_msb: bool| {
            rows.iter().take(8).enumerate().fold(0u8, |v, (r, row)| {
                let on = row & (0x80 >> c) != 0;
                let pin = if top_is_msb { 0x80 >> r } else { 1 << r };
                if on {
                    v | pin
                } else {
                    v
                }
            })
        };
        let mut out = match self {
            Self::Fx80 => vec![ESC, b'*', 4, 8, 0],
            Self::P6 => vec![ESC, b'*', 6, 9, 0],
            Self::ImageWriter => vec![ESC, b'G', b'0', b'0', b'0', b'8'],
        };
        out.extend((0..8).map(|c| column(c, self != Self::ImageWriter)));
        if self == Self::P6 {
            out.push(0);
        }
        out
    }

    fn from_code(c: u8) -> Option<Self> {
        [Self::Fx80, Self::P6, Self::ImageWriter]
            .get(usize::from(c))
            .copied()
    }
}

/// Where the parser stands in a control sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seq {
    Text,
    Esc,
    Fs,
    /// `left` parameter bytes of `cmd` to come.
    Params {
        cmd: u8,
        left: u8,
        first: u8,
    },
    /// Skip up to and including a zero byte.
    UntilZero,
    /// `left` columns to come, `step` dots wide, of `bytes` bytes each; 0
    /// bytes: an ImageWriter column.
    Graphics {
        left: u16,
        step: u8,
        bytes: u8,
    },
    /// ImageWriter: decimal digits of `cmd`, `left` to come.
    Digits {
        cmd: u8,
        left: u8,
        value: u16,
    },
    /// ImageWriter `ESC V`: the byte to repeat `count` times.
    Repeat {
        count: u16,
    },
}

/// Parameters of an ESC/P code; `None`: it ends with a zero byte.
fn params(cmd: u8) -> Option<u8> {
    match cmd {
        b'B' | b'D' => None,
        b'K' | b'L' | b'Y' | b'Z' | b'$' => Some(2),
        b'*' => Some(3),
        b'-' | b'W' | b'3' | b'A' | b'J' | b'+' | b'C' | b'l' | b'Q' | b'N' | b'R' | b'S'
        | b'x' | b'!' | b'U' | b't' | b'k' | b'p' => Some(1),
        _ => Some(0),
    }
}

/// Columns an inch and bytes a column in mode `m` of `ESC *`.
fn star_mode(m: u8) -> (usize, u8) {
    match m {
        1 | 2 => (120, 1),
        3 => (240, 1),
        4 => (80, 1),
        5 => (72, 1),
        6 => (90, 1),
        32 => (60, 3),
        33 => (120, 3),
        38 => (90, 3),
        39 => (180, 3),
        40 => (360, 3),
        71 => (180, 6),
        72 => (360, 6),
        _ => (60, 1),
    }
}

pub struct Epson {
    model: Model,
    width: usize,
    height: usize,
    stride: usize,
    /// One bit a dot, MSB left; empty while the page is blank.
    page: Vec<u8>,
    x: usize,
    y: usize,
    spacing: usize,
    /// Dots a character advances, before wide.
    cell: usize,
    wide: bool,
    /// Wide until the end of the line (SO).
    wide_line: bool,
    /// ImageWriter: line feeds move the paper back (`ESC r`).
    reverse: bool,
    bold: bool,
    underline: bool,
    seq: Seq,
    /// Bytes of the graphics column under way.
    column: [u8; 6],
    got: u8,
    /// ImageWriter: dots of a graphics column at the current pitch.
    pitch_step: u8,
    /// A page that left the printer, for the owner to take.
    pub finished: Option<Vec<u8>>,
}

impl Default for Epson {
    fn default() -> Self {
        Self::new()
    }
}

impl Epson {
    pub fn new() -> Self {
        Self::with_model(Model::Fx80)
    }

    pub fn with_model(model: Model) -> Self {
        let (dx, dy) = model.dpi();
        let (width, height) = (dx * PAGE.0 / 10, dy * PAGE.1 / 10);
        Self {
            model,
            width,
            height,
            stride: width / 8,
            page: Vec::new(),
            x: 0,
            y: 0,
            spacing: dy / 6,
            cell: dx / 10,
            wide: false,
            wide_line: false,
            reverse: false,
            bold: false,
            underline: false,
            seq: Seq::Text,
            column: [0; 6],
            got: 0,
            pitch_step: 9,
            finished: None,
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    /// How often a picture repeats each row for square dots.
    pub fn row_repeat(&self) -> usize {
        let (dx, dy) = self.model.dpi();
        (dx / dy).max(1)
    }

    /// The page's size in dots.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// The page in the printer, if anything is on it.
    pub fn page(&self) -> Option<&[u8]> {
        (!self.page.is_empty()).then_some(self.page.as_slice())
    }

    /// Is the dot at (`x`, `y`) printed?
    pub fn dot(&self, x: usize, y: usize) -> bool {
        self.page
            .get(y * self.stride + x / 8)
            .is_some_and(|b| b & (0x80 >> (x % 8)) != 0)
    }

    fn set(&mut self, x: usize, y: usize) {
        if x >= self.width || y >= self.height {
            return;
        }
        if self.page.is_empty() {
            self.page = vec![0; self.stride * self.height];
        }
        self.page[y * self.stride + x / 8] |= 0x80 >> (x % 8);
    }

    /// The page leaves the printer: [`Epson::finished`] holds it.
    pub fn eject(&mut self) {
        if !self.page.is_empty() {
            self.finished = Some(std::mem::take(&mut self.page));
        }
        self.x = 0;
        self.y = 0;
    }

    fn glyph_height(&self) -> usize {
        self.model.dpi().1 / 8
    }

    fn feed_paper(&mut self, dots: usize) {
        self.y += dots;
        if self.y + self.glyph_height() > self.height {
            self.eject();
        }
    }

    fn advance(&self) -> usize {
        if self.wide || self.wide_line {
            self.cell * 2
        } else {
            self.cell
        }
    }

    fn newline(&mut self) {
        self.x = 0;
        self.wide_line = false;
        self.feed_paper(self.spacing);
    }

    fn glyph(&mut self, c: char) {
        let advance = self.advance();
        if self.x + advance > self.width {
            self.newline();
        }
        let (x0, y0) = (self.x, self.y);
        // The source glyph, stretched into the cell by nearest neighbour.
        let target = advance * 5 / 6;
        let height = self.glyph_height();
        let source_width = get_raster_width(FontWeight::Regular, SOURCE);
        let raster = get_raster(c, FontWeight::Regular, SOURCE)
            .or_else(|| get_raster('?', FontWeight::Regular, SOURCE));
        if let Some(raster) = raster {
            let rows = raster.raster();
            let strike = if self.bold { 2 } else { 1 };
            for dy in 0..height {
                let row = &rows[dy * rows.len() / height];
                for dx in 0..target {
                    if row[dx * source_width / target] >= 0x80 {
                        for s in 0..strike {
                            self.set(x0 + dx + s, y0 + dy);
                        }
                    }
                }
            }
        }
        if self.underline {
            for dx in 0..advance {
                self.set(x0 + dx, y0 + height + 1);
            }
        }
        self.x += advance;
    }

    /// A column of `pins` pins `pin_step` dots apart; pin `p` fires when
    /// `fire(p)` says so.
    fn pins(&mut self, pins: usize, pin_step: usize, step: u8, fire: impl Fn(usize) -> bool) {
        let step = usize::from(step);
        for pin in 0..pins {
            if fire(pin) {
                for dy in 0..pin_step {
                    for dx in 0..step {
                        self.set(self.x + dx, self.y + pin * pin_step + dy);
                    }
                }
            }
        }
        self.x += step;
    }

    /// The bytes of a column are in: top pin first, MSB first.
    fn column_done(&mut self, step: u8, bytes: u8) {
        let column = self.column;
        let bit = |p: usize| column[p / 8] & (0x80 >> (p % 8)) != 0;
        match bytes {
            1 => self.pins(8, self.model.steps().2, step, bit),
            // 24 pins, 1/180 inch apart.
            3 => self.pins(24, (self.model.dpi().1 / 180).max(1), step, bit),
            // Two passes of 24 pins, 1/360 inch apart.
            _ => self.pins(48, (self.model.dpi().1 / 360).max(1), step, bit),
        }
    }

    fn text(&mut self, b: u8) {
        match b {
            ESC => self.seq = Seq::Esc,
            FS if self.model == Model::P6 => self.seq = Seq::Fs,
            0x0d => self.x = 0,
            0x0a if self.reverse => self.y = self.y.saturating_sub(self.spacing),
            0x0a => {
                self.wide_line = false;
                self.feed_paper(self.spacing);
            }
            0x0c => self.eject(),
            0x08 => self.x = self.x.saturating_sub(self.advance()),
            0x09 => {
                let stop = self.cell * 8;
                self.x = (self.x / stop + 1) * stop;
                if self.x >= self.width {
                    self.newline();
                }
            }
            0x0e => self.wide_line = true,
            0x14 => self.wide_line = false,
            0x0f => self.cell = self.model.dpi().0 / 17,
            0x12 => self.cell = self.model.dpi().0 / 10,
            0x20..=0x7e => self.glyph(char::from(b)),
            0xa0..=0xfe => self.glyph('?'),
            _ => {}
        }
    }

    /// Back to the power-on settings; the paper stays where it is.
    fn settings_reset(&mut self) {
        let (x, y) = (self.x, self.y);
        let page = std::mem::take(&mut self.page);
        let finished = self.finished.take();
        *self = Self::with_model(self.model);
        (self.x, self.y, self.page, self.finished) = (x, y, page, finished);
    }

    fn esc_p(&mut self, cmd: u8) {
        self.seq = Seq::Text;
        let (dx, dy) = self.model.dpi();
        match params(cmd) {
            None => self.seq = Seq::UntilZero,
            Some(0) => match cmd {
                b'@' => self.settings_reset(),
                b'E' | b'G' => self.bold = true,
                b'F' | b'H' => self.bold = false,
                b'M' => self.cell = dx / 12,
                b'P' => self.cell = dx / 10,
                b'g' => self.cell = dx / 15,
                b'0' => self.spacing = dy / 8,
                b'1' => self.spacing = dy * 7 / 72,
                b'2' => self.spacing = dy / 6,
                _ => {}
            },
            Some(left) => {
                self.seq = Seq::Params {
                    cmd,
                    left,
                    first: 0,
                }
            }
        }
    }

    /// The last parameter of `cmd` is `v`, the one before `first`.
    fn finish_params(&mut self, cmd: u8, first: u8, v: u8) {
        let flag = v & 1 != 0;
        let left = u16::from(first) | u16::from(v) << 8;
        let (unit, unit_a, _) = self.model.steps();
        self.seq = Seq::Text;
        let (per_inch, bytes) = match cmd {
            b'-' => {
                self.underline = flag;
                return;
            }
            b'W' => {
                self.wide = flag;
                return;
            }
            b'3' => {
                self.spacing = usize::from(v) * unit;
                return;
            }
            b'A' => {
                self.spacing = usize::from(v) * unit_a;
                return;
            }
            b'J' => {
                self.feed_paper(usize::from(v) * unit);
                return;
            }
            // n/360 inch.
            b'+' if self.model == Model::P6 => {
                self.spacing = usize::from(v);
                return;
            }
            b'K' => (60, 1),
            b'L' | b'Y' => (120, 1),
            b'Z' => (240, 1),
            // ESC * after its mode.
            c if c & 0x80 != 0 => star_mode(c & 0x7f),
            _ => return,
        };
        if left > 0 {
            self.got = 0;
            self.seq = Seq::Graphics {
                left,
                step: (self.model.dpi().0 / per_inch).max(1) as u8,
                bytes,
            };
        }
    }

    /// `FS` codes of the NEC: `FS 3 n` sets the line pitch to n/360 inch.
    fn fs(&mut self, b: u8) {
        self.seq = if b == b'3' {
            Seq::Params {
                cmd: FS,
                left: 1,
                first: 0,
            }
        } else {
            Seq::Text
        };
    }

    fn escape(&mut self, cmd: u8) {
        if self.model == Model::ImageWriter {
            self.esc_apple(cmd);
        } else {
            self.esc_p(cmd);
        }
    }

    /// The ImageWriter's `ESC` codes.
    fn esc_apple(&mut self, cmd: u8) {
        self.seq = Seq::Text;
        let digits = |cmd, left| Seq::Digits {
            cmd,
            left,
            value: 0,
        };
        let skip = |left| Seq::Params {
            cmd: 0,
            left,
            first: 0,
        };
        // Pitches: dots of a column (720 dots an inch, for 72, 80, 96, 107,
        // 120, 136, 144 and 160 columns an inch), eight columns a character.
        let pitch = match cmd {
            b'n' => 10,
            b'N' => 9,
            b'E' => 8,
            b'e' | b'q' => 6,
            b'Q' | b'p' => 5,
            b'P' => 4,
            _ => 0,
        };
        if pitch != 0 {
            self.pitch_step = pitch;
            self.cell = usize::from(pitch) * 8;
            return;
        }
        match cmd {
            b'A' => self.spacing = 24,
            b'B' => self.spacing = 18,
            b'r' => self.reverse = true,
            b'f' => self.reverse = false,
            b'c' => self.settings_reset(),
            b'!' => self.bold = true,
            b'"' => self.bold = false,
            b'X' => self.underline = true,
            b'Y' => self.underline = false,
            b'G' | b'S' => self.seq = digits(b'G', 4),
            b'V' => self.seq = digits(b'V', 4),
            b'F' => self.seq = digits(b'F', 4),
            b'T' => self.seq = digits(b'T', 2),
            b'L' => self.seq = digits(b'L', 3),
            b'Z' | b'D' => self.seq = skip(2),
            b'K' | b'a' => self.seq = skip(1),
            _ => {}
        }
    }

    /// The digits of an ImageWriter code are in.
    fn digits_done(&mut self, cmd: u8, value: u16) {
        self.seq = Seq::Text;
        match cmd {
            b'G' if value > 0 => {
                self.seq = Seq::Graphics {
                    left: value,
                    step: self.pitch_step,
                    bytes: 0,
                }
            }
            b'V' => self.seq = Seq::Repeat { count: value },
            b'F' => self.x = usize::from(value) * usize::from(self.pitch_step),
            b'T' => self.spacing = usize::from(value),
            _ => {}
        }
    }

    /// An ImageWriter column: top pin in bit 0.
    fn apple_column(&mut self, b: u8) {
        let (step, pin) = (self.pitch_step, self.model.steps().2);
        self.pins(8, pin, step, |p| b & (1 << p) != 0);
    }

    /// The next byte from the computer.
    pub fn feed(&mut self, b: u8) {
        match self.seq {
            Seq::Text => self.text(b),
            Seq::Esc => self.escape(b),
            Seq::Fs => self.fs(b),
            Seq::UntilZero => {
                if b == 0 {
                    self.seq = Seq::Text;
                }
            }
            // ESC * m n1 n2: the mode, then the count like ESC K.
            Seq::Params {
                cmd: b'*', left: 3, ..
            } => {
                self.seq = Seq::Params {
                    cmd: 0x80 | (b & 0x7f),
                    left: 2,
                    first: 0,
                }
            }
            Seq::Params { cmd, left: 2, .. } => {
                self.seq = Seq::Params {
                    cmd,
                    left: 1,
                    first: b,
                }
            }
            Seq::Params { cmd: FS, .. } => {
                self.seq = Seq::Text;
                self.spacing = usize::from(b);
            }
            Seq::Params { cmd, first, .. } => self.finish_params(cmd, first, b),
            Seq::Digits { cmd, left, value } => {
                let value = value
                    .saturating_mul(10)
                    .saturating_add(u16::from(b.wrapping_sub(b'0') % 10));
                if left > 1 {
                    self.seq = Seq::Digits {
                        cmd,
                        left: left - 1,
                        value,
                    };
                } else {
                    self.digits_done(cmd, value);
                }
            }
            Seq::Repeat { count } => {
                self.seq = Seq::Text;
                for _ in 0..count {
                    self.apple_column(b);
                }
            }
            Seq::Graphics { left, step, bytes } => {
                if bytes == 0 {
                    self.apple_column(b);
                } else {
                    self.column[usize::from(self.got)] = b;
                    self.got += 1;
                    if self.got < bytes {
                        return;
                    }
                    self.got = 0;
                    self.column_done(step, bytes);
                }
                self.seq = if left > 1 {
                    Seq::Graphics {
                        left: left - 1,
                        step,
                        bytes,
                    }
                } else {
                    Seq::Text
                };
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.model as u8);
        w.bytes(&self.page);
        w.u32(self.x as u32);
        w.u32(self.y as u32);
        w.u32(self.spacing as u32);
        w.u8(self.cell as u8);
        w.bool(self.wide);
        w.bool(self.wide_line);
        w.bool(self.reverse);
        w.bool(self.bold);
        w.bool(self.underline);
        let (kind, a, b, c) = match self.seq {
            Seq::Text => (0, 0, 0, 0),
            Seq::Esc => (1, 0, 0, 0),
            Seq::Params { cmd, left, first } => (2, u16::from(left), cmd, first),
            Seq::UntilZero => (3, 0, 0, 0),
            Seq::Graphics { left, step, bytes } => (4, left, step, bytes),
            Seq::Fs => (5, 0, 0, 0),
            Seq::Digits { cmd, left, value } => (6, value, cmd, left),
            Seq::Repeat { count } => (7, count, 0, 0),
        };
        w.u8(kind);
        w.u16(a);
        w.u8(b);
        w.u8(c);
        w.raw(&self.column);
        w.u8(self.got);
        w.u8(self.pitch_step);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let bad = StateError::Invalid;
        let model = Model::from_code(r.u8()?).ok_or(bad("printer model"))?;
        *self = Self::with_model(model);
        let page = r.bytes()?.to_vec();
        if !page.is_empty() && page.len() != self.stride * self.height {
            return Err(bad("printer page size"));
        }
        self.page = page;
        self.x = (r.u32()? as usize).min(self.width);
        self.y = (r.u32()? as usize).min(self.height);
        self.spacing = r.u32()? as usize;
        self.cell = usize::from(r.u8()?).max(1);
        self.wide = r.bool()?;
        self.wide_line = r.bool()?;
        self.reverse = r.bool()?;
        self.bold = r.bool()?;
        self.underline = r.bool()?;
        let (kind, a, b, c) = (r.u8()?, r.u16()?, r.u8()?, r.u8()?);
        self.seq = match kind {
            0 => Seq::Text,
            1 => Seq::Esc,
            2 => Seq::Params {
                cmd: b,
                left: a as u8,
                first: c,
            },
            3 => Seq::UntilZero,
            4 => Seq::Graphics {
                left: a,
                step: b.max(1),
                bytes: c.min(6),
            },
            5 => Seq::Fs,
            6 => Seq::Digits {
                cmd: b,
                left: c,
                value: a,
            },
            7 => Seq::Repeat { count: a },
            _ => return Err(bad("printer sequence")),
        };
        r.raw_into(&mut self.column)?;
        self.got = r.u8()?.min(5);
        self.pitch_step = r.u8()?.max(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLYPH: usize = 27;

    fn printed_on(model: Model, bytes: &[u8]) -> Epson {
        let mut e = Epson::with_model(model);
        bytes.iter().for_each(|&b| e.feed(b));
        e
    }

    fn printed(bytes: &[u8]) -> Epson {
        printed_on(Model::Fx80, bytes)
    }

    fn inked(e: &Epson, x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> bool {
        y.clone().any(|y| x.clone().any(|x| e.dot(x, y)))
    }

    #[test]
    fn text_goes_to_the_next_cell_and_line() {
        let e = printed(b"I\r\n I");
        assert!(inked(&e, 0..24, 0..GLYPH), "first line");
        assert!(!inked(&e, 24..48, 0..GLYPH), "only one letter");
        assert!(inked(&e, 24..48, 36..36 + GLYPH), "second line");
        assert!(!inked(&e, 0..24, 36..36 + GLYPH), "the space");
    }

    #[test]
    fn a_long_line_wraps() {
        let e = printed(&[b'X'; 81]);
        assert_eq!((e.x, e.y), (24, 36));
    }

    #[test]
    fn sizes_and_spacing() {
        assert_eq!(printed(b"\x0fA").x, 14, "condensed");
        assert_eq!(printed(b"\x1bMA").x, 20, "elite");
        assert_eq!(printed(b"\x1bW\x01A").x, 48, "wide");
        assert_eq!(printed(b"\x0eA\r\nA").x, 24, "SO ends with the line");
        assert_eq!(printed(b"\x1b3\x10\n").y, 16);
        assert_eq!(printed(b"\x1bA\x0c\n").y, 36);
        assert_eq!(printed(b"\x1bJ\x05").y, 5);
    }

    #[test]
    fn escape_parameters_never_print() {
        assert!(printed(b"\x1bQ\x50\x1bD\x08\x10\x18\x00\x1bl\x05")
            .page()
            .is_none());
    }

    #[test]
    fn graphics_columns_put_the_top_pin_first() {
        // ESC K, two columns: top pin, bottom pin.
        let e = printed(&[ESC, b'K', 2, 0, 0x80, 0x01, b'\r']);
        assert!(e.dot(0, 0) && e.dot(3, 2));
        assert!(!e.dot(0, 21));
        assert!(e.dot(4, 21) && e.dot(7, 23));
        // ESC * mode 3: one dot wide.
        let e = printed(&[ESC, b'*', 3, 1, 0, 0xff, b'\r']);
        assert!(e.dot(0, 0) && !e.dot(1, 0));
        // Data bytes that look like codes are graphics.
        let e = printed(&[ESC, b'Z', 1, 0, ESC, b'A']);
        assert_eq!(e.x, 1 + 24);
    }

    #[test]
    fn the_nec_has_24_pins_and_finer_steps() {
        let e = printed_on(Model::P6, b"A");
        assert_eq!(e.size(), (2880, 3960));
        assert_eq!(e.x, 36, "ten characters an inch");
        assert_eq!(printed_on(Model::P6, b"\x1b3\x10\n").y, 32, "n/180");
        assert_eq!(printed_on(Model::P6, b"\x1bA\x0a\n").y, 60, "n/60");
        assert_eq!(printed_on(Model::P6, b"\x1b+\x21\n").y, 33, "n/360");
        assert_eq!(printed_on(Model::P6, b"\x1c3\x21\n").y, 33, "FS 3");
        // ESC * 39: 180 dpi, three bytes a column; pin 24 is the last bit.
        let e = printed_on(Model::P6, &[ESC, b'*', 39, 1, 0, 0x80, 0, 0x01, b'\r']);
        assert!(e.dot(0, 0) && e.dot(1, 1));
        assert!(e.dot(0, 46) && e.dot(1, 47));
        assert!(!e.dot(0, 2));
        assert_eq!(e.seq, Seq::Text);
        // FS is a code on the NEC only: on the Epson nothing comes of it.
        assert_eq!(printed(&[FS]).seq, Seq::Text);
        assert_eq!(printed_on(Model::P6, &[FS]).seq, Seq::Fs);
    }

    #[test]
    fn the_imagewriter_has_its_own_codes() {
        let e = printed_on(Model::ImageWriter, b"\x1bpA");
        assert_eq!(e.size(), (5760, 1584));
        assert_eq!(e.row_repeat(), 5);
        assert_eq!(e.x, 40, "144 columns an inch, eight a character");
        assert_eq!(printed_on(Model::ImageWriter, b"\x1bT16\n").y, 16);
        assert_eq!(printed_on(Model::ImageWriter, b"\x1bA\n").y, 24);
        assert_eq!(printed_on(Model::ImageWriter, b"\x1bB\n").y, 18);
        // ESC G 0002 at 72 dots an inch: top pin in bit 0, count in digits.
        let mut bytes = b"\x1bn\x1bG0002".to_vec();
        bytes.extend_from_slice(&[0x01, 0x80, b'\r']);
        let e = printed_on(Model::ImageWriter, &bytes);
        assert!(e.dot(0, 0) && e.dot(9, 1));
        assert!(!e.dot(0, 14));
        assert!(e.dot(10, 14) && e.dot(19, 15));
        assert_eq!(e.seq, Seq::Text);
        // The driver's ESC r, LF, ESC f, LF comes back to where it was.
        assert_eq!(printed_on(Model::ImageWriter, b"\n\x1br\n\x1bf\n\n").y, 48);
        // ESC V repeats a column, ESC F places the head.
        let e = printed_on(Model::ImageWriter, b"\x1bV0003\xff");
        assert_eq!(e.x, 27);
        assert_eq!(printed_on(Model::ImageWriter, b"\x1bF0010").x, 90);
    }

    #[test]
    fn a_character_picture_fills_a_text_cell_on_every_model() {
        let rows = [0xff, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x81];
        for (model, cell) in [(Model::Fx80, 24), (Model::P6, 36), (Model::ImageWriter, 72)] {
            let e = printed_on(model, &model.glyph_bytes(&rows));
            assert_eq!(e.x, cell, "{model:?}");
            assert_eq!(e.seq, Seq::Text, "{model:?}");
            assert!(e.dot(0, 0), "{model:?}: top left");
        }
        // Bottom right, on the Epson: column 7 of 3 dots, pin 7 of 3.
        let e = printed_on(Model::Fx80, &Model::Fx80.glyph_bytes(&rows));
        assert!(e.dot(23, 23) && !e.dot(23, 12));
    }

    #[test]
    fn form_feed_hands_the_page_over() {
        let mut e = printed(b"A\x0c");
        let page = e.finished.take().expect("a page");
        assert_eq!(page.len(), e.stride * e.height);
        assert!(e.page().is_none());
        assert_eq!((e.x, e.y), (0, 0));
        // A blank page is not a page.
        assert!(printed(b"\x0c").finished.is_none());
        // Feeding past the bottom ejects it too.
        let mut bytes = b"A".to_vec();
        bytes.extend_from_slice(&[b'\n'; 70]);
        assert!(printed(&bytes).finished.is_some());
    }

    #[test]
    fn reset_keeps_the_paper_where_it_is() {
        let e = printed(b"A\x1b3\x10\n\x1bM\x1b@");
        assert_eq!((e.spacing, e.cell, e.y), (36, 24, 16));
        assert!(e.page().is_some());
    }

    #[test]
    fn savestate_round_trip_mid_sequence() {
        let a = printed_on(Model::P6, &[b'A', ESC, b'*', 39, 3, 0, 0xff]);
        let mut w = StateWriter::new();
        a.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Epson::new();
        b.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(b.model(), Model::P6);
        assert_eq!(b.seq, a.seq);
        assert_eq!(b.page, a.page);
        assert_eq!((b.x, b.got), (a.x, a.got));
    }
}
