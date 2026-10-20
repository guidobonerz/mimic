//! Native debug protocol, JSON-lines transport (version 2).
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
//!
//! | Event         | When                                                       |
//! | ------------- | ---------------------------------------------------------- |
//! | `hello`       | on connect: protocol name, version, machine                |
//! | `resumed`     | emulation starts running (`resume`, `step_over`, `step_out`, `run_to`) |
//! | `stopped`     | a run ends: breakpoint, watchpoint, finished step, jam, `pause` |
//! | `frame_ready` | after every emulated frame while running; only to clients that `subscribe` to it |
//!
//! Execution commands come in two flavours. `step_instr`, `step_cycle`,
//! `step_line`, `step_frame`, `pause` and `reset` are synchronous: the
//! response carries the stop state (`reason`, `pc`, `beam`). `resume`,
//! `step_over`, `step_out` and `run_to` are asynchronous: the response says
//! `{"running": true}` and a `stopped` event follows when the run ends.
//!
//! Version history: 1 (Phase 1) had execution control, registers, peek,
//! disassembly and exec breakpoints. 2 adds asynchronous steps, `run_to`,
//! `reset`, all breakpoint kinds with conditions and ignore counts, `poke`,
//! `chips`, savestates, the trace, `capabilities` and event subscriptions.
//!
//! Every change to this message set needs a contract test and a bump of
//! [`PROTOCOL_VERSION`].

use serde::Deserialize;
use serde_json::{json, Value};

use emu_core::{BeamPos, StopReason};

pub const PROTOCOL_NAME: &str = "polytrace-debug";
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

/// Device-addressed commands default to the main CPU (`devices()[0]`),
/// space-addressed ones to the first memory space (the main CPU's view).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Machine id, devices, memory spaces, run state.
    Info,
    /// Commands, breakpoint kinds and events this server supports.
    Capabilities,
    /// Which events this connection receives besides responses. `hello`,
    /// `resumed` and `stopped` always arrive; `frame_ready` only on request.
    Subscribe {
        events: Vec<String>,
    },
    Pause,
    Resume,
    /// Main CPU: `count` instructions (default 1), synchronous.
    StepInstr {
        count: Option<u32>,
    },
    /// Main CPU: like `step_instr`, but runs a subroutine call to its return.
    StepOver,
    /// Main CPU: run until the current subroutine returns.
    StepOut,
    StepCycle,
    /// Run to cycle 0 of the next raster line.
    StepLine,
    /// Run to the end of the current frame.
    StepFrame,
    /// Run until the main CPU fetches the instruction at `addr`.
    RunTo {
        space: Option<u16>,
        addr: u32,
    },
    /// `"soft"` (default, RESET line) or `"hard"` (power cycle).
    Reset {
        kind: Option<String>,
    },
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
    /// Debugger write (bypasses freezes and watchpoints); at most [`MAX_PEEK`] bytes.
    Poke {
        space: Option<u16>,
        addr: u32,
        bytes: Vec<u8>,
    },
    Disasm {
        device: Option<String>,
        /// Defaults to the device's PC.
        addr: Option<u32>,
        count: Option<u32>,
    },
    Beam,
    /// Register dumps of all chips.
    Chips,
    BreakAdd(BreakSpec),
    BreakRemove {
        bp: u32,
    },
    BreakEnable {
        bp: u32,
        enabled: bool,
    },
    /// Replace a breakpoint's condition (`None` removes it).
    BreakCondition {
        bp: u32,
        condition: Option<String>,
    },
    BreakList,
    /// Machine state as base64.
    SaveState,
    LoadState {
        data: String,
    },
    TraceStart {
        #[serde(default)]
        instructions: bool,
        #[serde(default)]
        reads: Vec<RangeSpec>,
        #[serde(default)]
        writes: Vec<RangeSpec>,
        capacity: Option<u32>,
    },
    TraceStop,
    /// Take the recorded entries (oldest first) and clear the buffer.
    TraceRead,
}

/// `break_add` arguments. `kind` is `exec` (default), `read`, `write`,
/// `raster`, `irq` or `nmi`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BreakSpec {
    pub kind: Option<String>,
    pub space: Option<u16>,
    pub addr: Option<u32>,
    /// Watchpoint length in bytes (default 1).
    pub len: Option<u32>,
    pub line: Option<u16>,
    /// Raster breakpoints: cycle within the line (default 0).
    pub cycle: Option<u16>,
    /// Expression (see [`crate::expr`]); the breakpoint only stops when it holds.
    pub condition: Option<String>,
    /// Hits (with the condition holding) to let pass before stopping.
    pub ignore: Option<u32>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct RangeSpec {
    pub space: Option<u16>,
    pub addr: u32,
    pub len: u32,
}

pub const MAX_PEEK: u32 = 0x10000;
pub const MAX_DISASM: u32 = 256;
pub const MAX_STEP_COUNT: u32 = 100_000;
pub const MAX_TRACE_CAPACITY: u32 = 1 << 20;

/// Commands listed by `capabilities`.
pub const COMMANDS: &[&str] = &[
    "info",
    "capabilities",
    "subscribe",
    "pause",
    "resume",
    "step_instr",
    "step_over",
    "step_out",
    "step_cycle",
    "step_line",
    "step_frame",
    "run_to",
    "reset",
    "regs",
    "set_reg",
    "peek",
    "poke",
    "disasm",
    "beam",
    "chips",
    "break_add",
    "break_remove",
    "break_enable",
    "break_condition",
    "break_list",
    "save_state",
    "load_state",
    "trace_start",
    "trace_stop",
    "trace_read",
];

pub const BREAK_KINDS: &[&str] = &["exec", "read", "write", "raster", "irq", "nmi"];
pub const EVENTS: &[&str] = &["hello", "resumed", "stopped", "frame_ready"];
/// Events a client must ask for with `subscribe`.
pub const OPTIONAL_EVENTS: &[&str] = &["frame_ready"];

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

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = B64
            .iter()
            .position(|&b| b == c)
            .ok_or_else(|| format!("invalid base64 character '{}'", c as char))?;
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 {
        return Err("truncated base64".into());
    }
    Ok(out)
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
        let r = parse_request(r#"{"id":10,"cmd":"step_instr"}"#).unwrap();
        assert_eq!(r.cmd, Command::StepInstr { count: None });
    }

    #[test]
    fn break_add_accepts_the_version_1_form() {
        let r = parse_request(r#"{"id":1,"cmd":"break_add","addr":4096}"#).unwrap();
        assert_eq!(
            r.cmd,
            Command::BreakAdd(BreakSpec {
                addr: Some(4096),
                ..BreakSpec::default()
            })
        );
        let r = parse_request(
            r#"{"id":2,"cmd":"break_add","kind":"write","addr":53280,"len":2,"condition":"A==1"}"#,
        )
        .unwrap();
        let Command::BreakAdd(spec) = r.cmd else {
            panic!("break_add")
        };
        assert_eq!(spec.kind.as_deref(), Some("write"));
        assert_eq!(spec.len, Some(2));
        assert_eq!(spec.condition.as_deref(), Some("A==1"));
    }

    #[test]
    fn bad_requests_keep_the_id_when_possible() {
        let e = parse_request(r#"{"id":3,"cmd":"fly"}"#).unwrap_err();
        assert_eq!(e["id"], 3);
        assert_eq!(e["ok"], false);
        let e = parse_request("not json").unwrap_err();
        assert!(e["id"].is_null());
    }

    #[test]
    fn base64_round_trip() {
        for (raw, enc) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"\xff\x00\x80", "/wCA"),
        ] {
            assert_eq!(base64_encode(raw), enc);
            assert_eq!(base64_decode(enc).unwrap(), raw);
        }
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        assert!(base64_decode("Zm9v!").is_err());
        assert!(base64_decode("Z").is_err());
    }
}
