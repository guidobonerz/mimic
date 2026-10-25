//! The CRT test pattern and helpers shared by the reference and GPU tests.

#![allow(dead_code)]

use std::path::PathBuf;

use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};

pub const W: u32 = 192;
pub const H: u32 = 144;

const fn rgb(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 { r, g, b }
}

/// Black, white, the six colour-bar hues at 75 %, then a grey ramp.
pub const PALETTE: [Rgb8; 16] = [
    rgb(0, 0, 0),
    rgb(255, 255, 255),
    rgb(191, 191, 0),
    rgb(0, 191, 191),
    rgb(0, 191, 0),
    rgb(191, 0, 191),
    rgb(191, 0, 0),
    rgb(0, 0, 191),
    rgb(32, 32, 32),
    rgb(64, 64, 64),
    rgb(96, 96, 96),
    rgb(128, 128, 128),
    rgb(160, 160, 160),
    rgb(192, 192, 192),
    rgb(224, 224, 224),
    rgb(191, 191, 191),
];

/// Deterministic test card (`W` × `H`, C64 PAL timing on S-Video):
/// colour bars, a grey ramp, then fine detail: 1- and 2-pixel vertical
/// lines, alternating horizontal lines, a checkerboard, and a small white
/// box on black (bloom, persistence). `shift` moves the box right.
pub fn pattern(shift: u32) -> Vec<u8> {
    let mut px = vec![0u8; (W * H) as usize];
    for y in 0..H {
        for x in 0..W {
            let v = match y {
                0..=35 => [15, 2, 3, 4, 5, 6, 7, 0][(x * 8 / W) as usize],
                36..=53 => [0, 8, 9, 10, 11, 12, 13, 14, 1][(x * 9 / W) as usize],
                54..=89 => match x * 4 / W {
                    0 => (x % 2) as u8,
                    1 => ((x / 2) % 2) as u8,
                    2 => (y % 2) as u8,
                    _ => ((x + y) % 2) as u8,
                },
                _ => {
                    let bx = 40 + shift;
                    u8::from((bx..bx + 24).contains(&x) && (104..128).contains(&y))
                }
            };
            px[(y * W + x) as usize] = v;
        }
    }
    px
}

pub fn frame(pixels: &[u8]) -> FrameRef<'_> {
    FrameRef {
        width: W,
        height: H,
        pixels: PixelData::Indexed {
            pixels,
            palette: &PALETTE,
        },
        luma_chroma: None,
        signal: SignalInfo {
            pixel_clock_hz: 7_881_984,
            active_pixels: W,
            total_pixels: 504,
            lines_per_field: 312,
            standard: VideoStandard::Pal,
            output: OutputType::SVideo,
            field: FieldParity::Progressive,
        },
    }
}

pub fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/reference/crt")
}

pub fn read_png(path: &PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgb).then_some((info.width, info.height, buf))
}

pub fn write_png(path: &PathBuf, img: &video_crt::Image) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    enc.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);
    enc.write_header()
        .unwrap()
        .write_image_data(&img.rgb)
        .unwrap();
}

/// (largest byte difference, mean byte difference, fraction of bytes off
/// by more than `slack`).
pub fn diff(a: &[u8], b: &[u8], slack: u8) -> (u8, f64, f64) {
    assert_eq!(a.len(), b.len());
    let mut max = 0;
    let mut total = 0u64;
    let mut off = 0usize;
    for (&x, &y) in a.iter().zip(b) {
        let d = x.abs_diff(y);
        max = max.max(d);
        total += u64::from(d);
        off += usize::from(d > slack);
    }
    let n = a.len().max(1) as f64;
    (max, total as f64 / n, off as f64 / n)
}
