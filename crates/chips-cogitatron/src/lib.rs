//! The custom chips of the Cogitatron 16, a fictional home computer of this
//! project: Babbage (chip-bus arbiter, Copper, Blitter, the 3D unit
//! Stereoscope), Lumière (video, sprites), Calliope (sound) and Telegraph
//! (network, timers, interrupts, input, SD card).
//!
//! Specification: `docs/cogitatron16/SPEC.md` sections 5–10. Built so far: Lumière's modes 0 and
//! 4, Babbage's DMA control, Copper and Blitter, Telegraph's interrupt controller;
//! the rest follows in Phase 31's later steps.
#![forbid(unsafe_code)]

pub mod babbage;
pub mod blitter;
pub mod lumiere;
pub mod telegraph;
