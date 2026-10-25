//! Media backends: image file, copy-on-write overlay, host directory, real
//! hardware.
//!
//! Real hardware sits behind cargo features (default off; golden rule 6
//! and "Drives and media" in CLAUDE.md: never needed to build):
//! - `flux`: flux decoding (PLL, Commodore GCR, Amiga and IBM MFM) and the
//!   disk images built from it — plain Rust, no device;
//! - `greaseweazle` (implies `flux`): the Greaseweazle's USB serial
//!   protocol, reading whole disks into D64/G64/D71/D81/ADF/IMG;
//! - `opencbm`: real 1541/1571/1581 drives through the OpenCBM library
//!   (ZoomFloppy/XUM1541), loaded at run time — the only module with
//!   `unsafe` code is its FFI layer.

#![cfg_attr(not(feature = "opencbm"), forbid(unsafe_code))]
#![cfg_attr(feature = "opencbm", deny(unsafe_code))]

#[cfg(feature = "flux")]
pub mod flux;
#[cfg(feature = "greaseweazle")]
pub mod greaseweazle;
pub mod host_dir;
#[cfg(not(target_arch = "wasm32"))]
pub mod host_fs;
pub mod image;
#[cfg(not(target_arch = "wasm32"))]
pub mod net;
#[cfg(feature = "opencbm")]
pub mod opencbm;
pub mod roms;

/// The host services of a native frontend, in a live journal: the real
/// network (`net` false: every network call reports "no network", the
/// clock still runs) and `folder` as the sandboxed file area.
#[cfg(not(target_arch = "wasm32"))]
pub fn host_journal(net: bool, folder: Option<std::path::PathBuf>) -> emu_core::host::Journal {
    emu_core::host::Journal::new(
        Some(Box::new(net::StdNet::new(net))),
        folder.map(|f| Box::new(host_fs::FsFolder::new(f)) as Box<dyn emu_core::host::FileHost>),
    )
}
