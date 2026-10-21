//! Signal and CRT stage on the machines' real palettes and timings: PAL
//! colour mixing with VIC-II colour pairs of equal luma, NTSC artifact
//! colours (C64 hires stripes, and the Atari 800XL's — its hires pixels are
//! half a colour-carrier cycle, the classic artifacting), the VDC staying
//! clean, PET monochrome; plus every monitor profile against a reference
//! image (tolerance-based, CPU path).
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
        VideoStandard::Pal => (384, 272, 7_881_984, 63),
        VideoStandard::Ntsc => (384, 235, 8_181_816, 65),
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

/// The Atari 800XL (built-in stub OS, no ROMs) showing a mode F screen:
/// 50 lines of `$AA` (pixels on at even positions), then 50 of `$55`, white
/// luminance on black — as the machine itself renders it.
fn atari_card(model: &str) -> Pattern {
    use emu_core::machine::Machine;
    use emu_core::{DebugHooks, DeviceId, Size};
    let mut config = emu_core::registry::MachineConfig {
        model: Some(model.into()),
        ..Default::default()
    };
    config.options.insert("roms".into(), "stub".into());
    let mut m = emu_machines::registry()
        .create("atari800xl", &config)
        .unwrap();
    let mut dbg = DebugHooks::new();
    for _ in 0..2 {
        m.run_frame(&mut dbg);
    }
    let poke = |m: &mut Box<dyn Machine>, addr: u32, v: u8| {
        m.debug_target(DeviceId("cpu")).unwrap().poke(
            machine_atari8::CPU_SPACE,
            addr,
            Size::Byte,
            u32::from(v),
        );
    };
    // Display list at $3000: 24 blank lines, 100 mode F lines from $4000.
    let mut dl = vec![0x70, 0x70, 0x70, 0x4f, 0x00, 0x40];
    dl.extend([0x0f; 99]);
    dl.extend([0x41, 0x00, 0x30]);
    for (i, &b) in dl.iter().enumerate() {
        poke(&mut m, 0x3000 + i as u32, b);
    }
    for i in 0..4000u32 {
        poke(&mut m, 0x4000 + i, if i < 2000 { 0xaa } else { 0x55 });
    }
    for (addr, v) in [
        (0xd402, 0x00), // DLISTL
        (0xd403, 0x30), // DLISTH
        (0xd017, 0x0e), // COLPF1: white luminance
        (0xd018, 0x00), // COLPF2: black
        (0xd01a, 0x00), // COLBK
    ] {
        poke(&mut m, addr, v);
    }
    for _ in 0..2 {
        m.run_frame(&mut dbg);
    }
    let f = m.framebuffer();
    let PixelData::Indexed { pixels, palette } = f.pixels else {
        panic!("indexed frame");
    };
    Pattern {
        w: f.width,
        h: f.height,
        pixels: pixels.to_vec(),
        palette: palette.to_vec(),
        signal: f.signal,
    }
}

/// Mean colour over the middle of the playfield on `line`.
fn mean(card: &Pattern, out: &[Rgb8], line: u32) -> (f64, f64, f64) {
    let xs = 120..280u32;
    let n = xs.len() as f64;
    let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
    for x in xs {
        let c = card.at(out, x, line);
        r += f64::from(c.r);
        g += f64::from(c.g);
        b += f64::from(c.b);
    }
    (r / n, g / n, b / n)
}

/// U and V of an RGB colour (BT.601).
fn uv((r, g, b): (f64, f64, f64)) -> (f64, f64) {
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    (0.492 * (b - y), 0.877 * (r - y))
}

#[test]
fn atari_ntsc_hires_stripes_show_artifact_colours() {
    let card = atari_card("ntsc");
    // Mode F starts at scan line 32 = frame row 24.
    let (aa, x55) = (24 + 25, 24 + 75);
    // The raw frame is white on black only.
    for line in [aa, x55] {
        for x in 120..280 {
            let i = card.pixels[(line * card.w + x) as usize];
            assert!(i == 0x00 || i == 0x0e, "raw pixel {i:02X}");
        }
    }
    let tv = signal::process(&card.frame(), &Profile::preset("tv").unwrap()).unwrap();
    let (ua, va) = uv(mean(&card, &tv, aa));
    let (ub, vb) = uv(mean(&card, &tv, x55));
    let sat = |u: f64, v: f64| (u * u + v * v).sqrt();
    assert!(sat(ua, va) > 20.0, "$AA stripes coloured: {ua:.1},{va:.1}");
    assert!(sat(ub, vb) > 20.0, "$55 stripes coloured: {ub:.1},{vb:.1}");
    // Shifting the pattern by one pixel turns the phase by 180°: the two
    // artifact colours are complementary.
    let cos = (ua * ub + va * vb) / (sat(ua, va) * sat(ub, vb));
    assert!(cos < -0.8, "complementary hues: cos {cos:.2}");
    // $AA lights the first half of every colour clock: the phase of hue 1,
    // the colour burst (−U).
    let (u1, v1) = (-1.0, 0.0);
    let cos1 = (ua * u1 + va * v1) / sat(ua, va);
    assert!(cos1 > 0.8, "$AA has hue 1's phase: cos {cos1:.2}");
    // S-Video (luma/chroma cable) shows the true picture: grey stripes.
    let sv = signal::process(&card.frame(), &Profile::preset("1702").unwrap()).unwrap();
    let (u, v) = uv(mean(&card, &sv, aa));
    assert!(sat(u, v) < 4.0, "S-Video stays grey: {u:.1},{v:.1}");
}

#[test]
fn atari_pal_has_no_stable_artifact_colours() {
    // PAL machines' hires pixels are not locked to the colour carrier and
    // the delay line averages what cross-colour remains.
    let ntsc = atari_card("ntsc");
    let pal = atari_card("pal");
    let tv = Profile::preset("tv").unwrap();
    let n = signal::process(&ntsc.frame(), &tv).unwrap();
    let p = signal::process(&pal.frame(), &tv).unwrap();
    let sat = |c: (f64, f64, f64)| {
        let (u, v) = uv(c);
        (u * u + v * v).sqrt()
    };
    let (sn, sp) = (sat(mean(&ntsc, &n, 49)), sat(mean(&pal, &p, 49)));
    assert!(sp < sn / 2.0, "PAL {sp:.1} vs NTSC {sn:.1}");
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
    let atari_ntsc = atari_card("ntsc");
    let atari_pal = atari_card("pal");
    let cases: [(&str, &Pattern, &str); 12] = [
        ("raw", &c64_pal, "c64-pal-raw"),
        ("1702", &c64_pal, "c64-pal-1702"),
        ("1084s", &c64_pal, "c64-pal-1084s"),
        ("pal-tv", &c64_pal, "c64-pal-pal-tv"),
        ("ntsc-tv", &c64_ntsc, "c64-ntsc-ntsc-tv"),
        ("1702", &c64_ntsc, "c64-ntsc-1702"),
        ("1084s-rgb", &vdc, "vdc-1084s-rgb"),
        ("pet-white", &pet, "pet-white"),
        ("pet-green", &pet, "pet-green"),
        ("tv", &atari_ntsc, "atari-ntsc-tv"),
        ("1702", &atari_ntsc, "atari-ntsc-1702"),
        ("tv", &atari_pal, "atari-pal-tv"),
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
