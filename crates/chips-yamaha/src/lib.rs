//! Yamaha chips: the [`v9938`] video display processor of the MSX2.
//!
//! Sources: Yamaha/ASCII/Microsoft, *V9938 MSX-VIDEO Technical Data Book*
//! (1985); ASCII, *MSX2 Technical Handbook* (1987), chapter 4 "VRAM and
//! the display screen". The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod v9938;

pub use v9938::V9938;
