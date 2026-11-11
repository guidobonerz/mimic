//! Screen overlays: a picture in front of the screen (the Vectrex's
//! coloured plastic sheets), laid over the machine's picture before the
//! CRT stage.
//!
//! Each overlay pixel (RGBA) is a filter: alpha is how much of the screen
//! it covers, the colour what it lets through. The screen's light passes
//! through the filter (`light × colour`), and the room's light lights the
//! printed sheet faintly (`AMBIENT × colour`), so an overlay's art shows
//! on a dark screen as on a real Vectrex. Alpha 0 leaves the picture as it
//! is. The overlay is scaled to the picture (nearest pixel). Overlays are
//! the user's own pictures (`media_backends::overlays`), never machine
//! state; the debugger and the raw preview never see them (golden rule 9).

use emu_core::frame::Rgb8;

/// The room's light on the sheet, as a fraction of full brightness.
pub const AMBIENT: f32 = 0.06;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row.
    pub rgba: Vec<u8>,
}

impl Overlay {
    /// A PNG picture (any colour type and bit depth).
    pub fn from_png(bytes: &[u8]) -> Result<Self, String> {
        let mut dec = png::Decoder::new(bytes);
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width, info.height);
        let px = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], c[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => px
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return Err("indexed PNG not expanded".into()),
        };
        if w == 0 || h == 0 || rgba.len() != (w * h * 4) as usize {
            return Err("empty or truncated overlay picture".into());
        }
        Ok(Self {
            width: w,
            height: h,
            rgba,
        })
    }

    /// The overlay's pixel over picture pixel (x, y) of a `w` × `h`
    /// picture.
    fn at(&self, x: u32, y: u32, w: u32, h: u32) -> [u8; 4] {
        let ox = (u64::from(x) * u64::from(self.width) / u64::from(w.max(1))) as u32;
        let oy = (u64::from(y) * u64::from(self.height) / u64::from(h.max(1))) as u32;
        let i = ((oy.min(self.height - 1) * self.width + ox.min(self.width - 1)) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    fn filter(c: [u8; 3], o: [u8; 4]) -> [u8; 3] {
        let a = f32::from(o[3]) / 255.0;
        if a == 0.0 {
            return c;
        }
        let mut out = [0; 3];
        for k in 0..3 {
            let f = f32::from(o[k]) / 255.0;
            let v = f32::from(c[k]);
            let seen = v * (1.0 - a) + (v * f + AMBIENT * 255.0 * f) * a;
            out[k] = seen.round().clamp(0.0, 255.0) as u8;
        }
        out
    }

    /// Over a `w` × `h` picture of RGBA pixels (the alpha stays).
    pub fn apply_rgba(&self, w: u32, h: u32, rgba: &mut [u8]) {
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let Some(p) = rgba.get_mut(i..i + 3) else {
                    return;
                };
                let c = Self::filter([p[0], p[1], p[2]], self.at(x, y, w, h));
                p.copy_from_slice(&c);
            }
        }
    }

    /// Over a `w` × `h` picture of [`Rgb8`] pixels.
    pub fn apply(&self, w: u32, h: u32, px: &mut [Rgb8]) {
        for y in 0..h {
            for x in 0..w {
                let Some(p) = px.get_mut((y * w + x) as usize) else {
                    return;
                };
                let [r, g, b] = Self::filter([p.r, p.g, p.b], self.at(x, y, w, h));
                *p = Rgb8 { r, g, b };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two pixels side by side: clear, and a half-covering red filter.
    fn two() -> Overlay {
        Overlay {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 0, 255, 0, 0, 255],
        }
    }

    #[test]
    fn a_filter_tints_and_shows_its_art_on_a_dark_screen() {
        let o = two();
        let mut px = vec![
            Rgb8 {
                r: 200,
                g: 200,
                b: 200
            };
            4
        ];
        o.apply(4, 1, &mut px);
        assert_eq!(
            px[0],
            Rgb8 {
                r: 200,
                g: 200,
                b: 200
            },
            "clear: unchanged"
        );
        assert_eq!(px[3].g, 0, "red lets no green through");
        assert!(px[3].r >= 200);
        let mut dark = vec![Rgb8::default(); 4];
        o.apply(4, 1, &mut dark);
        assert_eq!(dark[0], Rgb8::default());
        assert!(
            dark[3].r > 0 && dark[3].g == 0,
            "the sheet in the room's light"
        );
    }

    #[test]
    fn rgba_and_png_round_trip() {
        let mut png_bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png_bytes, 2, 1);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header()
                .unwrap()
                .write_image_data(&two().rgba)
                .unwrap();
        }
        let o = Overlay::from_png(&png_bytes).unwrap();
        assert_eq!(o, two());
        let mut rgba = vec![100, 100, 100, 255, 100, 100, 100, 255];
        o.apply_rgba(2, 1, &mut rgba);
        assert_eq!(&rgba[..4], &[100, 100, 100, 255]);
        assert_eq!(rgba[5], 0);
        assert!(Overlay::from_png(b"not a png").is_err());
    }
}
