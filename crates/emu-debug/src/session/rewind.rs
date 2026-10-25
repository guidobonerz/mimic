//! Rewind (`rewind_config`, `rewind_info`, `rewind`) and stepping backwards
//! (`step_back`).
//!
//! While running, the session keeps a savestate every `interval` frames in
//! a [`RewindBuffer`]. `step_back` goes back by instructions: emulation is
//! deterministic (golden rule 3), so it restores the newest state before
//! now and runs forward again to the wanted instruction. Changes made from
//! outside while paused (poke, registers, input, media, freezes, loading a
//! state) are not in any older state, so they set a barrier `step_back`
//! does not cross; a state taken right after them (the anchor) lets it
//! still step back to that point.

use serde_json::{json, Value};

use emu_core::rewind::RewindBuffer;
use emu_core::{Cycle, StopReason};

use super::Session;

/// Default memory budget of the ring when switched on without one.
pub const DEFAULT_BUDGET_MB: u32 = 128;
/// Most instructions `step_back` replays to find its target.
const MAX_REPLAY_INSTRUCTIONS: usize = 20_000_000;

#[derive(Debug, Default)]
pub(super) struct Rewind {
    buf: Option<RewindBuffer>,
    /// Frames between two states (≥ 1 while on).
    interval: u32,
    countdown: u32,
    /// A state taken while paused (after a change from outside, or before
    /// the first step), so `step_back` works without the ring too.
    anchor: Option<(Cycle, Vec<u8>)>,
    /// The anchor lies between two instructions of the main CPU, so it is
    /// a place `step_back` may stop at.
    anchor_on_boundary: bool,
    /// The machine stands between two instructions now (after `step_instr`
    /// or `step_back`); other stops may be inside one.
    pub(super) on_boundary: bool,
    /// The latest change from outside: older states do not contain it.
    barrier: Cycle,
}

impl Session {
    /// Machine state and freezes in one blob.
    fn pack(&self) -> Vec<u8> {
        let m = self.machine.save_state();
        let f = self.freeze_state();
        let mut out = Vec::with_capacity(4 + m.len() + f.len());
        out.extend_from_slice(&(m.len() as u32).to_le_bytes());
        out.extend_from_slice(&m);
        out.extend_from_slice(&f);
        out
    }

    fn unpack(&mut self, blob: &[u8]) -> Result<(), String> {
        let len = blob
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or("rewind state truncated")?;
        let m = blob.get(4..4 + len).ok_or("rewind state truncated")?;
        self.machine
            .load_state(m)
            .map_err(|e| format!("rewind state rejected: {e:?}"))?;
        self.load_freeze_state(&blob[4 + len..])
    }

    fn frame_no(&self) -> u64 {
        self.machine.beam().map_or(0, |b| b.frame)
    }

    /// After every emulated frame while running.
    pub(super) fn rewind_frame_done(&mut self) {
        let Some(interval) = self.rewind.buf.as_ref().map(|_| self.rewind.interval) else {
            return;
        };
        self.rewind.countdown = self.rewind.countdown.saturating_sub(1);
        if self.rewind.countdown > 0 {
            return;
        }
        self.rewind.countdown = interval;
        let (at, frame, state) = (self.machine.now(), self.frame_no(), self.pack());
        if let Some(buf) = &mut self.rewind.buf {
            buf.push(at, frame, state);
        }
    }

    /// Something outside the emulation changed the machine (or its
    /// freezes, input, media) now: older states lack it.
    pub(super) fn mutated(&mut self) {
        let now = self.machine.now();
        self.rewind.barrier = now;
        self.rewind.anchor = Some((now, self.pack()));
        self.rewind.anchor_on_boundary = self.rewind.on_boundary;
    }

    /// Before a synchronous step: make sure `step_back` has a state to
    /// start from.
    pub(super) fn anchor_before_step(&mut self) {
        let now = self.machine.now();
        let have = self
            .rewind
            .anchor
            .as_ref()
            .is_some_and(|(at, _)| *at <= now)
            || self
                .rewind
                .buf
                .as_ref()
                .is_some_and(|b| b.before(Cycle(now.0 + 1), self.rewind.barrier).is_some());
        if !have {
            self.rewind.anchor = Some((now, self.pack()));
            self.rewind.anchor_on_boundary = self.rewind.on_boundary;
        }
    }

    /// Switch the ring on (`frames` ≥ 1) or off (0) and/or change its budget.
    pub fn rewind_config(&mut self, frames: Option<u32>, budget_mb: Option<u32>) -> Value {
        let budget = |mb: u32| mb.max(1) as usize * 1024 * 1024;
        match frames {
            Some(0) => self.rewind.buf = None,
            Some(n) => {
                self.rewind.interval = n;
                self.rewind.countdown = n;
                if self.rewind.buf.is_none() {
                    let mb = budget_mb.unwrap_or(DEFAULT_BUDGET_MB);
                    self.rewind.buf = Some(RewindBuffer::new(budget(mb)));
                }
            }
            None => {}
        }
        if let (Some(mb), Some(buf)) = (budget_mb, &mut self.rewind.buf) {
            buf.set_budget(budget(mb));
        }
        self.rewind_info()
    }

    pub fn rewind_info(&self) -> Value {
        match &self.rewind.buf {
            None => json!({"enabled": false}),
            Some(b) => {
                let point = |p: Option<emu_core::rewind::RewindPoint>| {
                    p.map(|p| json!({"cycle": p.at.0, "frame": p.frame}))
                };
                json!({
                    "enabled": true,
                    "interval": self.rewind.interval,
                    "budget": b.budget(),
                    "bytes": b.bytes(),
                    "entries": b.len(),
                    "oldest": point(b.points().next()),
                    "newest": point(b.points().next_back()),
                })
            }
        }
    }

    /// Go back `count` states of the ring (1 = the newest before now) and
    /// stay paused there.
    pub fn rewind_back(&mut self, count: u32) -> Result<Value, String> {
        let count = count.max(1) as usize;
        self.stop_for_sync();
        let now = self.machine.now();
        let buf = self
            .rewind
            .buf
            .as_mut()
            .ok_or("rewind is off (rewind_config)")?;
        let first = buf.before(now, Cycle(0)).ok_or("no earlier state")?;
        let back = (first + count - 1).min(buf.len() - 1);
        let (point, state) = buf.rewind(back).ok_or("no earlier state")?;
        self.unpack(&state)?;
        self.rewind.anchor = Some((point.at, state));
        self.rewind.anchor_on_boundary = false;
        self.rewind.barrier = self.rewind.barrier.min(point.at);
        let mut v = self.state_json(&StopReason::StepDone);
        v["frame"] = json!(point.frame);
        v["cycle"] = json!(point.at.0);
        Ok(v)
    }

    /// Run instructions with the session's freezes but no breakpoints,
    /// trace or stepping state of its own; `on_step` sees `now` after each
    /// and returns false to stop.
    fn replay(&mut self, mut on_step: impl FnMut(Cycle) -> bool) -> Result<(), String> {
        let mut hooks = self.hooks.clone();
        hooks.clear_breakpoints();
        hooks.set_trace(emu_core::TraceConfig::default());
        hooks.set_line_step(false);
        for _ in 0..MAX_REPLAY_INSTRUCTIONS {
            hooks.set_instr_step(true);
            match self.machine.run_frame(&mut hooks) {
                StopReason::FrameDone => continue,
                StopReason::StepDone => {
                    if !on_step(self.machine.now()) {
                        return Ok(());
                    }
                }
                other => return Err(format!("replay stopped: {other:?}")),
            }
        }
        Err("too far back to replay".into())
    }

    /// The main CPU `count` instructions back.
    pub fn step_back(&mut self, count: u32) -> Result<Value, String> {
        let count = count.max(1) as usize;
        self.stop_for_sync();
        let target = self.machine.now();
        let barrier = self.rewind.barrier;
        // The newest state before now: the anchor or one from the ring.
        let anchor = self
            .rewind
            .anchor
            .as_ref()
            .filter(|(at, _)| *at < target && *at >= barrier)
            .map(|(at, s)| (*at, s.clone(), self.rewind.anchor_on_boundary));
        let ring = self.rewind.buf.as_ref().and_then(|b| {
            let back = b.before(target, barrier)?;
            b.state(back).map(|(p, s)| (p.at, s, false))
        });
        let (start, state, start_on_boundary) = match (anchor, ring) {
            (Some(a), Some(r)) => {
                if a.0 >= r.0 {
                    a
                } else {
                    r
                }
            }
            (Some(a), None) => a,
            (None, Some(r)) => r,
            (None, None) => return Err("no earlier state to step back from".into()),
        };
        let current = self.pack();
        // Pass 1: instruction boundaries between that state and now.
        self.unpack(&state)?;
        // The start itself counts when it lies between two instructions.
        let mut bounds = if start_on_boundary {
            vec![start]
        } else {
            Vec::new()
        };
        let found = self.replay(|now| {
            if now < target {
                bounds.push(now);
                true
            } else {
                false
            }
        });
        if let Err(e) = found {
            self.unpack(&current)?;
            return Err(e);
        }
        if bounds.len() < count {
            self.unpack(&current)?;
            return Err(format!(
                "only {} instruction(s) back available (since cycle {})",
                bounds.len(),
                start.0
            ));
        }
        // Pass 2: run again to the wanted boundary.
        let want = bounds[bounds.len() - count];
        self.unpack(&state)?;
        if want > start {
            self.replay(|now| now < want)?;
        }
        self.rewind.on_boundary = true;
        let mut v = self.state_json(&StopReason::StepDone);
        v["cycle"] = json!(self.machine.now().0);
        Ok(v)
    }
}
