//! Debug overlay (see `emu_core::overlay`): `modes`, the video mode each
//! visible line was drawn in.
//!
//! The mode of a line is sampled at its first visible cycle (horizontal
//! state $58, [`crate::video::FIRST_VISIBLE`]), from the soft switches as
//! the scanner uses them ([`crate::video`]): TEXT, the mixed-mode text
//! window (rows 20–23), HIRES, and on the IIe 80COL with AN3 off for the
//! double modes. A switch changed later in the line is not shown.
//!
//! The record is not machine state: it is kept only while the overlay is
//! switched on, double-buffered (the frame being drawn and the last
//! complete one) and never saved.

use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

use crate::video::{self, Modes, Video};

/// A line's mode (index into [`MODES`]); `NONE` = not recorded.
const NONE: u8 = u8::MAX;

/// Mode names and colours, by code.
const MODES: [(&str, emu_core::frame::Rgb8); 6] = [
    ("TEXT", colors::WHITE),
    ("80COL", colors::CYAN),
    ("LORES", colors::GREEN),
    ("DLR", colors::YELLOW),
    ("HIRES", colors::MAGENTA),
    ("DHR", colors::ORANGE),
];

pub const MODES_ID: &str = "modes";

/// Per-line record of the modes overlay.
#[derive(Clone, Debug, Default)]
pub struct ModesRecord {
    pub on: bool,
    cur: Vec<u8>,
    last: Vec<u8>,
}

/// The mode code line `line` (0–191) shows with switches `m`; `aux`: an
/// auxiliary bank is present (80-column modes possible).
pub fn mode_code(m: &Modes, line: u16, aux: bool) -> u8 {
    let text = m.text || (m.mixed && line >= 160);
    let double = m.col80 && aux;
    match (text, m.hires) {
        (true, _) => u8::from(double),
        (false, false) => 2 + u8::from(double && m.dhires),
        (false, true) => 4 + u8::from(double && m.dhires),
    }
}

impl ModesRecord {
    pub fn set(&mut self, on: bool) {
        if on && !self.on {
            self.cur = vec![NONE; usize::from(video::ACTIVE_LINES)];
            self.last = self.cur.clone();
        }
        if !on {
            self.cur = Vec::new();
            self.last = Vec::new();
        }
        self.on = on;
    }

    /// Before the scanner's cycle: sample the line at its first visible
    /// cycle.
    pub fn sample(&mut self, v: &Video, m: &Modes, aux: bool) {
        if v.h == video::FIRST_VISIBLE && v.line < video::ACTIVE_LINES {
            self.cur[usize::from(v.line)] = mode_code(m, v.line, aux);
        }
    }

    /// A frame ended: it becomes the last complete one.
    pub fn frame_done(&mut self) {
        std::mem::swap(&mut self.cur, &mut self.last);
        self.cur.fill(NONE);
    }

    /// Lines drawn so far in this frame (before `line_now`) from the
    /// current record, the rest from the last complete frame.
    fn line(&self, line: usize, line_now: u16) -> u8 {
        if line < usize::from(line_now) && self.cur[line] != NONE {
            self.cur[line]
        } else {
            self.last[line]
        }
    }

    pub fn info() -> OverlayInfo {
        OverlayInfo {
            id: MODES_ID,
            name: "Apple II video mode per line".into(),
        }
    }

    /// Bands of equal mode with a label at each change; `None` while off.
    pub fn overlay(&self, line_now: u16) -> Option<Overlay> {
        if !self.on {
            return None;
        }
        let mut shapes = Vec::new();
        let mut start = 0usize;
        let n = usize::from(video::ACTIVE_LINES);
        for l in 1..=n {
            let code = self.line(start, line_now);
            if l < n && self.line(l, line_now) == code {
                continue;
            }
            if let Some(&(name, color)) = MODES.get(usize::from(code)) {
                shapes.push(Shape::Band {
                    y: (start + video::BORDER_Y as usize) as i32,
                    h: (l - start) as u32,
                    color,
                    label: Some(name.into()),
                });
            }
            start = l;
        }
        Some(Overlay {
            id: MODES_ID,
            name: Self::info().name,
            shapes,
            legend: MODES
                .iter()
                .map(|&(text, color)| LegendEntry {
                    color,
                    text: text.into(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Apple2;
    use emu_core::registry::MachineConfig;
    use emu_core::{DebugHooks, Machine, StopReason};

    fn machine() -> Apple2 {
        let mut config = MachineConfig {
            model: Some("iie".into()),
            ..MachineConfig::default()
        };
        config.options.insert("roms".into(), "stub".into());
        Apple2::new(&config).unwrap()
    }

    fn frame(m: &mut Apple2) {
        assert_eq!(m.run_frame(&mut DebugHooks::new()), StopReason::FrameDone);
    }

    #[test]
    fn frame_rows_and_beam_columns_match_the_frame() {
        let mut m = machine();
        assert_eq!(m.frame_row(0), Some(video::BORDER_Y));
        assert_eq!(m.frame_row(191), Some(191 + video::BORDER_Y));
        assert_eq!(m.frame_row(192), None);
        m.board.video.h = video::FIRST_VISIBLE + 1;
        let want = video::BORDER_X as usize + video::PHASE_PX + video::DOTS * video::PX_PER_DOT;
        assert_eq!(usize::from(m.beam().unwrap().hpos), want);
        m.board.video.h = 3;
        assert_eq!(m.beam().unwrap().hpos, 0, "horizontal blanking");
    }

    #[test]
    fn modes_are_recorded_only_while_switched_on() {
        let mut m = machine();
        assert_eq!(m.overlays()[0].id, MODES_ID);
        frame(&mut m);
        assert!(m.overlay(MODES_ID).is_none(), "off");
        assert!(m.modes_rec.cur.is_empty(), "nothing recorded while off");
        m.set_overlays(&[MODES_ID]);
        frame(&mut m);
        // The stub switched TEXT on: one band of 192 text lines.
        let o = m.overlay(MODES_ID).unwrap();
        assert_eq!(
            o.shapes,
            vec![Shape::Band {
                y: video::BORDER_Y as i32,
                h: 192,
                color: colors::WHITE,
                label: Some("TEXT".into()),
            }]
        );
        m.board.modes.text = false;
        m.board.modes.hires = true;
        m.board.modes.mixed = true;
        frame(&mut m);
        let o = m.overlay(MODES_ID).unwrap();
        let bands: Vec<_> = o
            .shapes
            .iter()
            .map(|s| match s {
                Shape::Band { y, h, label, .. } => (*y, *h, label.clone().unwrap()),
                _ => panic!("band expected"),
            })
            .collect();
        let top = video::BORDER_Y as i32;
        assert_eq!(
            bands,
            vec![(top, 160, "HIRES".into()), (top + 160, 32, "TEXT".into())]
        );
        m.set_overlays(&[]);
        assert!(m.overlay(MODES_ID).is_none());
    }

    #[test]
    fn double_modes_need_80col_and_aux() {
        let m = Modes {
            hires: true,
            col80: true,
            dhires: true,
            ..Modes::default()
        };
        assert_eq!(MODES[usize::from(mode_code(&m, 0, true))].0, "DHR");
        assert_eq!(MODES[usize::from(mode_code(&m, 0, false))].0, "HIRES");
        let t = Modes {
            text: true,
            col80: true,
            ..Modes::default()
        };
        assert_eq!(MODES[usize::from(mode_code(&t, 0, true))].0, "80COL");
        let l = Modes {
            col80: true,
            dhires: true,
            ..Modes::default()
        };
        assert_eq!(MODES[usize::from(mode_code(&l, 0, true))].0, "DLR");
    }
}
