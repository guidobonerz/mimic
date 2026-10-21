use super::*;
use emu_core::frame::FieldParity;

const C64_PAL_CLOCK: u32 = 7_881_984;
/// Two pixels per NTSC subcarrier cycle, like the Atari's hires pixels.
const NTSC_2FSC: u32 = 7_159_090;

fn rgb(r: u8, g: u8, b: u8) -> Rgb8 {
    Rgb8 { r, g, b }
}

/// 75 % colour bars: white, yellow, cyan, green, magenta, red, blue, black.
const BARS: [Rgb8; 8] = [
    Rgb8 {
        r: 191,
        g: 191,
        b: 191,
    },
    Rgb8 {
        r: 191,
        g: 191,
        b: 0,
    },
    Rgb8 {
        r: 0,
        g: 191,
        b: 191,
    },
    Rgb8 { r: 0, g: 191, b: 0 },
    Rgb8 {
        r: 191,
        g: 0,
        b: 191,
    },
    Rgb8 { r: 191, g: 0, b: 0 },
    Rgb8 { r: 0, g: 0, b: 191 },
    Rgb8 { r: 0, g: 0, b: 0 },
];

struct Field {
    w: u32,
    h: u32,
    pixels: Vec<u8>,
    palette: Vec<Rgb8>,
    signal: SignalInfo,
}

impl Field {
    fn new(w: u32, h: u32, palette: &[Rgb8], standard: VideoStandard, clock: u32) -> Self {
        Self {
            w,
            h,
            pixels: vec![0; (w * h) as usize],
            palette: palette.to_vec(),
            signal: SignalInfo {
                pixel_clock_hz: clock,
                active_pixels: w,
                total_pixels: 504,
                lines_per_field: 312,
                standard,
                output: OutputType::SVideo,
                field: FieldParity::Progressive,
            },
        }
    }

    fn fill(mut self, f: impl Fn(u32, u32) -> u8) -> Self {
        for y in 0..self.h {
            for x in 0..self.w {
                self.pixels[(y * self.w + x) as usize] = f(x, y);
            }
        }
        self
    }

    fn output(mut self, output: OutputType) -> Self {
        self.signal.output = output;
        self
    }

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
}

fn profile(signal: Signal) -> Profile {
    Profile {
        signal,
        ..Profile::default()
    }
}

fn at(out: &[Rgb8], w: u32, x: u32, y: u32) -> Rgb8 {
    out[(y * w + x) as usize]
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

/// Colourfulness: spread between the strongest and weakest channel.
fn chroma(c: Rgb8) -> i32 {
    let v = [i32::from(c.r), i32::from(c.g), i32::from(c.b)];
    v.iter().max().unwrap() - v.iter().min().unwrap()
}

fn bars(standard: VideoStandard, clock: u32) -> Field {
    Field::new(320, 8, &BARS, standard, clock).fill(|x, _| (x / 40) as u8)
}

#[test]
fn effective_signal_follows_what_the_machine_delivers() {
    let svideo = profile(Signal::SVideo);
    assert_eq!(effective(&svideo, OutputType::SVideo), Signal::SVideo);
    assert_eq!(
        effective(&svideo, OutputType::Composite),
        Signal::Composite,
        "VIC-20/TED have no separate chroma"
    );
    assert_eq!(
        effective(&profile(Signal::Composite), OutputType::Rgb),
        Signal::Rgb,
        "C128 VDC stays clean"
    );
    assert_eq!(
        effective(&profile(Signal::Monochrome), OutputType::Rgb),
        Signal::Monochrome
    );
    assert_eq!(
        effective(&svideo, OutputType::Monochrome),
        Signal::Monochrome,
        "PET"
    );
    assert_eq!(
        effective(&profile(Signal::Rgb), OutputType::SVideo),
        Signal::Rgb
    );
}

#[test]
fn rgb_passes_unchanged() {
    let f = bars(VideoStandard::Pal, C64_PAL_CLOCK);
    assert!(process(&f.frame(), &profile(Signal::Rgb)).is_none());
    let vdc = bars(VideoStandard::Pal, 16_000_000).output(OutputType::Rgb);
    assert!(process(&vdc.frame(), &Profile::preset("pal-tv").unwrap()).is_none());
}

#[test]
fn colour_bars_survive_every_path() {
    for (standard, clock) in [
        (VideoStandard::Pal, C64_PAL_CLOCK),
        (VideoStandard::Ntsc, 8_181_816),
    ] {
        let f = bars(standard, clock);
        for signal in [Signal::SVideo, Signal::Composite] {
            let out = process(&f.frame(), &profile(signal)).unwrap();
            for (i, &want) in BARS.iter().enumerate() {
                for y in 1..8 {
                    let got = at(&out, 320, i as u32 * 40 + 20, y);
                    assert!(
                        dist(got, want) <= 8,
                        "{standard:?} {signal:?} bar {i} line {y}: {got:?} want {want:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn chroma_bandwidth_blurs_colour_edges_but_not_luma() {
    // Black | white: a luma edge stays sharp in S-Video.
    let f = Field::new(
        64,
        2,
        &[rgb(0, 0, 0), rgb(255, 255, 255)],
        VideoStandard::Pal,
        C64_PAL_CLOCK,
    )
    .fill(|x, _| u8::from(x >= 32));
    let out = process(&f.frame(), &profile(Signal::SVideo)).unwrap();
    assert!(dist(at(&out, 64, 31, 1), rgb(0, 0, 0)) <= 2);
    assert!(dist(at(&out, 64, 32, 1), rgb(255, 255, 255)) <= 2);
    // Red | blue: the colour transition spreads over several pixels.
    let f = Field::new(
        64,
        2,
        &[rgb(191, 0, 0), rgb(0, 0, 191)],
        VideoStandard::Pal,
        C64_PAL_CLOCK,
    )
    .fill(|x, _| u8::from(x >= 32));
    let out = process(&f.frame(), &profile(Signal::SVideo)).unwrap();
    let edge = at(&out, 64, 32, 1);
    assert!(dist(edge, rgb(0, 0, 191)) > 30, "{edge:?}");
}

#[test]
fn pal_delay_line_mixes_colours_of_equal_luma() {
    // Two colours with the same luma on alternating lines (PAL colour
    // mixing): the decoder shows their average on every line.
    let a = rgb(200, 60, 120);
    let yuv = to_yuv(a);
    let b = to_rgb(Yuv {
        y: yuv.y,
        u: -yuv.u,
        v: yuv.v * 0.2,
    });
    let f =
        Field::new(64, 10, &[a, b], VideoStandard::Pal, C64_PAL_CLOCK).fill(|_, y| (y % 2) as u8);
    let mix = to_rgb(Yuv {
        y: yuv.y,
        u: 0.0,
        v: (yuv.v + to_yuv(b).v) / 2.0,
    });
    for signal in [Signal::SVideo, Signal::Composite] {
        let out = process(&f.frame(), &profile(signal)).unwrap();
        for y in 1..10 {
            let got = at(&out, 64, 32, y);
            assert!(
                dist(got, mix) <= 6,
                "{signal:?} line {y}: {got:?} want {mix:?}"
            );
        }
        let mut p = profile(signal);
        p.delay_line = false;
        let out = process(&f.frame(), &p).unwrap();
        assert!(dist(at(&out, 64, 32, 4), a) <= 6);
        assert!(dist(at(&out, 64, 32, 5), b) <= 6);
    }
}

#[test]
fn ntsc_has_no_delay_line() {
    let a = rgb(200, 60, 120);
    let b = rgb(60, 140, 90);
    let f = Field::new(64, 6, &[a, b], VideoStandard::Ntsc, 8_181_816).fill(|_, y| (y % 2) as u8);
    let out = process(&f.frame(), &profile(Signal::SVideo)).unwrap();
    assert!(dist(at(&out, 64, 32, 2), a) <= 6);
    assert!(dist(at(&out, 64, 32, 3), b) <= 6);
}

#[test]
fn ntsc_artifact_colours_from_fine_luma_stripes() {
    // Alternating black/white pixels at half the subcarrier period: a
    // composite decoder reads them as colour, S-Video does not.
    let pal = [rgb(0, 0, 0), rgb(255, 255, 255)];
    let even = Field::new(80, 4, &pal, VideoStandard::Ntsc, NTSC_2FSC).fill(|x, _| (x % 2) as u8);
    let odd =
        Field::new(80, 4, &pal, VideoStandard::Ntsc, NTSC_2FSC).fill(|x, _| 1 - (x % 2) as u8);

    let s = process(&even.frame(), &profile(Signal::SVideo)).unwrap();
    assert!(chroma(at(&s, 80, 40, 2)) <= 4, "{:?}", at(&s, 80, 40, 2));

    let c0 = at(
        &process(&even.frame(), &profile(Signal::Composite)).unwrap(),
        80,
        40,
        2,
    );
    let c1 = at(
        &process(&odd.frame(), &profile(Signal::Composite)).unwrap(),
        80,
        40,
        2,
    );
    assert!(chroma(c0) > 60, "artifact colour {c0:?}");
    assert!(chroma(c1) > 60, "artifact colour {c1:?}");
    // Shifting the pattern by one pixel turns the phase by 180°: the
    // complementary colour.
    let (u0, u1) = (to_yuv(c0), to_yuv(c1));
    assert!(u0.u * u1.u + u0.v * u1.v < 0.0, "{c0:?} vs {c1:?}");
}

#[test]
fn composite_cross_luma_marks_colour_edges() {
    // A saturated colour next to grey of the same luma: S-Video keeps the
    // grey flat, composite leaves residual dots near the edge.
    let red = rgb(191, 0, 0);
    let grey_y = to_yuv(red).y;
    let grey = to_rgb(Yuv {
        y: grey_y,
        ..Yuv::default()
    });
    let f = Field::new(64, 2, &[red, grey], VideoStandard::Pal, C64_PAL_CLOCK)
        .fill(|x, _| u8::from(x >= 32));
    let out = process(&f.frame(), &profile(Signal::Composite)).unwrap();
    let far = at(&out, 64, 60, 1);
    assert!(dist(far, grey) <= 4, "{far:?} vs {grey:?}");
    let sv = process(&f.frame(), &profile(Signal::SVideo)).unwrap();
    let luma = |c: Rgb8| to_yuv(c).y;
    let worst = (26..38)
        .map(|x| (luma(at(&out, 64, x, 1)) - luma(at(&sv, 64, x, 1))).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst > 0.02,
        "composite luma near the edge differs: {worst}"
    );
}

#[test]
fn monochrome_shows_luma_only() {
    let f = Field::new(
        4,
        1,
        &[rgb(255, 0, 0), rgb(0x41, 0xff, 0)],
        VideoStandard::Pal,
        8_000_000,
    )
    .fill(|x, _| (x % 2) as u8);
    let out = process(&f.frame(), &profile(Signal::Monochrome)).unwrap();
    assert_eq!(out[0], rgb(76, 76, 76));
    assert_eq!(chroma(out[1]), 0);
}

#[test]
fn luma_chroma_overrides_the_palette() {
    let f = Field::new(64, 2, &[rgb(0, 0, 0)], VideoStandard::Pal, C64_PAL_CLOCK);
    // Pure +U at amplitude 0.2 on mid grey: a blue-ish tone.
    let lc = vec![
        LumaChroma {
            luma: 32768,
            chroma_phase: 0,
            chroma_amplitude: 13107,
        };
        128
    ];
    let mut frame = f.frame();
    frame.luma_chroma = Some(&lc);
    let out = process(&frame, &profile(Signal::SVideo)).unwrap();
    let want = to_rgb(Yuv {
        y: 0.5,
        u: 0.2,
        v: 0.0,
    });
    assert!(
        dist(at(&out, 64, 32, 1), want) <= 3,
        "{:?}",
        at(&out, 64, 32, 1)
    );
}

#[test]
fn saturation_zero_is_greyscale() {
    let f = bars(VideoStandard::Pal, C64_PAL_CLOCK);
    let mut p = profile(Signal::SVideo);
    p.saturation = 0.0;
    let out = process(&f.frame(), &p).unwrap();
    assert!(out.iter().all(|&c| chroma(c) <= 1));
}

#[test]
fn deterministic() {
    let f = bars(VideoStandard::Ntsc, 8_181_816);
    let p = Profile::preset("ntsc-tv").unwrap();
    assert_eq!(process(&f.frame(), &p), process(&f.frame(), &p));
}

#[test]
fn rgba_matches_the_frame_path() {
    let f = bars(VideoStandard::Pal, C64_PAL_CLOCK);
    let p = Profile::preset("pal-tv").unwrap();
    let rgba: Vec<u8> = (0..f.h)
        .flat_map(|y| (0..f.w).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            let c = f.frame().rgb_at(x, y).unwrap();
            [c.r, c.g, c.b, 255]
        })
        .collect();
    let want: Vec<u8> = process(&f.frame(), &p)
        .unwrap()
        .iter()
        .flat_map(|c| [c.r, c.g, c.b, 255])
        .collect();
    assert_eq!(process_rgba(f.w, f.h, &rgba, f.signal, &p), Some(want));
    assert_eq!(
        process_rgba(f.w, f.h, &rgba, f.signal, &Profile::default()),
        None
    );
}
