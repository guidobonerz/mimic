//! Lumière, the Cogitatron 16's video chip (SPEC.md section 8): registers
//! at FFFF:0800, the palette at FFFF:1000, and the picture from chip RAM.
//!
//! Built so far (steps 3 and 4): mode 0 (320 × 240, 8-bit indexed, pixels
//! doubled and centred) and mode 4 (100 × 37 text, 8 × 16 font from chip
//! RAM, blink and cursor). Modes 1–3 and the sprites follow in later
//! steps; until then those modes show the border colour.
//!
//! The picture is put out up to the beam ([`Lumiere::draw_to`]) before
//! anything changes a register, so a write in the middle of a line shows
//! from that pixel on (the Copper's colour splits). The framebuffer pointer
//! is taken over at the VBlank, or with a write to FBNOW from the next
//! line (split screens); from there the rows count anew (`row_addr`,
//! `row_line`). Without framebuffer DMA (DMACON bit 0) the screen shows
//! the border colour.
//!
//! Blink and cursor alternate every 32 fields (about 0.53 s) **[S]**. All
//! registers are 0 after reset (display off, border palette entry 0).

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

/// Picture size (SPEC.md 2).
pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 600;

/// Register offsets in segment FFFF.
pub const VMODE: u16 = 0x0800;
pub const FBSEG: u16 = 0x0802;
pub const FBOFS: u16 = 0x0804;
pub const PITCH: u16 = 0x0806;
pub const FINEX: u16 = 0x0808;
pub const BORDER: u16 = 0x080a;
pub const FGINDEX: u16 = 0x080c;
pub const TXTBANK: u16 = 0x080e;
pub const FONTSEG: u16 = 0x0810;
pub const FONTOFS: u16 = 0x0812;
pub const CURSOR: u16 = 0x0814;
pub const CURSHAPE: u16 = 0x0816;
pub const BEAMY: u16 = 0x0818;
pub const BEAMX: u16 = 0x081a;
pub const VSTAT: u16 = 0x081c;
pub const FBNOW: u16 = 0x081e;
pub const PALETTE: u16 = 0x1000;

/// Fields per blink phase.
const BLINK_FIELDS: u64 = 32;

/// Chip RAM as the video chip sees it (linear addresses).
pub trait ChipRam {
    fn chip_byte(&self, addr: u32) -> u8;
}

/// The beam: line `y` (0–627), pixel `x` (0–1059).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beam {
    pub x: u16,
    pub y: u16,
}

impl Beam {
    fn vblank(self) -> bool {
        self.y >= HEIGHT as u16
    }
    fn hblank(self) -> bool {
        self.x >= WIDTH as u16
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lumiere {
    vmode: u16,
    /// Written framebuffer pointer, and the one taken over.
    fb: (u16, u16),
    fb_active: (u16, u16),
    pitch: u16,
    finex: u16,
    border: u16,
    fgindex: u16,
    txtbank: u16,
    font: (u16, u16),
    cursor: u16,
    curshape: u16,
    /// VSTAT bit 2: pointer taken over (cleared on read).
    taken_over: bool,
    palette: [u16; 256],
    fields: u64,
    /// Framebuffer DMA on (Babbage's DMACON bit 0).
    dma: bool,
    /// FBNOW was written: take the pointer over at the end of the line.
    now_pending: bool,
    /// The current row's address and the line in it.
    row_addr: u32,
    row_line: u16,
    /// The line being put out and how far (pixels).
    line: u16,
    drawn: u16,
    pixels: Vec<Rgb8>,
}

impl Default for Lumiere {
    fn default() -> Self {
        Self {
            vmode: 0,
            fb: (0, 0),
            fb_active: (0, 0),
            pitch: 0,
            finex: 0,
            border: 0,
            fgindex: 0,
            txtbank: 0,
            font: (0, 0),
            cursor: 0,
            curshape: 0,
            taken_over: false,
            palette: [0; 256],
            fields: 0,
            dma: false,
            now_pending: false,
            row_addr: 0,
            row_line: 0,
            line: 0,
            drawn: 0,
            pixels: vec![Rgb8::default(); (WIDTH * HEIGHT) as usize],
        }
    }
}

/// RGB 5:6:5 to 8 bits a channel.
pub fn rgb565(c: u16) -> Rgb8 {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    Rgb8 {
        r: (r << 3 | r >> 2) as u8,
        g: (g << 2 | g >> 4) as u8,
        b: (b << 3 | b >> 2) as u8,
    }
}

/// A mode's picture area: first line, lines, lines a row (one fetched
/// row of bytes or characters).
fn area(mode: u16) -> Option<(u16, u16, u16)> {
    match mode {
        0 | 1 => Some((60, 480, 2)),
        2 | 3 => Some((0, 600, 1)),
        4 => Some((4, 592, 16)),
        _ => None,
    }
}

impl Lumiere {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset: every register 0.
    pub fn reset(&mut self) {
        let pixels = std::mem::take(&mut self.pixels);
        *self = Self {
            pixels,
            ..Self::default()
        };
    }

    pub fn pixels(&self) -> &[Rgb8] {
        &self.pixels
    }

    /// Fields since reset.
    pub fn fields(&self) -> u64 {
        self.fields
    }

    pub fn mode(&self) -> u16 {
        self.vmode & 7
    }

    pub fn display_on(&self) -> bool {
        self.vmode & 0x8000 != 0
    }

    pub fn palette(&self) -> &[u16; 256] {
        &self.palette
    }

    /// Framebuffer DMA on or off (Babbage's DMACON).
    pub fn set_dma(&mut self, on: bool) {
        self.dma = on;
    }

    /// Register read without side effects.
    pub fn peek(&self, off: u16, beam: Beam) -> u16 {
        let off = off & !1;
        match off {
            VMODE => self.vmode,
            FBSEG => self.fb.0,
            FBOFS => self.fb.1,
            PITCH => self.pitch,
            FINEX => self.finex,
            BORDER => self.border,
            FGINDEX => self.fgindex,
            TXTBANK => self.txtbank,
            FONTSEG => self.font.0,
            FONTOFS => self.font.1,
            CURSOR => self.cursor,
            CURSHAPE => self.curshape,
            BEAMY => beam.y,
            BEAMX => beam.x,
            VSTAT => {
                u16::from(beam.vblank())
                    | u16::from(beam.hblank()) << 1
                    | u16::from(self.taken_over) << 2
            }
            PALETTE..=0x11fe => self.palette[usize::from((off - PALETTE) / 2)],
            _ => 0,
        }
    }

    /// Register read (VSTAT bit 2 clears).
    pub fn read(&mut self, off: u16, beam: Beam) -> u16 {
        let v = self.peek(off, beam);
        if off & !1 == VSTAT {
            self.taken_over = false;
        }
        v
    }

    /// Register write. The machine puts the picture out up to the beam
    /// first ([`Lumiere::draw_to`]).
    pub fn write(&mut self, off: u16, v: u16) {
        let off = off & !1;
        match off {
            VMODE => self.vmode = v & 0x8007,
            FBSEG => self.fb.0 = v,
            FBOFS => self.fb.1 = v,
            PITCH => self.pitch = v,
            FINEX => self.finex = v & 1,
            BORDER => self.border = v,
            FGINDEX => self.fgindex = v & 0xff,
            TXTBANK => self.txtbank = v & 0xf0,
            FONTSEG => self.font.0 = v,
            FONTOFS => self.font.1 = v,
            CURSOR => self.cursor = v,
            CURSHAPE => self.curshape = v & 0xff,
            FBNOW => self.now_pending = true,
            PALETTE..=0x11fe => self.palette[usize::from((off - PALETTE) / 2)] = v,
            _ => {}
        }
    }

    fn take_over(&mut self) {
        self.fb_active = self.fb;
        self.row_addr = u32::from(self.fb.0) << 16 | u32::from(self.fb.1);
        self.row_line = 0;
    }

    /// The VBlank starts (line 600): the framebuffer pointer is taken over
    /// (VSTAT bit 2; an FBNOW take-over does not set it).
    pub fn vblank(&mut self) {
        self.take_over();
        self.taken_over = true;
        self.fields += 1;
    }

    /// Chip-bus slots the framebuffer fetch for picture line `y` takes
    /// (SPEC.md 5): the fixed DMA of the line before.
    pub fn fetch_slots(&self, y: u16) -> u16 {
        if !self.dma || !self.display_on() {
            return 0;
        }
        let Some((top, lines, per_row)) = area(self.mode()) else {
            return 0;
        };
        if !(top..top + lines).contains(&y) {
            return 0;
        }
        let first_of_row = (y - top).is_multiple_of(per_row);
        match self.mode() {
            0 => u16::from(first_of_row) * 80,
            1 => u16::from(first_of_row) * 160,
            2 => 100,
            3 => 200,
            _ => u16::from(first_of_row) * 50 + 25,
        }
    }

    fn colour(&self, index: u8) -> Rgb8 {
        rgb565(self.palette[usize::from(index)])
    }

    fn blink_on(&self) -> bool {
        (self.fields / BLINK_FIELDS).is_multiple_of(2)
    }

    /// Put out line `y` up to pixel `x` (exclusive) with the registers as
    /// they are now.
    pub fn draw_to(&mut self, y: u16, x: u16, mem: &dyn ChipRam) {
        if y != self.line || u32::from(y) >= HEIGHT {
            return;
        }
        let end = x.min(WIDTH as u16);
        if end <= self.drawn {
            return;
        }
        let mut px = std::mem::take(&mut self.pixels);
        let base = usize::from(y) * WIDTH as usize;
        let span = &mut px[base + usize::from(self.drawn)..base + usize::from(end)];
        self.span(y, self.drawn, span, mem);
        self.pixels = px;
        self.drawn = end;
    }

    /// The end of line `y`: put the rest out, step the row, take an FBNOW
    /// pointer over.
    pub fn end_line(&mut self, y: u16, mem: &dyn ChipRam) {
        self.draw_to(y, WIDTH as u16, mem);
        if let Some((top, lines, per_row)) = area(self.mode()) {
            if self.display_on() && (top..top + lines).contains(&y) {
                self.row_line += 1;
                if self.row_line >= per_row {
                    self.row_line = 0;
                    self.row_addr = self.row_addr.wrapping_add(u32::from(self.pitch));
                }
            }
        }
        if self.now_pending {
            self.now_pending = false;
            self.take_over();
        }
        self.line = if y + 1 >= 628 { 0 } else { y + 1 };
        self.drawn = 0;
    }

    /// Pixels `from`.. of line `y` into `out`, the registers as they are.
    fn span(&self, y: u16, from: u16, out: &mut [Rgb8], mem: &dyn ChipRam) {
        let border = self.colour(self.border as u8);
        let area = area(self.mode()).filter(|(top, lines, _)| (*top..top + lines).contains(&y));
        let Some((top, _, per_row)) = area.filter(|_| self.display_on() && self.dma) else {
            out.fill(border);
            return;
        };
        let line = self.row_line.min(per_row - 1);
        match self.mode() {
            0 => {
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i as u16;
                    *p = if (80..720).contains(&x) {
                        let at = self.row_addr.wrapping_add(u32::from((x - 80) / 2));
                        self.colour(mem.chip_byte(at))
                    } else {
                        border
                    };
                }
            }
            4 => {
                let font = u32::from(self.font.0) << 16 | u32::from(self.font.1);
                let blink = self.blink_on();
                let (first, last) = (self.curshape & 15, self.curshape >> 4 & 15);
                let cursor_row = u32::from((y - top) / per_row) * 100;
                let bank = self.txtbank as u8;
                // One character cell at a time.
                let mut cell = u32::MAX;
                let (mut bits, mut fg, mut bg) = (0u8, border, border);
                for (i, p) in out.iter_mut().enumerate() {
                    let x = from + i as u16;
                    let col = u32::from(x / 8);
                    if col != cell {
                        cell = col;
                        let at = self.row_addr.wrapping_add(2 * col);
                        let (ch, attr) = (mem.chip_byte(at), mem.chip_byte(at.wrapping_add(1)));
                        bits =
                            mem.chip_byte(font.wrapping_add(u32::from(ch) * 16 + u32::from(line)));
                        if attr & 0x80 != 0 && !blink {
                            bits = 0;
                        }
                        if cursor_row + col == u32::from(self.cursor)
                            && blink
                            && (first..=last).contains(&line)
                        {
                            bits = !bits;
                        }
                        fg = self.colour(bank | attr & 15);
                        bg = self.colour(bank | attr >> 4 & 7);
                    }
                    *p = if bits & (0x80 >> (x % 8)) != 0 {
                        fg
                    } else {
                        bg
                    };
                }
            }
            _ => out.fill(border),
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"LUMI", 2, |w| {
            for v in [
                self.vmode,
                self.fb.0,
                self.fb.1,
                self.fb_active.0,
                self.fb_active.1,
                self.pitch,
                self.finex,
                self.border,
                self.fgindex,
                self.txtbank,
                self.font.0,
                self.font.1,
                self.cursor,
                self.curshape,
            ] {
                w.u16(v);
            }
            w.bool(self.taken_over);
            for p in self.palette {
                w.u16(p);
            }
            w.u64(self.fields);
            w.bool(self.dma);
            w.bool(self.now_pending);
            w.u32(self.row_addr);
            w.u16(self.row_line);
            w.u16(self.line);
            w.u16(self.drawn);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"LUMI", 2)?;
        let mut n = Self {
            vmode: c.u16()? & 0x8007,
            fb: (c.u16()?, c.u16()?),
            fb_active: (c.u16()?, c.u16()?),
            pitch: c.u16()?,
            finex: c.u16()? & 1,
            border: c.u16()?,
            fgindex: c.u16()? & 0xff,
            txtbank: c.u16()? & 0xf0,
            font: (c.u16()?, c.u16()?),
            cursor: c.u16()?,
            curshape: c.u16()? & 0xff,
            taken_over: c.bool()?,
            palette: [0; 256],
            fields: 0,
            dma: false,
            now_pending: false,
            row_addr: 0,
            row_line: 0,
            line: 0,
            drawn: 0,
            pixels: Vec::new(),
        };
        for p in &mut n.palette {
            *p = c.u16()?;
        }
        n.fields = c.u64()?;
        n.dma = c.bool()?;
        n.now_pending = c.bool()?;
        n.row_addr = c.u32()?;
        n.row_line = c.u16()? & 15;
        n.line = c.u16()?.min(627);
        n.drawn = c.u16()?.min(WIDTH as u16);
        // Only now, after everything loaded: the picture stays.
        n.pixels = std::mem::take(&mut self.pixels);
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);
    impl ChipRam for Ram {
        fn chip_byte(&self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
    }

    #[test]
    fn rgb565_expands_to_full_range() {
        assert_eq!(
            rgb565(0xffff),
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(rgb565(0xf800), Rgb8 { r: 255, g: 0, b: 0 });
        assert_eq!(rgb565(0x07e0), Rgb8 { r: 0, g: 255, b: 0 });
        assert_eq!(rgb565(0x0015), Rgb8 { r: 0, g: 0, b: 173 });
    }

    #[test]
    fn registers_vstat_and_the_pointer_latch() {
        let mut l = Lumiere::new();
        l.write(VMODE, 0xffff);
        assert_eq!(l.peek(VMODE, Beam::default()), 0x8007, "unused bits");
        l.write(FBSEG, 2);
        l.write(PALETTE + 2 * 255, 0x1234);
        assert_eq!(l.peek(PALETTE + 510, Beam::default()), 0x1234);
        assert_eq!(l.fb_active, (0, 0), "not before the VBlank");
        let beam = Beam { x: 900, y: 610 };
        assert_eq!(l.peek(VSTAT, beam), 0b011);
        l.vblank();
        assert_eq!(l.fb_active, (2, 0));
        assert_eq!(l.read(VSTAT, beam), 0b111);
        assert_eq!(l.read(VSTAT, beam), 0b011, "bit 2 clears on read");
        assert_eq!((l.peek(BEAMX, beam), l.peek(BEAMY, beam)), (900, 610));
        l.reset();
        assert_eq!(l.peek(VMODE, beam), 0);
    }

    #[test]
    fn a_write_mid_line_shows_from_its_pixel_and_fbnow_restarts_the_rows() {
        let ram = Ram((0..=255u8).cycle().take(0x2_0000).collect());
        let mut l = Lumiere::new();
        l.set_dma(true);
        l.write(VMODE, 0x8000);
        l.write(PITCH, 320);
        l.write(PALETTE + 2 * 7, 0xffff);
        l.write(BORDER, 7);
        l.vblank();
        for y in 0..100 {
            l.end_line(y, &ram);
        }
        // Line 100: border white up to x 40, then palette 7 changed to red.
        l.draw_to(100, 40, &ram);
        l.write(PALETTE + 2 * 7, 0xf800);
        l.end_line(100, &ram);
        let p = |x: usize| l.pixels()[100 * 800 + x];
        assert_eq!((p(39), p(40)), (rgb565(0xffff), rgb565(0xf800)));
        // Row (100 − 60) / 2 = 20 of the picture: byte 20 × 320 = 6400.
        assert_eq!(p(80), rgb565(l.palette[(6400 % 256) as usize]));
        // FBNOW at line 101: line 102 shows row 0 of the new pointer.
        l.write(FBSEG, 1);
        l.write(FBNOW, 0);
        l.end_line(101, &ram);
        l.end_line(102, &ram);
        assert_eq!(
            l.pixels()[102 * 800 + 82],
            rgb565(l.palette[(0x1_0001 % 256) as usize])
        );
        assert_eq!(l.fetch_slots(103), 0, "an odd picture line fetches nothing");
        assert_eq!(l.fetch_slots(104), 80);
        l.set_dma(false);
        assert_eq!(l.fetch_slots(104), 0);
    }
}
