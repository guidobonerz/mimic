//! Console panel (Shift+Alt+← or a click on its handle): the machine's buttons
//! and switches ([`Machine::controls`](emu_core::Machine::controls)) one
//! under the other at the window's left edge ([`render_side`]); closed, only
//! a thin handle remains, as with the media panel at the right. A button is held while the left mouse button is down on it; a
//! click on a switch position sets the switch. The window sends the clicks
//! to the emulation thread, which queues them as
//! [`InputEvent::Control`](emu_core::input::InputEvent::Control) events.
//! The frontend adds its own controls (PAUSE, JOY KEYS, JOY PORT) whose
//! clicks stay in the window.

use emu_core::{ControlInfo, ControlKind};
use noto_sans_mono_bitmap::{get_raster_width, FontWeight, RasterHeight};

use crate::footer::{Rgb, Strip, BACKGROUND, BUSY, DIM, TEXT};

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

/// The frontend's switch for the gamepad button that fires the joystick
/// (positions in the order of [`PAD_FIRE_BUTTONS`]).
pub const PAD_FIRE: u16 = 0xfffc;

/// Gamepad buttons the fire switch offers, by position (layout-neutral:
/// South is A on an Xbox pad, X on a PlayStation pad).
pub const PAD_FIRE_BUTTONS: [emu_input::Button; 4] = [
    emu_input::Button::South,
    emu_input::Button::East,
    emu_input::Button::West,
    emu_input::Button::North,
];

/// The panel entry for the gamepad fire button at position `value`.
pub fn pad_fire_control(value: u8) -> ControlInfo {
    ControlInfo::switch(
        PAD_FIRE,
        "PAD FIRE",
        &["SOUTH", "EAST", "WEST", "NORTH"],
        value,
    )
}

/// The frontend's SAVE MAPPING button: keeps JOY KEYS, JOY PORT and PAD
/// FIRE for this machine (`<user folder>/input/<machine>.toml`).
pub const SAVE_MAPPING: u16 = 0xfffb;

pub fn save_mapping_control() -> ControlInfo {
    ControlInfo::button(SAVE_MAPPING, "SAVE MAPPING", false)
}

/// Cheats are shown as switches with ids from here on (the index into the
/// list the emulation thread published).
pub const CHEAT_BASE: u16 = 0xe000;

/// A panel entry with its own texts: a button (no positions) or a switch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: u16,
    pub name: String,
    pub positions: Vec<String>,
    pub value: u8,
}

impl From<&ControlInfo> for Item {
    fn from(c: &ControlInfo) -> Self {
        Item {
            id: c.id.0,
            name: c.name.to_string(),
            positions: match c.kind {
                ControlKind::Button => Vec::new(),
                ControlKind::Switch(p) => p.iter().map(|s| s.to_string()).collect(),
            },
            value: c.value,
        }
    }
}

/// Face of a button or switch position, and of the active one.
const FACE: Rgb = [0x38, 0x38, 0x38];
const ACTIVE: Rgb = [0x2E, 0x6B, 0x3A];

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

/// Characters a line of the open side panel holds.
const COLUMNS: usize = 24;
const HANDLE: Rgb = [0x30, 0x30, 0x30];
const TITLE: &str = "CONTROLS";

fn char_width(size: RasterHeight) -> usize {
    get_raster_width(FontWeight::Regular, size)
}

/// Width of the closed side panel: the handle, in window pixels.
pub fn handle_width(size: RasterHeight) -> u32 {
    (char_width(size) + 6) as u32
}

/// Width of the open side panel, handle included.
pub fn open_width(size: RasterHeight) -> u32 {
    handle_width(size) + (char_width(size) * (COLUMNS + 2)) as u32
}

/// The console panel at the window's left edge, as a picture.
pub struct Side {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Clickable controls; `row` is the line shown, from the top.
    pub hits: Vec<Hit>,
    /// Columns from here on are the handle (opens and closes the panel).
    pub handle_x0: usize,
    /// The largest useful scroll offset, in lines.
    pub max_scroll: usize,
}

/// The controls one under the other in a column `width` pixels wide: a
/// button on a line of its own, a switch's name on one line and its
/// positions under it (on more lines where they do not fit). Returns the
/// lines and the clickable areas by line.
fn column(controls: &[Item], x0: usize, width: u32, size: RasterHeight) -> (Vec<Strip>, Vec<Hit>) {
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    let inset = 2;
    let right = width as usize;
    if controls.is_empty() {
        let mut s = Strip::new(width, size);
        s.text(x0, "no console controls", DIM);
        lines.push(s);
    }
    for c in controls {
        if c.positions.is_empty() {
            let mut s = Strip::new(width, size);
            let label = format!(" {} ", c.name);
            let w = s.text_width(&label).min(right - x0);
            let active = c.value != 0;
            s.fill(x0, x0 + w, inset, if active { ACTIVE } else { FACE });
            s.text(x0, &label, if active { BUSY } else { TEXT });
            hits.push(Hit {
                row: lines.len(),
                x0,
                x1: x0 + w,
                id: c.id,
                position: None,
            });
            lines.push(s);
            continue;
        }
        let mut name = Strip::new(width, size);
        name.text(x0, &c.name, DIM);
        lines.push(name);
        let mut s = Strip::new(width, size);
        let mut x = x0;
        for (i, p) in c.positions.iter().enumerate() {
            let label = format!(" {p} ");
            let w = s.text_width(&label);
            if x > x0 && x + w > right {
                lines.push(std::mem::replace(&mut s, Strip::new(width, size)));
                x = x0;
            }
            let active = usize::from(c.value) == i;
            let x1 = (x + w).min(right);
            s.fill(x, x1, inset, if active { ACTIVE } else { FACE });
            s.text(x, &label, if active { BUSY } else { TEXT });
            hits.push(Hit {
                row: lines.len(),
                x0: x,
                x1,
                id: c.id,
                position: Some(i as u8),
            });
            // One pixel between positions.
            x = x1 + 1;
        }
        lines.push(s);
    }
    (lines, hits)
}

/// Draw the side panel, `height` window pixels high, from line `scroll`
/// on; closed it is only its handle at the right.
pub fn render_side(
    controls: &[Item],
    open: bool,
    scroll: usize,
    height: u32,
    size: RasterHeight,
) -> Side {
    let handle = handle_width(size) as usize;
    let width = if open {
        open_width(size)
    } else {
        handle_width(size)
    };
    let handle_x0 = width as usize - handle;
    let line_height = crate::footer::height(size) as usize;
    let visible = (height as usize / line_height.max(1)).max(1);
    let mut lines = Vec::new();
    let mut areas = Vec::new();
    if open {
        let x0 = char_width(size);
        let mut title = Strip::new(width, size);
        title.text(x0, TITLE, DIM);
        lines.push(title);
        let (more, hits) = column(controls, x0, handle_x0 as u32, size);
        // The column is narrower than the panel: widen its lines.
        for strip in more {
            let mut line = Strip::new(width, size);
            for (dst, src) in line
                .px
                .chunks_exact_mut(width as usize * 4)
                .zip(strip.px.chunks_exact(strip.width * 4))
            {
                dst[..src.len()].copy_from_slice(src);
            }
            lines.push(line);
        }
        areas.extend(hits.into_iter().map(|h| Hit {
            row: h.row + 1,
            ..h
        }));
    }
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let row_bytes = width as usize * 4;
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for strip in lines.iter().skip(scroll).take(visible) {
        rgba.extend_from_slice(&strip.px);
    }
    rgba.truncate(row_bytes * height as usize);
    while rgba.len() < row_bytes * height as usize {
        rgba.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }
    // The handle over all of it, with an arrow where it opens or closes.
    for line in rgba.chunks_exact_mut(row_bytes) {
        for x in handle_x0..width as usize {
            line[x * 4..x * 4 + 3].copy_from_slice(&HANDLE);
        }
    }
    let mut arrow = Strip::new(handle as u32, size);
    arrow.fill(0, handle, 0, HANDLE);
    arrow.text(3, if open { "<" } else { ">" }, TEXT);
    for (y, line) in arrow.px.chunks_exact(handle * 4).enumerate() {
        if y < height as usize {
            let at = y * row_bytes + handle_x0 * 4;
            rgba[at..at + handle * 4].copy_from_slice(line);
        }
    }
    let hits = areas
        .into_iter()
        .filter_map(|h| {
            let row = h.row.checked_sub(scroll).filter(|&r| r < visible)?;
            Some(Hit { row, ..h })
        })
        .collect();
    Side {
        rgba,
        width,
        height,
        hits,
        handle_x0,
        max_scroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(controls: &[ControlInfo]) -> Vec<Item> {
        controls.iter().map(Item::from).collect()
    }

    #[test]
    fn a_closed_side_panel_is_only_its_handle() {
        let size = RasterHeight::Size16;
        let p = render_side(
            &items(&[ControlInfo::button(0, "RESET", false)]),
            false,
            0,
            300,
            size,
        );
        assert_eq!(p.width, handle_width(size));
        assert_eq!(p.handle_x0, 0);
        assert_eq!(p.rgba.len(), (p.width * 300 * 4) as usize);
        assert!(p.hits.is_empty());
    }

    #[test]
    fn the_side_panel_puts_controls_one_under_the_other() {
        let size = RasterHeight::Size16;
        let controls = items(&[
            ControlInfo::button(0, "POWER", false),
            ControlInfo::switch(3, "TV TYPE", &["COLOR", "B/W"], 1),
            ControlInfo::button(1, "RESET", false),
        ]);
        let p = render_side(&controls, true, 0, 400, size);
        assert_eq!(p.width, open_width(size));
        assert_eq!(
            p.handle_x0,
            (open_width(size) - handle_width(size)) as usize
        );
        assert_eq!(p.rgba.len(), (p.width * 400 * 4) as usize);
        // Title, POWER, the switch's name, its positions, RESET.
        let rows: Vec<(usize, u16, Option<u8>)> =
            p.hits.iter().map(|h| (h.row, h.id, h.position)).collect();
        assert_eq!(
            rows,
            [(1, 0, None), (3, 3, Some(0)), (3, 3, Some(1)), (4, 1, None)]
        );
        assert!(p.hits.iter().all(|h| h.x1 <= p.handle_x0));
        let b = p.hits[2];
        assert_eq!(hit(&p.hits, 3, (b.x0 + b.x1) / 2), Some(b));
        // The selected position has the active face.
        let line = crate::footer::height(size) as usize;
        let at = |x: usize, row: usize| {
            let i = ((row * line + line / 2) * p.width as usize + x) * 4;
            [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2]]
        };
        assert_eq!(at(b.x0 + 1, 3), ACTIVE);
        assert_eq!(at(p.hits[1].x0 + 1, 3), FACE);
    }

    #[test]
    fn a_short_side_panel_scrolls_and_clicks_follow() {
        let size = RasterHeight::Size16;
        let controls: Vec<Item> = (0..10)
            .map(|i| Item::from(&ControlInfo::button(i, "RESET", false)))
            .collect();
        let height = 4 * crate::footer::height(size);
        let p = render_side(&controls, true, 0, height, size);
        assert_eq!(p.max_scroll, 11 - 4);
        assert_eq!(hit(&p.hits, 1, p.hits[0].x0).map(|h| h.id), Some(0));
        let down = render_side(&controls, true, 3, height, size);
        assert_eq!(hit(&down.hits, 1, down.hits[0].x0).map(|h| h.id), Some(3));
        let far = render_side(&controls, true, 99, height, size);
        assert_eq!(far.rgba.len(), (far.width * height * 4) as usize);
        assert!(far.hits.iter().any(|h| h.id == 9));
    }

    #[test]
    fn switch_positions_wrap_in_the_side_panel() {
        let size = RasterHeight::Size16;
        let controls = items(&[ControlInfo::switch(
            7,
            "MANY",
            &["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH"],
            0,
        )]);
        let p = render_side(&controls, true, 0, 400, size);
        assert!(p.hits.last().unwrap().row > p.hits[0].row, "positions wrap");
        assert!(p.hits.iter().all(|h| h.x1 <= p.handle_x0));
    }
}
