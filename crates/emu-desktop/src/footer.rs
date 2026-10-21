//! Status footer (Ctrl+Space): mouse pointer, joystick directions and drive
//! activity under the picture.
//!
//! The emulation thread collects joystick and drive state through the core
//! traits only (`ports`, `media_slots`, `MediaSlot::status`); the window adds
//! the mouse and draws the text in Noto Sans Mono (pre-rasterized,
//! anti-aliased glyphs from `noto-sans-mono-bitmap`, MIT crate, SIL Open
//! Font License font) into an RGBA strip at window resolution.

use std::collections::BTreeMap;

use emu_core::input::{Dirs, InputEvent, InputTarget, TimedInput};
use emu_core::media::{DriveStatus, MediaKind};
use emu_core::port::{PortId, PortKind};
use emu_core::Machine;
use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

/// Space above and below the text, in window pixels.
const PAD: usize = 3;

/// Font size for a window with this scale factor (1.0 = 96 dpi).
pub fn font_size(scale_factor: f64) -> RasterHeight {
    if scale_factor < 1.25 {
        RasterHeight::Size16
    } else if scale_factor < 1.75 {
        RasterHeight::Size20
    } else {
        RasterHeight::Size24
    }
}

/// Strip height in window pixels for `size`.
pub fn height(size: RasterHeight) -> u32 {
    (size.val() + 2 * PAD) as u32
}

/// Frames an error stays shown after the drive last signalled it (a 1541
/// flashes its LED, so the signal comes and goes).
const ERROR_HOLD: u32 = 100;
/// Frames activity stays shown, so short accesses are visible.
const ACTIVE_HOLD: u32 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Joy {
    pub port: &'static str,
    pub dirs: Dirs,
    pub buttons: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drive {
    pub name: &'static str,
    pub inserted: bool,
    pub status: DriveStatus,
}

/// What the emulation thread publishes for the footer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Footer {
    pub joys: Vec<Joy>,
    pub drives: Vec<Drive>,
}

/// Where the mouse is, from the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mouse {
    /// Not over the picture.
    #[default]
    Outside,
    /// Pointer over the picture, in frame pixels (border included).
    At(u32, u32),
    /// Captured for the machine: movement summed since the capture.
    Captured(i32, i32),
}

/// Emulation-thread side: remembers the last joystick event per port and
/// holds short drive signals long enough to be seen.
#[derive(Default)]
pub struct Tracker {
    joys: BTreeMap<u8, (Dirs, u8)>,
    /// Ports that carry a mouse or paddles (no joystick shown).
    analog: BTreeMap<u8, ()>,
    /// Per slot: frames left to show activity and an error.
    hold: BTreeMap<u8, (u32, u32)>,
}

impl Tracker {
    /// Note an input event on its way to the machine.
    pub fn observe(&mut self, e: &TimedInput) {
        let InputTarget::Port(PortId(port)) = e.target else {
            return;
        };
        match e.event {
            InputEvent::Digital { dirs, buttons } => {
                self.joys.insert(port, (dirs, buttons));
            }
            InputEvent::MouseRel { .. } | InputEvent::Analog { .. } => {
                self.analog.insert(port, ());
            }
            _ => {}
        }
    }

    /// The footer after a frame.
    pub fn snapshot(&mut self, m: &mut dyn Machine) -> Footer {
        let joys = m
            .ports()
            .into_iter()
            .filter(|p| {
                p.connected
                    && matches!(
                        p.kind,
                        PortKind::CbmControl
                            | PortKind::AmigaGameport
                            | PortKind::AtariJoy
                            | PortKind::CpcJoy
                    )
                    && !self.analog.contains_key(&p.id.0)
            })
            .map(|p| {
                let (dirs, buttons) = self.joys.get(&p.id.0).copied().unwrap_or_default();
                Joy {
                    port: p.name,
                    dirs,
                    buttons,
                }
            })
            .collect();
        let mut drives = Vec::new();
        for info in m.media_slots() {
            if !matches!(info.kind, MediaKind::Disk | MediaKind::Tape) {
                continue;
            }
            let now = m
                .media_slot(info.id)
                .map(|s| s.status())
                .unwrap_or_default();
            drives.push(Drive {
                name: info.name,
                inserted: info.inserted,
                status: held(self.hold.entry(info.id.0).or_default(), now),
            });
        }
        Footer { joys, drives }
    }
}

/// `now` with activity and errors held for a few frames; `hold` counts the
/// frames left for each.
fn held(hold: &mut (u32, u32), now: DriveStatus) -> DriveStatus {
    hold.0 = if now.active {
        ACTIVE_HOLD
    } else {
        hold.0.saturating_sub(1)
    };
    hold.1 = if now.error {
        ERROR_HOLD
    } else {
        hold.1.saturating_sub(1)
    };
    DriveStatus {
        active: hold.0 > 0,
        error: hold.1 > 0,
        track: now.track,
    }
}

pub type Rgb = [u8; 3];

const TEXT: Rgb = [0xC8, 0xC8, 0xC8];
const DIM: Rgb = [0x70, 0x70, 0x70];
const BUSY: Rgb = [0x50, 0xE0, 0x50];
const ERROR: Rgb = [0xFF, 0x50, 0x50];
const BACKGROUND: Rgb = [0x18, 0x18, 0x18];

fn joy_text(j: &Joy) -> String {
    let mut parts = Vec::new();
    for (d, name) in [
        (Dirs::UP, "up"),
        (Dirs::DOWN, "down"),
        (Dirs::LEFT, "left"),
        (Dirs::RIGHT, "right"),
    ] {
        if j.dirs.contains(d) {
            parts.push(name);
        }
    }
    if j.buttons != 0 {
        parts.push("fire");
    }
    if parts.is_empty() {
        parts.push("-");
    }
    format!("{}: {}", j.port, parts.join(" "))
}

/// The footer as coloured text segments.
pub fn segments(f: &Footer, mouse: Mouse) -> Vec<(String, Rgb)> {
    let mut out = vec![(
        match mouse {
            Mouse::Outside => "mouse -".to_string(),
            Mouse::At(x, y) => format!("mouse {x},{y}"),
            Mouse::Captured(x, y) => format!("mouse captured {x},{y}"),
        },
        TEXT,
    )];
    for j in &f.joys {
        let colour = if j.dirs.0 != 0 || j.buttons != 0 {
            BUSY
        } else {
            TEXT
        };
        out.push((joy_text(j), colour));
    }
    for d in &f.drives {
        let track = d.status.track.map(|t| format!(" T{t}")).unwrap_or_default();
        let (state, colour) = if d.status.error {
            ("ERROR", ERROR)
        } else if d.status.active {
            ("busy", BUSY)
        } else if d.inserted {
            ("idle", TEXT)
        } else {
            ("empty", DIM)
        };
        out.push((format!("{}: {state}{track}", d.name), colour));
    }
    out
}

/// Draw `segments`, separated by " | ", into an RGBA strip `width` pixels
/// wide and [`height`]`(size)` high; text beyond the width is cut off.
pub fn render(segments: &[(String, Rgb)], width: u32, size: RasterHeight) -> Vec<u8> {
    let width = width.max(1) as usize;
    let rows = height(size) as usize;
    let mut px = Vec::with_capacity(width * rows * 4);
    for _ in 0..width * rows {
        px.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }
    let advance = get_raster_width(FontWeight::Regular, size);
    let mut x = size.val() / 2;
    let mut put = |text: &str, colour: Rgb, x: &mut usize| {
        for ch in text.chars() {
            let Some(glyph) = get_raster(ch, FontWeight::Regular, size)
                .or_else(|| get_raster('?', FontWeight::Regular, size))
            else {
                continue;
            };
            for (row, line) in glyph.raster().iter().enumerate() {
                for (col, &a) in line.iter().enumerate() {
                    let gx = *x + col;
                    if a == 0 || gx >= width {
                        continue;
                    }
                    // Glyph intensity blends the colour over the background.
                    let i = ((row + PAD) * width + gx) * 4;
                    for c in 0..3 {
                        let (fg, bg) = (u16::from(colour[c]), u16::from(px[i + c]));
                        let a = u16::from(a);
                        px[i + c] = ((fg * a + bg * (255 - a)) / 255) as u8;
                    }
                }
            }
            *x += advance;
        }
    };
    for (i, (text, colour)) in segments.iter().enumerate() {
        if i > 0 {
            put(" | ", DIM, &mut x);
        }
        put(text, *colour, &mut x);
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycle;

    #[test]
    fn segments_show_mouse_joystick_and_drives() {
        let f = Footer {
            joys: vec![Joy {
                port: "control port 2",
                dirs: Dirs(Dirs::UP.0 | Dirs::RIGHT.0),
                buttons: 1,
            }],
            drives: vec![
                Drive {
                    name: "drive 8",
                    inserted: true,
                    status: DriveStatus {
                        active: true,
                        error: false,
                        track: Some(18),
                    },
                },
                Drive {
                    name: "drive 9",
                    inserted: true,
                    status: DriveStatus {
                        active: false,
                        error: true,
                        track: None,
                    },
                },
                Drive {
                    name: "tape",
                    inserted: false,
                    status: DriveStatus::default(),
                },
            ],
        };
        let s = segments(&f, Mouse::At(12, 34));
        let texts: Vec<&str> = s.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "mouse 12,34",
                "control port 2: up right fire",
                "drive 8: busy T18",
                "drive 9: ERROR",
                "tape: empty"
            ]
        );
        assert_eq!(s[2].1, BUSY);
        assert_eq!(s[3].1, ERROR);
        assert_eq!(segments(&f, Mouse::Outside)[0].0, "mouse -");
    }

    #[test]
    fn render_draws_text_and_clips() {
        let size = RasterHeight::Size16;
        let px = render(&[("I".into(), TEXT)], 20, size);
        assert_eq!(px.len(), 20 * height(size) as usize * 4);
        assert!(
            px.chunks(4).any(|p| p[0] > BACKGROUND[0] + 0x40),
            "glyph pixels drawn"
        );
        assert!(px.chunks(4).all(|p| p[3] == 255));
        assert_eq!(
            render(&[("I".into(), TEXT)], 4, size)[..4],
            [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255],
            "glyph beyond the width is cut off"
        );
    }

    #[test]
    fn tracker_holds_errors_and_follows_joystick() {
        let mut config = emu_core::registry::MachineConfig::default();
        config.options.insert("roms".into(), "stub".into());
        let mut m = emu_machines::registry().create("c64", &config).unwrap();
        let mut t = Tracker::default();
        let port = m
            .ports()
            .into_iter()
            .find(|p| p.connected && p.kind == PortKind::CbmControl)
            .expect("a joystick port");
        t.observe(&TimedInput {
            at: Cycle(0),
            target: InputTarget::Port(port.id),
            event: InputEvent::Digital {
                dirs: Dirs::LEFT,
                buttons: 0,
            },
        });
        let f = t.snapshot(m.as_mut());
        let j = f.joys.iter().find(|j| j.port == port.name).unwrap();
        assert_eq!(j.dirs, Dirs::LEFT);
    }

    #[test]
    fn errors_and_activity_are_held() {
        let mut hold = (0, 0);
        let flash = DriveStatus {
            active: true,
            error: true,
            track: Some(1),
        };
        assert_eq!(held(&mut hold, flash), flash);
        let off = DriveStatus::default();
        for _ in 1..ACTIVE_HOLD {
            assert!(held(&mut hold, off).active);
        }
        let s = held(&mut hold, off);
        assert!(!s.active && s.error);
        for _ in ACTIVE_HOLD + 1..ERROR_HOLD {
            held(&mut hold, off);
        }
        assert!(!held(&mut hold, off).error);
    }
}
