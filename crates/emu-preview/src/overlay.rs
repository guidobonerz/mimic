//! Drawing debug overlays onto a raw frame picture (see
//! `emu_core::overlay`): machine overlays plus two generic ones, the beam
//! marker (`beam`) and the raster grid (`grid`), which need only the beam
//! and [`emu_core::Machine::frame_row`]. Overlays are drawn on the raw
//! frame before any scaling; CRT pictures never get them.

use emu_core::frame::Rgb8;
use emu_core::overlay::{colors, Overlay, Shape};
use emu_core::{BeamPos, Machine};

use crate::RgbImage;

/// Generic overlay ids the preview draws itself.
pub const BEAM: &str = "beam";
pub const GRID: &str = "grid";

/// Everything to draw on one frame, collected on the emulation thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlaySet {
    /// Machine overlays.
    pub overlays: Vec<Overlay>,
    /// Beam marker: frame row and column of the beam.
    pub beam: Option<(u32, u32)>,
    /// Grid: (frame row, raster line) every 8 raster lines.
    pub grid: Vec<(u32, u16)>,
}

impl OverlaySet {
    /// Collect `ids` from the machine now (`beam` and `grid` included).
    pub fn collect(m: &dyn Machine, ids: &[String]) -> Self {
        let mut set = OverlaySet::default();
        for id in ids {
            match id.as_str() {
                BEAM => {
                    set.beam = m
                        .beam()
                        .and_then(|b| Some((m.frame_row(b.line)?, u32::from(b.hpos))));
                }
                GRID => {
                    set.grid = (0..1024u16)
                        .step_by(8)
                        .filter_map(|l| Some((m.frame_row(l)?, l)))
                        .collect();
                }
                other => set.overlays.extend(m.overlay(other)),
            }
        }
        set
    }

    /// Only the overlays named in `ids`.
    pub fn subset(&self, ids: &[String]) -> Self {
        let has = |id: &str| ids.iter().any(|i| i == id);
        OverlaySet {
            overlays: self
                .overlays
                .iter()
                .filter(|o| has(o.id))
                .cloned()
                .collect(),
            beam: self.beam.filter(|_| has(BEAM)),
            grid: if has(GRID) {
                self.grid.clone()
            } else {
                Vec::new()
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty() && self.beam.is_none() && self.grid.is_empty()
    }

    /// Draw onto `img` (the unscaled raw frame).
    pub fn draw(&self, img: &mut RgbImage) {
        for (row, line) in &self.grid {
            hline_dotted(img, *row as i32, colors::GREY);
            if line % 32 == 0 {
                text(img, 1, *row as i32 + 1, &line.to_string(), colors::GREY);
            }
        }
        for o in &self.overlays {
            for s in &o.shapes {
                shape(img, s, &o.legend);
            }
        }
        if let Some((row, col)) = self.beam {
            let (row, col) = (row as i32, col as i32);
            for x in 0..img.width as i32 {
                blend(img, x, row, colors::RED, 128);
            }
            for d in -3..=3 {
                put(img, col + d, row, colors::YELLOW);
                put(img, col, row + d, colors::YELLOW);
            }
        }
    }
}

/// The beam of a partial frame, as the generic `beam` overlay would draw
/// it; `None` when the machine cannot place it.
pub fn beam_row(m: &dyn Machine, beam: BeamPos) -> Option<u32> {
    m.frame_row(beam.line)
}

fn shape(img: &mut RgbImage, s: &Shape, legend: &[emu_core::overlay::LegendEntry]) {
    match s {
        Shape::Rect {
            x,
            y,
            w,
            h,
            color,
            label,
        } => {
            let (w, h) = (*w as i32, *h as i32);
            for i in 0..w.max(1) {
                put(img, x + i, *y, *color);
                put(img, x + i, y + h - 1, *color);
            }
            for j in 0..h.max(1) {
                put(img, *x, y + j, *color);
                put(img, x + w - 1, y + j, *color);
            }
            if let Some(l) = label {
                text(img, x + 1, y - 6, l, *color);
            }
        }
        Shape::Band { y, h, color, label } => {
            for j in 0..*h as i32 {
                for x in 0..img.width as i32 {
                    blend(img, x, y + j, *color, 96);
                }
            }
            if let Some(l) = label {
                text(img, 1, *y, l, *color);
            }
        }
        Shape::Mark { x, y, color, label } => {
            for d in -2..=2 {
                put(img, *x, y + d, *color);
            }
            if let Some(l) = label {
                text(img, x + 2, y - 6, l, *color);
            }
        }
        Shape::Cells {
            x,
            y,
            cell_w,
            cell_h,
            cols,
            rows,
            cells,
        } => {
            for r in 0..*rows {
                for c in 0..*cols {
                    let Some(&k) = cells.get((r * cols + c) as usize) else {
                        return;
                    };
                    let Some(e) = legend.get(usize::from(k)) else {
                        continue;
                    };
                    for j in 0..*cell_h {
                        for i in 0..*cell_w {
                            blend(
                                img,
                                x + (c * cell_w + i) as i32,
                                y + (r * cell_h + j) as i32,
                                e.color,
                                160,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn index(img: &RgbImage, x: i32, y: i32) -> Option<usize> {
    (x >= 0 && y >= 0 && (x as u32) < img.width && (y as u32) < img.height)
        .then(|| (y as usize * img.width as usize + x as usize) * 3)
}

fn put(img: &mut RgbImage, x: i32, y: i32, c: Rgb8) {
    if let Some(i) = index(img, x, y) {
        img.data[i..i + 3].copy_from_slice(&[c.r, c.g, c.b]);
    }
}

/// Mix `c` over the pixel with weight `a`/255.
fn blend(img: &mut RgbImage, x: i32, y: i32, c: Rgb8, a: u16) {
    if let Some(i) = index(img, x, y) {
        for (p, v) in img.data[i..i + 3].iter_mut().zip([c.r, c.g, c.b]) {
            *p = ((u16::from(*p) * (255 - a) + u16::from(v) * a) / 255) as u8;
        }
    }
}

fn hline_dotted(img: &mut RgbImage, y: i32, c: Rgb8) {
    for x in (0..img.width as i32).step_by(2) {
        blend(img, x, y, c, 160);
    }
}

/// 3 × 5 glyphs, rows top to bottom, 3 bits each (MSB left).
fn glyph(ch: char) -> [u8; 5] {
    match ch.to_ascii_uppercase() {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 3, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '$' => [3, 6, 2, 3, 6],
        ':' => [0, 2, 0, 2, 0],
        '=' => [0, 7, 0, 7, 0],
        '-' => [0, 0, 7, 0, 0],
        '+' => [0, 2, 7, 2, 0],
        '.' => [0, 0, 0, 0, 2],
        '/' => [1, 1, 2, 4, 4],
        '#' => [5, 7, 5, 7, 5],
        _ => [0; 5],
    }
}

/// Draw `s` in the 3 × 5 font with a dark shadow, so it reads on any
/// picture.
pub fn text(img: &mut RgbImage, x: i32, y: i32, s: &str, c: Rgb8) {
    let shadow = Rgb8 { r: 0, g: 0, b: 0 };
    for (i, ch) in s.chars().enumerate() {
        let g = glyph(ch);
        let gx = x + i as i32 * 4;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    put(img, gx + col + 1, y + row as i32 + 1, shadow);
                    put(img, gx + col, y + row as i32, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::overlay::LegendEntry;

    fn black(w: u32, h: u32) -> RgbImage {
        RgbImage {
            width: w,
            height: h,
            data: vec![0; (w * h * 3) as usize],
        }
    }

    fn at(img: &RgbImage, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.width + x) * 3) as usize;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    }

    #[test]
    fn shapes_land_where_they_belong() {
        let mut img = black(64, 32);
        let set = OverlaySet {
            overlays: vec![Overlay {
                id: "t",
                name: "test".into(),
                shapes: vec![
                    Shape::Rect {
                        x: 10,
                        y: 10,
                        w: 4,
                        h: 3,
                        color: colors::GREEN,
                        label: None,
                    },
                    Shape::Band {
                        y: 20,
                        h: 1,
                        color: colors::BLUE,
                        label: None,
                    },
                    Shape::Cells {
                        x: 0,
                        y: 30,
                        cell_w: 2,
                        cell_h: 1,
                        cols: 2,
                        rows: 1,
                        cells: vec![0, u8::MAX],
                    },
                    // Off the picture: clipped, no panic.
                    Shape::Rect {
                        x: -5,
                        y: 60,
                        w: 100,
                        h: 100,
                        color: colors::RED,
                        label: Some("X".into()),
                    },
                ],
                legend: vec![LegendEntry {
                    color: colors::WHITE,
                    text: "busy".into(),
                }],
            }],
            beam: Some((5, 40)),
            grid: vec![],
        };
        set.draw(&mut img);
        assert_eq!(at(&img, 10, 10), [64, 230, 64]);
        assert_eq!(at(&img, 13, 12), [64, 230, 64]);
        assert_eq!(at(&img, 11, 11), [0, 0, 0], "outline only");
        assert!(at(&img, 30, 20)[2] > 0, "band tints the whole row");
        assert!(at(&img, 1, 30)[0] > 0, "cell 0 drawn");
        assert_eq!(at(&img, 3, 30), [0, 0, 0], "cell 1 empty");
        assert_eq!(at(&img, 40, 5), [255, 230, 0], "beam cross");
        assert!(at(&img, 0, 5)[0] > 0, "beam line");
    }

    #[test]
    fn labels_render() {
        let mut img = black(32, 8);
        text(&mut img, 0, 0, "1A", colors::WHITE);
        assert_eq!(at(&img, 1, 0), [255, 255, 255], "top of the 1");
        assert_eq!(at(&img, 0, 0), [0, 0, 0]);
    }
}
