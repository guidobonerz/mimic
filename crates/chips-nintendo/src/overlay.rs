//! Shared pieces of the PPUs' debug overlays (see `emu_core::overlay`):
//! sprite spans recorded line by line, merged into boxes, and the
//! double-buffered per-frame record. Records are not machine state.

use emu_core::overlay::{colors, Shape};

/// One sprite on one line: sprite number, raster line, first frame column
/// and width in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub n: u8,
    pub line: u16,
    pub x: i32,
    pub w: u32,
}

/// A record for the frame being drawn and the last complete one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record<T> {
    pub cur: Vec<T>,
    pub last: Vec<T>,
}

impl<T> Default for Record<T> {
    fn default() -> Self {
        Record {
            cur: Vec::new(),
            last: Vec::new(),
        }
    }
}

impl<T> Record<T> {
    /// End of the frame: the record becomes the last complete one.
    pub fn end_frame(&mut self) {
        std::mem::swap(&mut self.cur, &mut self.last);
        self.cur.clear();
    }

    pub fn clear(&mut self) {
        self.cur.clear();
        self.last.clear();
    }

    /// The last complete frame or the frame so far.
    pub fn get(&self, last: bool) -> &[T] {
        if last {
            &self.last
        } else {
            &self.cur
        }
    }
}

/// Boxes from per-line spans: runs of consecutive lines with the same
/// sprite at the same column and width; `row` maps a raster line to its
/// frame row (lines without one are left out). Label: the sprite number;
/// colour: [`colors::SERIES`] by number.
pub fn sprite_rects(spans: &[Span], row: impl Fn(u16) -> Option<u32>) -> Vec<Shape> {
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
        if let (Some(top), Some(bottom)) = (row(s.line), row(sorted[j - 1].line)) {
            shapes.push(Shape::Rect {
                x: s.x,
                y: top as i32,
                w: s.w,
                h: bottom.saturating_sub(top) + 1,
                color: colors::SERIES[usize::from(s.n) % 8],
                label: Some(s.n.to_string()),
            });
        }
        i = j;
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_merge_into_boxes() {
        let span = |n, line, x| Span { n, line, x, w: 8 };
        let spans = [
            span(3, 10, 5),
            span(3, 11, 5),
            span(3, 12, 6), // moved: a new box
            span(3, 20, 6), // gap: a new box
            span(1, 4, 0),
        ];
        let r = sprite_rects(&spans, |l| Some(u32::from(l) * 2));
        assert_eq!(r.len(), 4);
        assert_eq!(
            r[1],
            Shape::Rect {
                x: 5,
                y: 20,
                w: 8,
                h: 3,
                color: colors::SERIES[3],
                label: Some("3".into()),
            }
        );
        let mut rec = Record::default();
        rec.cur.push(1);
        rec.end_frame();
        assert_eq!((rec.get(true), rec.get(false)), (&[1][..], &[][..]));
    }
}
