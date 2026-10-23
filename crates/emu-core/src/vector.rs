//! Vector-display output (Vectrex): the path the beam drew in one frame,
//! as straight segments with intensity. Machines with a vector display
//! deliver this through [`crate::Machine::vectors`] next to a rasterised
//! [`crate::FrameRef`]; phosphor decay and glow are post-processing in
//! `video-crt` (golden rule 9), the debugger and the preview carry the
//! segments as they are.

use crate::frame::Rgb8;

/// One stretch of the beam's path at a constant speed and intensity.
/// Coordinates are in the frame's units (square: one unit is the same
/// distance on both axes), origin top left, y downwards. A dot the beam
/// rests on has `x0 == x1` and `y0 == y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Segment {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// Beam intensity 0–255 (0 = off; such segments are not delivered).
    pub intensity: u8,
    /// The colour the viewer sees at full intensity: the phosphor's, or
    /// as seen through a filter in front of the screen (the Vectrex 3D
    /// Imager's colour wheel).
    pub color: Rgb8,
    /// How long the beam took, in machine cycles: with the length this
    /// gives the energy per distance, i.e. how bright the trace glows.
    pub cycles: u32,
}

/// The segments of one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VectorFrame<'a> {
    /// Size of the visible area in coordinate units.
    pub width: u32,
    pub height: u32,
    pub segments: &'a [Segment],
    /// Colour of the phosphor at full intensity (segments carry the colour
    /// actually seen in [`Segment::color`]).
    pub phosphor: Rgb8,
    /// Machine cycles per second, to turn `Segment::cycles` into time.
    pub cycle_hz: u32,
    /// Frames since power-on.
    pub frame: u64,
}
