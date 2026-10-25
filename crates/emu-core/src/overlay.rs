//! Debug overlays: what a video chip did where in the picture (bad lines,
//! sprite boxes, display-list mode lines, CRTC register changes, DMA slot
//! use), as shapes in the pixel coordinates of the machine's
//! [`crate::FrameRef`]. Machines describe them through
//! [`crate::Machine::overlays`] and [`crate::Machine::overlay`]; the preview
//! draws them on the raw frame only (golden rule 9). The beam marker and the
//! raster grid need no machine code beyond [`crate::Machine::frame_row`].

use crate::frame::Rgb8;

/// An overlay a machine offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayInfo {
    /// Short id for protocols and URLs ("badlines", "sprites", "dma").
    pub id: &'static str,
    pub name: String,
}

/// One element of an overlay. Coordinates are frame pixels (x right, y
/// down; rows of the main screen); shapes may reach outside the frame and
/// are clipped when drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A rectangle outline, e.g. a sprite's box, with an optional label.
    Rect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: Rgb8,
        label: Option<String>,
    },
    /// A row band from `y` over `h` rows (one marked raster line, a run of
    /// display-list mode lines), tinted, with an optional label at its left.
    Band {
        y: i32,
        h: u32,
        color: Rgb8,
        label: Option<String>,
    },
    /// A tick at one point of a row, e.g. where in a line a register
    /// changed.
    Mark {
        x: i32,
        y: i32,
        color: Rgb8,
        label: Option<String>,
    },
    /// A map of cells, e.g. DMA slots per line and cycle: `cols` × `rows`
    /// cells of `cell_w` × `cell_h` pixels from `(x, y)`, each a colour
    /// index into the overlay's legend (`u8::MAX` = nothing drawn).
    Cells {
        x: i32,
        y: i32,
        cell_w: u32,
        cell_h: u32,
        cols: u32,
        rows: u32,
        cells: Vec<u8>,
    },
}

/// A legend entry: what a colour means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegendEntry {
    pub color: Rgb8,
    pub text: String,
}

/// A decoded overlay for the current frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub id: &'static str,
    pub name: String,
    pub shapes: Vec<Shape>,
    /// Colours used by the shapes; [`Shape::Cells`] indexes into it.
    pub legend: Vec<LegendEntry>,
}

/// Colours overlays share, so they read alike on every machine.
pub mod colors {
    use crate::frame::Rgb8;

    pub const RED: Rgb8 = Rgb8 {
        r: 255,
        g: 64,
        b: 64,
    };
    pub const GREEN: Rgb8 = Rgb8 {
        r: 64,
        g: 230,
        b: 64,
    };
    pub const BLUE: Rgb8 = Rgb8 {
        r: 80,
        g: 140,
        b: 255,
    };
    pub const YELLOW: Rgb8 = Rgb8 {
        r: 255,
        g: 230,
        b: 0,
    };
    pub const CYAN: Rgb8 = Rgb8 {
        r: 0,
        g: 230,
        b: 230,
    };
    pub const MAGENTA: Rgb8 = Rgb8 {
        r: 255,
        g: 64,
        b: 255,
    };
    pub const ORANGE: Rgb8 = Rgb8 {
        r: 255,
        g: 150,
        b: 0,
    };
    pub const WHITE: Rgb8 = Rgb8 {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const GREY: Rgb8 = Rgb8 {
        r: 140,
        g: 140,
        b: 140,
    };
    /// Distinct colours in order, for numbered things (sprites 0–7 …).
    pub const SERIES: [Rgb8; 8] = [RED, GREEN, BLUE, YELLOW, CYAN, MAGENTA, ORANGE, WHITE];
}

/// Which overlays the frontends want recorded, shared between everyone who
/// asks (debugger, preview viewers): each id with how many want it. The
/// machine records the union; [`OverlayWants::apply`] tells it.
#[derive(Clone, Debug, Default)]
pub struct OverlayWants(std::sync::Arc<std::sync::Mutex<WantsInner>>);

#[derive(Debug, Default)]
struct WantsInner {
    counts: std::collections::BTreeMap<String, u32>,
    /// The set last passed to the machine.
    applied: Vec<String>,
}

impl OverlayWants {
    fn inner(&self) -> std::sync::MutexGuard<'_, WantsInner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Someone starts (`add`) or stops wanting `ids`.
    pub fn want(&self, ids: &[String], add: bool) {
        let mut w = self.inner();
        for id in ids {
            let n = w.counts.entry(id.clone()).or_insert(0);
            if add {
                *n += 1;
            } else {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    w.counts.remove(id);
                }
            }
        }
    }

    /// The ids anyone wants now.
    pub fn ids(&self) -> Vec<String> {
        self.inner().counts.keys().cloned().collect()
    }

    /// Pass the wanted set to the machine if it changed; true if so.
    pub fn apply(&self, m: &mut dyn crate::Machine) -> bool {
        let mut w = self.inner();
        let ids: Vec<String> = w.counts.keys().cloned().collect();
        if ids == w.applied {
            return false;
        }
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        m.set_overlays(&refs);
        w.applied = ids;
        true
    }
}
