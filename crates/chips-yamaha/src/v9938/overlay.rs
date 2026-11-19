//! Debug overlays of the V9938 (see `mimic_core::overlay`), as the
//! TMS9918A's in `chips-ti`: `sprites`, a box per sprite where the chip
//! drew it, line by line, and `limit`, a band on each line with one sprite
//! too many (a fifth in sprite mode 1, a ninth in mode 2).
//!
//! Recording happens only while an overlay is switched on. The record is
//! not machine state; the machine swaps it at the end of each field
//! ([`V9938::overlay_end_frame`]).

use mimic_core::overlay::{
    bound_boxes, colors, row_bounds, LegendEntry, Overlay, OverlayInfo, Shape, SpriteLine,
};

use super::V9938;

/// One sprite on one line: number, picture row, first column, width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    n: u8,
    line: u16,
    x: i32,
    w: u32,
    /// First and last frame column with a set pixel (`sprite_bounds`).
    opaque: Option<(i32, i32)>,
}

/// A sprite row as the renderer read it, for [`Records::sprite`]: the
/// pattern bits (column 0 in bit 15), the columns (8 or 16), the frame
/// pixels a column covers (2 × magnification) and whether the row shows
/// (colour not transparent, or TP set).
pub(super) struct Row {
    pub bits: u16,
    pub size: usize,
    pub scale: u32,
    pub shown: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Field {
    spans: Vec<Span>,
    limit: Vec<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Records {
    sprites: bool,
    bounds: bool,
    limit: bool,
    cur: Field,
    last: Field,
}

impl Records {
    /// Sprite `n` on picture row `line` from frame column `x`, `w` wide;
    /// `visible`: the active area's frame columns.
    #[inline]
    pub(super) fn sprite(
        &mut self,
        n: u8,
        line: u16,
        x: i32,
        w: u32,
        row: Row,
        visible: std::ops::Range<i32>,
    ) {
        if self.sprites || self.bounds {
            self.record(n, line, x, w, row, visible);
        }
    }

    #[cold]
    #[inline(never)]
    fn record(
        &mut self,
        n: u8,
        line: u16,
        x: i32,
        w: u32,
        row: Row,
        visible: std::ops::Range<i32>,
    ) {
        let cols = row.size as u32;
        let bits = u64::from(row.bits >> (16 - cols));
        let opaque = if row.shown {
            row_bounds(bits, cols, row.scale, x, visible)
        } else {
            None
        };
        self.cur.spans.push(Span {
            n,
            line,
            x,
            w,
            opaque,
        });
    }

    pub(super) fn limit(&mut self, line: u16) {
        if self.limit && self.cur.limit.last() != Some(&line) {
            self.cur.limit.push(line);
        }
    }
}

/// The overlays the V9938 offers.
pub fn infos() -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "sprites",
            name: "V9938 sprite boxes".into(),
        },
        OverlayInfo {
            id: "limit",
            name: "V9938 lines with too many sprites".into(),
        },
        OverlayInfo {
            id: "sprite_bounds",
            name: "V9938 sprite bounds".into(),
        },
    ]
}

impl V9938 {
    /// Switch recording on for the overlays in `ids`, off (and cleared)
    /// for the others.
    pub fn set_overlays(&mut self, ids: &[&str]) {
        self.ov = Records {
            sprites: ids.contains(&"sprites"),
            bounds: ids.contains(&"sprite_bounds"),
            limit: ids.contains(&"limit"),
            ..Records::default()
        };
    }

    pub fn overlays_on(&self) -> bool {
        self.ov.sprites || self.ov.bounds || self.ov.limit
    }

    /// End of a field: the record becomes the last complete one.
    pub fn overlay_end_frame(&mut self) {
        let ov = &mut self.ov;
        std::mem::swap(&mut ov.cur, &mut ov.last);
        ov.cur.spans.clear();
        ov.cur.limit.clear();
    }

    /// Overlay `id` from the last complete field (`last`) or the field so
    /// far; None for an unknown id or one switched off.
    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        let f = if last { &self.ov.last } else { &self.ov.cur };
        let info = infos().into_iter().find(|i| i.id == id)?;
        let (shapes, legend) = match id {
            "sprites" if self.ov.sprites => (rects(&f.spans), vec![]),
            "sprite_bounds" if self.ov.bounds => {
                let lines: Vec<SpriteLine> = f
                    .spans
                    .iter()
                    .map(|s| SpriteLine {
                        n: s.n,
                        line: s.line,
                        x: s.x,
                        w: s.w,
                        opaque: s.opaque,
                    })
                    .collect();
                (bound_boxes(&lines, |l| Some(u32::from(l))), vec![])
            }
            "limit" if self.ov.limit => {
                let mut shapes: Vec<Shape> = Vec::new();
                for &l in &f.limit {
                    match shapes.last_mut() {
                        Some(Shape::Band { y, h, .. }) if *y + *h as i32 == i32::from(l) => *h += 1,
                        _ => shapes.push(Shape::Band {
                            y: i32::from(l),
                            h: 1,
                            color: colors::RED,
                            label: Some(format!("5S row {l}")),
                        }),
                    }
                }
                let legend = vec![LegendEntry {
                    color: colors::RED,
                    text: "one sprite too many on the line (5S)".into(),
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

/// Boxes from the per-line spans: runs of consecutive lines with the same
/// sprite at the same column and width.
fn rects(spans: &[Span]) -> Vec<Shape> {
    let mut sorted = spans.to_vec();
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
        shapes.push(Shape::Rect {
            x: s.x,
            y: i32::from(s.line),
            w: s.w,
            h: (j - i) as u32,
            color: colors::SERIES[usize::from(s.n) % 8],
            label: Some(s.n.to_string()),
        });
        i = j;
    }
    shapes
}
