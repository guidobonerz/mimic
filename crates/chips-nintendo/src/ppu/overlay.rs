//! Debug overlays of the 2C02/2C07 (see `emu_core::overlay`): `sprites`,
//! a box per sprite on the lines sprite evaluation put it on (so a sprite
//! moved or multiplexed during the frame shows where it really was), and
//! `sprite0`, a mark where the sprite-0 hit happened (NESdev wiki, "PPU
//! sprite evaluation", "PPU OAM" and "PPU registers": PPUSTATUS bit 6).
//!
//! Recording happens only while an overlay is switched on (one flag test
//! per evaluated line otherwise); the record is not machine state. The
//! machine ends the frame with [`Ppu::overlay_end_frame`] at the start of
//! vertical blank, where its frames end.

use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

use super::{Ppu, BORDER, HEIGHT};
use crate::overlay::{sprite_rects, Record, Span};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PpuOverlays {
    pub(super) sprites: bool,
    pub(super) sprite0: bool,
    spans: Record<Span>,
    /// Pixel x (0–255) and line of the sprite-0 hit.
    hits: Record<(u16, u16)>,
}

impl PpuOverlays {
    /// Sprite `n` found for line `line` at OAM X `x`.
    pub(super) fn record_sprite(&mut self, n: usize, line: u16, x: u8) {
        if usize::from(line) < HEIGHT {
            self.spans.cur.push(Span {
                n: n as u8,
                line,
                x: (BORDER + usize::from(x)) as i32,
                w: 8,
            });
        }
    }

    pub(super) fn record_hit(&mut self, x: usize, line: u16) {
        self.hits.cur.push((x as u16, line));
    }
}

/// The PPU's overlays.
pub fn infos(chip: &str) -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: "sprites",
            name: format!("{chip} sprite boxes"),
        },
        OverlayInfo {
            id: "sprite0",
            name: format!("{chip} sprite-0 hit"),
        },
    ]
}

impl Ppu {
    /// Switch recording on for the PPU's overlays in `ids`, off (and
    /// cleared) for the others.
    pub fn set_overlays(&mut self, ids: &[&str]) {
        let ov = &mut self.ov;
        ov.sprites = ids.contains(&"sprites");
        ov.sprite0 = ids.contains(&"sprite0");
        if !ov.sprites {
            ov.spans.clear();
        }
        if !ov.sprite0 {
            ov.hits.clear();
        }
    }

    pub fn overlays_on(&self) -> bool {
        self.ov.sprites || self.ov.sprite0
    }

    /// End of a frame: the record becomes the last complete one.
    pub fn overlay_end_frame(&mut self) {
        self.ov.spans.end_frame();
        self.ov.hits.end_frame();
    }

    /// Overlay `id` of the last complete frame (`last`) or the frame so
    /// far; None for an unknown id or one switched off.
    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        let info = infos(self.model.chip_name())
            .into_iter()
            .find(|i| i.id == id)?;
        let (shapes, legend) = match id {
            "sprites" if self.ov.sprites => (
                sprite_rects(self.ov.spans.get(last), |l| Some(u32::from(l))),
                vec![],
            ),
            "sprite0" if self.ov.sprite0 => {
                let shapes = self
                    .ov
                    .hits
                    .get(last)
                    .iter()
                    .map(|&(x, line)| Shape::Mark {
                        x: (BORDER + usize::from(x)) as i32,
                        y: i32::from(line),
                        color: colors::YELLOW,
                        label: Some(format!("sprite 0 hit {line}:{}", x + 1)),
                    })
                    .collect();
                let legend = vec![LegendEntry {
                    color: colors::YELLOW,
                    text: "sprite-0 hit (line:dot)".into(),
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
