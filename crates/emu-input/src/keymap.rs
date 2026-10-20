//! Positional keymaps: host key positions (W3C `KeyboardEvent.code` names,
//! as delivered by browsers and mapped from winit) to machine keys.
//!
//! A host key may produce several machine keys (C64 cursor up = SHIFT +
//! CRSR DOWN). Machine key codes follow each machine's documented encoding;
//! for the C64 and VIC-20 that is `column * 8 + row` of the keyboard matrix,
//! RESTORE 64 (see `machine-c64::keyboard`, `machine-vic20::keyboard`).

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

const VIC20_LSHIFT: u16 = k(3, 1);
const VIC20_RSHIFT: u16 = k(4, 6);

/// VIC-20 keyboard (same layout as the C64, other matrix) on the same PC
/// keys as [`C64_POSITIONAL`].
const VIC20_POSITIONAL: &[(&str, &[u16])] = &[
    ("Backquote", &[k(1, 0)]), // ←
    ("Digit1", &[k(0, 0)]),
    ("Digit2", &[k(7, 0)]),
    ("Digit3", &[k(0, 1)]),
    ("Digit4", &[k(7, 1)]),
    ("Digit5", &[k(0, 2)]),
    ("Digit6", &[k(7, 2)]),
    ("Digit7", &[k(0, 3)]),
    ("Digit8", &[k(7, 3)]),
    ("Digit9", &[k(0, 4)]),
    ("Digit0", &[k(7, 4)]),
    ("Minus", &[k(0, 5)]),  // +
    ("Equal", &[k(7, 5)]),  // -
    ("Insert", &[k(0, 6)]), // £
    ("Home", &[k(7, 6)]),
    ("Backspace", &[k(0, 7)]), // INST/DEL
    ("Tab", &[k(2, 0)]),       // CTRL
    ("KeyQ", &[k(6, 0)]),
    ("KeyW", &[k(1, 1)]),
    ("KeyE", &[k(6, 1)]),
    ("KeyR", &[k(1, 2)]),
    ("KeyT", &[k(6, 2)]),
    ("KeyY", &[k(1, 3)]),
    ("KeyU", &[k(6, 3)]),
    ("KeyI", &[k(1, 4)]),
    ("KeyO", &[k(6, 4)]),
    ("KeyP", &[k(1, 5)]),
    ("BracketLeft", &[k(6, 5)]),   // @
    ("BracketRight", &[k(1, 6)]),  // *
    ("Delete", &[k(6, 6)]),        // ↑
    ("PageUp", &[64]),             // RESTORE
    ("Escape", &[k(3, 0)]),        // RUN/STOP
    ("CapsLock", &[VIC20_LSHIFT]), // SHIFT LOCK
    ("KeyA", &[k(2, 1)]),
    ("KeyS", &[k(5, 1)]),
    ("KeyD", &[k(2, 2)]),
    ("KeyF", &[k(5, 2)]),
    ("KeyG", &[k(2, 3)]),
    ("KeyH", &[k(5, 3)]),
    ("KeyJ", &[k(2, 4)]),
    ("KeyK", &[k(5, 4)]),
    ("KeyL", &[k(2, 5)]),
    ("Semicolon", &[k(5, 5)]), // :
    ("Quote", &[k(2, 6)]),     // ;
    ("Backslash", &[k(5, 6)]), // =
    ("Enter", &[k(1, 7)]),
    ("ControlLeft", &[k(5, 0)]), // C=
    ("ShiftLeft", &[VIC20_LSHIFT]),
    ("KeyZ", &[k(4, 1)]),
    ("KeyX", &[k(3, 2)]),
    ("KeyC", &[k(4, 2)]),
    ("KeyV", &[k(3, 3)]),
    ("KeyB", &[k(4, 3)]),
    ("KeyN", &[k(3, 4)]),
    ("KeyM", &[k(4, 4)]),
    ("Comma", &[k(3, 5)]),
    ("Period", &[k(4, 5)]),
    ("Slash", &[k(3, 6)]),
    ("ShiftRight", &[VIC20_RSHIFT]),
    ("ArrowDown", &[k(3, 7)]),
    ("ArrowRight", &[k(2, 7)]),
    ("ArrowUp", &[VIC20_RSHIFT, k(3, 7)]),
    ("ArrowLeft", &[VIC20_RSHIFT, k(2, 7)]),
    ("Space", &[k(4, 0)]),
    ("F1", &[k(4, 7)]),
    ("F2", &[VIC20_LSHIFT, k(4, 7)]),
    ("F3", &[k(5, 7)]),
    ("F4", &[VIC20_LSHIFT, k(5, 7)]),
    ("F5", &[k(6, 7)]),
    ("F6", &[VIC20_LSHIFT, k(6, 7)]),
    ("F7", &[k(7, 7)]),
    ("F8", &[VIC20_LSHIFT, k(7, 7)]),
];

pub static VIC20: Keymap = Keymap {
    machine: "vic20",
    entries: VIC20_POSITIONAL,
};

/// The positional keymap for a machine id.
pub fn positional(machine: &str) -> Option<&'static Keymap> {
    match machine {
        "c64" => Some(&C64),
        "vic20" => Some(&VIC20),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Entries = &'static [(&'static str, &'static [u16])];

    const MAPS: [(Entries, u16); 2] = [
        (C64_POSITIONAL, C64_LSHIFT),
        (VIC20_POSITIONAL, VIC20_LSHIFT),
    ];

    #[test]
    fn every_matrix_key_is_reachable_once_as_a_single_key() {
        for (map, lshift) in MAPS {
            let mut seen = [0u8; 65];
            for (_, keys) in map {
                if let [single] = keys {
                    seen[usize::from(*single)] += 1;
                }
            }
            // LSHIFT is on both ShiftLeft and CapsLock (SHIFT LOCK).
            seen[usize::from(lshift)] -= 1;
            assert!(seen.iter().all(|&n| n == 1), "{seen:?}");
        }
    }

    #[test]
    fn host_names_are_unique() {
        for (map, _) in MAPS {
            let mut names: Vec<_> = map.iter().map(|(n, _)| *n).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), map.len());
        }
    }

    #[test]
    fn both_layouts_use_the_same_host_keys() {
        let names = |map: Entries| {
            let mut n: Vec<_> = map.iter().map(|(n, _)| *n).collect();
            n.sort_unstable();
            n
        };
        assert_eq!(names(C64_POSITIONAL), names(VIC20_POSITIONAL));
    }

    #[test]
    fn cursor_up_is_shift_plus_down() {
        let keys: Vec<_> = C64.lookup("ArrowUp").unwrap().collect();
        assert_eq!(keys, [MachineKey(C64_RSHIFT), MachineKey(k(0, 7))]);
        assert!(C64.lookup("NumLock").is_none());
    }
}
