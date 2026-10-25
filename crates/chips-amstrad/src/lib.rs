//! Amstrad CPC chips: CRTC 6845, Gate Array, AY-3-8912, 8255 PPI, FDC 765,
//! and the graphics data views of the CRTC and Gate Array ([`gfx`]).

#![forbid(unsafe_code)]

pub mod ay;
pub mod crtc;
pub mod fdc;
pub mod gate_array;
pub mod gfx;
pub mod palette;
pub mod ppi;
