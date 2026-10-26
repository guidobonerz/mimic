//! Debug overlays of the custom chips (see `emu_core::overlay`), drawn on
//! [`Custom::frame`]:
//!
//! - `dma`: what used each DMA slot (colour clock) of each line — refresh,
//!   disk, audio, sprites, bitplanes, Copper, Blitter, CPU or nobody (HRM
//!   chapter 7, "DMA time slot allocation"), one cell per colour clock,
//!   four frame pixels wide, at the frame column of that colour clock (the
//!   beam's `hpos`, 4 × (h − [`FIRST_CCK`])).
//! - `sprites`: a box around each run of lines on which a hardware sprite
//!   actually showed pixels (inside the display window after the line's
//!   bitplane data, or in the border with ECS BRDSPRT; behind-playfield
//!   priority is not considered); a sprite reused further down the field
//!   gets another box.
//! - `copper`: a mark at each register write the Copper made, at the beam
//!   position of the write.
//!
//! The record is not machine state: it is kept only while an overlay is
//! switched on ([`Custom::set_overlays`]), is not saved, and costs the
//! colour-clock path one test of an `Option` when off. It is double
//! buffered: the overlays show the last complete field while the beam is
//! in the vertical blank, and the field so far once it has passed it
//! (halted mid-frame).

use emu_core::frame::Rgb8;
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

use super::{Custom, FIRST_CCK, PAL_LINE, WIDTH};
use crate::regnames;

/// Colour clocks per recorded line (a line has at most 228; ECS
/// HTOTAL up to 256 more are not recorded).
const STRIDE: usize = 256;
/// Lines recorded per field.
const LINES: usize = 1024;
/// Sprite boxes and Copper moves kept per field.
const MAX_BOXES: usize = 256;
const MAX_MOVES: usize = 4096;

/// What used a DMA slot (index into the `dma` overlay's legend).
pub mod slot {
    pub const REFRESH: u8 = 0;
    pub const DISK: u8 = 1;
    pub const AUDIO: u8 = 2;
    pub const SPRITE: u8 = 3;
    pub const BITPLANE: u8 = 4;
    pub const COPPER: u8 = 5;
    pub const BLITTER: u8 = 6;
    pub const CPU: u8 = 7;
    pub const FREE: u8 = 8;
    /// Not recorded.
    pub const NONE: u8 = u8::MAX;
}

const LEGEND: [(&str, Rgb8); 9] = [
    ("refresh", colors::GREY),
    ("disk", colors::YELLOW),
    ("audio", colors::MAGENTA),
    ("sprite", colors::CYAN),
    ("bitplane", colors::BLUE),
    ("copper", colors::GREEN),
    ("blitter", colors::RED),
    ("cpu", colors::WHITE),
    (
        "free",
        Rgb8 {
            r: 0x30,
            g: 0x30,
            b: 0x30,
        },
    ),
];

/// A run of lines a sprite showed on: high-resolution pixel columns in
/// Denise line coordinates and raster lines, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpriteBox {
    pub sprite: u8,
    pub x0: u32,
    pub x1: u32,
    pub y0: u16,
    pub y1: u16,
}

/// A register write by the Copper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopperMove {
    pub v: u16,
    pub h: u16,
    pub reg: u16,
    pub value: u16,
}

/// One field's record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldRecord {
    /// [`slot`] codes, `LINES` × `STRIDE` (empty when `dma` is off).
    pub dma: Vec<u8>,
    pub sprites: Vec<SpriteBox>,
    pub moves: Vec<CopperMove>,
}

/// The overlays' record (not machine state).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayRecord {
    pub dma: bool,
    pub sprites: bool,
    pub copper: bool,
    pub current: FieldRecord,
    pub last: FieldRecord,
    open: [Option<SpriteBox>; 8],
}

impl OverlayRecord {
    fn new(dma: bool, sprites: bool, copper: bool) -> Self {
        let mut r = Self {
            dma,
            sprites,
            copper,
            current: FieldRecord::default(),
            last: FieldRecord::default(),
            open: [None; 8],
        };
        if dma {
            r.current.dma = vec![slot::NONE; LINES * STRIDE];
            r.last.dma = vec![slot::NONE; LINES * STRIDE];
        }
        r
    }

    /// Slot `h` of line `v` was used by `code`.
    #[inline]
    pub(crate) fn slot(&mut self, v: u16, h: u16, code: u8) {
        if let Some(b) = self
            .current
            .dma
            .get_mut(usize::from(v) * STRIDE + usize::from(h))
        {
            *b = code;
        }
    }

    pub(crate) fn copper_move(&mut self, v: u16, h: u16, reg: u16, value: u16) {
        if self.copper && self.current.moves.len() < MAX_MOVES {
            self.current.moves.push(CopperMove { v, h, reg, value });
        }
    }

    fn close(&mut self, n: usize) {
        if let Some(b) = self.open[n].take() {
            if self.current.sprites.len() < MAX_BOXES {
                self.current.sprites.push(b);
            }
        }
    }

    /// Denise finished raster line `v`; `seen[n]` is the first and last
    /// pixel sprite `n` showed (`u32::MAX, 0` if none).
    pub(crate) fn sprite_line(&mut self, v: u16, seen: &[(u32, u32); 8]) {
        for (n, &(x0, x1)) in seen.iter().enumerate() {
            if x0 > x1 {
                self.close(n);
                continue;
            }
            match &mut self.open[n] {
                Some(b) if b.y1 + 1 == v => {
                    b.x0 = b.x0.min(x0);
                    b.x1 = b.x1.max(x1);
                    b.y1 = v;
                }
                _ => {
                    self.close(n);
                    self.open[n] = Some(SpriteBox {
                        sprite: n as u8,
                        x0,
                        x1,
                        y0: v,
                        y1: v,
                    });
                }
            }
        }
    }

    /// The field ended: it becomes the last one.
    pub(crate) fn end_field(&mut self) {
        for n in 0..8 {
            self.close(n);
        }
        std::mem::swap(&mut self.current, &mut self.last);
        self.current.dma.fill(slot::NONE);
        self.current.sprites.clear();
        self.current.moves.clear();
    }
}

/// No sprite pixel seen on a line yet.
pub(crate) const UNSEEN: (u32, u32) = (u32::MAX, 0);

impl Custom {
    /// The overlays the chips offer.
    pub fn overlays() -> Vec<OverlayInfo> {
        vec![
            OverlayInfo {
                id: "dma",
                name: "Agnus DMA slots".into(),
            },
            OverlayInfo {
                id: "sprites",
                name: "Sprites shown".into(),
            },
            OverlayInfo {
                id: "copper",
                name: "Copper register writes".into(),
            },
        ]
    }

    /// Record the overlays in `ids` from now on (unknown ids are ignored;
    /// none switches recording off and drops the record).
    pub fn set_overlays(&mut self, ids: &[&str]) {
        let on = |id: &str| ids.contains(&id);
        let (dma, sprites, copper) = (on("dma"), on("sprites"), on("copper"));
        self.denise.rec_sprites = sprites;
        self.denise.spr_seen = [UNSEEN; 8];
        if !(dma || sprites || copper) {
            self.rec = None;
            return;
        }
        match self.rec.as_deref_mut() {
            Some(r) if r.dma == dma => {
                r.sprites = sprites;
                r.copper = copper;
            }
            _ => self.rec = Some(Box::new(OverlayRecord::new(dma, sprites, copper))),
        }
    }

    /// The frame row showing raster line `line` (the field as
    /// [`Custom::frame`] delivers it; an interlaced picture's fields are
    /// delivered one at a time).
    pub fn frame_row(&self, line: u16) -> Option<u32> {
        let row = u32::from(line.checked_sub(self.standard.top())?);
        (row < self.standard.height()).then_some(row)
    }

    /// Overlay `id`, if it is switched on.
    pub fn overlay(&self, id: &str) -> Option<Overlay> {
        let rec = self.rec.as_deref()?;
        let field = if self.v < self.standard.top() {
            &rec.last
        } else {
            &rec.current
        };
        let top = i32::from(self.standard.top());
        let first = FIRST_CCK as i32;
        // Columns per high-resolution pixel: 2 in 35 ns output.
        let k = (self.width() / WIDTH) as i32;
        let info = Self::overlays().into_iter().find(|o| o.id == id)?;
        let (shapes, legend) = match id {
            "dma" if rec.dma => {
                let cols = (u32::from(PAL_LINE) + 1 - FIRST_CCK).min(WIDTH / 4);
                let rows = self.standard.height();
                let mut cells = Vec::with_capacity((cols * rows) as usize);
                for r in 0..rows as usize {
                    let at = (r + top as usize) * STRIDE + FIRST_CCK as usize;
                    cells.extend_from_slice(&field.dma[at..at + cols as usize]);
                }
                let shape = Shape::Cells {
                    x: 0,
                    y: 0,
                    cell_w: 4 * k as u32,
                    cell_h: 1,
                    cols,
                    rows,
                    cells,
                };
                let legend = LEGEND
                    .iter()
                    .map(|&(text, color)| LegendEntry {
                        color,
                        text: text.into(),
                    })
                    .collect();
                (vec![shape], legend)
            }
            "sprites" if rec.sprites => {
                let shapes = field
                    .sprites
                    .iter()
                    .map(|b| Shape::Rect {
                        x: k * (b.x0 as i32 - 4 * first),
                        y: i32::from(b.y0) - top,
                        w: k as u32 * (b.x1 - b.x0 + 1),
                        h: u32::from(b.y1 - b.y0) + 1,
                        color: colors::SERIES[usize::from(b.sprite & 7)],
                        label: Some(b.sprite.to_string()),
                    })
                    .collect();
                let legend = (0..8)
                    .map(|n| LegendEntry {
                        color: colors::SERIES[n],
                        text: format!("sprite {n}"),
                    })
                    .collect();
                (shapes, legend)
            }
            "copper" if rec.copper => {
                let shapes = field
                    .moves
                    .iter()
                    .map(|m| Shape::Mark {
                        x: 4 * k * (i32::from(m.h) - first),
                        y: i32::from(m.v) - top,
                        color: colors::GREEN,
                        label: Some(format!("{}={:04X}", regnames::name(m.reg), m.value)),
                    })
                    .collect();
                let legend = vec![LegendEntry {
                    color: colors::GREEN,
                    text: "Copper MOVE".into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chipram::ChipRam;
    use crate::custom::{Chipset, Standard};
    use crate::{dma, reg};
    use emu_core::{Savestate, StateWriter};

    fn setup() -> (Custom, ChipRam) {
        let mut c = Custom::new(Standard::Pal, 0x7fffe, Chipset::OCS);
        for (r, v) in [
            (reg::BPLCON0, 0x1200),
            (reg::DIWSTRT, 0x2c81),
            (reg::DIWSTOP, 0x2cc1),
            (reg::DDFSTRT, 0x38),
            (reg::DDFSTOP, 0xd0),
            (reg::BPL1PTH, 0x0001),
            (reg::BPL1PTH + 2, 0x0000),
            (reg::DMACON, 0x8000 | dma::DMAEN | dma::BPLEN),
        ] {
            c.write(r, v);
        }
        (c, ChipRam::new(0x8_0000))
    }

    fn field(c: &mut Custom, chip: &mut ChipRam) {
        for _ in 0..200_000 {
            if c.tick(chip, false).field_done {
                return;
            }
        }
        panic!("no field end");
    }

    fn cell(o: &Overlay, x: u32, y: u32) -> u8 {
        match &o.shapes[0] {
            Shape::Cells { cols, cells, .. } => cells[(y * cols + x) as usize],
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn dma_slots_show_the_bitplane_fetches() {
        let (mut c, mut chip) = setup();
        assert!(c.overlay("dma").is_none(), "off: nothing");
        c.set_overlays(&["dma"]);
        field(&mut c, &mut chip);
        let o = c.overlay("dma").unwrap();
        assert_eq!(o.legend.len(), 9);
        assert_eq!(o.legend[usize::from(slot::BITPLANE)].text, "bitplane");
        let Shape::Cells {
            x,
            cell_w,
            cols,
            rows,
            ..
        } = o.shapes[0]
        else {
            panic!()
        };
        assert_eq!((x, cell_w, rows), (0, 4, 286));
        assert_eq!(cols, 228 - FIRST_CCK);
        // Line $2C is frame row $2C - 26; plane 1 is fetched in the last
        // slot of each 8-slot group from DDFSTRT $38: $3F, $47 … $D7.
        let row = 0x2c - 26;
        let col = |h: u32| h - FIRST_CCK;
        assert_eq!(cell(&o, col(0x3f), row), slot::BITPLANE);
        assert_eq!(cell(&o, col(0xd7), row), slot::BITPLANE);
        assert_eq!(cell(&o, col(0x3e), row), slot::FREE);
        assert_eq!(cell(&o, col(0xdf), row), slot::FREE);
        assert_eq!(cell(&o, col(0x3f), row - 1), slot::FREE, "above the window");
        // Off again: no record.
        c.set_overlays(&[]);
        assert!(c.overlay("dma").is_none());
        assert!(c.rec.is_none());
    }

    #[test]
    fn sprite_boxes_where_a_sprite_showed() {
        let (mut c, mut chip) = setup();
        // Sprite 0: VSTART $40, VSTOP $44, HSTART $90, leftmost pixel set.
        let words = [
            0x4048u16, 0x4400, 0x8000, 0, 0x8000, 0, 0x8000, 0, 0x8000, 0, 0, 0,
        ];
        for (i, w) in words.into_iter().enumerate() {
            chip.set_word(0x2000 + 2 * i as u32, w);
        }
        c.write(reg::SPR0PTH + 2, 0x2000);
        c.write(reg::DMACON, 0x8000 | dma::SPREN);
        c.set_overlays(&["sprites"]);
        assert!(c.overlay("dma").is_none(), "only what is switched on");
        field(&mut c, &mut chip);
        let o = c.overlay("sprites").unwrap();
        // The first pixel is one low-resolution pixel right of HSTART:
        // high-resolution pixel 2 × $91, frame column 2 × $91 − 4 × $38.
        assert_eq!(
            o.shapes,
            vec![Shape::Rect {
                x: 2 * 0x91 - 4 * 0x38,
                y: 0x40 - 26,
                w: 2,
                h: 4,
                color: colors::SERIES[0],
                label: Some("0".into()),
            }]
        );
    }

    #[test]
    fn copper_moves_are_marked_where_they_happen() {
        let (mut c, mut chip) = setup();
        for (i, w) in [0x5001u16, 0xfffe, 0x0180, 0x0f00, 0xffff, 0xfffe]
            .into_iter()
            .enumerate()
        {
            chip.set_word(0x4_0000 + 2 * i as u32, w);
        }
        c.write(reg::COP1LCH, 0x0004);
        c.write(reg::COP1LCL, 0x0000);
        c.write(reg::DMACON, 0x8000 | dma::COPEN);
        c.set_overlays(&["copper", "dma"]);
        field(&mut c, &mut chip);
        let o = c.overlay("copper").unwrap();
        assert_eq!(o.shapes.len(), 1);
        let Shape::Mark {
            x, y, ref label, ..
        } = o.shapes[0]
        else {
            panic!()
        };
        assert_eq!(y, 0x50 - 26);
        assert!((-4 * 0x38..-4 * 0x30).contains(&x), "{x}");
        assert_eq!(label.as_deref(), Some("COLOR00=0F00"));
        // The MOVE's slot (even, before $38: outside the picture's cells)
        // and its WAIT's fetches show as Copper slots in the record.
        let rec = c.rec.as_deref().unwrap();
        let h = (x / 4 + 0x38) as usize;
        assert_eq!(rec.last.dma[0x50 * STRIDE + h], slot::COPPER);
    }

    #[test]
    fn frame_rows_and_savestates() {
        let (mut c, mut chip) = setup();
        assert_eq!(c.frame_row(25), None);
        assert_eq!(c.frame_row(26), Some(0));
        assert_eq!(c.frame_row(26 + 285), Some(285));
        assert_eq!(c.frame_row(26 + 286), None);
        let save = |c: &Custom| {
            let mut w = StateWriter::new();
            c.save(&mut w);
            w.into_bytes()
        };
        let mut d = c.clone();
        c.set_overlays(&["dma", "sprites", "copper"]);
        field(&mut c, &mut chip.clone());
        field(&mut d, &mut chip);
        assert_eq!(save(&c), save(&d), "the record is not state");
        // Loading a state keeps the recording switched on.
        let s = save(&d);
        c.load(&mut emu_core::StateReader::new(&s)).unwrap();
        assert!(c.overlay("dma").is_some());
    }
}
