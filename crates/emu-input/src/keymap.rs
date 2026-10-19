//! Positional keymaps: host key positions (W3C `KeyboardEvent.code` names,
//! as delivered by browsers and mapped from winit) to machine keys.
//!
//! A host key may produce several machine keys (C64 cursor up = SHIFT +
//! CRSR DOWN). Machine key codes follow each machine's documented encoding;
//! for the C64 that is `column * 8 + row` of the keyboard matrix, RESTORE 64
//! (see `machine-c64::keyboard`).

use emu_core::input::MachineKey;

pub struct Keymap {
    pub machine: &'static str,
    entries: &'static [(&'static str, &'static [u16])],
}

impl Keymap {
    /// Machine keys for a host key, if mapped.
    pub fn lookup(&self, code: &str) -> Option<impl Iterator<Item = MachineKey>> {
        self.entries
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, keys)| keys.iter().map(|&k| MachineKey(k)))
    }
}

const fn k(col: u16, row: u16) -> u16 {
    col * 8 + row
}

const C64_LSHIFT: u16 = k(1, 7);
const C64_RSHIFT: u16 = k(6, 4);

/// C64 keyboard laid onto a PC (US) keyboard by position. Keys the PC lacks
/// in that position: £ on Insert, ↑ on Delete, RESTORE on Page Up, = on the
/// backslash key.
const C64_POSITIONAL: &[(&str, &[u16])] = &[
    ("Backquote", &[k(7, 1)]), // ←
    ("Digit1", &[k(7, 0)]),
    ("Digit2", &[k(7, 3)]),
    ("Digit3", &[k(1, 0)]),
    ("Digit4", &[k(1, 3)]),
    ("Digit5", &[k(2, 0)]),
    ("Digit6", &[k(2, 3)]),
    ("Digit7", &[k(3, 0)]),
    ("Digit8", &[k(3, 3)]),
    ("Digit9", &[k(4, 0)]),
    ("Digit0", &[k(4, 3)]),
    ("Minus", &[k(5, 0)]),  // +
    ("Equal", &[k(5, 3)]),  // -
    ("Insert", &[k(6, 0)]), // £
    ("Home", &[k(6, 3)]),
    ("Backspace", &[k(0, 0)]), // INST/DEL
    ("Tab", &[k(7, 2)]),       // CTRL
    ("KeyQ", &[k(7, 6)]),
    ("KeyW", &[k(1, 1)]),
    ("KeyE", &[k(1, 6)]),
    ("KeyR", &[k(2, 1)]),
    ("KeyT", &[k(2, 6)]),
    ("KeyY", &[k(3, 1)]),
    ("KeyU", &[k(3, 6)]),
    ("KeyI", &[k(4, 1)]),
    ("KeyO", &[k(4, 6)]),
    ("KeyP", &[k(5, 1)]),
    ("BracketLeft", &[k(5, 6)]),  // @
    ("BracketRight", &[k(6, 1)]), // *
    ("Delete", &[k(6, 6)]),       // ↑
    ("PageUp", &[64]),            // RESTORE
    ("Escape", &[k(7, 7)]),       // RUN/STOP
    ("CapsLock", &[C64_LSHIFT]),  // SHIFT LOCK
    ("KeyA", &[k(1, 2)]),
    ("KeyS", &[k(1, 5)]),
    ("KeyD", &[k(2, 2)]),
    ("KeyF", &[k(2, 5)]),
    ("KeyG", &[k(3, 2)]),
    ("KeyH", &[k(3, 5)]),
    ("KeyJ", &[k(4, 2)]),
    ("KeyK", &[k(4, 5)]),
    ("KeyL", &[k(5, 2)]),
    ("Semicolon", &[k(5, 5)]), // :
    ("Quote", &[k(6, 2)]),     // ;
    ("Backslash", &[k(6, 5)]), // =
    ("Enter", &[k(0, 1)]),
    ("ControlLeft", &[k(7, 5)]), // C=
    ("ShiftLeft", &[C64_LSHIFT]),
    ("KeyZ", &[k(1, 4)]),
    ("KeyX", &[k(2, 7)]),
    ("KeyC", &[k(2, 4)]),
    ("KeyV", &[k(3, 7)]),
    ("KeyB", &[k(3, 4)]),
    ("KeyN", &[k(4, 7)]),
    ("KeyM", &[k(4, 4)]),
    ("Comma", &[k(5, 7)]),
    ("Period", &[k(5, 4)]),
    ("Slash", &[k(6, 7)]),
    ("ShiftRight", &[C64_RSHIFT]),
    ("ArrowDown", &[k(0, 7)]),
    ("ArrowRight", &[k(0, 2)]),
    ("ArrowUp", &[C64_RSHIFT, k(0, 7)]),
    ("ArrowLeft", &[C64_RSHIFT, k(0, 2)]),
    ("Space", &[k(7, 4)]),
    ("F1", &[k(0, 4)]),
    ("F2", &[C64_LSHIFT, k(0, 4)]),
    ("F3", &[k(0, 5)]),
    ("F4", &[C64_LSHIFT, k(0, 5)]),
    ("F5", &[k(0, 6)]),
    ("F6", &[C64_LSHIFT, k(0, 6)]),
    ("F7", &[k(0, 3)]),
    ("F8", &[C64_LSHIFT, k(0, 3)]),
];

pub static C64: Keymap = Keymap {
    machine: "c64",
    entries: C64_POSITIONAL,
};

/// The positional keymap for a machine id.
pub fn positional(machine: &str) -> Option<&'static Keymap> {
    match machine {
        "c64" => Some(&C64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_matrix_key_is_reachable_once_as_a_single_key() {
        let mut seen = [0u8; 65];
        for (_, keys) in C64_POSITIONAL {
            if let [single] = keys {
                seen[usize::from(*single)] += 1;
            }
        }
        // LSHIFT is on both ShiftLeft and CapsLock (SHIFT LOCK).
        seen[usize::from(C64_LSHIFT)] -= 1;
        assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
    }

    #[test]
    fn host_names_are_unique() {
        let mut names: Vec<_> = C64_POSITIONAL.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), C64_POSITIONAL.len());
    }

    #[test]
    fn cursor_up_is_shift_plus_down() {
        let keys: Vec<_> = C64.lookup("ArrowUp").unwrap().collect();
        assert_eq!(keys, [MachineKey(C64_RSHIFT), MachineKey(k(0, 7))]);
        assert!(C64.lookup("NumLock").is_none());
    }
}
