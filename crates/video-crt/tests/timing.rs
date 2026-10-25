//! CPU-path timing (development aid, not a gate):
//! `cargo test -p video-crt --release --test timing -- --ignored --nocapture`
//! renders a C64-sized PAL frame (404 × 284) at scale 2 through each preset.

use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use video_crt::{Crt, Profile};

#[test]
#[ignore]
fn c64_frame_at_scale_2() {
    let (w, h) = (404u32, 284u32);
    let palette: Vec<Rgb8> = (0..16u8)
        .map(|i| Rgb8 {
            r: i * 16,
            g: 255 - i * 16,
            b: i * 8,
        })
        .collect();
    let pixels: Vec<u8> = (0..w * h)
        .map(|i| ((i % w) / 7 + i / w / 5) as u8 % 16)
        .collect();
    let frame = FrameRef {
        width: w,
        height: h,
        pixels: PixelData::Indexed {
            pixels: &pixels,
            palette: &palette,
        },
        luma_chroma: None,
        signal: SignalInfo {
            pixel_clock_hz: 7_881_984,
            active_pixels: w,
            total_pixels: 504,
            lines_per_field: 312,
            standard: VideoStandard::Pal,
            output: OutputType::SVideo,
            field: FieldParity::Progressive,
        },
    };
    {
        let profile = Profile::preset("1702").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(video_crt::signal::process(&frame, &profile));
        }
        eprintln!(
            "signal stage of 1702 (single thread): {:.2} ms",
            start.elapsed().as_secs_f64() * 50.0
        );
    }
    for name in ["raw", "lite", "1702", "pal-tv", "gt65"] {
        let profile = Profile::preset(name).unwrap();
        let mut crt = Crt::new();
        crt.render(&frame, &profile, 2);
        let n = 20;
        let start = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(crt.render(&frame, &profile, 2));
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
        let img = crt.render(&frame, &profile, 2);
        eprintln!(
            "{name}: {ms:.2} ms per frame ({}x{})",
            img.width, img.height
        );
    }
}
