//! Host-neutral input events and the timestamped input queue.
//!
//! Events are applied at defined cycle positions, which keeps emulation
//! deterministic and makes input record/replay possible.

use std::collections::VecDeque;
use std::ops::BitOr;

use crate::cycle::Cycle;
use crate::port::PortId;

/// Joystick directions as a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Dirs(pub u8);

impl Dirs {
    pub const NONE: Dirs = Dirs(0);
    pub const UP: Dirs = Dirs(1);
    pub const DOWN: Dirs = Dirs(2);
    pub const LEFT: Dirs = Dirs(4);
    pub const RIGHT: Dirs = Dirs(8);

    pub const fn contains(self, other: Dirs) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Dirs {
    type Output = Dirs;

    fn bitor(self, rhs: Dirs) -> Dirs {
        Dirs(self.0 | rhs.0)
    }
}

/// A key of the emulated keyboard: its matrix position or keycode as defined
/// by the machine's keymap, never a host keycode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MachineKey(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputEvent {
    /// Joystick.
    Digital {
        dirs: Dirs,
        buttons: u8,
    },
    /// Paddles / pots.
    Analog {
        axis: u8,
        value: u16,
    },
    MouseRel {
        dx: i16,
        dy: i16,
        buttons: u8,
    },
    LightPen {
        x: u16,
        y: u16,
        trigger: bool,
    },
    Key {
        machine_key: MachineKey,
        down: bool,
    },
}

/// Where an event goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputTarget {
    Port(PortId),
    Keyboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimedInput {
    pub at: Cycle,
    pub target: InputTarget,
    pub event: InputEvent,
}

/// Input events ordered by timestamp; events with equal timestamps keep their
/// insertion order.
#[derive(Clone, Debug, Default)]
pub struct InputQueue {
    events: VecDeque<TimedInput>,
}

impl InputQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, ev: TimedInput) {
        let index = self.events.partition_point(|e| e.at <= ev.at);
        self.events.insert(index, ev);
    }

    /// The next event due at or before `now`, if any.
    pub fn pop_due(&mut self, now: Cycle) -> Option<TimedInput> {
        if self.events.front().is_some_and(|e| e.at <= now) {
            self.events.pop_front()
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at: u64, code: u16) -> TimedInput {
        TimedInput {
            at: Cycle(at),
            target: InputTarget::Keyboard,
            event: InputEvent::Key {
                machine_key: MachineKey(code),
                down: true,
            },
        }
    }

    #[test]
    fn events_come_out_in_time_then_insertion_order() {
        let mut q = InputQueue::new();
        q.push(key(20, 1));
        q.push(key(10, 2));
        q.push(key(20, 3));
        q.push(key(10, 4));
        assert_eq!(q.pop_due(Cycle(5)), None);
        let order: Vec<_> = std::iter::from_fn(|| q.pop_due(Cycle(20))).collect();
        assert_eq!(order, vec![key(10, 2), key(10, 4), key(20, 1), key(20, 3)]);
        assert!(q.is_empty());
    }

    #[test]
    fn dirs_combine() {
        let d = Dirs::UP | Dirs::LEFT;
        assert!(d.contains(Dirs::UP) && d.contains(Dirs::LEFT));
        assert!(!d.contains(Dirs::DOWN));
        assert!(d.contains(Dirs::NONE));
    }
}
