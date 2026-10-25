//! Graphics data views: the graphics a video chip works from (sprites,
//! character sets, bitmaps, screen memory, tiles, palettes), decoded into a
//! picture and as raw bytes, so a debugger can show them without knowing
//! the chip (golden rule 1). Machines offer them through
//! [`crate::Machine::gfx_views`] and [`crate::Machine::gfx_view`]; the
//! decoding helpers here are shared by the machines.
//!
//! Views are built on demand from side-effect-free reads (golden rule 2):
//! asking for one never changes the machine.

use std::collections::BTreeMap;

use crate::frame::{FieldParity, FrameRef, OutputType, PixelData, Rgb8, SignalInfo, VideoStandard};
use crate::machine::MemSpaceId;

/// What a view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GfxKind {
    /// Movable objects: sprites, players/missiles, MOBs, OAM.
    Sprites,
    /// Character generator patterns.
    Charset,
    /// Bitmap / bitplane memory.
    Bitmap,
    /// Screen (name table / text) memory rendered with its character set.
    Screen,
    /// Tile patterns (NES/SNES pattern data, TMS9918 patterns).
    Tiles,
    /// The colour registers / palette memory.
    Palette,
}

impl GfxKind {
    pub fn name(self) -> &'static str {
        match self {
            GfxKind::Sprites => "sprites",
            GfxKind::Charset => "charset",
            GfxKind::Bitmap => "bitmap",
            GfxKind::Screen => "screen",
            GfxKind::Tiles => "tiles",
            GfxKind::Palette => "palette",
        }
    }
}

/// One option a view understands, for help texts and clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GfxOptionInfo {
    pub name: &'static str,
    pub help: &'static str,
}

/// Options most views understand. Machines may add their own.
pub const OPT_ADDR: GfxOptionInfo = GfxOptionInfo {
    name: "addr",
    help: "start address in the view's memory space (default: where the chip points now)",
};
pub const OPT_INDEX: GfxOptionInfo = GfxOptionInfo {
    name: "index",
    help: "show only this one object (sprite number, plane, table)",
};
pub const OPT_MULTICOLOR: GfxOptionInfo = GfxOptionInfo {
    name: "multicolor",
    help: "0 = hires, 1 = multicolour (default: as the chip is set)",
};
pub const OPT_COUNT: GfxOptionInfo = GfxOptionInfo {
    name: "count",
    help: "number of characters / tiles / objects to show",
};
pub const OPT_BPP: GfxOptionInfo = GfxOptionInfo {
    name: "bpp",
    help: "bits per pixel of the pattern data",
};
pub const OPT_PALETTE: GfxOptionInfo = GfxOptionInfo {
    name: "palette",
    help: "palette / colour set to draw with",
};

/// Which view to fetch and how: numeric options by name (see
/// [`GfxViewInfo::options`]). Unknown options are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GfxOptions {
    values: BTreeMap<String, u32>,
}

impl GfxOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, value: u32) -> Self {
        self.set(name, value);
        self
    }

    pub fn set(&mut self, name: &str, value: u32) {
        self.values.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        self.values.get(name).copied()
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        self.get(name).map(|v| v != 0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, u32)> {
        self.values.iter().map(|(k, &v)| (k.as_str(), v))
    }
}

/// A view a machine offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GfxViewInfo {
    /// Short id for protocols and URLs ("sprites", "vdc-charset").
    pub id: &'static str,
    pub kind: GfxKind,
    /// Human-readable, naming the chip ("VIC-II sprites").
    pub name: String,
    pub options: Vec<GfxOptionInfo>,
}

/// The picture of a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GfxPixels {
    Indexed { pixels: Vec<u8>, palette: Vec<Rgb8> },
    Rgb(Vec<Rgb8>),
}

/// A decoded view: picture, the parameters it was decoded with and the raw
/// bytes it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GfxView {
    pub id: &'static str,
    pub kind: GfxKind,
    pub name: String,
    /// Source of `raw`: memory space and first address.
    pub space: MemSpaceId,
    pub addr: u32,
    /// Resolved parameters (size, colours, mode, bank, pointers…), in the
    /// order the machine lists them.
    pub params: Vec<(&'static str, u32)>,
    pub width: u32,
    pub height: u32,
    pub pixels: GfxPixels,
    /// The bytes the picture was decoded from (as peeked).
    pub raw: Vec<u8>,
}

/// Signal metadata for a view's picture: not a video signal, square pixels.
const VIEW_SIGNAL: SignalInfo = SignalInfo {
    pixel_clock_hz: 0,
    active_pixels: 0,
    total_pixels: 0,
    lines_per_field: 0,
    standard: VideoStandard::Pal,
    output: OutputType::Rgb,
    field: FieldParity::Progressive,
};

impl GfxView {
    /// The picture as a frame, so encoders and viewers for frames take it.
    pub fn frame(&self) -> FrameRef<'_> {
        FrameRef {
            width: self.width,
            height: self.height,
            pixels: match &self.pixels {
                GfxPixels::Indexed { pixels, palette } => PixelData::Indexed { pixels, palette },
                GfxPixels::Rgb(p) => PixelData::Rgb(p),
            },
            luma_chroma: None,
            signal: SignalInfo {
                active_pixels: self.width,
                total_pixels: self.width,
                lines_per_field: self.height,
                ..VIEW_SIGNAL
            },
        }
    }

    pub fn param(&self, name: &str) -> Option<u32> {
        self.params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, v)| v)
    }
}

/// An indexed picture to draw a view into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    /// A `width` × `height` canvas filled with colour `bg`.
    pub fn new(width: u32, height: u32, bg: u8) -> Self {
        Canvas {
            width,
            height,
            pixels: vec![bg; width as usize * height as usize],
        }
    }

    /// A canvas for `count` cells of `cell_w` × `cell_h` in rows of `cols`,
    /// with `gap` pixels of colour `bg` between cells.
    pub fn grid(count: u32, cols: u32, cell_w: u32, cell_h: u32, gap: u32, bg: u8) -> Self {
        let cols = cols.clamp(1, count.max(1));
        let rows = count.max(1).div_ceil(cols);
        Canvas::new(cols * (cell_w + gap) - gap, rows * (cell_h + gap) - gap, bg)
    }

    /// Top-left corner of cell `i` in a [`Canvas::grid`] layout.
    pub fn cell_origin(i: u32, cols: u32, cell_w: u32, cell_h: u32, gap: u32) -> (u32, u32) {
        let cols = cols.max(1);
        ((i % cols) * (cell_w + gap), (i / cols) * (cell_h + gap))
    }

    pub fn set(&mut self, x: u32, y: u32, c: u8) {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) as usize] = c;
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Option<u8> {
        (x < self.width && y < self.height).then(|| self.pixels[(y * self.width + x) as usize])
    }

    /// Fill a rectangle (clipped).
    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, c: u8) {
        for yy in y..y.saturating_add(h).min(self.height) {
            for xx in x..x.saturating_add(w).min(self.width) {
                self.pixels[(yy * self.width + xx) as usize] = c;
            }
        }
    }

    /// Draw one 1-bit-per-pixel row of 8 pixels (MSB left) at `(x, y)`,
    /// each pixel `xscale` wide: set bits in `fg`, clear in `bg` (`None`
    /// leaves them).
    pub fn row_1bpp(&mut self, x: u32, y: u32, byte: u8, fg: u8, bg: Option<u8>, xscale: u32) {
        for b in 0..8 {
            let on = byte & (0x80 >> b) != 0;
            let c = if on { Some(fg) } else { bg };
            if let Some(c) = c {
                for s in 0..xscale {
                    self.set(x + b * xscale + s, y, c);
                }
            }
        }
    }

    /// Draw one row of 4 double-wide pixels from bit pairs (MSB pair left);
    /// `colors[pair]` (`None` leaves the pixel).
    pub fn row_2bpp(&mut self, x: u32, y: u32, byte: u8, colors: [Option<u8>; 4], xscale: u32) {
        for p in 0..4 {
            let pair = (byte >> (6 - 2 * p)) & 3;
            if let Some(c) = colors[pair as usize] {
                for s in 0..2 * xscale {
                    self.set(x + p * 2 * xscale + s, y, c);
                }
            }
        }
    }

    /// Finish into an indexed view picture.
    pub fn into_pixels(self, palette: Vec<Rgb8>) -> GfxPixels {
        GfxPixels::Indexed {
            pixels: self.pixels,
            palette,
        }
    }
}

/// A palette of `n` shades of grey from black to white, for data without
/// colours of its own (pattern tables).
pub fn grey_palette(n: usize) -> Vec<Rgb8> {
    let n = n.max(2);
    (0..n)
        .map(|i| {
            let v = (i * 255 / (n - 1)) as u8;
            Rgb8 { r: v, g: v, b: v }
        })
        .collect()
}

/// Colour index of pixel `x` (0–7, left to right) of a planar tile row:
/// bit `7 - x` of each plane byte, plane 0 the least significant.
pub fn planar_pixel(planes: &[u8], x: u32) -> u8 {
    planes
        .iter()
        .enumerate()
        .fold(0, |acc, (i, &p)| acc | (((p >> (7 - x)) & 1) << i))
}

/// A palette view: one square swatch of `size` pixels per colour, `cols`
/// to a row. Palettes of up to 256 colours stay indexed.
pub fn palette_canvas(colors: &[Rgb8], cols: u32, size: u32) -> (u32, u32, GfxPixels) {
    let n = colors.len() as u32;
    let mut c = Canvas::grid(n, cols, size, size, 1, 0);
    let indexed = colors.len() <= 255;
    let mut rgb = Vec::new();
    for i in 0..n {
        let (x, y) = Canvas::cell_origin(i, cols, size, size, 1);
        c.fill(x, y, size, size, if indexed { i as u8 + 1 } else { 0 });
    }
    if indexed {
        let mut palette = vec![Rgb8::default()];
        palette.extend_from_slice(colors);
        let (w, h) = (c.width, c.height);
        return (w, h, c.into_pixels(palette));
    }
    // More than 255 colours: draw directly.
    rgb.resize(c.pixels.len(), Rgb8::default());
    for i in 0..n {
        let (x, y) = Canvas::cell_origin(i, cols, size, size, 1);
        for yy in y..y + size {
            for xx in x..x + size {
                rgb[(yy * c.width + xx) as usize] = colors[i as usize];
            }
        }
    }
    (c.width, c.height, GfxPixels::Rgb(rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_layout_and_rows() {
        let mut c = Canvas::grid(3, 2, 8, 8, 1, 0);
        assert_eq!((c.width, c.height), (17, 17));
        assert_eq!(Canvas::cell_origin(2, 2, 8, 8, 1), (0, 9));
        c.row_1bpp(9, 0, 0b1000_0001, 5, Some(1), 1);
        assert_eq!(c.get(9, 0), Some(5));
        assert_eq!(c.get(10, 0), Some(1));
        assert_eq!(c.get(16, 0), Some(5));
        c.row_2bpp(0, 1, 0b00_01_10_11, [None, Some(1), Some(2), Some(3)], 1);
        assert_eq!(c.get(0, 1), Some(0), "pair 00 left transparent");
        assert_eq!(c.get(2, 1), Some(1));
        assert_eq!(c.get(5, 1), Some(2));
        assert_eq!(c.get(7, 1), Some(3));
    }

    #[test]
    fn planar_pixels_combine_planes() {
        assert_eq!(planar_pixel(&[0x80, 0x80], 0), 3);
        assert_eq!(planar_pixel(&[0x00, 0x40], 1), 2);
        assert_eq!(planar_pixel(&[0x01], 7), 1);
    }

    #[test]
    fn palette_views_and_frames() {
        let colors = [Rgb8 { r: 9, g: 0, b: 0 }, Rgb8 { r: 0, g: 9, b: 0 }];
        let (w, h, pixels) = palette_canvas(&colors, 16, 4);
        assert_eq!((w, h), (9, 4));
        let view = GfxView {
            id: "palette",
            kind: GfxKind::Palette,
            name: "test".into(),
            space: MemSpaceId(0),
            addr: 0,
            params: vec![("count", 2)],
            width: w,
            height: h,
            pixels,
            raw: vec![],
        };
        let f = view.frame();
        assert_eq!(f.rgb_at(0, 0), Some(colors[0]));
        assert_eq!(f.rgb_at(5, 3), Some(colors[1]));
        assert_eq!(f.rgb_at(4, 0), Some(Rgb8::default()), "gap");
        assert_eq!(view.param("count"), Some(2));
        let big: Vec<Rgb8> = (0..300)
            .map(|i| Rgb8 {
                r: i as u8,
                g: 0,
                b: 0,
            })
            .collect();
        assert!(matches!(palette_canvas(&big, 16, 2).2, GfxPixels::Rgb(_)));
        let o = GfxOptions::new().with("addr", 0x2000).with("multicolor", 1);
        assert_eq!(o.get("addr"), Some(0x2000));
        assert_eq!(o.flag("multicolor"), Some(true));
        assert_eq!(o.get("x"), None);
    }
}
