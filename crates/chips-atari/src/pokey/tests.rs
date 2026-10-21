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

#[test]
fn timer_1_at_64_khz() {
    let mut p = pokey();
    p.write(0x00, 9); // AUDF1
    p.write(0x0e, IRQ_TIMER1);
    p.write(0x09, 0); // STIMER
    let first = cycles_until(&mut p, IRQ_TIMER1, 10_000).unwrap();
    p.write(0x0e, 0);
    p.write(0x0e, IRQ_TIMER1);
    let second = cycles_until(&mut p, IRQ_TIMER1, 10_000).unwrap();
    assert_eq!(second, 10 * 28, "AUDF+1 ticks of 28 cycles");
    assert!(first <= 10 * 28);
    assert!(p.irq());
}

#[test]
fn fast_and_joined_periods() {
    let mut p = pokey();
    p.write(0x08, 0x40); // channel 1 at 1.79 MHz
    p.write(0x00, 10);
    p.write(0x0e, IRQ_TIMER1);
    p.write(0x09, 0);
    let first = cycles_until(&mut p, IRQ_TIMER1, 1000).unwrap();
    assert_eq!(first, 10 + 4);

    let mut p = pokey();
    p.write(0x08, 0x28); // join 3+4, channel 3 at 1.79 MHz
    p.write(0x04, 0x28); // AUDF3
    p.write(0x06, 0x00); // AUDF4
    p.write(0x0e, IRQ_TIMER4);
    p.write(0x09, 0);
    let first = cycles_until(&mut p, IRQ_TIMER4, 1000).unwrap();
    assert_eq!(first, 0x28 + 7);
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
    p.write(0x08, 0x28);
    p.write(0x04, 0x28);
    p.write(0x06, 0x00);
    p.write(0x0e, IRQ_SEROUT_NEEDED | IRQ_SEROUT_DONE);
    assert_eq!(p.peek(0x0e) & IRQ_SEROUT_DONE, 0, "idle: finished");
    p.write(0x0d, 0x31);
    assert_eq!(p.peek(0x0e) & IRQ_SEROUT_NEEDED, 0, "holding register free");
    assert_ne!(p.peek(0x0e) & IRQ_SEROUT_DONE, 0, "busy");
    p.write(0x0e, IRQ_SEROUT_DONE);
    let t = cycles_until(&mut p, IRQ_SEROUT_DONE, 10_000).unwrap();
    // Ten bits of two timer-4 periods of 47 cycles.
    assert!((10 * 2 * 47 - 47..=10 * 2 * 47 + 47).contains(&t), "{t}");
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
    p.write(0x0f, 0);
    let held = p.peek(0x0a);
    p.tick();
    assert_eq!(p.peek(0x0a), held);
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
