//! Debug overlays of the Atari 8-bit computers (see `emu_core::overlay`):
//! the display list's mode lines, where players and missiles were shown and
//! where display-list interrupts fired, recorded per field while switched
//! on. The record is debugger state, not machine state: it is not in
//! savestates, and with every overlay off the machine keeps no recorder at
//! all (one `Option` test per cycle).
//!
//! Frame coordinates: raster line `l` is frame row `l − 8` (lines 8–247
//! are shown), colour clock `cc` is frame column `2·(cc − 32)`, so cycle
//! `c` starts at column `4c − 64` (Altirra Hardware Reference Manual,
//! "ANTIC": two colour clocks per cycle; GTIA's output from colour clock
//! 32, as in `chips_atari::gtia`).
//!
//! Approximations: players and missiles are sampled from the GTIA
//! registers (GRAFPx/GRAFM, HPOSPx/HPOSMx, SIZEPx/SIZEM) at the end of each
//! line, so changes within a line (re-used players) show as the line's
//! last state; PRIOR (hidden behind the playfield, fifth player) is not
//! considered.

use chips_atari::antic::{Antic, CYCLES_PER_LINE, FIRST_LINE, VBLANK_LINE};
use chips_atari::gtia::Gtia;
use emu_core::frame::Rgb8;
use emu_core::overlay::{colors, LegendEntry, Overlay, OverlayInfo, Shape};

/// Overlay ids, in the order of [`Recorder`]'s switches.
pub const DLIST: &str = "dlist";
pub const PLAYERS: &str = "players";
pub const DLI: &str = "dli";
const IDS: [&str; 3] = [DLIST, PLAYERS, DLI];

/// Visible colour clocks (outside horizontal blank), as GTIA draws them.
const VISIBLE_CC: (u16, u16) = (0x22, 0xde);
const OBJECT_LABELS: [&str; 8] = ["P0", "P1", "P2", "P3", "M0", "M1", "M2", "M3"];

pub fn infos() -> Vec<OverlayInfo> {
    vec![
        OverlayInfo {
            id: DLIST,
            name: "ANTIC display list mode lines".into(),
        },
        OverlayInfo {
            id: PLAYERS,
            name: "GTIA players and missiles".into(),
        },
        OverlayInfo {
            id: DLI,
            name: "ANTIC display-list interrupts".into(),
        },
    ]
}

/// The frame row of raster line `line`.
pub fn frame_row(line: u16) -> Option<u32> {
    (FIRST_LINE..VBLANK_LINE)
        .contains(&line)
        .then(|| u32::from(line - FIRST_LINE))
}

/// Lines `y0..=y1` on which an object covered colour clocks `x0..x1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Run {
    y0: u16,
    y1: u16,
    x0: u16,
    x1: u16,
}

/// What one field recorded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FieldRec {
    /// Display-list instructions: (line fetched, instruction).
    instrs: Vec<(u16, u8)>,
    /// Players 0–3, missiles 0–3.
    runs: [Vec<Run>; 8],
    /// DLIs: (line, cycle of the NMI).
    dlis: Vec<(u16, u16)>,
}

/// Width multiplier of a 2-bit GTIA size field.
fn size_mult(size: u8) -> u16 {
    match size & 3 {
        1 => 2,
        3 => 4,
        _ => 1,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recorder {
    on: [bool; 3],
    cur: FieldRec,
    last: FieldRec,
}

impl Recorder {
    /// A recorder for the overlays among `ids`; `None` if there are none.
    pub fn new(ids: &[&str]) -> Option<Self> {
        let on = IDS.map(|id| ids.contains(&id));
        on.iter().any(|&o| o).then(|| Recorder {
            on,
            ..Recorder::default()
        })
    }

    pub fn is_on(&self, id: &str) -> bool {
        IDS.iter()
            .position(|&i| i == id)
            .is_some_and(|n| self.on[n])
    }

    /// Forget what was recorded (after loading a savestate).
    pub fn clear(&mut self) {
        self.cur = FieldRec::default();
        self.last = FieldRec::default();
    }

    /// After the chips' work of a cycle, before ANTIC's beam moves on.
    pub fn cycle(&mut self, antic: &Antic, gtia: &Gtia, nmi: bool) {
        let line = antic.line();
        if !(FIRST_LINE..VBLANK_LINE).contains(&line) {
            return;
        }
        if self.on[0] {
            if let Some(ir) = antic.fetched_instr() {
                self.cur.instrs.push((line, ir));
            }
        }
        if self.on[2] && nmi && antic.nmist & 0x80 != 0 {
            self.cur.dlis.push((line, antic.cycle()));
        }
        if self.on[1] && antic.cycle() == CYCLES_PER_LINE - 1 {
            self.sample_objects(gtia, line);
        }
    }

    /// The field ended: it becomes the last complete one.
    pub fn end_field(&mut self) {
        self.last = std::mem::take(&mut self.cur);
    }

    fn sample_objects(&mut self, gtia: &Gtia, line: u16) {
        for n in 0..8 {
            let span = if n < 4 {
                let g = gtia.grafp[n];
                (g != 0).then(|| {
                    let mult = size_mult(gtia.sizep[n]);
                    let first = g.leading_zeros() as u16;
                    let last = 7 - g.trailing_zeros() as u16;
                    let x = u16::from(gtia.hposp[n]);
                    (x + first * mult, x + (last + 1) * mult)
                })
            } else {
                let m = n - 4;
                let bits = (gtia.grafm >> (2 * m)) & 3;
                (bits != 0).then(|| {
                    let mult = size_mult(gtia.sizem >> (2 * m));
                    let first = u16::from(bits & 2 == 0);
                    let last = u16::from(bits & 1 != 0);
                    let x = u16::from(gtia.hposm[m]);
                    (x + first * mult, x + (last + 1) * mult)
                })
            };
            let Some((x0, x1)) = span else { continue };
            let (x0, x1) = (x0.max(VISIBLE_CC.0), x1.min(VISIBLE_CC.1));
            if x0 >= x1 {
                continue;
            }
            let runs = &mut self.cur.runs[n];
            match runs.last_mut() {
                Some(r) if r.y1 + 1 == line => {
                    r.y1 = line;
                    r.x0 = r.x0.min(x0);
                    r.x1 = r.x1.max(x1);
                }
                _ => runs.push(Run {
                    y0: line,
                    y1: line,
                    x0,
                    x1,
                }),
            }
        }
    }

    /// Overlay `id`: the last complete field, or the field so far when the
    /// beam is inside one (halted mid-frame).
    pub fn overlay(&self, id: &str, mid_field: bool) -> Option<Overlay> {
        if !self.is_on(id) {
            return None;
        }
        let rec = if mid_field { &self.cur } else { &self.last };
        let info = infos().into_iter().find(|i| i.id == id)?;
        let (shapes, legend) = match info.id {
            DLIST => dlist_shapes(rec),
            PLAYERS => player_shapes(rec),
            _ => dli_shapes(rec),
        };
        Some(Overlay {
            id: info.id,
            name: info.name,
            shapes,
            legend,
        })
    }
}

fn row(line: u16) -> i32 {
    i32::from(line) - i32::from(FIRST_LINE)
}

/// Label of a display-list instruction: the mode in hex (BLK, JMP, JVB)
/// and its flags.
fn instr_label(ir: u8) -> String {
    let mode = ir & 0x0f;
    let mut s = match mode {
        0 => "BLK".to_string(),
        1 if ir & 0x40 != 0 => "JVB".to_string(),
        1 => "JMP".to_string(),
        m => format!("{m:X}"),
    };
    let mut flag = |bit: u8, name: &str| {
        if ir & bit != 0 {
            s.push(' ');
            s.push_str(name);
        }
    };
    if mode >= 2 {
        flag(0x40, "LMS");
    }
    flag(0x80, "DLI");
    if mode >= 2 {
        flag(0x10, "HS");
        flag(0x20, "VS");
    }
    s
}

fn instr_color(ir: u8) -> Rgb8 {
    match ir & 0x0f {
        0 => colors::GREY,
        1 => colors::ORANGE,
        m => colors::SERIES[usize::from(m) % 6],
    }
}

/// One band per run of mode lines: an instruction and the following
/// identical ones (not LMS, not jumps) up to the next different one.
fn dlist_shapes(rec: &FieldRec) -> (Vec<Shape>, Vec<LegendEntry>) {
    let mut shapes = Vec::new();
    let mut i = 0;
    let n = rec.instrs.len();
    while i < n {
        let (line, ir) = rec.instrs[i];
        let mode = ir & 0x0f;
        let mergeable = mode != 1 && !(mode >= 2 && ir & 0x40 != 0);
        let mut j = i + 1;
        while mergeable && j < n && rec.instrs[j].1 == ir {
            j += 1;
        }
        // A JVB waits for vertical blank: its band runs to the end.
        let end = if mode == 1 && ir & 0x40 != 0 {
            VBLANK_LINE
        } else {
            rec.instrs.get(j).map_or(VBLANK_LINE, |&(l, _)| l)
        };
        let mut label = instr_label(ir);
        if j - i > 1 {
            label.push_str(&format!(" x{}", j - i));
        }
        shapes.push(Shape::Band {
            y: row(line),
            h: u32::from(end.saturating_sub(line).max(1)),
            color: instr_color(ir),
            label: Some(label),
        });
        i = j;
    }
    let legend = vec![
        LegendEntry {
            color: colors::GREY,
            text: "blank lines".into(),
        },
        LegendEntry {
            color: colors::ORANGE,
            text: "JMP / JVB".into(),
        },
    ];
    (shapes, legend)
}

fn player_shapes(rec: &FieldRec) -> (Vec<Shape>, Vec<LegendEntry>) {
    let mut shapes = Vec::new();
    for (n, runs) in rec.runs.iter().enumerate() {
        for r in runs {
            shapes.push(Shape::Rect {
                x: 2 * (i32::from(r.x0) - 32),
                y: row(r.y0),
                w: 2 * u32::from(r.x1 - r.x0),
                h: u32::from(r.y1 - r.y0 + 1),
                color: colors::SERIES[n],
                label: Some(OBJECT_LABELS[n].into()),
            });
        }
    }
    let legend = OBJECT_LABELS
        .iter()
        .zip(colors::SERIES)
        .map(|(t, color)| LegendEntry {
            color,
            text: (*t).into(),
        })
        .collect();
    (shapes, legend)
}

fn dli_shapes(rec: &FieldRec) -> (Vec<Shape>, Vec<LegendEntry>) {
    let shapes = rec
        .dlis
        .iter()
        .map(|&(line, cycle)| Shape::Mark {
            x: i32::from((cycle * 4).saturating_sub(64)),
            y: row(line),
            color: colors::YELLOW,
            label: Some("DLI".into()),
        })
        .collect();
    let legend = vec![LegendEntry {
        color: colors::YELLOW,
        text: "DLI (NMI)".into(),
    }];
    (shapes, legend)
}
