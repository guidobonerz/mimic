//! The Master System / Game Gear VDP (315-5124 of the SMS, 315-5246 of the
//! SMS 2, 315-5378 of the Game Gear): the TMS9918's display modes plus
//! mode 4.
//!
//! **Ports** (the machine decodes them): data (read: the read buffer, then
//! the next VRAM byte; write: VRAM or CRAM by the code register, also into
//! the buffer), control (write: the two-byte command word — the address
//! register's 14 bits and the 2-bit code: 0 VRAM read, 1 VRAM write, 2 a
//! register write `10?? RRRR` with the data in the first byte, 3 CRAM
//! write; read: the status `INT OVR COL -----`, which clears the flags,
//! the line interrupt and the first-byte flag), the V counter and the H
//! counter.
//!
//! **CRAM.** SMS: 32 bytes `--BBGGRR`, two palettes of 16 (sprites use the
//! second). Game Gear: 32 words `----BBBBGGGGRRRR`; a write to an even
//! address only latches the byte, the odd one writes the word.
//!
//! **Mode 4.** A 32 × 28 name table of words `---pcvhnnnnnnnnn` (32 × 32
//! for the 224- and 240-line screens, at register 2's address | $0700),
//! 4-plane 8 × 8 patterns (32 bytes, the four planes of a row next to each
//! other); horizontal scroll (register 8, the left `fine` pixels are the
//! backdrop), vertical scroll (register 9, latched at the end of the
//! active display, wrapping at 224 or 256), the locks of register 0 (bit 6
//! no horizontal scroll on lines 0–15, bit 7 no vertical scroll for
//! columns 24–31), bit 5 masking column 0. 64 sprites from the attribute
//! table (Y, then X and pattern pairs from offset $80), eight a line (a
//! ninth sets OVR), 8 × 8 or 8 × 16 (register 1 bit 1), zoomed by register
//! 1 bit 0 (the SMS VDP zooms only the first four horizontally), bit 3 of
//! register 0 shifts them 8 pixels left, Y = $D0 ends the list on a
//! 192-line screen, overlapping opaque pixels set COL; the background's
//! priority bit puts its opaque pixels over the sprites.
//!
//! **Interrupts.** The frame interrupt flag rises on line $C1 (224 lines:
//! $E1, 240: $F1) and drives /INT while register 1 bit 5 is set; the line
//! counter counts down on lines 0 to the active height (reloaded from
//! register 10 on the others) and on underflow raises the line interrupt,
//! which drives /INT while register 0 bit 4 is set.
//!
//! **Timing.** 342 pixel clocks a line (three per two Z80 T-states), 262
//! lines (PAL 313); line 0 is the first active line. The V counter jumps
//! as MacDonald's tables give it (NTSC 192: $00–$DA, $D5–$FF).
//!
//! Quirks of the SMS VDP (not the SMS 2 / Game Gear): register 2 bit 0
//! ANDs bit 4 of the name table row (the Japanese Y's), register 5 bit 0
//! clear takes X and pattern from the table's lower half, register 6 bits
//! 1–0 clear mask pattern bits 8 and 6, the 224/240-line screens do not
//! exist, only four zoomed sprites.
//!
//! Sources: Charles MacDonald, *Sega Master System VDP documentation*
//! (2002-11-12) and *SMS/GG hardware notes*; Texas Instruments TMS9918A
//! data for the legacy modes. The implementation is this project's own.

use mimic_core::frame::Rgb8;
use mimic_core::overlay::{bound_boxes, Overlay, OverlayInfo, Shape, SpriteLine};
use mimic_core::savestate::{StateError, StateReader, StateWriter};

/// Pixel clocks per line.
pub const LINE: u16 = 342;
/// Left border pixels in the picture, the active 256, the right border.
pub const LEFT: u32 = 13;
pub const WIDTH: u32 = 13 + 256 + 15;
/// The Game Gear's LCD: the middle 160 × 144 of a 256 × 192 screen.
pub const GG_WIDTH: u32 = 160;
pub const GG_HEIGHT: u32 = 144;
const GG_X: usize = 48;
const GG_Y: u16 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VdpKind {
    /// 315-5124 (Mark III, SMS).
    Sms1,
    /// 315-5246 (SMS 2): the extended resolutions, the quirks fixed.
    Sms2,
    /// 315-5378 (Game Gear): 12-bit CRAM, the 160 × 144 window.
    GameGear,
}

/// The TMS9918's fixed colours, which the SMS 2 shows in the legacy modes.
const TMS_PALETTE: [u32; 16] = [
    0x000000, 0x000000, 0x21c842, 0x5edc78, 0x5455ed, 0x7d76fc, 0xd4524d, 0x42ebf5, 0xfc5554,
    0xff7978, 0xd4c154, 0xe6ce80, 0x21b03b, 0xc95bba, 0xcccccc, 0xffffff,
];

const fn rgb(c: u32) -> Rgb8 {
    Rgb8 {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
    }
}

/// The overlays the VDP offers.
pub fn overlay_infos(chip: &str) -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "sprites",
            name: format!("{chip} sprite boxes"),
        },
        OverlayInfo {
            id: "sprite_bounds",
            name: format!("{chip} sprite bounds"),
        },
    ]
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Overlays {
    boxes: bool,
    bounds: bool,
    cur: Vec<SpriteLine>,
    last: Vec<SpriteLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmsVdp {
    pub kind: VdpKind,
    pal: bool,
    pub vram: Vec<u8>,
    /// SMS: bytes (low 6 bits); Game Gear: 12-bit words.
    pub cram: [u16; 32],
    cram_latch: u8,
    pub regs: [u8; 11],
    pub addr: u16,
    pub code: u8,
    /// The first byte of a command word, waiting for the second.
    latch: Option<u8>,
    read_buffer: u8,
    /// INT, OVR, COL (bits 7–5).
    pub status: u8,
    line_int: bool,
    line_counter: u8,
    /// Register 9 as the frame uses it.
    vscroll: u8,
    pub line: u16,
    pub dot: u16,
    pub fields: u64,
    h_latch: u8,
    frame: Vec<Rgb8>,
    ov: Overlays,
}

impl SmsVdp {
    pub fn new(kind: VdpKind, pal: bool) -> Self {
        let (w, h) = Self::frame_size(kind, pal);
        Self {
            kind,
            pal,
            vram: vec![0; 0x4000],
            cram: [0; 32],
            cram_latch: 0,
            regs: [0; 11],
            addr: 0,
            code: 0,
            latch: None,
            read_buffer: 0,
            status: 0,
            line_int: false,
            line_counter: 0,
            vscroll: 0,
            line: 0,
            dot: 0,
            fields: 0,
            h_latch: 0,
            frame: vec![Rgb8::default(); (w * h) as usize],
            ov: Overlays::default(),
        }
    }

    fn frame_size(kind: VdpKind, pal: bool) -> (u32, u32) {
        match kind {
            VdpKind::GameGear => (GG_WIDTH, GG_HEIGHT),
            _ => (WIDTH, if pal { 294 } else { 243 }),
        }
    }

    /// The picture: width, height, pixels.
    pub fn frame(&self) -> (u32, u32, &[Rgb8]) {
        let (w, h) = Self::frame_size(self.kind, self.pal);
        (w, h, &self.frame)
    }

    pub fn chip_name(&self) -> &'static str {
        match self.kind {
            VdpKind::Sms1 => "SMS VDP",
            VdpKind::Sms2 => "SMS 2 VDP",
            VdpKind::GameGear => "GG VDP",
        }
    }

    pub fn lines(&self) -> u16 {
        if self.pal {
            313
        } else {
            262
        }
    }

    /// Register reset: the Z80 and the VDP restart, VRAM and CRAM stay.
    pub fn reset(&mut self) {
        self.regs = [0; 11];
        self.latch = None;
        self.status = 0;
        self.line_int = false;
    }

    fn mode4(&self) -> bool {
        self.regs[0] & 0x04 != 0
    }

    /// The active lines: 192, or 224 / 240 on the SMS 2 and Game Gear (M2
    /// and M1 or M3 in mode 4; the Game Gear's LCD stays 192 in GG mode).
    pub fn height(&self) -> u16 {
        if !self.mode4() || self.kind == VdpKind::Sms1 || self.regs[0] & 0x02 == 0 {
            return 192;
        }
        match (self.regs[1] & 0x10 != 0, self.regs[1] & 0x08 != 0) {
            (true, false) => 224,
            (false, true) if self.pal => 240,
            _ => 192,
        }
    }

    /// The borders above and below the active lines in the picture.
    fn borders(&self) -> (u16, u16) {
        match (self.pal, self.height()) {
            (false, 224) => (11, 8),
            (false, _) => (27, 24),
            (true, 224) => (38, 32),
            (true, 240) => (30, 24),
            (true, _) => (54, 48),
        }
    }

    /// The picture row of `line`, if it has one.
    pub fn line_row(&self, line: u16) -> Option<u32> {
        if self.kind == VdpKind::GameGear {
            return (GG_Y..GG_Y + GG_HEIGHT as u16)
                .contains(&line)
                .then(|| u32::from(line - GG_Y));
        }
        let (top, bottom) = self.borders();
        let h = self.height();
        let total = self.lines();
        if line < h + bottom {
            Some(u32::from(top + line))
        } else if line >= total - top {
            Some(u32::from(line - (total - top)))
        } else {
            None
        }
    }

    /// The V counter as the CPU reads it (MacDonald's tables).
    pub fn v_counter(&self) -> u8 {
        let l = self.line;
        let total = self.lines();
        let jump = match (self.pal, self.height()) {
            (false, 192) => 0xda,
            (false, 224) => 0xea,
            (false, _) => 0x106,
            (true, 192) => 0xf2,
            (true, 224) => 0x102,
            (true, _) => 0x10a,
        };
        if l <= jump {
            l as u8
        } else {
            (l - (total - 256)) as u8
        }
    }

    /// The H counter latched at the last TH change ([`SmsVdp::latch_h`]).
    pub fn h_counter(&self) -> u8 {
        self.h_latch
    }

    /// A TH pin changed: the H counter is latched (the upper 8 of 9 bits;
    /// the active display starts at 0).
    pub fn latch_h(&mut self) {
        self.h_latch = (self.dot >> 1) as u8;
    }

    /// /INT.
    pub fn int(&self) -> bool {
        (self.status & 0x80 != 0 && self.regs[1] & 0x20 != 0)
            || (self.line_int && self.regs[0] & 0x10 != 0)
    }

    pub fn read_data(&mut self) -> u8 {
        self.latch = None;
        let v = self.read_buffer;
        self.read_buffer = self.vram[usize::from(self.addr)];
        self.addr = (self.addr + 1) & 0x3fff;
        v
    }

    pub fn write_data(&mut self, v: u8) {
        self.latch = None;
        if self.code == 3 {
            if self.kind == VdpKind::GameGear {
                let a = usize::from(self.addr & 0x3f);
                if a & 1 == 0 {
                    self.cram_latch = v;
                } else {
                    self.cram[a >> 1] = (u16::from(v) << 8 | u16::from(self.cram_latch)) & 0x0fff;
                }
            } else {
                self.cram[usize::from(self.addr & 0x1f)] = u16::from(v & 0x3f);
            }
        } else {
            self.vram[usize::from(self.addr)] = v;
        }
        self.read_buffer = v;
        self.addr = (self.addr + 1) & 0x3fff;
    }

    pub fn read_status(&mut self) -> u8 {
        self.latch = None;
        let v = self.status | 0x1f;
        self.status = 0;
        self.line_int = false;
        v
    }

    /// The data buffer and the status, without side effects.
    pub fn peek(&self) -> (u8, u8) {
        (self.read_buffer, self.status | 0x1f)
    }

    pub fn write_control(&mut self, v: u8) {
        let Some(lo) = self.latch.take() else {
            self.latch = Some(v);
            self.addr = (self.addr & 0x3f00) | u16::from(v);
            return;
        };
        self.addr = (u16::from(v & 0x3f) << 8) | u16::from(lo);
        self.code = v >> 6;
        match self.code {
            0 => {
                self.read_buffer = self.vram[usize::from(self.addr)];
                self.addr = (self.addr + 1) & 0x3fff;
            }
            2 => {
                if let Some(r) = self.regs.get_mut(usize::from(v & 0x0f)) {
                    *r = lo;
                }
            }
            _ => {}
        }
    }

    /// One pixel clock. Returns true when a field ended.
    pub fn tick(&mut self) -> bool {
        self.dot += 1;
        if self.dot < LINE {
            return false;
        }
        self.dot = 0;
        self.draw_line();
        self.line += 1;
        let mut ended = false;
        if self.line >= self.lines() {
            self.line = 0;
            self.fields += 1;
            ended = true;
            let ov = &mut self.ov;
            if ov.boxes || ov.bounds {
                ov.last = std::mem::take(&mut ov.cur);
            }
        }
        self.start_line();
        ended
    }

    /// The events at the start of a line: the line counter, the frame
    /// interrupt, the vertical scroll latch.
    fn start_line(&mut self) {
        let h = self.height();
        if self.line <= h {
            let (c, under) = self.line_counter.overflowing_sub(1);
            if under {
                self.line_counter = self.regs[10];
                self.line_int = true;
            } else {
                self.line_counter = c;
            }
        } else {
            self.line_counter = self.regs[10];
        }
        if self.line == h + 1 {
            self.status |= 0x80;
        }
        if self.line == h || self.line == 0 && self.fields <= 1 {
            self.vscroll = self.regs[9];
        }
    }

    fn color(&self, index: usize) -> Rgb8 {
        let c = self.cram[index & 31];
        if self.kind == VdpKind::GameGear {
            let s = |n: u16| ((c >> n) & 15) as u8 * 17;
            Rgb8 {
                r: s(0),
                g: s(4),
                b: s(8),
            }
        } else {
            let s = |n: u16| ((c >> n) & 3) as u8 * 85;
            Rgb8 {
                r: s(0),
                g: s(2),
                b: s(4),
            }
        }
    }

    /// A legacy mode's colour 0–15 (0: transparent → backdrop).
    fn tms_color(&self, c: u8) -> Rgb8 {
        if self.kind == VdpKind::GameGear {
            self.color(16 + usize::from(c & 15))
        } else {
            rgb(TMS_PALETTE[usize::from(c & 15)])
        }
    }

    fn backdrop(&self) -> Rgb8 {
        if self.mode4() {
            self.color(16 + usize::from(self.regs[7] & 15))
        } else {
            self.tms_color(self.regs[7] & 15)
        }
    }

    fn draw_line(&mut self) {
        let Some(row) = self.line_row(self.line) else {
            return;
        };
        let backdrop = self.backdrop();
        let mut line = [backdrop; 256];
        if self.line < self.height() && self.regs[1] & 0x40 != 0 {
            if self.mode4() {
                self.mode4_line(self.line, &mut line);
            } else {
                self.tms_line(self.line, &mut line);
            }
        }
        if self.kind == VdpKind::GameGear {
            let base = row as usize * GG_WIDTH as usize;
            self.frame[base..base + GG_WIDTH as usize]
                .copy_from_slice(&line[GG_X..GG_X + GG_WIDTH as usize]);
        } else {
            let base = row as usize * WIDTH as usize;
            let out = &mut self.frame[base..base + WIDTH as usize];
            out.fill(backdrop);
            out[LEFT as usize..LEFT as usize + 256].copy_from_slice(&line);
        }
    }

    /// Mode 4: the background, then the sprites (see the module doc).
    fn mode4_line(&mut self, y: u16, out: &mut [Rgb8; 256]) {
        let h = self.height();
        let tall = h != 192;
        let names = if tall {
            (usize::from(self.regs[2] & 0x0c) << 10) | 0x700
        } else {
            usize::from(self.regs[2] & 0x0e) << 10
        };
        let hscroll = if self.regs[0] & 0x40 != 0 && y < 16 {
            0
        } else {
            self.regs[8]
        };
        let fine = usize::from(hscroll & 7);
        let wrap = if tall { 256 } else { 224 };
        // Background: colour index and whether it is a priority pixel.
        let mut bg = [(0u8, false); 256];
        for (x, px) in bg.iter_mut().enumerate() {
            if x < fine {
                *px = (0, false);
                continue;
            }
            let column = x >> 3;
            let vscroll = if self.regs[0] & 0x80 != 0 && column >= 24 {
                0
            } else {
                self.vscroll
            };
            let sy = (usize::from(y) + usize::from(vscroll)) % wrap;
            let mut row = sy >> 3;
            if self.kind == VdpKind::Sms1 && self.regs[2] & 1 == 0 {
                row &= !0x10;
            }
            let sx = (x as u8).wrapping_sub(hscroll);
            let col = usize::from(sx >> 3);
            let a = names + row * 64 + col * 2;
            let entry =
                u16::from(self.vram[a & 0x3fff]) | u16::from(self.vram[(a + 1) & 0x3fff]) << 8;
            let pattern = usize::from(entry & 0x1ff);
            let r = if entry & 0x400 != 0 {
                7 - (sy & 7)
            } else {
                sy & 7
            };
            let bit = if entry & 0x200 != 0 {
                usize::from(sx & 7)
            } else {
                7 - usize::from(sx & 7)
            };
            let p = pattern * 32 + r * 4;
            let idx = (0..4).fold(0u8, |acc, plane| {
                acc | ((self.vram[(p + plane) & 0x3fff] >> bit) & 1) << plane
            });
            let pal = if entry & 0x800 != 0 { 16 } else { 0 };
            *px = (pal | idx, entry & 0x1000 != 0 && idx != 0);
        }
        for (o, &(c, _)) in out.iter_mut().zip(&bg) {
            *o = self.color(usize::from(c));
        }
        // Sprites.
        let sat = usize::from(self.regs[5] & 0x7e) << 7;
        let xn = if self.kind == VdpKind::Sms1 && self.regs[5] & 1 == 0 {
            sat
        } else {
            sat + 0x80
        };
        let big = self.regs[1] & 0x02 != 0;
        let zoom = self.regs[1] & 0x01 != 0;
        let height = if big { 16 } else { 8 } * if zoom { 2 } else { 1 };
        let shift = if self.regs[0] & 0x08 != 0 { 8 } else { 0 };
        let mut shown = 0;
        let mut owner = [false; 256];
        let mut sprite = [None::<u8>; 256];
        let record = self.ov.boxes || self.ov.bounds;
        for i in 0..64usize {
            let sy = self.vram[(sat + i) & 0x3fff];
            if !tall && sy == 0xd0 {
                break;
            }
            let mut top = i32::from(sy) + 1;
            if top > 240 {
                top -= 256;
            }
            let dy = i32::from(y) - top;
            if !(0..height).contains(&dy) {
                continue;
            }
            if shown == 8 {
                self.status |= 0x40;
                break;
            }
            let x = i32::from(self.vram[(xn + i * 2) & 0x3fff]) - shift;
            let mut n = usize::from(self.vram[(xn + i * 2 + 1) & 0x3fff]);
            if big {
                n &= 0xfe;
            }
            n |= usize::from(self.regs[6] & 0x04) << 6;
            if self.kind == VdpKind::Sms1 {
                if self.regs[6] & 0x01 == 0 {
                    n &= !0x40;
                }
                if self.regs[6] & 0x02 == 0 {
                    n &= !0x100;
                }
            }
            let zx = zoom && (self.kind != VdpKind::Sms1 || shown < 4);
            let mut r = (dy / if zoom { 2 } else { 1 }) as usize;
            if r >= 8 {
                n += 1;
                r -= 8;
            }
            shown += 1;
            let p = n * 32 + r * 4;
            let planes: [u8; 4] = std::array::from_fn(|k| self.vram[(p + k) & 0x3fff]);
            let width = if zx { 16 } else { 8 };
            let mut opaque: Option<(i32, i32)> = None;
            for px in 0..width {
                let sx = x + px;
                if !(0..256).contains(&sx) {
                    continue;
                }
                let bit = 7 - (px / if zx { 2 } else { 1 }) as usize;
                let idx = planes
                    .iter()
                    .enumerate()
                    .fold(0u8, |acc, (k, b)| acc | ((b >> bit) & 1) << k);
                if idx == 0 {
                    continue;
                }
                let s = sx as usize;
                if owner[s] {
                    self.status |= 0x20;
                    continue;
                }
                owner[s] = true;
                sprite[s] = Some(16 | idx);
                if record {
                    let f = LEFT as i32 + sx;
                    opaque = Some(opaque.map_or((f, f), |(a, b)| (a.min(f), b.max(f))));
                }
            }
            if record {
                self.ov.cur.push(SpriteLine {
                    n: i as u8,
                    line: y,
                    x: LEFT as i32 + x,
                    w: width as u32,
                    opaque,
                });
            }
        }
        for x in 0..256 {
            if let Some(c) = sprite[x] {
                if !bg[x].1 {
                    out[x] = self.color(usize::from(c));
                }
            }
        }
        if self.regs[0] & 0x20 != 0 {
            let b = self.backdrop();
            out[..8].fill(b);
        }
    }

    /// The TMS9918 modes (Graphics I, II, text, multicolor, the TMS
    /// sprites).
    fn tms_line(&mut self, y: u16, out: &mut [Rgb8; 256]) {
        let r = &self.regs;
        let (m1, m2, m3) = (r[1] & 0x10 != 0, r[1] & 0x08 != 0, r[0] & 0x02 != 0);
        let names = usize::from(r[2] & 0x0f) << 10;
        let mut line = [0u8; 256];
        if m1 {
            // Text: 40 columns of 6 pixels from pixel 8.
            let patterns = usize::from(r[4] & 7) << 11;
            let (fg, bg) = (r[7] >> 4, r[7] & 15);
            for col in 0..40 {
                let name = usize::from(self.vram[(names + usize::from(y / 8) * 40 + col) & 0x3fff]);
                let bits = self.vram[(patterns + name * 8 + usize::from(y & 7)) & 0x3fff];
                for i in 0..6 {
                    line[8 + col * 6 + i] = if bits & (0x80 >> i) != 0 { fg } else { bg };
                }
            }
        } else {
            let row = usize::from(y / 8);
            for col in 0..32 {
                let name = usize::from(self.vram[(names + row * 32 + col) & 0x3fff]);
                let (bits, color) = if m2 {
                    // Multicolor: 4 × 4 blocks.
                    let patterns = usize::from(r[4] & 7) << 11;
                    let c = self.vram
                        [(patterns + name * 8 + (row & 3) * 2 + usize::from(y & 7) / 4) & 0x3fff];
                    (0xf0, c)
                } else if m3 {
                    let third = (row / 8) << 8;
                    let patterns = (usize::from(r[4] & 4) << 11)
                        + ((third + name) & ((usize::from(r[4] & 3) << 8) | 0xff)) * 8;
                    let colors = (usize::from(r[3] & 0x80) << 6)
                        + ((third + name) & ((usize::from(r[3] & 0x7f) << 3) | 7)) * 8;
                    (
                        self.vram[(patterns + usize::from(y & 7)) & 0x3fff],
                        self.vram[(colors + usize::from(y & 7)) & 0x3fff],
                    )
                } else {
                    let patterns = usize::from(r[4] & 7) << 11;
                    let colors = usize::from(r[3]) << 6;
                    (
                        self.vram[(patterns + name * 8 + usize::from(y & 7)) & 0x3fff],
                        self.vram[(colors + name / 8) & 0x3fff],
                    )
                };
                for i in 0..8 {
                    line[col * 8 + i] = if bits & (0x80 >> i) != 0 {
                        color >> 4
                    } else {
                        color & 15
                    };
                }
            }
            self.tms_sprites(y, &mut line);
        }
        for (o, &c) in out.iter_mut().zip(&line) {
            if c != 0 {
                *o = self.tms_color(c);
            }
        }
    }

    fn tms_sprites(&mut self, y: u16, line: &mut [u8; 256]) {
        let r = self.regs;
        let attrs = usize::from(r[5] & 0x7f) << 7;
        let patterns = usize::from(r[6] & 7) << 11;
        let size = if r[1] & 0x02 != 0 { 16 } else { 8 };
        let mag = if r[1] & 0x01 != 0 { 2 } else { 1 };
        let mut shown = 0;
        let mut owner = [false; 256];
        for n in 0..32usize {
            let a = attrs + n * 4;
            let sy = self.vram[a & 0x3fff];
            if sy == 0xd0 {
                break;
            }
            let top = if sy > 0xe0 {
                i32::from(sy) - 255
            } else {
                i32::from(sy) + 1
            };
            let dy = i32::from(y) - top;
            if !(0..size * mag).contains(&dy) {
                continue;
            }
            if shown == 4 {
                self.status |= 0x40;
                break;
            }
            shown += 1;
            let c = self.vram[(a + 3) & 0x3fff];
            let x0 = i32::from(self.vram[(a + 1) & 0x3fff]) - if c & 0x80 != 0 { 32 } else { 0 };
            let mut name = usize::from(self.vram[(a + 2) & 0x3fff]);
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
                let b = self.vram[(patterns + name * 8 + (bit / 8) * 16 + row) & 0x3fff];
                if b & (0x80 >> (bit % 8)) == 0 {
                    continue;
                }
                let x = x as usize;
                if owner[x] {
                    self.status |= 0x20;
                    continue;
                }
                owner[x] = true;
                if c & 15 != 0 {
                    line[x] = c & 15;
                }
            }
        }
    }

    /// Record sprite spans for the overlays in `ids`.
    pub fn set_overlays(&mut self, ids: &[&str]) {
        self.ov.boxes = ids.contains(&"sprites");
        self.ov.bounds = ids.contains(&"sprite_bounds");
        if !self.ov.boxes && !self.ov.bounds {
            self.ov.cur.clear();
            self.ov.last.clear();
        }
    }

    /// Overlay `id`: the last complete field (`last`) or the field so far.
    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        let lines = if last { &self.ov.last } else { &self.ov.cur };
        // Lines are the VDP's (0 = the first active one); the Game Gear's
        // window starts at line 24.
        let row = |l: u16| self.line_row(l);
        let shapes: Vec<Shape> = match id {
            "sprites" if self.ov.boxes => {
                let full: Vec<SpriteLine> = lines
                    .iter()
                    .map(|s| SpriteLine {
                        opaque: Some((s.x, s.x + s.w as i32 - 1)),
                        ..*s
                    })
                    .collect();
                bound_boxes(&full, row)
            }
            "sprite_bounds" if self.ov.bounds => bound_boxes(lines, row),
            _ => return None,
        };
        let shapes = if self.kind == VdpKind::GameGear {
            // The window's columns.
            shapes
                .into_iter()
                .map(|s| match s {
                    Shape::Rect {
                        x,
                        y,
                        w,
                        h,
                        color,
                        label,
                    } => Shape::Rect {
                        x: x - LEFT as i32 - GG_X as i32,
                        y,
                        w,
                        h,
                        color,
                        label,
                    },
                    other => other,
                })
                .collect()
        } else {
            shapes
        };
        let info = overlay_infos(self.chip_name())
            .into_iter()
            .find(|i| i.id == id)?;
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend: Vec::new(),
        })
    }

    pub fn overlays_on(&self) -> bool {
        self.ov.boxes || self.ov.bounds
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"SVDP", 1, |w| {
            w.bytes(&self.vram);
            for c in self.cram {
                w.u16(c);
            }
            w.u8(self.cram_latch);
            w.raw(&self.regs);
            w.u16(self.addr);
            w.u8(self.code);
            w.bool(self.latch.is_some());
            w.u8(self.latch.unwrap_or(0));
            w.u8(self.read_buffer);
            w.u8(self.status);
            w.bool(self.line_int);
            w.u8(self.line_counter);
            w.u8(self.vscroll);
            w.u16(self.line);
            w.u16(self.dot);
            w.u64(self.fields);
            w.u8(self.h_latch);
        });
    }

    /// Atomic: on an error nothing changed.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"SVDP", 1)?;
        let mut n = self.clone();
        let vram = c.bytes()?;
        if vram.len() != 0x4000 {
            return Err(StateError::Invalid("VRAM size"));
        }
        n.vram = vram.to_vec();
        for x in &mut n.cram {
            *x = c.u16()? & 0x0fff;
        }
        n.cram_latch = c.u8()?;
        c.raw_into(&mut n.regs)?;
        n.addr = c.u16()? & 0x3fff;
        n.code = c.u8()? & 3;
        let has = c.bool()?;
        let lo = c.u8()?;
        n.latch = has.then_some(lo);
        n.read_buffer = c.u8()?;
        n.status = c.u8()? & 0xe0;
        n.line_int = c.bool()?;
        n.line_counter = c.u8()?;
        n.vscroll = c.u8()?;
        n.line = c.u16()?;
        n.dot = c.u16()?;
        if n.line >= n.lines() || n.dot >= LINE {
            return Err(StateError::Invalid("beam position"));
        }
        n.fields = c.u64()?;
        n.h_latch = c.u8()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
