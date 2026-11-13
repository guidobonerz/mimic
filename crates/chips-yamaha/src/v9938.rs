//! Yamaha V9938 video display processor (MSX-VIDEO), the MSX2's successor
//! of the TMS9918A (V9938 Technical Data Book: chapter 1 "Registers", 2
//! "Screen modes", 3 "Sprites", 4 "Commands"; the timing from its appendix
//! "Display timing").
//!
//! **CPU interface.** Four ports.
//! - Port 0: VRAM data, through a read-ahead buffer as on the TMS9918A.
//! - Port 1, write: pairs. A second byte with bit 7 set and bit 6 clear
//!   writes the first byte to register `b & $3F`; with bit 7 clear the
//!   pair sets the low 14 bits of the VRAM address (bit 6 clear: read
//!   ahead). Read: the status register R15 selects.
//! - Port 2: the palette, two bytes per entry (`0RRR0BBB`, `00000GGG`) to
//!   the entry R16 points at; R16 then advances.
//! - Port 3: the register R17 points at (bits 5–0); R17 then advances
//!   unless its bit 7 is set. R17 itself cannot be written this way.
//!
//! The VRAM address is 17 bits: R14 holds bits 16–14. The 14-bit counter's
//! carry advances R14 in the V9938's own modes (M4 or M5 set), not in the
//! TMS9918A's.
//!
//! **Registers** (write-only): R0 `0 DG IE2 IE1 M5 M4 M3 0`; R1 `0 BL IE0
//! M1 M2 0 SI MAG`; R2 name table; R3 and R10 colour table; R4 pattern
//! generator; R5 and R11 sprite attributes; R6 sprite patterns; R7 text
//! and backdrop colour; R8 `MS LP TP CB VR 0 SPD BW` (TP: colour 0 is the
//! palette's, not transparent; SPD: sprites off); R9 `LN 0 S1 S0 IL EO NT
//! DC` (LN: 212 lines; IL: interlace; EO: two pages alternate by field;
//! NT: 1 = 313 lines, 50 Hz); R12 the text blink colours; R13 the blink
//! periods (on, off) in units of 10 fields; R14 VRAM address high; R15
//! status pointer; R16 palette pointer; R17 register pointer; R18 display
//! adjust (bits 3–0 horizontal, 7–4 vertical: 1…7 move the picture left
//! or up, 15…8 right or down); R19 the line of the line interrupt; R23
//! the first line shown (vertical scroll); R32–R46 the command registers
//! ([`cmd`]). R20–R22 (colour burst) are kept and not used.
//!
//! **Status.** S0 `F 5S C` and the fifth (ninth) sprite's number, as on
//! the TMS9918A; reading clears F, 5S and C. S1 `FL LPS ID(5) FH`: the ID
//! is 0, FH the line interrupt flag (set at the end of the active part of
//! the line whose number, counted from the first line shown plus R23,
//! equals R19; reading S1 clears it; while IE1 is off it stays set only to
//! the end of that line). S2 `TR VR HR BD 1 1 EO CE`. S3–S6 the place of
//! the first sprite collision (X + 12, Y + 8; reading S5 clears it). S7
//! the colour a command read, S8/S9 the column a search stopped at. /INT
//! is low while F and IE0 or FH and IE1 are set.
//!
//! **Modes** (M5…M1): `00000` G1, `00001` T1 (40 columns), `00010` MC,
//! `00100` G2, `01000` G3 (G2 with sprite mode 2), `01001` T2 (80 columns
//! of 6 half-width pixels, a blink bit per character in the colour table),
//! `01100` G4 (256 × 212, 4 bits per pixel, 128 bytes a line, four pages
//! of 32 KB), `10000` G5 (512 × 212, 2 bits), `10100` G6 (512 × 212, 4
//! bits, 256 bytes a line, two pages of 64 KB), `11100` G7 (256 × 212,
//! 8 bits `GGGRRRBB`). In G4–G7 R2's bits 6–5 (G6, G7: bit 5) select the
//! page. Other combinations show the backdrop. Colour 0 shows the
//! backdrop colour unless TP is set (not in G7). In G5 the backdrop and
//! the sprites' colours are pairs of 2-bit colours for the two halves of
//! a low-resolution pixel.
//!
//! **Sprites.** Mode 1 (G1, G2, MC) is the TMS9918A's: four on a line, Y
//! = 208 ends the list. Mode 2 (G3–G7): eight on a line, Y = 216 ends the
//! list, and a colour table of 16 bytes per sprite 512 bytes below the
//! attribute table gives each sprite line its colour (bits 3–0) and the
//! bits EC (7: 32 pixels left), CC (6) and IC (5: no collision). A CC
//! line is drawn with the priority of the nearest lower-numbered sprite
//! on the line without CC, its colour ORed where the two overlap; without
//! such a sprite it is not drawn. Sprites follow R23. None in T1 and T2.
//!
//! **Timing.** As the TMS9918A model of `chips-ti`: 342 clocks per line
//! (four of the chip's 21.48 MHz master clocks each), [`V9938::tick`] runs
//! one; a line is drawn when its active part begins, from the registers,
//! the palette and VRAM as they are then (changes within a line show on
//! the next). 262 lines, or 313 with NT, taken over at the start of each
//! field. Lines are counted from the first row of the picture: the top
//! border (27 lines, PAL 54; ten fewer with LN), 192 or 212 active lines,
//! the bottom border, then 19 blanked lines. F is set at the end of the
//! last active line.
//!
//! **Picture.** [`V9938::frame`]: 568 half-width pixels (26 + 512 + 30) by
//! 243 rows (PAL 294), as RGB; a palette step of 0–7 becomes 0, 36, 73 …
//! 255. Interlace is not drawn as a taller picture: the fields follow each
//! other (with EO they show the two pages alternately).
//!
//! Not modelled: the colour bus, light pen and mouse (MS, LP), external
//! synchronisation (S1, S0), the expansion RAM (MXC, MXS, MXD in the
//! command argument are ignored), the physical interleaving of VRAM in G6
//! and G7, the VRAM access time limits. The first byte of a control pair
//! does not touch the address before the second arrives. The page shown
//! under EO and blinking, FH while IE1 is off and the commands' speeds are
//! this model's reading of the data book, not measured.

use emu_core::frame::Rgb8;
use emu_core::{StateError, StateReader, StateWriter};

pub mod cmd;
pub mod gfx;
pub mod overlay;

/// VRAM size: 128 KB.
pub const VRAM: usize = 0x2_0000;
/// Clocks per line (four master clocks each).
pub const LINE: u16 = 342;
/// Picture width in half-width pixels: left border, active area, right
/// border.
pub const WIDTH: u32 = 2 * (13 + 256 + 15);
/// Picture rows of a 262-line and of a 313-line field.
pub const HEIGHT_NTSC: u32 = 243;
pub const HEIGHT_PAL: u32 = 294;
/// Left border in half-width pixels.
pub const LEFT: usize = 26;
/// Clock of the first and after the last active pixel within a line.
const ACTIVE_START: u16 = 13;
const ACTIVE_END: u16 = ACTIVE_START + 256;

/// Mode numbers: M5 M4 M3 M2 M1.
pub mod mode {
    pub const G1: u8 = 0x00;
    pub const T1: u8 = 0x01;
    pub const MC: u8 = 0x02;
    pub const G2: u8 = 0x04;
    pub const G3: u8 = 0x08;
    pub const T2: u8 = 0x09;
    pub const G4: u8 = 0x0c;
    pub const G5: u8 = 0x10;
    pub const G6: u8 = 0x14;
    pub const G7: u8 = 0x1c;
}

/// Status register 0 bits.
pub mod status {
    pub const F: u8 = 0x80;
    pub const FIFTH: u8 = 0x40;
    pub const COINCIDENCE: u8 = 0x20;
}

/// The palette after reset, as (R, G, B) steps of 0–7 (data book, "Colour
/// palette register": the TMS9918A's colours).
pub const DEFAULT_PALETTE: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (0, 0, 0),
    (1, 6, 1),
    (3, 7, 3),
    (1, 1, 7),
    (2, 3, 7),
    (5, 1, 1),
    (2, 6, 7),
    (7, 1, 1),
    (7, 3, 3),
    (6, 6, 1),
    (6, 6, 4),
    (1, 4, 1),
    (6, 2, 5),
    (5, 5, 5),
    (7, 7, 7),
];

/// The sprites' 16 colours in G7, which has no palette, as (R, G, B).
pub const G7_SPRITE_COLORS: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (0, 0, 2),
    (3, 0, 0),
    (3, 0, 2),
    (0, 3, 0),
    (0, 3, 2),
    (3, 3, 0),
    (3, 3, 2),
    (7, 4, 2),
    (0, 0, 7),
    (7, 0, 0),
    (7, 0, 7),
    (0, 7, 0),
    (0, 7, 7),
    (7, 7, 0),
    (7, 7, 7),
];

/// A colour step of 0–7 as 8 bits.
const LEVEL: [u8; 8] = [0, 36, 73, 109, 146, 182, 219, 255];

pub fn rgb777(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 {
        r: LEVEL[usize::from(r & 7)],
        g: LEVEL[usize::from(g & 7)],
        b: LEVEL[usize::from(b & 7)],
    }
}

/// A G7 colour byte `GGGRRRBB`; the two blue bits are the steps 0, 2, 4, 7.
pub fn rgb332(v: u8) -> Rgb8 {
    const BLUE: [u8; 4] = [0, 2, 4, 7];
    rgb777(v >> 2 & 7, v >> 5, BLUE[usize::from(v & 3)])
}

/// A palette entry as written (`0RRR0BBB` low, `00000GGG` high).
fn entry(r: u8, g: u8, b: u8) -> u16 {
    u16::from(g) << 8 | u16::from(r) << 4 | u16::from(b)
}

fn entry_rgb(e: u16) -> Rgb8 {
    rgb777((e >> 4) as u8 & 7, (e >> 8) as u8 & 7, e as u8 & 7)
}

/// Pixel codes of a line before they become colours.
const OPAQUE: u16 = 0x800; // | palette index, never transparent (sprites)
const DIRECT: u16 = 0x100; // | G7 colour byte
const G7_SPRITE: u16 = 0x200; // | one of the 16 fixed sprite colours
const BORDER: u16 = 0x400;

/// No sprite on a pixel.
const NONE: u8 = 0xff;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V9938 {
    pub vram: Vec<u8>,
    /// R0–R46 (R24–R31 do not exist).
    pub regs: [u8; 47],
    /// The 16 palette entries: `00000GGG 0RRR0BBB`.
    pub palette: [u16; 16],
    /// Status register 0.
    pub status: u8,
    /// The line interrupt flag (S1 bit 0).
    fh: bool,
    /// The low 14 bits of the VRAM address.
    pub addr: u16,
    read_buffer: u8,
    /// The first byte of a control pair, of a palette pair.
    latch: Option<u8>,
    palette_latch: Option<u8>,
    /// Beam: row (0 = the picture's first) and clock in the line.
    pub line: u16,
    pub dot: u16,
    /// Fields since power-on.
    pub fields: u64,
    /// NT as taken over at the start of this field, and at power-on.
    pal: bool,
    pal_default: bool,
    /// The second field of a pair (S2's EO).
    odd: bool,
    /// The blink phase (T2's blink colours, the even page) and the fields
    /// left in it.
    blink_on: bool,
    blink_left: u8,
    /// The first collision's place (S3–S6).
    collision: (u16, u16),
    cmd: cmd::State,
    frame: Vec<Rgb8>,
    /// Debug overlay records (not machine state, not in savestates).
    ov: overlay::Records,
}

impl V9938 {
    /// `pal`: R9's NT bit after reset (a 50 Hz machine).
    pub fn new(pal: bool) -> Self {
        let mut v = Self {
            vram: vec![0; VRAM],
            regs: [0; 47],
            palette: [0; 16],
            status: 0,
            fh: false,
            addr: 0,
            read_buffer: 0,
            latch: None,
            palette_latch: None,
            line: 0,
            dot: 0,
            fields: 0,
            pal,
            pal_default: pal,
            odd: false,
            blink_on: false,
            blink_left: 0,
            collision: (0, 0),
            cmd: cmd::State::default(),
            frame: vec![Rgb8::default(); (WIDTH * HEIGHT_PAL) as usize],
            ov: overlay::Records::default(),
        };
        v.reset();
        v
    }

    /// RESET: the registers cleared (NT as the machine's standard), the
    /// palette back to its defaults, a running command stopped; the beam
    /// keeps running, VRAM keeps its contents.
    pub fn reset(&mut self) {
        self.regs = [0; 47];
        self.regs[9] = if self.pal_default { 2 } else { 0 };
        for (p, (r, g, b)) in self.palette.iter_mut().zip(DEFAULT_PALETTE) {
            *p = entry(r, g, b);
        }
        self.status = 0;
        self.fh = false;
        self.latch = None;
        self.palette_latch = None;
        self.collision = (0, 0);
        self.cmd = cmd::State::default();
    }

    /// The picture: [`WIDTH`] × [`V9938::height`].
    pub fn frame(&self) -> &[Rgb8] {
        &self.frame[..(WIDTH * self.height()) as usize]
    }

    /// 313 lines a field.
    pub fn is_pal(&self) -> bool {
        self.pal
    }

    /// NT after reset, as given to [`V9938::new`].
    pub fn pal_default(&self) -> bool {
        self.pal_default
    }

    pub fn lines(&self) -> u16 {
        if self.pal {
            313
        } else {
            262
        }
    }

    /// Picture rows.
    pub fn height(&self) -> u32 {
        if self.pal {
            HEIGHT_PAL
        } else {
            HEIGHT_NTSC
        }
    }

    /// Active lines: 192, or 212 with LN.
    pub fn active_lines(&self) -> u16 {
        if self.regs[9] & 0x80 != 0 {
            212
        } else {
            192
        }
    }

    /// R18's adjustments in pixels and lines (negative: left, up).
    fn adjust(&self) -> (i16, i16) {
        let n = |v: u8| {
            let v = i16::from(v & 15);
            if v < 8 {
                -v
            } else {
                16 - v
            }
        };
        (n(self.regs[18]), n(self.regs[18] >> 4))
    }

    /// The row of the first active line.
    pub fn display_start(&self) -> u16 {
        let top: i16 = if self.pal { 54 } else { 27 };
        let ln = if self.regs[9] & 0x80 != 0 { 10 } else { 0 };
        (top - ln + self.adjust().1) as u16
    }

    /// The picture row showing raster line `line`, if it is shown.
    pub fn line_row(&self, line: u16) -> Option<u32> {
        (u32::from(line) < self.height()).then_some(u32::from(line))
    }

    /// The display mode, M5…M1 (see [`mode`]).
    pub fn mode(&self) -> u8 {
        let r1 = self.regs[1];
        (self.regs[0] & 0x0e) << 1 | (r1 >> 2 & 2) | (r1 >> 4 & 1)
    }

    /// /INT asserted.
    pub fn int(&self) -> bool {
        (self.status & status::F != 0 && self.regs[1] & 0x20 != 0)
            || (self.fh && self.regs[0] & 0x10 != 0)
    }

    /// The whole VRAM address.
    pub fn vram_addr(&self) -> usize {
        usize::from(self.regs[14] & 7) << 14 | usize::from(self.addr)
    }

    fn advance(&mut self) {
        self.addr = (self.addr + 1) & 0x3fff;
        if self.addr == 0 && self.regs[0] & 0x0c != 0 {
            self.regs[14] = (self.regs[14] + 1) & 7;
        }
    }

    /// Port 0 read.
    pub fn read_data(&mut self) -> u8 {
        self.latch = None;
        let v = self.read_buffer;
        self.read_buffer = self.vram[self.vram_addr()];
        self.advance();
        v
    }

    /// Port 0 write.
    pub fn write_data(&mut self, v: u8) {
        self.latch = None;
        let a = self.vram_addr();
        self.vram[a] = v;
        self.read_buffer = v;
        self.advance();
    }

    /// Port 1 write.
    pub fn write_control(&mut self, v: u8) {
        let Some(first) = self.latch.take() else {
            self.latch = Some(v);
            return;
        };
        if v & 0x80 != 0 {
            if v & 0x40 == 0 {
                self.write_reg(v & 0x3f, first);
            }
            return;
        }
        self.addr = u16::from(v & 0x3f) << 8 | u16::from(first);
        if v & 0x40 == 0 {
            self.read_buffer = self.vram[self.vram_addr()];
            self.advance();
        }
    }

    /// Port 2 write: the palette.
    pub fn write_palette(&mut self, v: u8) {
        let Some(first) = self.palette_latch.take() else {
            self.palette_latch = Some(v);
            return;
        };
        let n = usize::from(self.regs[16] & 15);
        self.palette[n] = u16::from(v & 7) << 8 | u16::from(first & 0x77);
        self.regs[16] = (self.regs[16] + 1) & 15;
    }

    /// Port 3 write: the register R17 points at.
    pub fn write_indirect(&mut self, v: u8) {
        let n = self.regs[17] & 0x3f;
        if n != 17 {
            self.write_reg(n, v);
        }
        if self.regs[17] & 0x80 == 0 {
            self.regs[17] = (n + 1) & 0x3f;
        }
    }

    /// A register write.
    pub fn write_reg(&mut self, n: u8, v: u8) {
        let n = usize::from(n);
        if n >= self.regs.len() || (24..32).contains(&n) {
            return;
        }
        match n {
            0 => {
                self.regs[0] = v;
                if v & 0x10 == 0 {
                    self.fh = false;
                }
            }
            14 => self.regs[14] = v & 7,
            15 => self.regs[15] = v & 15,
            16 => {
                self.regs[16] = v & 15;
                self.palette_latch = None;
            }
            44 => {
                self.regs[44] = v;
                self.cmd_data(v);
            }
            46 => {
                self.regs[46] = v;
                self.cmd_start(v);
            }
            _ => self.regs[n] = v,
        }
    }

    /// Status register `n` without the effects of reading it.
    pub fn status_peek(&self, n: u8) -> u8 {
        match n {
            0 => self.status,
            1 => u8::from(self.fh),
            2 => {
                let start = self.display_start();
                let vr = self.line < start || self.line >= start + self.active_lines();
                let hr = !(ACTIVE_START..ACTIVE_END).contains(&self.dot);
                u8::from(self.cmd.tr) << 7
                    | u8::from(vr) << 6
                    | u8::from(hr) << 5
                    | u8::from(self.cmd.bd) << 4
                    | 0x0c
                    | u8::from(self.odd) << 1
                    | u8::from(self.cmd.op != 0)
            }
            3 => self.collision.0 as u8,
            4 => (self.collision.0 >> 8) as u8 | 0xfe,
            5 => self.collision.1 as u8,
            6 => (self.collision.1 >> 8) as u8 | 0xfc,
            7 => self.cmd.color,
            8 => self.cmd.found_x as u8,
            9 => (self.cmd.found_x >> 8) as u8 | 0xfe,
            _ => 0xff,
        }
    }

    /// Port 1 read: the status register R15 selects.
    pub fn read_status(&mut self) -> u8 {
        self.latch = None;
        let n = self.regs[15] & 15;
        let v = self.status_peek(n);
        match n {
            0 => self.status &= !(status::F | status::FIFTH | status::COINCIDENCE),
            1 => self.fh = false,
            5 => self.collision = (0, 0),
            7 => self.cmd_read(),
            _ => {}
        }
        v
    }

    /// Side-effect-free reads for the debugger: (data buffer, the selected
    /// status register).
    pub fn peek(&self) -> (u8, u8) {
        (self.read_buffer, self.status_peek(self.regs[15] & 15))
    }

    /// One clock. Returns true at the end of a field.
    pub fn tick(&mut self) -> bool {
        if self.dot == ACTIVE_START {
            self.draw_line();
        } else if self.dot == ACTIVE_END {
            // The line interrupt: lines count from the first one shown.
            let y = self.line.wrapping_sub(self.display_start());
            if y < 256 && y as u8 == self.regs[19].wrapping_sub(self.regs[23]) {
                self.fh = true;
            }
        }
        if self.cmd.op != 0 {
            self.cmd_run(4);
        }
        self.dot += 1;
        if self.dot < LINE {
            return false;
        }
        self.dot = 0;
        self.line += 1;
        if self.regs[0] & 0x10 == 0 {
            self.fh = false;
        }
        if self.line == self.display_start() + self.active_lines() {
            self.status |= status::F;
        }
        if self.line < self.lines() {
            return false;
        }
        self.line = 0;
        self.fields += 1;
        self.pal = self.regs[9] & 2 != 0;
        self.odd = !self.odd;
        self.blink();
        true
    }

    /// The blink phase, once a field: R13's on and off times in units of
    /// ten fields; a time of 0 holds the other phase.
    fn blink(&mut self) {
        let (on, off) = (self.regs[13] >> 4, self.regs[13] & 15);
        if on == 0 || off == 0 {
            self.blink_on = on != 0;
            self.blink_left = 0;
            return;
        }
        if self.blink_left > 1 {
            self.blink_left -= 1;
            return;
        }
        self.blink_on = !self.blink_on;
        self.blink_left = 10 * if self.blink_on { on } else { off };
    }

    fn v(&self, a: usize) -> u8 {
        self.vram[a & (VRAM - 1)]
    }

    fn name_base(&self) -> usize {
        usize::from(self.regs[2] & 0x7f) << 10
    }

    fn color_base(&self) -> usize {
        usize::from(self.regs[10] & 7) << 14 | usize::from(self.regs[3]) << 6
    }

    fn pattern_base(&self) -> usize {
        usize::from(self.regs[4] & 0x3f) << 11
    }

    fn sprite_attr_base(&self) -> usize {
        usize::from(self.regs[11] & 3) << 15 | usize::from(self.regs[5]) << 7
    }

    fn sprite_pattern_base(&self) -> usize {
        usize::from(self.regs[6] & 0x3f) << 11
    }

    /// The sprite mode of display mode `m`: 0 none, 1, 2.
    pub fn sprite_mode(m: u8) -> u8 {
        match m {
            mode::G1 | mode::MC | mode::G2 => 1,
            mode::G3 | mode::G4 | mode::G5 | mode::G6 | mode::G7 => 2,
            _ => 0,
        }
    }

    /// The first byte of the bitmap page shown in mode `m` now.
    fn page_base(&self, m: u8) -> usize {
        let mut page = usize::from(self.regs[2] >> 5 & 3);
        if (self.regs[9] & 4 != 0 && !self.odd) || self.blink_on {
            page &= !1;
        }
        match m {
            mode::G6 | mode::G7 => (page & 1) << 16,
            _ => page << 15,
        }
    }

    /// The palette as colours.
    pub fn colors(&self) -> [Rgb8; 16] {
        self.palette.map(entry_rgb)
    }

    /// The backdrop of the two halves of a pixel.
    fn border(&self, m: u8, pal: &[Rgb8; 16]) -> [Rgb8; 2] {
        let r7 = self.regs[7];
        match m {
            mode::G7 => [rgb332(r7); 2],
            mode::G5 => [pal[usize::from(r7 >> 2 & 3)], pal[usize::from(r7 & 3)]],
            _ => [pal[usize::from(r7 & 15)]; 2],
        }
    }

    /// Draw the current line into the picture (borders included).
    fn draw_line(&mut self) {
        if u32::from(self.line) >= self.height() {
            return;
        }
        let m = self.mode();
        let pal = self.colors();
        let border = self.border(m, &pal);
        let mut out = [border[0]; WIDTH as usize];
        for p in out.iter_mut().skip(1).step_by(2) {
            *p = border[1];
        }
        let y = self.line.wrapping_sub(self.display_start());
        if y < self.active_lines() && self.regs[1] & 0x40 != 0 {
            let ly = (y as u8).wrapping_add(self.regs[23]);
            let mut px = [BORDER; 512];
            self.background(m, ly, self.page_base(m), &mut px);
            let x0 = (LEFT as i16 + 2 * self.adjust().0) as usize;
            if Self::sprite_mode(m) != 0 && self.regs[8] & 2 == 0 {
                let mut spr = [NONE; 256];
                if Self::sprite_mode(m) == 1 {
                    self.sprites1(ly, y, x0, &mut spr);
                } else {
                    self.sprites2(ly, y, x0, &mut spr);
                }
                for (x, &c) in spr.iter().enumerate() {
                    if c == NONE {
                        continue;
                    }
                    let pair = match m {
                        mode::G7 => [G7_SPRITE | u16::from(c); 2],
                        mode::G5 => [OPAQUE | u16::from(c >> 2), OPAQUE | u16::from(c & 3)],
                        _ => [OPAQUE | u16::from(c); 2],
                    };
                    px[2 * x..2 * x + 2].copy_from_slice(&pair);
                }
            }
            let tp = self.regs[8] & 0x20 != 0;
            for (i, &code) in px.iter().enumerate() {
                out[x0 + i] = resolve(code, tp, border[(x0 + i) & 1], &pal);
            }
        }
        let base = usize::from(self.line) * WIDTH as usize;
        self.frame[base..base + WIDTH as usize].copy_from_slice(&out);
    }

    /// VRAM line `ly` of mode `m` as 512 pixel codes; `page` is the first
    /// byte of the bitmap page.
    fn background(&self, m: u8, ly: u8, page: usize, px: &mut [u16; 512]) {
        let y = usize::from(ly);
        match m {
            mode::G1 | mode::G2 | mode::G3 | mode::MC => {
                let names = self.name_base() + (y / 8) * 32;
                let patterns = self.pattern_base();
                let colors = self.color_base();
                for col in 0..32 {
                    let n = usize::from(self.v(names + col));
                    let (pattern, color) = match m {
                        mode::G1 => (self.v(patterns + n * 8 + (y & 7)), self.v(colors + n / 8)),
                        mode::MC => {
                            let row = ((y / 8) & 3) * 2 + (y & 7) / 4;
                            (0xf0, self.v(patterns + n * 8 + row))
                        }
                        _ => {
                            let offset = ((y / 64) << 8 | n) << 3 | (y & 7);
                            (
                                self.v((patterns & 0x1_e000)
                                    | (offset & ((patterns & 0x1800) | 0x7ff))),
                                self.v((colors & 0x1_e000) | (offset & ((colors & 0x1fc0) | 0x3f))),
                            )
                        }
                    };
                    for i in 0..8 {
                        let c = if pattern & (0x80 >> i) != 0 {
                            color >> 4
                        } else {
                            color & 15
                        };
                        px[col * 16 + 2 * i] = u16::from(c);
                        px[col * 16 + 2 * i + 1] = u16::from(c);
                    }
                }
            }
            mode::T1 => {
                let names = self.name_base() + (y / 8) * 40;
                let patterns = self.pattern_base();
                let (fg, bg) = (self.regs[7] >> 4, self.regs[7] & 15);
                for col in 0..40 {
                    let n = usize::from(self.v(names + col));
                    let pattern = self.v(patterns + n * 8 + (y & 7));
                    for i in 0..6 {
                        let c = if pattern & (0x80 >> i) != 0 { fg } else { bg };
                        let x = 16 + (col * 6 + i) * 2;
                        px[x] = u16::from(c);
                        px[x + 1] = u16::from(c);
                    }
                }
            }
            mode::T2 => {
                let names = (usize::from(self.regs[2] & 0x7c) << 10) + (y / 8) * 80;
                let blinks = (self.color_base() & 0x1_fe00) + (y / 8) * 10;
                let patterns = self.pattern_base();
                for col in 0..80 {
                    let n = usize::from(self.v(names + col));
                    let pattern = self.v(patterns + n * 8 + (y & 7));
                    let blink =
                        self.blink_on && self.v(blinks + col / 8) & (0x80 >> (col % 8)) != 0;
                    let color = self.regs[if blink { 12 } else { 7 }];
                    for i in 0..6 {
                        let c = if pattern & (0x80 >> i) != 0 {
                            color >> 4
                        } else {
                            color & 15
                        };
                        px[16 + col * 6 + i] = u16::from(c);
                    }
                }
            }
            mode::G4 => {
                let line = page + y * 128;
                for (i, pair) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let b = self.v(line + i);
                    pair[..2].fill(u16::from(b >> 4));
                    pair[2..].fill(u16::from(b & 15));
                }
            }
            mode::G5 => {
                let line = page + y * 128;
                for (i, four) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let b = self.v(line + i);
                    for (k, p) in four.iter_mut().enumerate() {
                        *p = u16::from(b >> (6 - 2 * k) & 3);
                    }
                }
            }
            mode::G6 => {
                let line = page + y * 256;
                for (i, pair) in px.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let b = self.v(line + i);
                    pair[0] = u16::from(b >> 4);
                    pair[1] = u16::from(b & 15);
                }
            }
            mode::G7 => {
                let line = page + y * 256;
                for (i, pair) in px.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    pair.fill(DIRECT | u16::from(self.v(line + i)));
                }
            }
            _ => {}
        }
    }

    /// The sprites' size in pixels and their magnification.
    fn sprite_size(&self) -> (usize, usize) {
        (
            if self.regs[1] & 2 != 0 { 16 } else { 8 },
            if self.regs[1] & 1 != 0 { 2 } else { 1 },
        )
    }

    /// The pattern bits of sprite line `row`, 16 wide (8×8: the high byte).
    fn sprite_row(&self, name: u8, row: usize, size: usize) -> u16 {
        let patterns = self.sprite_pattern_base();
        if size == 16 {
            let a = patterns + usize::from(name & 0xfc) * 8 + row;
            u16::from(self.v(a)) << 8 | u16::from(self.v(a + 16))
        } else {
            u16::from(self.v(patterns + usize::from(name) * 8 + row)) << 8
        }
    }

    fn collide(&mut self, x: usize, y: u16) {
        if self.status & status::COINCIDENCE == 0 {
            self.collision = (x as u16 + 12, y + 8);
        }
        self.status |= status::COINCIDENCE;
    }

    /// Sprite mode 1 on VRAM line `ly` (display line `y`, the active
    /// area's first picture column `x0`); sets 5S and C.
    fn sprites1(&mut self, ly: u8, y: u16, x0: usize, out: &mut [u8; 256]) {
        let attrs = self.sprite_attr_base();
        let (size, mag) = self.sprite_size();
        let tp = self.regs[8] & 0x20 != 0;
        let mut shown = 0;
        let mut last = 0u8;
        let mut owner = [false; 256];
        for n in 0..32u8 {
            let a = attrs + usize::from(n) * 4;
            let sy = self.v(a);
            if sy == 208 {
                break;
            }
            last = n;
            let dy = usize::from(ly.wrapping_sub(sy).wrapping_sub(1));
            if dy >= size * mag {
                continue;
            }
            if shown == 4 {
                self.ov.limit(self.line);
                if self.status & status::FIFTH == 0 {
                    self.status = (self.status & 0xe0) | status::FIFTH | n;
                }
                return;
            }
            shown += 1;
            let color_byte = self.v(a + 3);
            let color = color_byte & 15;
            let sx = i32::from(self.v(a + 1)) - if color_byte & 0x80 != 0 { 32 } else { 0 };
            self.ov
                .sprite(n, self.line, x0 as i32 + 2 * sx, 2 * (size * mag) as u32);
            let bits = self.sprite_row(self.v(a + 2), dy / mag, size);
            for p in 0..size * mag {
                let x = sx + p as i32;
                if !(0..256).contains(&x) || bits & (0x8000 >> (p / mag)) == 0 {
                    continue;
                }
                let x = x as usize;
                if owner[x] {
                    self.collide(x, y);
                    continue;
                }
                owner[x] = true;
                if color != 0 || tp {
                    out[x] = color;
                }
            }
        }
        if self.status & status::FIFTH == 0 {
            self.status = (self.status & 0xe0) | last;
        }
    }

    /// Sprite mode 2 on VRAM line `ly`; sets 5S (a ninth sprite) and C.
    fn sprites2(&mut self, ly: u8, y: u16, x0: usize, out: &mut [u8; 256]) {
        let colors = self.sprite_attr_base() & 0x1_fc00;
        let attrs = colors + 0x200;
        let (size, mag) = self.sprite_size();
        let tp = self.regs[8] & 0x20 != 0;
        let mut shown = 0;
        let mut last = 0u8;
        // Per pixel: the sprite group that owns it (main sprite + 1) and
        // whether a colliding sprite has a set bit there.
        let mut owner = [0u8; 256];
        let mut solid = [false; 256];
        let mut group = 0u8;
        for n in 0..32u8 {
            let a = attrs + usize::from(n) * 4;
            let sy = self.v(a);
            if sy == 216 {
                break;
            }
            last = n;
            let dy = usize::from(ly.wrapping_sub(sy).wrapping_sub(1));
            if dy >= size * mag {
                continue;
            }
            if shown == 8 {
                self.ov.limit(self.line);
                if self.status & status::FIFTH == 0 {
                    self.status = (self.status & 0xe0) | status::FIFTH | n;
                }
                return;
            }
            shown += 1;
            let row = dy / mag;
            let cb = self.v(colors + usize::from(n) * 16 + row);
            let color = cb & 15;
            let cc = cb & 0x40 != 0;
            let sx = i32::from(self.v(a + 1)) - if cb & 0x80 != 0 { 32 } else { 0 };
            self.ov
                .sprite(n, self.line, x0 as i32 + 2 * sx, 2 * (size * mag) as u32);
            if !cc {
                group = n + 1;
            } else if group == 0 {
                // No sprite before it to share a priority with.
                continue;
            }
            let bits = self.sprite_row(self.v(a + 2), row, size);
            for p in 0..size * mag {
                let x = sx + p as i32;
                if !(0..256).contains(&x) || bits & (0x8000 >> (p / mag)) == 0 {
                    continue;
                }
                let x = x as usize;
                if !cc && cb & 0x20 == 0 && (color != 0 || tp) {
                    if solid[x] {
                        self.collide(x, y);
                    }
                    solid[x] = true;
                }
                if owner[x] == group && cc {
                    out[x] |= color;
                } else if owner[x] == 0 && (color != 0 || tp) {
                    owner[x] = group;
                    out[x] = color;
                }
            }
        }
        if self.status & status::FIFTH == 0 {
            self.status = (self.status & 0xe0) | last;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"V938", 1, |w| {
            w.bytes(&self.vram);
            w.raw(&self.regs);
            for p in self.palette {
                w.u16(p);
            }
            w.u8(self.status);
            w.bool(self.fh);
            w.u16(self.addr);
            w.u8(self.read_buffer);
            for l in [self.latch, self.palette_latch] {
                w.bool(l.is_some());
                w.u8(l.unwrap_or(0));
            }
            w.u16(self.line);
            w.u16(self.dot);
            w.u64(self.fields);
            w.bool(self.pal);
            w.bool(self.odd);
            w.bool(self.blink_on);
            w.u8(self.blink_left);
            w.u16(self.collision.0);
            w.u16(self.collision.1);
            self.cmd.save(w);
        });
    }

    /// The picture is redrawn, not loaded.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"V938", 1)?;
        let vram = c.bytes()?;
        if vram.len() != VRAM {
            return Err(StateError::Invalid("VRAM size"));
        }
        let mut regs = [0u8; 47];
        c.raw_into(&mut regs)?;
        let mut palette = [0u16; 16];
        for p in &mut palette {
            *p = c.u16()? & 0x777;
        }
        let status = c.u8()?;
        let fh = c.bool()?;
        let addr = c.u16()? & 0x3fff;
        let read_buffer = c.u8()?;
        let mut latches = [None; 2];
        for l in &mut latches {
            let set = c.bool()?;
            let v = c.u8()?;
            *l = set.then_some(v);
        }
        let line = c.u16()?;
        let dot = c.u16()?;
        let fields = c.u64()?;
        let pal = c.bool()?;
        let odd = c.bool()?;
        let blink_on = c.bool()?;
        let blink_left = c.u8()?;
        let collision = (c.u16()?, c.u16()?);
        let cmd = cmd::State::load(&mut c)?;
        if line >= if pal { 313 } else { 262 } || dot >= LINE {
            return Err(StateError::Invalid("VDP beam position"));
        }
        regs[14] &= 7;
        regs[15] &= 15;
        regs[16] &= 15;
        self.vram = vram.to_vec();
        self.regs = regs;
        self.palette = palette;
        self.status = status;
        self.fh = fh;
        self.addr = addr;
        self.read_buffer = read_buffer;
        [self.latch, self.palette_latch] = latches;
        self.line = line;
        self.dot = dot;
        self.fields = fields;
        self.pal = pal;
        self.odd = odd;
        self.blink_on = blink_on;
        self.blink_left = blink_left;
        self.collision = collision;
        self.cmd = cmd;
        Ok(())
    }
}

/// A pixel code as a colour; `border` is the backdrop of this half pixel.
fn resolve(code: u16, tp: bool, border: Rgb8, pal: &[Rgb8; 16]) -> Rgb8 {
    if code & BORDER != 0 {
        border
    } else if code & OPAQUE != 0 {
        pal[usize::from(code & 15)]
    } else if code & DIRECT != 0 {
        rgb332(code as u8)
    } else if code & G7_SPRITE != 0 {
        let (r, g, b) = G7_SPRITE_COLORS[usize::from(code & 15)];
        rgb777(r, g, b)
    } else if code == 0 && !tp {
        border
    } else {
        pal[usize::from(code & 15)]
    }
}

#[cfg(test)]
mod tests;
