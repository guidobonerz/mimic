//! Console panel (Ctrl+P): the machine's buttons and switches
//! ([`Machine::controls`](emu_core::Machine::controls)) as a strip under the
//! picture. A button is held while the left mouse button is down on it; a
//! click on a switch position sets the switch. The window sends the clicks
//! to the emulation thread, which queues them as
//! [`InputEvent::Control`](emu_core::input::InputEvent::Control) events.
//! The frontend adds its own controls (PAUSE, JOY KEYS, JOY PORT) whose
//! clicks stay in the window.

use emu_core::{ControlInfo, ControlKind};
use noto_sans_mono_bitmap::RasterHeight;

use crate::footer::{Rgb, Strip, BUSY, DIM, TEXT};

/// The frontend's own switch on the panel: which host keys play the
/// joystick (positions in the order of [`emu_input::JoyKeys::ALL`]). Its
/// clicks stay in the window, the machine never sees them.
pub const JOY_KEYS: u16 = 0xffff;

/// The panel entry for the keyboard joystick setting `value`.
pub fn joy_keys_control(value: u8) -> ControlInfo {
    ControlInfo::switch(JOY_KEYS, "JOY KEYS", &["OFF", "NUMPAD", "CURSOR"], value)
}

/// The frontend's PAUSE button: stops the whole machine and lets it run
/// again; lit while the machine is stopped (also by a debugger).
pub const PAUSE: u16 = 0xfffd;

/// The panel entry for the PAUSE button.
pub fn pause_control(paused: bool) -> ControlInfo {
    ControlInfo::button(PAUSE, "PAUSE", paused)
}

/// The frontend's switch for the port the joysticks play (shown when the
/// machine has more than one joystick port).
pub const JOY_PORT: u16 = 0xfffe;

const PORT_NAMES: [&str; 4] = ["1", "2", "3", "4"];

/// The panel entry for joysticks in port `value` (0-based) of `ports`.
pub fn joy_port_control(ports: u8, value: u8) -> ControlInfo {
    let n = usize::from(ports).clamp(1, PORT_NAMES.len());
    ControlInfo::switch(JOY_PORT, "JOY PORT", &PORT_NAMES[..n], value)
}

/// Face of a button or switch position, and of the active one.
const FACE: Rgb = [0x38, 0x38, 0x38];
const ACTIVE: Rgb = [0x2E, 0x6B, 0x3A];
/// Gap between controls and around box labels, in characters.
const GAP: &str = "  ";

/// A clickable area of the panel: row `row`, columns `x0..x1`, the control
/// and, for a switch, the position it selects (`None`: a button).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub row: usize,
    pub x0: usize,
    pub x1: usize,
    pub id: u16,
    pub position: Option<u8>,
}

/// The control at column `x` of row `row`.
pub fn hit(hits: &[Hit], row: usize, x: usize) -> Option<Hit> {
    hits.iter()
        .copied()
        .find(|h| h.row == row && (h.x0..h.x1).contains(&x))
}

/// Draw the controls into rows `width` pixels wide, starting a new row
/// where a control would not fit; returns the pixels (rows one under the
/// other), the clickable areas and the number of rows.
pub fn render(
    controls: &[ControlInfo],
    width: u32,
    size: RasterHeight,
) -> (Vec<u8>, Vec<Hit>, usize) {
    let margin = size.val() / 2;
    let mut rows = vec![Strip::new(width, size)];
    let mut hits = Vec::new();
    let mut x = margin;
    let inset = 2;
    let boxed = |strip: &mut Strip, x: &mut usize, label: &str, active: bool| {
        let x0 = *x;
        let w = strip.text_width(&format!(" {label} "));
        strip.fill(x0, x0 + w, inset, if active { ACTIVE } else { FACE });
        strip.text(x0, &format!(" {label} "), if active { BUSY } else { TEXT });
        *x = x0 + w;
        (x0, x0 + w)
    };
    if controls.is_empty() {
        rows[0].text(x, "no console controls", DIM);
    }
    for c in controls {
        let strip = &rows[0];
        let need = match c.kind {
            ControlKind::Button => strip.text_width(&format!(" {} ", c.name)),
            ControlKind::Switch(positions) => {
                strip.text_width(&format!("{} ", c.name))
                    + positions
                        .iter()
                        .map(|p| strip.text_width(&format!(" {p} ")))
                        .sum::<usize>()
            }
        };
        if x > margin && x + need > width as usize {
            rows.push(Strip::new(width, size));
            x = margin;
        }
        let row = rows.len() - 1;
        let strip = &mut rows[row];
        match c.kind {
            ControlKind::Button => {
                let (x0, x1) = boxed(strip, &mut x, c.name, c.value != 0);
                hits.push(Hit {
                    row,
                    x0,
                    x1,
                    id: c.id.0,
                    position: None,
                });
            }
            ControlKind::Switch(positions) => {
                x = strip.text(x, &format!("{} ", c.name), DIM);
                for (i, p) in positions.iter().enumerate() {
                    let (x0, x1) = boxed(strip, &mut x, p, usize::from(c.value) == i);
                    hits.push(Hit {
                        row,
                        x0,
                        x1,
                        id: c.id.0,
                        position: Some(i as u8),
                    });
                }
            }
        }
        x = strip.text(x, GAP, DIM);
    }
    let n = rows.len();
    (rows.into_iter().flat_map(|r| r.px).collect(), hits, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_and_switch_positions_are_clickable() {
        let controls = [
            ControlInfo::button(0, "POWER", false),
            ControlInfo::switch(3, "TV TYPE", &["COLOR", "B/W"], 1),
        ];
        let size = RasterHeight::Size16;
        let (px, hits, rows) = render(&controls, 600, size);
        assert_eq!(rows, 1);
        assert_eq!(px.len(), 600 * crate::footer::height(size) as usize * 4);
        assert_eq!(hits.len(), 3, "POWER, COLOR, B/W");
        assert_eq!(hits[0].position, None);
        assert_eq!((hits[1].id, hits[1].position), (3, Some(0)));
        assert_eq!((hits[2].id, hits[2].position), (3, Some(1)));
        assert!(hits[0].x1 <= hits[1].x0 && hits[1].x1 <= hits[2].x0);
        let mid = (hits[2].x0 + hits[2].x1) / 2;
        assert_eq!(hit(&hits, 0, mid), Some(hits[2]));
        assert_eq!(hit(&hits, 0, 599), None);
        assert_eq!(hit(&hits, 1, mid), None);
        // The selected position has the active face.
        let row = 2 * 600 * 4;
        let at = |x: usize| &px[row + x * 4..row + x * 4 + 3];
        assert_eq!(at(hits[2].x0 + 1), ACTIVE);
        assert_eq!(at(hits[1].x0 + 1), FACE);
    }

    #[test]
    fn controls_wrap_into_more_rows() {
        let controls = [
            ControlInfo::button(0, "POWER", false),
            ControlInfo::button(1, "RESET", false),
            ControlInfo::switch(3, "TV TYPE", &["COLOR", "B/W"], 0),
        ];
        let size = RasterHeight::Size16;
        let row_px = 250 * crate::footer::height(size) as usize * 4;
        let (px, hits, rows) = render(&controls, 250, size);
        assert_eq!(rows, 2);
        assert_eq!(px.len(), 2 * row_px);
        assert_eq!((hits[0].row, hits[1].row), (0, 0));
        assert_eq!((hits[2].row, hits[3].row), (1, 1), "the switch moved down");
        assert!(hits[2].x0 < hits[1].x0, "from the left again");
    }
}
