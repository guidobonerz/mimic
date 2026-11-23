//! The Ultimate 64 Elite II board (Gideon's Logic; also the board of the
//! Commodore 64 Ultimate): the blocks inside its FPGA that the board's
//! firmware talks to.
//!
//! The firmware is a program for a RISC-V soft processor in the FPGA
//! (`cpu-rv32`). It reaches everything else through an 8-bit I/O bus at
//! `$10000000`–`$10FFFFFF`; this crate holds the blocks on that bus. The
//! FPGA's configuration itself cannot be run: every block here is written
//! anew from descriptions of its registers and behaviour.
//!
//! Conventions shared by all blocks ([`io`]):
//! - a block sees the 24-bit I/O address (`addr & $FFFFFF`), not an offset
//!   from its own base, so the addresses in the code are those of the
//!   documentation;
//! - accesses are bytes; the bus bridge splits a 16- or 32-bit access into
//!   bytes at ascending addresses, least significant first, each with its
//!   own side effects;
//! - nothing faults: what is not modelled reads 0 and ignores writes;
//! - time is the board's 100 MHz clock ([`io::IoCtx::now`], clocks since
//!   power-on);
//! - a block that wants the processor's interrupt line says so through
//!   [`irq::Irq`];
//! - blocks do not reach the host: what must persist (the flash) is kept
//!   as bytes that the machine loads and stores.
//!
//! Sources: the hardware analysis of the UE2-C64U-Emulator project
//! (`docs/hw/00-memory-map.md` … `12-gaps.md`, read for facts; see
//! `docs/sources.md`), which cites the 1541 Ultimate firmware and VHDL.
#![forbid(unsafe_code)]

pub mod board;
pub mod c64port;
pub mod flash;
pub mod io;
pub mod irq;
pub mod itu;
pub mod overlay;
pub mod rmii;
pub mod sdcard;
pub mod u64io;
pub mod usb;
pub mod wifi;

pub use io::{
    unit, C64Rom, C64Side, EtherLink, IoCtx, IoDevice, MemLink, MemStorage, NullC64, Storage,
    CLOCK_HZ, SECTOR,
};
pub use irq::Irq;
