//! Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM), ATR, XEX, CAS, DSK, CDT.
//!
//! Pure parsing and encoding: no files, no host access (golden rule 10).
//! Bytes come from a `MediaBackend`.

#![forbid(unsafe_code)]

pub mod crt;
pub mod d64;
pub mod gcr;
pub mod mfm;
pub mod prg;
pub mod t64;
pub mod tap;

pub use d64::{Format, D64};
pub use prg::Prg;
