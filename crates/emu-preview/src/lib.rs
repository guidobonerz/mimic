//! Live preview: frame encoders, snapshot endpoint, WebSocket stream, overlays.
//!
//! Works on [`FrameRef`] only (golden rule 1). Phase 1 provides the PNG
//! encoder used by snapshots and the headless runner.

#![forbid(unsafe_code)]

use emu_core::FrameRef;

/// Encode the raw frame (no CRT processing) as an 8-bit RGB PNG.
/// Invalid palette indices become black.
pub fn encode_png(frame: &FrameRef<'_>) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(frame.width as usize * frame.height as usize * 3);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let c = frame.rgb_at(x, y).unwrap_or_default();
            rgb.extend_from_slice(&[c.r, c.g, c.b]);
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, frame.width, frame.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        // Writing into a Vec cannot fail for valid dimensions; an empty
        // frame yields an empty buffer instead of a panic.
        let Ok(mut writer) = enc.write_header() else {
            return Vec::new();
        };
        if writer.write_image_data(&rgb).is_err() {
            return Vec::new();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{FieldParity, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard};

    #[test]
    fn png_round_trip() {
        let palette = [
            Rgb8 { r: 1, g: 2, b: 3 },
            Rgb8 {
                r: 250,
                g: 128,
                b: 0,
            },
        ];
        let pixels = [0, 1, 1, 0, 9, 1];
        let frame = FrameRef {
            width: 3,
            height: 2,
            pixels: PixelData::Indexed {
                pixels: &pixels,
                palette: &palette,
            },
            luma_chroma: None,
            signal: SignalInfo {
                pixel_clock_hz: 1,
                active_pixels: 3,
                total_pixels: 3,
                lines_per_field: 2,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        };
        let data = encode_png(&frame);
        let decoder = png::Decoder::new(data.as_slice());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..6], &[1, 2, 3, 250, 128, 0]);
        assert_eq!(&buf[12..15], &[0, 0, 0], "invalid index is black");
    }
}
