//! Console panel (Shift+Alt+← or a click on its handle): the machine's buttons
//! and switches ([`Machine::controls`](mimic_core::Machine::controls)) one
//! under the other at the window's left edge ([`render_side`]); closed, only
//! a thin handle remains, as with the media panel at the right. A button is held while the left mouse button is down on it; a
//! click on a switch position sets the switch. The window sends the clicks
//! to the emulation thread, which queues them as
//! [`InputEvent::Control`](mimic_core::input::InputEvent::Control) events.
//! The frontend adds its own controls (PAUSE, JOY KEYS, JOY PORT) whose
//! clicks stay in the window.
//!
//! Under the controls the panel shows the machine's lamps
//! ([`Machine::lights`](mimic_core::Machine::lights): an LED strip, case
//! lighting): each group's name and its LEDs as rows of small cells in
//! the LEDs' colours, outlined so that a dark LED is seen too. They are
//! not clickable.

use mimic_core::{ControlInfo, ControlKind, LightGroup};
use noto_sans_mono_bitmap::{get_raster_width, FontWeight, RasterHeight};

use crate::footer::{Rgb, Strip, BACKGROUND, BUSY, DIM, TEXT};

/// The frontend's own switch on the panel: which host keys play the
/// joystick (positions in the order of [`mimic_input::JoyKeys::ALL`]). Its
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
pub const PAD_FIRE_BUTTONS: [mimic_input::Button; 4] = [
    mimic_input::Button::South,
    mimic_input::Button::East,
    mimic_input::Button::West,
    mimic_input::Button::North,
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

/// The frontend's PASTE button: the machine's keyboard types the text in
/// the clipboard (Ctrl+V); shown for machines that can type text.
pub const PASTE: u16 = 0xfffa;

pub fn paste_control() -> ControlInfo {
    ControlInfo::button(PASTE, "PASTE", false)
}

/// Debug overlays (sprite boxes, raster lines …) are shown as OFF/ON
/// switches with ids from here on (the index into the list the emulation
/// thread published, `Shared::overlay_list`); clicks stay in the window.
pub const OVERLAY_BASE: u16 = 0xd000;

/// The panel entry for overlay `index` of the published list.
pub fn overlay_item(index: usize, name: &str, on: bool) -> Item {
    Item {
        id: OVERLAY_BASE + index as u16,
        name: name.chars().take(COLUMNS).collect(),
        positions: vec!["OFF".into(), "ON".into()],
        value: u8::from(on),
    }
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

/// Outline of an LED cell.
const LED_EDGE: Rgb = [0x48, 0x48, 0x48];

/// How `count` LEDs lie in a column `width` pixels wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightGrid {
    /// LEDs a row (the last row may have fewer).
    pub per_row: usize,
    pub rows: usize,
    /// From one cell's left edge to the next's.
    pub pitch: usize,
    /// A cell's width and height, outline included.
    pub cell: usize,
    /// Rows of pixels above a cell in its line.
    pub top: usize,
}

/// The grid for `count` LEDs: as many rows as the width asks for, all
/// about equally long (30 LEDs where 17 fit: two rows of 15).
pub fn light_grid(count: usize, width: usize, size: RasterHeight) -> LightGrid {
    let line = crate::footer::height(size) as usize;
    let pitch = char_width(size) * 3 / 2;
    let mut cell = (pitch - 2).min(line - 4);
    // As much room above as below.
    cell -= (line - cell) % 2;
    let fit = (width / pitch).max(1);
    let rows = count.div_ceil(fit);
    LightGrid {
        per_row: count.div_ceil(rows.max(1)),
        rows,
        pitch,
        cell,
        top: (line - cell) / 2,
    }
}

/// The lamps under the controls: per group a line with its name, then
/// its LEDs from `x0` on in lines `width` pixels wide.
fn light_lines(lights: &[LightGroup], x0: usize, width: u32, size: RasterHeight) -> Vec<Strip> {
    let mut lines = Vec::new();
    for group in lights.iter().filter(|g| !g.leds.is_empty()) {
        let mut name = Strip::new(width, size);
        name.text(x0, group.name, DIM);
        lines.push(name);
        let grid = light_grid(group.leds.len(), width as usize - x0, size);
        for row in group.leds.chunks(grid.per_row) {
            let mut s = Strip::new(width, size);
            for (i, led) in row.iter().enumerate() {
                let x = x0 + i * grid.pitch;
                s.fill(x, x + grid.cell, grid.top, LED_EDGE);
                s.fill(
                    x + 1,
                    x + grid.cell - 1,
                    grid.top + 1,
                    [led.r, led.g, led.b],
                );
            }
            lines.push(s);
        }
    }
    lines
}

/// Draw the side panel, `height` window pixels high, from line `scroll`
/// on; closed it is only its handle at the right. `lights` come under
/// the controls and scroll with them.
pub fn render_side(
    controls: &[Item],
    lights: &[LightGroup],
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
        for strip in light_lines(lights, x0, handle_x0 as u32, size) {
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
            &[],
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
        let p = render_side(&controls, &[], true, 0, 400, size);
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
        let p = render_side(&controls, &[], true, 0, height, size);
        assert_eq!(p.max_scroll, 11 - 4);
        assert_eq!(hit(&p.hits, 1, p.hits[0].x0).map(|h| h.id), Some(0));
        let down = render_side(&controls, &[], true, 3, height, size);
        assert_eq!(hit(&down.hits, 1, down.hits[0].x0).map(|h| h.id), Some(3));
        let far = render_side(&controls, &[], true, 99, height, size);
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
        let p = render_side(&controls, &[], true, 0, 400, size);
        assert!(p.hits.last().unwrap().row > p.hits[0].row, "positions wrap");
        assert!(p.hits.iter().all(|h| h.x1 <= p.handle_x0));
    }

    fn strip(name: &'static str, leds: &[Rgb]) -> LightGroup {
        LightGroup {
            name,
            leds: leds
                .iter()
                .map(|c| mimic_core::frame::Rgb8 {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                })
                .collect(),
        }
    }

    /// The pixel at (`x`, `y` within line `row`) of a panel picture.
    fn pixel(p: &Side, size: RasterHeight, row: usize, x: usize, y: usize) -> Rgb {
        let line = crate::footer::height(size) as usize;
        let i = ((row * line + y) * p.width as usize + x) * 4;
        [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2]]
    }

    #[test]
    fn lights_are_cells_under_the_controls() {
        let size = RasterHeight::Size16;
        let controls = items(&[ControlInfo::button(0, "RESET", false)]);
        const RED: Rgb = [255, 0, 0];
        const BLUE: Rgb = [0, 63, 255];
        const DARK: Rgb = [0, 0, 0];
        let lights = [strip("LED STRIP", &[RED, DARK, BLUE])];
        let p = render_side(&controls, &lights, true, 0, 400, size);
        let plain = render_side(&controls, &[], true, 0, 400, size);
        assert_eq!((p.width, p.height), (plain.width, plain.height));
        // Clicks are the controls' as before; the lamps take none.
        assert_eq!(p.hits, plain.hits);
        // Title, RESET, the group's name, one row of LEDs.
        let line = crate::footer::height(size) as usize;
        let row_bytes = p.width as usize * 4;
        assert_eq!(
            p.rgba[..2 * line * row_bytes],
            plain.rgba[..2 * line * row_bytes]
        );
        assert_ne!(
            p.rgba[2 * line * row_bytes..3 * line * row_bytes],
            plain.rgba[2 * line * row_bytes..3 * line * row_bytes],
            "the name is drawn"
        );
        assert_eq!(
            p.rgba[4 * line * row_bytes..],
            plain.rgba[4 * line * row_bytes..]
        );
        let x0 = char_width(size);
        let g = light_grid(3, p.handle_x0 - x0, size);
        assert_eq!((g.rows, g.per_row), (1, 3));
        assert!(g.cell >= 6 && g.pitch > g.cell && g.top + g.cell <= line);
        let mid = g.top + g.cell / 2;
        for (i, colour) in [RED, DARK, BLUE].into_iter().enumerate() {
            let x = x0 + i * g.pitch;
            // The outline all around, the colour inside, the gap after.
            assert_eq!(pixel(&p, size, 3, x, mid), LED_EDGE);
            assert_eq!(pixel(&p, size, 3, x + g.cell - 1, mid), LED_EDGE);
            assert_eq!(pixel(&p, size, 3, x + g.cell / 2, g.top), LED_EDGE);
            assert_eq!(
                pixel(&p, size, 3, x + g.cell / 2, g.top + g.cell - 1),
                LED_EDGE
            );
            assert_eq!(pixel(&p, size, 3, x + 1, g.top + 1), colour);
            assert_eq!(
                pixel(&p, size, 3, x + g.cell - 2, g.top + g.cell - 2),
                colour
            );
            assert_eq!(pixel(&p, size, 3, x + g.cell / 2, mid), colour);
            assert_eq!(pixel(&p, size, 3, x + g.cell, mid), BACKGROUND);
            assert_eq!(pixel(&p, size, 3, x + g.cell / 2, g.top - 1), BACKGROUND);
        }
        // Nothing after the last LED.
        assert_eq!(pixel(&p, size, 3, x0 + 3 * g.pitch + 1, mid), BACKGROUND);
        // A new colour is a new picture, the same colours the same one.
        let again = render_side(&controls, &lights, true, 0, 400, size);
        assert_eq!(again.rgba, p.rgba);
        let other = [strip("LED STRIP", &[RED, DARK, RED])];
        let changed = render_side(&controls, &other, true, 0, 400, size);
        assert_ne!(changed.rgba, p.rgba);
    }

    #[test]
    fn without_lights_the_panel_is_as_before() {
        let size = RasterHeight::Size16;
        let controls = items(&[
            ControlInfo::button(0, "POWER", false),
            ControlInfo::switch(3, "TV TYPE", &["COLOR", "B/W"], 1),
        ]);
        let plain = render_side(&controls, &[], true, 0, 300, size);
        // A group without LEDs is no group.
        let empty = render_side(&controls, &[strip("LED STRIP", &[])], true, 0, 300, size);
        assert_eq!(empty.rgba, plain.rgba);
        assert_eq!(empty.max_scroll, plain.max_scroll);
        assert_eq!(empty.hits, plain.hits);
        // The closed panel shows none.
        let lights = [strip("LED STRIP", &[[255, 0, 0]; 8])];
        let closed = render_side(&controls, &lights, false, 0, 300, size);
        assert_eq!(
            closed.rgba,
            render_side(&controls, &[], false, 0, 300, size).rgba
        );
    }

    #[test]
    fn many_lights_wrap_into_even_rows() {
        for size in [
            RasterHeight::Size16,
            RasterHeight::Size20,
            RasterHeight::Size24,
        ] {
            let x0 = char_width(size);
            let width = (open_width(size) - handle_width(size)) as usize - x0;
            let fit = width / light_grid(1, width, size).pitch;
            assert!((12..30).contains(&fit), "{fit} LEDs a row");
            // One more than fits: two rows, the longer first.
            let g = light_grid(fit + 1, width, size);
            assert_eq!((g.rows, g.per_row), (2, fit.div_ceil(2) + (fit + 1) % 2));
            assert_eq!(light_grid(fit, width, size).rows, 1);
            let g = light_grid(84, width, size);
            assert!(g.per_row <= fit && g.per_row * g.rows >= 84);
            assert!(g.per_row * (g.rows - 1) < 84);
            assert!(
                (g.per_row - 1) * g.pitch + g.cell <= width,
                "inside the column"
            );
        }
        let size = RasterHeight::Size16;
        // Two groups: the LEDs' colour tells where each one lies.
        let leds: Vec<Rgb> = (0..30).map(|i| [10 + i as u8, 200, 0]).collect();
        let lights = [
            strip("CASE TOP", &leds),
            strip("KEYBOARD", &[[0, 0, 255]; 5]),
        ];
        let p = render_side(&[], &lights, true, 0, 600, size);
        let x0 = char_width(size);
        let g = light_grid(30, p.handle_x0 - x0, size);
        assert_eq!((g.rows, g.per_row), (2, 15));
        let mid = g.top + g.cell / 2;
        let centre = |i: usize| x0 + i * g.pitch + g.cell / 2;
        // Title, "no console controls", name, two rows, name, one row.
        for (k, led) in leds.iter().enumerate() {
            assert_eq!(pixel(&p, size, 3 + k / 15, centre(k % 15), mid), *led);
        }
        assert_eq!(pixel(&p, size, 4, centre(15), mid), BACKGROUND);
        assert_eq!(pixel(&p, size, 6, centre(4), mid), [0, 0, 255]);
        assert_eq!(pixel(&p, size, 6, centre(5), mid), BACKGROUND);
        assert_eq!(pixel(&p, size, 7, centre(0), mid), BACKGROUND);
        // Every cell stays left of the handle.
        assert!(centre(14) + g.cell < p.handle_x0);
    }

    #[test]
    fn lights_scroll_with_the_panel() {
        let size = RasterHeight::Size16;
        let controls: Vec<Item> = (0..3)
            .map(|i| Item::from(&ControlInfo::button(i, "RESET", false)))
            .collect();
        let lights = [strip("LED STRIP", &[[255, 0, 0]; 40])];
        let x0 = char_width(size);
        let height = 4 * crate::footer::height(size);
        let p = render_side(&controls, &lights, true, 0, height, size);
        let g = light_grid(40, p.handle_x0 - x0, size);
        // Title, three buttons, the name, the LEDs' rows; four lines fit.
        assert_eq!(p.max_scroll, 5 + g.rows - 4);
        let mid = g.top + g.cell / 2;
        let x = x0 + g.cell / 2;
        assert!((0..4).all(|row| pixel(&p, size, row, x, mid) != [255, 0, 0]));
        // Scrolled to the end, the LEDs' rows are the last lines.
        let end = render_side(&controls, &lights, true, p.max_scroll, height, size);
        for row in 4 - g.rows.min(4)..4 {
            assert_eq!(pixel(&end, size, row, x, mid), [255, 0, 0]);
        }
        // The buttons moved up with them.
        let full = render_side(&controls, &lights, true, 0, 20 * height, size);
        assert_eq!(full.max_scroll, 0);
        for h in &end.hits {
            let at = full.hits.iter().find(|f| f.id == h.id).unwrap();
            assert_eq!(at.row, h.row + p.max_scroll);
        }
    }
}
