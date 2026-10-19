//! Emulated input peripherals: joysticks, paddles, mice, light pens, keypads.

#![forbid(unsafe_code)]

pub mod joystick;

pub use joystick::Joystick;
