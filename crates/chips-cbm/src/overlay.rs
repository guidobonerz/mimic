//! Shared pieces of the chips' debug overlays (see `emu_core::overlay`).
//!
//! A chip records what an overlay needs only while the overlay is switched
//! on. The record lives in a [`Recorder`]: `None` when nothing is switched
//! on (the emulation then pays one never-taken branch where it would
//! record), not machine state (ignored by `PartialEq`, never saved), and
//! double-buffered per field so a view shows the last complete field below
//! the beam and the field so far above it, as the frame buffer does.

use emu_core::overlay::{colors, Shape};

/// A chip's overlay record, boxed so a switched-off recorder costs a null
/// pointer. Equality ignores it: it is not machine state.
#[derive(Clone, Default)]
pub(crate) struct Recorder<T>(pub(crate) Option<Box<T>>);

impl<T> PartialEq for Recorder<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> Eq for Recorder<T> {}

impl<T> std::fmt::Debug for Recorder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Recorder(on)"
        } else {
            "Recorder(off)"
        })
    }
}

/// Raster lines marked in one field (bit per line).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LineMarks(Vec<u64>);

impl LineMarks {
    pub(crate) fn set(&mut self, line: u16) {
        let (w, b) = (usize::from(line / 64), line % 64);
        if self.0.len() <= w {
            self.0.resize(w + 1, 0);
        }
        self.0[w] |= 1 << b;
    }

    pub(crate) fn get(&self, line: u16) -> bool {
        self.0
            .get(usize::from(line / 64))
            .is_some_and(|w| w & (1 << (line % 64)) != 0)
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// One band per marked line: lines before `beam_line` from `cur` (this
/// field so far), the others from `last` (the previous field), at the
/// frame rows `row` gives; the first band carries `label`.
pub(crate) fn line_bands(
    cur: &LineMarks,
    last: &LineMarks,
    lines: u16,
    beam_line: u16,
    row: impl Fn(u16) -> Option<u32>,
    label: &str,
) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let mut rows: Vec<u32> = (0..lines)
        .filter(|&l| {
            if l < beam_line {
                cur.get(l)
            } else {
                last.get(l)
            }
        })
        .filter_map(&row)
        .collect();
    rows.sort_unstable();
    for (i, y) in rows.into_iter().enumerate() {
        shapes.push(Shape::Band {
            y: y as i32,
            h: 1,
            color: colors::ORANGE,
            label: (i == 0).then(|| label.to_string()),
        });
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_and_bands() {
        let mut cur = LineMarks::default();
        let mut last = LineMarks::default();
        cur.set(3);
        last.set(3);
        last.set(100);
        cur.set(200); // after the beam: not shown
        assert!(cur.get(3) && !cur.get(4) && !cur.get(1000));
        let bands = line_bands(&cur, &last, 312, 50, |l| Some(u32::from(l) + 1), "x");
        assert_eq!(bands.len(), 2);
        assert!(matches!(&bands[0], Shape::Band { y: 4, label: Some(l), .. } if l == "x"));
        assert!(matches!(
            &bands[1],
            Shape::Band {
                y: 101,
                label: None,
                ..
            }
        ));
        cur.clear();
        assert!(!cur.get(3));
        assert_eq!(Recorder::<u8>(None), Recorder(Some(Box::new(1))));
    }
}
