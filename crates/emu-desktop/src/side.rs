//! Media side panel (Shift+Alt+→ or a click on its handle): the machine's disk
//! drives, datasette and cartridge slot at the window's right edge, each
//! with its state, its image, one button that mounts an empty slot (green)
//! or ejects a full one (red), and one to save.
//!
//! The list comes from the core traits only (`media_slots`, through
//! [`crate::footer::Slot`]), so every machine gets the panel without a line
//! of its own; a machine without such a slot gets neither panel nor handle.
//! The panel is a picture at window resolution, drawn like the footer and
//! the console panel; closed, only a thin handle remains.

use emu_core::media::MediaKind;
use noto_sans_mono_bitmap::{get_raster_width, FontWeight, RasterHeight};

use crate::footer::{self, Rgb, Slot, Strip, BACKGROUND, BUSY, BUTTON, DIM, ERROR, TEXT};

/// Characters a line of the open panel holds.
const COLUMNS: usize = 34;
const HANDLE: Rgb = [0x30, 0x30, 0x30];
/// The toggle button: green mounts, red ejects.
const MOUNT: Rgb = [0x1e, 0x7a, 0x3c];
const EJECT: Rgb = [0xa8, 0x2c, 0x2c];
/// The pill on Save: unsaved changes.
const DIRTY: Rgb = [0xff, 0x30, 0x30];
const PILL_WIDTH: usize = 8;
const PILL_HEIGHT: usize = 5;
const TITLE: &str = "MEDIA";

/// What a click on the panel does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Mount(u8, MediaKind),
    Eject(u8),
    Save(u8),
}

/// A clickable area in panel pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub x0: usize,
    pub x1: usize,
    pub y0: usize,
    pub y1: usize,
    pub action: Action,
}

/// The panel as a picture.
pub struct Picture {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub hits: Vec<Hit>,
    /// The largest useful scroll offset, in lines.
    pub max_scroll: usize,
}

/// The action at panel pixel (`x`, `y`).
pub fn hit(hits: &[Hit], x: usize, y: usize) -> Option<Action> {
    hits.iter()
        .find(|h| (h.x0..h.x1).contains(&x) && (h.y0..h.y1).contains(&y))
        .map(|h| h.action)
}

fn char_width(size: RasterHeight) -> usize {
    get_raster_width(FontWeight::Regular, size)
}

/// Width of the closed panel: the handle, in window pixels.
pub fn handle_width(size: RasterHeight) -> u32 {
    (char_width(size) + 6) as u32
}

/// Width of the open panel, handle included.
pub fn open_width(size: RasterHeight) -> u32 {
    handle_width(size) + (char_width(size) * (COLUMNS + 2)) as u32
}

/// `text` in at most `max` characters, keeping both ends: the numbers of a
/// disk set are at the end of its file names.
fn fit(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    let tail = keep / 2;
    let head = keep - tail;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}...{end}")
}

fn state(s: &Slot) -> (String, Rgb) {
    let track = s.status.track.map(|t| format!(" T{t}")).unwrap_or_default();
    if s.status.error {
        (format!("ERROR{track}"), ERROR)
    } else if s.status.active {
        (format!("busy{track}"), BUSY)
    } else if s.inserted {
        (format!("idle{track}"), TEXT)
    } else {
        ("empty".into(), DIM)
    }
}

/// A small red pill from `x` on at the top of the line, its corners cut.
fn pill(strip: &mut Strip, x: usize) {
    for y in 1..1 + PILL_HEIGHT {
        let corner = y == 1 || y == PILL_HEIGHT;
        for px in x..x + PILL_WIDTH {
            if corner && (px == x || px == x + PILL_WIDTH - 1) {
                continue;
            }
            let i = (y * strip.width + px) * 4;
            strip.px[i..i + 3].copy_from_slice(&DIRTY);
        }
    }
}

/// One line of buttons: Mount or Eject by the slot's state, then Save, grey
/// and without a hit area on an empty slot.
fn buttons(strip: &mut Strip, x: usize, line: usize, s: &Slot, hits: &mut Vec<(usize, Hit)>) {
    let mut x = x;
    let toggle = if s.inserted {
        ("Eject", Action::Eject(s.id), true, EJECT)
    } else {
        ("Mount", Action::Mount(s.id, s.kind), true, MOUNT)
    };
    let row = [toggle, ("Save", Action::Save(s.id), s.inserted, BUTTON)];
    for (label, action, live, colour) in row {
        let text = format!(" {label} ");
        let w = strip.text_width(&text);
        strip.fill(x, x + w, 2, colour);
        strip.text(x, &text, if live { TEXT } else { DIM });
        if s.dirty && matches!(action, Action::Save(_)) {
            pill(strip, x + w - PILL_WIDTH);
        }
        if live {
            let hit = Hit {
                x0: x,
                x1: x + w,
                y0: 0,
                y1: 0,
                action,
            };
            hits.push((line, hit));
        }
        x += w + char_width(strip.size);
    }
}

/// Draw the panel, `height` window pixels high, from line `scroll` on.
pub fn render(
    slots: &[Slot],
    open: bool,
    scroll: usize,
    height: u32,
    size: RasterHeight,
) -> Picture {
    let handle = handle_width(size) as usize;
    let width = if open {
        open_width(size)
    } else {
        handle_width(size)
    };
    let line_height = footer::height(size) as usize;
    let visible = (height as usize / line_height.max(1)).max(1);
    let mut lines: Vec<Strip> = Vec::new();
    // Clickable areas by content line, before scrolling.
    let mut areas: Vec<(usize, Hit)> = Vec::new();
    if open {
        let x = handle + char_width(size);
        let mut title = Strip::new(width, size);
        title.text(x, TITLE, DIM);
        lines.push(title);
        for s in slots {
            let mut head = Strip::new(width, size);
            let after = head.text(x, s.name, TEXT);
            let (text, colour) = state(s);
            head.text(after + char_width(size), &text, colour);
            lines.push(head);
            let mut image = Strip::new(width, size);
            match (&s.file, s.inserted) {
                (Some(file), true) => image.text(x, &fit(file, COLUMNS), BUSY),
                (None, true) => image.text(x, "(inserted)", TEXT),
                _ => image.text(x, "-", DIM),
            };
            lines.push(image);
            let mut row = Strip::new(width, size);
            buttons(&mut row, x, lines.len(), s, &mut areas);
            lines.push(row);
        }
    }
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = scroll.min(max_scroll);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for strip in lines.iter().skip(scroll).take(visible) {
        rgba.extend_from_slice(&strip.px);
    }
    rgba.truncate(width as usize * height as usize * 4);
    while rgba.len() < width as usize * height as usize * 4 {
        rgba.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }
    // The handle over all of it, with an arrow where it opens or closes.
    let row_bytes = width as usize * 4;
    for line in rgba.chunks_exact_mut(row_bytes) {
        for x in 0..handle {
            line[x * 4..x * 4 + 3].copy_from_slice(&HANDLE);
        }
    }
    let mut arrow = Strip::new(handle as u32, size);
    arrow.fill(0, handle, 0, HANDLE);
    arrow.text(3, if open { ">" } else { "<" }, TEXT);
    for (y, line) in arrow.px.chunks_exact(handle * 4).enumerate() {
        if y < height as usize {
            rgba[y * row_bytes..y * row_bytes + handle * 4].copy_from_slice(line);
        }
    }
    let mut hits = vec![Hit {
        x0: 0,
        x1: handle,
        y0: 0,
        y1: height as usize,
        action: Action::Toggle,
    }];
    hits.extend(areas.into_iter().filter_map(|(line, h)| {
        let shown = line.checked_sub(scroll).filter(|&l| l < visible)?;
        Some(Hit {
            y0: shown * line_height,
            y1: (shown + 1) * line_height,
            ..h
        })
    }));
    Picture {
        rgba,
        width,
        height,
        hits,
        max_scroll,
    }
}

/// File name extensions the open dialog offers for a slot.
pub fn extensions(kind: MediaKind) -> &'static [&'static str] {
    match kind {
        MediaKind::Disk => &[
            "d64",
            "d71",
            "d81",
            "d80",
            "d82",
            "g64",
            "atr",
            "dsk",
            "adf",
            "dms",
            "do",
            "po",
            "nib",
            "woz",
            "img",
            "image",
            "dc42",
            "diskcopy42",
            "hda",
            "hdv",
            "hfv",
            "mdr",
        ],
        MediaKind::Tape => &["tap", "cas", "cdt", "tzx", "p", "81", "prg", "wav", "ptp", "uef"],
        MediaKind::Cards => &["deck", "crd", "chx"],
        MediaKind::BatteryRam => &["srm", "sav"],
        MediaKind::Program => &[
            "prg", "p00", "t64", "xex", "com", "exe", "sna", "z80", "bin", "hex",
        ],
        MediaKind::Cartridge => &[
            "crt", "car", "a26", "vec", "bin", "rom", "col", "nes", "sfc", "smc",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::DriveStatus;

    #[test]
    fn program_slots_offer_their_files() {
        assert!(extensions(MediaKind::Program).contains(&"hex"));
        assert!(extensions(MediaKind::Tape).contains(&"ptp"));
    }

    const SIZE: RasterHeight = RasterHeight::Size16;
    /// Lines of a slot: state, image, buttons.
    const SLOT_LINES: usize = 3;

    fn slot(id: u8, name: &'static str, file: Option<&str>) -> Slot {
        Slot {
            id,
            name,
            kind: MediaKind::Disk,
            inserted: file.is_some(),
            status: DriveStatus::default(),
            dirty: false,
            file: file.map(String::from),
        }
    }

    fn pixel(p: &Picture, x: usize, y: usize) -> Rgb {
        let i = (y * p.width as usize + x) * 4;
        [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2]]
    }

    fn line_y(line: usize) -> usize {
        line * footer::height(SIZE) as usize + 2
    }

    #[test]
    fn a_closed_panel_is_only_its_handle() {
        let p = render(&[slot(0, "drive 8", None)], false, 0, 300, SIZE);
        assert_eq!(p.width, handle_width(SIZE));
        assert_eq!(p.rgba.len(), (p.width * 300 * 4) as usize);
        assert_eq!(p.hits.len(), 1);
        assert_eq!(hit(&p.hits, 1, 299), Some(Action::Toggle));
    }

    #[test]
    fn buttons_act_on_their_own_slot() {
        let slots = [slot(3, "DF0:", Some("a.adf")), slot(10, "SCSI 0", None)];
        let p = render(&slots, true, 0, 400, SIZE);
        assert_eq!(p.rgba.len(), (p.width * 400 * 4) as usize);
        let x = handle_width(SIZE) as usize + char_width(SIZE) + 2;
        // Title, then three lines a slot.
        // A full slot's one button ejects, in red.
        assert_eq!(hit(&p.hits, x, line_y(3)), Some(Action::Eject(3)));
        assert_eq!(pixel(&p, x, line_y(3) + 3), EJECT);
        assert!(!p
            .hits
            .iter()
            .any(|h| h.action == Action::Mount(3, MediaKind::Disk)));
        assert!(
            p.hits.iter().any(|h| h.action == Action::Save(3)),
            "a mounted disk can be saved"
        );
        // An empty slot's mounts, in green; there is nothing to save.
        assert_eq!(
            hit(&p.hits, x, line_y(6)),
            Some(Action::Mount(10, MediaKind::Disk))
        );
        assert_eq!(pixel(&p, x, line_y(6) + 3), MOUNT);
        assert!(!p
            .hits
            .iter()
            .any(|h| matches!(h.action, Action::Eject(10) | Action::Save(10))));
    }

    #[test]
    fn a_dirty_disk_shows_a_pill_on_save() {
        let mut slots = [slot(3, "DF0:", Some("a.adf"))];
        let save = |slots: &[Slot]| {
            let p = render(slots, true, 0, 400, SIZE);
            let s = *p.hits.iter().find(|h| h.action == Action::Save(3)).unwrap();
            pixel(&p, s.x1 - PILL_WIDTH / 2, s.y0 + 2)
        };
        assert_ne!(save(&slots), DIRTY);
        slots[0].dirty = true;
        assert_eq!(save(&slots), DIRTY);
    }

    #[test]
    fn a_short_window_scrolls_and_clicks_follow() {
        let slots: Vec<Slot> = (0..9).map(|i| slot(i, "SCSI", None)).collect();
        let height = 10 * footer::height(SIZE);
        let p = render(&slots, true, 0, height, SIZE);
        assert_eq!(p.max_scroll, 1 + 9 * SLOT_LINES - 10);
        let x = handle_width(SIZE) as usize + char_width(SIZE) + 2;
        assert_eq!(
            hit(&render(&slots, true, 0, height, SIZE).hits, x, line_y(3)),
            Some(Action::Mount(0, MediaKind::Disk))
        );
        let down = render(&slots, true, 3, height, SIZE);
        assert_eq!(
            hit(&down.hits, x, line_y(3)),
            Some(Action::Mount(1, MediaKind::Disk))
        );
        // Past the end it stops at the last line.
        let far = render(&slots, true, 999, height, SIZE);
        assert_eq!(far.rgba.len(), (far.width * height * 4) as usize);
        assert!(far
            .hits
            .iter()
            .any(|h| h.action == Action::Mount(8, MediaKind::Disk)));
    }

    #[test]
    fn long_names_keep_both_ends() {
        assert_eq!(fit("short.adf", 34), "short.adf");
        let f = fit("SSW_6.0.8-800k_Disk1of4_with_a_long_name_too.dsk", 20);
        assert_eq!(f.chars().count(), 20);
        assert!(f.starts_with("SSW_6.0.") && f.ends_with("oo.dsk"), "{f}");
    }

    #[test]
    fn a_window_smaller_than_a_line_still_gives_a_whole_picture() {
        let p = render(&[slot(0, "drive 8", Some("a.d64"))], true, 0, 5, SIZE);
        assert_eq!(p.rgba.len(), (p.width * 5 * 4) as usize);
    }
}
