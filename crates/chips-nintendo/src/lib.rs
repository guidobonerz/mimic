//! Nintendo NES/Famicom chips: the [`ppu`] 2C02 (NTSC) / 2C07 (PAL)
//! picture processing unit with its [`palette`], and the [`apu`] of the
//! 2A03/2A07 (two pulse channels, triangle, noise, delta modulation, frame
//! counter). The CPU core of the 2A03 is `cpu-6502`'s `Rp2A03` variant.
//!
//! Sources: NESdev wiki, "PPU", "PPU registers", "PPU rendering", "PPU
//! scrolling", "PPU sprite evaluation", "PPU palettes", "NTSC video",
//! "APU", "APU Frame Counter", "APU Length Counter", "APU Envelope",
//! "APU Sweep", "APU Pulse", "APU Triangle", "APU Noise", "APU DMC",
//! "APU Mixer" (read 2026-10-23); Brad Taylor, "2A03 technical reference".
//! The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod apu;
pub mod palette;
pub mod ppu;

pub use apu::{Apu, ApuModel};
pub use ppu::{Ppu, PpuBus, PpuModel};
