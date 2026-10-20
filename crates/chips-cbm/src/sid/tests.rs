use super::*;

const CLOCK: u32 = 985_248;

fn sid() -> Sid {
    Sid::new(SidModel::Mos8580, CLOCK)
}

/// Voice 3 (registers $0E–$14) with `ctrl`, freq and ADSR.
fn voice3(s: &mut Sid, freq: u16, ctrl: u8, ad: u8, sr: u8) {
    s.write(0x0e, freq as u8);
    s.write(0x0f, (freq >> 8) as u8);
    s.write(0x13, ad);
    s.write(0x14, sr);
    s.write(0x12, ctrl);
}

#[test]
fn sawtooth_period_follows_the_frequency_register() {
    let mut s = sid();
    voice3(&mut s, 0x1000, 0x20, 0, 0);
    let mut wraps = 0;
    let mut last = 0;
    for _ in 0..40_960 {
        s.clock();
        let osc = s.peek(0x1b);
        if osc < last {
            wraps += 1;
        }
        last = osc;
    }
    // 2^24 / 4096 = 4096 cycles per period.
    assert_eq!(wraps, 10);
}

#[test]
fn attack_time_matches_the_data_sheet() {
    let mut s = sid();
    voice3(&mut s, 0, 0x01, 0x00, 0xf0); // attack 2 ms, sustain 15
    let mut cycles = 0;
    while s.peek(0x1c) < 255 {
        s.clock();
        cycles += 1;
        assert!(cycles < 5_000);
    }
    // 2 ms at 1 MHz.
    assert!((1_700..=2_100).contains(&cycles), "{cycles}");

    let mut s = sid();
    voice3(&mut s, 0, 0x01, 0xa0, 0xf0); // attack 500 ms
    for _ in 0..250_000 {
        s.clock();
    }
    assert!((120..=135).contains(&s.peek(0x1c)), "half way after 250 ms");
}

#[test]
fn decay_to_sustain_then_release_to_zero() {
    let mut s = sid();
    voice3(&mut s, 0, 0x01, 0x00, 0x80); // sustain 8 → level 136
    for _ in 0..200_000 {
        s.clock();
    }
    assert_eq!(s.peek(0x1c), 136);
    s.write(0x12, 0x00); // gate off, release rate 0
    for _ in 0..200_000 {
        s.clock();
    }
    assert_eq!(s.peek(0x1c), 0);
}

#[test]
fn noise_is_noisy_and_test_bit_resets() {
    let mut s = sid();
    voice3(&mut s, 0xffff, 0x80, 0, 0);
    let samples: Vec<u8> = (0..2_000)
        .map(|_| {
            s.clock();
            s.peek(0x1b)
        })
        .collect();
    let distinct = samples
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert!(distinct > 50, "{distinct} distinct values");
    s.write(0x12, 0x88);
    s.clock();
    assert_eq!(s.voices[2].acc, 0);
}

#[test]
fn hard_sync_resets_the_synced_oscillator() {
    let mut s = sid();
    // Voice 2 (source of voice 3's sync) fast, voice 3 slow with sync.
    s.write(0x07, 0x00);
    s.write(0x08, 0x40);
    voice3(&mut s, 0x0100, 0x22, 0, 0);
    let mut max = 0;
    for _ in 0..20_000 {
        s.clock();
        max = max.max(s.voices[2].acc);
    }
    // Without sync voice 3 would reach 20 000 × $100; sync keeps it below
    // one period of voice 2 ($100 × 2^24/$4000 = 1024 cycles).
    assert!(max <= 0x100 * 1024 + 0x100, "{max:x}");
}

fn rms(s: &mut Sid, n: usize) -> f64 {
    let v: Vec<f64> = (0..n).map(|_| f64::from(s.clock())).collect();
    let mean = v.iter().sum::<f64>() / n as f64;
    (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt()
}

#[test]
fn low_pass_filter_attenuates_high_frequencies() {
    let tone = |filter: bool| {
        let mut s = sid();
        s.write(0x18, 0x1f); // LP, volume 15
        s.write(0x15, 0);
        s.write(0x16, 0x04); // low cutoff
        s.write(0x17, if filter { 0x04 } else { 0x00 });
        voice3(&mut s, 0x8000, 0x41, 0x00, 0xf0); // ~15 kHz pulse
        s.write(0x10, 0x00);
        s.write(0x11, 0x08);
        for _ in 0..5_000 {
            s.clock();
        }
        rms(&mut s, 20_000)
    };
    let (dry, wet) = (tone(false), tone(true));
    assert!(wet < dry / 10.0, "dry {dry:.0} filtered {wet:.0}");
}

#[test]
fn volume_register_is_audible_on_the_6581_only() {
    let mut a = Sid::new(SidModel::Mos6581, CLOCK);
    let mut b = Sid::new(SidModel::Mos8580, CLOCK);
    let level = |s: &mut Sid, vol: u8| {
        s.write(0x18, vol);
        s.clock()
    };
    assert_ne!(level(&mut a, 0), level(&mut a, 15));
    assert_eq!(level(&mut b, 0), level(&mut b, 15));
}

#[test]
fn write_only_registers_read_the_last_bus_value() {
    let mut s = sid();
    s.write(0x05, 0x5a);
    assert_eq!(s.peek(0x00), 0x5a);
    assert_eq!(s.peek(0x19), 0xff, "POTX without paddles");
    assert_eq!(s.peek(0x3c), s.peek(0x1c), "mirrored every 32 bytes");
}

#[test]
fn savestate_round_trip() {
    let mut a = Sid::new(SidModel::Mos6581, CLOCK);
    a.write(0x18, 0x1f);
    a.write(0x17, 0xf1);
    a.write(0x16, 0x40);
    a.write(0x00, 0x34);
    a.write(0x01, 0x12);
    a.write(0x05, 0x22);
    a.write(0x04, 0x41);
    for _ in 0..7_777 {
        a.clock();
    }
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = Sid::new(SidModel::Mos6581, CLOCK);
    b.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(a, b);
    let x: Vec<i32> = (0..1000).map(|_| a.clock()).collect();
    let y: Vec<i32> = (0..1000).map(|_| b.clock()).collect();
    assert_eq!(x, y);
    assert!(b.load(&mut StateReader::new(&data[..20])).is_err());
}
