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

/// One sprite on one raster line, as a video chip found it, for
/// [`bound_boxes`]: sprite number, line, the frame columns it covers
/// (`x`, `w` wide) and the first and last frame column (inclusive) where
/// it set a pixel (`None`: nothing set on this line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteLine {
    pub n: u8,
    pub line: u16,
    pub x: i32,
    pub w: u32,
    pub opaque: Option<(i32, i32)>,
}

/// Tight boxes round the pixels sprites really set (the `sprite_bounds`
/// overlays): the lines of a sprite at one column and width on
/// consecutive lines are one use of it (as its full-size box), and its box
/// spans the columns and lines of that use's set pixels. A use without a
/// set pixel gets none. `row` maps a raster line to its frame row (lines
/// without one are left out). Label: the sprite number; colour:
/// [`colors::SERIES`] by number.
pub fn bound_boxes(lines: &[SpriteLine], row: impl Fn(u16) -> Option<u32>) -> Vec<Shape> {
    let mut sorted = lines.to_vec();
    sorted.sort_by_key(|s| (s.n, s.x, s.w, s.line));
    let mut shapes = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let s = sorted[i];
        let mut j = i + 1;
        while j < sorted.len()
            && (sorted[j].n, sorted[j].x, sorted[j].w) == (s.n, s.x, s.w)
            && sorted[j].line == sorted[j - 1].line + 1
        {
            j += 1;
        }
        let set: Vec<(u32, (i32, i32))> = sorted[i..j]
            .iter()
            .filter_map(|l| Some((row(l.line)?, l.opaque?)))
            .collect();
        if let (Some(top), Some(bottom)) = (
            set.iter().map(|&(r, _)| r).min(),
            set.iter().map(|&(r, _)| r).max(),
        ) {
            let x0 = set.iter().map(|&(_, (a, _))| a).min().unwrap_or(0);
            let x1 = set.iter().map(|&(_, (_, b))| b).max().unwrap_or(0);
            shapes.push(Shape::Rect {
                x: x0,
                y: top as i32,
                w: (x1 - x0 + 1).max(1) as u32,
                h: bottom - top + 1,
                color: colors::SERIES[usize::from(s.n) % 8],
                label: Some(s.n.to_string()),
            });
        }
        i = j;
    }
    shapes
}

/// The first and last set column of a sprite row: `bits` with the
/// leftmost of `cols` pattern columns in bit `cols - 1`, each column `scale`
/// frame pixels wide from frame column `x`; only columns in `visible`
/// (frame columns, end exclusive) count. `None` when none is set there.
pub fn row_bounds(
    bits: u64,
    cols: u32,
    scale: u32,
    x: i32,
    visible: std::ops::Range<i32>,
) -> Option<(i32, i32)> {
    let mut first = None;
    let mut last = None;
    for c in 0..cols {
        if bits >> (cols - 1 - c) & 1 == 0 {
            continue;
        }
        let a = x + (c * scale) as i32;
        let b = a + scale as i32 - 1;
        let (a, b) = (a.max(visible.start), b.min(visible.end - 1));
        if a > b {
            continue;
        }
        first.get_or_insert(a);
        last = Some(b);
    }
    Some((first?, last?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_boxes_hug_the_set_pixels_of_each_use() {
        let line = |line, opaque| SpriteLine {
            n: 2,
            line,
            x: 100,
            w: 16,
            opaque,
        };
        let lines = [
            line(10, None),
            line(11, Some((104, 107))),
            line(12, Some((102, 105))),
            line(13, None),
            // Another use further down, nothing set: no box.
            line(30, None),
        ];
        let b = bound_boxes(&lines, |l| Some(u32::from(l)));
        assert_eq!(
            b,
            [Shape::Rect {
                x: 102,
                y: 11,
                w: 6,
                h: 2,
                color: colors::SERIES[2],
                label: Some("2".into()),
            }]
        );
    }

    #[test]
    fn row_bounds_scale_and_clip() {
        // 8 columns, bits 0b0011_1000: columns 2-4.
        assert_eq!(row_bounds(0b0011_1000, 8, 1, 10, 0..256), Some((12, 14)));
        assert_eq!(row_bounds(0b0011_1000, 8, 2, 10, 0..256), Some((14, 19)));
        assert_eq!(row_bounds(0b1000_0000, 8, 1, -1, 0..256), None, "clipped");
        assert_eq!(row_bounds(0, 16, 1, 0, 0..256), None);
    }
}
