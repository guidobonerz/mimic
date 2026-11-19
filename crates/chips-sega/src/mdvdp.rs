//! The Mega Drive / Genesis VDP (315-5313) in mode 5.
//!
//! **Ports** (the machine decodes $C00000–$C0001F): data ($00/$02),
//! control ($04/$06), HV counter ($08–$0E). Byte writes repeat the byte in
//! both halves of the word. The control port takes register writes
//! (`10?R RRRR DDDD DDDD`) and two-word command words (`CD1 CD0 A13–A0`,
//! then `CD5–CD2 A15 A14` in the low byte); a register write clears the
//! code register; the first half alone updates A13–A0 and CD1–CD0. Codes:
//! 0 VRAM read, 1 VRAM write, 3 CRAM write, 4 VSRAM read, 5 VSRAM write,
//! 8 CRAM read; CD5 starts a DMA. Data port writes go to VRAM (byte-swapped
//! at odd addresses), CRAM (`----bbb-ggg-rrr-`, 64 words) or VSRAM (40
//! 10-bit words); the address advances by register 15.
//!
//! **DMA** (register 1 bit 4 enables it; register 23 bits 7–6 the kind):
//! 68000 → VDP (the machine reads the words, [`MdVdp::dma_word`]; the
//! 68000 is frozen meanwhile, [`MdVdp::dma_cost`]), VRAM fill (started by
//! the next data port write: the low byte to the address, then the high
//! byte to `address ^ 1`, length times), VRAM copy (byte by byte, the
//! source from registers 21–22). Length 0 counts as $10000. A transfer to
//! CRAM stops once the address passes $7F. Fills and copies happen at
//! once; the status's DMA bit stays set for the time the manual's transfer
//! rates give.
//!
//! **Picture.** Planes A and B (32, 64 or 128 cells by register 16), the
//! window replacing plane A in the area of registers 17–18, 80 sprites
//! (64 in H32) linked through their link fields, 20 a line (16 in H32) and
//! 320 pixels (256) of them, a sprite with X = 0 masking the rest of its
//! lines; horizontal scroll per screen, cell or line (register 11 bits
//! 1–0, table at register 13), vertical scroll per screen or 2-cell column
//! (bit 2; VSRAM). Priority, back to front: backdrop, plane B, plane A,
//! sprites, then the high-priority B, A and sprites. Shadow/highlight
//! (register 12 bit 3): the background at half intensity unless a plane
//! has priority there, sprite colours $3E/$3F shadow or highlight what is
//! beneath, $0E/$1E/$2E stay normal. H32 (256 pixels) or H40 (320), 224
//! or 240 lines (register 1 bit 3), interlace (register 12 bits 2–1;
//! double resolution renders the field's lines from 8 × 16 patterns).
//!
//! **Timing.** 3420 master clocks a line (H40: 8 a pixel, H32: 10), 262
//! lines (PAL 313). Line `n` begins when the V counter changes to it, at
//! H counter $A5 (H32: $85); its pixels follow from H counter $00, where
//! the line is drawn with the registers as they are then. The line
//! counter (register 10) counts down when a line begins, on lines 1 to
//! the one after the active lines (loaded on line 0, the others and at
//! an underflow); an underflow raises the level 4 interrupt (register 0 bit
//! 4). The level 6 interrupt rises on line $E0 at H counter $08 (register
//! 1 bit 5), and the Z80's /INT for that line. The H and V counters follow
//! MacDonald's tables.
//!
//! Simplifications of this project: no FIFO (the status reads it empty
//! and the 68000 never waits on it), register 0 bit 2 (the 3-bit palette)
//! is ignored, the intensities are linear, a VRAM fill or copy happens at
//! once, the mode 4 (Master System) display is not there.
//!
//! Sources: Charles MacDonald, *Sega Genesis VDP documentation* v1.5f
//! (2000) and *VDP timing for an NTSC Genesis in display mode 5* v0.4
//! (2000). The implementation is this project's own.

use mimic_core::frame::Rgb8;
use mimic_core::overlay::{bound_boxes, Overlay, OverlayInfo, Shape, SpriteLine};
use mimic_core::savestate::{StateError, StateReader, StateWriter};

/// Master clocks per line.
pub const LINE_MCLK: u32 = 3420;
/// Picture widths: borders and the active pixels.
pub const H40_WIDTH: u32 = 14 + 320 + 14;
pub const H32_WIDTH: u32 = 13 + 256 + 15;
const H40_LEFT: usize = 14;
const H32_LEFT: usize = 13;
/// Where in the line the active pixels begin (H counter $00).
const ACTIVE_H40: u32 = 46 * 16;
const ACTIVE_H32: u32 = 37 * 20;
/// The level 6 interrupt: H counter $08 on line $E0.
const VINT_H40: u32 = ACTIVE_H40 + 8 * 16;
const VINT_H32: u32 = ACTIVE_H32 + 8 * 20;

/// 3-bit colour levels.
const LEVEL: [u8; 8] = [0, 36, 73, 109, 146, 182, 219, 255];

pub fn overlay_infos() -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "sprites",
            name: "VDP sprite boxes".into(),
        },
        OverlayInfo {
            id: "sprite_bounds",
            name: "VDP sprite bounds".into(),
        },
    ]
}

/// A 68000 → VDP transfer for the machine to carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaRequest {
    /// Byte address in the 68000's space.
    pub source: u32,
    /// Words.
    pub words: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Overlays {
    boxes: bool,
    bounds: bool,
    cur: Vec<SpriteLine>,
    last: Vec<SpriteLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdVdp {
    pal: bool,
    pub vram: Vec<u8>,
    pub cram: [u16; 64],
    pub vsram: [u16; 40],
    pub regs: [u8; 24],
    pub addr: u16,
    pub code: u8,
    pending: bool,
    /// A VRAM fill waits for the data port.
    fill_armed: bool,
    read_buffer: u16,
    vint_pending: bool,
    hint_pending: bool,
    /// Status bits 6 and 5 (overflow, collision).
    flags: u8,
    line_counter: u8,
    pub line: u16,
    /// Master clocks into the line.
    pub pos: u32,
    pub fields: u64,
    odd: bool,
    /// Master clocks since power-on; the DMA bit is set until `dma_end`.
    pub now: u64,
    dma_end: u64,
    hv_latch: Option<u16>,
    /// The Z80's /INT (line $E0).
    z80_int: bool,
    h40_frame: bool,
    frame: Vec<Rgb8>,
    ov: Overlays,
}

/// One pixel of a layer: colour (palette × 16 + index, 0 transparent) and
/// priority.
#[derive(Clone, Copy, Default)]
struct Px {
    color: u8,
    prio: bool,
}

impl MdVdp {
    pub fn new(pal: bool) -> Self {
        let mut v = Self {
            pal,
            vram: vec![0; 0x10000],
            cram: [0; 64],
            vsram: [0; 40],
            regs: [0; 24],
            addr: 0,
            code: 0,
            pending: false,
            fill_armed: false,
            read_buffer: 0,
            vint_pending: false,
            hint_pending: false,
            flags: 0,
            line_counter: 0,
            line: 0,
            pos: 0,
            fields: 0,
            odd: false,
            now: 0,
            dma_end: 0,
            hv_latch: None,
            z80_int: false,
            h40_frame: false,
            frame: Vec::new(),
            ov: Overlays::default(),
        };
        v.frame = vec![Rgb8::default(); (H32_WIDTH * v.frame_height()) as usize];
        v
    }

    pub fn is_pal(&self) -> bool {
        self.pal
    }

    pub fn lines(&self) -> u16 {
        if self.pal {
            313
        } else {
            262
        }
    }

    fn frame_height(&self) -> u32 {
        if self.pal {
            294
        } else {
            243
        }
    }

    pub fn h40(&self) -> bool {
        self.regs[12] & 0x01 != 0
    }

    /// Active lines: 224, or 240 (register 1 bit 3; PAL only shows it).
    pub fn height(&self) -> u16 {
        if self.regs[1] & 0x08 != 0 && self.pal {
            240
        } else {
            224
        }
    }

    fn interlace2(&self) -> bool {
        self.regs[12] & 0x06 == 0x06
    }

    fn interlaced(&self) -> bool {
        self.regs[12] & 0x02 != 0
    }

    fn borders(&self) -> (u16, u16) {
        match (self.pal, self.height()) {
            (false, _) => (11, 8),
            (true, 240) => (30, 24),
            (true, _) => (38, 32),
        }
    }

    /// The picture row of `line`.
    pub fn line_row(&self, line: u16) -> Option<u32> {
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

    /// The picture: width, height, pixels.
    pub fn frame(&self) -> (u32, u32, &[Rgb8]) {
        let w = if self.h40_frame { H40_WIDTH } else { H32_WIDTH };
        (w, self.frame_height(), &self.frame)
    }

    pub fn reset(&mut self) {
        self.regs = [0; 24];
        self.pending = false;
        self.fill_armed = false;
        self.code = 0;
        self.vint_pending = false;
        self.hint_pending = false;
        self.flags = 0;
        self.dma_end = 0;
        self.hv_latch = None;
    }

    // ---- Interrupts ----------------------------------------------------

    /// The 68000's interrupt level (6, 4 or 0).
    pub fn ipl(&self) -> u8 {
        if self.vint_pending && self.regs[1] & 0x20 != 0 {
            6
        } else if self.hint_pending && self.regs[0] & 0x10 != 0 {
            4
        } else {
            0
        }
    }

    /// The 68000 acknowledged interrupt `level`.
    pub fn acknowledge(&mut self, level: u8) {
        match level {
            6 => self.vint_pending = false,
            4 => self.hint_pending = false,
            _ => {}
        }
    }

    pub fn z80_int(&self) -> bool {
        self.z80_int
    }

    /// External interrupt edge on TH (register 11 bit 3 enables level 2;
    /// register 0 bit 1 latches the HV counter). Returns whether level 2
    /// is raised.
    pub fn th_edge(&mut self) -> bool {
        if self.regs[0] & 0x02 != 0 {
            self.hv_latch = Some(self.hv_live());
        }
        self.regs[11] & 0x08 != 0
    }

    // ---- Counters ------------------------------------------------------

    fn h_counter(&self) -> u8 {
        let (u, start, jump_from, jump_to) = if self.h40() {
            (self.pos / 16, 0xa5u32, 0xb6u32, 0xe4u32)
        } else {
            (self.pos / 20, 0x85, 0x93, 0xe9)
        };
        let mut h = start + u;
        if h > jump_from {
            h = h - jump_from - 1 + jump_to;
        }
        (h & 0xff) as u8
    }

    fn v_counter(&self) -> u8 {
        let l = u32::from(self.line);
        let v = match (self.pal, self.height()) {
            (false, 224) if l > 0xea => l - 6,
            (true, 224) if l > 0x102 => l - 0x103 + 0x1ca,
            (true, 240) if l > 0x10a => l - 0x10b + 0x1d2,
            _ => l,
        };
        if self.interlace2() {
            let v2 = (v << 1) | u32::from(self.odd);
            ((v2 & 0xfe) | (v2 >> 8 & 1)) as u8
        } else {
            v as u8
        }
    }

    fn hv_live(&self) -> u16 {
        u16::from(self.v_counter()) << 8 | u16::from(self.h_counter())
    }

    /// The HV counter port.
    pub fn hv_counter(&self) -> u16 {
        self.hv_latch.unwrap_or_else(|| self.hv_live())
    }

    fn active_start(&self) -> u32 {
        if self.h40() {
            ACTIVE_H40
        } else {
            ACTIVE_H32
        }
    }

    /// The status word (bits 15–10 here as the fixed `0011 01`; the bus
    /// puts the next opcode's bits there).
    pub fn status(&self) -> u16 {
        let h = self.h_counter();
        let hblank = if self.h40() {
            !(0x06..0xb2).contains(&h)
        } else {
            !(0x06..0x93).contains(&h)
        };
        let vblank = self.line >= self.height() || self.regs[1] & 0x40 == 0;
        0x3400
            | 0x0200
            | u16::from(self.vint_pending) << 7
            | u16::from(self.flags & 0x60)
            | u16::from(self.odd && self.interlaced()) << 4
            | u16::from(vblank) << 3
            | u16::from(hblank) << 2
            | u16::from(self.now < self.dma_end) << 1
            | u16::from(self.pal)
    }

    /// Read the control port: the status; clears the write-pending flag
    /// and the sprite flags.
    pub fn read_status(&mut self) -> u16 {
        let s = self.status();
        self.pending = false;
        self.flags = 0;
        s
    }

    // ---- Ports ---------------------------------------------------------

    fn increment(&mut self) {
        self.addr = self.addr.wrapping_add(u16::from(self.regs[15]));
    }

    pub fn read_data(&mut self) -> u16 {
        self.pending = false;
        let a = self.addr;
        let v = match self.code & 0x0f {
            0 => u16::from_be_bytes([
                self.vram[usize::from(a & !1)],
                self.vram[usize::from(a | 1)],
            ]),
            4 => {
                self.vsram
                    .get(usize::from(a >> 1 & 0x3f))
                    .copied()
                    .unwrap_or(0)
                    & 0x3ff
            }
            8 => self.cram[usize::from(a >> 1 & 0x3f)] & 0x0eee,
            _ => self.read_buffer,
        };
        self.read_buffer = v;
        self.increment();
        v
    }

    pub fn write_data(&mut self, v: u16) {
        self.pending = false;
        if self.fill_armed {
            self.fill_armed = false;
            self.fill(v);
            return;
        }
        self.store(v);
        self.increment();
    }

    /// One word to the RAM the code selects (data port, DMA).
    fn store(&mut self, v: u16) {
        let a = self.addr;
        match self.code & 0x0f {
            1 => {
                let [hi, lo] = v.to_be_bytes();
                let (x, y) = if a & 1 != 0 { (lo, hi) } else { (hi, lo) };
                self.vram[usize::from(a & !1)] = x;
                self.vram[usize::from(a | 1)] = y;
            }
            3 => self.cram[usize::from(a >> 1 & 0x3f)] = v & 0x0eee,
            5 => {
                if let Some(s) = self.vsram.get_mut(usize::from(a >> 1 & 0x3f)) {
                    *s = v & 0x3ff;
                }
            }
            _ => {}
        }
    }

    fn dma_length(&self) -> u32 {
        match u32::from(self.regs[19]) | u32::from(self.regs[20]) << 8 {
            0 => 0x10000,
            n => n,
        }
    }

    fn fill(&mut self, v: u16) {
        let len = self.dma_length();
        let [hi, lo] = v.to_be_bytes();
        match self.code & 0x0f {
            1 => {
                self.vram[usize::from(self.addr)] = lo;
                for _ in 0..len {
                    self.vram[usize::from(self.addr ^ 1)] = hi;
                    self.increment();
                }
            }
            _ => {
                for _ in 0..len {
                    self.store(v);
                    self.increment();
                }
            }
        }
        self.dma_end = self.now + u64::from(len) * u64::from(self.dma_cost_unit(2));
    }

    fn copy(&mut self) {
        let len = self.dma_length();
        let mut src = u16::from(self.regs[21]) | u16::from(self.regs[22]) << 8;
        for _ in 0..len {
            self.vram[usize::from(self.addr)] = self.vram[usize::from(src)];
            src = src.wrapping_add(1);
            self.increment();
        }
        self.regs[21] = src as u8;
        self.regs[22] = (src >> 8) as u8;
        self.dma_end = self.now + u64::from(len) * u64::from(self.dma_cost_unit(1));
    }

    /// Master clocks per transfer unit now (the manual's rates; `kind` 0
    /// 68000 → VDP words, 1 copy bytes, 2 fill bytes).
    fn dma_cost_unit(&self, kind: u8) -> u32 {
        let blank = self.line >= self.height() || self.regs[1] & 0x40 == 0;
        let per_line = match (kind, self.h40(), blank) {
            (0, true, true) => 205,
            (0, true, false) => 18,
            (0, false, true) => 167,
            (0, false, false) => 16,
            (1, true, true) => 102,
            (1, true, false) => 9,
            (1, false, true) => 83,
            (1, false, false) => 8,
            (_, true, true) => 204,
            (_, true, false) => 17,
            (_, false, true) => 166,
            _ => 15,
        };
        LINE_MCLK / per_line
    }

    /// The 68000's frozen time for a transfer of `words` started now.
    pub fn dma_cost(&self, words: u32) -> u64 {
        u64::from(words) * u64::from(self.dma_cost_unit(0))
    }

    /// One word of a 68000 → VDP transfer. False: the transfer stops
    /// (CRAM past $7F).
    pub fn dma_word(&mut self, v: u16) -> bool {
        if self.code & 0x0f == 3 && self.addr > 0x7f {
            return false;
        }
        self.store(v);
        self.increment();
        true
    }

    /// Advance the source registers after a transfer of `words`.
    pub fn dma_done(&mut self, words: u32, frozen: u64) {
        let src = (u32::from(self.regs[21]) | u32::from(self.regs[22]) << 8).wrapping_add(words);
        self.regs[21] = src as u8;
        self.regs[22] = (src >> 8) as u8;
        self.regs[19] = 0;
        self.regs[20] = 0;
        self.dma_end = self.now + frozen;
    }

    /// Write the control port; a 68000 → VDP DMA comes back for the machine.
    pub fn write_control(&mut self, v: u16) -> Option<DmaRequest> {
        if !self.pending {
            if v & 0xc000 == 0x8000 {
                let r = usize::from(v >> 8 & 0x1f);
                if let Some(reg) = self.regs.get_mut(r) {
                    *reg = v as u8;
                }
                self.code = 0;
                return None;
            }
            self.pending = true;
            self.addr = (self.addr & 0xc000) | (v & 0x3fff);
            self.code = (self.code & 0x3c) | (v >> 14) as u8;
            return None;
        }
        self.pending = false;
        self.addr = (self.addr & 0x3fff) | (v & 3) << 14;
        self.code = (self.code & 3) | ((v >> 2) as u8 & 0x3c);
        if self.code & 0x20 == 0 || self.regs[1] & 0x10 == 0 {
            if self.code & 0x0f == 0 {
                self.read_buffer = u16::from_be_bytes([
                    self.vram[usize::from(self.addr & !1)],
                    self.vram[usize::from(self.addr | 1)],
                ]);
            }
            return None;
        }
        match self.regs[23] >> 6 {
            0 | 1 => {
                let source = (u32::from(self.regs[21])
                    | u32::from(self.regs[22]) << 8
                    | u32::from(self.regs[23] & 0x7f) << 16)
                    << 1;
                Some(DmaRequest {
                    source,
                    words: self.dma_length(),
                })
            }
            2 => {
                self.fill_armed = true;
                None
            }
            _ => {
                self.copy();
                None
            }
        }
    }

    /// Side-effect-free view of the data and control ports.
    pub fn peek(&self) -> (u16, u16) {
        (self.read_buffer, self.status())
    }

    // ---- Time ----------------------------------------------------------

    /// Advance `mclk` master clocks. Returns true when a field ended.
    pub fn advance(&mut self, mut mclk: u32) -> bool {
        let mut ended = false;
        while mclk > 0 {
            let next = self.next_event();
            let step = (next - self.pos).min(mclk);
            self.pos += step;
            self.now += u64::from(step);
            mclk -= step;
            if self.pos == next {
                ended |= self.event();
            }
        }
        ended
    }

    fn next_event(&self) -> u32 {
        let active = self.active_start();
        let vint = if self.h40() { VINT_H40 } else { VINT_H32 };
        [active, vint, LINE_MCLK]
            .into_iter()
            .find(|&e| e > self.pos)
            .unwrap_or(LINE_MCLK)
    }

    fn event(&mut self) -> bool {
        let active = self.active_start();
        let vint = if self.h40() { VINT_H40 } else { VINT_H32 };
        if self.pos == active {
            self.draw_line();
        }
        if self.pos == vint && self.line == self.height() {
            self.vint_pending = true;
            self.z80_int = true;
        }
        if self.pos < LINE_MCLK {
            return false;
        }
        self.pos = 0;
        self.line += 1;
        let mut ended = false;
        if self.line >= self.lines() {
            self.line = 0;
            self.fields += 1;
            self.odd = !self.odd;
            ended = true;
            self.h40_frame = self.h40();
            let size =
                (if self.h40_frame { H40_WIDTH } else { H32_WIDTH } * self.frame_height()) as usize;
            if self.frame.len() != size {
                self.frame = vec![Rgb8::default(); size];
            }
            if self.ov.boxes || self.ov.bounds {
                self.ov.last = std::mem::take(&mut self.ov.cur);
            }
        }
        self.z80_int = false;
        // The line counter at the start of a line: loaded on line 0 and
        // after the active lines, counted down on the others.
        if self.line != 0 && self.line <= self.height() {
            let (c, under) = self.line_counter.overflowing_sub(1);
            if under {
                self.line_counter = self.regs[10];
                self.hint_pending = true;
            } else {
                self.line_counter = c;
            }
        } else {
            self.line_counter = self.regs[10];
        }
        ended
    }

    // ---- Rendering -----------------------------------------------------

    fn rgb(&self, color: u8, shade: Shade) -> Rgb8 {
        let c = self.cram[usize::from(color & 63)];
        let f = |n: u16| -> u8 {
            let v = LEVEL[usize::from(c >> n & 7)];
            match shade {
                Shade::Normal => v,
                Shade::Shadow => v / 2,
                Shade::Highlight => 128 + v / 2,
            }
        };
        Rgb8 {
            r: f(1),
            g: f(5),
            b: f(9),
        }
    }

    fn draw_line(&mut self) {
        let Some(row) = self.line_row(self.line) else {
            return;
        };
        let w = if self.h40_frame { H40_WIDTH } else { H32_WIDTH } as usize;
        let left = if self.h40_frame { H40_LEFT } else { H32_LEFT };
        let base = row as usize * w;
        if base + w > self.frame.len() {
            return;
        }
        let backdrop = self.rgb(self.regs[7], Shade::Normal);
        let width = if self.h40() { 320 } else { 256 };
        let mut line = vec![backdrop; width];
        if self.line < self.height() && self.regs[1] & 0x40 != 0 {
            self.render(&mut line);
        }
        let out = &mut self.frame[base..base + w];
        out.fill(backdrop);
        let n = width.min(w - left);
        out[left..left + n].copy_from_slice(&line[..n]);
    }

    /// The display line (interlace mode 2: the field's line of 448).
    fn y(&self) -> u32 {
        if self.interlace2() {
            u32::from(self.line) * 2 + u32::from(self.odd)
        } else {
            u32::from(self.line)
        }
    }

    fn word(&self, a: usize) -> u16 {
        u16::from_be_bytes([self.vram[a & 0xffff], self.vram[(a + 1) & 0xffff]])
    }

    /// The pixel of pattern `entry` at (`px`, `py`) within the cell.
    fn pattern_pixel(&self, entry: u16, px: u32, py: u32) -> u8 {
        let tall = self.interlace2();
        let rows = if tall { 16 } else { 8 };
        let px = if entry & 0x0800 != 0 { 7 - px } else { px };
        let py = if entry & 0x1000 != 0 {
            rows - 1 - py
        } else {
            py
        };
        let name = u32::from(entry & 0x7ff);
        let addr = if tall {
            (name & !1) * 64 + py * 4 + px / 2
        } else {
            name * 32 + py * 4 + px / 2
        };
        let b = self.vram[(addr & 0xffff) as usize];
        let idx = if px & 1 == 0 { b >> 4 } else { b & 15 };
        if idx == 0 {
            0
        } else {
            ((entry >> 13 & 3) as u8) << 4 | idx
        }
    }

    fn plane_size(&self) -> (u32, u32) {
        let s = |v: u8| match v & 3 {
            0 => 32,
            1 => 64,
            2 => 32,
            _ => 128,
        };
        (s(self.regs[16]), s(self.regs[16] >> 4))
    }

    /// One plane's pixel at screen `x`.
    fn plane_pixel(&self, base: usize, b_plane: bool, x: u32, hscroll: u32) -> Px {
        let (cols, rows) = self.plane_size();
        let tall = self.interlace2();
        let cell_h = if tall { 16 } else { 8 };
        let vs_index = if self.regs[11] & 0x04 != 0 {
            ((x / 16) * 2) as usize + usize::from(b_plane)
        } else {
            usize::from(b_plane)
        };
        let vscroll = u32::from(self.vsram.get(vs_index).copied().unwrap_or(0));
        let pw = cols * 8;
        let ph = rows * cell_h;
        let px = (x + pw * 4 - (hscroll % pw)) % pw;
        let py = (self.y() + vscroll) % ph;
        let row = if self.regs[16] & 3 == 2 {
            0
        } else {
            py / cell_h
        };
        let entry = self.word(base + ((row * cols + px / 8) * 2) as usize);
        Px {
            color: self.pattern_pixel(entry, px & 7, py % cell_h),
            prio: entry & 0x8000 != 0,
        }
    }

    fn window_pixel(&self, x: u32) -> Px {
        let tall = self.interlace2();
        let cell_h = if tall { 16 } else { 8 };
        let (cols, base) = if self.h40() {
            (64, usize::from(self.regs[3] & 0x3c) << 10)
        } else {
            (32, usize::from(self.regs[3] & 0x3e) << 10)
        };
        let y = self.y();
        let entry = self.word(base + (((y / cell_h) * cols + x / 8) * 2) as usize);
        Px {
            color: self.pattern_pixel(entry, x & 7, y % cell_h),
            prio: entry & 0x8000 != 0,
        }
    }

    /// Whether the window covers screen `x` on this line.
    fn in_window(&self, x: u32) -> bool {
        let wv = u32::from(self.regs[18] & 0x1f) * 8;
        let down = self.regs[18] & 0x80 != 0;
        let line = u32::from(self.line);
        if (down && line >= wv) || (!down && line < wv) {
            return true;
        }
        let wh = u32::from(self.regs[17] & 0x1f) * 16;
        if self.regs[17] & 0x80 != 0 {
            x >= wh
        } else {
            x < wh
        }
    }

    fn hscroll(&self) -> (u32, u32) {
        let base = usize::from(self.regs[13] & 0x3f) << 10;
        let line = self.line as usize;
        let offset = match self.regs[11] & 3 {
            0 => 0,
            1 => (line & 7) * 4,
            2 => (line & !7) * 4,
            _ => line * 4,
        };
        (
            u32::from(self.word(base + offset) & 0x3ff),
            u32::from(self.word(base + offset + 2) & 0x3ff),
        )
    }

    /// The sprites of this line into `out` (colour 0: none).
    fn sprites(&mut self, out: &mut [Px]) {
        let h40 = self.h40();
        let (max_total, max_line, max_px) = if h40 { (80, 20, 320) } else { (64, 16, 256) };
        let tall = self.interlace2();
        let cell_h: i32 = if tall { 16 } else { 8 };
        let sat = if h40 {
            usize::from(self.regs[5] & 0x7e) << 9
        } else {
            usize::from(self.regs[5] & 0x7f) << 9
        };
        let y = self.y() as i32;
        let mut found: Vec<usize> = Vec::new();
        let mut n = 0usize;
        for _ in 0..max_total {
            let e = sat + n * 8;
            let sy = i32::from(self.word(e) & 0x3ff) - if tall { 256 } else { 128 };
            let size = self.vram[(e + 2) & 0xffff];
            let vs = i32::from(size & 3) + 1;
            if (sy..sy + vs * cell_h).contains(&y) {
                if found.len() == max_line {
                    self.flags |= 0x40;
                    break;
                }
                found.push(e);
            }
            let link = usize::from(self.vram[(e + 3) & 0xffff] & 0x7f);
            if link == 0 || link >= max_total {
                break;
            }
            n = link;
        }
        let record = self.ov.boxes || self.ov.bounds;
        let mut budget = max_px;
        let mut taken = vec![false; out.len()];
        for (i, &e) in found.iter().enumerate() {
            let sy = i32::from(self.word(e) & 0x3ff) - if tall { 256 } else { 128 };
            let size = self.vram[(e + 2) & 0xffff];
            let (hs, vs) = (i32::from(size >> 2 & 3) + 1, i32::from(size & 3) + 1);
            let entry = self.word(e + 4);
            let sx_raw = self.word(e + 6) & 0x1ff;
            if sx_raw == 0 && i > 0 {
                break;
            }
            let sx = i32::from(sx_raw) - 128;
            let mut dy = y - sy;
            if entry & 0x1000 != 0 {
                dy = vs * cell_h - 1 - dy;
            }
            let (cy, py) = (dy / cell_h, dy % cell_h);
            let mut opaque: Option<(i32, i32)> = None;
            let width = hs * 8;
            for px in 0..width {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                let x = sx + px;
                if !(0..out.len() as i32).contains(&x) {
                    continue;
                }
                let mut cx = px / 8;
                if entry & 0x0800 != 0 {
                    cx = hs - 1 - cx;
                }
                let name =
                    (entry & 0x7ff).wrapping_add((cx * vs + cy) as u16 * if tall { 2 } else { 1 });
                let cell = (entry & 0xf800) | (name & 0x7ff);
                // The flips are already in the cell choice; the pixel
                // within the cell flips by the entry's bits.
                let ppx = (px & 7) as u32;
                let ppy = py as u32;
                let color = self.pattern_pixel(cell & !0x1000, ppx, ppy);
                if color == 0 {
                    continue;
                }
                let xi = x as usize;
                if taken[xi] {
                    self.flags |= 0x20;
                    continue;
                }
                taken[xi] = true;
                out[xi] = Px {
                    color,
                    prio: entry & 0x8000 != 0,
                };
                if record {
                    let f = x + if h40 { H40_LEFT } else { H32_LEFT } as i32;
                    opaque = Some(opaque.map_or((f, f), |(a, b)| (a.min(f), b.max(f))));
                }
            }
            if record {
                let left = if h40 { H40_LEFT } else { H32_LEFT } as i32;
                self.ov.cur.push(SpriteLine {
                    n: ((e - sat) / 8) as u8,
                    line: self.line,
                    x: left + sx,
                    w: width as u32,
                    opaque,
                });
            }
            if budget == 0 {
                break;
            }
        }
    }

    fn render(&mut self, line: &mut [Rgb8]) {
        let width = line.len();
        let a_base = usize::from(self.regs[2] & 0x38) << 10;
        let b_base = usize::from(self.regs[4] & 0x07) << 13;
        let (hs_a, hs_b) = self.hscroll();
        let mut spr = vec![Px::default(); width];
        self.sprites(&mut spr);
        let sh = self.regs[12] & 0x08 != 0;
        let backdrop = self.regs[7] & 63;
        for (x, out) in line.iter_mut().enumerate() {
            let xu = x as u32;
            let b = self.plane_pixel(b_base, true, xu, hs_b);
            let a = if self.in_window(xu) {
                self.window_pixel(xu)
            } else {
                self.plane_pixel(a_base, false, xu, hs_a)
            };
            let s = spr[x];
            // Back to front.
            let mut color = backdrop;
            let mut layers = [(b, false), (a, false), (s, true)];
            let mut top: Option<(Px, bool)> = None;
            for pass in [false, true] {
                for (p, is_sprite) in &mut layers {
                    if p.color != 0 && p.prio == pass {
                        top = Some((*p, *is_sprite));
                    }
                }
            }
            if let Some((p, _)) = top {
                color = p.color;
            }
            if !sh {
                *out = self.rgb(color, Shade::Normal);
                continue;
            }
            // Shadow/highlight.
            let bg_prio = a.prio || b.prio;
            let mut shade = if bg_prio {
                Shade::Normal
            } else {
                Shade::Shadow
            };
            if s.color != 0 && (s.prio || !bg_prio) {
                match s.color {
                    0x3e => {
                        shade = shade.darker();
                        color = top_background(a, b, backdrop);
                    }
                    0x3f => {
                        shade = shade.lighter();
                        color = top_background(a, b, backdrop);
                    }
                    c if c & 15 == 14 && c < 0x30 => shade = Shade::Normal,
                    _ => {
                        if s.prio {
                            shade = Shade::Normal;
                        }
                    }
                }
            }
            *out = self.rgb(color, shade);
        }
    }

    // ---- Overlays ------------------------------------------------------

    pub fn set_overlays(&mut self, ids: &[&str]) {
        self.ov.boxes = ids.contains(&"sprites");
        self.ov.bounds = ids.contains(&"sprite_bounds");
        if !self.ov.boxes && !self.ov.bounds {
            self.ov.cur.clear();
            self.ov.last.clear();
        }
    }

    pub fn overlays_on(&self) -> bool {
        self.ov.boxes || self.ov.bounds
    }

    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        let lines = if last { &self.ov.last } else { &self.ov.cur };
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
        let info = overlay_infos().into_iter().find(|i| i.id == id)?;
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend: Vec::new(),
        })
    }

    // ---- Savestates ----------------------------------------------------

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"MVDP", 1, |w| {
            w.bytes(&self.vram);
            for c in self.cram {
                w.u16(c);
            }
            for v in self.vsram {
                w.u16(v);
            }
            w.raw(&self.regs);
            w.u16(self.addr);
            w.u8(self.code);
            w.bool(self.pending);
            w.bool(self.fill_armed);
            w.u16(self.read_buffer);
            w.bool(self.vint_pending);
            w.bool(self.hint_pending);
            w.u8(self.flags);
            w.u8(self.line_counter);
            w.u16(self.line);
            w.u32(self.pos);
            w.u64(self.fields);
            w.bool(self.odd);
            w.u64(self.now);
            w.u64(self.dma_end);
            w.bool(self.hv_latch.is_some());
            w.u16(self.hv_latch.unwrap_or(0));
            w.bool(self.z80_int);
            w.bool(self.h40_frame);
        });
    }

    /// Atomic: on an error nothing changed.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"MVDP", 1)?;
        let mut n = self.clone();
        let vram = c.bytes()?;
        if vram.len() != 0x10000 {
            return Err(StateError::Invalid("VRAM size"));
        }
        n.vram = vram.to_vec();
        for x in &mut n.cram {
            *x = c.u16()? & 0x0eee;
        }
        for x in &mut n.vsram {
            *x = c.u16()? & 0x3ff;
        }
        c.raw_into(&mut n.regs)?;
        n.addr = c.u16()?;
        n.code = c.u8()? & 0x3f;
        n.pending = c.bool()?;
        n.fill_armed = c.bool()?;
        n.read_buffer = c.u16()?;
        n.vint_pending = c.bool()?;
        n.hint_pending = c.bool()?;
        n.flags = c.u8()? & 0x60;
        n.line_counter = c.u8()?;
        n.line = c.u16()?;
        n.pos = c.u32()?;
        if n.line >= n.lines() || n.pos >= LINE_MCLK {
            return Err(StateError::Invalid("beam position"));
        }
        n.fields = c.u64()?;
        n.odd = c.bool()?;
        n.now = c.u64()?;
        n.dma_end = c.u64()?;
        let latched = c.bool()?;
        let hv = c.u16()?;
        n.hv_latch = latched.then_some(hv);
        n.z80_int = c.bool()?;
        n.h40_frame = c.bool()?;
        let size = (if n.h40_frame { H40_WIDTH } else { H32_WIDTH } * n.frame_height()) as usize;
        if n.frame.len() != size {
            n.frame = vec![Rgb8::default(); size];
        }
        *self = n;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shade {
    Shadow,
    Normal,
    Highlight,
}

impl Shade {
    fn darker(self) -> Self {
        match self {
            Shade::Highlight => Shade::Normal,
            _ => Shade::Shadow,
        }
    }

    fn lighter(self) -> Self {
        match self {
            Shade::Shadow => Shade::Normal,
            _ => Shade::Highlight,
        }
    }
}

/// The colour beneath an operator sprite.
fn top_background(a: Px, b: Px, backdrop: u8) -> u8 {
    let mut c = backdrop;
    for pass in [false, true] {
        for p in [b, a] {
            if p.color != 0 && p.prio == pass {
                c = p.color;
            }
        }
    }
    c
}

pub mod gfx;

#[cfg(test)]
mod tests;
