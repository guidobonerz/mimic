//! Texas Instruments chips: the [`vdp`] TMS9918A (NTSC) / TMS9929A (PAL)
//! video display processor and the [`psg`] SN76489AN sound generator, as
//! the ColecoVision (and later the MSX, the TI-99/4A, the SG-1000) use them.
//!
//! Sources: Texas Instruments, *TMS9918A/TMS9928A/TMS9929A Video Display
//! Processors Data Manual* (1982); Texas Instruments, *SN76489AN Digital
//! Complex Sound Generator* datasheet; SMS Power, "SN76489" notes (noise
//! shift register, period 0). The implementation is this project's own.

#![forbid(unsafe_code)]

pub mod psg;
pub mod vdp;

pub use psg::Sn76489;
pub use vdp::{Tms9918, VdpModel};
