//! Debug overlays of the TMS9918A/TMS9929A (see `emu_core::overlay`):
//! `sprites`, a box per sprite where the VDP drew it, line by line as it
//! found the sprites (so a sprite moved during the frame shows where it
//! really was), and `limit`, a band on each line where a fifth sprite was
//! found (the 5S condition; TI data manual, section 2.3, "Sprites").
//!
//! Recording happens only while an overlay is switched on: the renderer
//! tests one flag per line otherwise. The record is not machine state
//! (never saved); the machine swaps it at the end of each field
//! ([`Tms9918::overlay_end_frame`]) so a finished field can be shown while
//! the next is drawn.

use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

use super::Tms9918;

/// One sprite on one line: sprite number, raster line, first frame column
/// and width in frame pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    n: u8,
    line: u16,
    x: i32,
    w: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Field {
    spans: Vec<Span>,
    /// Lines where a fifth sprite was found.
    limit: Vec<u16>,
}

/// The overlay switches and the records of the current and the last field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VdpOverlays {
    pub(super) sprites: bool,
    pub(super) limit: bool,
    cur: Field,
    last: Field,
}

impl VdpOverlays {
    pub(super) fn record_sprite(&mut self, n: u8, line: u16, x: i32, w: u32) {
        self.cur.spans.push(Span { n, line, x, w });
    }

    pub(super) fn record_limit(&mut self, line: u16) {
        if self.cur.limit.last() != Some(&line) {
            self.cur.limit.push(line);
        }
    }
}

/// The overlays a TMS9918A offers.
pub fn infos(chip: &str) -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "sprites",
            name: format!("{chip} sprite boxes"),
        },
        OverlayInfo {
            id: "limit",
            name: format!("{chip} fifth-sprite lines"),
        },
    ]
}

impl Tms9918 {
    /// Switch recording on for the overlays in `ids`, off (and cleared)
    /// for the others.
    pub fn set_overlays(&mut self, ids: &[&str]) {
        let sprites = ids.contains(&"sprites");
        let limit = ids.contains(&"limit");
        if !sprites {
            self.ov.cur.spans.clear();
            self.ov.last.spans.clear();
        }
        if !limit {
            self.ov.cur.limit.clear();
            self.ov.last.limit.clear();
        }
        self.ov.sprites = sprites;
        self.ov.limit = limit;
    }

    /// Whether any overlay records.
    pub fn overlays_on(&self) -> bool {
        self.ov.sprites || self.ov.limit
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
        let info = infos(self.model.chip_name())
            .into_iter()
            .find(|i| i.id == id)?;
        let (shapes, legend) = match id {
            "sprites" if self.ov.sprites => (self.sprite_rects(&f.spans), vec![]),
            "limit" if self.ov.limit => {
                let mut shapes = Vec::new();
                for (first, n) in runs(&f.limit) {
                    if let Some(y) = self.line_row(first) {
                        shapes.push(Shape::Band {
                            y: y as i32,
                            h: n,
                            color: colors::RED,
                            label: Some(format!("5S line {first}")),
                        });
                    }
                }
                let legend = vec![LegendEntry {
                    color: colors::RED,
                    text: "a fifth sprite on the line (5S)".into(),
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

    /// Boxes from the per-line spans: runs of consecutive lines with the
    /// same sprite at the same column and width.
    fn sprite_rects(&self, spans: &[Span]) -> Vec<Shape> {
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
            let last = sorted[j - 1].line;
            if let (Some(top), Some(bottom)) = (self.line_row(s.line), self.line_row(last)) {
                shapes.push(Shape::Rect {
                    x: s.x,
                    y: top as i32,
                    w: s.w,
                    h: bottom - top + 1,
                    color: colors::SERIES[usize::from(s.n) % 8],
                    label: Some(s.n.to_string()),
                });
            }
            i = j;
        }
        shapes
    }
}

/// Runs of consecutive line numbers: (first, length).
fn runs(lines: &[u16]) -> Vec<(u16, u32)> {
    let mut out: Vec<(u16, u32)> = Vec::new();
    for &l in lines {
        match out.last_mut() {
            Some((first, n)) if u32::from(*first) + *n == u32::from(l) => *n += 1,
            _ => out.push((l, 1)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vdp::{VdpModel, LINE};

    /// Graphics I, 8×8 sprites unmagnified, attributes at $1B00, patterns
    /// at $3800; five sprites on lines 10–17 (Y = 9).
    fn scene() -> Tms9918 {
        let mut vdp = Tms9918::new(VdpModel::Ntsc);
        vdp.regs = [0, 0x40, 0x06, 0x80, 0x00, 0x36, 0x07, 0x04];
        for n in 0..5usize {
            let a = 0x1b00 + n * 4;
            vdp.vram[a..a + 4].copy_from_slice(&[9, 20 + 10 * n as u8, 0, 15]);
        }
        vdp.vram[0x1b00 + 5 * 4] = 0xd0;
        vdp
    }

    fn run_field(vdp: &mut Tms9918) {
        for _ in 0..u32::from(LINE) * u32::from(vdp.model.lines()) {
            if vdp.tick() {
                vdp.overlay_end_frame();
            }
        }
    }

    #[test]
    fn sprite_boxes_and_limit_bands() {
        let mut vdp = scene();
        run_field(&mut vdp);
        assert!(vdp.overlay("sprites", true).is_none(), "switched off");
        assert_eq!(vdp.ov, VdpOverlays::default(), "nothing recorded");
        vdp.set_overlays(&["sprites", "limit"]);
        run_field(&mut vdp);
        let o = vdp.overlay("sprites", true).unwrap();
        assert_eq!(o.shapes.len(), 4, "the fifth sprite is not drawn");
        let top = 10 + u32::from(VdpModel::Ntsc.top());
        assert_eq!(
            o.shapes[1],
            Shape::Rect {
                x: 13 + 30,
                y: top as i32,
                w: 8,
                h: 8,
                color: colors::SERIES[1],
                label: Some("1".into()),
            }
        );
        let l = vdp.overlay("limit", true).unwrap();
        assert_eq!(
            l.shapes,
            [Shape::Band {
                y: top as i32,
                h: 8,
                color: colors::RED,
                label: Some("5S line 10".into()),
            }]
        );
        vdp.set_overlays(&[]);
        assert!(vdp.overlay("limit", true).is_none());
        assert_eq!(vdp.ov, VdpOverlays::default());
    }

    #[test]
    fn line_rows() {
        let vdp = Tms9918::new(VdpModel::Pal);
        assert_eq!(vdp.line_row(0), Some(54));
        assert_eq!(vdp.line_row(191 + 48), Some(54 + 191 + 48));
        assert_eq!(vdp.line_row(192 + 48), None, "blanking");
        assert_eq!(vdp.line_row(313 - 54), Some(0), "top border");
        assert_eq!(vdp.line_row(313), None);
    }
}
