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

/// C128 keyboard: the C64 layout ([`C64_POSITIONAL`], same matrix codes)
/// with the C128's own cursor keys on the arrows and its extra keys on F9
/// (ESC), F10 (HELP), F11 (TAB), End (LINE FEED), left Alt (ALT), Scroll
/// Lock (NO SCROLL), Num Lock (40/80 DISPLAY) and Caps Lock (CAPS LOCK).
/// The numeric keypad stays the joystick; the C128 keypad is not mapped.
const C128_POSITIONAL: &[(&str, &[u16])] = &[
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
    ("PageUp", &[88]),            // RESTORE
    ("Escape", &[k(7, 7)]),       // RUN/STOP
    ("CapsLock", &[90]),          // CAPS LOCK
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
    ("ArrowDown", &[k(10, 4)]),
    ("ArrowRight", &[k(10, 6)]),
    ("ArrowUp", &[k(10, 3)]),
    ("ArrowLeft", &[k(10, 5)]),
    ("Space", &[k(7, 4)]),
    ("F1", &[k(0, 4)]),
    ("F2", &[C64_LSHIFT, k(0, 4)]),
    ("F3", &[k(0, 5)]),
    ("F4", &[C64_LSHIFT, k(0, 5)]),
    ("F5", &[k(0, 6)]),
    ("F6", &[C64_LSHIFT, k(0, 6)]),
    ("F7", &[k(0, 3)]),
    ("F8", &[C64_LSHIFT, k(0, 3)]),
    ("F9", &[k(9, 0)]),          // ESC
    ("F10", &[k(8, 0)]),         // HELP
    ("F11", &[k(8, 3)]),         // TAB
    ("End", &[k(9, 3)]),         // LINE FEED
    ("AltLeft", &[k(10, 0)]),    // ALT
    ("ScrollLock", &[k(10, 7)]), // NO SCROLL
    ("NumLock", &[89]),          // 40/80 DISPLAY
];

pub static C128: Keymap = Keymap {
    machine: "c128",
    entries: C128_POSITIONAL,
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

const TED_SHIFT: u16 = k(1, 7);

/// C16/Plus/4 keyboard (`column * 8 + row`, see `machine-ted::keyboard`)
/// on the same PC keys as [`C64_POSITIONAL`]: ESC where the C64 has ←,
/// separate cursor keys, F1–F3 with F4–F6 shifted, HELP on F7.
const TED_POSITIONAL: &[(&str, &[u16])] = &[
    ("Backquote", &[k(6, 4)]), // ESC
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
    ("Minus", &[k(6, 6)]),     // +
    ("Equal", &[k(5, 6)]),     // -
    ("Backslash", &[k(6, 5)]), // =
    ("Insert", &[k(0, 2)]),    // £
    ("Home", &[k(7, 1)]),
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
    ("BracketLeft", &[k(0, 7)]),  // @
    ("BracketRight", &[k(6, 1)]), // *
    ("Escape", &[k(7, 7)]),       // RUN/STOP
    ("CapsLock", &[TED_SHIFT]),   // SHIFT LOCK
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
    ("Enter", &[k(0, 1)]),
    ("ControlLeft", &[k(7, 5)]), // C=
    ("ShiftLeft", &[TED_SHIFT]),
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
    ("ShiftRight", &[TED_SHIFT]),
    ("ArrowUp", &[k(5, 3)]),
    ("ArrowDown", &[k(5, 0)]),
    ("ArrowLeft", &[k(6, 0)]),
    ("ArrowRight", &[k(6, 3)]),
    ("Space", &[k(7, 4)]),
    ("F1", &[k(0, 4)]),
    ("F2", &[k(0, 5)]),
    ("F3", &[k(0, 6)]),
    ("F4", &[TED_SHIFT, k(0, 4)]),
    ("F5", &[TED_SHIFT, k(0, 5)]),
    ("F6", &[TED_SHIFT, k(0, 6)]),
    ("F7", &[k(0, 3)]), // HELP
    ("F8", &[TED_SHIFT, k(0, 3)]),
];

pub static TED: Keymap = Keymap {
    machine: "plus4",
    entries: TED_POSITIONAL,
};

/// PET graphics keyboard code (`row * 8 + column`).
const fn g(row: u16, col: u16) -> u16 {
    row * 8 + col
}

/// PET business keyboard code (`100 + row * 8 + column`).
const fn b(row: u16, col: u16) -> u16 {
    100 + row * 8 + col
}

/// PET keyboards by key label: each host key names the graphics and the
/// business key with that label (see `machine-pet::keyboard`); the machine
/// ignores the codes of the keyboard it does not have. Keypad keys map to
/// the PC keypad; graphics-only symbols without a PC key of their own are
/// on Page Up (@) and Page Down (?).
const PET_LABELLED: &[(&str, &[u16])] = &[
    ("Backquote", &[g(0, 5), b(9, 0)]), // ←
    ("Digit0", &[g(8, 6), b(1, 3)]),
    ("Digit1", &[g(6, 6), b(8, 7)]),
    ("Digit2", &[g(7, 6), b(7, 7)]),
    ("Digit3", &[g(6, 7), b(6, 7)]),
    ("Digit4", &[g(4, 6), b(5, 7)]),
    ("Digit5", &[g(5, 6), b(2, 7)]),
    ("Digit6", &[g(4, 7), b(3, 7)]),
    ("Digit7", &[g(2, 6), b(1, 4)]),
    ("Digit8", &[g(3, 6), b(0, 4)]),
    ("Digit9", &[g(2, 7), b(1, 7)]),
    ("Minus", &[g(8, 7), b(0, 3)]),     // -
    ("Equal", &[g(9, 7)]),              // =
    ("Backspace", &[g(1, 7), b(4, 7)]), // DEL
    ("Tab", &[b(4, 0)]),                // TAB
    ("Insert", &[g(9, 0), b(8, 0)]),    // RVS
    ("Home", &[g(0, 6), b(8, 4)]),      // HOME
    ("Delete", &[g(2, 5), b(1, 5)]),    // ↑
    ("Escape", &[g(9, 4), b(9, 4)]),    // STOP
    ("F1", &[b(2, 0)]),                 // ESC
    ("KeyA", &[g(4, 0), b(3, 0)]),
    ("KeyB", &[g(6, 2), b(6, 2)]),
    ("KeyC", &[g(6, 1), b(6, 1)]),
    ("KeyD", &[g(4, 1), b(3, 1)]),
    ("KeyE", &[g(2, 1), b(5, 1)]),
    ("KeyF", &[g(5, 1), b(2, 2)]),
    ("KeyG", &[g(4, 2), b(3, 2)]),
    ("KeyH", &[g(5, 2), b(2, 3)]),
    ("KeyI", &[g(3, 3), b(4, 5)]),
    ("KeyJ", &[g(4, 3), b(3, 3)]),
    ("KeyK", &[g(5, 3), b(2, 5)]),
    ("KeyL", &[g(4, 4), b(3, 5)]),
    ("KeyM", &[g(6, 3), b(8, 3)]),
    ("KeyN", &[g(7, 2), b(7, 2)]),
    ("KeyO", &[g(2, 4), b(5, 5)]),
    ("KeyP", &[g(3, 4), b(4, 6)]),
    ("KeyQ", &[g(2, 0), b(5, 0)]),
    ("KeyR", &[g(3, 1), b(4, 2)]),
    ("KeyS", &[g(5, 0), b(2, 1)]),
    ("KeyT", &[g(2, 2), b(5, 2)]),
    ("KeyU", &[g(2, 3), b(5, 3)]),
    ("KeyV", &[g(7, 1), b(7, 1)]),
    ("KeyW", &[g(3, 0), b(4, 1)]),
    ("KeyX", &[g(7, 0), b(8, 1)]),
    ("KeyY", &[g(3, 2), b(4, 3)]),
    ("KeyZ", &[g(6, 0), b(7, 0)]),
    ("BracketLeft", &[g(9, 1), b(5, 6)]),                 // [
    ("BracketRight", &[g(8, 2), b(2, 4)]),                // ]
    ("Backslash", &[g(1, 3), b(4, 4)]),                   // \
    ("Semicolon", &[g(6, 4), b(2, 6)]),                   // ;
    ("Quote", &[g(5, 4), b(9, 5)]),                       // :
    ("Comma", &[g(7, 3), b(7, 3)]),                       // ,
    ("Period", &[g(9, 6), b(6, 4)]),                      // .
    ("Slash", &[g(3, 7), b(8, 6)]),                       // /
    ("Enter", &[g(6, 5), b(3, 4)]),                       // RETURN
    ("NumpadEnter", &[g(6, 5), b(3, 4)]),                 // RETURN
    ("ShiftLeft", &[g(8, 0), b(6, 0)]),                   // LSHIFT
    ("ShiftRight", &[g(8, 5), b(6, 6)]),                  // RSHIFT
    ("Space", &[g(9, 2), b(8, 2)]),                       // SPACE
    ("ArrowDown", &[g(1, 6), b(5, 4)]),                   // CRSR_DOWN
    ("ArrowRight", &[g(0, 7), b(0, 5)]),                  // CRSR_RIGHT
    ("ArrowUp", &[g(8, 5), b(6, 6), g(1, 6), b(5, 4)]),   // SHIFT + CRSR DOWN
    ("ArrowLeft", &[g(8, 5), b(6, 6), g(0, 7), b(0, 5)]), // SHIFT + CRSR RIGHT
    ("Numpad0", &[g(8, 6), b(7, 4)]),
    ("Numpad1", &[g(6, 6), b(1, 0)]),
    ("Numpad2", &[g(7, 6), b(0, 0)]),
    ("Numpad3", &[g(6, 7), b(9, 1)]),
    ("Numpad4", &[g(4, 6), b(1, 1)]),
    ("Numpad5", &[g(5, 6), b(0, 1)]),
    ("Numpad6", &[g(4, 7), b(9, 2)]),
    ("Numpad7", &[g(2, 6), b(1, 2)]),
    ("Numpad8", &[g(3, 6), b(0, 2)]),
    ("Numpad9", &[g(2, 7), b(9, 3)]),
    ("NumpadDecimal", &[g(9, 6), b(6, 3)]),  // .
    ("NumpadSubtract", &[g(8, 7), b(0, 3)]), // -
    ("NumpadAdd", &[g(7, 7)]),               // +
    ("NumpadMultiply", &[g(5, 7)]),          // *
    ("NumpadDivide", &[g(3, 7), b(8, 6)]),   // /
    ("PageUp", &[g(8, 1), b(3, 6)]),         // @
    ("PageDown", &[g(7, 4)]),                // ?
];

pub static PET: Keymap = Keymap {
    machine: "pet",
    entries: PET_LABELLED,
};

/// Atari 800XL keys (POKEY key codes 0–63; SHIFT 64, CONTROL 65, BREAK
/// 66, START 67, SELECT 68, OPTION 69, RESET 70 — see
/// `machine-atari8::keyboard`).
const A_SHIFT: u16 = 64;
const A_CONTROL: u16 = 65;

/// Atari keyboard laid onto a PC (US) keyboard by position. The Atari's
/// `< >` sit on `- =`, its `- =` on `[ ]`, `+ *` on `' \`; the console
/// keys on F2 START, F3 SELECT, F4 OPTION, F5 RESET, F6 HELP, F7 and Pause
/// BREAK; the Atari (inverse) key on right Alt. The cursor keys are
/// CONTROL with `- = + *`, as on the machine.
const ATARI_POSITIONAL: &[(&str, &[u16])] = &[
    ("Escape", &[0x1c]),
    ("Backquote", &[0x1c]),
    ("Digit1", &[0x1f]),
    ("Digit2", &[0x1e]),
    ("Digit3", &[0x1a]),
    ("Digit4", &[0x18]),
    ("Digit5", &[0x1d]),
    ("Digit6", &[0x1b]),
    ("Digit7", &[0x33]),
    ("Digit8", &[0x35]),
    ("Digit9", &[0x30]),
    ("Digit0", &[0x32]),
    ("Minus", &[0x36]),     // <
    ("Equal", &[0x37]),     // >
    ("Backspace", &[0x34]), // DELETE/BACK S
    ("Tab", &[0x2c]),
    ("KeyQ", &[0x2f]),
    ("KeyW", &[0x2e]),
    ("KeyE", &[0x2a]),
    ("KeyR", &[0x28]),
    ("KeyT", &[0x2d]),
    ("KeyY", &[0x2b]),
    ("KeyU", &[0x0b]),
    ("KeyI", &[0x0d]),
    ("KeyO", &[0x08]),
    ("KeyP", &[0x0a]),
    ("BracketLeft", &[0x0e]),  // -
    ("BracketRight", &[0x0f]), // =
    ("Enter", &[0x0c]),
    ("NumpadEnter", &[0x0c]),
    ("ControlLeft", &[A_CONTROL]),
    ("KeyA", &[0x3f]),
    ("KeyS", &[0x3e]),
    ("KeyD", &[0x3a]),
    ("KeyF", &[0x38]),
    ("KeyG", &[0x3d]),
    ("KeyH", &[0x39]),
    ("KeyJ", &[0x01]),
    ("KeyK", &[0x05]),
    ("KeyL", &[0x00]),
    ("Semicolon", &[0x02]),
    ("Quote", &[0x06]),     // +
    ("Backslash", &[0x07]), // *
    ("CapsLock", &[0x3c]),
    ("ShiftLeft", &[A_SHIFT]),
    ("ShiftRight", &[A_SHIFT]),
    ("KeyZ", &[0x17]),
    ("KeyX", &[0x16]),
    ("KeyC", &[0x12]),
    ("KeyV", &[0x10]),
    ("KeyB", &[0x15]),
    ("KeyN", &[0x23]),
    ("KeyM", &[0x25]),
    ("Comma", &[0x20]),
    ("Period", &[0x22]),
    ("Slash", &[0x26]),
    ("AltRight", &[0x27]), // Atari / inverse
    ("Space", &[0x21]),
    ("ArrowUp", &[A_CONTROL, 0x0e]),
    ("ArrowDown", &[A_CONTROL, 0x0f]),
    ("ArrowLeft", &[A_CONTROL, 0x06]),
    ("ArrowRight", &[A_CONTROL, 0x07]),
    ("Delete", &[A_CONTROL, 0x34]), // delete character
    ("Insert", &[A_CONTROL, 0x37]), // insert character
    ("F2", &[67]),                  // START
    ("F3", &[68]),                  // SELECT
    ("F4", &[69]),                  // OPTION
    ("F5", &[70]),                  // RESET
    ("F6", &[0x11]),                // HELP
    ("F7", &[66]),                  // BREAK
    ("Pause", &[66]),
];

pub static ATARI: Keymap = Keymap {
    machine: "atari8",
    entries: ATARI_POSITIONAL,
};

/// Atari 2600: console switches on F1–F5 (machine keys 0–4, see
/// `machine-atari2600::keys`) and the two keypads as 3 × 4 key blocks in
/// reading order (1 2 3 / 4 5 6 / 7 8 9 / * 0 #): the left one on
/// 1 2 3 / Q W E / A S D / Z X C (codes 16–27), the right one on
/// 8 9 0 / I O P / K L ; / , . / (codes 32–43).
const ATARI2600_POSITIONAL: &[(&str, &[u16])] = &[
    ("F1", &[1]), // SELECT
    ("F2", &[0]), // RESET
    ("F3", &[2]), // colour / black and white
    ("F4", &[3]), // left difficulty A/B
    ("F5", &[4]), // right difficulty A/B
    ("Digit1", &[16]),
    ("Digit2", &[17]),
    ("Digit3", &[18]),
    ("KeyQ", &[19]),
    ("KeyW", &[20]),
    ("KeyE", &[21]),
    ("KeyA", &[22]),
    ("KeyS", &[23]),
    ("KeyD", &[24]),
    ("KeyZ", &[25]),
    ("KeyX", &[26]),
    ("KeyC", &[27]),
    ("Digit8", &[32]),
    ("Digit9", &[33]),
    ("Digit0", &[34]),
    ("KeyI", &[35]),
    ("KeyO", &[36]),
    ("KeyP", &[37]),
    ("KeyK", &[38]),
    ("KeyL", &[39]),
    ("Semicolon", &[40]),
    ("Comma", &[41]),
    ("Period", &[42]),
    ("Slash", &[43]),
];

pub static ATARI2600: Keymap = Keymap {
    machine: "atari2600",
    entries: ATARI2600_POSITIONAL,
};

/// ColecoVision: the two controllers' keypads (machine keys 16–30 and
/// 32–46, see `machine-coleco::keys`): port 1 on the digit row (1–9, 0,
/// the minus key for *, the equals key for #, the brackets for the Super
/// Action purple and blue buttons, X for the right fire button), port 2 on
/// the numeric keypad (its multiply and divide keys for * and #, its plus
/// key for the right fire button).
const COLECO_POSITIONAL: &[(&str, &[u16])] = &[
    ("Digit1", &[16]),
    ("Digit2", &[17]),
    ("Digit3", &[18]),
    ("Digit4", &[19]),
    ("Digit5", &[20]),
    ("Digit6", &[21]),
    ("Digit7", &[22]),
    ("Digit8", &[23]),
    ("Digit9", &[24]),
    ("Minus", &[25]),
    ("Digit0", &[26]),
    ("Equal", &[27]),
    ("BracketLeft", &[28]),
    ("BracketRight", &[29]),
    ("KeyX", &[30]),
    ("Numpad1", &[32]),
    ("Numpad2", &[33]),
    ("Numpad3", &[34]),
    ("Numpad4", &[35]),
    ("Numpad5", &[36]),
    ("Numpad6", &[37]),
    ("Numpad7", &[38]),
    ("Numpad8", &[39]),
    ("Numpad9", &[40]),
    ("NumpadMultiply", &[41]),
    ("Numpad0", &[42]),
    ("NumpadDivide", &[43]),
    ("NumpadAdd", &[46]),
];

pub static COLECO: Keymap = Keymap {
    machine: "coleco",
    entries: COLECO_POSITIONAL,
};

/// NES: controller 1's buttons (machine keys 0-3, see `machine-nes::keys`):
/// X = A, Z = B, right Shift = Select, Enter = Start; the cross is the
/// joystick (cursor keys, Space also A).
const NES_POSITIONAL: &[(&str, &[u16])] = &[
    ("KeyX", &[0]),
    ("KeyZ", &[1]),
    ("ShiftRight", &[2]),
    ("Enter", &[3]),
];

pub static NES: Keymap = Keymap {
    machine: "nes",
    entries: NES_POSITIONAL,
};

/// SNES: controller 1's buttons (machine keys 0-7 = B A Select Start Y X
/// L R, see `machine-snes::keys`): Z = B, X = A, A = Y, S = X, Q = L,
/// W = R, right Shift = Select, Enter = Start; the cross is the joystick
/// (cursor keys, Space also B).
const SNES_POSITIONAL: &[(&str, &[u16])] = &[
    ("KeyZ", &[0]),
    ("KeyX", &[1]),
    ("ShiftRight", &[2]),
    ("Enter", &[3]),
    ("KeyA", &[4]),
    ("KeyS", &[5]),
    ("KeyQ", &[6]),
    ("KeyW", &[7]),
];

pub static SNES: Keymap = Keymap {
    machine: "snes",
    entries: SNES_POSITIONAL,
};

/// Vectrex: controller 1's buttons 1–4 (machine keys 16–19, see
/// `machine-vectrex::keys`) on Z X C V, a row like on the controller; the
/// stick is the joystick (cursor keys, Space also button 1).
const VECTREX_POSITIONAL: &[(&str, &[u16])] = &[
    ("KeyZ", &[16]),
    ("KeyX", &[17]),
    ("KeyC", &[18]),
    ("KeyV", &[19]),
];

pub static VECTREX: Keymap = Keymap {
    machine: "vectrex",
    entries: VECTREX_POSITIONAL,
};

/// Amstrad CPC keys: `line * 8 + bit` of the keyboard matrix (see
/// `machine-cpc::keyboard`).
const fn cpc(line: u16, bit: u16) -> u16 {
    line * 8 + bit
}

/// CPC keyboard laid onto a PC (US) keyboard by position: the CPC's
/// `- ^` on `- =`, `@ [` on `[ ]`, `: ;` on `; '`, `]` on the backslash
/// key, `\` on the ISO key; DEL on Backspace, CLR on Delete, COPY on
/// left Alt; the function keys f0–f9 and f. on the numeric keypad (and
/// F1–F10), the small ENTER on the keypad's Enter.
const CPC_POSITIONAL: &[(&str, &[u16])] = &[
    ("Escape", &[cpc(8, 2)]),
    ("Digit1", &[cpc(8, 0)]),
    ("Digit2", &[cpc(8, 1)]),
    ("Digit3", &[cpc(7, 1)]),
    ("Digit4", &[cpc(7, 0)]),
    ("Digit5", &[cpc(6, 1)]),
    ("Digit6", &[cpc(6, 0)]),
    ("Digit7", &[cpc(5, 1)]),
    ("Digit8", &[cpc(5, 0)]),
    ("Digit9", &[cpc(4, 1)]),
    ("Digit0", &[cpc(4, 0)]),
    ("Minus", &[cpc(3, 1)]),
    ("Equal", &[cpc(3, 0)]),     // ^
    ("Backspace", &[cpc(9, 7)]), // DEL
    ("Delete", &[cpc(2, 0)]),    // CLR
    ("Tab", &[cpc(8, 4)]),
    ("KeyQ", &[cpc(8, 3)]),
    ("KeyW", &[cpc(7, 3)]),
    ("KeyE", &[cpc(7, 2)]),
    ("KeyR", &[cpc(6, 2)]),
    ("KeyT", &[cpc(6, 3)]),
    ("KeyY", &[cpc(5, 3)]),
    ("KeyU", &[cpc(5, 2)]),
    ("KeyI", &[cpc(4, 3)]),
    ("KeyO", &[cpc(4, 2)]),
    ("KeyP", &[cpc(3, 3)]),
    ("BracketLeft", &[cpc(3, 2)]),  // @
    ("BracketRight", &[cpc(2, 1)]), // [
    ("Enter", &[cpc(2, 2)]),        // RETURN
    ("CapsLock", &[cpc(8, 6)]),
    ("KeyA", &[cpc(8, 5)]),
    ("KeyS", &[cpc(7, 4)]),
    ("KeyD", &[cpc(7, 5)]),
    ("KeyF", &[cpc(6, 5)]),
    ("KeyG", &[cpc(6, 4)]),
    ("KeyH", &[cpc(5, 4)]),
    ("KeyJ", &[cpc(5, 5)]),
    ("KeyK", &[cpc(4, 5)]),
    ("KeyL", &[cpc(4, 4)]),
    ("Semicolon", &[cpc(3, 5)]), // :
    ("Quote", &[cpc(3, 4)]),     // ;
    ("Backslash", &[cpc(2, 3)]), // ]
    ("ShiftLeft", &[cpc(2, 5)]),
    ("ShiftRight", &[cpc(2, 5)]),
    ("IntlBackslash", &[cpc(2, 6)]), // backslash
    ("KeyZ", &[cpc(8, 7)]),
    ("KeyX", &[cpc(7, 7)]),
    ("KeyC", &[cpc(7, 6)]),
    ("KeyV", &[cpc(6, 7)]),
    ("KeyB", &[cpc(6, 6)]),
    ("KeyN", &[cpc(5, 6)]),
    ("KeyM", &[cpc(4, 6)]),
    ("Comma", &[cpc(4, 7)]),
    ("Period", &[cpc(3, 7)]),
    ("Slash", &[cpc(3, 6)]),
    ("ControlLeft", &[cpc(2, 7)]),
    ("ControlRight", &[cpc(2, 7)]),
    ("AltLeft", &[cpc(1, 1)]), // COPY
    ("Space", &[cpc(5, 7)]),
    ("ArrowUp", &[cpc(0, 0)]),
    ("ArrowRight", &[cpc(0, 1)]),
    ("ArrowDown", &[cpc(0, 2)]),
    ("ArrowLeft", &[cpc(1, 0)]),
    ("Numpad0", &[cpc(1, 7)]),
    ("Numpad1", &[cpc(1, 5)]),
    ("Numpad2", &[cpc(1, 6)]),
    ("Numpad3", &[cpc(0, 5)]),
    ("Numpad4", &[cpc(2, 4)]),
    ("Numpad5", &[cpc(1, 4)]),
    ("Numpad6", &[cpc(0, 4)]),
    ("Numpad7", &[cpc(1, 2)]),
    ("Numpad8", &[cpc(1, 3)]),
    ("Numpad9", &[cpc(0, 3)]),
    ("NumpadDecimal", &[cpc(0, 7)]),
    ("NumpadEnter", &[cpc(0, 6)]), // ENTER
    ("F1", &[cpc(1, 5)]),
    ("F2", &[cpc(1, 6)]),
    ("F3", &[cpc(0, 5)]),
    ("F4", &[cpc(2, 4)]),
    ("F5", &[cpc(1, 4)]),
    ("F6", &[cpc(0, 4)]),
    ("F7", &[cpc(1, 2)]),
    ("F8", &[cpc(1, 3)]),
    ("F9", &[cpc(0, 3)]),
    ("F10", &[cpc(1, 7)]),
];

/// Amiga keyboard laid onto a PC (US) keyboard by position; machine keys
/// are the Amiga's raw key codes (HRM, keyboard appendix; see
/// `machine-amiga::keyboard`). HELP on Insert, the keypad's ( and ) on
/// Num Lock and Scroll Lock, the Amiga keys on the Windows/Command keys,
/// both Ctrl keys on CTRL.
const AMIGA_POSITIONAL: &[(&str, &[u16])] = &[
    ("Backquote", &[0x00]),
    ("Digit1", &[0x01]),
    ("Digit2", &[0x02]),
    ("Digit3", &[0x03]),
    ("Digit4", &[0x04]),
    ("Digit5", &[0x05]),
    ("Digit6", &[0x06]),
    ("Digit7", &[0x07]),
    ("Digit8", &[0x08]),
    ("Digit9", &[0x09]),
    ("Digit0", &[0x0a]),
    ("Minus", &[0x0b]),
    ("Equal", &[0x0c]),
    ("Backslash", &[0x0d]),
    ("Numpad0", &[0x0f]),
    ("KeyQ", &[0x10]),
    ("KeyW", &[0x11]),
    ("KeyE", &[0x12]),
    ("KeyR", &[0x13]),
    ("KeyT", &[0x14]),
    ("KeyY", &[0x15]),
    ("KeyU", &[0x16]),
    ("KeyI", &[0x17]),
    ("KeyO", &[0x18]),
    ("KeyP", &[0x19]),
    ("BracketLeft", &[0x1a]),
    ("BracketRight", &[0x1b]),
    ("Numpad1", &[0x1d]),
    ("Numpad2", &[0x1e]),
    ("Numpad3", &[0x1f]),
    ("KeyA", &[0x20]),
    ("KeyS", &[0x21]),
    ("KeyD", &[0x22]),
    ("KeyF", &[0x23]),
    ("KeyG", &[0x24]),
    ("KeyH", &[0x25]),
    ("KeyJ", &[0x26]),
    ("KeyK", &[0x27]),
    ("KeyL", &[0x28]),
    ("Semicolon", &[0x29]),
    ("Quote", &[0x2a]),
    ("Numpad4", &[0x2d]),
    ("Numpad5", &[0x2e]),
    ("Numpad6", &[0x2f]),
    ("IntlBackslash", &[0x30]),
    ("KeyZ", &[0x31]),
    ("KeyX", &[0x32]),
    ("KeyC", &[0x33]),
    ("KeyV", &[0x34]),
    ("KeyB", &[0x35]),
    ("KeyN", &[0x36]),
    ("KeyM", &[0x37]),
    ("Comma", &[0x38]),
    ("Period", &[0x39]),
    ("Slash", &[0x3a]),
    ("NumpadDecimal", &[0x3c]),
    ("Numpad7", &[0x3d]),
    ("Numpad8", &[0x3e]),
    ("Numpad9", &[0x3f]),
    ("Space", &[0x40]),
    ("Backspace", &[0x41]),
    ("Tab", &[0x42]),
    ("NumpadEnter", &[0x43]),
    ("Enter", &[0x44]),
    ("Escape", &[0x45]),
    ("Delete", &[0x46]),
    ("NumpadSubtract", &[0x4a]),
    ("ArrowUp", &[0x4c]),
    ("ArrowDown", &[0x4d]),
    ("ArrowRight", &[0x4e]),
    ("ArrowLeft", &[0x4f]),
    ("F1", &[0x50]),
    ("F2", &[0x51]),
    ("F3", &[0x52]),
    ("F4", &[0x53]),
    ("F5", &[0x54]),
    ("F6", &[0x55]),
    ("F7", &[0x56]),
    ("F8", &[0x57]),
    ("F9", &[0x58]),
    ("F10", &[0x59]),
    ("NumLock", &[0x5a]),
    ("ScrollLock", &[0x5b]),
    ("NumpadDivide", &[0x5c]),
    ("NumpadMultiply", &[0x5d]),
    ("NumpadAdd", &[0x5e]),
    ("Insert", &[0x5f]),
    ("ShiftLeft", &[0x60]),
    ("ShiftRight", &[0x61]),
    ("CapsLock", &[0x62]),
    ("ControlLeft", &[0x63]),
    ("ControlRight", &[0x63]),
    ("AltLeft", &[0x64]),
    ("AltRight", &[0x65]),
    ("MetaLeft", &[0x66]),
    ("MetaRight", &[0x67]),
];

pub static AMIGA: Keymap = Keymap {
    machine: "amiga",
    entries: AMIGA_POSITIONAL,
};

pub static CPC: Keymap = Keymap {
    machine: "cpc",
    entries: CPC_POSITIONAL,
};

/// ZX81: row · 5 + column of its 8 × 5 matrix (see
/// `machine-zx81::keyboard`). Letters, digits, SHIFT, SPACE, NEW LINE
/// (Enter) and `.`; the keyword symbols come from SHIFT on the machine.
const ZX81_POSITIONAL: &[(&str, &[u16])] = &[
    ("ShiftLeft", &[0]),
    ("ShiftRight", &[0]),
    ("KeyZ", &[1]),
    ("KeyX", &[2]),
    ("KeyC", &[3]),
    ("KeyV", &[4]),
    ("KeyA", &[5]),
    ("KeyS", &[6]),
    ("KeyD", &[7]),
    ("KeyF", &[8]),
    ("KeyG", &[9]),
    ("KeyQ", &[10]),
    ("KeyW", &[11]),
    ("KeyE", &[12]),
    ("KeyR", &[13]),
    ("KeyT", &[14]),
    ("Digit1", &[15]),
    ("Digit2", &[16]),
    ("Digit3", &[17]),
    ("Digit4", &[18]),
    ("Digit5", &[19]),
    ("Digit0", &[20]),
    ("Digit9", &[21]),
    ("Digit8", &[22]),
    ("Digit7", &[23]),
    ("Digit6", &[24]),
    ("KeyP", &[25]),
    ("KeyO", &[26]),
    ("KeyI", &[27]),
    ("KeyU", &[28]),
    ("KeyY", &[29]),
    ("Enter", &[30]),
    ("NumpadEnter", &[30]),
    ("KeyL", &[31]),
    ("KeyK", &[32]),
    ("KeyJ", &[33]),
    ("KeyH", &[34]),
    ("Space", &[35]),
    ("Period", &[36]),
    ("KeyM", &[37]),
    ("KeyN", &[38]),
    ("KeyB", &[39]),
    // Convenience: Backspace = SHIFT+0 (RUBOUT), arrows = SHIFT+5…8.
    ("Backspace", &[0, 20]),
    ("ArrowLeft", &[0, 19]),
    ("ArrowDown", &[0, 24]),
    ("ArrowUp", &[0, 23]),
    ("ArrowRight", &[0, 22]),
];

pub static ZX81: Keymap = Keymap {
    machine: "zx81",
    entries: ZX81_POSITIONAL,
};

/// Apple II: machine keys are what a key sends unshifted on the IIe's US
/// keyboard (see `machine-apple2::keyboard`), so the PC's keys map by
/// their legends. Backspace is ← (the Apple's backspace), Delete is
/// DELETE, the Alt keys are Open Apple and Solid Apple, F5 is RESET
/// (with Ctrl on the IIe).
const APPLE2_POSITIONAL: &[(&str, &[u16])] = &[
    ("Backquote", &[0x60]),
    ("Digit1", &[0x31]),
    ("Digit2", &[0x32]),
    ("Digit3", &[0x33]),
    ("Digit4", &[0x34]),
    ("Digit5", &[0x35]),
    ("Digit6", &[0x36]),
    ("Digit7", &[0x37]),
    ("Digit8", &[0x38]),
    ("Digit9", &[0x39]),
    ("Digit0", &[0x30]),
    ("Minus", &[0x2d]),
    ("Equal", &[0x3d]),
    ("Backspace", &[0x08]),
    ("Tab", &[0x09]),
    ("KeyQ", &[0x71]),
    ("KeyW", &[0x77]),
    ("KeyE", &[0x65]),
    ("KeyR", &[0x72]),
    ("KeyT", &[0x74]),
    ("KeyY", &[0x79]),
    ("KeyU", &[0x75]),
    ("KeyI", &[0x69]),
    ("KeyO", &[0x6f]),
    ("KeyP", &[0x70]),
    ("BracketLeft", &[0x5b]),
    ("BracketRight", &[0x5d]),
    ("Backslash", &[0x5c]),
    ("CapsLock", &[0x102]),
    ("KeyA", &[0x61]),
    ("KeyS", &[0x73]),
    ("KeyD", &[0x64]),
    ("KeyF", &[0x66]),
    ("KeyG", &[0x67]),
    ("KeyH", &[0x68]),
    ("KeyJ", &[0x6a]),
    ("KeyK", &[0x6b]),
    ("KeyL", &[0x6c]),
    ("Semicolon", &[0x3b]),
    ("Quote", &[0x27]),
    ("Enter", &[0x0d]),
    ("NumpadEnter", &[0x0d]),
    ("ShiftLeft", &[0x100]),
    ("ShiftRight", &[0x100]),
    ("KeyZ", &[0x7a]),
    ("KeyX", &[0x78]),
    ("KeyC", &[0x63]),
    ("KeyV", &[0x76]),
    ("KeyB", &[0x62]),
    ("KeyN", &[0x6e]),
    ("KeyM", &[0x6d]),
    ("Comma", &[0x2c]),
    ("Period", &[0x2e]),
    ("Slash", &[0x2f]),
    ("ControlLeft", &[0x101]),
    ("ControlRight", &[0x101]),
    ("AltLeft", &[0x104]),
    ("AltRight", &[0x105]),
    ("Space", &[0x20]),
    ("Escape", &[0x1b]),
    ("Delete", &[0x7f]),
    ("ArrowLeft", &[0x08]),
    ("ArrowRight", &[0x15]),
    ("ArrowUp", &[0x0b]),
    ("ArrowDown", &[0x0a]),
    ("F5", &[0x103]),
];

pub static APPLE2: Keymap = Keymap {
    machine: "apple2",
    entries: APPLE2_POSITIONAL,
};

/// The positional keymap for a machine id.
pub fn positional(machine: &str) -> Option<&'static Keymap> {
    match machine {
        "c64" => Some(&C64),
        "c128" => Some(&C128),
        "vic20" => Some(&VIC20),
        "pet" => Some(&PET),
        "plus4" | "c16" => Some(&TED),
        "atari8" => Some(&ATARI),
        "atari2600" => Some(&ATARI2600),
        "coleco" => Some(&COLECO),
        "nes" => Some(&NES),
        "snes" => Some(&SNES),
        "vectrex" => Some(&VECTREX),
        "cpc" => Some(&CPC),
        "amiga" => Some(&AMIGA),
        "zx81" => Some(&ZX81),
        "apple2" => Some(&APPLE2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atari_keys_are_reachable_once() {
        let mut seen = std::collections::BTreeMap::new();
        for (code, keys) in ATARI_POSITIONAL {
            if let [single] = keys {
                *seen.entry(*single).or_insert(0) += 1;
            }
            assert!(keys.iter().all(|&k| k <= 70), "{code}");
        }
        // Every key code of the 800XL keyboard (the 1200XL's F1–F4 and the
        // unused codes have no key).
        let unused = [0x03, 0x04, 0x09, 0x13, 0x14, 0x19, 0x24, 0x29, 0x31, 0x3b];
        for k in (0..64).filter(|k| !unused.contains(k)) {
            assert!(seen.contains_key(&k), "key code {k:#04x} unreachable");
        }
        assert_eq!(positional("atari8").unwrap().machine, "atari8");
        let vcs = positional("atari2600").unwrap();
        let codes: Vec<u16> = ATARI2600_POSITIONAL.iter().map(|(_, k)| k[0]).collect();
        assert_eq!(codes.len(), 5 + 24);
        assert!(vcs.lookup("KeyC").unwrap().eq([MachineKey(27)]));
    }

    #[test]
    fn cpc_keys_are_reachable() {
        let mut seen = std::collections::BTreeSet::new();
        for (_, keys) in CPC_POSITIONAL {
            seen.extend(keys.iter().copied());
        }
        for code in 0..80u16 {
            // Line 9 bits 0-6 are joystick 0 (fire 3 included).
            let joystick = code / 8 == 9 && code % 8 < 7;
            assert_eq!(seen.contains(&code), !joystick, "key {code}");
        }
        assert_eq!(positional("cpc").unwrap().machine, "cpc");
    }

    type Entries = &'static [(&'static str, &'static [u16])];

    /// Each layout with its SHIFT key, which several host keys may name.
    const MAPS: [(Entries, u16); 3] = [
        (C64_POSITIONAL, C64_LSHIFT),
        (VIC20_POSITIONAL, VIC20_LSHIFT),
        (TED_POSITIONAL, TED_SHIFT),
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
            // SHIFT is also on CapsLock (SHIFT LOCK), and on the Plus/4 on
            // both shift keys.
            seen[usize::from(lshift)] = seen[usize::from(lshift)].min(1);
            // The C16/Plus/4 has no RESTORE.
            if map == TED_POSITIONAL {
                seen[64] = 1;
            }
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
    fn c128_adds_its_own_keys() {
        let mut names: Vec<_> = C128_POSITIONAL.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), C128_POSITIONAL.len());
        let up: Vec<_> = C128.lookup("ArrowUp").unwrap().collect();
        assert_eq!(up, [MachineKey(k(10, 3))], "no SHIFT needed");
        assert_eq!(C128.lookup("NumLock").unwrap().next(), Some(MachineKey(89)));
        // Every C64 matrix key is still reachable on its own, except the
        // matrix cursor keys (CRSR → and ↓): the arrows use the dedicated ones.
        for code in (0..64u16).filter(|&c| c != k(0, 2) && c != k(0, 7)) {
            assert!(
                C128_POSITIONAL.iter().any(|(_, keys)| keys == &[code]),
                "matrix key {code}"
            );
        }
    }

    #[test]
    fn pet_codes_are_in_range_and_host_names_unique() {
        let mut names: Vec<_> = PET_LABELLED.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PET_LABELLED.len());
        for (_, keys) in PET_LABELLED {
            assert!(keys.iter().all(|&k| k < 80 || (100..180).contains(&k)));
        }
        let a: Vec<_> = PET.lookup("KeyA").unwrap().collect();
        assert_eq!(a, [MachineKey(g(4, 0)), MachineKey(b(3, 0))]);
    }

    #[test]
    fn both_layouts_use_the_same_host_keys() {
        let names = |map: Entries| {
            let mut n: Vec<_> = map.iter().map(|(n, _)| *n).collect();
            n.sort_unstable();
            n
        };
        assert_eq!(names(C64_POSITIONAL), names(VIC20_POSITIONAL));
        let ted = names(TED_POSITIONAL);
        assert!(ted.iter().all(|n| names(C64_POSITIONAL).contains(n)));
    }

    #[test]
    fn cursor_up_is_shift_plus_down() {
        let keys: Vec<_> = C64.lookup("ArrowUp").unwrap().collect();
        assert_eq!(keys, [MachineKey(C64_RSHIFT), MachineKey(k(0, 7))]);
        assert!(C64.lookup("NumLock").is_none());
    }
}
