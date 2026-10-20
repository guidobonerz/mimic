//! Debug server: native protocol + adapters (VICE binary monitor, GDB RSP, DAP).
//!
//! The native protocol (JSON lines over TCP, see [`protocol`]) covers
//! execution control including asynchronous steps, all breakpoint kinds
//! with conditions ([`expr`]), memory, registers, chips, savestates and the
//! raster trace. It talks to machines only through `emu-core` traits
//! (golden rule 1).

#![forbid(unsafe_code)]

pub mod expr;
pub mod protocol;
pub mod server;
pub mod session;
pub mod symbols;
pub mod vice;

pub use server::{DebugServer, Endpoint, ServerConfig, Service};
pub use session::{FrameHook, Session};
