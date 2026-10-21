//! Host-neutral input events and the timestamped input queue.
//!
//! Events are applied at defined cycle positions, which keeps emulation
//! deterministic and makes input record/replay possible.

use std::collections::VecDeque;
use std::ops::BitOr;

use crate::controls::ControlId;
use crate::cycle::Cycle;
use crate::port::PortId;
use crate::savestate::{Savestate, StateError, StateReader, StateWriter};

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
    /// A console button or switch set to `value` (see
    /// [`crate::controls`]); goes to [`InputTarget::Panel`].
    Control {
        id: ControlId,
        value: u8,
    },
}

/// Where an event goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputTarget {
    Port(PortId),
    Keyboard,
    /// The console's buttons and switches.
    Panel,
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

    /// Drop all queued events (machine reset).
    pub fn clear(&mut self) {
        self.events.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Upper bound on queued events; protects savestate loading.
pub const MAX_QUEUED: usize = 1 << 16;

fn write_event(w: &mut StateWriter, e: &TimedInput) {
    w.u64(e.at.0);
    match e.target {
        InputTarget::Keyboard => w.u8(0xff),
        InputTarget::Panel => w.u8(0xfe),
        InputTarget::Port(p) => w.u8(p.0.min(0xfd)),
    }
    match e.event {
        InputEvent::Digital { dirs, buttons } => {
            w.u8(0);
            w.u8(dirs.0);
            w.u8(buttons);
        }
        InputEvent::Analog { axis, value } => {
            w.u8(1);
            w.u8(axis);
            w.u16(value);
        }
        InputEvent::MouseRel { dx, dy, buttons } => {
            w.u8(2);
            w.u16(dx as u16);
            w.u16(dy as u16);
            w.u8(buttons);
        }
        InputEvent::LightPen { x, y, trigger } => {
            w.u8(3);
            w.u16(x);
            w.u16(y);
            w.bool(trigger);
        }
        InputEvent::Key { machine_key, down } => {
            w.u8(4);
            w.u16(machine_key.0);
            w.bool(down);
        }
        InputEvent::Control { id, value } => {
            w.u8(5);
            w.u16(id.0);
            w.u8(value);
        }
    }
}

fn read_event(r: &mut StateReader<'_>) -> Result<TimedInput, StateError> {
    let at = Cycle(r.u64()?);
    let target = match r.u8()? {
        0xff => InputTarget::Keyboard,
        0xfe => InputTarget::Panel,
        p => InputTarget::Port(PortId(p)),
    };
    let event = match r.u8()? {
        0 => InputEvent::Digital {
            dirs: Dirs(r.u8()?),
            buttons: r.u8()?,
        },
        1 => InputEvent::Analog {
            axis: r.u8()?,
            value: r.u16()?,
        },
        2 => InputEvent::MouseRel {
            dx: r.u16()? as i16,
            dy: r.u16()? as i16,
            buttons: r.u8()?,
        },
        3 => InputEvent::LightPen {
            x: r.u16()?,
            y: r.u16()?,
            trigger: r.bool()?,
        },
        4 => InputEvent::Key {
            machine_key: MachineKey(r.u16()?),
            down: r.bool()?,
        },
        5 => InputEvent::Control {
            id: ControlId(r.u16()?),
            value: r.u8()?,
        },
        _ => return Err(StateError::Invalid("input event kind")),
    };
    Ok(TimedInput { at, target, event })
}

const QUEUE_TAG: [u8; 4] = *b"INPQ";
const QUEUE_VERSION: u16 = 1;

impl Savestate for InputQueue {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(QUEUE_TAG, QUEUE_VERSION, |w| {
            w.u32(self.events.len() as u32);
            for e in &self.events {
                write_event(w, e);
            }
        });
    }

    /// Atomic: on error the queue is unchanged.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(QUEUE_TAG, QUEUE_VERSION)?;
        let n = c.u32()? as usize;
        if n > MAX_QUEUED {
            return Err(StateError::Invalid("input queue length"));
        }
        let mut events = VecDeque::with_capacity(n);
        for _ in 0..n {
            events.push_back(read_event(&mut c)?);
        }
        if !events
            .iter()
            .zip(events.iter().skip(1))
            .all(|(a, b)| a.at <= b.at)
        {
            return Err(StateError::Invalid("input queue order"));
        }
        self.events = events;
        Ok(())
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
    fn queue_savestate_round_trip() {
        let mut q = InputQueue::new();
        q.push(key(5, 7));
        q.push(TimedInput {
            at: Cycle(9),
            target: InputTarget::Port(PortId(1)),
            event: InputEvent::Digital {
                dirs: Dirs::UP | Dirs::RIGHT,
                buttons: 1,
            },
        });
        q.push(TimedInput {
            at: Cycle(9),
            target: InputTarget::Port(PortId(0)),
            event: InputEvent::MouseRel {
                dx: -3,
                dy: 4,
                buttons: 2,
            },
        });
        let mut w = StateWriter::new();
        q.save(&mut w);
        let data = w.into_bytes();
        let mut r = InputQueue::new();
        r.load(&mut StateReader::new(&data)).unwrap();
        let a: Vec<_> = std::iter::from_fn(|| q.pop_due(Cycle(100))).collect();
        let b: Vec<_> = std::iter::from_fn(|| r.pop_due(Cycle(100))).collect();
        assert_eq!(a, b);

        let mut keep = InputQueue::new();
        keep.push(key(1, 1));
        assert!(keep
            .load(&mut StateReader::new(&data[..data.len() - 1]))
            .is_err());
        assert_eq!(keep.len(), 1);
    }

    #[test]
    fn dirs_combine() {
        let d = Dirs::UP | Dirs::LEFT;
        assert!(d.contains(Dirs::UP) && d.contains(Dirs::LEFT));
        assert!(!d.contains(Dirs::DOWN));
        assert!(d.contains(Dirs::NONE));
    }
}
