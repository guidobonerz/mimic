//! PET keyboards: 10 rows × 8 columns. PIA1 PA0–PA3 select a row through
//! a 74145 BCD decoder (values 10–15 select none); PIA1 port B reads its
//! columns (0 = pressed).
//!
//! Two matrices exist. The graphics keyboard (2001, 3032, 4032) has the
//! digits on a keypad and every symbol on its own key; the business
//! keyboard (8032, some 4032) has a typewriter layout with a separate
//! keypad. The tables below follow the key decode tables of the editor
//! ROMs (901447-24 / 901498-01 graphics, 901474-04 business), which list
//! rows 9 to 0 and in each row columns 7 to 0.
//!
//! [`MachineKey`] codes: graphics keys are `row * 8 + column` (0–79),
//! business keys `100 + row * 8 + column` (100–179). A machine ignores the
//! codes of the other keyboard, so a host key can name both.

use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::Cycle;

/// Which matrix the machine has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Graphics,
    Business,
}

/// First code of the business keyboard.
pub const BUSINESS_BASE: u16 = 100;

/// Graphics keyboard, rows 0–9, columns 0–7. Empty names are unused.
const GRAPHICS: [[&str; 8]; 10] = [
    ["!", "#", "%", "&", "(", "←", "HOME", "CRSR_RIGHT"],
    ["\"", "$", "'", "\\", ")", "", "CRSR_DOWN", "DEL"],
    ["Q", "E", "T", "U", "O", "↑", "7", "9"],
    ["W", "R", "Y", "I", "P", "", "8", "/"],
    ["A", "D", "G", "J", "L", "", "4", "6"],
    ["S", "F", "H", "K", ":", "", "5", "*"],
    ["Z", "C", "B", "M", ";", "RETURN", "1", "3"],
    ["X", "V", "N", ",", "?", "", "2", "+"],
    ["LSHIFT", "@", "]", "", ">", "RSHIFT", "0", "-"],
    ["RVS", "[", " ", "<", "STOP", "", ".", "="],
];

/// Business keyboard, rows 0–9, columns 0–7. `KP` marks the keypad.
const BUSINESS: [[&str; 8]; 10] = [
    ["KP2", "KP5", "KP8", "KP-", "8", "CRSR_RIGHT", "", ""],
    ["KP1", "KP4", "KP7", "0", "7", "↑", "", "9"],
    ["ESC", "S", "F", "H", "]", "K", ";", "5"],
    ["A", "D", "G", "J", "RETURN", "L", "@", "6"],
    ["TAB", "W", "R", "Y", "\\", "I", "P", "DEL"],
    ["Q", "E", "T", "U", "CRSR_DOWN", "O", "[", "4"],
    ["LSHIFT", "C", "B", "KP.", ".", "", "RSHIFT", "3"],
    ["Z", "V", "N", ",", "KP0", "", "", "2"],
    ["RVS", "X", " ", "M", "HOME", "", "/", "1"],
    ["←", "KP3", "KP6", "KP9", "STOP", ":", "", ""],
];

impl Layout {
    fn table(self) -> &'static [[&'static str; 8]; 10] {
        match self {
            Layout::Graphics => &GRAPHICS,
            Layout::Business => &BUSINESS,
        }
    }

    fn base(self) -> u16 {
        match self {
            Layout::Graphics => 0,
            Layout::Business => BUSINESS_BASE,
        }
    }

    /// The key with this label, if the keyboard has one.
    pub fn key(self, label: &str) -> Option<MachineKey> {
        let t = self.table();
        (0..80)
            .find(|&i| t[i / 8][i % 8] == label)
            .map(|i| MachineKey(self.base() + i as u16))
    }

    /// (row, column) of a code of this keyboard.
    fn position(self, k: MachineKey) -> Option<(usize, usize)> {
        let i = k.0.checked_sub(self.base())?;
        (i < 80).then_some((usize::from(i / 8), usize::from(i % 8)))
    }

    /// The key (and whether SHIFT is needed) that types `c`: upper-case
    /// letters, digits, space, RETURN and the symbols with their own key.
    pub fn key_for_char(self, c: char) -> Option<(MachineKey, bool)> {
        let c = match c {
            '\r' | '\n' => return self.key("RETURN").map(|k| (k, false)),
            c => c.to_ascii_uppercase(),
        };
        let mut buf = [0u8; 4];
        self.key(c.encode_utf8(&mut buf)).map(|k| (k, false))
    }
}

/// Cycles a typed key stays down, and the gap before the next one (the
/// editor scans once per 1/50 or 1/60 s interrupt).
const TYPE_HOLD: u64 = 40_000;
const TYPE_GAP: u64 = 40_000;

/// Key events that type `text` from cycle `start` (characters without a key
/// are skipped). Returns the events and the cycle after the last release.
pub fn typed_events(layout: Layout, text: &str, start: u64) -> (Vec<TimedInput>, u64) {
    let key_event = |at: u64, k: MachineKey, down: bool| TimedInput {
        at: Cycle(at),
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: k,
            down,
        },
    };
    let mut events = Vec::new();
    let mut t = start;
    for c in text.chars() {
        let Some((k, _)) = layout.key_for_char(c) else {
            continue;
        };
        events.push(key_event(t, k, true));
        events.push(key_event(t + TYPE_HOLD, k, false));
        t += TYPE_HOLD + TYPE_GAP;
    }
    (events, t)
}

/// Pressed keys: one byte of columns per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyboard {
    pub layout: Layout,
    pub rows: [u8; 10],
}

impl Keyboard {
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            rows: [0; 10],
        }
    }

    /// Press or release; returns false for codes of another keyboard.
    pub fn set(&mut self, k: MachineKey, down: bool) -> bool {
        let Some((row, col)) = self.layout.position(k) else {
            return false;
        };
        if down {
            self.rows[row] |= 1 << col;
        } else {
            self.rows[row] &= !(1 << col);
        }
        true
    }

    /// Column levels for the row selected by PA0–PA3 (0 = pressed).
    pub fn columns(&self, select: u8) -> u8 {
        match self.rows.get(usize::from(select & 0x0f)) {
            Some(r) => !r,
            None => 0xff,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_unique_per_layout() {
        for layout in [Layout::Graphics, Layout::Business] {
            let mut labels: Vec<&str> = layout
                .table()
                .iter()
                .flatten()
                .copied()
                .filter(|l| !l.is_empty())
                .collect();
            let n = labels.len();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(labels.len(), n, "{layout:?}");
        }
    }

    #[test]
    fn rows_are_selected_by_bcd() {
        let mut k = Keyboard::new(Layout::Graphics);
        let a = Layout::Graphics.key("A").unwrap();
        assert!(k.set(a, true));
        assert_eq!(k.columns(4), 0xfe);
        assert_eq!(k.columns(3), 0xff);
        assert_eq!(k.columns(12), 0xff, "no row for 12");
        let b = Layout::Business.key("A").unwrap();
        assert_eq!(b, MachineKey(100 + 3 * 8));
        assert!(!k.set(b, true), "business code on a graphics keyboard");
    }

    #[test]
    fn typing_uses_the_layout() {
        assert_eq!(
            Layout::Graphics.key_for_char('6'),
            Some((MachineKey(4 * 8 + 7), false))
        );
        assert_eq!(
            Layout::Business.key_for_char('6'),
            Some((MachineKey(100 + 3 * 8 + 7), false)),
            "main row, not the keypad"
        );
        assert_eq!(Layout::Business.key_for_char('*'), None);
        let (ev, end) = typed_events(Layout::Graphics, "RUN\n", 10);
        assert_eq!(ev.len(), 8);
        assert_eq!(end, 10 + 4 * 80_000);
    }
}
