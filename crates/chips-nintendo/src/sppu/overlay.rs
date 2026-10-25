//! Debug overlay of the S-PPU (see `emu_core::overlay`): `sprites`, a box
//! per object on the lines the sprite scan found it in range (fullsnes,
//! "SNES PPU Sprites (OBJ)": the 32 objects a line), so objects changed
//! during the frame show where they really were. Frame columns are two
//! per pixel (the 512-column picture); rows follow interlace as the field
//! was drawn.
//!
//! Recording happens only while switched on (one flag test per line
//! otherwise); the record is not machine state. The machine ends the
//! frame with [`SPpu::overlay_end_frame`] where its frames end (the start
//! of vertical blank).

use emu_core::overlay::{Overlay, OverlayInfo};

use super::{SPpu, LINES};
use crate::overlay::{sprite_rects, Record, Span};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SPpuOverlays {
    pub(super) sprites: bool,
    spans: Record<Span>,
    /// (interlace, field) the current and the last frame were drawn with.
    rows: [(bool, bool); 2],
}

impl SPpuOverlays {
    /// Object `i` in range on line `y` at pixel `x` (−256…255), `w` wide.
    pub(super) fn record_sprite(&mut self, i: usize, y: u16, x: i32, w: u16, rows: (bool, bool)) {
        self.rows[0] = rows;
        self.spans.cur.push(Span {
            n: i as u8,
            line: y,
            x: x * 2,
            w: u32::from(w) * 2,
        });
    }
}

/// The picture row of line `y` (1–239) with interlace and the field as
/// given; None for lines without a row.
pub fn row_of(y: u16, interlace: bool, field: bool) -> Option<u32> {
    if y == 0 || usize::from(y) > LINES {
        return None;
    }
    let line = u32::from(y) - 1;
    Some(if interlace {
        line * 2 + u32::from(field)
    } else {
        line
    })
}

pub fn infos() -> Vec<OverlayInfo> {
    vec![OverlayInfo {
        id: "sprites",
        name: "S-PPU sprite boxes".into(),
    }]
}

impl SPpu {
    /// The picture row showing line `y` now.
    pub fn frame_row(&self, y: u16) -> Option<u32> {
        row_of(y, self.interlace(), self.field)
    }

    pub fn set_overlays(&mut self, ids: &[&str]) {
        self.ov.sprites = ids.contains(&"sprites");
        if !self.ov.sprites {
            self.ov.spans.clear();
        }
    }

    pub fn overlays_on(&self) -> bool {
        self.ov.sprites
    }

    pub fn overlay_end_frame(&mut self) {
        self.ov.spans.end_frame();
        self.ov.rows[1] = self.ov.rows[0];
    }

    /// Overlay `id` of the last complete frame (`last`) or the frame so
    /// far; None for an unknown id or one switched off.
    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        if id != "sprites" || !self.ov.sprites {
            return None;
        }
        let (il, field) = self.ov.rows[usize::from(last)];
        Some(Overlay {
            id: "sprites",
            name: infos()[0].name.clone(),
            shapes: sprite_rects(self.ov.spans.get(last), |l| row_of(l, il, field)),
            legend: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::overlay::{colors, Shape};

    #[test]
    fn sprite_boxes_follow_the_line_scan() {
        let mut p = SPpu::new(false);
        p.write(0x00, 0x0f);
        p.write(0x01, 0x00); // 8/16 sizes
                             // Object 5 at X = 40, Y = 99 (lines 100–107), small; the others
                             // parked below the picture.
        for i in 0..128 {
            p.oam[i * 4 + 1] = 0xf0;
        }
        p.oam[5 * 4..5 * 4 + 2].copy_from_slice(&[40, 99]);
        for y in 1..=224 {
            p.render_line(y);
        }
        assert!(p.overlay("sprites", false).is_none(), "switched off");
        assert_eq!(p.ov, SPpuOverlays::default(), "nothing recorded");
        p.set_overlays(&["sprites"]);
        for y in 1..=224 {
            p.render_line(y);
        }
        p.overlay_end_frame();
        let o = p.overlay("sprites", true).unwrap();
        assert_eq!(
            o.shapes,
            [Shape::Rect {
                x: 80,
                y: 99,
                w: 16,
                h: 8,
                color: colors::SERIES[5],
                label: Some("5".into()),
            }]
        );
        assert!(p.overlay("sprites", false).unwrap().shapes.is_empty());
        assert_eq!(row_of(1, true, true), Some(1));
        assert_eq!(row_of(240, false, false), None);
    }
}
