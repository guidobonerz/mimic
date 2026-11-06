//! Foundry's music and sound effects (step 8), generated at build time: a
//! waltz in A minor of this project's own (16 bars, 3/4) for four Calliope
//! channels — brass melody, bass, reed "pah-pah", noise drums — and two
//! effects for the sprite scene (a bell "ping" sample on channel 7, a
//! noise thud on channel 6).
//!
//! Two assembler includes come out: `music.inc` (constants, included at
//! the top) and `music_data.inc` (`mus_blob`, copied to `MUS_SEG:0000`,
//! and `mus_setup`, register/value pairs the ROM writes at start).
//!
//! The block in chip RAM: the player's variables (row timer, row pointer)
//! at 0000, the period of every MIDI note at 0010 (4 MHz / (32 × f): the
//! waves are one 32-sample cycle), the song at 0200 (4 bytes a row:
//! melody, bass, reed, drums; 0 = nothing new, $FF = release, else a MIDI
//! note, for the drums the noise rate), the waves at 0800 and the ping at
//! 0900.

use std::f64::consts::PI;
use std::fmt::Write;

/// Chip RAM segment of the block (after the 3D scene's buffers).
pub const SEG: u32 = 0x0019;
const VARS: usize = 0x0000;
const PERIODS: usize = 0x0010;
const SONG: usize = 0x0200;
const BRASS: usize = 0x0800;
const BASS: usize = 0x0820;
const REED: usize = 0x0840;
const PING: usize = 0x0900;
const PING_LEN: usize = 2048;
const END: usize = PING + PING_LEN;
/// Fields a row lasts: a bar of 6 rows is 54 fields (0.9 s).
const SPEED: u16 = 9;
const ROWS_PER_BAR: usize = 6;

/// MIDI note number of "A4", "G#5", "C3"; "-" (hold) is 0, "." ($FF)
/// releases.
fn note(s: &str) -> u8 {
    match s {
        "-" => 0,
        "." => 0xff,
        _ => {
            let b = s.as_bytes();
            let base = match b[0] {
                b'C' => 0,
                b'D' => 2,
                b'E' => 4,
                b'F' => 5,
                b'G' => 7,
                b'A' => 9,
                b'B' => 11,
                _ => panic!("note {s}"),
            };
            let (sharp, oct) = if b[1] == b'#' {
                (1, &s[2..])
            } else {
                (0, &s[1..])
            };
            let oct: i32 = oct.parse().expect("octave");
            (12 * (oct + 1) + base + sharp) as u8
        }
    }
}

/// The song: 16 bars of melody, chords as (root, third, fifth).
fn song() -> Vec<[u8; 4]> {
    let melody = [
        "A4 - - - C5 B4",
        "A4 - E4 - A4 C5",
        "D5 - - - F5 E5",
        "D5 - A4 - D5 F5",
        "E5 - - - D5 C5",
        "B4 - G#4 - B4 D5",
        "C5 - B4 - A4 -",
        "E5 - - - . -",
        "F5 - - - E5 D5",
        "C5 - A4 - C5 F5",
        "E5 - - - D5 C5",
        "G4 - E5 - D5 C5",
        "D5 - F5 - A5 -",
        "G#5 - E5 - D5 B4",
        "C5 - A4 - E4 -",
        "A4 - - - . -",
    ];
    let chords = [
        ("A2", "C4", "E4"),
        ("A2", "C4", "E4"),
        ("D2", "F4", "A4"),
        ("D2", "F4", "A4"),
        ("E2", "G#4", "B4"),
        ("E2", "G#4", "B4"),
        ("A2", "C4", "E4"),
        ("E2", "G#4", "B4"),
        ("F2", "A4", "C5"),
        ("F2", "A4", "C5"),
        ("C3", "E4", "G4"),
        ("C3", "E4", "G4"),
        ("D2", "F4", "A4"),
        ("E2", "G#4", "B4"),
        ("A2", "C4", "E4"),
        ("A2", "C4", "E4"),
    ];
    let mut rows = Vec::new();
    for (bar, (m, (root, third, fifth))) in melody.iter().zip(chords).enumerate() {
        let m: Vec<u8> = m.split_whitespace().map(note).collect();
        assert_eq!(m.len(), ROWS_PER_BAR, "bar {bar}");
        let bass = [note(root), 0, 0xff, 0, 0, 0];
        let reed = [0, 0, note(third), 0xff, note(fifth), 0xff];
        // Drums: a low thump on the downbeat, a hiss on beats 2 and 3.
        let drums = [52, 0, 100, 0, 100, 0];
        for k in 0..ROWS_PER_BAR {
            rows.push([m[k], bass[k], reed[k], drums[k]]);
        }
    }
    rows
}

/// One cycle of 32 signed samples from harmonics (number, amplitude),
/// scaled to `peak`.
fn wave(harmonics: &[(f64, f64)], peak: f64) -> [u8; 32] {
    let raw: Vec<f64> = (0..32)
        .map(|t| {
            let x = 2.0 * PI * f64::from(t) / 32.0;
            harmonics.iter().map(|(h, a)| a * (h * x).sin()).sum()
        })
        .collect();
    let max = raw.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let mut out = [0u8; 32];
    for (o, v) in out.iter_mut().zip(raw) {
        *o = (v / max * peak).round() as i8 as u8;
    }
    out
}

/// A struck bell: two partials, decaying, 16 samples a cycle.
fn ping() -> Vec<u8> {
    (0..PING_LEN)
        .map(|t| {
            let t = t as f64;
            let x = 2.0 * PI * t / 16.0;
            let v = (x.sin() + 0.5 * (2.76 * x).sin()) * (-t / 450.0).exp();
            (v / 1.5 * 110.0).round() as i8 as u8
        })
        .collect()
}

/// Calliope period for MIDI note `m` with 32-sample waves.
fn period(m: u8) -> u16 {
    let f = 440.0 * 2f64.powf((f64::from(m) - 69.0) / 12.0);
    (4_000_000.0 / (32.0 * f)).round().clamp(16.0, 65535.0) as u16
}

fn block() -> Vec<u8> {
    let mut b = vec![0u8; END];
    let put16 =
        |b: &mut Vec<u8>, at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    put16(&mut b, VARS, 1); // the first row at the first VBlank
    put16(&mut b, VARS + 2, SONG as u16);
    for m in 0..128u8 {
        put16(&mut b, PERIODS + 2 * usize::from(m), period(m));
    }
    let rows = song();
    assert!(SONG + 4 * rows.len() <= BRASS);
    for (k, r) in rows.iter().enumerate() {
        b[SONG + 4 * k..SONG + 4 * k + 4].copy_from_slice(r);
    }
    let odd = |h: f64| if h as u32 % 2 == 1 { 1.0 } else { 0.7 };
    let brass: Vec<(f64, f64)> = (1..=8)
        .map(f64::from)
        .map(|h| (h, odd(h) / h.powf(1.1)))
        .collect();
    b[BRASS..BRASS + 32].copy_from_slice(&wave(&brass, 100.0));
    // Triangle: odd harmonics falling with 1/h², alternating.
    let tri: Vec<(f64, f64)> = [1.0, 3.0, 5.0, 7.0, 9.0]
        .iter()
        .enumerate()
        .map(|(k, h)| (*h, if k % 2 == 0 { 1.0 } else { -1.0 } / (h * h)))
        .collect();
    b[BASS..BASS + 32].copy_from_slice(&wave(&tri, 110.0));
    b[REED..REED + 32].copy_from_slice(&wave(
        &[(1.0, 1.0), (3.0, 0.4), (5.0, 0.2), (7.0, 0.1)],
        90.0,
    ));
    b[PING..END].copy_from_slice(&ping());
    b
}

/// Register/value pairs for the channels (offsets in the I/O segment).
fn setup() -> Vec<(u16, u16)> {
    let seg = SEG as u16;
    let ch = |n: u16, regs: &[(u16, u16)]| -> Vec<(u16, u16)> {
        regs.iter()
            .map(|&(r, v)| (0x3000 + 0x20 * n + r, v))
            .collect()
    };
    // AUDSEG 00, AUDOFS 02, AUDLENL 06, AUDPER 08, AUDVOL 0A, AUDPAN 0C,
    // AUDLLENL 14, AUDCTL 16, AUDENV1 18 (decay << 8 | attack), AUDENV2
    // 1A (release << 8 | sustain).
    let wave = |n: u16, ofs: usize, vol: u16, pan: u16, env1: u16, env2: u16| {
        ch(
            n,
            &[
                (0x00, seg),
                (0x02, ofs as u16),
                (0x06, 32),
                (0x14, 32),
                (0x08, period(69)),
                (0x0a, vol),
                (0x0c, pan),
                (0x18, env1),
                (0x1a, env2),
                (0x16, 0x11), // play, envelope, 8 bit
            ],
        )
    };
    let mut v = vec![(0x3100, 100)]; // AUDMASTER
    v.extend(wave(0, BRASS, 56, 16, 8 << 8 | 120, 20 << 8 | 180));
    v.extend(wave(1, BASS, 72, 16, 4 << 8 | 200, 30 << 8 | 150));
    v.extend(wave(2, REED, 34, 9, 20 << 8 | 150, 40 << 8 | 90));
    // Drums and the thud: noise, envelope, attack at once.
    v.extend(ch(
        3,
        &[
            (0x0a, 40),
            (0x0c, 22),
            (0x18, 60 << 8),
            (0x1a, 60 << 8),
            (0x08, period(100)),
            (0x16, 0x31),
        ],
    ));
    v.extend(ch(
        6,
        &[
            (0x0a, 70),
            (0x0c, 20),
            (0x18, 30 << 8),
            (0x1a, 30 << 8),
            (0x08, period(40)),
            (0x16, 0x31),
        ],
    ));
    // The ping: one shot, started by the sprite scene.
    v.extend(ch(
        7,
        &[
            (0x00, seg),
            (0x02, PING as u16),
            (0x06, PING_LEN as u16),
            (0x14, 0),
            (0x08, 125),
            (0x0a, 60),
            (0x0c, 12),
        ],
    ));
    v
}

fn words(out: &mut String, bytes: &[u8]) {
    for line in bytes.chunks(32) {
        let w: Vec<String> = line
            .chunks(2)
            .map(|p| format!("${:04x}", u16::from_le_bytes([p[0], p[1]])))
            .collect();
        writeln!(out, "        .word   {}", w.join(",")).unwrap();
    }
}

/// (`music.inc`, `music_data.inc`).
pub fn includes() -> (String, String) {
    let rows = song().len();
    let set = setup();
    let consts = format!(
        "; generated by build.rs (music.rs)\n\
         MUS_SEG  = ${SEG:04x}\nMUS_WORDS = {}\nMUS_TICK = ${VARS:04x}\nMUS_ROW  = ${:04x}\n\
         MUS_PER  = ${PERIODS:04x}\nMUS_SONG = ${SONG:04x}\nMUS_SONG_END = ${:04x}\n\
         MUS_SPEED = {SPEED}\nMUS_SETUP_N = {}\n",
        END / 2,
        VARS + 2,
        SONG + 4 * rows,
        set.len()
    );
    let mut data =
        String::from("; generated by build.rs (music.rs)\n        .align  2\nmus_blob:\n");
    words(&mut data, &block());
    data.push_str("mus_setup:\n");
    for (r, v) in set {
        writeln!(data, "        .word   ${r:04x}, ${v:04x}").unwrap();
    }
    (consts, data)
}
