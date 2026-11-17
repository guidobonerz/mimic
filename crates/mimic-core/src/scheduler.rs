//! Master clock, timed events and clock dividers.
//!
//! A machine owns one [`Scheduler`] and advances it once per master cycle.
//! Chips that would otherwise count down every cycle (timers, drive motors,
//! input injection, frame end) schedule an [`Event`] instead and are woken
//! when it is due. Events due at the same cycle fire in the order they were
//! scheduled, so runs are deterministic (golden rule 3).
//!
//! Devices on other clocks (1541 CPU next to a C64, Amiga E clock) use a
//! [`ClockDivider`]: an exact integer ratio with no drift and no floats.

use crate::cycle::Cycle;
use crate::savestate::{Savestate, StateError, StateReader, StateWriter};

/// Machine-defined event identifier, e.g. `const CIA1_TIMER_A: EventId = EventId(1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub u16);

/// A due event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub at: Cycle,
    pub id: EventId,
    /// Free payload for the machine (e.g. a device index).
    pub data: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    at: u64,
    seq: u64,
    id: EventId,
    data: u32,
}

impl Entry {
    fn key(&self) -> (u64, u64) {
        (self.at, self.seq)
    }
}

/// Upper bound on queued events; protects savestate loading against
/// absurd counts. Real machines need a few dozen.
pub const MAX_EVENTS: usize = 4096;

/// Master clock plus a deterministic event queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scheduler {
    now: u64,
    /// Sorted latest-first, so the next event is at the end.
    queue: Vec<Entry>,
    next_seq: u64,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn now(&self) -> Cycle {
        Cycle(self.now)
    }

    /// Advance the master clock by one cycle.
    #[inline]
    pub fn tick(&mut self) {
        self.now += 1;
    }

    /// Advance the master clock by `n` cycles without firing events; use
    /// [`Scheduler::next_at`] to avoid skipping over one.
    pub fn advance(&mut self, n: u64) {
        self.now += n;
    }

    /// Schedule `id` at absolute time `at`. Times in the past fire on the
    /// next [`Scheduler::pop_due`]. Returns false (and drops the event) if
    /// the queue is full; machines treat that as a fault.
    pub fn schedule_at(&mut self, at: Cycle, id: EventId, data: u32) -> bool {
        if self.queue.len() >= MAX_EVENTS {
            return false;
        }
        let entry = Entry {
            at: at.0.max(self.now),
            seq: self.next_seq,
            id,
            data,
        };
        self.next_seq += 1;
        // Latest first: insert before the first entry that is due earlier.
        let pos = self.queue.partition_point(|e| e.key() > entry.key());
        self.queue.insert(pos, entry);
        true
    }

    /// Schedule `id` `delay` cycles from now.
    pub fn schedule_in(&mut self, delay: u64, id: EventId, data: u32) -> bool {
        self.schedule_at(Cycle(self.now.saturating_add(delay)), id, data)
    }

    /// Remove every pending event with `id`; returns how many were removed.
    pub fn cancel(&mut self, id: EventId) -> usize {
        let before = self.queue.len();
        self.queue.retain(|e| e.id != id);
        before - self.queue.len()
    }

    /// Time of the earliest pending event with `id`.
    pub fn pending(&self, id: EventId) -> Option<Cycle> {
        self.queue
            .iter()
            .rev()
            .find(|e| e.id == id)
            .map(|e| Cycle(e.at))
    }

    /// Time of the next pending event.
    #[inline]
    pub fn next_at(&self) -> Option<Cycle> {
        self.queue.last().map(|e| Cycle(e.at))
    }

    /// Remove and return the next event if it is due (`at <= now`). Call in
    /// a loop each cycle: `while let Some(ev) = sched.pop_due() { … }`.
    #[inline]
    pub fn pop_due(&mut self) -> Option<Event> {
        match self.queue.last() {
            Some(e) if e.at <= self.now => {
                let e = self.queue.pop()?;
                Some(Event {
                    at: Cycle(e.at),
                    id: e.id,
                    data: e.data,
                })
            }
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

const SCHED_TAG: [u8; 4] = *b"SCHD";
const SCHED_VERSION: u16 = 1;

impl Savestate for Scheduler {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(SCHED_TAG, SCHED_VERSION, |w| {
            w.u64(self.now);
            w.u64(self.next_seq);
            w.u32(self.queue.len() as u32);
            for e in &self.queue {
                w.u64(e.at);
                w.u64(e.seq);
                w.u16(e.id.0);
                w.u32(e.data);
            }
        });
    }

    /// Atomic: on error the scheduler is unchanged.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(SCHED_TAG, SCHED_VERSION)?;
        let now = c.u64()?;
        let next_seq = c.u64()?;
        let count = c.u32()? as usize;
        if count > MAX_EVENTS {
            return Err(StateError::Invalid("scheduler event count"));
        }
        let mut queue = Vec::with_capacity(count);
        for _ in 0..count {
            let entry = Entry {
                at: c.u64()?,
                seq: c.u64()?,
                id: EventId(c.u16()?),
                data: c.u32()?,
            };
            if entry.seq >= next_seq {
                return Err(StateError::Invalid("scheduler event sequence"));
            }
            queue.push(entry);
        }
        if !queue.windows(2).all(|w| w[0].key() > w[1].key()) {
            return Err(StateError::Invalid("scheduler queue order"));
        }
        *self = Scheduler {
            now,
            queue,
            next_seq,
        };
        Ok(())
    }
}

/// Exact clock ratio: a device clock running at `num/den` of the master
/// clock. Each master cycle yields 0, 1 or more device ticks; the remainder
/// is carried, so the long-run rate is exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockDivider {
    num: u32,
    den: u32,
    acc: u32,
}

impl ClockDivider {
    /// Device clock = master clock × `num` / `den`. `None` if `den` is 0.
    /// Example: 1541 (1 000 000 Hz) on a PAL C64 (985 248 Hz):
    /// `ClockDivider::new(1_000_000, 985_248)`.
    pub fn new(num: u32, den: u32) -> Option<Self> {
        (den != 0).then_some(Self { num, den, acc: 0 })
    }

    /// Advance one master cycle; returns the number of device ticks.
    #[inline]
    pub fn tick(&mut self) -> u32 {
        let total = u64::from(self.acc) + u64::from(self.num);
        let den = u64::from(self.den);
        self.acc = (total % den) as u32;
        (total / den) as u32
    }

    pub fn ratio(&self) -> (u32, u32) {
        (self.num, self.den)
    }
}

const CLKD_TAG: [u8; 4] = *b"CLKD";
const CLKD_VERSION: u16 = 1;

impl Savestate for ClockDivider {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(CLKD_TAG, CLKD_VERSION, |w| {
            w.u32(self.num);
            w.u32(self.den);
            w.u32(self.acc);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(CLKD_TAG, CLKD_VERSION)?;
        let (num, den, acc) = (c.u32()?, c.u32()?, c.u32()?);
        if den == 0 || acc >= den {
            return Err(StateError::Invalid("clock divider"));
        }
        *self = ClockDivider { num, den, acc };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: EventId = EventId(1);
    const B: EventId = EventId(2);

    fn drain(s: &mut Scheduler) -> Vec<(u64, u16, u32)> {
        let mut out = Vec::new();
        while let Some(e) = s.pop_due() {
            out.push((e.at.0, e.id.0, e.data));
        }
        out
    }

    #[test]
    fn events_fire_in_time_then_schedule_order() {
        let mut s = Scheduler::new();
        s.schedule_in(5, A, 1);
        s.schedule_in(3, B, 2);
        s.schedule_in(5, B, 3);
        s.schedule_in(3, A, 4);
        assert_eq!(s.next_at(), Some(Cycle(3)));
        s.advance(2);
        assert!(drain(&mut s).is_empty());
        s.tick();
        assert_eq!(drain(&mut s), [(3, 2, 2), (3, 1, 4)]);
        s.advance(10);
        assert_eq!(drain(&mut s), [(5, 1, 1), (5, 2, 3)]);
        assert!(s.is_empty());
    }

    #[test]
    fn past_events_fire_now() {
        let mut s = Scheduler::new();
        s.advance(100);
        s.schedule_at(Cycle(10), A, 0);
        assert_eq!(s.next_at(), Some(Cycle(100)));
        assert_eq!(drain(&mut s), [(100, 1, 0)]);
    }

    #[test]
    fn cancel_and_pending() {
        let mut s = Scheduler::new();
        s.schedule_in(9, A, 0);
        s.schedule_in(4, A, 0);
        s.schedule_in(6, B, 0);
        assert_eq!(s.pending(A), Some(Cycle(4)));
        assert_eq!(s.cancel(A), 2);
        assert_eq!(s.pending(A), None);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn queue_is_bounded() {
        let mut s = Scheduler::new();
        for _ in 0..MAX_EVENTS {
            assert!(s.schedule_in(1, A, 0));
        }
        assert!(!s.schedule_in(1, A, 0));
    }

    #[test]
    fn savestate_round_trip_continues_identically() {
        let mut s = Scheduler::new();
        s.schedule_in(7, A, 1);
        s.schedule_in(7, B, 2);
        s.schedule_in(2, B, 3);
        s.advance(3);
        let mut w = StateWriter::new();
        s.save(&mut w);
        let data = w.into_bytes();

        let mut t = Scheduler::new();
        t.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(t, s);
        s.schedule_in(4, A, 9);
        t.schedule_in(4, A, 9);
        s.advance(10);
        t.advance(10);
        assert_eq!(drain(&mut s), drain(&mut t));
    }

    #[test]
    fn corrupt_savestate_leaves_scheduler_unchanged() {
        let mut s = Scheduler::new();
        s.schedule_in(7, A, 1);
        s.schedule_in(2, B, 3);
        let mut w = StateWriter::new();
        s.save(&mut w);
        let mut data = w.into_bytes();

        let mut t = Scheduler::new();
        t.schedule_in(1, A, 42);
        let before = t.clone();
        assert!(t
            .load(&mut StateReader::new(&data[..data.len() - 1]))
            .is_err());
        assert_eq!(t, before);

        // Swap the two entries' times: queue order becomes invalid.
        // Layout: header 10, now 8, next_seq 8, count 4, entries of 22 bytes.
        let first = 10 + 8 + 8 + 4;
        let second = first + 22;
        for i in 0..8 {
            data.swap(first + i, second + i);
        }
        assert_eq!(
            t.load(&mut StateReader::new(&data)),
            Err(StateError::Invalid("scheduler queue order"))
        );
        assert_eq!(t, before);
    }

    #[test]
    fn clock_divider_is_exact_over_a_second() {
        let mut d = ClockDivider::new(1_000_000, 985_248).unwrap();
        let ticks: u64 = (0..985_248).map(|_| u64::from(d.tick())).sum();
        assert_eq!(ticks, 1_000_000);

        let mut e = ClockDivider::new(1, 10).unwrap();
        let pattern: Vec<u32> = (0..20).map(|_| e.tick()).collect();
        assert_eq!(pattern.iter().sum::<u32>(), 2);
        assert_eq!(pattern[9], 1);
        assert!(ClockDivider::new(1, 0).is_none());
    }

    #[test]
    fn clock_divider_savestate() {
        let mut d = ClockDivider::new(3, 7).unwrap();
        d.tick();
        d.tick();
        let mut w = StateWriter::new();
        d.save(&mut w);
        let data = w.into_bytes();
        let mut e = ClockDivider::new(1, 1).unwrap();
        e.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(e, d);
        assert_eq!(
            (0..7).map(|_| e.tick()).collect::<Vec<_>>(),
            (0..7).map(|_| d.tick()).collect::<Vec<_>>()
        );
    }
}
