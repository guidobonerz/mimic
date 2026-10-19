//! A minimal raster video chip for the reference machine.
//!
//! Not real hardware: a deliberately simple chip with C64-like PAL timing so
//! the beam API, partial frames, raster IRQs, line stepping and the raster
//! trace can be tested end to end before the VIC-II exists.
//!
//! Timing: 312 lines × 63 cycles per field, 8 pixels per cycle. The visible
//! area is lines 0–271 and cycles 0–47 (384 × 272 pixels); a 320 × 200
//! bitmap window sits at lines 36–235, cycles 4–43, read from RAM at
//! `$2000` (40 bytes per line, MSB = leftmost pixel).
//!
//! Registers (`$D000` page, unlisted addresses read `$FF`):
//!
//! | Addr    | R/W | Meaning                                            |
//! | ------- | --- | -------------------------------------------------- |
//! | `$D000` | R/W | border colour (bits 0–3)                           |
//! | `$D001` | R/W | bitmap background colour (bits 0–3)                |
//! | `$D002` | R/W | bitmap foreground colour (bits 0–3)                |
//! | `$D010` | R   | current raster line, bits 0–7                      |
//! | `$D011` | R   | current raster line, bit 8 in bit 0                |
//! | `$D012` | R/W | raster IRQ compare line, bits 0–7                  |
//! | `$D013` | R/W | raster IRQ compare line, bit 8 in bit 0            |
//! | `$D019` | R/W | IRQ status: bit 0 raster; write 1 to clear         |
//! | `$D01A` | R/W | IRQ enable: bit 0 raster                           |

use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use emu_core::{BeamPos, Savestate, StateError, StateReader, StateWriter};

pub const LINES: u16 = 312;
pub const CYCLES_PER_LINE: u16 = 63;
pub const VISIBLE_LINES: u16 = 272;
pub const VISIBLE_CYCLES: u16 = 48;
pub const WIDTH: u32 = VISIBLE_CYCLES as u32 * 8;
pub const HEIGHT: u32 = VISIBLE_LINES as u32;

const WINDOW_LINES: std::ops::Range<u16> = 36..236;
const WINDOW_CYCLES: std::ops::Range<u16> = 4..44;
pub const BITMAP_BASE: u16 = 0x2000;

pub const IO_BASE: u16 = 0xd000;
pub const IO_END: u16 = 0xd0ff;

/// 16 colours, chosen for this test chip (not taken from any emulator).
pub const PALETTE: [Rgb8; 16] = [
    rgb(0x00, 0x00, 0x00),
    rgb(0xff, 0xff, 0xff),
    rgb(0xaa, 0x00, 0x00),
    rgb(0x00, 0xaa, 0xaa),
    rgb(0xaa, 0x00, 0xaa),
    rgb(0x00, 0xaa, 0x00),
    rgb(0x00, 0x00, 0xaa),
    rgb(0xff, 0xff, 0x55),
    rgb(0xaa, 0x55, 0x00),
    rgb(0x55, 0x33, 0x00),
    rgb(0xff, 0x55, 0x55),
    rgb(0x33, 0x33, 0x33),
    rgb(0x77, 0x77, 0x77),
    rgb(0x55, 0xff, 0x55),
    rgb(0x55, 0x55, 0xff),
    rgb(0xbb, 0xbb, 0xbb),
];

const fn rgb(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 { r, g, b }
}

const SIGNAL: SignalInfo = SignalInfo {
    pixel_clock_hz: 7_881_984,
    active_pixels: WIDTH,
    total_pixels: CYCLES_PER_LINE as u32 * 8,
    lines_per_field: LINES as u32,
    standard: VideoStandard::Pal,
    output: OutputType::Rgb,
    field: FieldParity::Progressive,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    pub border: u8,
    pub background: u8,
    pub foreground: u8,
    pub irq_line: u16,
    pub irq_status: u8,
    pub irq_enable: u8,
    pub frame: u64,
    pub line: u16,
    pub cycle: u16,
    pixels: Vec<u8>,
}

impl Default for Video {
    fn default() -> Self {
        Self {
            border: 14,
            background: 6,
            foreground: 1,
            irq_line: 0,
            irq_status: 0,
            irq_enable: 0,
            frame: 0,
            line: 0,
            cycle: 0,
            pixels: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }
}

impl Video {
    pub fn beam(&self) -> BeamPos {
        BeamPos {
            frame: self.frame,
            line: self.line,
            cycle: self.cycle,
            hpos: self.cycle * 8,
        }
    }

    /// IRQ output line (active high).
    pub fn irq(&self) -> bool {
        self.irq_status & self.irq_enable & 1 != 0
    }

    /// Draw the 8 pixels under the beam, then advance the beam one cycle.
    /// Returns true when a new field starts.
    pub fn cycle(&mut self, ram: &[u8; 0x10000]) -> bool {
        if self.line < VISIBLE_LINES && self.cycle < VISIBLE_CYCLES {
            self.draw(ram);
        }
        self.cycle += 1;
        if self.cycle < CYCLES_PER_LINE {
            return false;
        }
        self.cycle = 0;
        self.line += 1;
        let new_field = self.line == LINES;
        if new_field {
            self.line = 0;
            self.frame += 1;
        }
        if self.line == self.irq_line {
            self.irq_status |= 1;
        }
        new_field
    }

    fn draw(&mut self, ram: &[u8; 0x10000]) {
        let row = self.line as usize * WIDTH as usize + self.cycle as usize * 8;
        let out = &mut self.pixels[row..row + 8];
        if WINDOW_LINES.contains(&self.line) && WINDOW_CYCLES.contains(&self.cycle) {
            let offset = (self.line - WINDOW_LINES.start) * 40 + (self.cycle - WINDOW_CYCLES.start);
            let bits = ram[usize::from(BITMAP_BASE + offset)];
            for (i, px) in out.iter_mut().enumerate() {
                *px = if bits & (0x80 >> i) != 0 {
                    self.foreground
                } else {
                    self.background
                };
            }
        } else {
            out.fill(self.border);
        }
    }

    #[cfg(test)]
    pub(crate) fn clear_pixels(&mut self) {
        self.pixels.fill(0);
    }

    pub fn peek(&self, addr: u16) -> u8 {
        match addr {
            0xd000 => self.border,
            0xd001 => self.background,
            0xd002 => self.foreground,
            0xd010 => self.line as u8,
            0xd011 => (self.line >> 8) as u8,
            0xd012 => self.irq_line as u8,
            0xd013 => (self.irq_line >> 8) as u8,
            0xd019 => self.irq_status,
            0xd01a => self.irq_enable,
            _ => 0xff,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr {
            0xd000 => self.border = v & 15,
            0xd001 => self.background = v & 15,
            0xd002 => self.foreground = v & 15,
            0xd012 => self.irq_line = (self.irq_line & 0x100) | u16::from(v),
            0xd013 => self.irq_line = (self.irq_line & 0xff) | (u16::from(v & 1) << 8),
            0xd019 => self.irq_status &= !v,
            0xd01a => self.irq_enable = v & 1,
            _ => {}
        }
    }

    pub fn frame(&self) -> FrameRef<'_> {
        FrameRef {
            width: WIDTH,
            height: HEIGHT,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &PALETTE,
            },
            luma_chroma: None,
            signal: SIGNAL,
        }
    }
}

const TAG: [u8; 4] = *b"SIMV";
const VERSION: u16 = 1;

impl Savestate for Video {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.border);
            w.u8(self.background);
            w.u8(self.foreground);
            w.u16(self.irq_line);
            w.u8(self.irq_status);
            w.u8(self.irq_enable);
            w.u64(self.frame);
            w.u16(self.line);
            w.u16(self.cycle);
            w.bytes(&self.pixels);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let v = Video {
            border: c.u8()? & 15,
            background: c.u8()? & 15,
            foreground: c.u8()? & 15,
            irq_line: c.u16()? & 0x1ff,
            irq_status: c.u8()? & 1,
            irq_enable: c.u8()? & 1,
            frame: c.u64()?,
            line: c.u16()?,
            cycle: c.u16()?,
            pixels: c.bytes()?.to_vec(),
        };
        if v.line >= LINES || v.cycle >= CYCLES_PER_LINE {
            return Err(StateError::Invalid("video beam position"));
        }
        if v.pixels.len() != (WIDTH * HEIGHT) as usize || v.pixels.iter().any(|&p| p > 15) {
            return Err(StateError::Invalid("video pixels"));
        }
        *self = v;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_is_312_lines_of_63_cycles() {
        let ram = [0u8; 0x10000];
        let mut v = Video::default();
        let mut cycles = 0u32;
        while !v.cycle(&ram) {
            cycles += 1;
        }
        assert_eq!(cycles + 1, 312 * 63);
        assert_eq!(
            v.beam(),
            BeamPos {
                frame: 1,
                line: 0,
                cycle: 0,
                hpos: 0
            }
        );
    }

    #[test]
    fn draws_border_and_bitmap() {
        let mut ram = [0u8; 0x10000];
        ram[usize::from(BITMAP_BASE)] = 0b1000_0001;
        let mut v = Video::default();
        while !v.cycle(&ram) {}
        let f = v.frame();
        assert_eq!(f.rgb_at(0, 0), Some(PALETTE[14]), "border");
        assert_eq!(f.rgb_at(32, 36), Some(PALETTE[1]), "first bitmap pixel set");
        assert_eq!(f.rgb_at(33, 36), Some(PALETTE[6]), "background");
        assert_eq!(f.rgb_at(39, 36), Some(PALETTE[1]));
    }

    #[test]
    fn raster_irq_latches_and_clears() {
        let ram = [0u8; 0x10000];
        let mut v = Video::default();
        v.write(0xd012, 2);
        v.write(0xd01a, 1);
        for _ in 0..63 * 2 - 1 {
            v.cycle(&ram);
        }
        assert!(!v.irq());
        v.cycle(&ram);
        assert_eq!(v.peek(0xd010), 2);
        assert!(v.irq());
        v.write(0xd019, 1);
        assert!(!v.irq());
    }

    #[test]
    fn savestate_round_trip_and_validation() {
        let ram = [0u8; 0x10000];
        let mut v = Video::default();
        for _ in 0..1000 {
            v.cycle(&ram);
        }
        let mut w = StateWriter::new();
        v.save(&mut w);
        let data = w.into_bytes();
        let mut u = Video::default();
        u.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(u, v);

        let mut bad = data.clone();
        // line field: header 10 + 3 colours + 2 + 2 + 8 frame = offset 25.
        bad[25] = 0xff;
        bad[26] = 0xff;
        assert!(u.load(&mut StateReader::new(&bad)).is_err());
        assert_eq!(u, v, "failed load leaves state unchanged");
    }
}
