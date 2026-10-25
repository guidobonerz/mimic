//! Debug overlays of the TED (see `emu_core::overlay`): `badlines`, a band
//! on every raster line on which the TED takes the CPU's whole fetch window
//! (the first line of each character row with the display enabled, as
//! [`Ted`] models its DMA; register map after the TED 7360R0 preliminary
//! data sheet). Recorded only while switched on; see `crate::overlay`.

use emu_core::overlay::{Overlay, OverlayInfo};

use super::Ted;
use crate::overlay::{line_bands, LineMarks, Recorder};

pub const BADLINES: &str = "badlines";

/// The overlays a TED offers.
pub fn overlays() -> Vec<OverlayInfo> {
    vec![OverlayInfo {
        id: BADLINES,
        name: "TED DMA lines".into(),
    }]
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Record {
    pub(crate) lines: LineMarks,
    pub(crate) last: LineMarks,
}

impl Record {
    pub(crate) fn end_field(&mut self) {
        std::mem::swap(&mut self.lines, &mut self.last);
        self.lines.clear();
    }
}

impl Ted {
    /// Switch recording on for the overlays among `ids` the TED has.
    pub fn set_overlays(&mut self, ids: &[&str]) {
        if ids.contains(&BADLINES) {
            self.rec.0.get_or_insert_with(Box::default);
        } else {
            self.rec = Recorder(None);
        }
    }

    /// Frame-buffer row showing raster line `line`, if it is visible (the
    /// window starts at `first_line` and wraps past the last line).
    pub fn frame_row(&self, line: u16) -> Option<u32> {
        let t = self.timing();
        if line >= t.lines {
            return None;
        }
        let y = u32::from((line + t.lines - t.first_line) % t.lines);
        (y < t.height).then_some(y)
    }

    /// Overlay `id` if it is switched on.
    pub fn overlay(&self, id: &str) -> Option<Overlay> {
        let r = self.rec.0.as_deref()?;
        let info = overlays().into_iter().find(|o| o.id == id)?;
        let shapes = line_bands(
            &r.lines,
            &r.last,
            self.timing().lines,
            self.line,
            |l| self.frame_row(l),
            "DMA lines",
        );
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ted::{Model, TedMemory};
    use emu_core::overlay::Shape;

    struct Ram;

    impl TedMemory for Ram {
        fn fetch(&self, _addr: u16, _rom: bool) -> u8 {
            0
        }
    }

    fn field(t: &mut Ted) {
        while !t.tick(&Ram).frame_end {}
    }

    #[test]
    fn hpos_is_the_frame_column_of_the_tick() {
        let mut t = Ted::new(Model::Mos8360);
        while !(t.line == 100 && t.tick == 40) {
            t.tick(&Ram);
        }
        let x = u32::from(t.beam().hpos);
        t.write(0x19, 5);
        while t.line != 101 {
            t.tick(&Ram);
        }
        t.write(0x19, 0);
        field(&mut t);
        let at = t.frame_row(100).unwrap() * super::super::WIDTH;
        let px = |c: u32| t.pixels()[(at + c) as usize];
        assert_eq!((px(x - 1), px(x)), (0, 5));
        t.tick = 0;
        assert_eq!(t.beam().hpos, 0, "clamped left");
        t.tick = 113;
        assert_eq!(t.beam().hpos, 383, "clamped right");
    }

    #[test]
    fn frame_rows_and_dma_lines() {
        let mut t = Ted::new(Model::Mos8360);
        assert_eq!(t.frame_row(274), Some(0));
        assert_eq!(t.frame_row(311), Some(37));
        assert_eq!(t.frame_row(0), Some(38));
        assert_eq!(t.frame_row(245), Some(283));
        assert_eq!(t.frame_row(246), None);
        assert_eq!(t.frame_row(312), None);
        // Border colour 5 on line 100 only lands on that line's row.
        t.write(0x19, 0);
        while t.line != 100 {
            t.tick(&Ram);
        }
        t.write(0x19, 5);
        while t.line != 101 {
            t.tick(&Ram);
        }
        t.write(0x19, 0);
        field(&mut t);
        let row = t.frame_row(100).unwrap();
        let px = |r: u32| t.pixels()[(r * super::super::WIDTH) as usize];
        assert_eq!((px(row - 1), px(row), px(row + 1)), (0, 5, 0));

        assert!(t.overlay(BADLINES).is_none());
        t.set_overlays(&[BADLINES]);
        t.write(0x06, 0x1b); // DEN, 25 rows, YSCROLL 3
        field(&mut t);
        let o = t.overlay(BADLINES).unwrap();
        let rows: Vec<i32> = o
            .shapes
            .iter()
            .map(|s| match s {
                Shape::Band { y, .. } => *y,
                _ => unreachable!(),
            })
            .collect();
        // Character rows start at line 1 + YSCROLL.
        let expect: Vec<i32> = (0..25)
            .map(|r| t.frame_row(4 + 8 * r).unwrap() as i32)
            .collect();
        assert_eq!(rows, expect);
        t.set_overlays(&[]);
        assert!(t.overlay(BADLINES).is_none());
    }
}
