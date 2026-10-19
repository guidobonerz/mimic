//! Typing text into a C64 as timed key events (for tests and replays).

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;
use machine_c64::keyboard::{key_for_char, keys};

/// Cycles a key stays down, and the gap before the next one. The KERNAL
/// scans once per 1/60 s IRQ (~16 400 cycles); two scans per phase are safe.
const HOLD: u64 = 40_000;
const GAP: u64 = 40_000;

fn key(at: u64, k: MachineKey, down: bool) -> TimedInput {
    TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: k,
            down,
        },
    }
}

/// Key events that type `text` starting at cycle `start`. Returns the
/// events and the cycle after the last release. Characters without a key
/// are skipped.
pub fn type_text(text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    let mut events = Vec::new();
    let mut t = start;
    for c in text.chars() {
        let Some((k, shift)) = key_for_char(c) else {
            continue;
        };
        if shift {
            events.push(key(t, keys::LSHIFT, true));
        }
        events.push(key(t, k, true));
        events.push(key(t + HOLD, k, false));
        if shift {
            events.push(key(t + HOLD, keys::LSHIFT, false));
        }
        t += HOLD + GAP;
    }
    (events, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_characters_hold_shift() {
        let (ev, end) = type_text("A\"", 100);
        assert_eq!(ev.len(), 2 + 4);
        assert_eq!(end, 100 + 2 * (HOLD + GAP));
        assert!(matches!(
            ev[2].event,
            InputEvent::Key { machine_key, down: true } if machine_key == keys::LSHIFT
        ));
    }
}
