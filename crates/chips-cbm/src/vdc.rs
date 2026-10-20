//! MOS 8563 Video Display Controller (C128 80 columns, RGBI output).
//!
//! The CPU reaches it through two addresses: `$D600` writes the register
//! number and reads the status (7 ready, 6 light pen, 5 vertical blank,
//! 0–2 version), `$D601` reads/writes the selected register. The VDC has
//! its own 16 KB (or 64 KB) of RAM, reached through R18/R19 (update
//! address) and R31 (data, auto-increment).
//!
//! | Reg     | Function                                                          |
//! | ------- | ----------------------------------------------------------------- |
//! | R0–R9   | CRTC timing: horizontal total/displayed, vertical total/adjust/displayed, sync, scan lines per row − 1 |
//! | R10/R11 | cursor: 6–5 mode (solid, off, blink 1/16, 1/32), 4–0 start; end line |
//! | R12/R13 | display start address                                             |
//! | R14/R15 | cursor address                                                    |
//! | R18/R19 | update address                                                    |
//! | R20/R21 | attribute start address                                           |
//! | R22     | 7–4 character cell width − 1, 3–0 displayed pixels                |
//! | R23     | displayed scan lines per character                                |
//! | R24     | 7 block copy (1) / fill (0), 6 reverse screen, 5 blink rate, 4–0 vertical scroll |
//! | R25     | 7 bitmap mode, 6 attributes on, 5 semigraphics, 4 double pixel, 3–0 horizontal scroll |
//! | R26     | 7–4 foreground, 3–0 background colour                             |
//! | R27     | address increment per row                                         |
//! | R28     | 7–5 character set address (A15–A13), 4 64 KB RAM                  |
//! | R29     | underline scan line                                               |
//! | R30     | word count: writing it copies or fills that many bytes            |
//! | R31     | data                                                              |
//! | R32/R33 | block copy source                                                 |
//! | R34–R36 | display enable begin/end, refresh cycles                          |
//!
//! Attribute bytes: 7 alternate character set, 6 reverse, 5 underline,
//! 4 blink, 3–0 colour (RGBI). Source: Commodore 128 Programmer's
//! Reference Guide, chapter 10; MOS 8563 data sheet.
//!
//! Timing: [`Vdc::tick`] is one character clock (2 MHz). The frame buffer
//! shows the display window with a margin in the background colour; it is
//! aligned to the window, not to sync. Simplifications: VRAM accesses and
//! block operations complete at once (the status is always ready);
//! horizontal smooth scroll, semigraphics and light pen are not modelled.

use emu_core::frame::{FieldParity, OutputType, Rgb8, SignalInfo, VideoStandard};
use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Character clocks per second (16 MHz dot clock / 8).
pub const CHAR_CLOCK_HZ: u32 = 2_000_000;

const MARGIN_X: u32 = 40;
const MARGIN_Y: u32 = 24;
/// Frame buffer: 80 columns of 8 pixels, 25 rows of 8 lines, plus margin.
pub const WIDTH: u32 = 640 + 2 * MARGIN_X;
pub const HEIGHT: u32 = 200 + 2 * MARGIN_Y;

/// Writable bits per register.
const MASKS: [u8; 37] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0xff, 0xff, 0x03, 0x1f, 0x7f, 0x1f, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0x0f,
];

/// The RGBI colours (CGA-like: dark yellow is brown).
pub fn palette() -> [Rgb8; 16] {
    let mut p = [Rgb8::default(); 16];
    for (i, c) in p.iter_mut().enumerate() {
        let level = |bit: usize| -> u8 {
            let on = i & bit != 0;
            let bright = i & 1 != 0;
            match (on, bright) {
                (true, true) => 0xff,
                (true, false) => 0xaa,
                (false, true) => 0x55,
                (false, false) => 0,
            }
        };
        *c = Rgb8 {
            r: level(8),
            g: if i == 12 { 0x55 } else { level(4) },
            b: level(2),
        };
    }
    p[14] = Rgb8 {
        r: 0xaa,
        g: 0xaa,
        b: 0xaa,
    };
    p
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vdc {
    pub regs: [u8; 37],
    addr: u8,
    ram: Vec<u8>,
    hc: u8,
    sc: u8,
    row: u8,
    adjusting: bool,
    adjust: u8,
    /// Scan line within the frame.
    line: u16,
    frame: u64,
    standard: VideoStandard,
    pixels: Vec<u8>,
    /// Lowest frame-buffer line drawn this frame.
    drawn_to: u32,
}

impl Vdc {
    /// A VDC with `ram_kb` (16 or 64) KB of RAM and the C128 KERNAL's
    /// 80 × 25 set-up as power-on values.
    pub fn new(ram_kb: usize, standard: VideoStandard) -> Self {
        let mut regs = [0u8; 37];
        let init: [u8; 37] = [
            126, 80, 102, 0x49, 32, 0, 25, 29, 0, 7, 0x20, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0x08, 0,
            0x78, 8, 0x20, 0x40, 0xf0, 0, 0x20, 7, 0, 0, 0, 0, 125, 100, 5,
        ];
        for (i, v) in init.into_iter().enumerate() {
            regs[i] = v & MASKS[i];
        }
        Self {
            regs,
            addr: 0,
            ram: vec![0; ram_kb.clamp(16, 64) * 1024],
            hc: 0,
            sc: 0,
            row: 0,
            adjusting: false,
            adjust: 0,
            line: 0,
            frame: 0,
            standard,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
            drawn_to: 0,
        }
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    /// Write VDC RAM directly (debugger), leaving the registers alone.
    pub fn poke_ram(&mut self, addr: u16, v: u8) {
        self.set_vram(addr, v);
    }

    fn r16(&self, hi: usize) -> u16 {
        u16::from_be_bytes([self.regs[hi], self.regs[hi + 1]])
    }

    fn set_r16(&mut self, hi: usize, v: u16) {
        [self.regs[hi], self.regs[hi + 1]] = v.to_be_bytes();
    }

    fn vram(&self, a: u16) -> u8 {
        self.ram[usize::from(a) & (self.ram.len() - 1)]
    }

    fn set_vram(&mut self, a: u16, v: u8) {
        let len = self.ram.len();
        self.ram[usize::from(a) & (len - 1)] = v;
    }

    pub fn frame_lines(&self) -> u32 {
        (u32::from(self.regs[4]) + 1) * (u32::from(self.regs[9]) + 1) + u32::from(self.regs[5])
    }

    pub fn signal(&self) -> SignalInfo {
        let cell = u32::from(self.regs[22] >> 4) + 1;
        SignalInfo {
            pixel_clock_hz: CHAR_CLOCK_HZ * 8,
            active_pixels: WIDTH,
            total_pixels: (u32::from(self.regs[0]) + 1) * cell,
            lines_per_field: self.frame_lines(),
            standard: self.standard,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }

    fn vblank(&self) -> bool {
        self.adjusting || self.row >= self.regs[6]
    }

    /// Side-effect-free read of `$D600` (status) or `$D601` (register).
    pub fn peek(&self, reg: u8) -> u8 {
        if reg & 1 == 0 {
            let vblank = if self.vblank() { 0x20 } else { 0 };
            return 0x80 | vblank | if self.ram.len() > 0x4000 { 2 } else { 1 };
        }
        match self.addr {
            31 => self.vram(self.r16(18)),
            a @ 0..=36 => self.regs[usize::from(a)],
            _ => 0xff,
        }
    }

    /// `$D601` reads of R31 advance the update address.
    pub fn read(&mut self, reg: u8) -> u8 {
        let v = self.peek(reg);
        if reg & 1 == 1 && self.addr == 31 {
            let u = self.r16(18).wrapping_add(1);
            self.set_r16(18, u);
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        if reg & 1 == 0 {
            self.addr = v & 0x3f;
            return;
        }
        let a = usize::from(self.addr);
        if a > 36 {
            return;
        }
        match a {
            30 => {
                self.regs[30] = v;
                self.block(v);
            }
            31 => {
                self.regs[31] = v;
                let u = self.r16(18);
                self.set_vram(u, v);
                self.set_r16(18, u.wrapping_add(1));
            }
            _ => self.regs[a] = v & MASKS[a],
        }
    }

    /// R30 written: copy (R24 bit 7) from R32/R33 or fill with R31, `n`
    /// bytes (0 = 256), at the update address.
    fn block(&mut self, n: u8) {
        let count = if n == 0 { 256 } else { u16::from(n) };
        let copy = self.regs[24] & 0x80 != 0;
        let mut dst = self.r16(18);
        let mut src = self.r16(32);
        for _ in 0..count {
            let v = if copy { self.vram(src) } else { self.regs[31] };
            if copy {
                self.regs[31] = v;
                src = src.wrapping_add(1);
            }
            self.set_vram(dst, v);
            dst = dst.wrapping_add(1);
        }
        self.set_r16(18, dst);
        if copy {
            self.set_r16(32, src);
        }
    }

    /// One character clock. Returns true when a new frame starts.
    pub fn tick(&mut self) -> bool {
        let r = &self.regs;
        let shown = !self.adjusting && self.hc < r[1] && self.row < r[6];
        if shown {
            self.draw_char();
        }
        if self.hc >= self.regs[0] {
            self.hc = 0;
            return self.end_line();
        }
        self.hc += 1;
        false
    }

    fn end_line(&mut self) -> bool {
        self.line += 1;
        if self.adjusting {
            self.adjust += 1;
            if self.adjust >= self.regs[5] {
                self.new_frame();
                return true;
            }
            return false;
        }
        if self.sc >= self.regs[9] {
            self.sc = 0;
            self.row = self.row.wrapping_add(1);
            if self.row > self.regs[4] {
                if self.regs[5] > 0 {
                    self.adjusting = true;
                    self.adjust = 0;
                } else {
                    self.new_frame();
                    return true;
                }
            }
        } else {
            self.sc += 1;
        }
        false
    }

    fn new_frame(&mut self) {
        self.row = 0;
        self.sc = 0;
        self.adjusting = false;
        self.line = 0;
        self.frame += 1;
        // The margin, and window lines not drawn this frame (after a switch
        // to fewer rows), show the background colour.
        let bg = self.regs[26] & 0x0f;
        let w = WIDTH as usize;
        let window_end = ((HEIGHT - MARGIN_Y) * WIDTH) as usize;
        let drawn = (self.drawn_to.max(MARGIN_Y) * WIDTH) as usize;
        self.pixels[..(MARGIN_Y * WIDTH) as usize].fill(bg);
        self.pixels[drawn.min(window_end)..].fill(bg);
        for row in self.pixels.chunks_mut(w) {
            row[..MARGIN_X as usize].fill(bg);
            row[w - MARGIN_X as usize..].fill(bg);
        }
        self.drawn_to = 0;
    }

    fn draw_char(&mut self) {
        let r = &self.regs;
        let y = MARGIN_Y + u32::from(self.row) * (u32::from(r[9]) + 1) + u32::from(self.sc);
        let x0 = MARGIN_X + u32::from(self.hc) * 8;
        if y >= HEIGHT - MARGIN_Y || x0 + 8 > WIDTH - MARGIN_X {
            return;
        }
        let stride = u16::from(r[1]).wrapping_add(u16::from(r[27]));
        let cell = u16::from(self.row)
            .wrapping_mul(stride)
            .wrapping_add(u16::from(self.hc));
        let attrs = r[25] & 0x40 != 0;
        let attr = if attrs {
            self.vram(self.r16(20).wrapping_add(cell))
        } else {
            r[26] >> 4
        };
        let bitmap = r[25] & 0x80 != 0;
        let sc = u16::from(self.sc);
        let mut pattern = if bitmap {
            let line = u16::from(self.row) * (u16::from(r[9]) + 1) + sc;
            self.vram(
                self.r16(12)
                    .wrapping_add(line.wrapping_mul(stride))
                    .wrapping_add(u16::from(self.hc)),
            )
        } else {
            let code = u16::from(self.vram(self.r16(12).wrapping_add(cell)));
            let alt = if attrs && attr & 0x80 != 0 { 256 } else { 0 };
            let bytes = if r[9] < 16 { 16 } else { 32 };
            let base = u16::from(r[28] & 0xe0) << 8;
            if self.sc < r[23].max(1) || r[23] == 0 {
                self.vram(base.wrapping_add((code + alt) * bytes + sc))
            } else {
                0
            }
        };
        let blink_on = (self.frame >> if r[24] & 0x20 != 0 { 5 } else { 4 }) & 1 == 0;
        if attrs && !bitmap {
            if attr & 0x20 != 0 && self.sc == r[29] & 0x1f {
                pattern = 0xff;
            }
            if attr & 0x10 != 0 && !blink_on {
                pattern = 0;
            }
            if attr & 0x40 != 0 {
                pattern = !pattern;
            }
        }
        // Cursor.
        let cursor_at = self.r16(12).wrapping_add(cell) == self.r16(14);
        if cursor_at && !bitmap {
            let mode = (r[10] >> 5) & 3;
            let visible = match mode {
                0 => true,
                1 => false,
                2 => (self.frame >> 4) & 1 == 0,
                _ => (self.frame >> 5) & 1 == 0,
            };
            let lines = (r[10] & 0x1f)..=(r[11] & 0x1f);
            if visible && lines.contains(&self.sc) {
                pattern = !pattern;
            }
        }
        if r[24] & 0x40 != 0 {
            pattern = !pattern;
        }
        let fg = attr & 0x0f;
        let bg = r[26] & 0x0f;
        self.drawn_to = self.drawn_to.max(y + 1);
        let at = (y * WIDTH + x0) as usize;
        for (i, p) in self.pixels[at..at + 8].iter_mut().enumerate() {
            *p = if pattern & (0x80 >> i) != 0 { fg } else { bg };
        }
    }
}

const TAG: [u8; 4] = *b"8563";
const VERSION: u16 = 1;

impl Savestate for Vdc {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.regs);
            w.u8(self.addr);
            w.u8(self.hc);
            w.u8(self.sc);
            w.u8(self.row);
            w.bool(self.adjusting);
            w.u8(self.adjust);
            w.u16(self.line);
            w.u64(self.frame);
            w.u32(self.drawn_to);
            w.bytes(&self.ram);
            w.raw(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut v = Vdc::new(self.ram.len() / 1024, self.standard);
        c.raw_into(&mut v.regs)?;
        v.addr = c.u8()? & 0x3f;
        v.hc = c.u8()?;
        v.sc = c.u8()?;
        v.row = c.u8()?;
        v.adjusting = c.bool()?;
        v.adjust = c.u8()?;
        v.line = c.u16()?;
        v.frame = c.u64()?;
        v.drawn_to = c.u32()?.min(HEIGHT);
        let ram = c.bytes()?;
        if ram.len() != v.ram.len() {
            return Err(StateError::Invalid("VDC RAM size differs"));
        }
        v.ram.copy_from_slice(ram);
        c.raw_into(&mut v.pixels)?;
        for p in &mut v.pixels {
            *p &= 0x0f;
        }
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: &mut Vdc) -> u32 {
        let mut clocks = 1;
        while !v.tick() {
            clocks += 1;
        }
        clocks
    }

    fn set(v: &mut Vdc, reg: u8, value: u8) {
        v.write(0, reg);
        v.write(1, value);
    }

    #[test]
    fn ram_through_update_address_and_data() {
        let mut v = Vdc::new(16, VideoStandard::Pal);
        set(&mut v, 18, 0x12);
        set(&mut v, 19, 0x34);
        set(&mut v, 31, 0xaa);
        set(&mut v, 31, 0xbb);
        assert_eq!(&v.ram()[0x1234..0x1236], &[0xaa, 0xbb]);
        set(&mut v, 18, 0x12);
        set(&mut v, 19, 0x34);
        v.write(0, 31);
        assert_eq!(v.peek(1), 0xaa, "peek does not advance");
        assert_eq!(v.read(1), 0xaa);
        assert_eq!(v.read(1), 0xbb);
        // 16 KB wrap.
        set(&mut v, 18, 0x40);
        set(&mut v, 19, 0x00);
        set(&mut v, 31, 0x77);
        assert_eq!(v.ram()[0], 0x77);
    }

    #[test]
    fn block_fill_and_copy() {
        let mut v = Vdc::new(64, VideoStandard::Pal);
        set(&mut v, 18, 0x10);
        set(&mut v, 19, 0x00);
        set(&mut v, 24, 0x00); // fill
        set(&mut v, 31, 0x55); // writes one byte
        set(&mut v, 30, 9); // nine more
        assert!(v.ram()[0x1000..0x100a].iter().all(|&b| b == 0x55));
        assert_eq!(v.ram()[0x100a], 0);
        set(&mut v, 32, 0x10);
        set(&mut v, 33, 0x00);
        set(&mut v, 18, 0x20);
        set(&mut v, 19, 0x00);
        set(&mut v, 24, 0x80); // copy
        set(&mut v, 30, 0); // 256 bytes
        assert_eq!(
            &v.ram()[0x2000..0x200b],
            &v.ram()[0x1000..0x100b].to_vec()[..]
        );
        assert_eq!(v.r16(18), 0x2100);
        assert_eq!(v.r16(32), 0x1100);
    }

    #[test]
    fn frame_timing_and_status() {
        let mut v = Vdc::new(16, VideoStandard::Ntsc);
        frame(&mut v);
        // 127 clocks × (33 rows × 8 lines) at 2 MHz: ~59.7 Hz.
        assert_eq!(frame(&mut v), 127 * 33 * 8);
        assert_eq!(v.peek(0) & 0x80, 0x80, "always ready");
        assert_eq!(v.peek(0) & 0x20, 0, "frame start is not blanked");
    }

    #[test]
    fn text_with_attributes() {
        let mut v = Vdc::new(16, VideoStandard::Pal);
        // Screen at 0, attributes at $0800, characters at $2000.
        v.ram[0] = 1;
        v.ram[1] = 1;
        v.ram[0x0800] = 0x0d; // light yellow
        v.ram[0x0801] = 0x40 | 0x03; // reverse, light blue
        v.ram[0x2000 + 16] = 0xf0; // character 1, line 0
        set(&mut v, 14, 0x07);
        set(&mut v, 15, 0xd0); // cursor away
        frame(&mut v);
        frame(&mut v);
        let row: Vec<u8> = (0..16)
            .map(|x| v.pixels()[(MARGIN_Y * WIDTH + MARGIN_X + x) as usize])
            .collect();
        assert_eq!(row, [13, 13, 13, 13, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3]);
        assert_eq!(v.pixels()[0], 0, "margin in the background colour");
    }

    #[test]
    fn palette_is_rgbi() {
        let p = palette();
        assert_eq!(p[0], Rgb8::default());
        assert_eq!(
            p[15],
            Rgb8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(
            p[2],
            Rgb8 {
                r: 0,
                g: 0,
                b: 0xaa
            }
        );
        assert_eq!(
            p[12],
            Rgb8 {
                r: 0xaa,
                g: 0x55,
                b: 0
            },
            "brown"
        );
    }

    #[test]
    fn savestate_round_trip() {
        let mut v = Vdc::new(64, VideoStandard::Pal);
        set(&mut v, 31, 0x42);
        for _ in 0..12_345 {
            v.tick();
        }
        let mut w = StateWriter::new();
        v.save(&mut w);
        let data = w.into_bytes();
        let mut u = Vdc::new(64, VideoStandard::Pal);
        u.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(u, v);
        let mut small = Vdc::new(16, VideoStandard::Pal);
        assert!(small.load(&mut StateReader::new(&data)).is_err());
    }
}
