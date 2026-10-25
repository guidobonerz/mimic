//! The SNES's debug overlays (see `emu_core::overlay`): the S-PPU's
//! `sprites` (recorded by `chips-nintendo`), `hdma`, a band on each line
//! whose registers HDMA rewrote (the transfer at the end of line v-1
//! takes effect on line v, so the band sits on line v; labelled with the
//! first write's channel and B-bus register), and `irq`, marks where the
//! H/V timer set TIMEUP and where the NMI rose (fullsnes, "SNES DMA and
//! HDMA", "SNES PPU Interrupts"). A frame's record runs from one start of
//! vertical blank to the next, where `run_frame` returns. Not machine
//! state: never saved.
//!
//! The marks' columns map the dot (H counter / 4) to the picture as dot
//! 22 = column 0, two columns a dot (fullsnes, "SNES Timing H/V Events":
//! the picture's first pixel near H = 22); dots outside the picture sit at
//! its nearest edge, their label tells line and dot.

use chips_nintendo::overlay::Record;
use chips_nintendo::sppu::overlay::row_of;
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Nmi,
    HvIrq,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Overlays {
    /// Overlays switched on (kept over a power cycle).
    pub ids: Vec<&'static str>,
    pub hdma: bool,
    pub irq: bool,
    /// (line, channel, B-bus register) of each line's first HDMA write.
    hdma_lines: Record<(u16, u8, u8)>,
    /// (event, line, dot).
    marks: Record<(Event, u16, u16)>,
    /// (interlace, field) of the current and the last frame.
    rows: [(bool, bool); 2],
    /// The beam is at the start of a frame: `overlay` shows the last one.
    pub fresh: bool,
}

/// The frame column of dot `dot` (H counter / 4) in the 512-column
/// picture ([`emu_core::BeamPos::hpos`]): dot 22 is the first pixel
/// (column 0), two columns a dot; dots before the picture clamp to 0,
/// after it (horizontal blank) to 511.
pub(crate) fn dot_column(dot: u16) -> u16 {
    ((i32::from(dot) - 22) * 2).clamp(0, 511) as u16
}

pub(crate) fn infos() -> Vec<OverlayInfo> {
    let mut v = chips_nintendo::sppu::overlay::infos();
    v.push(OverlayInfo {
        id: "hdma",
        name: "HDMA register writes per line".into(),
    });
    v.push(OverlayInfo {
        id: "irq",
        name: "H/V-IRQ and NMI".into(),
    });
    v
}

impl Overlays {
    pub fn set(&mut self, ids: &[&str]) {
        self.ids = infos()
            .into_iter()
            .map(|o| o.id)
            .filter(|id| ids.contains(id))
            .collect();
        self.hdma = self.ids.contains(&"hdma");
        self.irq = self.ids.contains(&"irq");
        if !self.hdma {
            self.hdma_lines.clear();
        }
        if !self.irq {
            self.marks.clear();
        }
    }

    pub fn any(&self) -> bool {
        !self.ids.is_empty()
    }

    /// An HDMA write in the transfer for line `line`.
    pub fn record_hdma(&mut self, line: u16, ch: usize, reg: u8, rows: (bool, bool)) {
        self.rows[0] = rows;
        if self.hdma_lines.cur.last().is_none_or(|l| l.0 != line) {
            self.hdma_lines.cur.push((line, ch as u8, reg));
        }
    }

    pub fn record_event(&mut self, e: Event, line: u16, dot: u16, rows: (bool, bool)) {
        self.rows[0] = rows;
        self.marks.cur.push((e, line, dot));
    }

    pub fn end_frame(&mut self) {
        self.hdma_lines.end_frame();
        self.marks.end_frame();
        self.rows[1] = self.rows[0];
        self.fresh = true;
    }

    pub fn overlay(&self, id: &str, last: bool) -> Option<Overlay> {
        let (il, field) = self.rows[usize::from(last)];
        let h = if il { 2 } else { 1 };
        let info = infos().into_iter().find(|i| i.id == id)?;
        let (shapes, legend) = match id {
            "hdma" if self.hdma => {
                let shapes = self
                    .hdma_lines
                    .get(last)
                    .iter()
                    .filter_map(|&(line, ch, reg)| {
                        let y = row_of(line, il, false)?;
                        Some(Shape::Band {
                            y: y as i32,
                            h,
                            color: colors::SERIES[usize::from(ch)],
                            label: Some(format!("ch{ch} $21{reg:02X}")),
                        })
                    })
                    .collect();
                let legend = (0..8)
                    .map(|ch| LegendEntry {
                        color: colors::SERIES[ch],
                        text: format!("HDMA channel {ch}"),
                    })
                    .collect();
                (shapes, legend)
            }
            "irq" if self.irq => {
                let last_row = if il { 477 } else { 238 };
                let shapes = self
                    .marks
                    .get(last)
                    .iter()
                    .map(|&(e, line, dot)| {
                        let y = row_of(line, il, field)
                            .map_or(if line == 0 { 0 } else { last_row }, |r| r as i32);
                        let (color, name) = match e {
                            Event::Nmi => (colors::MAGENTA, "NMI"),
                            Event::HvIrq => (colors::CYAN, "IRQ"),
                        };
                        Shape::Mark {
                            x: i32::from(dot_column(dot)),
                            y,
                            color,
                            label: Some(format!("{name} {line}:{dot}")),
                        }
                    })
                    .collect();
                let legend = vec![
                    LegendEntry {
                        color: colors::MAGENTA,
                        text: "NMI (vertical blank)".into(),
                    },
                    LegendEntry {
                        color: colors::CYAN,
                        text: "H/V timer IRQ (TIMEUP)".into(),
                    },
                ];
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
