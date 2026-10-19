//! Native debug protocol, JSON-lines transport (version 1).
//!
//! One JSON object per line in both directions. Clients send requests:
//!
//! ```json
//! {"id": 1, "cmd": "regs", "device": "cpu"}
//! ```
//!
//! and get exactly one response per request, in order:
//!
//! ```json
//! {"id": 1, "ok": true, "result": {"PC": 2048, "A": 0, ...}}
//! {"id": 2, "ok": false, "error": "unknown device 'z80'"}
//! ```
//!
//! The server also pushes events, which carry `"event"` instead of `"id"`:
//! `hello` on connect (protocol name and version), and `stopped` when a
//! `resume` ends at a breakpoint, watchpoint or CPU jam.
//!
//! Every change to this message set needs a contract test and a bump of
//! [`PROTOCOL_VERSION`].

use serde::Deserialize;
use serde_json::{json, Value};

use emu_core::{BeamPos, StopReason};

pub const PROTOCOL_NAME: &str = "polytrace-debug";
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

/// Device-addressed commands default to the main CPU (`devices()[0]`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Machine id, devices, memory spaces, run state.
    Info,
    Pause,
    Resume,
    StepInstr,
    StepCycle,
    /// Run to cycle 0 of the next raster line.
    StepLine,
    /// Run to the end of the current frame.
    StepFrame,
    Regs {
        device: Option<String>,
    },
    SetReg {
        device: Option<String>,
        name: String,
        value: u32,
    },
    /// Side-effect-free memory read; `len` bytes, at most [`MAX_PEEK`].
    Peek {
        space: Option<u16>,
        addr: u32,
        len: u32,
    },
    Disasm {
        device: Option<String>,
        /// Defaults to the device's PC.
        addr: Option<u32>,
        count: Option<u32>,
    },
    Beam,
    BreakAdd {
        space: Option<u16>,
        addr: u32,
    },
    BreakRemove {
        bp: u32,
    },
    BreakList,
}

pub const MAX_PEEK: u32 = 0x10000;
pub const MAX_DISASM: u32 = 256;

pub fn ok(id: u64, result: Value) -> Value {
    json!({"id": id, "ok": true, "result": result})
}

pub fn err(id: Option<u64>, msg: impl Into<String>) -> Value {
    json!({"id": id, "ok": false, "error": msg.into()})
}

pub fn hello(machine: &str) -> Value {
    json!({
        "event": "hello",
        "protocol": PROTOCOL_NAME,
        "version": PROTOCOL_VERSION,
        "machine": machine,
    })
}

pub fn beam_json(beam: Option<BeamPos>) -> Value {
    match beam {
        Some(b) => json!({"frame": b.frame, "line": b.line, "cycle": b.cycle, "hpos": b.hpos}),
        None => Value::Null,
    }
}

pub fn stop_json(reason: &StopReason) -> Value {
    match *reason {
        StopReason::FrameDone => json!({"kind": "frame_done"}),
        StopReason::StepDone => json!({"kind": "step_done"}),
        StopReason::Breakpoint(id) => json!({"kind": "breakpoint", "bp": id.0}),
        StopReason::Watchpoint {
            id,
            space,
            addr,
            write,
        } => json!({
            "kind": "watchpoint", "bp": id.0, "space": space.0, "addr": addr, "write": write,
        }),
        StopReason::CpuJam { device, pc } => {
            json!({"kind": "cpu_jam", "device": device.0, "pc": pc})
        }
        StopReason::Fault { device, reason } => {
            json!({"kind": "fault", "device": device.0, "reason": reason})
        }
    }
}

/// Parse one request line. On failure returns the error response to send,
/// with the request id if one could be recovered.
pub fn parse_request(line: &str) -> Result<Request, Value> {
    serde_json::from_str(line).map_err(|e| {
        let id = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("id").and_then(Value::as_u64));
        err(id, format!("bad request: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands_with_defaults() {
        let r = parse_request(r#"{"id":7,"cmd":"regs"}"#).unwrap();
        assert_eq!(
            r,
            Request {
                id: 7,
                cmd: Command::Regs { device: None }
            }
        );
        let r = parse_request(r#"{"id":8,"cmd":"peek","addr":53248,"len":2}"#).unwrap();
        assert_eq!(
            r.cmd,
            Command::Peek {
                space: None,
                addr: 0xd000,
                len: 2
            }
        );
        let r = parse_request(r#"{"id":9,"cmd":"step_line"}"#).unwrap();
        assert_eq!(r.cmd, Command::StepLine);
    }

    #[test]
    fn bad_requests_keep_the_id_when_possible() {
        let e = parse_request(r#"{"id":3,"cmd":"fly"}"#).unwrap_err();
        assert_eq!(e["id"], 3);
        assert_eq!(e["ok"], false);
        let e = parse_request("not json").unwrap_err();
        assert!(e["id"].is_null());
    }
}
