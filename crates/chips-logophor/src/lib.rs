//! The custom chips of the Logophor 16, a fictional home computer of this
//! project: Babbage (chip-bus arbiter, Copper, Blitter, the 3D unit
//! Stereoscope), Lumière (video, sprites), Calliope (sound) and Telegraph
//! (network, timers, interrupts, input, SD card).
//!
//! Specification: `docs/logophor16/SPEC.md` sections 5–10. Built so far:
//! Lumière (modes 0, 1 and 4, sprites), Babbage (DMA control, Copper,
//! Blitter, Stereoscope), Calliope and Telegraph (interrupts, timers,
//! clock, input, SD card, network).
#![forbid(unsafe_code)]

pub mod babbage;
pub mod blitter;
pub mod calliope;
pub mod lumiere;
pub mod stereoscope;
pub mod telegraph;
pub mod telegraph_io;
pub mod telegraph_net;
