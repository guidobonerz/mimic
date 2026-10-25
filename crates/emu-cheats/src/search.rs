//! Cheat search (memory finder): a snapshot of candidate addresses
//! narrowed step by step (CLAUDE.md "Cheat search"). Works on side-effect
//! free peeks between frames; it never halts or slows emulation beyond the
//! copy.

use emu_core::{MemSpaceId, Size};

/// Most addresses one search may cover.
pub const MAX_SEARCH: u32 = 1 << 24;

/// How candidates are narrowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq(u32),
    Ne(u32),
    Lt(u32),
    Gt(u32),
    Increased,
    Decreased,
    Unchanged,
    Changed,
    IncreasedBy(u32),
    DecreasedBy(u32),
}

impl Op {
    /// By its protocol name (`eq`, `increased_by` …) with its value.
    pub fn parse(name: &str, value: Option<u32>) -> Result<Op, String> {
        let need = || value.ok_or_else(|| format!("'{name}' needs a value"));
        Ok(match name {
            "eq" => Op::Eq(need()?),
            "ne" => Op::Ne(need()?),
            "lt" => Op::Lt(need()?),
            "gt" => Op::Gt(need()?),
            "increased" => Op::Increased,
            "decreased" => Op::Decreased,
            "unchanged" => Op::Unchanged,
            "changed" => Op::Changed,
            "increased_by" => Op::IncreasedBy(need()?),
            "decreased_by" => Op::DecreasedBy(need()?),
            other => return Err(format!("unknown search operation '{other}'")),
        })
    }

    fn keeps(self, old: u32, now: u32, mask: u32) -> bool {
        match self {
            Op::Eq(v) => now == v,
            Op::Ne(v) => now != v,
            Op::Lt(v) => now < v,
            Op::Gt(v) => now > v,
            Op::Increased => now > old,
            Op::Decreased => now < old,
            Op::Unchanged => now == old,
            Op::Changed => now != old,
            Op::IncreasedBy(d) => now == old.wrapping_add(d) & mask,
            Op::DecreasedBy(d) => now == old.wrapping_sub(d) & mask,
        }
    }
}

/// A search in progress: candidate addresses with their last values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub space: MemSpaceId,
    pub size: Size,
    pub candidates: Vec<(u32, u32)>,
}

impl Search {
    /// Snapshot `len` bytes from `addr` in steps of `size`; `peek` reads a
    /// value (None: not readable, skipped).
    pub fn start(
        space: MemSpaceId,
        size: Size,
        addr: u32,
        len: u32,
        mut peek: impl FnMut(u32) -> Option<u32>,
    ) -> Result<Self, String> {
        if len == 0 || len > MAX_SEARCH {
            return Err(format!("len must be 1–{MAX_SEARCH:#x}"));
        }
        let step = size.bytes();
        let mut candidates = Vec::new();
        let mut off = 0;
        while off + step <= len {
            let a = addr.wrapping_add(off);
            if let Some(v) = peek(a) {
                candidates.push((a, v));
            }
            off += step;
        }
        Ok(Self {
            space,
            size,
            candidates,
        })
    }

    /// Keep the candidates for which `op` holds against their last value;
    /// remember the new values. The number left.
    pub fn next(&mut self, op: Op, mut peek: impl FnMut(u32) -> Option<u32>) -> usize {
        let mask = self.size.mask();
        self.candidates.retain_mut(|(addr, last)| {
            let Some(now) = peek(*addr) else {
                return false;
            };
            let kept = op.keeps(*last, now, mask);
            *last = now;
            kept
        });
        self.candidates.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrows_to_the_counter() {
        let mut mem = [5u8, 5, 9, 5];
        let mut s = Search::start(MemSpaceId(0), Size::Byte, 0, 4, |a| {
            Some(u32::from(mem[a as usize]))
        })
        .unwrap();
        assert_eq!(s.candidates.len(), 4);
        mem[1] = 4; // lives lost
        mem[3] = 6;
        let n = s.next(Op::Decreased, |a| Some(u32::from(mem[a as usize])));
        assert_eq!(n, 1);
        assert_eq!(s.candidates, [(1, 4)]);
        assert!(Op::parse("eq", None).is_err());
        assert_eq!(
            Op::parse("decreased_by", Some(1)).unwrap(),
            Op::DecreasedBy(1)
        );
        assert!(Search::start(MemSpaceId(0), Size::Byte, 0, 0, |_| None).is_err());
    }
}
