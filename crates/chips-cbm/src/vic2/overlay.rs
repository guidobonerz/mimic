//! Debug overlays of the VIC-II (see `emu_core::overlay`):
//!
//! - `badlines`: a band on every raster line on which the VIC-II did its
//!   c-accesses (Bauer 3.5, "Bad Lines": the CPU is stopped for 40–43
//!   cycles), recorded where the c-accesses happen.
//! - `sprites`: a box for every run of lines on which a sprite was actually
//!   shown (its shifter started at its X coordinate, Bauer 3.8.1), so a
//!   multiplexed sprite gets one box per use; 24 or 48 (X-expanded) pixels
//!   wide, labelled with the sprite number.
//!
//! Both are recorded only while switched on ([`Vic2::set_overlays`]); see
//! `crate::overlay` for the record's rules.

use emu_core::overlay::{colors, Overlay, OverlayInfo, Shape};

use super::{Timing, Vic2};
use crate::overlay::{line_bands, LineMarks, Recorder};

pub const BADLINES: &str = "badlines";
pub const SPRITES: &str = "sprites";

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
}

/// What the overlays need of a field.
#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    badlines: bool,
    sprites: bool,
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

    #[cold]
    #[inline(never)]
    pub(crate) fn mark_bad(&mut self) {
        self.line_bad = true;
    }

    /// Sprite `n`'s shifter started at `x` on `line`.
    #[cold]
    #[inline(never)]
    pub(crate) fn sprite_shown(&mut self, n: usize, line: u16, x: u16, xexp: bool) {
        if !self.sprites {
            return;
        }
        let w = if xexp { 48 } else { 24 };
        match &mut self.open[n] {
            Some(r) if r.x == x && r.w == w && r.last + 1 == line => r.last = line,
            open => {
                if let Some(r) = open.take() {
                    self.runs.push(r);
                }
                *open = Some(Run {
                    n: n as u8,
                    x,
                    w,
                    first: line,
                    last: line,
                });
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
        if !badlines && !sprites {
            self.rec = Recorder(None);
            return;
        }
        let r = self.rec.0.get_or_insert_with(Box::default);
        r.badlines = badlines;
        r.sprites = sprites;
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
            _ => return None,
        };
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend: Vec::new(),
        })
    }

    fn sprite_boxes(&self, r: &Record) -> Vec<Shape> {
        let t = self.timing();
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
        let mut shapes = Vec::new();
        for (run, first, last) in runs {
            let Some(x) = t.x_column(run.x) else {
                continue;
            };
            // Split where consecutive lines are not consecutive rows.
            let mut seg: Option<(u32, u32)> = None;
            let push = |(y, h): (u32, u32), shapes: &mut Vec<Shape>| {
                shapes.push(Shape::Rect {
                    x,
                    y: y as i32,
                    w: u32::from(run.w),
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
                            push(s, &mut shapes);
                        }
                        seg = row.map(|y| (y, 1));
                    }
                }
            }
            if let Some(s) = seg {
                push(s, &mut shapes);
            }
        }
        shapes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vic2::{Model, VicMemory};
    use emu_core::{Savestate, StateReader, StateWriter};

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
            assert_eq!((v.pixels()[at + x - 1], v.pixels()[at + x]), (0, 5));
            assert_eq!(v.timing().cycle_column(1), 0, "clamped left");
            assert_eq!(v.timing().cycle_column(63), 383, "clamped right");
        }
    }

    const PAL_ROWS: [u16; 5] = [15, 16, 287, 288, 312];

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
