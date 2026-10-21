//! Console panel (Ctrl+P): the machine's buttons and switches
//! ([`Machine::controls`](emu_core::Machine::controls)) as a strip under the
//! picture. A button is held while the left mouse button is down on it; a
//! click on a switch position sets the switch. The window sends the clicks
//! to the emulation thread, which queues them as
//! [`InputEvent::Control`](emu_core::input::InputEvent::Control) events.

use emu_core::{ControlInfo, ControlKind};
use noto_sans_mono_bitmap::RasterHeight;

use crate::footer::{Rgb, Strip, BUSY, DIM, TEXT};

/// Face of a button or switch position, and of the active one.
const FACE: Rgb = [0x38, 0x38, 0x38];
const ACTIVE: Rgb = [0x2E, 0x6B, 0x3A];
/// Gap between controls and around box labels, in characters.
const GAP: &str = "  ";

/// A clickable area of the strip: columns `x0..x1`, the control and, for a
/// switch, the position it selects (`None`: a button).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub x0: usize,
    pub x1: usize,
    pub id: u16,
    pub position: Option<u8>,
}

/// The control under column `x`.
pub fn hit(hits: &[Hit], x: usize) -> Option<Hit> {
    hits.iter().copied().find(|h| (h.x0..h.x1).contains(&x))
}

/// Draw the controls into a strip `width` pixels wide; returns its pixels
/// and the clickable areas.
pub fn render(controls: &[ControlInfo], width: u32, size: RasterHeight) -> (Vec<u8>, Vec<Hit>) {
    let mut strip = Strip::new(width, size);
    let mut hits = Vec::new();
    let mut x = size.val() / 2;
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
        strip.text(x, "no console controls", DIM);
    }
    for c in controls {
        match c.kind {
            ControlKind::Button => {
                let (x0, x1) = boxed(&mut strip, &mut x, c.name, c.value != 0);
                hits.push(Hit {
                    x0,
                    x1,
                    id: c.id.0,
                    position: None,
                });
            }
            ControlKind::Switch(positions) => {
                x = strip.text(x, &format!("{} ", c.name), DIM);
                for (i, p) in positions.iter().enumerate() {
                    let (x0, x1) = boxed(&mut strip, &mut x, p, usize::from(c.value) == i);
                    hits.push(Hit {
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
    (strip.px, hits)
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
        let (px, hits) = render(&controls, 600, size);
        assert_eq!(px.len(), 600 * crate::footer::height(size) as usize * 4);
        assert_eq!(hits.len(), 3, "POWER, COLOR, B/W");
        assert_eq!(hits[0].position, None);
        assert_eq!((hits[1].id, hits[1].position), (3, Some(0)));
        assert_eq!((hits[2].id, hits[2].position), (3, Some(1)));
        assert!(hits[0].x1 <= hits[1].x0 && hits[1].x1 <= hits[2].x0);
        let mid = (hits[2].x0 + hits[2].x1) / 2;
        assert_eq!(hit(&hits, mid), Some(hits[2]));
        assert_eq!(hit(&hits, 599), None);
        // The selected position has the active face.
        let row = 2 * 600 * 4;
        let at = |x: usize| &px[row + x * 4..row + x * 4 + 3];
        assert_eq!(at(hits[2].x0 + 1), ACTIVE);
        assert_eq!(at(hits[1].x0 + 1), FACE);
    }
}
