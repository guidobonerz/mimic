//! Raw stream (`/stream`): uncompressed pixels for local viewers, after the
//! idea of the Ultimate 64's video stream. No image encoding at all: indexed
//! frames go out as palette indices (4 bits per pixel with up to 16 colours,
//! else 8), RGB frames as 24-bit pixels, and after the first frame only the
//! lines that changed. The viewer keeps the picture and colours it itself.
//! A C64 frame is ≈ 57 KB at most, so 50 fps fit easily on loopback or a LAN.
//! Only the idea is taken from the Ultimate 64 ("Data Streams",
//! <https://1541u-documentation.readthedocs.io/en/latest/data_streams.html>);
//! the layout is our own and not compatible with it.
//!
//! Video message (little endian), [`VIDEO_HEADER_LEN`] bytes of header:
//!
//! | Offset | Size | Field                                                        |
//! | ------ | ---- | ------------------------------------------------------------ |
//! | 0      | 4    | magic `PTRV`                                                 |
//! | 4      | 1    | version ([`RAW_VERSION`])                                    |
//! | 5      | 1    | flags: bit 0 key frame, bit 1 paused, bit 2 palette follows  |
//! | 6      | 1    | bits per pixel: 4, 8 (indices) or 24 (RGB)                   |
//! | 7      | 1    | reserved (0)                                                 |
//! | 8      | 8    | frame number                                                 |
//! | 16     | 2    | beam line (`0xffff`: no beam)                                |
//! | 18     | 2    | beam cycle (`0xffff`: no beam)                               |
//! | 20     | 2    | width                                                        |
//! | 22     | 2    | height                                                       |
//! | 24     | 2    | palette entries that follow (0 if bit 2 is clear)            |
//! | 26     | 2    | number of line runs that follow                              |
//! | 28     | 4    | reserved (0)                                                 |
//!
//! Then the palette (3 bytes R, G, B per entry), then each run: first line
//! (u16), line count (u16) and the packed lines. A line is `(width + 1) / 2`
//! bytes at 4 bpp (left pixel in the low nibble), `width` bytes at 8 bpp and
//! `3 * width` at 24 bpp. A key frame resets the picture (all lines and, for
//! indices, the palette follow); indices without a palette entry are black.
//! A message without runs and palette only updates beam and paused state.
//!
//! Audio message, [`AUDIO_HEADER_LEN`] bytes of header: magic `PTRA`,
//! version, channels (u8), reserved (u16), sample rate (u32), sample count
//! over all channels (u32); then the interleaved signed 16-bit samples.

use emu_core::frame::Rgb8;
use emu_core::AudioFormat;

use crate::{OwnedFrame, OwnedPixels};

pub const VIDEO_MAGIC: &[u8; 4] = b"PTRV";
pub const AUDIO_MAGIC: &[u8; 4] = b"PTRA";
pub const RAW_VERSION: u8 = 1;
pub const VIDEO_HEADER_LEN: usize = 32;
pub const AUDIO_HEADER_LEN: usize = 16;

pub const FLAG_KEY: u8 = 1;
pub const FLAG_PAUSED: u8 = 2;
pub const FLAG_PALETTE: u8 = 4;

/// A frame in wire layout.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Packed {
    width: u32,
    height: u32,
    bpp: u8,
    /// Empty for RGB frames.
    palette: Vec<Rgb8>,
    /// `height` lines of `line_len` bytes.
    lines: Vec<u8>,
    line_len: usize,
    running: bool,
}

impl Packed {
    fn new(f: &OwnedFrame) -> Self {
        let (w, h) = (f.width as usize, f.height as usize);
        let (bpp, palette, line_len, lines) = match &f.pixels {
            OwnedPixels::Indexed { pixels, palette } => {
                if palette.len() <= 16 && pixels.iter().all(|&p| p < 16) {
                    let line_len = w.div_ceil(2);
                    let mut lines = Vec::with_capacity(line_len * h);
                    for row in pixels.chunks_exact(w.max(1)).take(h) {
                        lines.extend(
                            row.chunks(2)
                                .map(|p| p[0] | p.get(1).map_or(0, |&q| q << 4)),
                        );
                    }
                    (4, palette.clone(), line_len, lines)
                } else {
                    (8, palette.clone(), w, pixels.clone())
                }
            }
            OwnedPixels::Rgb(pixels) => (
                24,
                Vec::new(),
                w * 3,
                pixels.iter().flat_map(|c| [c.r, c.g, c.b]).collect(),
            ),
        };
        Self {
            width: f.width,
            height: f.height,
            bpp,
            palette,
            lines,
            line_len,
            running: f.running,
        }
    }

    fn line(&self, y: usize) -> &[u8] {
        &self.lines[y * self.line_len..(y + 1) * self.line_len]
    }
}

/// The video side of one `/stream` connection: remembers what the viewer
/// already has and sends only the difference.
#[derive(Debug, Default)]
pub struct RawEncoder {
    prev: Option<Packed>,
}

impl RawEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The message that brings the viewer to `frame`; `None` if it already
    /// shows exactly that (running, same picture).
    pub fn encode(&mut self, frame: &OwnedFrame) -> Option<Vec<u8>> {
        let cur = Packed::new(frame);
        let prev = self.prev.take();
        let key = prev.as_ref().is_none_or(|p| {
            (p.width, p.height, p.bpp, p.line_len) != (cur.width, cur.height, cur.bpp, cur.line_len)
        });
        let palette =
            cur.bpp != 24 && (key || prev.as_ref().is_some_and(|p| p.palette != cur.palette));
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for y in 0..cur.height as usize {
            if key || prev.as_ref().is_some_and(|p| p.line(y) != cur.line(y)) {
                match runs.last_mut() {
                    Some((first, count)) if *first + *count == y => *count += 1,
                    _ => runs.push((y, 1)),
                }
            }
        }
        let state_changed = prev.as_ref().is_some_and(|p| p.running != cur.running);
        if runs.is_empty() && !palette && cur.running && !state_changed {
            self.prev = Some(cur);
            return None;
        }
        let changed: usize = runs.iter().map(|&(_, n)| n).sum();
        let mut m = Vec::with_capacity(
            VIDEO_HEADER_LEN + cur.palette.len() * 3 + runs.len() * 4 + changed * cur.line_len,
        );
        m.extend_from_slice(VIDEO_MAGIC);
        m.push(RAW_VERSION);
        let mut flags = 0;
        if key {
            flags |= FLAG_KEY;
        }
        if !cur.running {
            flags |= FLAG_PAUSED;
        }
        if palette {
            flags |= FLAG_PALETTE;
        }
        m.push(flags);
        m.push(cur.bpp);
        m.push(0);
        let (frame_no, line, cycle) = match frame.beam {
            Some(b) => (b.frame, b.line, b.cycle),
            None => (0, u16::MAX, u16::MAX),
        };
        m.extend_from_slice(&frame_no.to_le_bytes());
        m.extend_from_slice(&line.to_le_bytes());
        m.extend_from_slice(&cycle.to_le_bytes());
        let entries = if palette {
            cur.palette.len().min(256)
        } else {
            0
        };
        for v in [cur.width as usize, cur.height as usize, entries, runs.len()] {
            m.extend_from_slice(&(v.min(0xffff) as u16).to_le_bytes());
        }
        m.extend_from_slice(&[0; 4]);
        for c in &cur.palette[..entries] {
            m.extend_from_slice(&[c.r, c.g, c.b]);
        }
        for &(first, count) in &runs {
            m.extend_from_slice(&(first as u16).to_le_bytes());
            m.extend_from_slice(&(count as u16).to_le_bytes());
            m.extend_from_slice(&cur.lines[first * cur.line_len..(first + count) * cur.line_len]);
        }
        self.prev = Some(cur);
        Some(m)
    }
}

/// One audio message: `samples` interleaved as `format` says.
pub fn audio_message(samples: &[i16], format: AudioFormat) -> Vec<u8> {
    let mut m = Vec::with_capacity(AUDIO_HEADER_LEN + samples.len() * 2);
    m.extend_from_slice(AUDIO_MAGIC);
    m.push(RAW_VERSION);
    m.push(format.channels);
    m.extend_from_slice(&[0; 2]);
    m.extend_from_slice(&format.sample_rate.to_le_bytes());
    m.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for s in samples {
        m.extend_from_slice(&s.to_le_bytes());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
    use emu_core::BeamPos;

    fn frame(width: u32, height: u32, pixels: OwnedPixels, running: bool) -> OwnedFrame {
        OwnedFrame {
            width,
            height,
            pixels,
            signal: SignalInfo {
                pixel_clock_hz: 1,
                active_pixels: width,
                total_pixels: width,
                lines_per_field: height,
                standard: VideoStandard::Pal,
                output: OutputType::Composite,
                field: FieldParity::Progressive,
            },
            beam: Some(BeamPos {
                frame: 9,
                line: 3,
                cycle: 4,
                hpos: 0,
            }),
            running,
        }
    }

    fn indexed(pixels: &[u8], colours: usize) -> OwnedPixels {
        OwnedPixels::Indexed {
            pixels: pixels.to_vec(),
            palette: (0..colours as u8)
                .map(|i| Rgb8 { r: i, g: 0, b: 0 })
                .collect(),
        }
    }

    fn u16_at(m: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([m[o], m[o + 1]])
    }

    #[test]
    fn key_frame_packs_nibbles_low_first() {
        let mut enc = RawEncoder::new();
        let m = enc
            .encode(&frame(3, 2, indexed(&[1, 2, 3, 4, 5, 6], 16), true))
            .unwrap();
        assert_eq!(&m[..4], VIDEO_MAGIC);
        assert_eq!(m[5], FLAG_KEY | FLAG_PALETTE);
        assert_eq!(m[6], 4);
        assert_eq!(u64::from_le_bytes(m[8..16].try_into().unwrap()), 9);
        assert_eq!((u16_at(&m, 20), u16_at(&m, 22)), (3, 2));
        assert_eq!((u16_at(&m, 24), u16_at(&m, 26)), (16, 1));
        let runs = VIDEO_HEADER_LEN + 16 * 3;
        assert_eq!((u16_at(&m, runs), u16_at(&m, runs + 2)), (0, 2));
        assert_eq!(&m[runs + 4..], &[0x21, 0x03, 0x54, 0x06]);
    }

    #[test]
    fn deltas_carry_only_changed_lines() {
        let mut enc = RawEncoder::new();
        let mut px = vec![0u8; 4 * 5];
        enc.encode(&frame(4, 5, indexed(&px, 16), true)).unwrap();
        assert_eq!(
            enc.encode(&frame(4, 5, indexed(&px, 16), true)),
            None,
            "unchanged"
        );
        px[4] = 7; // line 1
        px[12] = 7; // line 3
        px[17] = 7; // line 4
        let m = enc.encode(&frame(4, 5, indexed(&px, 16), true)).unwrap();
        assert_eq!(m[5], 0, "delta without palette");
        assert_eq!(u16_at(&m, 26), 2, "runs 1 and 3–4");
        let o = VIDEO_HEADER_LEN;
        assert_eq!((u16_at(&m, o), u16_at(&m, o + 2)), (1, 1));
        assert_eq!(&m[o + 4..o + 6], &[0x07, 0x00]);
        let o = o + 6;
        assert_eq!((u16_at(&m, o), u16_at(&m, o + 2)), (3, 2));
        assert_eq!(m.len(), o + 4 + 4);
    }

    #[test]
    fn palette_pause_and_format_changes() {
        let mut enc = RawEncoder::new();
        let px = [0u8; 4];
        enc.encode(&frame(2, 2, indexed(&px, 16), true)).unwrap();
        let mut other = indexed(&px, 16);
        if let OwnedPixels::Indexed { palette, .. } = &mut other {
            palette[0].g = 99;
        }
        let m = enc.encode(&frame(2, 2, other, true)).unwrap();
        assert_eq!(
            (m[5], u16_at(&m, 24), u16_at(&m, 26)),
            (FLAG_PALETTE, 16, 0)
        );
        assert_eq!(m[VIDEO_HEADER_LEN + 1], 99);

        let m = enc.encode(&frame(2, 2, indexed(&px, 16), false)).unwrap();
        assert_eq!(
            m[5] & FLAG_PAUSED,
            FLAG_PAUSED,
            "paused: header even without changes"
        );
        assert!(enc.encode(&frame(2, 2, indexed(&px, 16), false)).is_some());

        let m = enc
            .encode(&frame(2, 2, indexed(&[0, 20, 0, 0], 32), true))
            .unwrap();
        assert_eq!(
            (m[5] & FLAG_KEY, m[6]),
            (FLAG_KEY, 8),
            "index ≥ 16: 8 bpp key frame"
        );
        let rgb = OwnedPixels::Rgb(vec![Rgb8 { r: 1, g: 2, b: 3 }; 4]);
        let m = enc.encode(&frame(2, 2, rgb, true)).unwrap();
        assert_eq!((m[5], m[6], u16_at(&m, 24)), (FLAG_KEY, 24, 0));
        assert_eq!(&m[VIDEO_HEADER_LEN + 4..VIDEO_HEADER_LEN + 7], &[1, 2, 3]);
    }

    #[test]
    fn audio_layout() {
        let m = audio_message(
            &[1, -2],
            AudioFormat {
                sample_rate: 48_000,
                channels: 1,
            },
        );
        assert_eq!(&m[..4], AUDIO_MAGIC);
        assert_eq!((m[4], m[5]), (RAW_VERSION, 1));
        assert_eq!(u32::from_le_bytes(m[8..12].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(m[12..16].try_into().unwrap()), 2);
        assert_eq!(&m[16..], &[1, 0, 0xfe, 0xff]);
    }
}
