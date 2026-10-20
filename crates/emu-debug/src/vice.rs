//! VICE binary monitor adapter (API version 2), so tools written for VICE
//! (VS64, C64 Studio, IceBro Lite …) debug any machine here.
//!
//! Implemented after the protocol description in the VICE manual, chapter
//! "Binary monitor"; no VICE code is used. Each command runs on the
//! emulation thread against the [`Session`] together with this
//! connection's checkpoint table ([`ViceState`]); the network task only
//! frames bytes.
//!
//! Mapping to the native session:
//!
//! - Memspace 0 is the main CPU (`devices()[0]`), memspaces 1–4 are the
//!   devices named `drive8`–`drive11`. Banks are the main CPU's memory
//!   spaces by name (`cpu`, `ram` …), plus `default` for the first one.
//!   Memory reads are always peeks (golden rule 2); the side-effect flag is
//!   ignored.
//! - Register names follow VICE where they differ only in name (`S` → `SP`,
//!   `P` → `FL`); `LIN` and `CYC` are the beam position (read-only).
//! - Checkpoints become session breakpoints: exec ranges one per address
//!   (at most [`MAX_EXEC_RANGE`]), load/store ranges one watchpoint each.
//!   Conditions use VICE syntax (see [`translate_condition`]).
//! - As in VICE, any command stops a running machine. Stops are reported as
//!   checkpoint (0x11, if one was hit), registers (0x31) and stopped (0x62)
//!   events; resuming as 0x63. Checkpoints belong to the connection and
//!   are removed when it closes.
//!
//! Not supported (error 0x83): keyboard feed, CPU history, joyport and
//! userport set, quit, and resources (error 0x01). Autostart takes PRG and
//! T64 files through the machine's program slot; `run = 0` still runs
//! BASIC programs.

use std::collections::BTreeMap;

use serde_json::Value;

use emu_core::frame::PixelData;
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind};
use emu_core::{DeviceId, MemSpaceId, Size};

use crate::protocol::{BreakSpec, Command};
use crate::session::Session;

pub const DEFAULT_PORT: u16 = 6502;

const STX: u8 = 0x02;
const API_VERSION: u8 = 0x02;
/// Request id of event responses.
pub const EVENT_ID: u32 = 0xffff_ffff;
/// Longest accepted command body (a full 64 KB memory set fits).
pub const MAX_BODY: usize = 1 << 20;
/// Longest exec checkpoint range; each address is one breakpoint.
pub const MAX_EXEC_RANGE: u32 = 256;

pub mod cmd {
    pub const MEMORY_GET: u8 = 0x01;
    pub const MEMORY_SET: u8 = 0x02;
    pub const CHECKPOINT_GET: u8 = 0x11;
    pub const CHECKPOINT_SET: u8 = 0x12;
    pub const CHECKPOINT_DELETE: u8 = 0x13;
    pub const CHECKPOINT_LIST: u8 = 0x14;
    pub const CHECKPOINT_TOGGLE: u8 = 0x15;
    pub const CONDITION_SET: u8 = 0x22;
    pub const REGISTERS_GET: u8 = 0x31;
    pub const REGISTERS_SET: u8 = 0x32;
    pub const DUMP: u8 = 0x41;
    pub const UNDUMP: u8 = 0x42;
    pub const RESOURCE_GET: u8 = 0x51;
    pub const RESOURCE_SET: u8 = 0x52;
    pub const ADVANCE_INSTRUCTIONS: u8 = 0x71;
    pub const KEYBOARD_FEED: u8 = 0x72;
    pub const EXECUTE_UNTIL_RETURN: u8 = 0x73;
    pub const PING: u8 = 0x81;
    pub const BANKS_AVAILABLE: u8 = 0x82;
    pub const REGISTERS_AVAILABLE: u8 = 0x83;
    pub const DISPLAY_GET: u8 = 0x84;
    pub const VICE_INFO: u8 = 0x85;
    pub const PALETTE_GET: u8 = 0x91;
    pub const EXIT: u8 = 0xaa;
    pub const QUIT: u8 = 0xbb;
    pub const RESET: u8 = 0xcc;
    pub const AUTOSTART: u8 = 0xdd;
}

/// Event response types.
pub mod event {
    pub const CHECKPOINT: u8 = 0x11;
    pub const REGISTERS: u8 = 0x31;
    pub const JAM: u8 = 0x61;
    pub const STOPPED: u8 = 0x62;
    pub const RESUMED: u8 = 0x63;
}

pub mod error {
    pub const OK: u8 = 0x00;
    pub const NOT_FOUND: u8 = 0x01;
    pub const BAD_MEMSPACE: u8 = 0x02;
    pub const BAD_LENGTH: u8 = 0x80;
    pub const BAD_PARAMETER: u8 = 0x81;
    pub const BAD_API: u8 = 0x82;
    pub const BAD_COMMAND: u8 = 0x83;
    pub const FAILED: u8 = 0x8f;
}

/// One command as sent by the client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub api: u8,
    pub id: u32,
    pub cmd: u8,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The stream does not start with STX: the connection is out of sync.
    NoStx,
    TooLong,
}

/// Parse one command from the front of `buf`. `Ok(None)` if more bytes are
/// needed; otherwise the request and the number of bytes it used.
pub fn parse(buf: &[u8]) -> Result<Option<(Request, usize)>, FrameError> {
    let Some(&first) = buf.first() else {
        return Ok(None);
    };
    if first != STX {
        return Err(FrameError::NoStx);
    }
    if buf.len() < 11 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]) as usize;
    if len > MAX_BODY {
        return Err(FrameError::TooLong);
    }
    if buf.len() < 11 + len {
        return Ok(None);
    }
    Ok(Some((
        Request {
            api: buf[1],
            id: u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]),
            cmd: buf[10],
            body: buf[11..11 + len].to_vec(),
        },
        11 + len,
    )))
}

/// Encode one response.
pub fn response(kind: u8, error: u8, id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + body.len());
    out.push(STX);
    out.push(API_VERSION);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.push(kind);
    out.push(error);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// A VICE checkpoint and the session breakpoints implementing it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Checkpoint {
    start: u16,
    end: u16,
    stop: bool,
    enabled: bool,
    /// 1 load, 2 store, 4 exec.
    op: u8,
    temporary: bool,
    memspace: u8,
    has_condition: bool,
    bps: Vec<u32>,
}

/// Per-connection adapter state.
#[derive(Clone, Debug, Default)]
pub struct ViceState {
    checkpoints: BTreeMap<u32, Checkpoint>,
    next: u32,
}

/// What a command handler produces: error code and body of the response,
/// plus frames to send after it.
struct Reply {
    error: u8,
    body: Vec<u8>,
    after: Vec<u8>,
}

impl Reply {
    fn ok(body: Vec<u8>) -> Self {
        Reply {
            error: error::OK,
            body,
            after: Vec::new(),
        }
    }

    fn err(code: u8) -> Self {
        Reply {
            error: code,
            body: Vec::new(),
            after: Vec::new(),
        }
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Handle one command on the emulation thread; returns the bytes to send.
pub fn handle(st: &mut ViceState, s: &mut Session, req: &Request) -> Vec<u8> {
    if req.api != 1 && req.api != API_VERSION {
        return response(req.cmd, error::BAD_API, req.id, &[]);
    }
    // Like VICE: any command enters the monitor.
    if s.is_running() {
        let _ = s.command(Command::Pause);
    }
    let mut extra = Vec::new();
    let reply = dispatch(st, s, req, &mut extra);
    let kind = match req.cmd {
        // Checkpoint commands answer with checkpoint info.
        cmd::CHECKPOINT_SET | cmd::CHECKPOINT_GET if reply.error == error::OK => event::CHECKPOINT,
        cmd::REGISTERS_SET if reply.error == error::OK => event::REGISTERS,
        c => c,
    };
    let mut out = extra;
    out.extend(response(kind, reply.error, req.id, &reply.body));
    out.extend(reply.after);
    out
}

/// `extra` receives frames that precede the response (checkpoint list items).
fn dispatch(st: &mut ViceState, s: &mut Session, req: &Request, extra: &mut Vec<u8>) -> Reply {
    let b = &req.body;
    let need = |n: usize| b.len() >= n;
    match req.cmd {
        cmd::MEMORY_GET => {
            if !need(6) {
                return Reply::err(error::BAD_LENGTH);
            }
            let (start, end, ms) = (u16_at(b, 1), u16_at(b, 3), b[5]);
            let bank = if need(8) { u16_at(b, 6) } else { 0 };
            if end < start {
                return Reply::err(error::BAD_PARAMETER);
            }
            let Some((dev, space)) = resolve_space(s, ms, bank) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            let Some(target) = s.machine_mut().debug_target(dev) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            let len = u32::from(end - start) + 1;
            let mut body = Vec::with_capacity(2 + len as usize);
            body.extend_from_slice(&(len as u16).to_le_bytes());
            for i in 0..len {
                match target.peek(space, u32::from(start) + i, Size::Byte) {
                    Some(v) => body.push(v as u8),
                    None => return Reply::err(error::FAILED),
                }
            }
            Reply::ok(body)
        }
        cmd::MEMORY_SET => {
            if !need(8) {
                return Reply::err(error::BAD_LENGTH);
            }
            let (start, end, ms, bank) = (u16_at(b, 1), u16_at(b, 3), b[5], u16_at(b, 6));
            if end < start {
                return Reply::err(error::BAD_PARAMETER);
            }
            let data = &b[8..];
            if data.len() != usize::from(end - start) + 1 {
                return Reply::err(error::BAD_LENGTH);
            }
            let Some((dev, space)) = resolve_space(s, ms, bank) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            let Some(target) = s.machine_mut().debug_target(dev) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            for (i, v) in data.iter().enumerate() {
                if !target.poke(
                    space,
                    u32::from(start) + i as u32,
                    Size::Byte,
                    u32::from(*v),
                ) {
                    return Reply::err(error::FAILED);
                }
            }
            Reply::ok(Vec::new())
        }
        cmd::CHECKPOINT_GET => {
            if !need(4) {
                return Reply::err(error::BAD_LENGTH);
            }
            let n = u32_at(b, 0);
            match st.checkpoints.get(&n) {
                Some(cp) => Reply::ok(checkpoint_body(s, n, cp, false)),
                None => Reply::err(error::NOT_FOUND),
            }
        }
        cmd::CHECKPOINT_SET => checkpoint_set(st, s, b),
        cmd::CHECKPOINT_DELETE => {
            if !need(4) {
                return Reply::err(error::BAD_LENGTH);
            }
            match st.checkpoints.remove(&u32_at(b, 0)) {
                Some(cp) => {
                    for bp in cp.bps {
                        let _ = s.command(Command::BreakRemove { bp });
                    }
                    Reply::ok(Vec::new())
                }
                None => Reply::err(error::NOT_FOUND),
            }
        }
        cmd::CHECKPOINT_LIST => {
            for (n, cp) in &st.checkpoints {
                extra.extend(response(
                    event::CHECKPOINT,
                    error::OK,
                    req.id,
                    &checkpoint_body(s, *n, cp, false),
                ));
            }
            Reply::ok((st.checkpoints.len() as u32).to_le_bytes().to_vec())
        }
        cmd::CHECKPOINT_TOGGLE => {
            if !need(5) {
                return Reply::err(error::BAD_LENGTH);
            }
            let Some(cp) = st.checkpoints.get_mut(&u32_at(b, 0)) else {
                return Reply::err(error::NOT_FOUND);
            };
            cp.enabled = b[4] != 0;
            for &bp in &cp.bps {
                let _ = s.command(Command::BreakEnable {
                    bp,
                    enabled: cp.enabled,
                });
            }
            Reply::ok(Vec::new())
        }
        cmd::CONDITION_SET => {
            if !need(5) || b.len() < 5 + usize::from(b[4]) {
                return Reply::err(error::BAD_LENGTH);
            }
            let Some(cp) = st.checkpoints.get_mut(&u32_at(b, 0)) else {
                return Reply::err(error::NOT_FOUND);
            };
            let text = String::from_utf8_lossy(&b[5..5 + usize::from(b[4])]).into_owned();
            let condition = translate_condition(&text, &register_aliases(s));
            for &bp in &cp.bps {
                let r = s.command(Command::BreakCondition {
                    bp,
                    condition: Some(condition.clone()),
                });
                if r.is_err() {
                    return Reply::err(error::BAD_PARAMETER);
                }
            }
            cp.has_condition = true;
            Reply::ok(Vec::new())
        }
        cmd::REGISTERS_GET => {
            let ms = b.first().copied().unwrap_or(0);
            match registers_body(s, ms) {
                Some(body) => Reply::ok(body),
                None => Reply::err(error::BAD_MEMSPACE),
            }
        }
        cmd::REGISTERS_SET => {
            if !need(3) {
                return Reply::err(error::BAD_LENGTH);
            }
            let ms = b[0];
            let Some(dev) = memspace_device(s, ms) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            let table = register_table(s, dev);
            let count = usize::from(u16_at(b, 1));
            let mut i = 3;
            for _ in 0..count {
                let Some(&size) = b.get(i) else {
                    return Reply::err(error::BAD_LENGTH);
                };
                let size = usize::from(size);
                if size < 3 || b.len() < i + 1 + size {
                    return Reply::err(error::BAD_LENGTH);
                }
                let (id, value) = (b[i + 1], u16_at(b, i + 2));
                let Some(reg) = table.get(usize::from(id)) else {
                    return Reply::err(error::NOT_FOUND);
                };
                if let Some(name) = reg.native {
                    if let Some(t) = s.machine_mut().debug_target(dev) {
                        t.set_reg(name, u32::from(value));
                    }
                }
                i += 1 + size;
            }
            match registers_body(s, ms) {
                Some(body) => Reply::ok(body),
                None => Reply::err(error::BAD_MEMSPACE),
            }
        }
        cmd::DUMP => {
            if !need(3) || b.len() < 3 + usize::from(b[2]) {
                return Reply::err(error::BAD_LENGTH);
            }
            let path = String::from_utf8_lossy(&b[3..3 + usize::from(b[2])]).into_owned();
            match std::fs::write(&path, s.machine().save_state()) {
                Ok(()) => Reply::ok(Vec::new()),
                Err(_) => Reply::err(error::FAILED),
            }
        }
        cmd::UNDUMP => {
            if !need(1) || b.len() < 1 + usize::from(b[0]) {
                return Reply::err(error::BAD_LENGTH);
            }
            let path = String::from_utf8_lossy(&b[1..1 + usize::from(b[0])]).into_owned();
            let Ok(data) = std::fs::read(&path) else {
                return Reply::err(error::FAILED);
            };
            if s.machine_mut().load_state(&data).is_err() {
                return Reply::err(error::FAILED);
            }
            Reply::ok(main_pc(s).to_le_bytes().to_vec())
        }
        cmd::RESOURCE_GET | cmd::RESOURCE_SET => Reply::err(error::NOT_FOUND),
        cmd::ADVANCE_INSTRUCTIONS => {
            if !need(3) {
                return Reply::err(error::BAD_LENGTH);
            }
            let (over, count) = (b[0] != 0, u32::from(u16_at(b, 1)).max(1));
            if over {
                // Stepping over is asynchronous; the stop arrives as an event.
                if count > 1 {
                    return Reply::err(error::BAD_PARAMETER);
                }
                return match s.command(Command::StepOver) {
                    Ok(_) => Reply::ok(Vec::new()),
                    Err(_) => Reply::err(error::FAILED),
                };
            }
            match s.command(Command::StepInstr { count: Some(count) }) {
                Ok(state) => {
                    let mut r = Reply::ok(Vec::new());
                    r.after = stop_frames(st, s, &state);
                    r
                }
                Err(_) => Reply::err(error::FAILED),
            }
        }
        cmd::EXECUTE_UNTIL_RETURN => match s.command(Command::StepOut) {
            Ok(_) => Reply::ok(Vec::new()),
            Err(_) => Reply::err(error::FAILED),
        },
        cmd::PING => Reply::ok(Vec::new()),
        cmd::BANKS_AVAILABLE => {
            let banks = banks(s);
            let mut body = (banks.len() as u16).to_le_bytes().to_vec();
            for (id, name) in banks {
                let name = &name.as_bytes()[..name.len().min(255)];
                body.push(3 + name.len() as u8);
                body.extend_from_slice(&id.to_le_bytes());
                body.push(name.len() as u8);
                body.extend_from_slice(name);
            }
            Reply::ok(body)
        }
        cmd::REGISTERS_AVAILABLE => {
            let ms = b.first().copied().unwrap_or(0);
            let Some(dev) = memspace_device(s, ms) else {
                return Reply::err(error::BAD_MEMSPACE);
            };
            let table = register_table(s, dev);
            let mut body = (table.len() as u16).to_le_bytes().to_vec();
            for (id, reg) in table.iter().enumerate() {
                let name = reg.name.as_bytes();
                body.push(3 + name.len() as u8);
                body.push(id as u8);
                body.push(reg.bits);
                body.push(name.len() as u8);
                body.extend_from_slice(name);
            }
            Reply::ok(body)
        }
        cmd::DISPLAY_GET => {
            let f = s.machine().framebuffer();
            let PixelData::Indexed { pixels, .. } = f.pixels else {
                return Reply::err(error::FAILED);
            };
            let (w, h) = (f.width as u16, f.height as u16);
            let mut body = 13u32.to_le_bytes().to_vec();
            for v in [w, h, 0, 0, w, h] {
                body.extend_from_slice(&v.to_le_bytes());
            }
            body.push(8);
            body.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
            body.extend_from_slice(pixels);
            Reply::ok(body)
        }
        cmd::PALETTE_GET => {
            let f = s.machine().framebuffer();
            let PixelData::Indexed { palette, .. } = f.pixels else {
                return Reply::err(error::FAILED);
            };
            let mut body = (palette.len() as u16).to_le_bytes().to_vec();
            for c in palette {
                body.extend_from_slice(&[3, c.r, c.g, c.b]);
            }
            Reply::ok(body)
        }
        cmd::VICE_INFO => Reply::ok(vec![4, 3, 8, 0, 0, 4, 0, 0, 0, 0]),
        cmd::EXIT => match s.command(Command::Resume) {
            Ok(_) => Reply::ok(Vec::new()),
            Err(_) => Reply::err(error::FAILED),
        },
        cmd::RESET => {
            let kind = match b.first().copied().unwrap_or(0) {
                0 => "soft",
                1 => "hard",
                _ => return Reply::err(error::BAD_PARAMETER),
            };
            match s.command(Command::Reset {
                kind: Some(kind.into()),
            }) {
                Ok(_) => Reply::ok(Vec::new()),
                Err(_) => Reply::err(error::FAILED),
            }
        }
        cmd::AUTOSTART => autostart(s, b),
        _ => Reply::err(error::BAD_COMMAND),
    }
}

fn checkpoint_set(st: &mut ViceState, s: &mut Session, b: &[u8]) -> Reply {
    if b.len() < 8 {
        return Reply::err(error::BAD_LENGTH);
    }
    let cp = Checkpoint {
        start: u16_at(b, 0),
        end: u16_at(b, 2),
        stop: b[4] != 0,
        enabled: b[5] != 0,
        op: b[6] & 7,
        temporary: b[7] != 0,
        memspace: b.get(8).copied().unwrap_or(0),
        has_condition: false,
        bps: Vec::new(),
    };
    if cp.end < cp.start || cp.op == 0 {
        return Reply::err(error::BAD_PARAMETER);
    }
    let len = u32::from(cp.end - cp.start) + 1;
    if cp.op & 4 != 0 && len > MAX_EXEC_RANGE {
        return Reply::err(error::BAD_PARAMETER);
    }
    let Some((_, space)) = resolve_space(s, cp.memspace, 0) else {
        return Reply::err(error::BAD_MEMSPACE);
    };
    let mut specs = Vec::new();
    if cp.op & 4 != 0 {
        for a in cp.start..=cp.end {
            specs.push(("exec", u32::from(a), 1));
        }
    }
    if cp.op & 1 != 0 {
        specs.push(("read", u32::from(cp.start), len));
    }
    if cp.op & 2 != 0 {
        specs.push(("write", u32::from(cp.start), len));
    }
    let mut bps = Vec::new();
    for (kind, addr, len) in specs {
        let r = s.command(Command::BreakAdd(BreakSpec {
            kind: Some(kind.into()),
            space: Some(space.0),
            addr: Some(addr),
            len: Some(len),
            enabled: Some(cp.enabled),
            ..BreakSpec::default()
        }));
        match r.ok().and_then(|v| v["bp"].as_u64()) {
            Some(bp) => bps.push(bp as u32),
            None => {
                for bp in bps {
                    let _ = s.command(Command::BreakRemove { bp });
                }
                return Reply::err(error::FAILED);
            }
        }
    }
    st.next += 1;
    let n = st.next;
    let cp = Checkpoint { bps, ..cp };
    let body = checkpoint_body(s, n, &cp, false);
    st.checkpoints.insert(n, cp);
    Reply::ok(body)
}

fn checkpoint_body(s: &mut Session, n: u32, cp: &Checkpoint, hit: bool) -> Vec<u8> {
    let hits: u32 = match s.command(Command::BreakList) {
        Ok(Value::Array(list)) => list
            .iter()
            .filter(|bp| {
                bp["bp"]
                    .as_u64()
                    .is_some_and(|id| cp.bps.contains(&(id as u32)))
            })
            .filter_map(|bp| bp["hits"].as_u64())
            .sum::<u64>() as u32,
        _ => 0,
    };
    let mut body = n.to_le_bytes().to_vec();
    body.push(u8::from(hit));
    body.extend_from_slice(&cp.start.to_le_bytes());
    body.extend_from_slice(&cp.end.to_le_bytes());
    body.extend_from_slice(&[
        u8::from(cp.stop),
        u8::from(cp.enabled),
        cp.op,
        u8::from(cp.temporary),
    ]);
    body.extend_from_slice(&hits.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.push(u8::from(cp.has_condition));
    body.push(cp.memspace);
    body
}

fn autostart(s: &mut Session, b: &[u8]) -> Reply {
    if b.len() < 4 || b.len() < 4 + usize::from(b[3]) {
        return Reply::err(error::BAD_LENGTH);
    }
    let file_index = u16_at(b, 1);
    let path = String::from_utf8_lossy(&b[4..4 + usize::from(b[3])]).into_owned();
    if file_index != 0 {
        return Reply::err(error::BAD_PARAMETER);
    }
    let lower = path.to_ascii_lowercase();
    if [".d64", ".d71", ".d81", ".g64"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        // Disk autostart needs a machine-level autostart command.
        return Reply::err(error::BAD_COMMAND);
    }
    let Ok(data) = std::fs::read(&path) else {
        return Reply::err(error::FAILED);
    };
    let name = std::path::Path::new(&path)
        .file_name()
        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    if s.command(Command::Reset {
        kind: Some("soft".into()),
    })
    .is_err()
    {
        return Reply::err(error::FAILED);
    }
    let m = s.machine_mut();
    let Some(slot) = m
        .media_slots()
        .into_iter()
        .find(|slot| slot.kind == MediaKind::Program)
    else {
        return Reply::err(error::BAD_COMMAND);
    };
    let Some(slot) = m.media_slot(slot.id) else {
        return Reply::err(error::FAILED);
    };
    let media = Media {
        name,
        kind: MediaKind::Program,
        backend: Box::new(Bytes(data)),
    };
    match slot.insert(media) {
        Ok(()) => Reply::ok(Vec::new()),
        Err(_) => Reply::err(error::FAILED),
    }
}

/// An in-memory image for autostart.
struct Bytes(Vec<u8>);

impl MediaBackend for Bytes {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        let start = usize::try_from(offset).map_err(|_| MediaError::OutOfRange)?;
        let src = self
            .0
            .get(start..start + buf.len())
            .ok_or(MediaError::OutOfRange)?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn write(&mut self, _offset: u64, _data: &[u8]) -> Result<(), MediaError> {
        Err(MediaError::WriteProtected)
    }

    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The connection closed: remove its checkpoints.
pub fn disconnect(st: &mut ViceState, s: &mut Session) {
    for (_, cp) in std::mem::take(&mut st.checkpoints) {
        for bp in cp.bps {
            let _ = s.command(Command::BreakRemove { bp });
        }
    }
}

/// Translate a session event into VICE events (run on the emulation thread).
pub fn on_event(st: &mut ViceState, s: &mut Session, ev: &Value) -> Vec<u8> {
    match ev["event"].as_str() {
        Some("stopped") => stop_frames(st, s, ev),
        Some("resumed") => {
            let pc = ev["pc"].as_u64().unwrap_or(0) as u16;
            response(event::RESUMED, error::OK, EVENT_ID, &pc.to_le_bytes())
        }
        _ => Vec::new(),
    }
}

/// Checkpoint (if one was hit), registers and stopped events for a stop
/// state (`reason`, `pc`).
fn stop_frames(st: &mut ViceState, s: &mut Session, state: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    let reason = &state["reason"];
    if let Some(bp) = reason["bp"].as_u64() {
        let hit = st
            .checkpoints
            .iter()
            .find(|(_, cp)| cp.bps.contains(&(bp as u32)))
            .map(|(n, cp)| (*n, cp.clone()));
        if let Some((n, cp)) = hit {
            out.extend(response(
                event::CHECKPOINT,
                error::OK,
                EVENT_ID,
                &checkpoint_body(s, n, &cp, true),
            ));
            if cp.temporary {
                st.checkpoints.remove(&n);
                for bp in cp.bps {
                    let _ = s.command(Command::BreakRemove { bp });
                }
            }
            if !cp.stop {
                // A trace point: report and carry on.
                let _ = s.command(Command::Resume);
                return out;
            }
        }
    }
    if let Some(body) = registers_body(s, 0) {
        out.extend(response(event::REGISTERS, error::OK, EVENT_ID, &body));
    }
    let pc = state["pc"]
        .as_u64()
        .map_or_else(|| main_pc(s), |pc| pc as u16)
        .to_le_bytes();
    if reason["kind"] == "cpu_jam" {
        out.extend(response(event::JAM, error::OK, EVENT_ID, &pc));
    }
    out.extend(response(event::STOPPED, error::OK, EVENT_ID, &pc));
    out
}

/// One register as VICE clients see it.
struct ViceReg {
    name: String,
    /// The native register behind it; `None` for the beam values.
    native: Option<&'static str>,
    bits: u8,
    value: u32,
}

/// VICE names for native register names that differ only in spelling.
fn vice_name(native: &str) -> String {
    match native.to_ascii_uppercase().as_str() {
        "S" => "SP".into(),
        "P" => "FL".into(),
        other => other.into(),
    }
}

fn register_table(s: &mut Session, dev: DeviceId) -> Vec<ViceReg> {
    let beam = s.machine().beam();
    let mut table: Vec<ViceReg> = s
        .machine_mut()
        .debug_target(dev)
        .map(|t| t.regs().0)
        .unwrap_or_default()
        .into_iter()
        .map(|r| ViceReg {
            name: vice_name(r.name),
            native: Some(r.name),
            bits: r.bits,
            value: r.value,
        })
        .collect();
    if let Some(b) = beam {
        table.push(ViceReg {
            name: "LIN".into(),
            native: None,
            bits: 16,
            value: u32::from(b.line),
        });
        table.push(ViceReg {
            name: "CYC".into(),
            native: None,
            bits: 16,
            value: u32::from(b.cycle),
        });
    }
    table
}

/// (VICE name, native name) pairs for condition translation.
fn register_aliases(s: &mut Session) -> Vec<(String, String)> {
    let Some(dev) = s.machine().devices().first().copied() else {
        return Vec::new();
    };
    register_table(s, dev)
        .into_iter()
        .map(|r| {
            let native = r.native.map_or_else(
                || {
                    if r.name == "LIN" {
                        "line".into()
                    } else {
                        "cycle".into()
                    }
                },
                str::to_owned,
            );
            (r.name, native)
        })
        .collect()
}

fn registers_body(s: &mut Session, ms: u8) -> Option<Vec<u8>> {
    let dev = memspace_device(s, ms)?;
    let table = register_table(s, dev);
    let mut body = (table.len() as u16).to_le_bytes().to_vec();
    for (id, r) in table.iter().enumerate() {
        body.push(3);
        body.push(id as u8);
        body.extend_from_slice(&(r.value as u16).to_le_bytes());
    }
    Some(body)
}

fn main_pc(s: &mut Session) -> u16 {
    let Some(dev) = s.machine().devices().first().copied() else {
        return 0;
    };
    s.machine_mut()
        .debug_target(dev)
        .and_then(|t| t.regs().get("PC"))
        .unwrap_or(0) as u16
}

/// Memspace 0: the main CPU; 1–4: drives 8–11.
fn memspace_device(s: &Session, ms: u8) -> Option<DeviceId> {
    let devices = s.machine().devices();
    match ms {
        0 => devices.first().copied(),
        1..=4 => {
            let name = format!("drive{}", 7 + ms);
            devices.into_iter().find(|d| d.0 == name)
        }
        _ => None,
    }
}

/// Banks: the main CPU's memory spaces, `default` first.
fn banks(s: &Session) -> Vec<(u16, String)> {
    let Some(main) = s.machine().devices().first().copied() else {
        return Vec::new();
    };
    let spaces: Vec<_> = s
        .machine()
        .memory_spaces()
        .into_iter()
        .filter(|sp| sp.device == main)
        .collect();
    let mut out = Vec::new();
    if let Some(first) = spaces.first() {
        out.push((first.id.0, "default".to_string()));
    }
    out.extend(spaces.iter().map(|sp| (sp.id.0, sp.name.to_string())));
    out
}

/// The memory space for a memspace/bank pair. The bank selects among the
/// main CPU's spaces and is ignored for drives.
fn resolve_space(s: &Session, ms: u8, bank: u16) -> Option<(DeviceId, MemSpaceId)> {
    let dev = memspace_device(s, ms)?;
    let spaces: Vec<_> = s
        .machine()
        .memory_spaces()
        .into_iter()
        .filter(|sp| sp.device == dev)
        .collect();
    let space = if ms == 0 {
        spaces
            .iter()
            .find(|sp| sp.id.0 == bank)
            .or_else(|| spaces.first())?
    } else {
        spaces.first()?
    };
    Some((dev, space.id))
}

/// VICE monitor condition syntax to [`crate::expr`] syntax: register names
/// (`A`, `SP`, `.A`, `RL`/`LIN` = raster line, `CY`/`CYC` = cycle), bare
/// numbers are hex as in the VICE monitor, `@bank:addr` reads memory.
/// Already-prefixed numbers (`$`, `%`, `0x`) and operators pass through.
pub fn translate_condition(text: &str, regs: &[(String, String)]) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut close_mem = false;
    while i < b.len() {
        let c = b[i] as char;
        if c == '@' {
            // @bank:addr → [addr]
            let colon = text[i..].find(':').map(|p| i + p + 1).unwrap_or(i + 1);
            out.push('[');
            close_mem = true;
            i = colon;
            continue;
        }
        if c == '.' && b.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic()) {
            i += 1;
            continue;
        }
        if c == '$' || c == '%' {
            let end = (i + 1..b.len())
                .find(|&j| !b[j].is_ascii_hexdigit())
                .unwrap_or(b.len());
            out.push_str(&text[i..end]);
            i = end;
            if close_mem {
                out.push(']');
                close_mem = false;
            }
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            let end = (i..b.len())
                .find(|&j| !(b[j].is_ascii_alphanumeric() || b[j] == b'_'))
                .unwrap_or(b.len());
            let word = &text[i..end];
            let upper = word.to_ascii_uppercase();
            let mapped = match upper.as_str() {
                "RL" | "LIN" => Some("line".to_string()),
                "CY" | "CYC" => Some("cycle".to_string()),
                _ => regs
                    .iter()
                    .find(|(v, _)| *v == upper)
                    .map(|(_, n)| n.clone()),
            };
            match mapped {
                Some(name) => out.push_str(&name),
                None if upper.starts_with("0X") => out.push_str(word),
                None if word.bytes().all(|d| d.is_ascii_hexdigit()) => {
                    out.push('$');
                    out.push_str(word);
                }
                None => out.push_str(word),
            }
            i = end;
            if close_mem {
                out.push(']');
                close_mem = false;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    if close_mem {
        out.push(']');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_parse_incrementally() {
        let msg = [
            0x02, 0x02, 0x08, 0, 0, 0, 0xad, 0xde, 0x34, 0x12, 0x12, 0xe2, 0xfc, 0xe3, 0xfc, 1, 1,
            4, 1,
        ];
        for n in 0..msg.len() {
            assert_eq!(parse(&msg[..n]), Ok(None), "{n} bytes");
        }
        let (req, used) = parse(&msg).unwrap().unwrap();
        assert_eq!(used, msg.len());
        assert_eq!(req.id, 0x1234_dead);
        assert_eq!(req.cmd, cmd::CHECKPOINT_SET);
        assert_eq!(req.body, [0xe2, 0xfc, 0xe3, 0xfc, 1, 1, 4, 1]);
        assert_eq!(parse(&[0x41]), Err(FrameError::NoStx));
        let huge = [0x02, 0x02, 0xff, 0xff, 0xff, 0x7f, 0, 0, 0, 0, 0x01];
        assert_eq!(parse(&huge), Err(FrameError::TooLong));
    }

    #[test]
    fn responses_follow_the_documented_header() {
        // Stopped event from the manual: 02 02 02000000 62 00 ffffffff cf e5
        assert_eq!(
            response(event::STOPPED, error::OK, EVENT_ID, &[0xcf, 0xe5]),
            [0x02, 0x02, 2, 0, 0, 0, 0x62, 0x00, 0xff, 0xff, 0xff, 0xff, 0xcf, 0xe5]
        );
    }

    #[test]
    fn conditions_translate_to_native_syntax() {
        let regs = vec![
            ("A".to_string(), "A".to_string()),
            ("SP".to_string(), "S".to_string()),
            ("FL".to_string(), "P".to_string()),
        ];
        let t = |s: &str| translate_condition(s, &regs);
        assert_eq!(t("A == ff"), "A == $ff");
        assert_eq!(t(".A == $10 && SP < f0"), "A == $10 && S < $f0");
        assert_eq!(t("@io:$d012 == 30"), "[$d012] == $30");
        assert_eq!(t("@cpu:d020 != 0"), "[$d020] != $0");
        assert_eq!(t("RL == 100 || CY > 0x20"), "line == $100 || cycle > 0x20");
        assert_eq!(t("(FL & %00000010) == 2"), "(P & %00000010) == $2");
        for case in ["A == ff", "@io:$d012 == 30", "RL == 100 || CY > 0x20"] {
            assert!(crate::expr::parse(&t(case)).is_ok(), "{case}");
        }
    }
}
