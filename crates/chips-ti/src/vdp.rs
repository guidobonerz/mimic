//! TMS9918A (NTSC) and TMS9929A (PAL) video display processor (TI data
//! manual, sections 2 "Architecture", 3 "Video display processor" and 4
//! "Timing").
//!
//! **CPU interface.** Two ports: data (mode 0) and control (mode 1).
//! - Control writes come in pairs. The second byte with bit 7 set writes
//!   the first byte to register `b & 7`; otherwise the pair sets the 14-bit
//!   VRAM address (first byte low, second byte bits 5–0 high), and with bit
//!   6 clear the VDP reads the byte there into its read-ahead buffer and
//!   advances the address.
//! - Data reads return the read-ahead buffer and refill it from the
//!   address, which then advances; data writes store at the address (the
//!   buffer takes the byte too) and advance it. Any data access or status
//!   read resets the control-byte latch.
//! - Status reads (control port) return F (7, frame: set at the end of the
//!   last active line), 5S (6, a fifth sprite on a line), C (5, sprites
//!   coincide) and bits 4–0, the number of the fifth sprite (or of the last
//!   sprite looked at when no fifth one was found). Reading clears F, 5S
//!   and C.
//! - /INT is low while F and register 1's IE are set.
//!
//! **Registers.** R0: bit 1 M3, bit 0 external video. R1: bit 7 4/16K, 6
//! BLANK (0 shows only the backdrop), 5 IE, 4 M1, 3 M2, 1 SIZE (16×16
//! sprites), 0 MAG (sprites doubled). R2 name table at (R2 & $0F)·$400, R3
//! colour table at R3·$40, R4 pattern table at (R4 & 7)·$800, R5 sprite
//! attributes at (R5 & $7F)·$80, R6 sprite patterns at (R6 & 7)·$800, R7
//! text colour (7–4) and backdrop (3–0).
//!
//! **Modes.** Graphics I (no M bit): 32×24 names, one colour byte per 8
//! patterns. Graphics II (M3): the screen in three thirds of 256 patterns
//! each with a colour byte per pattern line; R3 bits 6–0 and R4 bits 1–0
//! mask the table indices (R3 bit 7 and R4 bit 2 pick the half of VRAM).
//! Multicolour (M2): each name a 2×2 block of 4×4-pixel colours, the
//! pattern byte for a row chosen by the row number modulo 4. Text (M1):
//! 40×24 characters of 6×8 pixels in R7's colours, no sprites; the picture
//! is 240 pixels wide with 8 more border pixels on each side.
//!
//! **Sprites.** 32 entries of Y, X, name, colour (bit 7 EC shifts the
//! sprite 32 pixels left); Y = $D0 ends the list; a sprite shows from line
//! Y + 1 (Y above $E0 counts from −32). At most four per line, earlier
//! numbers in front; colour 0 is transparent. Coincidence counts any two
//! sprites' set pattern bits on a displayed pixel.
//!
//! **Timing.** 342 pixel clocks per line (the colour subcarrier × 1.5:
//! 5.369 MHz NTSC, 5.320 MHz PAL), 262 lines NTSC and 313 PAL. A line:
//! 256 active pixels, right border 15, blanking and sync 50, left border
//! 13 (text mode: 240 active, borders 19 and 25). A field: 192 active
//! lines, bottom border 24 (PAL 48), blanking and sync 19, top border 27
//! (PAL 54). [`Tms9918::tick`] runs one pixel clock; a line's pixels are
//! drawn when its active area begins, from the registers and VRAM as they
//! are then.
//!
//! **Picture.** [`Tms9918::frame`] holds the borders and the active area:
//! 284 pixels (13 + 256 + 15) by 243 lines NTSC (27 + 192 + 24) or 294
//! PAL (54 + 192 + 48), as palette indices 0–15; colour 0 (transparent)
//! shows black. The 4/16K bit and external video are not modelled (16 KB
//! addressing always; no external video input). CPU accesses are never
//! refused: the data manual's minimum time between VRAM accesses during
//! the active display is not enforced.

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// VRAM size.
pub const VRAM: usize = 0x4000;
/// Pixel clocks per line.
pub const LINE: u16 = 342;
/// Picture width: left border, active area, right border.
pub const WIDTH: u32 = 13 + 256 + 15;
/// Left border pixels in the picture.
const LEFT: u32 = 13;

/// Which chip: the field length and the top/bottom borders differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VdpModel {
    /// TMS9918A, NTSC: 262 lines.
    Ntsc,
    /// TMS9929A, PAL: 313 lines.
    Pal,
}

impl VdpModel {
    pub fn lines(self) -> u16 {
        match self {
            VdpModel::Ntsc => 262,
            VdpModel::Pal => 313,
        }
    }

    /// Top border lines (shown in the picture).
    pub fn top(self) -> u16 {
        match self {
            VdpModel::Ntsc => 27,
            VdpModel::Pal => 54,
        }
    }

    /// Bottom border lines (shown in the picture).
    pub fn bottom(self) -> u16 {
        match self {
            VdpModel::Ntsc => 24,
            VdpModel::Pal => 48,
        }
    }

    /// Picture height: top border, 192 active lines, bottom border.
    pub fn height(self) -> u32 {
        u32::from(self.top() + 192 + self.bottom())
    }
}

/// The 16 colours as RGB, computed from the data manual's luminance and
/// colour-difference voltages (table 2-3); colour 0 is transparent and
/// shows black when nothing is behind it.
pub const PALETTE: [Rgb8; 16] = [
    rgb(0x000000),
    rgb(0x000000),
    rgb(0x21c842),
    rgb(0x5edc78),
    rgb(0x5455ed),
    rgb(0x7d76fc),
    rgb(0xd4524d),
    rgb(0x42ebf5),
    rgb(0xfc5554),
    rgb(0xff7978),
    rgb(0xd4c154),
    rgb(0xe6ce80),
    rgb(0x21b03b),
    rgb(0xc95bba),
    rgb(0xcccccc),
    rgb(0xffffff),
];

const fn rgb(c: u32) -> Rgb8 {
    Rgb8 {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
    }
}

/// Status register bits.
pub mod status {
    pub const F: u8 = 0x80;
    pub const FIFTH: u8 = 0x40;
    pub const COINCIDENCE: u8 = 0x20;
}

/// Pixel clock of the first active pixel within a line (the line starts
/// with the left border's 13 pixels in this model's numbering: pixel
/// clock 0 is the first border pixel).
const ACTIVE_START: u16 = 13;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tms9918 {
    pub model: VdpModel,
    pub vram: Vec<u8>,
    pub regs: [u8; 8],
    pub status: u8,
    /// VRAM address (14 bits).
    pub addr: u16,
    read_buffer: u8,
    /// The first byte of a control pair, waiting for the second.
    latch: Option<u8>,
    /// Beam: line (0 = first active line) and pixel clock in the line.
    pub line: u16,
    pub dot: u16,
    /// Fields since power-on.
    pub fields: u64,
    frame: Vec<u8>,
}

impl Tms9918 {
    pub fn new(model: VdpModel) -> Self {
        Self {
            model,
            vram: vec![0; VRAM],
            regs: [0; 8],
            status: 0,
            addr: 0,
            read_buffer: 0,
            latch: None,
            line: 0,
            dot: 0,
            fields: 0,
            frame: vec![0; (WIDTH * model.height()) as usize],
        }
    }

    /// RESET: registers cleared (blank, no interrupt), the beam keeps
    /// running, VRAM keeps its contents.
    pub fn reset(&mut self) {
        self.regs = [0; 8];
        self.status = 0;
        self.latch = None;
    }

    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    /// /INT asserted.
    pub fn int(&self) -> bool {
        self.status & status::F != 0 && self.regs[1] & 0x20 != 0
    }

    /// Data port read (CPU).
    pub fn read_data(&mut self) -> u8 {
        self.latch = None;
        let v = self.read_buffer;
        self.read_buffer = self.vram[usize::from(self.addr)];
        self.addr = (self.addr + 1) & 0x3fff;
        v
    }

    /// Data port write (CPU).
    pub fn write_data(&mut self, v: u8) {
        self.latch = None;
        self.vram[usize::from(self.addr)] = v;
        self.read_buffer = v;
        self.addr = (self.addr + 1) & 0x3fff;
    }

    /// Status read (CPU): clears F, 5S and C.
    pub fn read_status(&mut self) -> u8 {
        self.latch = None;
        let v = self.status;
        self.status &= !(status::F | status::FIFTH | status::COINCIDENCE);
        v
    }

    /// Side-effect-free reads for the debugger: (data buffer, status).
    pub fn peek(&self) -> (u8, u8) {
        (self.read_buffer, self.status)
    }

    /// Control port write (CPU).
    pub fn write_control(&mut self, v: u8) {
        let Some(first) = self.latch.take() else {
            self.latch = Some(v);
            // The low address byte takes effect at once (data manual: the
            // first byte goes to the address register's low half).
            self.addr = (self.addr & 0x3f00) | u16::from(v);
            return;
        };
        if v & 0x80 != 0 {
            self.regs[usize::from(v & 7)] = first;
            return;
        }
        self.addr = u16::from(v & 0x3f) << 8 | u16::from(first);
        if v & 0x40 == 0 {
            self.read_buffer = self.vram[usize::from(self.addr)];
            self.addr = (self.addr + 1) & 0x3fff;
        }
    }

    fn m1(&self) -> bool {
        self.regs[1] & 0x10 != 0
    }

    fn m2(&self) -> bool {
        self.regs[1] & 0x08 != 0
    }

    fn m3(&self) -> bool {
        self.regs[0] & 0x02 != 0
    }

    fn backdrop(&self) -> u8 {
        self.regs[7] & 15
    }

    /// One pixel clock. Returns true at the end of a field.
    pub fn tick(&mut self) -> bool {
        if self.dot == ACTIVE_START {
            self.draw_line();
        }
        self.dot += 1;
        if self.dot < LINE {
            return false;
        }
        self.dot = 0;
        self.line += 1;
        if self.line == 192 {
            // The end of the last active line: the frame flag.
            self.status |= status::F;
        }
        if self.line >= self.model.lines() {
            self.line = 0;
            self.fields += 1;
            return true;
        }
        false
    }

    /// The picture row of the current line, if it is shown.
    fn picture_row(&self) -> Option<u32> {
        let top = self.model.top();
        let lines = self.model.lines();
        let row = if self.line < 192 + self.model.bottom() {
            self.line + top
        } else if self.line >= lines - top {
            self.line - (lines - top)
        } else {
            return None;
        };
        Some(u32::from(row))
    }

    /// Draw the current line into the picture (borders included).
    fn draw_line(&mut self) {
        let Some(row) = self.picture_row() else {
            return;
        };
        let base = (row * WIDTH) as usize;
        let backdrop = self.backdrop();
        let mut px = [backdrop; WIDTH as usize];
        let active = self.line < 192 && self.regs[1] & 0x40 != 0;
        if active {
            let y = self.line;
            let text = self.m1();
            let mut line = [0u8; 256];
            if text {
                self.text_line(y, &mut line);
                for (i, c) in line.iter().take(240).enumerate() {
                    px[LEFT as usize + 8 + i] = *c;
                }
            } else {
                if self.m2() {
                    self.multicolor_line(y, &mut line);
                } else if self.m3() {
                    self.graphics2_line(y, &mut line);
                } else {
                    self.graphics1_line(y, &mut line);
                }
                self.sprites(y, &mut line);
                for (i, c) in line.iter().enumerate() {
                    px[LEFT as usize + i] = *c;
                }
            }
            for c in &mut px {
                if *c == 0 {
                    *c = backdrop;
                }
            }
        }
        self.frame[base..base + WIDTH as usize].copy_from_slice(&px);
    }

    fn name_base(&self) -> usize {
        usize::from(self.regs[2] & 0x0f) << 10
    }

    fn vram_at(&self, a: usize) -> u8 {
        self.vram[a & 0x3fff]
    }

    /// Two colours of a pattern byte into 8 pixels.
    fn put8(out: &mut [u8], pattern: u8, color: u8) {
        let (fg, bg) = (color >> 4, color & 15);
        for (i, p) in out.iter_mut().take(8).enumerate() {
            *p = if pattern & (0x80 >> i) != 0 { fg } else { bg };
        }
    }

    fn graphics1_line(&self, y: u16, out: &mut [u8; 256]) {
        let names = self.name_base() + usize::from(y / 8) * 32;
        let patterns = usize::from(self.regs[4] & 7) << 11;
        let colors = usize::from(self.regs[3]) << 6;
        for col in 0..32 {
            let n = usize::from(self.vram_at(names + col));
            let pattern = self.vram_at(patterns + n * 8 + usize::from(y & 7));
            let color = self.vram_at(colors + n / 8);
            Self::put8(&mut out[col * 8..], pattern, color);
        }
    }

    fn graphics2_line(&self, y: u16, out: &mut [u8; 256]) {
        let names = self.name_base() + usize::from(y / 8) * 32;
        let third = usize::from(y / 64) << 8;
        let r3 = usize::from(self.regs[3]);
        let r4 = usize::from(self.regs[4]);
        let pattern_mask = (r4 & 3) << 11 | 0x7ff;
        let color_mask = (r3 & 0x7f) << 6 | 0x3f;
        for col in 0..32 {
            let index = third | usize::from(self.vram_at(names + col));
            let offset = index << 3 | usize::from(y & 7);
            let pattern = self.vram_at((r4 & 4) << 11 | (offset & pattern_mask));
            let color = self.vram_at((r3 & 0x80) << 6 | (offset & color_mask));
            Self::put8(&mut out[col * 8..], pattern, color);
        }
    }

    fn multicolor_line(&self, y: u16, out: &mut [u8; 256]) {
        let names = self.name_base() + usize::from(y / 8) * 32;
        let patterns = usize::from(self.regs[4] & 7) << 11;
        let row = usize::from((y / 8) & 3) * 2 + usize::from((y & 7) / 4);
        for col in 0..32 {
            let n = usize::from(self.vram_at(names + col));
            let b = self.vram_at(patterns + n * 8 + row);
            for i in 0..4 {
                out[col * 8 + i] = b >> 4;
                out[col * 8 + 4 + i] = b & 15;
            }
        }
    }

    fn text_line(&self, y: u16, out: &mut [u8; 256]) {
        let names = self.name_base() + usize::from(y / 8) * 40;
        let patterns = usize::from(self.regs[4] & 7) << 11;
        let color = self.regs[7];
        for col in 0..40 {
            let n = usize::from(self.vram_at(names + col));
            let pattern = self.vram_at(patterns + n * 8 + usize::from(y & 7));
            let (fg, bg) = (color >> 4, color & 15);
            for i in 0..6 {
                out[col * 6 + i] = if pattern & (0x80 >> i) != 0 { fg } else { bg };
            }
        }
    }

    /// Sprites of line `y` over the playfield; sets 5S and C.
    fn sprites(&mut self, y: u16, out: &mut [u8; 256]) {
        let attrs = usize::from(self.regs[5] & 0x7f) << 7;
        let patterns = usize::from(self.regs[6] & 7) << 11;
        let size: i32 = if self.regs[1] & 0x02 != 0 { 16 } else { 8 };
        let mag: i32 = if self.regs[1] & 0x01 != 0 { 2 } else { 1 };
        let mut shown = 0;
        let mut last = 0u8;
        let mut owner = [false; 256];
        for n in 0..32u8 {
            let a = attrs + usize::from(n) * 4;
            let sy = self.vram_at(a);
            if sy == 0xd0 {
                break;
            }
            last = n;
            // Y is the line before the sprite's first; above $E0 it counts
            // from the top border.
            let top = if sy > 0xe0 {
                i32::from(sy) - 256 + 1
            } else {
                i32::from(sy) + 1
            };
            let dy = i32::from(y) - top;
            if dy < 0 || dy >= size * mag {
                continue;
            }
            if shown == 4 {
                if self.status & status::FIFTH == 0 {
                    self.status = (self.status & 0xe0) | status::FIFTH | n;
                }
                // The fifth sprite is not shown; the status keeps it.
                return;
            }
            shown += 1;
            let sx = i32::from(self.vram_at(a + 1));
            let mut name = usize::from(self.vram_at(a + 2));
            let color_byte = self.vram_at(a + 3);
            let color = color_byte & 15;
            let x0 = sx - if color_byte & 0x80 != 0 { 32 } else { 0 };
            if size == 16 {
                name &= 0xfc;
            }
            let row = (dy / mag) as usize;
            for px in 0..size * mag {
                let x = x0 + px;
                if !(0..256).contains(&x) {
                    continue;
                }
                let bit = (px / mag) as usize;
                // 16×16: names n, n+1 left half (rows 0–7, 8–15), n+2, n+3
                // the right half.
                let byte_addr = patterns + name * 8 + (bit / 8) * 16 + row;
                let pattern = self.vram_at(byte_addr);
                if pattern & (0x80 >> (bit % 8)) == 0 {
                    continue;
                }
                let x = x as usize;
                if owner[x] {
                    self.status |= status::COINCIDENCE;
                    continue;
                }
                owner[x] = true;
                if color != 0 {
                    out[x] = color;
                }
            }
        }
        if self.status & status::FIFTH == 0 {
            self.status = (self.status & 0xe0) | last;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"V918", 1, |w| {
            w.bytes(&self.vram);
            w.raw(&self.regs);
            w.u8(self.status);
            w.u16(self.addr);
            w.u8(self.read_buffer);
            w.bool(self.latch.is_some());
            w.u8(self.latch.unwrap_or(0));
            w.u16(self.line);
            w.u16(self.dot);
            w.u64(self.fields);
        });
    }

    /// The model is configuration; the picture is redrawn.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"V918", 1)?;
        let vram = c.bytes()?;
        if vram.len() != VRAM {
            return Err(StateError::Invalid("VRAM size"));
        }
        let mut regs = [0u8; 8];
        c.raw_into(&mut regs)?;
        let status = c.u8()?;
        let addr = c.u16()? & 0x3fff;
        let read_buffer = c.u8()?;
        let latched = c.bool()?;
        let latch = c.u8()?;
        let line = c.u16()?;
        let dot = c.u16()?;
        let fields = c.u64()?;
        if line >= self.model.lines() || dot >= LINE {
            return Err(StateError::Invalid("VDP beam position"));
        }
        self.vram = vram.to_vec();
        self.regs = regs;
        self.status = status;
        self.addr = addr;
        self.read_buffer = read_buffer;
        self.latch = latched.then_some(latch);
        self.line = line;
        self.dot = dot;
        self.fields = fields;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
