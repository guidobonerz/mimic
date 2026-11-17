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

    /// When the next event is due, if any.
    pub fn next_at(&self) -> Option<Cycle> {
        self.events.front().map(|e| e.at)
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

/// Type `text` line by line with `typed` (a machine's own "key events for
/// this text from this cycle" function) and leave `pause` cycles after
/// every RETURN, the time an interpreter needs to take a program line in
/// before it reads the keyboard again. Returns the events and the cycle
/// after the last pause.
pub fn typed_lines(
    text: &str,
    start: Cycle,
    pause: u64,
    mut typed: impl FnMut(&str, u64) -> (Vec<TimedInput>, u64),
) -> (Vec<TimedInput>, Cycle) {
    let mut events = Vec::new();
    let mut t = start.0;
    for line in text.split_inclusive('\n') {
        let (ev, end) = typed(line, t);
        events.extend(ev);
        t = end.max(t);
        if line.ends_with('\n') {
            t += pause;
        }
    }
    (events, Cycle(t))
}

/// How long typed keys stay down and up, in the machine's cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeTiming {
    /// A key stays down this long (two keyboard scans or more).
    pub hold: u64,
    /// All keys are up this long before the next character.
    pub gap: u64,
    /// SHIFT and other modifiers go down this long before their key.
    pub lead: u64,
}

/// Key events that type `text` from cycle `start`, one character after
/// the other. `keys` names the keys of a character, modifiers first and
/// the key itself last; characters it has no keys for are skipped. Returns
/// the events and the cycle after the last gap.
pub fn typed_chars(
    text: &str,
    start: u64,
    timing: TypeTiming,
    keys: impl Fn(char) -> Option<Vec<u16>>,
) -> (Vec<TimedInput>, u64) {
    let event = |at: u64, key: u16, down: bool| TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(key),
            down,
        },
    };
    let mut events = Vec::new();
    let mut t = start;
    for c in text.chars() {
        let Some(list) = keys(c) else { continue };
        let Some((&key, modifiers)) = list.split_last() else {
            continue;
        };
        let lead = if modifiers.is_empty() { 0 } else { timing.lead };
        events.extend(modifiers.iter().map(|&m| event(t, m, true)));
        events.push(event(t + lead, key, true));
        events.push(event(t + lead + timing.hold, key, false));
        events.extend(
            modifiers
                .iter()
                .rev()
                .map(|&m| event(t + lead + timing.hold, m, false)),
        );
        t += lead + timing.hold + timing.gap;
    }
    (events, t)
}

/// Most characters a [`TextFeed`] holds; more is refused.
pub const MAX_FEED: usize = 256 * 1024;

/// Longest piece a [`TextFeed`] hands to the machine at once.
const FEED_PIECE: usize = 128;

/// Text waiting to be typed into a machine: pasted from the host's
/// clipboard or sent by a script. It lives on the host side (as the user's
/// fingers do) and hands the machine one line at a time through
/// [`Machine::typed_keys`](crate::Machine::typed_keys), so the machine's
/// input queue stays short, whatever the length of the text, and the rest
/// can be dropped at any time.
#[derive(Clone, Debug, Default)]
pub struct TextFeed {
    pieces: VecDeque<String>,
    chars: usize,
    /// The cycle the keys handed over so far end at, and how long they take.
    until: Option<(Cycle, u64)>,
    /// Calls in a row in which the system's keyboard buffer took nothing.
    refused: u32,
    /// The buffer took nothing for too long: the rest goes through the keys.
    keys_only: bool,
}

/// Calls of [`TextFeed::feed`] (frames) a full keyboard buffer is waited
/// for before the text is typed with keys instead: a program that does not
/// read the buffer must not hold the feed for ever.
const BUFFER_PATIENCE: u32 = 250;

impl TextFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add text behind what is waiting. Line ends (CR LF, CR, LF) become
    /// `\n`, tabs spaces. Returns the characters taken; 0 when the feed
    /// would grow over [`MAX_FEED`].
    pub fn push(&mut self, text: &str) -> usize {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let count = text.chars().count();
        if count == 0 || self.chars + count > MAX_FEED {
            return 0;
        }
        for line in text.split_inclusive('\n') {
            let mut piece = String::new();
            for c in line.chars() {
                piece.push(if c == '\t' { ' ' } else { c });
                if piece.chars().count() == FEED_PIECE {
                    self.pieces.push_back(std::mem::take(&mut piece));
                }
            }
            if !piece.is_empty() {
                self.pieces.push_back(piece);
            }
        }
        self.chars += count;
        count
    }

    /// Drop the text not yet handed to the machine.
    pub fn clear(&mut self) {
        self.pieces.clear();
        self.chars = 0;
        self.refused = 0;
        self.keys_only = false;
    }

    /// Characters not yet handed to the machine.
    pub fn pending(&self) -> usize {
        self.chars
    }

    /// Text is waiting, or keys handed over are still being typed at `now`.
    pub fn active(&self, now: Cycle) -> bool {
        !self.pieces.is_empty() || self.until.is_some_and(|(until, _)| now < until)
    }

    /// Keys handed over are still being typed at `now`.
    fn keys_down(&mut self, now: Cycle) -> bool {
        if let Some((until, span)) = self.until {
            // Further away than the keys take: the machine's time went
            // back (a savestate, a rewind); those keys are gone.
            if now < until && until.0 - now.0 <= span {
                return true;
            }
            self.until = None;
        }
        false
    }

    /// Call before every frame: the next characters go straight into the
    /// system's keyboard buffer where the machine has one
    /// ([`Machine::paste_chars`](crate::Machine::paste_chars)), many times
    /// faster than keys; otherwise as [`TextFeed::pump`], whose events it
    /// then returns. A buffer that takes nothing for some seconds sends
    /// the rest of the text through the keys.
    pub fn feed(&mut self, machine: &mut dyn crate::Machine) -> Vec<TimedInput> {
        if self.keys_only || self.keys_down(machine.now()) {
            return self.pump(machine);
        }
        if self.pieces.is_empty() {
            return Vec::new();
        }
        // The next lines in one go: a buffer holds more than a short line.
        let mut text = String::new();
        for piece in &self.pieces {
            text.push_str(piece);
            if text.len() >= FEED_PIECE {
                break;
            }
        }
        match machine.paste_chars(&text) {
            None => self.pump(machine),
            Some(0) => {
                self.refused += 1;
                if self.refused < BUFFER_PATIENCE {
                    return Vec::new();
                }
                self.keys_only = true;
                self.pump(machine)
            }
            Some(taken) => {
                self.refused = 0;
                let mut left = taken;
                while left > 0 {
                    let Some(piece) = self.pieces.front_mut() else {
                        break;
                    };
                    let count = piece.chars().count();
                    if left >= count {
                        self.pieces.pop_front();
                        self.chars = self.chars.saturating_sub(count);
                        left -= count;
                    } else {
                        let bytes = piece.char_indices().nth(left).map_or(0, |(at, _)| at);
                        piece.drain(..bytes);
                        self.chars = self.chars.saturating_sub(left);
                        left = 0;
                    }
                }
                Vec::new()
            }
        }
    }

    /// Call before every frame: once the machine has typed what it was
    /// given, the key events of the next piece, to be queued with
    /// [`Machine::input`](crate::Machine::input). A machine that cannot
    /// type drops the text. Keys only, for recorded input;
    /// [`TextFeed::feed`] is the fast way.
    pub fn pump(&mut self, machine: &dyn crate::Machine) -> Vec<TimedInput> {
        let now = machine.now();
        if self.keys_down(now) {
            return Vec::new();
        }
        let Some(piece) = self.pieces.pop_front() else {
            self.keys_only = false;
            return Vec::new();
        };
        self.chars = self.chars.saturating_sub(piece.chars().count());
        match machine.typed_keys(&piece, now) {
            Some((events, end)) => {
                self.until = Some((end, end.0.saturating_sub(now.0)));
                events
            }
            None => {
                self.clear();
                Vec::new()
            }
        }
    }
}

/// Fill an operating system's keyboard buffer for
/// [`Machine::paste_chars`](crate::Machine::paste_chars): up to `room`
/// characters off the front of `text` go to `put` as the codes `code`
/// gives them, characters without a code are dropped. Returns the
/// characters of `text` used up, dropped ones included.
pub fn fill_key_buffer(
    text: &str,
    room: usize,
    code: impl Fn(char) -> Option<u8>,
    mut put: impl FnMut(usize, u8),
) -> usize {
    let (mut used, mut stored) = (0, 0);
    for c in text.chars() {
        if let Some(b) = code(c) {
            if stored == room {
                break;
            }
            put(stored, b);
            stored += 1;
        }
        used += 1;
    }
    used
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
    fn typed_lines_pause_after_every_return() {
        let (events, end) = typed_lines("AB\nC", Cycle(100), 50, |line, from| {
            let n = line.chars().count() as u64;
            (
                (0..n).map(|i| key(from + i * 10, 1)).collect(),
                from + n * 10,
            )
        });
        let at: Vec<u64> = events.iter().map(|e| e.at.0).collect();
        // The first line with its RETURN takes 30 cycles, then the pause.
        assert_eq!(at, [100, 110, 120, 180]);
        assert_eq!(end, Cycle(190));
    }

    #[test]
    fn a_feed_hands_the_text_over_line_by_line() {
        use crate::machine::Machine;
        use crate::testing::basic::ToyBasic;
        let mut m = ToyBasic::new(&[]);
        let mut feed = TextFeed::new();
        assert_eq!(feed.push("10 A\r\n20\tB\rRUN"), 13);
        assert_eq!(feed.pending(), 13);
        assert!(feed.active(m.now()));
        // The first line at once, the second only when the first is typed.
        let first = feed.pump(&m);
        assert_eq!(first.len(), 2 * 5);
        assert!(feed.pump(&m).is_empty());
        assert_eq!(feed.pending(), 8);
        for e in first {
            m.input(e);
        }
        let mut dbg = crate::debug::DebugHooks::new();
        let mut frames = 0;
        while feed.active(m.now()) {
            for e in feed.pump(&m) {
                m.input(e);
            }
            m.run_frame(&mut dbg);
            frames += 1;
            assert!(frames < 100);
        }
        assert_eq!(m.typed(), "10 A\n20 B\nRUN");
        assert_eq!(feed.pending(), 0);

        // Too much is refused whole; clearing drops what waits.
        assert_eq!(feed.push(&"x".repeat(MAX_FEED + 1)), 0);
        assert_eq!(feed.push("A\nB\n"), 4);
        feed.clear();
        assert_eq!(feed.pending(), 0);
        assert!(feed.pump(&m).is_empty());
    }

    #[test]
    fn a_feed_fills_the_keyboard_buffer_where_there_is_one() {
        use crate::machine::Machine;
        use crate::testing::basic::ToyBasic;
        let mut m = ToyBasic::new(&[]);
        m.key_buffer = Some(4);
        let mut feed = TextFeed::new();
        feed.push("10 A\n20 B\n");
        // Four characters a call, no key events, across the line's end.
        assert!(feed.feed(&mut m).is_empty());
        assert_eq!((m.typed().as_str(), feed.pending()), ("10 A", 6));
        assert!(feed.feed(&mut m).is_empty());
        assert_eq!(m.typed(), "10 A\n20 ");
        assert!(feed.feed(&mut m).is_empty());
        assert_eq!((m.typed().as_str(), feed.pending()), ("10 A\n20 B\n", 0));
        assert!(!feed.active(m.now()));

        // A buffer that stays full: after a while the keys type the rest.
        m.key_buffer = Some(0);
        feed.push("C\n");
        for _ in 1..BUFFER_PATIENCE {
            assert!(feed.feed(&mut m).is_empty());
        }
        assert_eq!(feed.feed(&mut m).len(), 2 * 2);
        assert_eq!(feed.pending(), 0);
    }

    #[test]
    fn a_key_buffer_drops_what_has_no_code() {
        let mut got = Vec::new();
        let code = |c: char| c.is_ascii_uppercase().then_some(c as u8);
        let used = fill_key_buffer("AbC-DE", 3, code, |i, b| got.push((i, b)));
        // "b" and "-" are used up without a place in the buffer.
        assert_eq!(used, 5);
        assert_eq!(got, [(0, b'A'), (1, b'C'), (2, b'D')]);
        assert_eq!(fill_key_buffer("abc", 0, code, |_, _| unreachable!()), 3);
    }

    #[test]
    fn a_feed_goes_on_after_the_machine_went_back_in_time() {
        use crate::machine::Machine;
        use crate::testing::basic::ToyBasic;
        let mut late = ToyBasic::new(&[]);
        let mut dbg = crate::debug::DebugHooks::new();
        for _ in 0..50 {
            late.run_frame(&mut dbg);
        }
        let mut feed = TextFeed::new();
        feed.push("A\nB\n");
        assert!(!feed.pump(&late).is_empty());
        // The same feed on a machine at cycle 0: the keys of the first
        // line are far in its future, so it does not wait for them.
        let early = ToyBasic::new(&[]);
        assert!(!feed.pump(&early).is_empty());
    }

    #[test]
    fn dirs_combine() {
        let d = Dirs::UP | Dirs::LEFT;
        assert!(d.contains(Dirs::UP) && d.contains(Dirs::LEFT));
        assert!(!d.contains(Dirs::DOWN));
        assert!(d.contains(Dirs::NONE));
    }
}
