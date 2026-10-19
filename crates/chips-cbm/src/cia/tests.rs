use super::*;

const NONE: u8 = 0xff;

/// Run one cycle: tick, then read `reg` (as the CPU would in that cycle).
fn read_cycle(c: &mut Cia, reg: u8) -> u8 {
    c.tick();
    c.read(reg, NONE, NONE)
}

fn write_cycle(c: &mut Cia, reg: u8, v: u8) {
    c.tick();
    c.write(reg, v);
}

fn idle(c: &mut Cia, n: usize) {
    for _ in 0..n {
        c.tick();
    }
}

fn timer_a(latch: u16) -> Cia {
    let mut c = Cia::new(CiaModel::Mos6526);
    c.write(0x04, latch as u8);
    c.write(0x05, (latch >> 8) as u8); // stopped: loads the counter
    idle(&mut c, 2);
    c
}

#[test]
fn power_on_state() {
    let c = Cia::new(CiaModel::Mos6526);
    assert_eq!((c.ta.counter, c.ta.latch), (0xffff, 0xffff));
    assert_eq!(c.peek(0x00, NONE, NONE), 0xff, "inputs pulled up");
    assert!(!c.irq());
}

#[test]
fn start_takes_effect_after_two_cycles() {
    let mut c = timer_a(10);
    write_cycle(&mut c, 0x0e, 0x01);
    let reads: Vec<u8> = (0..5).map(|_| read_cycle(&mut c, 0x04)).collect();
    assert_eq!(reads, [10, 10, 9, 8, 7]);
}

#[test]
fn continuous_mode_never_shows_zero_and_period_is_latch_plus_one() {
    let mut c = timer_a(2);
    write_cycle(&mut c, 0x0e, 0x01);
    let reads: Vec<u8> = (0..14).map(|_| read_cycle(&mut c, 0x04)).collect();
    // 2 2 | 1 2 2 | 1 2 2 | ...: after each reload the next clock is dropped.
    assert_eq!(reads, [2, 2, 1, 2, 2, 1, 2, 2, 1, 2, 2, 1, 2, 2]);
    assert!(!reads.contains(&0));
}

#[test]
fn underflow_sets_flag_and_irq_one_cycle_later_on_old_cia() {
    let mut c = timer_a(2);
    c.write(0x0d, 0x81);
    write_cycle(&mut c, 0x0e, 0x01);
    let mut flag_cycle = None;
    for i in 0..10 {
        c.tick();
        if flag_cycle.is_none() && c.peek(0x0d, NONE, NONE) & icr::TA != 0 {
            flag_cycle = Some(i);
            assert!(!c.irq(), "old 6526: IRQ follows one cycle later");
            c.tick();
            assert!(c.irq());
            break;
        }
    }
    assert!(flag_cycle.is_some());
    assert_eq!(c.read(0x0d, NONE, NONE), 0x81);
    assert!(!c.irq(), "reading ICR clears IRQ");
    assert_eq!(c.read(0x0d, NONE, NONE), 0x00, "and the flags");
}

#[test]
fn new_cia_raises_irq_in_the_flag_cycle() {
    let mut c = Cia::new(CiaModel::Mos6526A);
    c.write(0x04, 2);
    c.write(0x05, 0);
    c.write(0x0d, 0x81);
    write_cycle(&mut c, 0x0e, 0x01);
    for _ in 0..10 {
        c.tick();
        if c.peek(0x0d, NONE, NONE) & icr::TA != 0 {
            assert!(c.irq());
            return;
        }
    }
    panic!("no underflow");
}

#[test]
fn reading_icr_in_the_underflow_cycle_prevents_the_interrupt() {
    let mut c = timer_a(2);
    c.write(0x0d, 0x81);
    write_cycle(&mut c, 0x0e, 0x01);
    loop {
        c.tick();
        if c.peek(0x0d, NONE, NONE) & icr::TA != 0 {
            assert_eq!(c.read(0x0d, NONE, NONE), 0x01, "flag seen, no IRQ bit yet");
            break;
        }
    }
    c.tick();
    assert!(!c.irq(), "interrupt cancelled");
}

#[test]
fn enabling_the_mask_for_a_set_flag_raises_irq_and_clearing_does_not_lower_it() {
    let mut c = timer_a(1);
    c.write(0x0e, 0x09); // one-shot, start
    idle(&mut c, 10);
    assert_eq!(c.peek(0x0d, NONE, NONE), 0x01);
    assert!(!c.irq());
    write_cycle(&mut c, 0x0d, 0x81);
    c.tick();
    assert!(
        !c.irq(),
        "mask write takes effect one cycle later (Lorenz imr)"
    );
    c.tick();
    assert!(c.irq());
    write_cycle(&mut c, 0x0d, 0x01);
    assert!(c.irq(), "only an ICR read clears the IRQ");
}

#[test]
fn one_shot_stops_and_clears_start() {
    let mut c = timer_a(3);
    write_cycle(&mut c, 0x0e, 0x09);
    let reads: Vec<u8> = (0..10).map(|_| read_cycle(&mut c, 0x04)).collect();
    assert_eq!(reads, [3, 3, 2, 1, 3, 3, 3, 3, 3, 3]);
    assert_eq!(c.peek(0x0e, NONE, NONE) & CR_START, 0);
    assert_eq!(c.peek(0x0d, NONE, NONE), 0x01);
}

#[test]
fn force_load_is_a_strobe() {
    let mut c = timer_a(100);
    c.write(0x04, 0x34);
    c.write(0x05, 0x12); // stopped: loads $1234 too
    idle(&mut c, 2);
    c.write(0x04, 0x78);
    c.write(0x05, 0x56);
    idle(&mut c, 2);
    assert_eq!(c.ta.counter, 0x5678);
    c.tick(); // leave the reload cycle
    c.write(0x04, 0x00); // latch $5600; a low-byte write never loads
    write_cycle(&mut c, 0x0e, 0x10);
    assert_eq!(c.peek(0x0e, NONE, NONE), 0, "strobe not stored");
    // The load happens two cycles after the write (LoadA0 → LoadA1).
    assert_eq!(read_cycle(&mut c, 0x04), 0x78);
    assert_eq!(read_cycle(&mut c, 0x04), 0x00);
}

#[test]
fn high_byte_write_loads_only_when_stopped() {
    let mut c = timer_a(0x2000);
    write_cycle(&mut c, 0x0e, 0x01);
    idle(&mut c, 10);
    let before = c.ta.counter;
    write_cycle(&mut c, 0x05, 0x40);
    c.tick();
    assert_eq!(c.ta.latch, 0x4000);
    assert!(
        c.ta.counter < before,
        "running timer keeps counting from its value"
    );
}

#[test]
fn timer_b_counts_timer_a_underflows() {
    let mut c = timer_a(4); // TA period 5
    c.write(0x06, 3);
    c.write(0x07, 0);
    idle(&mut c, 2);
    c.write(0x0f, 0x41); // TB: count TA underflows, start
    c.write(0x0e, 0x01);
    idle(&mut c, 5 * 4 + 5);
    assert_eq!(
        c.peek(0x0d, NONE, NONE) & icr::TB,
        icr::TB,
        "TB underflowed after 4 TA underflows"
    );
}

#[test]
fn pb6_pulse_and_toggle_outputs() {
    let mut c = timer_a(2);
    c.write(0x0e, 0x03); // start, PB6 on, pulse mode
    let pulses: Vec<bool> = (0..10)
        .map(|_| {
            c.tick();
            c.port_b_out() & 0x40 != 0
        })
        .collect();
    // First underflow in the 4th cycle after start, then every 3 cycles.
    let at: Vec<usize> = (0..10).filter(|&i| pulses[i]).collect();
    assert_eq!(at, [3, 6, 9], "one-cycle pulse per underflow");

    let mut c = timer_a(2);
    c.write(0x0e, 0x07); // toggle mode: START rising edge sets output high
    assert_ne!(c.port_b_out() & 0x40, 0);
    let levels: Vec<bool> = (0..9)
        .map(|_| {
            c.tick();
            c.port_b_out() & 0x40 != 0
        })
        .collect();
    assert_eq!(
        levels,
        [true, true, true, false, false, false, true, true, true]
    );
}

#[test]
fn ports_combine_outputs_with_external_pull_downs() {
    let mut c = Cia::new(CiaModel::Mos6526);
    c.write(0x02, 0xff);
    c.write(0x00, 0xfe); // drive PA0 low (keyboard column 0)
    assert_eq!(c.port_a_out(), 0xfe);
    // Keyboard pulls PB3 low; DDRB = 0 so port B is input.
    assert_eq!(c.read(0x01, NONE, 0xf7), 0xf7);
    // An output driven high reads low if something external pulls it down.
    assert_eq!(c.read(0x00, 0x7f, NONE), 0x7e);
}

#[test]
fn time_of_day_counts_latches_and_alarms() {
    let mut c = Cia::new(CiaModel::Mos6526);
    c.write(0x0e, 0x80); // 50 Hz
    c.write(0x0b, 0x11);
    c.write(0x0a, 0x59);
    c.write(0x09, 0x59);
    c.write(0x08, 0x09); // starts the clock
    c.write(0x0f, 0x80); // writes now go to the alarm
    c.write(0x0b, 0x92); // 12 PM
    c.write(0x0a, 0x00);
    c.write(0x09, 0x00);
    c.write(0x08, 0x00);
    c.write(0x0d, 0x84);
    for _ in 0..4 {
        c.tod_tick();
    }
    assert_eq!(c.peek(0x08, NONE, NONE), 9, "five ticks per tenth at 50 Hz");
    c.tod_tick();
    assert_eq!(c.read(0x0b, NONE, NONE), 0x92, "11 AM rolls to 12 PM");
    assert_eq!(c.peek(0x0d, NONE, NONE) & icr::ALARM, icr::ALARM);
    // Latched until tenths is read.
    for _ in 0..5 {
        c.tod_tick();
    }
    assert_eq!(c.read(0x09, NONE, NONE), 0x00);
    assert_eq!(c.read(0x08, NONE, NONE), 0x00, "latched value");
    assert_eq!(c.read(0x08, NONE, NONE), 0x01, "latch released");
}

#[test]
fn icr_read_in_the_timer_b_underflow_cycle_reports_the_flag() {
    // Lorenz cia1tb (old CIA) #05: the flag is visible in that very cycle.
    for model in [CiaModel::Mos6526, CiaModel::Mos6526A] {
        let mut c = Cia::new(model);
        c.write(0x06, 2);
        c.write(0x07, 0);
        idle(&mut c, 2);
        c.write(0x0f, 0x01);
        let mut seen = None;
        for _ in 0..10 {
            c.tick();
            if c.peek(0x0d, NONE, NONE) & icr::TB != 0 {
                seen = Some(c.read(0x0d, NONE, NONE) & icr::TB);
                break;
            }
        }
        assert_eq!(seen, Some(icr::TB), "{model:?}");
    }
}

#[test]
fn latch_write_in_the_reload_cycle_reaches_the_counter() {
    // Lorenz cia1ta #08.
    let mut c = timer_a(2);
    write_cycle(&mut c, 0x0e, 0x01);
    loop {
        c.tick();
        if c.peek(0x0d, NONE, NONE) & icr::TA != 0 {
            break;
        }
    }
    c.write(0x04, 20);
    assert_eq!(c.ta.counter, 20);
    c.tick();
    c.write(0x04, 30);
    assert_eq!(c.ta.counter, 20, "only in the reload cycle");
}
#[test]
fn savestate_round_trip_mid_count() {
    let mut a = timer_a(7);
    a.write(0x0d, 0x81);
    a.write(0x0e, 0x01);
    a.write(0x0f, 0x41);
    idle(&mut a, 13);
    let mut w = StateWriter::new();
    a.save(&mut w);
    let data = w.into_bytes();
    let mut b = Cia::new(CiaModel::Mos6526);
    b.load(&mut StateReader::new(&data)).unwrap();
    assert_eq!(a, b);
    for _ in 0..50 {
        assert_eq!(read_cycle(&mut a, 0x04), read_cycle(&mut b, 0x04));
        assert_eq!(a.irq(), b.irq());
    }

    let before = b.clone();
    assert!(b
        .load(&mut StateReader::new(&data[..data.len() - 2]))
        .is_err());
    assert_eq!(b, before);
}
