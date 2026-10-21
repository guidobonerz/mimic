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
//! | `media_changed` | media inserted or ejected through the protocol      |
//! | `device_connected` | a device plugged into or pulled from a port (`device: null`) |
//! | `freeze_blocked_write` | a guest write to a frozen address was suppressed (at most one per frame; to subscribers). A write watchpoint on the address stops at the code |
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
//! 3 adds `break_condition` and symbols (`symbols_load`, `symbols_list`,
//! `symbols_clear`, labels in `disasm`, `symbol` in `break_add`/`run_to`)
//! and the PC in `resumed`. 4 adds remote control of devices: ports
//! (`list_ports`, `connect_device`, `disconnect_device`), media
//! (`list_media_slots`, `insert_media`, `eject_media`, `flush_media`,
//! `set_drive_mode`), `input`, `record_start`/`record_stop`/`replay_input`
//! and the `media_changed` and `device_connected` events. 5 adds the
//! freezer (`freeze_add`/`remove`/`toggle`/`list`, frozen offsets in
//! `peek`, freezes in `save_state`/`load_state` and recordings), the cheat
//! search (`search_start`/`next`/`results`), cheat files
//! (`cheats_load`/`save`) and the optional `freeze_blocked_write` event.
//! 6 adds `media_control` (a slot's own keys, e.g. a datasette's "play",
//! "record", "stop", "rewind", "forward"). 7 renames the protocol from
//! `polytrace-debug` to `mimic-debug` (and the replay format to `mimic-replay`).
//! 8 adds the console panel: `controls` (the machine's buttons and
//! switches with their state) and `set_control`, and the `"panel"` input
//! target with `{"type": "control", "id": n, "value": v}` events.
//!
//! Every change to this message set needs a contract test and a bump of
//! [`PROTOCOL_VERSION`].

use serde::Deserialize;
use serde_json::{json, Value};

use emu_core::{BeamPos, StopReason};

pub const PROTOCOL_NAME: &str = "mimic-debug";
pub const PROTOCOL_VERSION: u32 = 8;

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
    /// Run until the main CPU fetches the instruction at `addr` (or at
    /// the address of `symbol`).
    RunTo {
        space: Option<u16>,
        addr: Option<u32>,
        symbol: Option<String>,
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
    /// Machine state (`data`) and the freezes (`freezes`) as base64.
    SaveState,
    /// Restore `data`; `freezes`, if given, replaces the freeze list.
    LoadState {
        data: String,
        freezes: Option<String>,
    },
    /// Freeze `size` bytes (1, 2, 4) at `addr` or `symbol` to `value`
    /// (default: what is there now). `mode`: `block_writes` (default),
    /// `reapply_per_frame` or `reapply_on_read`.
    FreezeAdd {
        space: Option<u16>,
        addr: Option<u32>,
        symbol: Option<String>,
        size: Option<u8>,
        value: Option<u32>,
        mode: Option<String>,
        label: Option<String>,
        enabled: Option<bool>,
    },
    FreezeRemove {
        freeze: u32,
    },
    /// Switch on or off; toggles without `enabled`.
    FreezeToggle {
        freeze: u32,
        enabled: Option<bool>,
    },
    FreezeList,
    /// Snapshot candidate addresses: `len` bytes from `addr` (default the
    /// whole space, at most 16 MB), values of `size` bytes.
    SearchStart {
        space: Option<u16>,
        size: Option<u8>,
        addr: Option<u32>,
        len: Option<u32>,
    },
    /// Keep the candidates for which `op` holds: `eq`, `ne`, `lt`, `gt`
    /// (against `value`), `increased`, `decreased`, `unchanged`, `changed`
    /// (against the last snapshot), `increased_by`, `decreased_by` (`value`).
    SearchNext {
        op: String,
        value: Option<u32>,
    },
    /// The first `max` candidates (default 100) with current and last value.
    SearchResults {
        max: Option<u32>,
    },
    /// Add the freezes of a TOML cheat file (`path` on this host, or `text`).
    CheatsLoad {
        path: Option<String>,
        text: Option<String>,
    },
    /// The freezes as a TOML cheat file; written to `path` if given.
    CheatsSave {
        path: Option<String>,
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
    /// Load labels from a file on this host (`path`) or from `text`.
    /// `format`: `vice` or `kickass`; detected from the content if absent.
    /// They belong to `space` (default: the main CPU's) and add to the
    /// symbols already loaded.
    SymbolsLoad {
        path: Option<String>,
        text: Option<String>,
        format: Option<String>,
        space: Option<u16>,
    },
    SymbolsList,
    SymbolsClear,
    /// Ports with their kind, connection state and the devices the
    /// frontend can plug in.
    ListPorts,
    ConnectDevice {
        port: u8,
        device: String,
    },
    DisconnectDevice {
        port: u8,
    },
    ListMediaSlots,
    /// Insert an image file from this host (copy-on-write).
    InsertMedia {
        slot: u8,
        path: String,
    },
    EjectMedia {
        slot: u8,
    },
    /// Write changes back through the slot's backend.
    FlushMedia {
        slot: u8,
    },
    /// `"true"` (true drive emulation) or `"virtual"`.
    SetDriveMode {
        slot: u8,
        mode: String,
    },
    /// Operate the slot's own keys (datasette: "play", "record", "stop",
    /// "rewind", "forward").
    MediaControl {
        slot: u8,
        command: String,
    },
    /// The console's buttons and switches: `[{"id", "name", "kind":
    /// "button" | "switch", "positions": [...], "value"}]`.
    Controls,
    /// Set a button (1 held, 0 released) or switch (position index); queued
    /// like input, applied at the next emulated cycle.
    SetControl {
        /// The control's id (`id` is the request's).
        control: u16,
        value: u8,
    },
    /// Queue input events: `[{"target": "keyboard" | "panel" | {"port": n},
    /// "event": {...}, "delay": cycles}]` (see [`crate::host::event_json`]).
    Input {
        events: Vec<Value>,
    },
    /// Start recording input (from a savestate taken now).
    RecordStart,
    /// Stop recording; the result is the recording.
    RecordStop,
    /// Restore a recording's start state and queue its events.
    ReplayInput {
        replay: Value,
    },
}

/// `break_add` arguments. `kind` is `exec` (default), `read`, `write`,
/// `raster`, `irq` or `nmi`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BreakSpec {
    pub kind: Option<String>,
    pub space: Option<u16>,
    pub addr: Option<u32>,
    /// A loaded symbol instead of `addr` (its space unless `space` is given).
    pub symbol: Option<String>,
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
    "symbols_load",
    "symbols_list",
    "symbols_clear",
    "list_ports",
    "connect_device",
    "disconnect_device",
    "list_media_slots",
    "insert_media",
    "eject_media",
    "flush_media",
    "set_drive_mode",
    "media_control",
    "controls",
    "set_control",
    "input",
    "record_start",
    "record_stop",
    "replay_input",
    "freeze_add",
    "freeze_remove",
    "freeze_toggle",
    "freeze_list",
    "search_start",
    "search_next",
    "search_results",
    "cheats_load",
    "cheats_save",
];

pub const BREAK_KINDS: &[&str] = &["exec", "read", "write", "raster", "irq", "nmi"];
pub const EVENTS: &[&str] = &[
    "hello",
    "resumed",
    "stopped",
    "frame_ready",
    "media_changed",
    "device_connected",
    "freeze_blocked_write",
];
/// Events a client must ask for with `subscribe`.
pub const OPTIONAL_EVENTS: &[&str] = &["frame_ready", "freeze_blocked_write"];

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
