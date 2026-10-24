//! The profile picker: when mimic-desktop starts without a machine and a
//! profile file exists, the window first lists the start profiles
//! (`emu-profile`); a click on one starts its machine. The list is drawn
//! as a picture in the window, with the footer's font.

use emu_core::frame::{FieldParity, OutputType, SignalInfo, VideoStandard};
use noto_sans_mono_bitmap::RasterHeight;

use crate::emu::Frame;
use crate::footer::{self, Strip, BUSY, DIM, TEXT};

/// Picture width in pixels.
pub const WIDTH: u32 = 720;
const SIZE: RasterHeight = RasterHeight::Size20;
/// Lines above the first profile (title, file, blank).
const HEADER: usize = 3;
const HOVER: footer::Rgb = [0x30, 0x48, 0x38];
const ERROR: footer::Rgb = [0xFF, 0x60, 0x60];

pub struct Picker {
    /// Profile name and what it is.
    pub entries: Vec<(String, String)>,
    /// The profile file, for the title.
    pub file: String,
    pub hover: Option<usize>,
    /// Why the last chosen profile did not start.
    pub error: Option<String>,
}

fn line_height() -> u32 {
    footer::height(SIZE)
}

impl Picker {
    pub fn lines(&self) -> usize {
        HEADER + self.entries.len() + 2
    }

    /// The list as a picture.
    pub fn frame(&self) -> Frame {
        let lh = line_height();
        let height = lh * self.lines() as u32;
        let mut rgba = Vec::with_capacity((WIDTH * height * 4) as usize);
        let mut line = |text: &[(&str, footer::Rgb)], bg: Option<footer::Rgb>| {
            let mut s = Strip::new(WIDTH, SIZE);
            if let Some(bg) = bg {
                s.fill(0, WIDTH as usize, 0, bg);
            }
            let mut x = 16;
            for (t, c) in text {
                x = s.text(x, t, *c);
            }
            rgba.extend_from_slice(&s.px);
        };
        line(&[("mimic: choose a start profile", BUSY)], None);
        line(&[(&self.file, DIM)], None);
        line(&[], None);
        let width = self.entries.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (i, (name, what)) in self.entries.iter().enumerate() {
            let bg = (self.hover == Some(i)).then_some(HOVER);
            line(&[(&format!("{name:<width$}  "), TEXT), (what, DIM)], bg);
        }
        line(&[], None);
        match &self.error {
            Some(e) => line(&[(e, ERROR)], None),
            None => line(&[("click a profile to start it", DIM)], None),
        }
        Frame {
            width: WIDTH,
            height,
            rgba,
            signal: SignalInfo {
                pixel_clock_hz: WIDTH * height * 60,
                active_pixels: WIDTH,
                total_pixels: WIDTH,
                lines_per_field: height,
                standard: VideoStandard::Pal,
                output: OutputType::Rgb,
                field: FieldParity::Progressive,
            },
        }
    }

    /// The profile at picture row `y`.
    pub fn row_at(&self, y: u32) -> Option<usize> {
        let line = (y / line_height()) as usize;
        line.checked_sub(HEADER).filter(|&i| i < self.entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_map_to_profiles() {
        let p = Picker {
            entries: vec![("a".into(), "first".into()), ("b".into(), "second".into())],
            file: "profiles.yaml".into(),
            hover: Some(1),
            error: None,
        };
        let f = p.frame();
        assert_eq!(f.width, WIDTH);
        assert_eq!(f.rgba.len(), (f.width * f.height * 4) as usize);
        let lh = line_height();
        assert_eq!(p.row_at(0), None);
        assert_eq!(p.row_at(lh * 3 + 1), Some(0));
        assert_eq!(p.row_at(lh * 4 + 1), Some(1));
        assert_eq!(p.row_at(lh * 5 + 1), None);
        // The hovered row is highlighted.
        let px =
            |y: u32| &f.rgba[((y * WIDTH + 2) * 4) as usize..((y * WIDTH + 2) * 4 + 3) as usize];
        assert_eq!(px(lh * 4 + 1), HOVER);
        assert_ne!(px(lh * 3 + 1), HOVER);
    }
}
