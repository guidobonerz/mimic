//! Status footer (Ctrl+Space): emulated time and frame count, mouse
//! pointer, joystick directions and drive activity under the picture.
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
    /// Emulated time since power-on in milliseconds, and the frame count
    /// it comes from (to name a moment when something hangs).
    pub elapsed_ms: u64,
    pub frame: u64,
    pub joys: Vec<Joy>,
    pub drives: Vec<Drive>,
}

/// `elapsed_ms` as `m:ss.t`.
pub fn clock(elapsed_ms: u64) -> String {
    let tenths = elapsed_ms / 100;
    format!("{}:{:02}.{}", tenths / 600, (tenths / 10) % 60, tenths % 10)
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

    /// The footer after a frame; `frame_ms` is the emulated length of a
    /// frame in milliseconds.
    pub fn snapshot(&mut self, m: &mut dyn Machine, frame_ms: f64) -> Footer {
        let frame = m.beam().map_or(0, |b| b.frame);
        let elapsed_ms = (frame as f64 * frame_ms) as u64;
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
        Footer {
            elapsed_ms,
            frame,
            joys,
            drives,
        }
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

pub const TEXT: Rgb = [0xC8, 0xC8, 0xC8];
pub const DIM: Rgb = [0x70, 0x70, 0x70];
pub const BUSY: Rgb = [0x50, 0xE0, 0x50];
const ERROR: Rgb = [0xFF, 0x50, 0x50];
pub const BACKGROUND: Rgb = [0x18, 0x18, 0x18];

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
    let mut out = vec![(format!("{} f{}", clock(f.elapsed_ms), f.frame), TEXT)];
    out.push((
        match mouse {
            Mouse::Outside => "mouse -".to_string(),
            Mouse::At(x, y) => format!("mouse {x},{y}"),
            Mouse::Captured(x, y) => format!("mouse captured {x},{y}"),
        },
        TEXT,
    ));
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

/// The speed slider's steps in percent of real time (a host too slow for a
/// step runs as fast as it can).
pub const SPEEDS: [u32; 8] = [10, 25, 50, 100, 150, 200, 400, 800];
/// The index of 100 % in [`SPEEDS`].
pub const NORMAL_SPEED: usize = 3;

/// Percent of real time for slider step `index` (out of range: 100).
pub fn speed_percent(index: u8) -> u32 {
    SPEEDS.get(usize::from(index)).copied().unwrap_or(100)
}

fn speed_label(index: usize) -> String {
    format!("{}%", SPEEDS[index])
}

/// Where the speed slider's track lies in the footer (window pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slider {
    pub x0: usize,
    pub x1: usize,
}

impl Slider {
    /// The step under column `x`, clamped to the track (for dragging).
    pub fn step_at(&self, x: f64) -> usize {
        let w = (self.x1 - self.x0).max(1) as f64;
        let t = ((x - self.x0 as f64) / w).clamp(0.0, 0.999);
        (t * SPEEDS.len() as f64) as usize
    }

    pub fn contains(&self, x: f64) -> bool {
        x >= self.x0 as f64 && x < self.x1 as f64
    }
}

/// Knob and track colours of the speed slider.
const TRACK: Rgb = [0x50, 0x50, 0x50];
const KNOB: Rgb = [0x2E, 0x8B, 0x4A];

/// Draw the speed slider (step `speed` of [`SPEEDS`]) and then `segments`,
/// separated by " | ", into an RGBA strip `width` pixels wide and
/// [`height`]`(size)` high; text beyond the width is cut off. Returns the
/// pixels and the slider's track.
pub fn render(
    segments: &[(String, Rgb)],
    speed: usize,
    width: u32,
    size: RasterHeight,
) -> (Vec<u8>, Slider) {
    let mut strip = Strip::new(width, size);
    let mut x = size.val() / 2;
    x = strip.text(x, "speed ", DIM);
    // One cell (two characters wide) per step: a thin track, the current
    // step as a knob.
    let cell = strip.text_width("  ");
    let rows = height(size) as usize;
    let slider = Slider {
        x0: x,
        x1: x + cell * SPEEDS.len(),
    };
    strip.fill(slider.x0, slider.x1, rows / 2 - 1, TRACK);
    for i in 0..SPEEDS.len() {
        // Tick marks at every step.
        let tx = slider.x0 + i * cell + cell / 2;
        strip.fill(tx, tx + 1, rows / 2 - 3, TRACK);
    }
    let knob = slider.x0 + speed.min(SPEEDS.len() - 1) * cell;
    strip.fill(knob + 2, knob + cell - 2, 3, KNOB);
    x = slider.x1;
    let label = speed_label(speed.min(SPEEDS.len() - 1));
    let colour = if speed == NORMAL_SPEED { TEXT } else { BUSY };
    x = strip.text(x, &format!(" {label:<4}"), colour);
    for (text, colour) in segments {
        x = strip.text(x, " | ", DIM);
        x = strip.text(x, text, *colour);
    }
    (strip.px, slider)
}

/// An RGBA strip one text line high at window resolution.
pub struct Strip {
    pub width: usize,
    pub size: RasterHeight,
    pub px: Vec<u8>,
}

impl Strip {
    pub fn new(width: u32, size: RasterHeight) -> Self {
        let width = width.max(1) as usize;
        let rows = height(size) as usize;
        let mut px = Vec::with_capacity(width * rows * 4);
        for _ in 0..width * rows {
            px.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
        }
        Self { width, size, px }
    }

    /// Width of `text` in pixels.
    pub fn text_width(&self, text: &str) -> usize {
        text.chars().count() * get_raster_width(FontWeight::Regular, self.size)
    }

    /// Fill columns `x0..x1` of the strip, `inset` rows from top and bottom.
    pub fn fill(&mut self, x0: usize, x1: usize, inset: usize, colour: Rgb) {
        let rows = self.px.len() / 4 / self.width;
        for y in inset..rows.saturating_sub(inset) {
            for x in x0..x1.min(self.width) {
                let i = (y * self.width + x) * 4;
                self.px[i..i + 3].copy_from_slice(&colour);
            }
        }
    }

    /// Draw `text` from `x` on; returns the x after it. Text beyond the
    /// width is cut off.
    pub fn text(&mut self, mut x: usize, text: &str, colour: Rgb) -> usize {
        let advance = get_raster_width(FontWeight::Regular, self.size);
        for ch in text.chars() {
            let Some(glyph) = get_raster(ch, FontWeight::Regular, self.size)
                .or_else(|| get_raster('?', FontWeight::Regular, self.size))
            else {
                continue;
            };
            for (row, line) in glyph.raster().iter().enumerate() {
                for (col, &a) in line.iter().enumerate() {
                    let gx = x + col;
                    if a == 0 || gx >= self.width {
                        continue;
                    }
                    // Glyph intensity blends the colour over what is there.
                    let i = ((row + PAD) * self.width + gx) * 4;
                    for (dst, &fg) in self.px[i..i + 3].iter_mut().zip(&colour) {
                        let (fg, bg, a) = (u16::from(fg), u16::from(*dst), u16::from(a));
                        *dst = ((fg * a + bg * (255 - a)) / 255) as u8;
                    }
                }
            }
            x += advance;
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycle;

    #[test]
    fn segments_show_mouse_joystick_and_drives() {
        let f = Footer {
            elapsed_ms: 95_420,
            frame: 4772,
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
                "1:35.4 f4772",
                "mouse 12,34",
                "control port 2: up right fire",
                "drive 8: busy T18",
                "drive 9: ERROR",
                "tape: empty"
            ]
        );
        assert_eq!(s[3].1, BUSY);
        assert_eq!(s[4].1, ERROR);
        assert_eq!(segments(&f, Mouse::Outside)[1].0, "mouse -");
        assert_eq!(clock(0), "0:00.0");
        assert_eq!(clock(3_599_999), "59:59.9");
        assert_eq!(clock(3_600_000), "60:00.0");
    }

    #[test]
    fn render_draws_text_and_clips() {
        let size = RasterHeight::Size16;
        let (px, _) = render(&[("I".into(), TEXT)], NORMAL_SPEED, 400, size);
        assert_eq!(px.len(), 400 * height(size) as usize * 4);
        assert!(
            px.chunks(4).any(|p| p[0] > BACKGROUND[0] + 0x40),
            "glyph pixels drawn"
        );
        assert!(px.chunks(4).all(|p| p[3] == 255));
        assert_eq!(
            render(&[], NORMAL_SPEED, 4, size).0[..4],
            [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255],
            "glyph beyond the width is cut off"
        );
    }

    #[test]
    fn speed_slider_steps_follow_the_pointer() {
        let size = RasterHeight::Size16;
        let (px, s) = render(&[], NORMAL_SPEED, 600, size);
        assert!(s.x1 > s.x0);
        let cell = (s.x1 - s.x0) / SPEEDS.len();
        // The knob sits in the cell of the current step.
        let row = (height(size) as usize / 2) * 600 * 4;
        let at = |x: usize| &px[row + x * 4..row + x * 4 + 3];
        assert_eq!(at(s.x0 + NORMAL_SPEED * cell + cell / 2), KNOB);
        assert_ne!(at(s.x0 + cell / 2 - 2), KNOB);
        // Clicks map to steps; dragging past the ends clamps.
        assert_eq!(s.step_at((s.x0 + cell / 2) as f64), 0);
        assert_eq!(s.step_at((s.x0 + 5 * cell + 1) as f64), 5);
        assert_eq!(s.step_at(0.0), 0);
        assert_eq!(s.step_at(1e6), SPEEDS.len() - 1);
        assert!(s.contains((s.x0 + 1) as f64) && !s.contains(s.x1 as f64));
        assert_eq!(speed_percent(NORMAL_SPEED as u8), 100);
        assert_eq!(speed_percent(7), 800, "top step");
        assert_eq!(speed_percent(99), 100);
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
        let f = t.snapshot(m.as_mut(), 20.0);
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
