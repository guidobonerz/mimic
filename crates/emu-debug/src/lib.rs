//! Debug server: native protocol + adapters (VICE binary monitor, GDB RSP, DAP).
//!
//! Phase 1 provides the native protocol over JSON lines on TCP with
//! execution control (pause/resume/step), registers, memory, disassembly,
//! exec breakpoints and the beam. It talks to machines only through
//! `emu-core` traits (golden rule 1).

#![forbid(unsafe_code)]

pub mod protocol;
pub mod server;
pub mod session;

pub use server::{DebugServer, ServerConfig};
pub use session::Session;
