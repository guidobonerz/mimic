//! Debug overlay of the Atari 2600 (see `emu_core::overlay`): where in the
//! picture the program strobed the TIA's position registers (RESP0, RESP1,
//! RESM0, RESM1, RESBL) and HMOVE in the last field. Recorded only while
//! switched on; the record is debugger state, not machine state (not in
//! savestates, ignored by comparisons).
//!
//! Frame coordinates: the frame is assembled from VSYNC, so TIA line `l`
//! is frame row `l` (up to the field height of the standard), and colour
//! clock `cc` is column `cc − 68` (68 clocks of horizontal blank; Stella
//! Programmer's Guide, "Horizontal timing"). A strobe during horizontal
//! blank is marked at column 0, like the beam position.

use chips_atari::tia::{self, HMOVE, RESBL, RESM0, RESM1, RESP0, RESP1};
use emu_core::frame::{Rgb8, VideoStandard};
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

pub const OBJECTS: &str = "objects";

/// Strobes marked, with their colours.
const STROBES: [(u8, &str, Rgb8); 6] = [
    (RESP0, "RESP0", colors::RED),
    (RESP1, "RESP1", colors::GREEN),
    (RESM0, "RESM0", colors::BLUE),
    (RESM1, "RESM1", colors::YELLOW),
    (RESBL, "RESBL", colors::CYAN),
    (HMOVE, "HMOVE", colors::GREY),
];

pub fn infos() -> Vec<OverlayInfo> {
    vec![OverlayInfo {
        id: OBJECTS,
        name: "TIA position strobes and HMOVE".into(),
    }]
}

/// The frame row of TIA line `line`.
pub fn frame_row(standard: VideoStandard, line: u16) -> Option<u32> {
    (u32::from(line) < tia::height(standard)).then_some(u32::from(line))
}

/// One strobe: TIA line, colour clock, register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Strobe {
    line: u16,
    clock: u8,
    reg: u8,
}

/// The record; equal to any other so that machine-state comparisons
/// ignore it.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    cur: Vec<Strobe>,
    last: Vec<Strobe>,
}

impl PartialEq for Recorder {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Recorder {}

impl Recorder {
    pub fn new(ids: &[&str]) -> Option<Self> {
        ids.contains(&OBJECTS).then(Recorder::default)
    }

    /// A write to TIA register `reg` at the beam position.
    pub fn write(&mut self, reg: u8, line: u16, clock: u8) {
        if STROBES.iter().any(|s| s.0 == reg) {
            self.cur.push(Strobe { line, clock, reg });
        }
    }

    pub fn end_field(&mut self) {
        self.last = std::mem::take(&mut self.cur);
    }

    pub fn clear(&mut self) {
        self.cur.clear();
        self.last.clear();
    }

    /// The last complete field, or the field so far (`mid_field`).
    pub fn overlay(&self, id: &str, mid_field: bool) -> Option<Overlay> {
        if id != OBJECTS {
            return None;
        }
        let rec = if mid_field { &self.cur } else { &self.last };
        let shapes = rec
            .iter()
            .filter_map(|s| {
                let &(_, name, color) = STROBES.iter().find(|t| t.0 == s.reg)?;
                Some(Shape::Mark {
                    x: i32::from(s.clock.saturating_sub(tia::HBLANK)),
                    y: i32::from(s.line),
                    color,
                    label: Some(name.into()),
                })
            })
            .collect();
        let legend = STROBES
            .iter()
            .map(|&(_, name, color)| LegendEntry {
                color,
                text: name.into(),
            })
            .collect();
        let info = &infos()[0];
        Some(Overlay {
            id: info.id,
            name: info.name.clone(),
            shapes,
            legend,
        })
    }
}
