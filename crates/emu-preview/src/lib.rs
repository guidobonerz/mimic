//! Live preview: frame encoders, snapshot endpoint, WebSocket stream, overlays.
//!
//! Works on [`FrameRef`] only (golden rule 1). Frames come raw (the
//! default, what tests compare) or through the CPU CRT path of `video-crt`
//! (`filter=crt`). The HTTP/WebSocket server ([`server`], native targets
//! only) encodes from [`OwnedFrame`] copies on its own threads; the
//! emulator never waits for it.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
pub mod server;

use emu_core::frame::{PixelData, Rgb8, SignalInfo};
use emu_core::{BeamPos, FrameRef, Machine, ScreenId};
use video_crt::Profile;

/// Encode the raw frame (no CRT processing) as an 8-bit RGB PNG.
/// Invalid palette indices become black.
pub fn encode_png(frame: &FrameRef<'_>) -> Vec<u8> {
    let img = RgbImage::from_frame(frame);
    png_rgb(img.width, img.height, &img.data)
}

/// Encode the frame through a monitor profile at vertical `scale` (rows
/// per field line; 2 is the natural TV size).
pub fn encode_png_crt(frame: &FrameRef<'_>, profile: &Profile, scale: u32) -> Vec<u8> {
    let img = video_crt::render(frame, profile, scale);
    png_rgb(img.width, img.height, &img.rgb)
}

fn png_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        // Writing into a Vec cannot fail for valid dimensions; an empty
        // frame yields an empty buffer instead of a panic.
        let Ok(mut writer) = enc.write_header() else {
            return Vec::new();
        };
        if writer.write_image_data(rgb).is_err() {
            return Vec::new();
        }
    }
    out
}

/// A copy of one field, taken on the emulation thread and encoded elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedFrame {
    pub width: u32,
    pub height: u32,
    /// Resolved colours, row-major.
    pub pixels: Vec<Rgb8>,
    pub signal: SignalInfo,
    pub beam: Option<BeamPos>,
    /// False while a debugger holds the machine: the field may be partial
    /// (lines below the beam are from the previous field).
    pub running: bool,
}

impl OwnedFrame {
    /// A copy of the machine's main output.
    pub fn from_machine(m: &dyn Machine, running: bool) -> Self {
        Self::from_frame(&m.framebuffer(), m.beam(), running)
    }

    /// A copy of one output; `None` if the machine has no such screen.
    pub fn from_screen(m: &dyn Machine, id: ScreenId, running: bool) -> Option<Self> {
        Some(Self::from_frame(&m.screen(id)?, m.beam(), running))
    }

    fn from_frame(f: &FrameRef<'_>, beam: Option<BeamPos>, running: bool) -> Self {
        let mut pixels = Vec::with_capacity(f.width as usize * f.height as usize);
        match f.pixels {
            PixelData::Indexed { pixels: p, palette } => pixels.extend(
                p.iter()
                    .map(|&i| palette.get(usize::from(i)).copied().unwrap_or_default()),
            ),
            PixelData::Rgb(p) => pixels.extend_from_slice(p),
        }
        pixels.resize(f.width as usize * f.height as usize, Rgb8::default());
        Self {
            width: f.width,
            height: f.height,
            pixels,
            signal: f.signal,
            beam,
            running,
        }
    }

    pub fn as_frame_ref(&self) -> FrameRef<'_> {
        FrameRef {
            width: self.width,
            height: self.height,
            pixels: PixelData::Rgb(&self.pixels),
            luma_chroma: None,
            signal: self.signal,
        }
    }
}

/// 8-bit RGB pixels, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn from_frame(frame: &FrameRef<'_>) -> Self {
        let mut data = Vec::with_capacity(frame.width as usize * frame.height as usize * 3);
        for y in 0..frame.height {
            for x in 0..frame.width {
                let c = frame.rgb_at(x, y).unwrap_or_default();
                data.extend_from_slice(&[c.r, c.g, c.b]);
            }
        }
        Self {
            width: frame.width,
            height: frame.height,
            data,
        }
    }

    /// Nearest-neighbour enlargement by an integer factor.
    pub fn scaled(self, factor: u32) -> Self {
        if factor <= 1 {
            return self;
        }
        let (w, f) = (self.width as usize, factor as usize);
        let mut data = Vec::with_capacity(self.data.len() * f * f);
        for row in self.data.chunks_exact(w * 3) {
            let mut line = Vec::with_capacity(w * 3 * f);
            for px in row.as_chunks::<3>().0 {
                for _ in 0..f {
                    line.extend_from_slice(px);
                }
            }
            for _ in 0..f {
                data.extend_from_slice(&line);
            }
        }
        Self {
            width: self.width * factor,
            height: self.height * factor,
            data,
        }
    }

    /// The pixels inside `rect`.
    pub fn crop(&self, rect: Rect) -> Self {
        let mut data = Vec::with_capacity(rect.w as usize * rect.h as usize * 3);
        for y in rect.y..rect.y + rect.h {
            let start = (y as usize * self.width as usize + rect.x as usize) * 3;
            data.extend_from_slice(&self.data[start..start + rect.w as usize * 3]);
        }
        Self {
            width: rect.w,
            height: rect.h,
            data,
        }
    }
}

/// How a preview frame is rendered.
#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    /// Raw pixels, enlarged by an integer factor.
    Raw { scale: u32 },
    /// Through a monitor profile, `scale` rows per field line.
    Crt { profile: Profile, scale: u32 },
}

impl Filter {
    pub fn render(&self, frame: &OwnedFrame) -> RgbImage {
        let f = frame.as_frame_ref();
        match self {
            Filter::Raw { scale } => RgbImage::from_frame(&f).scaled(*scale),
            Filter::Crt { profile, scale } => {
                let img = video_crt::render(&f, profile, *scale);
                RgbImage {
                    width: img.width,
                    height: img.height,
                    data: img.rgb,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The smallest rectangle containing every pixel that differs; `None` if
/// the images are equal. Images of different size differ everywhere.
pub fn dirty_rect(prev: &RgbImage, cur: &RgbImage) -> Option<Rect> {
    let full = Rect {
        x: 0,
        y: 0,
        w: cur.width,
        h: cur.height,
    };
    if prev.width != cur.width || prev.height != cur.height {
        return Some(full);
    }
    let w = cur.width as usize * 3;
    let rows = |i: usize| {
        (
            &prev.data[i * w..(i + 1) * w],
            &cur.data[i * w..(i + 1) * w],
        )
    };
    let h = cur.height as usize;
    let top = (0..h).find(|&y| rows(y).0 != rows(y).1)?;
    let bottom = (top..h).rev().find(|&y| rows(y).0 != rows(y).1)?;
    let (mut left, mut right) = (cur.width as usize, 0);
    for y in top..=bottom {
        let (a, b) = rows(y);
        if let Some(l) =
            (0..cur.width as usize).find(|&x| a[x * 3..x * 3 + 3] != b[x * 3..x * 3 + 3])
        {
            left = left.min(l);
            let r = (l..cur.width as usize)
                .rev()
                .find(|&x| a[x * 3..x * 3 + 3] != b[x * 3..x * 3 + 3])
                .unwrap_or(l);
            right = right.max(r);
        }
    }
    Some(Rect {
        x: left as u32,
        y: top as u32,
        w: (right - left + 1) as u32,
        h: (bottom - top + 1) as u32,
    })
}

/// Image formats of the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Qoi,
}

impl Format {
    pub fn encode(self, img: &RgbImage) -> Vec<u8> {
        match self {
            Format::Png => png_rgb(img.width, img.height, &img.data),
            Format::Qoi => qoi::encode_to_vec(&img.data, img.width, img.height).unwrap_or_default(),
        }
    }

    fn code(self) -> u8 {
        match self {
            Format::Png => 0,
            Format::Qoi => 1,
        }
    }
}

/// Stream message header: magic, version, kind, format, flags, frame,
/// beam line/cycle, full image size, placed rectangle (little endian).
pub const HEADER_LEN: usize = 32;
pub const MAGIC: &[u8; 4] = b"PTPV";
pub const STREAM_VERSION: u8 = 1;

/// One stream message. `key`: the image replaces everything (rect = full
/// size); otherwise it is drawn at `rect` over the previous picture. An
/// empty `rect` carries no image: the picture is unchanged and only the
/// header (beam, paused flag) is news.
pub fn stream_message(
    frame: &OwnedFrame,
    full: &RgbImage,
    rect: Rect,
    key: bool,
    format: Format,
) -> Vec<u8> {
    let image = if key {
        format.encode(full)
    } else if rect.w == 0 || rect.h == 0 {
        Vec::new()
    } else {
        format.encode(&full.crop(rect))
    };
    let mut m = Vec::with_capacity(HEADER_LEN + image.len());
    m.extend_from_slice(MAGIC);
    m.push(STREAM_VERSION);
    m.push(u8::from(!key));
    m.push(format.code());
    m.push(u8::from(!frame.running));
    let beam = frame.beam.unwrap_or_default();
    m.extend_from_slice(&beam.frame.to_le_bytes());
    let (line, cycle) = match frame.beam {
        Some(b) => (b.line, b.cycle),
        None => (u16::MAX, u16::MAX),
    };
    m.extend_from_slice(&line.to_le_bytes());
    m.extend_from_slice(&cycle.to_le_bytes());
    for v in [full.width, full.height, rect.x, rect.y, rect.w, rect.h] {
        m.extend_from_slice(&(v.min(0xffff) as u16).to_le_bytes());
    }
    m.extend_from_slice(&image);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{FieldParity, OutputType, VideoStandard};

    fn signal() -> SignalInfo {
        SignalInfo {
            pixel_clock_hz: 1,
            active_pixels: 3,
            total_pixels: 3,
            lines_per_field: 2,
            standard: VideoStandard::Pal,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        }
    }

    fn decode_png(data: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(data);
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

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
            signal: signal(),
        };
        let (w, h, buf) = decode_png(&encode_png(&frame));
        assert_eq!((w, h), (3, 2));
        assert_eq!(&buf[..6], &[1, 2, 3, 250, 128, 0]);
        assert_eq!(&buf[12..15], &[0, 0, 0], "invalid index is black");
    }

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> RgbImage {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[f(x, y), 0, 0]);
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn dirty_rect_bounds_the_changes() {
        let a = image(8, 6, |_, _| 0);
        assert_eq!(dirty_rect(&a, &a), None);
        let b = image(8, 6, |x, y| {
            u8::from((x == 2 && y == 1) || (x == 5 && y == 4))
        });
        assert_eq!(
            dirty_rect(&a, &b),
            Some(Rect {
                x: 2,
                y: 1,
                w: 4,
                h: 4
            })
        );
        let c = image(4, 4, |_, _| 0);
        assert_eq!(dirty_rect(&a, &c).unwrap().w, 4, "size change: full frame");
        let crop = b.crop(dirty_rect(&a, &b).unwrap());
        assert_eq!((crop.width, crop.height), (4, 4));
        assert_eq!(
            crop.data[0], 1,
            "top-left of the rectangle is the first change"
        );
    }

    #[test]
    fn scaling_and_formats() {
        let img = image(2, 1, |x, _| x as u8 * 100).scaled(2);
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.data[..12], [0, 0, 0, 0, 0, 0, 100, 0, 0, 100, 0, 0]);
        let (w, h, data) = decode_png(&Format::Png.encode(&img));
        assert_eq!((w, h, data), (4, 2, img.data.clone()));
        let (header, data) = qoi::decode_to_vec(Format::Qoi.encode(&img)).unwrap();
        assert_eq!((header.width, header.height), (4, 2));
        assert_eq!(data, img.data);
    }

    #[test]
    fn stream_message_header() {
        let frame = OwnedFrame {
            width: 2,
            height: 1,
            pixels: vec![Rgb8::default(); 2],
            signal: signal(),
            beam: Some(BeamPos {
                frame: 7,
                line: 100,
                cycle: 12,
                hpos: 0,
            }),
            running: false,
        };
        let full = image(2, 1, |_, _| 5);
        let rect = Rect {
            x: 1,
            y: 0,
            w: 1,
            h: 1,
        };
        let m = stream_message(&frame, &full, rect, false, Format::Qoi);
        assert_eq!(&m[..4], MAGIC);
        assert_eq!(m[4..8], [STREAM_VERSION, 1, 1, 1], "delta, qoi, paused");
        assert_eq!(u64::from_le_bytes(m[8..16].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes([m[16], m[17]]), 100);
        assert_eq!(u16::from_le_bytes([m[24], m[25]]), 1, "rect x");
        let (header, _) = qoi::decode_to_vec(&m[HEADER_LEN..]).unwrap();
        assert_eq!((header.width, header.height), (1, 1), "only the rectangle");
    }
}
