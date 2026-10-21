use super::*;

fn pokey() -> Pokey {
    let mut p = Pokey::new();
    p.write(0x0f, 0x03); // SKCTL: out of init, keyboard scan + debounce
    p
}

/// Cycles until IRQST shows `bit`, up to `limit`.
fn cycles_until(p: &mut Pokey, bit: u8, limit: u32) -> Option<u32> {
    (1..=limit).find(|_| {
        p.tick();
        p.peek(0x0e) & bit == 0
    })
}

/// Cycles between two consecutive IRQs of `bit` (after the first one).
fn period(p: &mut Pokey, bit: u8) -> u32 {
    p.write(0x0e, bit);
    cycles_until(p, bit, 100_000).expect("first IRQ");
    p.write(0x0e, 0);
    p.write(0x0e, bit);
    cycles_until(p, bit, 100_000).expect("second IRQ")
}

#[test]
fn timer_1_at_64_khz() {
    let mut p = pokey();
    p.write(0x00, 9); // AUDF1
    assert_eq!(
        period(&mut p, IRQ_TIMER1),
        10 * 28,
        "AUDF+1 ticks of 28 cycles"
    );
    assert!(p.irq());
}

#[test]
fn fast_and_joined_periods() {
    let mut p = pokey();
    p.write(0x08, 0x40); // channel 1 at 1.79 MHz
    p.write(0x00, 10);
    assert_eq!(period(&mut p, IRQ_TIMER1), 10 + 4);
    // First IRQ after STIMER: AUDF+4 cycles to the underflow, 3 more until
    // it takes effect, visible the cycle after.
    p.tick();
    p.write(0x0e, 0);
    p.write(0x09, 0);
    p.write(0x0e, IRQ_TIMER1);
    assert_eq!(cycles_until(&mut p, IRQ_TIMER1, 1000), Some(10 + 8));

    let mut p = pokey();
    p.write(0x08, 0x28); // join 3+4, channel 3 at 1.79 MHz
    p.write(0x04, 0x28); // AUDF3
    p.write(0x06, 0x00); // AUDF4
    assert_eq!(period(&mut p, IRQ_TIMER4), 0x28 + 7, "AUDF16+7");
}

#[test]
fn keyboard_scan_latches_with_debounce() {
    let mut p = pokey();
    p.write(0x0e, IRQ_KEY);
    p.set_shift(true);
    p.set_key(0x3f, true); // A
    let at = cycles_until(&mut p, IRQ_KEY, 64 * 114 * 3).expect("key IRQ");
    assert!(at > 64 * 114, "seen on two passes: {at}");
    assert_eq!(p.peek(0x09), 0x7f, "KBCODE A + SHIFT");
    assert_eq!(p.peek(0x0f) & 0x0c, 0, "SKSTAT key and shift down");
    p.set_key(0x3f, false);
    for _ in 0..64 * 114 {
        p.tick();
    }
    assert_eq!(p.peek(0x0f) & 0x04, 0x04, "released");
}

#[test]
fn serial_output_at_19200_baud() {
    let mut p = pokey();
    p.write(0x0f, 0x23); // serial output clocked by channel 4
    p.write(0x08, 0x28);
    p.write(0x04, 0x28);
    p.write(0x06, 0x00);
    p.write(0x0e, IRQ_SEROUT_NEEDED | IRQ_SEROUT_DONE);
    assert_eq!(p.peek(0x0e) & IRQ_SEROUT_DONE, 0, "idle: finished");
    p.write(0x0d, 0x31);
    // The shifter takes the byte on the next serial clock.
    cycles_until(&mut p, IRQ_SEROUT_NEEDED, 200).expect("holding register free");
    assert_ne!(p.peek(0x0e) & IRQ_SEROUT_DONE, 0, "busy");
    p.write(0x0e, IRQ_SEROUT_DONE);
    let t = cycles_until(&mut p, IRQ_SEROUT_DONE, 10_000).unwrap();
    // Ten bits of two timer-4 periods of 47 cycles (the first period
    // depends on where the free-running timer stands).
    assert!((10 * 2 * 47 - 100..=10 * 2 * 47 + 100).contains(&t), "{t}");
    assert_eq!(p.take_sent(), vec![0x31]);
}

#[test]
fn random_runs_except_in_init() {
    let mut p = pokey();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..200 {
        p.tick();
        seen.insert(p.peek(0x0a));
    }
    assert!(seen.len() > 50, "{} distinct values", seen.len());
    // Initialization mode fills the counters with ones.
    p.write(0x0f, 0);
    for _ in 0..20 {
        p.tick();
    }
    assert_eq!(p.peek(0x0a), 0xff);
    p.tick();
    assert_eq!(p.peek(0x0a), 0xff);
}

#[test]
fn irqen_clears_pending() {
    let mut p = pokey();
    p.write(0x0e, IRQ_BREAK);
    p.press_break();
    assert_eq!(p.peek(0x0e) & IRQ_BREAK, 0);
    p.write(0x0e, 0);
    assert_eq!(p.peek(0x0e) & IRQ_BREAK, IRQ_BREAK);
    assert!(!p.irq());
}

#[test]
fn savestate_round_trip() {
    let mut p = pokey();
    p.write(0x00, 5);
    p.write(0x0d, 0x42);
    p.set_key(5, true);
    for _ in 0..500 {
        p.tick();
    }
    let mut w = StateWriter::new();
    p.save(&mut w);
    let bytes = w.into_bytes();
    let mut q = Pokey::new();
    q.load(&mut StateReader::new(&bytes)).unwrap();
    assert_eq!(p, q);
    assert!(q.load(&mut StateReader::new(&bytes[..8])).is_err());
}

/// Output levels over `n` cycles.
fn levels(p: &mut Pokey, n: usize) -> Vec<u8> {
    (0..n)
        .map(|_| {
            p.tick();
            p.output()
        })
        .collect()
}

#[test]
fn pure_tone_and_volume_only() {
    let mut p = pokey();
    p.write(0x08, 0x40); // channel 1 at 1.79 MHz
    p.write(0x00, 46); // period 50 cycles
    p.write(0x01, 0xa8); // pure tone, volume 8
    let out = levels(&mut p, 1000);
    assert!(out.iter().all(|&v| v == 0 || v == 8));
    // A square wave: the output changes every 50 cycles.
    let changes = out.windows(2).filter(|w| w[0] != w[1]).count();
    assert!((19..=21).contains(&changes), "{changes}");
    p.write(0x01, 0x18); // volume only
    assert!(levels(&mut p, 100).iter().all(|&v| v == 8));
}

#[test]
fn polynomial_distortion_is_irregular() {
    let mut p = pokey();
    p.write(0x08, 0x40);
    p.write(0x00, 6); // underflow every 10 cycles
    p.write(0x01, 0x08); // 5-bit + 17-bit polynomial noise
    let out = levels(&mut p, 20_000);
    let high = out.iter().filter(|&&v| v == 8).count();
    assert!(
        (4_000..16_000).contains(&high),
        "noise mixes levels: {high}"
    );
    // Runs of equal level vary in length (not a square wave).
    let mut runs = std::collections::BTreeSet::new();
    let mut len = 0;
    for w in out.windows(2) {
        len += 1;
        if w[0] != w[1] {
            runs.insert(len);
            len = 0;
        }
    }
    assert!(runs.len() > 3, "{runs:?}");
}

#[test]
fn high_pass_filter_cancels_equal_frequencies() {
    let mut p = pokey();
    p.write(0x08, 0x64); // ch1 + ch3 at 1.79 MHz, filter ch1 by ch3
    p.write(0x00, 46);
    p.write(0x04, 46);
    p.write(0x01, 0xa8);
    p.write(0x09, 0);
    let out = levels(&mut p, 2000);
    let high = out.iter().filter(|&&v| v != 0).count();
    assert!(high < 100, "filtered to (almost) silence: {high}");
}

#[test]
fn pots_count_lines_after_potgo() {
    let mut p = pokey();
    p.set_pot(0, 100);
    p.set_pot(1, 1);
    p.write(0x0b, 0); // POTGO
    assert_eq!(p.peek(0x08), 0xff, "all counting");
    for _ in 0..50 * 114 {
        p.tick();
    }
    assert_eq!(p.peek(0x08) & 0x03, 0x01, "pot 1 done, pot 0 counting");
    assert_eq!(p.peek(0x01), 1);
    for _ in 0..60 * 114 {
        p.tick();
    }
    assert_eq!(p.peek(0x00), 100);
    for _ in 0..130 * 114 {
        p.tick();
    }
    assert_eq!(p.peek(0x08), 0, "scan over");
    assert_eq!(p.peek(0x02), 228, "nothing connected");
    // Fast scan: one step per cycle.
    p.write(0x0f, 0x07);
    p.write(0x0b, 0);
    for _ in 0..110 {
        p.tick();
    }
    assert_eq!(p.peek(0x00), 100);
}

#[test]
fn skctl_write_drops_the_byte_coming_in() {
    let mut p = pokey();
    p.write(0x0e, IRQ_SERIN);
    p.write(0x0f, 0x2b);
    p.set_serial_line(false, true);
    // The same mode again does not disturb the receiver.
    p.write(0x0f, 0x2b);
    p.set_serial_line(true, false);
    p.receive(0x01);
    assert_eq!(p.peek(0x0d), 0x01);
    p.write(0x0e, 0);
    p.write(0x0e, IRQ_SERIN);
    p.set_serial_line(false, true);
    p.write(0x0f, 0x13);
    p.set_serial_line(true, false);
    p.receive(0x55);
    assert_eq!(p.peek(0x0e) & IRQ_SERIN, IRQ_SERIN, "lost");
    p.receive(0xfa);
    assert_eq!(p.peek(0x0e) & IRQ_SERIN, 0);
    assert_eq!(p.peek(0x0d), 0xfa);
}
