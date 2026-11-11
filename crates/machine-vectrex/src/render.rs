//! The rasterised picture of a vector frame, drawn with the shared
//! rasteriser ([`emu_core::vector::Raster`]): every segment at full
//! weight, no persistence (decay across frames is `video-crt`'s job).
//!
//! Two looks ([`Style`], machine option `lines`): **fine** (default) and
//! **soft** (the first look that worked; the user's "option 1",
//! 2026-10-24); see [`emu_core::vector`].

use emu_core::frame::Rgb8;
pub use emu_core::vector::Style;
use emu_core::vector::{Raster, Segment};

/// Picture size in pixels (the tube is 4:5, portrait).
pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 1000;

/// The picture's raster at `unit` coordinate units a pixel.
pub fn raster(unit: i32, style: Style) -> Raster {
    Raster {
        width: WIDTH,
        height: HEIGHT,
        unit,
        style,
    }
}

/// Draw `segments` (coordinates in units of `unit` per pixel) into
/// `pixels`. `energy` is scratch space of the same size.
pub fn draw(
    segments: &[Segment],
    unit: i32,
    style: Style,
    energy: &mut [[f32; 3]],
    pixels: &mut [Rgb8],
) {
    raster(unit, style).draw(segments, energy, pixels);
}
