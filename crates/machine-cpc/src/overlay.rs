//! Debug overlays of the CPC (see `emu_core::overlay`), on the Gate
//! Array's picture:
//!
//! - `crtc`: a mark where a CRTC register write (`R7=1E`) or a Gate Array
//!   ink, mode or RAM configuration write (`INK 3=14`, `BORDER=14`,
//!   `MODE 1`, `RAM 4`) happened, at the monitor beam's position at that
//!   microsecond (16 picture pixels per microsecond).
//! - `vsync`: a band over the picture rows the monitor drew while the
//!   CRTC's VSYNC was active (normally none: the monitor starts its field
//!   36 lines after VSYNC, below the picture; they show when it is not
//!   locked to the CRTC, e.g. while a program moves R7).
//!
//! The record is not machine state: it exists only while an overlay is
//! switched on, is not saved, and costs the video microsecond and each
//! I/O write one test of an `Option` when off. Double buffered: the last
//! complete field while the beam is above the picture's first row, the
//! field so far once it has passed it.

use chips_amstrad::gate_array::HEIGHT;
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

/// Marks kept per field.
const MAX_MARKS: usize = 2048;
/// Monitor lines per field (free-running).
const FIELD_LINES: usize = 312;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Field {
    /// (monitor line, microsecond, label, from the Gate Array).
    marks: Vec<(u16, u8, String, bool)>,
    vsync: Vec<bool>,
}

impl Field {
    fn new() -> Self {
        Self {
            marks: Vec::new(),
            vsync: vec![false; FIELD_LINES],
        }
    }
}

/// The overlays' record (not machine state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    crtc: bool,
    vsync: bool,
    fields: u64,
    current: Field,
    last: Field,
}

pub fn overlays() -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "crtc",
            name: "CRTC and Gate Array writes".into(),
        },
        OverlayInfo {
            id: "vsync",
            name: "CRTC VSYNC".into(),
        },
    ]
}

/// The record for `ids`, `None` when none of them is one of ours.
pub fn record(ids: &[&str], old: Option<Box<Record>>) -> Option<Box<Record>> {
    let (crtc, vsync) = (ids.contains(&"crtc"), ids.contains(&"vsync"));
    if !(crtc || vsync) {
        return None;
    }
    let mut r = old.unwrap_or_else(|| {
        Box::new(Record {
            crtc,
            vsync,
            fields: 0,
            current: Field::new(),
            last: Field::new(),
        })
    });
    r.crtc = crtc;
    r.vsync = vsync;
    Some(r)
}

impl Record {
    /// One video microsecond: the monitor beam (line, µs, fields) after
    /// it, and the CRTC's VSYNC in it.
    pub fn tick(&mut self, beam: (u16, u8, u64), vsync: bool) {
        if beam.2 != self.fields {
            self.fields = beam.2;
            std::mem::swap(&mut self.current, &mut self.last);
            self.current.marks.clear();
            self.current.vsync.fill(false);
        }
        if vsync && self.vsync {
            if let Some(v) = self.current.vsync.get_mut(usize::from(beam.0)) {
                *v = true;
            }
        }
    }

    /// A register write labelled `label` at monitor position `beam`.
    pub fn mark(&mut self, beam: (u16, u8, u64), label: String, gate_array: bool) {
        if self.crtc && self.current.marks.len() < MAX_MARKS {
            self.current.marks.push((beam.0, beam.1, label, gate_array));
        }
    }

    /// Overlay `id`; `line` is the monitor line the beam is on now.
    pub fn overlay(&self, id: &str, line: u16) -> Option<Overlay> {
        let info = overlays().into_iter().find(|o| o.id == id)?;
        let field = if line == 0 { &self.last } else { &self.current };
        let (shapes, legend) = match id {
            "crtc" if self.crtc => {
                let shapes = field
                    .marks
                    .iter()
                    .map(|(y, us, label, ga)| Shape::Mark {
                        x: i32::from(*us) * 16,
                        y: i32::from(*y),
                        color: if *ga { colors::CYAN } else { colors::YELLOW },
                        label: Some(label.clone()),
                    })
                    .collect();
                let legend = vec![
                    LegendEntry {
                        color: colors::YELLOW,
                        text: "CRTC register".into(),
                    },
                    LegendEntry {
                        color: colors::CYAN,
                        text: "Gate Array".into(),
                    },
                ];
                (shapes, legend)
            }
            "vsync" if self.vsync => {
                let mut shapes = Vec::new();
                let rows = &field.vsync[..HEIGHT as usize];
                let mut y = 0;
                while y < rows.len() {
                    if rows[y] {
                        let start = y;
                        while y < rows.len() && rows[y] {
                            y += 1;
                        }
                        shapes.push(Shape::Band {
                            y: start as i32,
                            h: (y - start) as u32,
                            color: colors::MAGENTA,
                            label: Some("VSYNC".into()),
                        });
                    }
                    y += 1;
                }
                let legend = vec![LegendEntry {
                    color: colors::MAGENTA,
                    text: "VSYNC".into(),
                }];
                (shapes, legend)
            }
            _ => return None,
        };
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend,
        })
    }
}
