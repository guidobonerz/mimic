//! Sinclair chips: the ZX81's [`ula`] (Ferranti 2C184E/2C210E), which makes
//! the video signal together with the Z80, generates the NMI, decodes port
//! `$FE` (keyboard, VSYNC, tape) and the display-file opcode trick; and the
//! ZX Spectrum's ULA ([`spectrum`]: display from video RAM, contended
//! memory, port `$FE`, the frame interrupt).
//!
//! Sources: Sinclair ZX81 Service Manual; Wilf Rigter, "The ZX81 Video
//! Display System" / "ZX81 ULA" notes (the shift register, /NMI, /INT from
//! the refresh address, /WAIT during HALT); Ian Logan and Frank O'Hara,
//! *The Complete ZX81 ROM Disassembly* (how the ROM drives the ULA). The
//! implementation is this project's own.

#![forbid(unsafe_code)]

pub mod spectrum;
pub mod ula;

pub use spectrum::SpectrumUla;
pub use ula::Ula;
