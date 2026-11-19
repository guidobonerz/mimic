//! Debug overlays of the VIC-II (see `mimic_core::overlay`):
//!
//! - `badlines`: a band on every raster line on which the VIC-II did its
//!   c-accesses (Bauer 3.5, "Bad Lines": the CPU is stopped for 40–43
//!   cycles), recorded where the c-accesses happen.
//! - `sprites`: a box for every run of lines on which a sprite was actually
//!   shown (its shifter started at its X coordinate, Bauer 3.8.1), so a
//!   multiplexed sprite gets one box per use; 24 or 48 (X-expanded) pixels
//!   wide, labelled with the sprite number.
//! - `sprite_bounds`: for the same runs, the tight box round the pixels the
//!   sprite really set: the columns of all bits (multicolour: pairs) that
//!   are not 0 in the rows the shifter started with, and the lines from the
//!   first to the last such row. A run with no pixel set gets no box.
//!
//! Both are recorded only while switched on ([`Vic2::set_overlays`]); see
//! `crate::overlay` for the record's rules.

use mimic_core::overlay::{colors, Overlay, OverlayInfo, Shape};

use super::{Timing, Vic2};
use crate::overlay::{line_bands, LineMarks, Recorder};

pub const BADLINES: &str = "badlines";
pub const SPRITES: &str = "sprites";
pub const BOUNDS: &str = "sprite_bounds";

/// The overlays a VIC-II offers.
pub fn overlays() -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: BADLINES,
            name: "VIC-II bad lines".into(),
        },
        OverlayInfo {
            id: SPRITES,
            name: "VIC-II sprites".into(),
        },
        OverlayInfo {
            id: BOUNDS,
            name: "VIC-II sprite bounds".into(),
        },
    ]
}

/// A sprite shown on consecutive raster lines at one X with one width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    n: u8,
    x: u16,
    w: u16,
    first: u16,
    last: u16,
    /// The pixel columns (bit 23 = column 0) set on any of its lines.
    cols: u32,
    /// The first and last line with a pixel set.
    set: Option<(u16, u16)>,
}

impl Run {
    /// Line `line` shifts out `mask` (bit 23 = column 0).
    fn add(&mut self, line: u16, mask: u32) {
        if mask == 0 {
            return;
        }
        self.cols |= mask;
        self.set = Some(match self.set {
            Some((first, _)) => (first, line),
            None => (line, line),
        });
    }
}

/// The columns a row of sprite data sets: hires every 1 bit, multicolour
/// both columns of every pair that is not 00.
fn pixel_mask(data: u32, mc: bool) -> u32 {
    let data = data & 0xff_ffff;
    if mc {
        let pairs = (data | data >> 1) & 0x55_5555;
        pairs | pairs << 1
    } else {
        data
    }
}

/// What the overlays need of a field.
#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    badlines: bool,
    sprites: bool,
    bounds: bool,
    /// The current line had c-accesses.
    line_bad: bool,
    bad: LineMarks,
    last_bad: LineMarks,
    runs: Vec<Run>,
    last_runs: Vec<Run>,
    open: [Option<Run>; 8],
}

impl Record {
    pub(crate) fn badlines(&self) -> bool {
        self.badlines
    }

    pub(crate) fn sprites(&self) -> bool {
        self.sprites
    }

    pub(crate) fn bounds(&self) -> bool {
        self.bounds
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn mark_bad(&mut self) {
        self.line_bad = true;
    }

    /// Sprite `n`'s shifter started at `x` on `line` with the row `data`
    /// (24 bits, multicolour if `mc`).
    #[cold]
    #[inline(never)]
    pub(crate) fn sprite_shown(
        &mut self,
        n: usize,
        line: u16,
        x: u16,
        xexp: bool,
        data: u32,
        mc: bool,
    ) {
        if !self.sprites && !self.bounds {
            return;
        }
        let w = if xexp { 48 } else { 24 };
        let mask = pixel_mask(data, mc);
        match &mut self.open[n] {
            Some(r) if r.x == x && r.w == w && r.last + 1 == line => {
                r.last = line;
                r.add(line, mask);
            }
            open => {
                if let Some(r) = open.take() {
                    self.runs.push(r);
                }
                let mut r = Run {
                    n: n as u8,
                    x,
                    w,
                    first: line,
                    last: line,
                    cols: 0,
                    set: None,
                };
                r.add(line, mask);
                *open = Some(r);
            }
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn end_line(&mut self, line: u16) {
        if std::mem::take(&mut self.line_bad) && self.badlines {
            self.bad.set(line);
        }
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn end_field(&mut self) {
        for r in self.open.iter_mut().filter_map(Option::take) {
            self.runs.push(r);
        }
        std::mem::swap(&mut self.bad, &mut self.last_bad);
        self.bad.clear();
        std::mem::swap(&mut self.runs, &mut self.last_runs);
        self.runs.clear();
    }
}

impl Timing {
    /// Frame-buffer row showing raster line `line`, if it is visible.
    pub fn frame_row(&self, line: u16) -> Option<u32> {
        if line >= self.lines {
            return None;
        }
        self.row(line).map(|r| r as u32)
    }

    /// Frame column of the pixel at X coordinate `x` (the first pixel of a
    /// sprite at that X), if the line has one.
    fn x_column(&self, x: u16) -> Option<i32> {
        (0..u16::from(self.cycles) * 8)
            .find(|&p| self.x_at(p) == x)
            .map(|p| i32::from(p) - i32::from(self.visible_p0))
    }
}

impl Vic2 {
    /// Switch recording on for the overlays among `ids` that the VIC-II
    /// has, off for the others (off for all frees the record).
    pub fn set_overlays(&mut self, ids: &[&str]) {
        let badlines = ids.contains(&BADLINES);
        let sprites = ids.contains(&SPRITES);
        let bounds = ids.contains(&BOUNDS);
        if !badlines && !sprites && !bounds {
            self.rec = Recorder(None);
            return;
        }
        let r = self.rec.0.get_or_insert_with(Box::default);
        r.badlines = badlines;
        r.sprites = sprites;
        r.bounds = bounds;
    }

    /// Frame-buffer row showing raster line `line`.
    pub fn frame_row(&self, line: u16) -> Option<u32> {
        self.timing().frame_row(line)
    }

    /// Overlay `id` if it is switched on.
    pub fn overlay(&self, id: &str) -> Option<Overlay> {
        let r = self.rec.0.as_deref()?;
        let info = overlays().into_iter().find(|o| o.id == id)?;
        let t = self.timing();
        let shapes = match info.id {
            BADLINES if r.badlines => line_bands(
                &r.bad,
                &r.last_bad,
                t.lines,
                self.raster,
                |l| t.frame_row(l),
                "bad lines",
            ),
            SPRITES if r.sprites => self.sprite_boxes(r),
            BOUNDS if r.bounds => self.bound_boxes(r),
            _ => return None,
        };
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend: Vec::new(),
        })
    }

    /// The runs to show: this field's above the beam (finished and open),
    /// the last field's from the beam down, each with the lines shown.
    fn field_runs(&self, r: &Record) -> Vec<(Run, u16, u16)> {
        let beam = self.raster;
        // This field above the beam (finished and open runs), the last one
        // from the beam down.
        let mut runs: Vec<(Run, u16, u16)> = Vec::new();
        for run in r.runs.iter().chain(r.open.iter().flatten()) {
            if run.first < beam {
                runs.push((*run, run.first, run.last.min(beam - 1)));
            }
        }
        for run in &r.last_runs {
            if run.last >= beam {
                runs.push((*run, run.first.max(beam), run.last));
            }
        }
        runs
    }

    fn sprite_boxes(&self, r: &Record) -> Vec<Shape> {
        let t = self.timing();
        let mut shapes = Vec::new();
        for (run, first, last) in self.field_runs(r) {
            if let Some(x) = t.x_column(run.x) {
                rects(t, &run, first, last, x, u32::from(run.w), &mut shapes);
            }
        }
        shapes
    }

    fn bound_boxes(&self, r: &Record) -> Vec<Shape> {
        let t = self.timing();
        let mut shapes = Vec::new();
        for (run, first, last) in self.field_runs(r) {
            let (Some(x), Some((set_first, set_last))) = (t.x_column(run.x), run.set) else {
                continue;
            };
            let (first, last) = (first.max(set_first), last.min(set_last));
            if first > last {
                continue;
            }
            let scale = u32::from(run.w) / 24;
            let lo = run.cols.leading_zeros() - 8;
            let hi = 23 - run.cols.trailing_zeros();
            let x = x + (lo * scale) as i32;
            rects(t, &run, first, last, x, (hi - lo + 1) * scale, &mut shapes);
        }
        shapes
    }
}

/// Boxes `w` pixels wide from frame column `x` over raster lines
/// `first..=last` of `run`, split where consecutive lines are not
/// consecutive rows of the frame.
fn rects(t: &Timing, run: &Run, first: u16, last: u16, x: i32, w: u32, shapes: &mut Vec<Shape>) {
    let mut seg: Option<(u32, u32)> = None;
    let push = |(y, h): (u32, u32), shapes: &mut Vec<Shape>| {
        shapes.push(Shape::Rect {
            x,
            y: y as i32,
            w,
            h,
            color: colors::SERIES[usize::from(run.n)],
            label: Some(run.n.to_string()),
        });
    };
    for line in first..=last {
        match (t.frame_row(line), seg) {
            (Some(row), Some((y, h))) if y + h == row => seg = Some((y, h + 1)),
            (row, s) => {
                if let Some(s) = s {
                    push(s, shapes);
                }
                seg = row.map(|y| (y, 1));
            }
        }
    }
    if let Some(s) = seg {
        push(s, shapes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vic2::{Model, VicMemory};
    use mimic_core::{Savestate, StateReader, StateWriter};

    struct Ram;

    impl VicMemory for Ram {
        fn read(&self, _addr: u16) -> u8 {
            0xff
        }
        fn color(&self, _index: u16) -> u8 {
            1
        }
    }

    fn field(v: &mut Vic2) {
        loop {
            v.phi1(&Ram);
            if v.phi2(&Ram) {
                return;
            }
        }
    }

    fn run_to_line(v: &mut Vic2, line: u16) {
        while v.raster() != line {
            v.phi1(&Ram);
            v.phi2(&Ram);
        }
    }

    #[test]
    fn frame_rows_follow_the_visible_window() {
        assert_eq!(
            PAL_ROWS.map(|l| Vic2::new(Model::Mos6569).frame_row(l)),
            [None, Some(0), Some(271), None, None]
        );
        let n = Vic2::new(Model::Mos6567R8);
        assert_eq!(n.frame_row(41), Some(0));
        assert_eq!(n.frame_row(262), Some(221));
        assert_eq!(n.frame_row(0), Some(222));
        assert_eq!(n.frame_row(12), Some(234));
        assert_eq!(n.frame_row(13), None);
        assert_eq!(n.frame_row(263), None);
        // The border colour of line 100 lands on that row.
        let mut v = Vic2::new(Model::Mos6569);
        field(&mut v);
        run_to_line(&mut v, 100);
        v.write(0x20, 5);
        run_to_line(&mut v, 101);
        v.write(0x20, 0);
        field(&mut v);
        let t = v.timing();
        let row = v.frame_row(100).unwrap() as usize;
        assert_eq!(v.pixels()[row * t.width as usize], 5);
        assert_eq!(v.pixels()[(row - 1) * t.width as usize], 0);
        assert_eq!(v.pixels()[(row + 1) * t.width as usize], 0);
    }

    #[test]
    fn hpos_is_the_frame_column_of_the_cycle() {
        for model in [Model::Mos6569, Model::Mos6567R8] {
            let mut v = Vic2::new(model);
            run_to_line(&mut v, 100);
            while v.beam().cycle != 30 {
                v.phi1(&Ram);
                v.phi2(&Ram);
            }
            let x = usize::from(v.beam().hpos);
            v.write(0x20, 5);
            run_to_line(&mut v, 101);
            v.write(0x20, 0);
            field(&mut v);
            let at = v.frame_row(100).unwrap() as usize * v.timing().width as usize;
            // A colour register acts four pixels before the cycle's column,
            // the first of them still in the old colour on the 6569 (see
            // `Vic2::draw`).
            assert_eq!((v.pixels()[at + x - 4], v.pixels()[at + x - 3]), (0, 5));
            assert_eq!(v.timing().cycle_column(1), 0, "clamped left");
            assert_eq!(v.timing().cycle_column(63), 383, "clamped right");
        }
    }

    const PAL_ROWS: [u16; 5] = [15, 16, 287, 288, 312];

    /// Sprite data with empty rows 0-4 and `$0F` bytes after them (the
    /// sprite pointer reads `$0F` too: data at `$03C0`).
    struct Sparse;

    impl VicMemory for Sparse {
        fn read(&self, addr: u16) -> u8 {
            if (0x3c0..0x3cf).contains(&(addr & 0x3fff)) {
                0
            } else {
                0x0f
            }
        }
        fn color(&self, _index: u16) -> u8 {
            1
        }
    }

    fn sparse_field(v: &mut Vic2) {
        loop {
            v.phi1(&Sparse);
            if v.phi2(&Sparse) {
                return;
            }
        }
    }

    fn rect(s: &Shape) -> (i32, i32, u32, u32) {
        match s {
            Shape::Rect { x, y, w, h, .. } => (*x, *y, *w, *h),
            _ => panic!("{s:?}"),
        }
    }

    #[test]
    fn sprite_bounds_hug_the_pixels_set() {
        let mut v = Vic2::new(Model::Mos6569);
        v.set_overlays(&[SPRITES, BOUNDS]);
        v.write(0x11, 0x1b);
        v.write(0x15, 0x01);
        v.write(0x00, 100);
        v.write(0x01, 80);
        sparse_field(&mut v);
        sparse_field(&mut v);
        let (r81, r86) = (
            v.frame_row(81).unwrap() as i32,
            v.frame_row(86).unwrap() as i32,
        );
        let full = v.overlay(SPRITES).unwrap();
        assert_eq!(rect(&full.shapes[0]), (108, r81, 24, 21));
        // Rows 5-20 hold $0F $0F $0F: columns 4-7, 12-15, 20-23.
        let tight = v.overlay(BOUNDS).unwrap();
        assert_eq!(tight.shapes.len(), 1);
        assert_eq!(rect(&tight.shapes[0]), (112, r86, 20, 16));
        // X-expanded: every column twice as wide.
        v.write(0x1d, 0x01);
        sparse_field(&mut v);
        sparse_field(&mut v);
        let tight = v.overlay(BOUNDS).unwrap();
        assert_eq!(rect(&tight.shapes[0]), (116, r86, 40, 16));
        // Multicolour: $0F is the pairs 00 00 11 11, the same columns.
        v.write(0x1d, 0);
        v.write(0x1c, 0x01);
        sparse_field(&mut v);
        sparse_field(&mut v);
        let tight = v.overlay(BOUNDS).unwrap();
        assert_eq!(rect(&tight.shapes[0]), (112, r86, 20, 16));
        // Only the bounds on: recorded without the full boxes.
        v.set_overlays(&[BOUNDS]);
        sparse_field(&mut v);
        assert!(v.overlay(SPRITES).is_none());
        assert_eq!(v.overlay(BOUNDS).unwrap().shapes.len(), 1);
    }

    #[test]
    fn pixel_masks_of_hires_and_multicolour_rows() {
        assert_eq!(pixel_mask(0x80_0001, false), 0x80_0001);
        assert_eq!(pixel_mask(0x40_0001, true), 0xc0_0003);
        assert_eq!(pixel_mask(0, true), 0);
    }

    #[test]
    fn bad_lines_and_sprite_boxes() {
        let mut v = Vic2::new(Model::Mos6569);
        assert!(v.overlay(BADLINES).is_none(), "off");
        v.set_overlays(&[BADLINES, SPRITES]);
        v.write(0x11, 0x1b); // DEN, 25 rows, YSCROLL 3
        v.write(0x15, 0x01); // sprite 0 on
        v.write(0x00, 100); // X 100
        v.write(0x01, 80); // Y 80
        v.write(0x27, 2);
        field(&mut v);
        field(&mut v);
        let bad = v.overlay(BADLINES).unwrap();
        let rows: Vec<i32> = bad
            .shapes
            .iter()
            .map(|s| match s {
                Shape::Band { y, .. } => *y,
                _ => unreachable!(),
            })
            .collect();
        let expect: Vec<i32> = (0x30..=0xf7u16)
            .filter(|l| l & 7 == 3)
            .map(|l| v.frame_row(l).unwrap() as i32)
            .collect();
        assert_eq!(rows, expect);
        assert_eq!(rows.len(), 25);
        assert!(matches!(&bad.shapes[0], Shape::Band { label: Some(_), .. }));

        let sp = v.overlay(SPRITES).unwrap();
        assert_eq!(sp.shapes.len(), 1);
        let Shape::Rect {
            x,
            y,
            w,
            h,
            color,
            label,
        } = &sp.shapes[0]
        else {
            panic!()
        };
        // Sprite pixels start at X 100 = frame column 100 - 24 + 32 = 108;
        // the first shown line is Y + 1.
        assert_eq!(
            (*x, *y, *w, *h),
            (108, v.frame_row(81).unwrap() as i32, 24, 21)
        );
        assert_eq!((*color, label.as_deref()), (colors::SERIES[0], Some("0")));
        let t = v.timing();
        let row = v.frame_row(81).unwrap() as usize;
        assert_eq!(
            v.pixels()[row * t.width as usize + 108],
            2,
            "the box is where the pixels are"
        );
        assert_ne!(v.pixels()[row * t.width as usize + 107], 2);

        // Multiplexed: moved down within the field, the sprite shows twice.
        v.set_overlays(&[SPRITES]);
        run_to_line(&mut v, 120);
        v.write(0x01, 150);
        field(&mut v);
        let sp = v.overlay(SPRITES).unwrap();
        assert_eq!(sp.shapes.len(), 2);
        assert!(v.overlay(BADLINES).is_none(), "switched off");
        // Not state: the same savestate with or without the record, and a
        // load keeps what is switched on.
        let save = |v: &Vic2| {
            let mut w = StateWriter::new();
            v.save(&mut w);
            w.into_bytes()
        };
        let mut off = v.clone();
        off.set_overlays(&[]);
        assert_eq!(save(&v), save(&off));
        assert_eq!(v, off);
        let state = save(&v);
        v.load(&mut StateReader::new(&state)).unwrap();
        assert!(
            v.overlay(SPRITES).unwrap().shapes.is_empty(),
            "fresh record"
        );
        v.set_overlays(&[]);
        assert!(v.overlay(SPRITES).is_none());
    }
}
