//! Rewind ring buffer: savestates of the recent past within a memory
//! budget.
//!
//! The newest state is kept whole; every older one only as the difference
//! to its successor (the bytes XORed, zero runs collapsed). Consecutive
//! frames change little, so a frame of a 2 MB Amiga typically costs a few
//! kilobytes. Dropping the oldest entry never touches the others, and going
//! back `n` entries applies `n` differences to the newest state.
//!
//! The buffer belongs to the frontend or debugger, never to machine state
//! (savestates stay independent of it); states are opaque bytes.

use std::collections::VecDeque;

use crate::cycle::Cycle;

/// One stored moment.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    at: Cycle,
    frame: u64,
    /// Whole state for the newest entry; for older ones the difference to
    /// the next newer entry (see [`diff`]).
    data: Vec<u8>,
}

/// Where and when an entry was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RewindPoint {
    /// Master cycles since power-on ([`crate::Machine::now`]).
    pub at: Cycle,
    /// Frames since power-on, for display.
    pub frame: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewindBuffer {
    budget: usize,
    bytes: usize,
    /// Oldest first.
    entries: VecDeque<Entry>,
}

impl RewindBuffer {
    /// An empty buffer that keeps at most about `budget` bytes.
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            entries: VecDeque::new(),
        }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Change the budget, dropping old entries if needed.
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.trim();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// The moments held, oldest first.
    pub fn points(&self) -> impl DoubleEndedIterator<Item = RewindPoint> + '_ {
        self.entries.iter().map(|e| RewindPoint {
            at: e.at,
            frame: e.frame,
        })
    }

    /// Add the state of moment `at`. A state taken at an older moment than
    /// the newest entry first drops the entries after it (the timeline
    /// went another way).
    pub fn push(&mut self, at: Cycle, frame: u64, state: Vec<u8>) {
        self.truncate_after(at);
        if let Some(newest) = self.entries.back_mut() {
            if newest.at == at {
                // Same moment again: replace it.
                self.bytes -= newest.data.len();
                self.bytes += state.len();
                newest.data = state;
                newest.frame = frame;
                self.trim();
                return;
            }
            let d = diff(&state, &newest.data);
            self.bytes -= newest.data.len();
            self.bytes += d.len();
            newest.data = d;
        }
        self.bytes += state.len();
        self.entries.push_back(Entry {
            at,
            frame,
            data: state,
        });
        self.trim();
    }

    /// The state of entry `back` counted from the newest (0 = newest).
    pub fn state(&self, back: usize) -> Option<(RewindPoint, Vec<u8>)> {
        let n = self.entries.len();
        if back >= n {
            return None;
        }
        let newest = self.entries.back()?;
        let mut state = newest.data.clone();
        for i in (n - 1 - back..n - 1).rev() {
            state = patch(&state, &self.entries[i].data)?;
        }
        let e = &self.entries[n - 1 - back];
        Some((
            RewindPoint {
                at: e.at,
                frame: e.frame,
            },
            state,
        ))
    }

    /// How many entries back (0 = newest) the newest entry taken strictly
    /// before `at` is, and not before `not_before`.
    pub fn before(&self, at: Cycle, not_before: Cycle) -> Option<usize> {
        self.entries
            .iter()
            .rev()
            .position(|e| e.at < at)
            .filter(|&b| self.entries[self.entries.len() - 1 - b].at >= not_before)
    }

    /// Go back to entry `back` (0 = newest): its state, with every newer
    /// entry dropped, so the buffer continues from there.
    pub fn rewind(&mut self, back: usize) -> Option<(RewindPoint, Vec<u8>)> {
        let (point, state) = self.state(back)?;
        let keep = self.entries.len() - back;
        while self.entries.len() > keep {
            let e = self.entries.pop_back().expect("len checked");
            self.bytes -= e.data.len();
        }
        let newest = self.entries.back_mut().expect("keep >= 1");
        self.bytes -= newest.data.len();
        self.bytes += state.len();
        newest.data = state.clone();
        Some((point, state))
    }

    fn truncate_after(&mut self, at: Cycle) {
        let keep = self.entries.iter().take_while(|e| e.at <= at).count();
        if keep < self.entries.len() {
            if keep == 0 {
                self.clear();
                return;
            }
            self.rewind(self.entries.len() - keep);
        }
    }

    /// Drop the oldest entries until within budget (the newest stays).
    fn trim(&mut self) {
        while self.bytes > self.budget && self.entries.len() > 1 {
            let e = self.entries.pop_front().expect("len > 1");
            self.bytes -= e.data.len();
        }
    }
}

/// Tags of a difference record.
const SAME_LEN: u8 = 0;
const WHOLE: u8 = 1;

/// The difference that turns `newer` back into `older`: for equal lengths
/// the XOR of both as runs (zero count, literal count, literals; counts as
/// LEB128), otherwise `older` whole.
pub fn diff(newer: &[u8], older: &[u8]) -> Vec<u8> {
    if newer.len() != older.len() {
        let mut out = Vec::with_capacity(older.len() + 1);
        out.push(WHOLE);
        out.extend_from_slice(older);
        return out;
    }
    let mut out = vec![SAME_LEN];
    let n = newer.len();
    let mut i = 0;
    while i < n {
        let start = i;
        while i < n && newer[i] == older[i] {
            i += 1;
        }
        let zeros = i - start;
        let lit_start = i;
        // A literal run ends at the next stretch of 4+ equal bytes.
        while i < n {
            if newer[i] == older[i] {
                let mut j = i;
                while j < n && j - i < 4 && newer[j] == older[j] {
                    j += 1;
                }
                if j - i >= 4 || j == n {
                    break;
                }
                i = j;
            } else {
                i += 1;
            }
        }
        leb128(&mut out, zeros);
        leb128(&mut out, i - lit_start);
        out.extend(
            newer[lit_start..i]
                .iter()
                .zip(&older[lit_start..i])
                .map(|(a, b)| a ^ b),
        );
    }
    out
}

/// Apply [`diff`]: `newer` back to the older state; `None` if `d` is
/// malformed.
pub fn patch(newer: &[u8], d: &[u8]) -> Option<Vec<u8>> {
    match *d.first()? {
        WHOLE => Some(d[1..].to_vec()),
        SAME_LEN => {
            let mut out = newer.to_vec();
            let mut pos = 1;
            let mut i = 0;
            while pos < d.len() {
                let zeros = read_leb128(d, &mut pos)?;
                let lits = read_leb128(d, &mut pos)?;
                i += zeros;
                let src = d.get(pos..pos + lits)?;
                let dst = out.get_mut(i..i + lits)?;
                for (o, x) in dst.iter_mut().zip(src) {
                    *o ^= x;
                }
                pos += lits;
                i += lits;
            }
            Some(out)
        }
        _ => None,
    }
}

fn leb128(out: &mut Vec<u8>, mut v: usize) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn read_leb128(d: &[u8], pos: &mut usize) -> Option<usize> {
    let mut v = 0usize;
    for shift in (0..64).step_by(7) {
        let b = *d.get(*pos)?;
        *pos += 1;
        v |= usize::from(b & 0x7f).checked_shl(shift)?;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(seed: u8) -> Vec<u8> {
        let mut s = vec![0u8; 4096];
        s[17] = seed;
        s[2000] = seed.wrapping_mul(3);
        s[4095] = !seed;
        s
    }

    #[test]
    fn diff_and_patch_round_trip() {
        let a = state(1);
        let b = state(2);
        let d = diff(&b, &a);
        assert!(d.len() < 32, "small change, small difference: {}", d.len());
        assert_eq!(patch(&b, &d).unwrap(), a);
        let short = vec![9u8; 10];
        assert_eq!(patch(&a, &diff(&a, &short)).unwrap(), short);
        assert_eq!(patch(&a, &diff(&a, &a)).unwrap(), a);
        assert!(patch(&a, &[SAME_LEN, 0xff]).is_none(), "truncated varint");
        assert!(
            patch(&a, &[SAME_LEN, 0, 50]).is_none(),
            "literals past the end"
        );
        assert!(patch(&a, &[7]).is_none());
    }

    #[test]
    fn states_come_back_and_the_budget_holds() {
        let mut r = RewindBuffer::new(1 << 20);
        for i in 0..10u8 {
            r.push(Cycle(u64::from(i) * 100), u64::from(i), state(i));
        }
        assert_eq!(r.len(), 10);
        assert!(r.bytes() < 4096 + 10 * 64, "{}", r.bytes());
        for back in 0..10 {
            let (p, s) = r.state(back).unwrap();
            assert_eq!(p.frame, 9 - back as u64);
            assert_eq!(s, state(9 - back as u8));
        }
        assert_eq!(r.before(Cycle(550), Cycle(0)), Some(4), "entry at 500");
        assert_eq!(r.before(Cycle(500), Cycle(0)), Some(5), "strictly before");
        assert_eq!(
            r.before(Cycle(550), Cycle(520)),
            None,
            "not before the barrier"
        );
        assert_eq!(r.before(Cycle(0), Cycle(0)), None);
        // Going back drops the newer entries.
        let (p, s) = r.rewind(3).unwrap();
        assert_eq!((p.at, s), (Cycle(600), state(6)));
        assert_eq!(r.len(), 7);
        assert_eq!(r.state(0).unwrap().1, state(6));
        assert_eq!(r.state(6).unwrap().1, state(0));
        // A push from an older moment cuts the future off.
        r.push(Cycle(250), 99, state(42));
        assert_eq!(r.len(), 4);
        assert_eq!(r.state(1).unwrap().1, state(2));
        // A tight budget keeps the newest entries.
        r.set_budget(4200);
        assert!(r.bytes() <= 4200);
        assert_eq!(r.state(0).unwrap().1, state(42));
        r.set_budget(10);
        assert_eq!(r.len(), 1, "the newest stays");
    }
}
