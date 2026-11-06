//! The custom chips of the Logophor 16, a fictional home computer of this
//! project: Babbage (chip-bus arbiter, Jacquard, Lithograph, the 3D unit
//! Stereoscope), Lumière (video, sprites), Calliope (sound), the
//! Phonograph (MP3/AAC decoder) and Telegraph (network, timers,
//! interrupts, input, SD card).
//!
//! Specification: `docs/logophor16/SPEC.md` sections 5–10. Built so far:
//! Lumière (modes 0, 1 and 4, sprites), Babbage (DMA control, Jacquard,
//! Lithograph, Stereoscope), Calliope, the Phonograph and Telegraph (interrupts, timers,
//! clock, input, SD card, network).
#![forbid(unsafe_code)]

/// The clock (SPEC.md 3.12, 5): the CPU and the chip bus run at CLOCK x
/// 12 MHz (CPU cycles and bus slots a line grow with it; Calliope's
/// 48 kHz, the timers' 1 MHz and the picture keep their rates).
pub const CLOCK: u32 = 3;

pub mod babbage;
pub mod calliope;
pub mod lithograph;
pub mod lumiere;
pub mod phonograph;
pub mod stereoscope;
pub mod telegraph;
pub mod telegraph_io;
pub mod telegraph_net;
