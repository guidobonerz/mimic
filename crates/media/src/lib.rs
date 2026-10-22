//! Image formats: PRG, D64/D71/D81, T64/TAP, CRT, ADF(→MFM; extended ADF, DMS), ATR, XEX, CAS, DSK, CDT, DMK/JV1/JV3.
//!
//! Pure parsing and encoding: no files, no host access (golden rule 10).
//! Bytes come from a `MediaBackend`.

#![forbid(unsafe_code)]

pub mod adf;
pub mod adfx;
pub mod atr;
pub mod car;
pub mod cas;
pub mod cdt;
pub mod crt;
pub mod d64;
pub mod dmk;
pub mod dms;
pub mod dsk;
pub mod gcr;
pub mod mfm;
pub mod prg;
pub mod t64;
pub mod tap;
pub mod xex;

pub use d64::{Format, D64};
pub use prg::Prg;
