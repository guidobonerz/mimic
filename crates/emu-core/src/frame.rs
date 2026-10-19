//! Raw video output of a machine (golden rule 9: CRT effects come later, in
//! `video-crt`). Cropping and aspect correction happen downstream from the
//! signal metadata, never inside the machine.

/// 8-bit-per-channel color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoStandard {
    Pal,
    Ntsc,
}

/// The connection the machine's video output is designed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputType {
    Composite,
    /// Separate luma/chroma (C64, C128 40-col, Atari 800XL).
    SVideo,
    Rgb,
    Monochrome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldParity {
    Progressive,
    /// Interlaced even (long) field.
    Even,
    /// Interlaced odd (short) field.
    Odd,
}

/// Timing of the video signal, needed for aspect correction and CRT emulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignalInfo {
    pub pixel_clock_hz: u32,
    /// Visible pixels per line (including border), i.e. the frame width.
    pub active_pixels: u32,
    /// Pixels per line including horizontal blanking.
    pub total_pixels: u32,
    /// Lines per field including vertical blanking.
    pub lines_per_field: u32,
    pub standard: VideoStandard,
    pub output: OutputType,
    pub field: FieldParity,
}

/// Per-pixel composite components for colors a palette cannot express.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LumaChroma {
    pub luma: u16,
    /// Chroma phase in 1/65536 of a full turn.
    pub chroma_phase: u16,
    pub chroma_amplitude: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelData<'a> {
    /// Palette indices (8-bit machines).
    Indexed { pixels: &'a [u8], palette: &'a [Rgb8] },
    /// Direct color (Amiga, HAM).
    Rgb(&'a [Rgb8]),
}

/// A borrowed view of one field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRef<'a> {
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height` entries.
    pub pixels: PixelData<'a>,
    /// Optional composite components, same layout as `pixels`.
    pub luma_chroma: Option<&'a [LumaChroma]>,
    pub signal: SignalInfo,
}

impl FrameRef<'_> {
    /// Resolved color at `(x, y)`; `None` if out of bounds or the palette
    /// index is invalid.
    pub fn rgb_at(&self, x: u32, y: u32) -> Option<Rgb8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.width as usize + x as usize;
        match self.pixels {
            PixelData::Indexed { pixels, palette } => {
                pixels.get(i).and_then(|&p| palette.get(usize::from(p))).copied()
            }
            PixelData::Rgb(pixels) => pixels.get(i).copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_at_resolves_palette_and_bounds() {
        let palette = [Rgb8 { r: 0, g: 0, b: 0 }, Rgb8 { r: 255, g: 255, b: 255 }];
        let pixels = [0, 1, 1, 7];
        let frame = FrameRef {
            width: 2,
            height: 2,
            pixels: PixelData::Indexed { pixels: &pixels, palette: &palette },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 7_881_984,
                active_pixels: 2,
                total_pixels: 504,
                lines_per_field: 312,
                standard: VideoStandard::Pal,
                output: OutputType::SVideo,
                field: FieldParity::Progressive,
            },
        };
        assert_eq!(frame.rgb_at(1, 0), Some(palette[1]));
        assert_eq!(frame.rgb_at(1, 1), None, "palette index 7 is invalid");
        assert_eq!(frame.rgb_at(2, 0), None);
    }
}
