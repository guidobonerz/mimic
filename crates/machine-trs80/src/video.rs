//! The TRS-80 picture: 64 × 16 characters (or 32 × 16 with port `$FF`
//! bit 3 set, every dot twice as wide) from the video RAM, a 6 × 12-dot
//! cell each. Codes `$80–$FF` are block graphics: six blocks in a 2 × 3
//! grid (bits 0/1 the top left/right, 2/3 the middle, 4/5 the bottom; each
//! 3 dots wide and 4 lines high), bit 6 ignored. Text uses the character
//! generator ([`crate::charset`]); without the lowercase modification
//! bit 6 comes from the NOR of bits 5 and 7 as in the video RAM.
//!
//! Timing (60 Hz, [`Timing`]): Model I 10.6445 MHz dot clock, 672 dots a
//! line (384 shown), the CPU at a sixth: 112 CPU clocks a line; Model III
//! 10.1376 MHz, 640 dots, the CPU at a fifth: 128 clocks a line; 264
//! lines a field (192 shown). The picture is composed from the video
//! RAM at the end of each field (the Model I has no raster effects the
//! software can use). Source: TRS-80 Model I Technical Reference Manual
//! (video circuit); the line and field counts are this project's reading
//! of it.

use crate::charset::FONT;

pub const COLUMNS: usize = 64;
pub const ROWS: usize = 16;
pub const CELL_W: usize = 6;
pub const CELL_H: usize = 12;
/// Border around the 384 × 192 text area.
pub const BORDER_X: usize = 32;
pub const BORDER_Y: usize = 24;
pub const WIDTH: u32 = (COLUMNS * CELL_W + 2 * BORDER_X) as u32;
pub const HEIGHT: u32 = (ROWS * CELL_H + 2 * BORDER_Y) as u32;

/// Clocks and line timing of a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub cpu_hz: u32,
    pub dot_hz: u32,
    pub dots_per_line: u32,
    pub lines: u32,
    /// CPU clocks a line.
    pub t_per_line: u64,
}

impl Timing {
    pub const fn t_per_field(&self) -> u64 {
        self.t_per_line * self.lines as u64
    }
}

/// Model I: 10.6445 MHz dots, the CPU at a sixth, 672 dots a line.
pub const MODEL_I: Timing = Timing {
    cpu_hz: 1_774_080,
    dot_hz: 10_644_480,
    dots_per_line: 672,
    lines: 264,
    t_per_line: 112,
};

/// Model III: 10.1376 MHz dots, the CPU at a fifth (2.028 MHz), 640 dots
/// a line.
pub const MODEL_III: Timing = Timing {
    cpu_hz: 2_027_520,
    dot_hz: 10_137_600,
    dots_per_line: 640,
    lines: 264,
    t_per_line: 128,
};

/// The six dots of a cell line: text from the font, graphics from the
/// blocks.
fn cell_line(code: u8, line: usize, lowercase: bool) -> u8 {
    if code & 0x80 != 0 {
        let pair = (code >> (2 * (line / 4))) & 3;
        (if pair & 1 != 0 { 0b111_000 } else { 0 }) | (if pair & 2 != 0 { 0b000_111 } else { 0 })
    } else {
        let c = if lowercase {
            code & 0x7f
        } else {
            (code & 0x3f) | (u8::from(code & 0x20 == 0) << 6)
        };
        FONT[usize::from(c)][line]
    }
}

/// Compose the field into `pixels` (`WIDTH × HEIGHT`, 1 = lit).
pub fn render(vram: &[u8], wide: bool, lowercase: bool, pixels: &mut [u8]) {
    pixels.fill(0);
    let w = WIDTH as usize;
    for row in 0..ROWS {
        for line in 0..CELL_H {
            let y = BORDER_Y + row * CELL_H + line;
            let out = &mut pixels[y * w + BORDER_X..y * w + BORDER_X + COLUMNS * CELL_W];
            for col in 0..COLUMNS {
                // 32-column mode shows the even columns, each dot doubled.
                let (src, x0, scale) = if wide {
                    if col >= COLUMNS / 2 {
                        break;
                    }
                    (col * 2, col * CELL_W * 2, 2)
                } else {
                    (col, col * CELL_W, 1)
                };
                let dots = cell_line(vram[row * COLUMNS + src], line, lowercase);
                for d in 0..CELL_W {
                    if dots & (0x20 >> d) != 0 {
                        for s in 0..scale {
                            out[x0 + d * scale + s] = 1;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_blocks() {
        assert_eq!(cell_line(0x81, 0, false), 0b111_000);
        assert_eq!(cell_line(0x82, 3, false), 0b000_111);
        assert_eq!(cell_line(0x8c, 5, false), 0b111_111);
        assert_eq!(cell_line(0xb0, 11, false), 0b111_111);
        assert_eq!(cell_line(0xb0, 7, false), 0);
    }

    #[test]
    fn text_and_wide_mode() {
        let mut vram = vec![0x20u8; 1024];
        vram[0] = b'A';
        let mut px = vec![0u8; (WIDTH * HEIGHT) as usize];
        render(&vram, false, false, &mut px);
        let lit = px.iter().filter(|&&p| p != 0).count();
        assert!(lit > 10, "an A");
        render(&vram, true, false, &mut px);
        assert_eq!(
            px.iter().filter(|&&p| p != 0).count(),
            lit * 2,
            "dots doubled"
        );
    }
}
