//! Field stage: interlaced fields (Amiga LACE) into frames.
//!
//! Machines deliver one field at a time with its parity
//! ([`FieldParity::Even`] = the long field, the even output lines;
//! [`FieldParity::Odd`] = the short field, the odd ones). For interlaced
//! input the stage builds a frame of twice the lines:
//!
//! - [`Deinterlace::Weave`]: both fields interleaved, each line kept until
//!   the next field of its parity replaces it (sharp, combs on motion);
//! - [`Deinterlace::Bob`]: the current field's lines doubled (no combing,
//!   half the vertical resolution, flickers).
//!
//! Progressive input passes through unchanged ([`FieldStage::process`]
//! returns `None`). The stage keeps its own field memory, never machine
//! state (savestates stay CRT-independent).

use emu_core::frame::{FieldParity, FrameRef, Rgb8};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Deinterlace {
    #[default]
    Weave,
    Bob,
}

#[derive(Clone, Debug, Default)]
pub struct FieldStage {
    pub mode: Deinterlace,
    width: u32,
    height: u32,
    buf: Vec<Rgb8>,
}

impl FieldStage {
    pub fn new(mode: Deinterlace) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    /// A field in; for interlaced input the frame (width, 2 × height,
    /// pixels), else `None`.
    pub fn process(&mut self, field: &FrameRef<'_>) -> Option<(u32, u32, &[Rgb8])> {
        let offset = match field.signal.field {
            FieldParity::Progressive => {
                self.buf.clear();
                return None;
            }
            FieldParity::Even => 0,
            FieldParity::Odd => 1,
        };
        let (w, h) = (field.width, field.height);
        if (w, h) != (self.width, self.height) || self.buf.is_empty() {
            self.width = w;
            self.height = h;
            self.buf = vec![Rgb8::default(); (w * 2 * h) as usize];
        }
        for y in 0..h {
            let rows: &[u32] = match self.mode {
                Deinterlace::Weave => &[2 * y + offset],
                Deinterlace::Bob => &[2 * y, 2 * y + 1],
            };
            for &row in rows {
                let base = (row * w) as usize;
                for x in 0..w {
                    self.buf[base + x as usize] = field.rgb_at(x, y).unwrap_or_default();
                }
            }
        }
        Some((w, 2 * h, &self.buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{OutputType, PixelData, SignalInfo, VideoStandard};

    fn field(px: &[Rgb8], parity: FieldParity) -> FrameRef<'_> {
        FrameRef {
            width: 1,
            height: 2,
            pixels: PixelData::Rgb(px),
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 14_187_580,
                active_pixels: 1,
                total_pixels: 908,
                lines_per_field: 313,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: parity,
            },
        }
    }

    const fn c(v: u8) -> Rgb8 {
        Rgb8 { r: v, g: v, b: v }
    }

    #[test]
    fn weave_interleaves_both_fields() {
        let mut s = FieldStage::new(Deinterlace::Weave);
        s.process(&field(&[c(1), c(3)], FieldParity::Even));
        let (w, h, px) = s.process(&field(&[c(2), c(4)], FieldParity::Odd)).unwrap();
        assert_eq!((w, h), (1, 4));
        assert_eq!(px, &[c(1), c(2), c(3), c(4)]);
    }

    #[test]
    fn bob_doubles_lines_and_progressive_passes() {
        let mut s = FieldStage::new(Deinterlace::Bob);
        let (_, _, px) = s.process(&field(&[c(1), c(3)], FieldParity::Odd)).unwrap();
        assert_eq!(px, &[c(1), c(1), c(3), c(3)]);
        assert!(s
            .process(&field(&[c(1), c(3)], FieldParity::Progressive))
            .is_none());
    }
}
