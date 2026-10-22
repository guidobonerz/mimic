//! Debug server: native protocol + adapters (VICE binary monitor, GDB RSP, DAP).
//!
//! The native protocol (JSON lines over TCP, see [`protocol`]) covers
//! execution control including asynchronous steps, all breakpoint kinds
//! with conditions ([`expr`]), memory, registers, chips, savestates and the
//! raster trace. It talks to machines only through `emu-core` traits
//! (golden rule 1). Adapters: the VICE binary monitor ([`vice`]) and the
//! GDB remote protocol with a live-picture extension ([`gdb`], whose
//! pictures come from the `emu-preview` encoders).

#![forbid(unsafe_code)]

pub mod expr;
pub mod gdb;
pub mod host;
pub mod protocol;
pub mod server;
pub mod session;
pub mod symbols;
pub mod vice;

pub use server::{Control, DebugServer, Endpoint, ServerConfig, Service};
pub use session::{FrameHook, Session};
