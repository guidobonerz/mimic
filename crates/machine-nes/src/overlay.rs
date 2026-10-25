//! The NES's debug overlays (see `emu_core::overlay`): the PPU's
//! `sprites` and `sprite0` (recorded by `chips-nintendo`) and `irq`, a
//! mark at the line and dot where an interrupt request rose: the PPU's
//! /NMI (vertical blank), the cartridge's /IRQ (MMC3 scanline counter) and
//! the APU's (frame counter, DMC). A frame's record runs from one start of
//! vertical blank to the next, where `run_frame` returns. Not machine
//! state: never saved.

use chips_nintendo::ppu::{BORDER, HEIGHT, WIDTH};
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

use chips_nintendo::overlay::Record;

/// Who raised an interrupt request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Nmi,
    Cart,
    Apu,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Nmi => "NMI",
            Source::Cart => "IRQ",
            Source::Apu => "APU IRQ",
        }
    }

    fn color(self) -> emu_core::frame::Rgb8 {
        match self {
            Source::Nmi => colors::MAGENTA,
            Source::Cart => colors::CYAN,
            Source::Apu => colors::ORANGE,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Overlays {
    /// Overlays switched on (kept over a power cycle).
    pub ids: Vec<&'static str>,
    pub irq: bool,
    /// Request levels after the last cycle: NMI, cartridge, APU.
    levels: [bool; 3],
    marks: Record<(Source, u16, u16)>,
    /// The beam is at the start of a frame: `overlay` shows the last one.
    pub fresh: bool,
}

/// The frame column of PPU dot `dot` ([`emu_core::BeamPos::hpos`]): dot
/// d (1–256) outputs picture pixel d − 1 at column `BORDER + d − 1`; dot 0
/// (idle) is the left border's last column, and the dots after the
/// picture (257–340: horizontal blank) clamp to the right edge.
pub(crate) fn dot_column(dot: u16) -> u16 {
    (BORDER as i32 + i32::from(dot) - 1).clamp(0, WIDTH as i32 - 1) as u16
}

pub(crate) fn infos(chip: &str) -> Vec<OverlayInfo> {
    let mut v = chips_nintendo::ppu::overlay::infos(chip);
    v.push(OverlayInfo {
        id: "irq",
        name: "NMI and IRQ requests".into(),
    });
    v
}

impl Overlays {
    pub fn set(&mut self, ids: &[&str], chip: &str) {
        self.ids = infos(chip)
            .into_iter()
            .map(|o| o.id)
            .filter(|id| ids.contains(id))
            .collect();
        self.irq = self.ids.contains(&"irq");
        if !self.irq {
            self.marks.clear();
        }
    }

    pub fn any(&self) -> bool {
        !self.ids.is_empty()
    }

    /// The request levels after a CPU cycle, with the beam position.
    pub fn sample(&mut self, levels: [bool; 3], line: u16, dot: u16) {
        const SOURCES: [Source; 3] = [Source::Nmi, Source::Cart, Source::Apu];
        for (i, &on) in levels.iter().enumerate() {
            if on && !self.levels[i] {
                self.marks.cur.push((SOURCES[i], line, dot));
            }
        }
        self.levels = levels;
    }

    pub fn end_frame(&mut self) {
        self.marks.end_frame();
        self.fresh = true;
    }

    /// The `irq` overlay. Marks sit at the dot's frame column and the
    /// line's row; requests outside the picture (vertical blank, the
    /// pre-render line, horizontal blank) are moved to its nearest edge,
    /// their label tells where they really were.
    pub fn irq_overlay(&self, last: bool) -> Option<Overlay> {
        if !self.irq {
            return None;
        }
        let shapes = self
            .marks
            .get(last)
            .iter()
            .map(|&(src, line, dot)| {
                let x = i32::from(dot_column(dot));
                let y = if usize::from(line) < HEIGHT {
                    i32::from(line)
                } else if line < 250 {
                    HEIGHT as i32 - 1
                } else {
                    0
                };
                Shape::Mark {
                    x,
                    y,
                    color: src.color(),
                    label: Some(format!("{} {line}:{dot}", src.name())),
                }
            })
            .collect();
        let legend = [Source::Nmi, Source::Cart, Source::Apu]
            .iter()
            .map(|s| LegendEntry {
                color: s.color(),
                text: s.name().into(),
            })
            .collect();
        Some(Overlay {
            id: "irq",
            name: "NMI and IRQ requests".into(),
            shapes,
            legend,
        })
    }
}
