//! Signal and CRT stage on the machines' real palettes and timings: PAL
//! colour mixing with VIC-II colour pairs of equal luma, NTSC artifact
//! colours, the VDC staying clean, PET monochrome; plus every monitor
//! profile against a reference image (tolerance-based, CPU path).
//!
//! Regenerate the references after an intended change with
//! `EMU_UPDATE_REFERENCE=1 cargo test -p emu-tests --test crt_profiles`
//! and explain the change in the commit.

use std::path::PathBuf;

use emu_core::frame::{
    FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard,
};
use video_crt::{signal, Profile};

/// VIC-II colour pairs with the same luma (6569R3 levels): blue/brown,
/// purple/orange, green/light red, red/dark grey.
const MIX_PAIRS: [(u8, u8); 4] = [(6, 9), (4, 8), (5, 10), (2, 11)];

struct Pattern {
    w: u32,
    h: u32,
    pixels: Vec<u8>,
    palette: Vec<Rgb8>,
    signal: SignalInfo,
}

impl Pattern {
    fn frame(&self) -> FrameRef<'_> {
        FrameRef {
            width: self.w,
            height: self.h,
            pixels: PixelData::Indexed {
                pixels: &self.pixels,
                palette: &self.palette,
            },
            luma_chroma: None,
            signal: self.signal,
        }
    }

    fn at(&self, out: &[Rgb8], x: u32, y: u32) -> Rgb8 {
        out[(y * self.w + x) as usize]
    }
}

/// C64 test card: 16 colour bars, PAL mixing pairs on alternating lines,
/// hires stripes (1 and 2 pixels) in white on black.
fn c64_card(standard: VideoStandard) -> Pattern {
    let (w, h, clock, cycles) = match standard {
        VideoStandard::Pal => (404, 284, 7_881_984, 63),
        VideoStandard::Ntsc => (420, 235, 8_181_816, 65),
    };
    let mut pixels = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let band = y * 4 / h;
            pixels[(y * w + x) as usize] = match band {
                0 => (x * 16 / w) as u8,
                1 => {
                    let (a, b) = MIX_PAIRS[(x * 4 / w) as usize];
                    if y % 2 == 0 {
                        a
                    } else {
                        b
                    }
                }
                2 => (x % 2) as u8,
                _ => ((x / 2) % 2) as u8,
            };
        }
    }
    Pattern {
        w,
        h,
        pixels,
        palette: chips_cbm::vic2::palette::palette().to_vec(),
        signal: SignalInfo {
            pixel_clock_hz: clock,
            active_pixels: w,
            total_pixels: cycles * 8,
            lines_per_field: if standard == VideoStandard::Pal {
                312
            } else {
                263
            },
            standard,
            output: OutputType::SVideo,
            field: FieldParity::Progressive,
        },
    }
}

/// VDC test card: the 16 RGBI colours as bars, 80-column text-like
/// stripes below.
fn vdc_card() -> Pattern {
    let (w, h) = (640u32, 200u32);
    let pixels = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if y < 100 {
                (x * 16 / w) as u8
            } else {
                (x % 2) as u8 * 15
            }
        })
        .collect();
    Pattern {
        w,
        h,
        pixels,
        palette: chips_cbm::vdc::palette().to_vec(),
        signal: SignalInfo {
            pixel_clock_hz: 16_000_000,
            active_pixels: w,
            total_pixels: 1024,
            lines_per_field: 312,
            standard: VideoStandard::Pal,
            output: OutputType::Rgb,
            field: FieldParity::Progressive,
        },
    }
}

/// PET 4032-like card: on/off text pattern on the monochrome output.
fn pet_card() -> Pattern {
    let (w, h) = (320u32, 200u32);
    let pixels = (0..w * h)
        .map(|i| u8::from((i % w / 3 + i / w / 2) % 3 == 0))
        .collect();
    Pattern {
        w,
        h,
        pixels,
        palette: vec![
            Rgb8 { r: 0, g: 0, b: 0 },
            Rgb8 {
                r: 255,
                g: 255,
                b: 255,
            },
        ],
        signal: SignalInfo {
            pixel_clock_hz: 8_000_000,
            active_pixels: w,
            total_pixels: 512,
            lines_per_field: 312,
            standard: VideoStandard::Pal,
            output: OutputType::Monochrome,
            field: FieldParity::Progressive,
        },
    }
}

fn dist(a: Rgb8, b: Rgb8) -> i32 {
    [
        (i32::from(a.r) - i32::from(b.r)).abs(),
        (i32::from(a.g) - i32::from(b.g)).abs(),
        (i32::from(a.b) - i32::from(b.b)).abs(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

fn chroma(c: Rgb8) -> i32 {
    let v = [i32::from(c.r), i32::from(c.g), i32::from(c.b)];
    v.iter().max().unwrap() - v.iter().min().unwrap()
}

#[test]
fn c64_pal_colour_mixing_through_the_1702() {
    let card = c64_card(VideoStandard::Pal);
    let out = signal::process(&card.frame(), &Profile::preset("1702").unwrap()).unwrap();
    let pal = &card.palette;
    let band = card.h / 4;
    for (i, &(a, b)) in MIX_PAIRS.iter().enumerate() {
        let x = card.w * (2 * i as u32 + 1) / 8;
        let even = card.at(&out, x, band + band / 2);
        let odd = card.at(&out, x, band + band / 2 + 1);
        assert!(dist(even, odd) <= 3, "pair {a}/{b}: lines {even:?} {odd:?}");
        assert!(
            dist(even, pal[usize::from(a)]) > 6 && dist(even, pal[usize::from(b)]) > 6,
            "pair {a}/{b} shows a third colour: {even:?}"
        );
    }
    // Plain colour bars keep their colour.
    for c in 0..16u32 {
        let x = card.w * (2 * c + 1) / 32;
        let got = card.at(&out, x, band / 2);
        assert!(dist(got, pal[c as usize]) <= 4, "colour {c}: {got:?}");
    }
    // Without the delay line the pairs stay apart.
    let mut p = Profile::preset("1702").unwrap();
    p.delay_line = false;
    let out = signal::process(&card.frame(), &p).unwrap();
    let (a, _) = MIX_PAIRS[0];
    let got = card.at(&out, card.w / 8, band + band / 2);
    assert!(dist(got, pal[usize::from(a)]) <= 4, "{got:?}");
}

#[test]
fn c64_ntsc_composite_colours_hires_stripes() {
    let card = c64_card(VideoStandard::Ntsc);
    let band = card.h / 4;
    let y = 2 * band + band / 2;
    let composite = signal::process(&card.frame(), &Profile::preset("ntsc-tv").unwrap()).unwrap();
    let svideo = signal::process(&card.frame(), &Profile::preset("1702").unwrap()).unwrap();
    let x = card.w / 2;
    assert!(chroma(card.at(&svideo, x, y)) <= 6, "S-Video stays grey");
    let tinted = (x - 20..x + 20)
        .map(|x| chroma(card.at(&composite, x, y)))
        .max()
        .unwrap();
    assert!(tinted > 25, "composite shows artifact colour: {tinted}");
}

#[test]
fn vdc_stays_clean_on_colour_profiles() {
    let card = vdc_card();
    for name in ["1702", "pal-tv", "ntsc-tv", "1084s-rgb"] {
        assert!(
            signal::process(&card.frame(), &Profile::preset(name).unwrap()).is_none(),
            "{name}"
        );
    }
}

#[test]
fn pet_monitors_tint_luma() {
    let card = pet_card();
    let green = video_crt::render(&card.frame(), &Profile::preset("pet-green").unwrap(), 1);
    let white = video_crt::render(&card.frame(), &Profile::preset("pet-white").unwrap(), 1);
    let lit = (0..green.width)
        .find(|&x| green.pixel(x, 0)[1] > 128)
        .expect("a lit pixel");
    let [r, g, b] = green.pixel(lit, 0);
    assert!(g > 200 && r < 120 && b < 40, "green phosphor {r} {g} {b}");
    let [r, g, b] = white.pixel(lit, 0);
    assert!(r > 180 && g > 180 && b > 200, "white phosphor {r} {g} {b}");
}

fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("reference/crt")
}

fn read_png(path: &PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgb).then_some((info.width, info.height, buf))
}

fn write_png(path: &PathBuf, img: &video_crt::Image) {
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

#[test]
fn profiles_match_reference_images() {
    let update = std::env::var_os("EMU_UPDATE_REFERENCE").is_some();
    let c64_pal = c64_card(VideoStandard::Pal);
    let c64_ntsc = c64_card(VideoStandard::Ntsc);
    let vdc = vdc_card();
    let pet = pet_card();
    let cases: [(&str, &Pattern, &str); 9] = [
        ("raw", &c64_pal, "c64-pal-raw"),
        ("1702", &c64_pal, "c64-pal-1702"),
        ("1084s", &c64_pal, "c64-pal-1084s"),
        ("pal-tv", &c64_pal, "c64-pal-pal-tv"),
        ("ntsc-tv", &c64_ntsc, "c64-ntsc-ntsc-tv"),
        ("1702", &c64_ntsc, "c64-ntsc-1702"),
        ("1084s-rgb", &vdc, "vdc-1084s-rgb"),
        ("pet-white", &pet, "pet-white"),
        ("pet-green", &pet, "pet-green"),
    ];
    let mut failures = Vec::new();
    for (profile, card, name) in cases {
        let img = video_crt::render(&card.frame(), &Profile::preset(profile).unwrap(), 2);
        let path = reference_dir().join(format!("{name}.png"));
        if update {
            write_png(&path, &img);
            continue;
        }
        let Some((w, h, want)) = read_png(&path) else {
            failures.push(format!(
                "{name}: missing {} (EMU_UPDATE_REFERENCE=1)",
                path.display()
            ));
            continue;
        };
        if (w, h) != (img.width, img.height) {
            failures.push(format!(
                "{name}: size {}x{} vs {w}x{h}",
                img.width, img.height
            ));
            continue;
        }
        let diffs: Vec<u32> = img
            .rgb
            .iter()
            .zip(&want)
            .map(|(a, b)| u32::from(a.abs_diff(*b)))
            .collect();
        let max = diffs.iter().copied().max().unwrap_or(0);
        let mean = diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64;
        // Float rounding may differ between targets; structure may not.
        if max > 3 || mean > 0.05 {
            failures.push(format!("{name}: max diff {max}, mean {mean:.4}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
