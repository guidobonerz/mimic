//! One debugged machine: executes protocol commands. No I/O here; the
//! server feeds requests in and ships responses and events out, and tests
//! drive it directly.
//!
//! Breakpoint conditions and ignore counts live here, not in
//! [`DebugHooks`]: the hooks stop on every hit, the session evaluates the
//! condition with peeks and resumes silently if the hit does not count.
//! Asynchronous steps (`step_over`, `step_out`, `run_to`) are run modes of
//! the running session; [`Session::run_slice`] finishes them.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use emu_core::{
    BeamPos, BpId, BreakpointKind, DebugHooks, DebugTarget, DeviceId, Flow, InterruptKind, Machine,
    MemSpaceId, RegisterSet, ResetKind, Size, StopReason, TraceConfig, TraceEvent, TraceRange,
};

use crate::expr::{self, Expr};
use crate::protocol::{
    self, BreakSpec, Command, RangeSpec, Request, MAX_DISASM, MAX_PEEK, MAX_STEP_COUNT,
    MAX_TRACE_CAPACITY,
};
use crate::symbols::{self, SymbolTable};

/// Frames a synchronous step may run before giving up (e.g. `step_instr` on
/// a CPU held off the bus). Bounded so a command can never hang the session.
const MAX_STEP_FRAMES: u32 = 500;

/// Session-side breakpoint data.
#[derive(Clone, Debug)]
struct BpMeta {
    condition: Option<(String, Expr)>,
    ignore: u32,
    /// Hits that counted (condition held).
    hits: u32,
    /// Where `[addr]` in the condition reads.
    space: MemSpaceId,
    /// Whose registers the condition sees.
    device: DeviceId,
}

/// What the running session is doing besides running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunMode {
    Free,
    /// One instruction of the main CPU.
    Instr,
    /// A call is being stepped over: an internal breakpoint at the return
    /// address that only counts at the caller's stack depth or above.
    StepOver {
        bp: BpId,
        min_sp: Option<u32>,
    },
    /// Until an instruction with [`Flow::Return`] leaves the stack above
    /// `sp0`. `current` is the flow of the instruction being executed.
    StepOut {
        sp0: u32,
        current: Option<Flow>,
    },
    RunTo {
        bp: BpId,
    },
}

/// What breakpoint conditions see.
struct CondCtx<'a> {
    regs: RegisterSet,
    target: Option<&'a dyn DebugTarget>,
    space: MemSpaceId,
    beam: Option<BeamPos>,
}

impl expr::Context for CondCtx<'_> {
    fn reg(&self, name: &str) -> Option<u32> {
        self.regs.get(name)
    }

    fn peek(&self, addr: u32) -> Option<u32> {
        self.target?.peek(self.space, addr, Size::Byte)
    }

    fn beam(&self, name: &str) -> Option<u32> {
        let b = self.beam?;
        match name {
            "line" => Some(u32::from(b.line)),
            "cycle" => Some(u32::from(b.cycle)),
            "frame" => Some(b.frame as u32),
            _ => None,
        }
    }
}

/// The stack pointer by its conventional names.
fn stack_pointer(regs: &RegisterSet) -> Option<u32> {
    regs.get("SP").or_else(|| regs.get("S"))
}

pub struct Session {
    machine: Box<dyn Machine>,
    machine_id: String,
    hooks: DebugHooks,
    running: bool,
    mode: RunMode,
    bp_meta: BTreeMap<BpId, BpMeta>,
    /// Events produced since the last [`Session::take_events`].
    events: Vec<Value>,
    /// Connections that want `frame_ready`.
    frame_subscribers: u32,
    /// Why the last run ended (`None` while running or never run).
    last_stop: Option<StopReason>,
    symbols: SymbolTable,
    frame_hook: Option<FrameHook>,
}

/// Called with the machine whenever its picture may have changed: after
/// every frame while running (`true`), and after stops and commands while
/// paused (`false`; the frame may be partial). The live preview feeds its
/// frame copies from here.
pub type FrameHook = Box<dyn FnMut(&dyn Machine, bool) + Send>;

impl Session {
    pub fn new(machine: Box<dyn Machine>, machine_id: impl Into<String>) -> Self {
        Self {
            machine,
            machine_id: machine_id.into(),
            hooks: DebugHooks::new(),
            running: false,
            mode: RunMode::Free,
            bp_meta: BTreeMap::new(),
            events: Vec::new(),
            frame_subscribers: 0,
            last_stop: None,
            symbols: SymbolTable::default(),
            frame_hook: None,
        }
    }

    pub fn set_frame_hook(&mut self, hook: FrameHook) {
        self.frame_hook = Some(hook);
    }

    /// Run the frame hook now (adapters call this after commands that
    /// bypass [`Session::handle`]).
    pub fn notify_frame(&mut self) {
        if let Some(hook) = &mut self.frame_hook {
            hook(self.machine.as_ref(), self.running);
        }
    }

    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    pub fn machine(&self) -> &dyn Machine {
        self.machine.as_ref()
    }

    pub fn machine_mut(&mut self) -> &mut dyn Machine {
        self.machine.as_mut()
    }

    /// Why the session is paused: the end of the last run, `StepDone` for
    /// `pause` and synchronous steps. `None` while running.
    pub fn last_stop(&self) -> Option<StopReason> {
        if self.running {
            None
        } else {
            self.last_stop
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Start or stop free running (no events; for frontends at start-up).
    pub fn set_running(&mut self, running: bool) {
        self.cancel_mode();
        self.running = running;
    }

    /// A connection subscribed to (`true`) or left (`false`) `frame_ready`.
    pub fn frame_subscriber(&mut self, added: bool) {
        if added {
            self.frame_subscribers += 1;
        } else {
            self.frame_subscribers = self.frame_subscribers.saturating_sub(1);
        }
    }

    /// Events produced by commands and [`Session::run_slice`] since the last call.
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    /// Emulated duration of one field, for real-time pacing.
    pub fn frame_duration(&self) -> Duration {
        let s = self.machine.framebuffer().signal;
        let pixels = u64::from(s.lines_per_field) * u64::from(s.total_pixels);
        match u64::from(s.pixel_clock_hz) {
            0 => Duration::from_millis(20),
            hz => Duration::from_nanos(pixels.saturating_mul(1_000_000_000) / hz),
        }
    }

    /// While running: emulate up to the end of the current frame. A run that
    /// ends (breakpoint, finished step, jam) queues a `stopped` event; a
    /// finished frame queues `frame_ready` if anyone subscribed.
    pub fn run_slice(&mut self) {
        if !self.running {
            return;
        }
        let reason = match self.mode {
            RunMode::Free | RunMode::StepOver { .. } | RunMode::RunTo { .. } => self.run_filtered(),
            RunMode::Instr => {
                self.hooks.set_instr_step(true);
                self.run_filtered()
            }
            RunMode::StepOut { .. } => self.run_step_out(),
        };
        match reason {
            StopReason::FrameDone => {
                if self.frame_subscribers > 0 {
                    self.events.push(json!({
                        "event": "frame_ready",
                        "beam": protocol::beam_json(self.machine.beam()),
                    }));
                }
            }
            reason => {
                self.cancel_mode();
                self.running = false;
                self.last_stop = Some(reason);
                let mut ev = self.state_json(&reason);
                ev["event"] = json!("stopped");
                self.events.push(ev);
            }
        }
        self.notify_frame();
    }

    /// Step out: instruction by instruction until the frame ends or a
    /// return leaves the current subroutine.
    fn run_step_out(&mut self) -> StopReason {
        loop {
            let RunMode::StepOut { sp0, current } = self.mode else {
                return StopReason::StepDone;
            };
            let flow = match current {
                Some(f) => f,
                None => match self.main_pc() {
                    Some(pc) => self.disasm_main(pc).map_or(Flow::Normal, |i| i.flow),
                    None => return StopReason::StepDone,
                },
            };
            self.mode = RunMode::StepOut {
                sp0,
                current: Some(flow),
            };
            self.hooks.set_instr_step(true);
            match self.run_filtered() {
                StopReason::StepDone => {
                    self.mode = RunMode::StepOut { sp0, current: None };
                    let sp = self.main_regs().as_ref().and_then(stack_pointer);
                    if flow == Flow::Return && sp.is_some_and(|sp| sp > sp0) {
                        return StopReason::StepDone;
                    }
                }
                other => return other,
            }
        }
    }

    /// `run_frame`, resuming silently past hits that do not count.
    fn run_filtered(&mut self) -> StopReason {
        loop {
            let reason = self.machine.run_frame(&mut self.hooks);
            if let Some(reason) = self.counts(reason) {
                return reason;
            }
        }
    }

    /// The stop to report for `reason`, or `None` to keep running.
    fn counts(&mut self, reason: StopReason) -> Option<StopReason> {
        let id = match reason {
            StopReason::Breakpoint(id) | StopReason::Watchpoint { id, .. } => id,
            other => return Some(other),
        };
        match self.mode {
            RunMode::RunTo { bp } if bp == id => return Some(StopReason::StepDone),
            RunMode::StepOver { bp, min_sp } if bp == id => {
                let sp = self.main_regs().as_ref().and_then(stack_pointer);
                return match (min_sp, sp) {
                    (Some(min), Some(sp)) if sp < min => None,
                    _ => Some(StopReason::StepDone),
                };
            }
            _ => {}
        }
        let Some(meta) = self.bp_meta.get(&id).cloned() else {
            return Some(reason);
        };
        if let Some((_, cond)) = &meta.condition {
            if !self.condition_holds(cond, meta.device, meta.space) {
                return None;
            }
        }
        let meta = self.bp_meta.get_mut(&id)?;
        meta.hits = meta.hits.saturating_add(1);
        (meta.hits > meta.ignore).then_some(reason)
    }

    fn condition_holds(&mut self, cond: &Expr, device: DeviceId, space: MemSpaceId) -> bool {
        let beam = self.machine.beam();
        let regs = self
            .machine
            .debug_target(device)
            .map(|t| t.regs())
            .unwrap_or_default();
        let owner = self.space_owner(space).unwrap_or(device);
        let target = self.machine.debug_target(owner).map(|t| &*t);
        cond.holds(&CondCtx {
            regs,
            target,
            space,
            beam,
        })
    }

    /// End an asynchronous step: remove its internal breakpoint.
    fn cancel_mode(&mut self) {
        match std::mem::replace(&mut self.mode, RunMode::Free) {
            RunMode::StepOver { bp, .. } | RunMode::RunTo { bp } => {
                self.hooks.remove_breakpoint(bp);
            }
            RunMode::Instr | RunMode::StepOut { .. } => self.hooks.set_instr_step(false),
            RunMode::Free => {}
        }
    }

    /// Start an asynchronous run.
    fn start(&mut self, mode: RunMode) -> Value {
        self.mode = mode;
        self.running = true;
        let pc = self.main_pc();
        self.events.push(json!({"event": "resumed", "pc": pc}));
        json!({"running": true})
    }

    pub fn handle(&mut self, req: Request) -> Value {
        let response = match self.execute(req.cmd) {
            Ok(result) => protocol::ok(req.id, result),
            Err(msg) => protocol::err(Some(req.id), msg),
        };
        if !self.running {
            self.notify_frame();
        }
        response
    }

    /// Execute a command directly (adapters): the `result` of a response,
    /// or its error message.
    pub fn command(&mut self, cmd: Command) -> Result<Value, String> {
        self.execute(cmd)
    }

    fn execute(&mut self, cmd: Command) -> Result<Value, String> {
        match cmd {
            Command::Info => Ok(self.info()),
            Command::Capabilities => Ok(self.capabilities()),
            Command::Subscribe { events } => {
                // The server keeps per-connection subscriptions; here only
                // validate (a session without a server has one "connection").
                for e in &events {
                    if !protocol::OPTIONAL_EVENTS.contains(&e.as_str()) {
                        return Err(format!("unknown optional event '{e}'"));
                    }
                }
                Ok(json!({"events": events}))
            }
            Command::Pause => {
                let was_running = self.running;
                self.cancel_mode();
                self.running = false;
                if was_running {
                    self.last_stop = Some(StopReason::StepDone);
                }
                let state = self.state_json(&StopReason::StepDone);
                if was_running {
                    // Clients of asynchronous runs wait for `stopped`.
                    let mut ev = state.clone();
                    ev["event"] = json!("stopped");
                    self.events.push(ev);
                }
                Ok(state)
            }
            Command::Resume => {
                self.cancel_mode();
                Ok(self.start(RunMode::Free))
            }
            Command::StepInstr { count } => {
                let count = count.unwrap_or(1);
                if count == 0 || count > MAX_STEP_COUNT {
                    return Err(format!("count must be 1–{MAX_STEP_COUNT}"));
                }
                self.stop_for_sync();
                let mut reason = StopReason::StepDone;
                for _ in 0..count {
                    self.hooks.set_instr_step(true);
                    reason = self.run_until_stop();
                    self.hooks.set_instr_step(false);
                    if reason != StopReason::StepDone {
                        break;
                    }
                }
                Ok(self.state_json(&reason))
            }
            Command::StepOver => {
                self.stop_for_sync();
                let pc = self.main_pc().ok_or("main CPU has no PC")?;
                let ins = self.disasm_main(pc).ok_or("no debug target")?;
                if ins.flow != Flow::Call {
                    return Ok(self.start(RunMode::Instr));
                }
                let space = self.main_space()?;
                let ret = pc.wrapping_add(ins.bytes.len() as u32);
                let bp = self
                    .hooks
                    .add_breakpoint(BreakpointKind::Exec { space, addr: ret });
                let min_sp = self.main_regs().as_ref().and_then(stack_pointer);
                Ok(self.start(RunMode::StepOver { bp, min_sp }))
            }
            Command::StepOut => {
                self.stop_for_sync();
                let regs = self.main_regs().ok_or("no debug target")?;
                let sp0 = stack_pointer(&regs).ok_or("main CPU has no stack pointer")?;
                Ok(self.start(RunMode::StepOut { sp0, current: None }))
            }
            Command::RunTo {
                space,
                addr,
                symbol,
            } => {
                let (space, addr) = self.resolve(space, addr, symbol.as_deref())?;
                self.stop_for_sync();
                let bp = self
                    .hooks
                    .add_breakpoint(BreakpointKind::Exec { space, addr });
                Ok(self.start(RunMode::RunTo { bp }))
            }
            Command::StepCycle => {
                self.stop_for_sync();
                let reason = self.machine.step_cycle();
                Ok(self.state_json(&reason))
            }
            Command::StepLine => {
                if self.machine.beam().is_none() {
                    return Err("machine has no video beam".into());
                }
                self.stop_for_sync();
                self.hooks.set_line_step(true);
                let reason = self.run_until_stop();
                self.hooks.set_line_step(false);
                Ok(self.state_json(&reason))
            }
            Command::StepFrame => {
                self.stop_for_sync();
                let reason = self.run_filtered();
                Ok(self.state_json(&reason))
            }
            Command::Reset { kind } => {
                let kind = match kind.as_deref() {
                    None | Some("soft") => ResetKind::Soft,
                    Some("hard") => ResetKind::Hard,
                    Some(other) => return Err(format!("unknown reset kind '{other}'")),
                };
                self.stop_for_sync();
                self.machine.reset(kind);
                Ok(self.state_json(&StopReason::StepDone))
            }
            Command::Regs { device } => {
                let dev = self.device(device.as_deref())?;
                let target = self.machine.debug_target(dev).ok_or("no debug target")?;
                let regs: Vec<Value> = target
                    .regs()
                    .0
                    .iter()
                    .map(|r| json!({"name": r.name, "value": r.value, "bits": r.bits}))
                    .collect();
                Ok(json!({"device": dev.0, "regs": regs}))
            }
            Command::SetReg {
                device,
                name,
                value,
            } => {
                let dev = self.device(device.as_deref())?;
                let target = self.machine.debug_target(dev).ok_or("no debug target")?;
                if target.regs().get(&name).is_none() {
                    return Err(format!("unknown register '{name}'"));
                }
                target.set_reg(&name, value);
                Ok(json!({}))
            }
            Command::Peek { space, addr, len } => {
                if len > MAX_PEEK {
                    return Err(format!("len exceeds {MAX_PEEK}"));
                }
                let space = self.space(space)?;
                let owner = self.space_owner(space)?;
                let target = self.machine.debug_target(owner).ok_or("no debug target")?;
                let bytes = (0..len)
                    .map(|i| target.peek(space, addr.wrapping_add(i), Size::Byte))
                    .collect::<Option<Vec<u32>>>()
                    .ok_or("address space not readable")?;
                Ok(json!({"space": space.0, "addr": addr, "bytes": bytes}))
            }
            Command::Poke { space, addr, bytes } => {
                if bytes.len() > MAX_PEEK as usize {
                    return Err(format!("more than {MAX_PEEK} bytes"));
                }
                let space = self.space(space)?;
                let owner = self.space_owner(space)?;
                let target = self.machine.debug_target(owner).ok_or("no debug target")?;
                for (i, b) in bytes.iter().enumerate() {
                    let a = addr.wrapping_add(i as u32);
                    if !target.poke(space, a, Size::Byte, u32::from(*b)) {
                        return Err(format!("address {a:#x} is not writable"));
                    }
                }
                Ok(json!({}))
            }
            Command::Disasm {
                device,
                addr,
                count,
            } => {
                let count = count.unwrap_or(10).min(MAX_DISASM);
                let dev = self.device(device.as_deref())?;
                // Labels of the device's own view of memory.
                let space = self
                    .machine
                    .memory_spaces()
                    .iter()
                    .find(|s| s.device == dev)
                    .map(|s| s.id);
                let target = self.machine.debug_target(dev).ok_or("no debug target")?;
                let mut addr = match addr {
                    Some(a) => a,
                    None => target.regs().get("PC").ok_or("device has no PC")?,
                };
                let mut lines = Vec::new();
                for _ in 0..count {
                    let ins = target.disassemble(addr);
                    let mut line = json!({
                        "addr": ins.addr,
                        "bytes": ins.bytes,
                        "text": ins.to_string(),
                        "flow": format!("{:?}", ins.flow).to_ascii_lowercase(),
                    });
                    if let Some(space) = space {
                        if let Some(l) = self.symbols.name_at(space, ins.addr) {
                            line["label"] = json!(l);
                        }
                        if let Some(l) = ins.target.and_then(|t| self.symbols.name_at(space, t)) {
                            line["target_label"] = json!(l);
                        }
                    }
                    lines.push(line);
                    addr = addr.wrapping_add(ins.bytes.len().max(1) as u32);
                }
                Ok(json!({"device": dev.0, "lines": lines}))
            }
            Command::Beam => Ok(protocol::beam_json(self.machine.beam())),
            Command::Chips => {
                let chips: Vec<Value> = self
                    .machine
                    .chips()
                    .iter()
                    .map(|c| {
                        let regs: Vec<Value> = c
                            .regs
                            .iter()
                            .map(|r| {
                                json!({
                                    "name": r.name, "addr": r.addr,
                                    "value": r.value, "bits": r.bits,
                                })
                            })
                            .collect();
                        json!({"name": c.name, "regs": regs})
                    })
                    .collect();
                Ok(json!(chips))
            }
            Command::BreakAdd(spec) => self.break_add(spec),
            Command::BreakRemove { bp } => {
                let id = BpId(bp);
                if !self.bp_meta.contains_key(&id) {
                    return Err(format!("no breakpoint {bp}"));
                }
                self.bp_meta.remove(&id);
                self.hooks.remove_breakpoint(id);
                Ok(json!({}))
            }
            Command::BreakCondition { bp, condition } => {
                let condition = match condition {
                    Some(text) => {
                        let e = expr::parse(&text).map_err(|e| format!("condition: {e}"))?;
                        Some((text, e))
                    }
                    None => None,
                };
                let meta = self
                    .bp_meta
                    .get_mut(&BpId(bp))
                    .ok_or_else(|| format!("no breakpoint {bp}"))?;
                meta.condition = condition;
                Ok(json!({}))
            }
            Command::BreakEnable { bp, enabled } => {
                let id = BpId(bp);
                if !self.bp_meta.contains_key(&id) {
                    return Err(format!("no breakpoint {bp}"));
                }
                self.hooks.set_breakpoint_enabled(id, enabled);
                Ok(json!({}))
            }
            Command::BreakList => Ok(self.break_list()),
            Command::SaveState => Ok(json!({
                "data": protocol::base64_encode(&self.machine.save_state()),
            })),
            Command::LoadState { data } => {
                let bytes = protocol::base64_decode(&data)?;
                self.stop_for_sync();
                self.machine
                    .load_state(&bytes)
                    .map_err(|e| format!("savestate rejected: {e:?}"))?;
                Ok(self.state_json(&StopReason::StepDone))
            }
            Command::TraceStart {
                instructions,
                reads,
                writes,
                capacity,
            } => {
                let capacity = capacity.unwrap_or(65_536);
                if capacity == 0 || capacity > MAX_TRACE_CAPACITY {
                    return Err(format!("capacity must be 1–{MAX_TRACE_CAPACITY}"));
                }
                let ranges = |s: &Self, list: Vec<RangeSpec>| {
                    list.into_iter()
                        .map(|r| {
                            if r.len > emu_core::debug::MAX_RANGE {
                                return Err(format!(
                                    "range longer than {:#x}",
                                    emu_core::debug::MAX_RANGE
                                ));
                            }
                            Ok(TraceRange {
                                space: s.space(r.space)?,
                                addr: r.addr,
                                len: r.len,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()
                };
                let config = TraceConfig {
                    instructions,
                    reads: ranges(self, reads)?,
                    writes: ranges(self, writes)?,
                    capacity: capacity as usize,
                };
                self.hooks.set_trace(config);
                Ok(json!({}))
            }
            Command::SymbolsLoad {
                path,
                text,
                format,
                space,
            } => {
                let space = self.space(space)?;
                let text = match (path, text) {
                    (Some(path), None) => {
                        let meta = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?;
                        if meta.len() > symbols::MAX_FILE as u64 {
                            return Err(format!("{path}: larger than {} bytes", symbols::MAX_FILE));
                        }
                        let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
                        String::from_utf8_lossy(&bytes).into_owned()
                    }
                    (None, Some(text)) => text,
                    _ => return Err("give either path or text".into()),
                };
                let format = match format.as_deref() {
                    Some(f) => symbols::Format::from_name(f)
                        .ok_or_else(|| format!("unknown format '{f}'"))?,
                    None => symbols::Format::detect(&text).ok_or("unknown symbol file format")?,
                };
                let (syms, bad) = symbols::parse(&text, format);
                let count = syms.len();
                for (name, addr) in syms {
                    self.symbols.insert(space, name, addr);
                }
                Ok(json!({
                    "format": format.name(), "loaded": count, "skipped": bad,
                    "total": self.symbols.len(),
                }))
            }
            Command::SymbolsList => {
                let list: Vec<Value> = self
                    .symbols
                    .iter()
                    .map(
                        |(name, space, addr)| json!({"name": name, "space": space.0, "addr": addr}),
                    )
                    .collect();
                Ok(json!(list))
            }
            Command::SymbolsClear => {
                self.symbols.clear();
                Ok(json!({}))
            }
            Command::TraceStop => {
                self.hooks.set_trace(TraceConfig::default());
                Ok(json!({}))
            }
            Command::TraceRead => {
                let dropped = self.hooks.trace_dropped();
                let entries: Vec<Value> = self
                    .hooks
                    .drain_trace()
                    .iter()
                    .map(|e| {
                        let mut v = match e.event {
                            TraceEvent::Instr { device, pc } => {
                                json!({"kind": "instr", "device": device.0, "pc": pc})
                            }
                            TraceEvent::Read {
                                space, addr, value, ..
                            } => json!({
                                "kind": "read", "space": space.0, "addr": addr, "value": value,
                            }),
                            TraceEvent::Write {
                                space, addr, value, ..
                            } => json!({
                                "kind": "write", "space": space.0, "addr": addr, "value": value,
                            }),
                        };
                        v["beam"] = protocol::beam_json(Some(e.beam));
                        v
                    })
                    .collect();
                Ok(json!({"entries": entries, "dropped": dropped}))
            }
        }
    }

    /// Synchronous execution commands stop a running session first.
    fn stop_for_sync(&mut self) {
        self.cancel_mode();
        self.running = false;
        self.last_stop = Some(StopReason::StepDone);
    }

    /// Space and address from either `addr` or `symbol`.
    fn resolve(
        &self,
        space: Option<u16>,
        addr: Option<u32>,
        symbol: Option<&str>,
    ) -> Result<(MemSpaceId, u32), String> {
        match (addr, symbol) {
            (Some(addr), None) => Ok((self.space(space)?, addr)),
            (None, Some(name)) => {
                let (sym_space, addr) = self
                    .symbols
                    .lookup(name)
                    .ok_or_else(|| format!("unknown symbol '{name}'"))?;
                let space = match space {
                    Some(_) => self.space(space)?,
                    None => sym_space,
                };
                Ok((space, addr))
            }
            (Some(_), Some(_)) => Err("give either addr or symbol".into()),
            (None, None) => Err("addr missing".into()),
        }
    }

    fn break_add(&mut self, spec: BreakSpec) -> Result<Value, String> {
        let located = match (spec.addr, spec.symbol.as_deref()) {
            (None, None) => None,
            (addr, symbol) => Some(self.resolve(spec.space, addr, symbol)?),
        };
        let space = match located {
            Some((space, _)) => space,
            None => self.space(spec.space)?,
        };
        let device = self.space_owner(space)?;
        let addr = || located.map(|(_, a)| a).ok_or("addr missing");
        let len = spec.len.unwrap_or(1);
        if len == 0 || len > emu_core::debug::MAX_RANGE {
            return Err(format!("len must be 1–{:#x}", emu_core::debug::MAX_RANGE));
        }
        let kind = match spec.kind.as_deref().unwrap_or("exec") {
            "exec" => BreakpointKind::Exec {
                space,
                addr: addr()?,
            },
            "read" => BreakpointKind::Read {
                space,
                addr: addr()?,
                len,
            },
            "write" => BreakpointKind::Write {
                space,
                addr: addr()?,
                len,
            },
            "raster" => {
                if self.machine.beam().is_none() {
                    return Err("machine has no video beam".into());
                }
                BreakpointKind::Raster {
                    line: spec.line.ok_or("line missing")?,
                    cycle: spec.cycle,
                }
            }
            "irq" => BreakpointKind::Interrupt(InterruptKind::Irq),
            "nmi" => BreakpointKind::Interrupt(InterruptKind::Nmi),
            other => return Err(format!("unknown breakpoint kind '{other}'")),
        };
        let condition = match spec.condition {
            Some(text) => {
                let e = expr::parse(&text).map_err(|e| format!("condition: {e}"))?;
                Some((text, e))
            }
            None => None,
        };
        let id = self.hooks.add_breakpoint(kind);
        if spec.enabled == Some(false) {
            self.hooks.set_breakpoint_enabled(id, false);
        }
        self.bp_meta.insert(
            id,
            BpMeta {
                condition,
                ignore: spec.ignore.unwrap_or(0),
                hits: 0,
                space,
                device,
            },
        );
        Ok(json!({"bp": id.0}))
    }

    fn break_list(&self) -> Value {
        let list: Vec<Value> = self
            .hooks
            .breakpoints()
            .filter_map(|(id, bp)| {
                let meta = self.bp_meta.get(&id)?;
                let mut v = json!({
                    "bp": id.0, "enabled": bp.enabled, "hits": meta.hits, "ignore": meta.ignore,
                    "condition": meta.condition.as_ref().map(|(t, _)| t),
                });
                match bp.kind {
                    BreakpointKind::Exec { space, addr } => {
                        v["kind"] = json!("exec");
                        v["space"] = json!(space.0);
                        v["addr"] = json!(addr);
                    }
                    BreakpointKind::Read { space, addr, len }
                    | BreakpointKind::Write { space, addr, len } => {
                        let write = matches!(bp.kind, BreakpointKind::Write { .. });
                        v["kind"] = json!(if write { "write" } else { "read" });
                        v["space"] = json!(space.0);
                        v["addr"] = json!(addr);
                        v["len"] = json!(len);
                    }
                    BreakpointKind::Raster { line, cycle } => {
                        v["kind"] = json!("raster");
                        v["line"] = json!(line);
                        v["cycle"] = json!(cycle.unwrap_or(0));
                    }
                    BreakpointKind::Interrupt(InterruptKind::Irq) => v["kind"] = json!("irq"),
                    BreakpointKind::Interrupt(InterruptKind::Nmi) => v["kind"] = json!("nmi"),
                }
                Some(v)
            })
            .collect();
        json!(list)
    }

    /// Run frames until something other than the end of a frame stops us.
    fn run_until_stop(&mut self) -> StopReason {
        for _ in 0..MAX_STEP_FRAMES {
            match self.run_filtered() {
                StopReason::FrameDone => continue,
                reason => return reason,
            }
        }
        StopReason::FrameDone
    }

    fn device(&self, name: Option<&str>) -> Result<DeviceId, String> {
        let devices = self.machine.devices();
        match name {
            None => devices
                .first()
                .copied()
                .ok_or_else(|| "machine has no devices".into()),
            Some(n) => devices
                .into_iter()
                .find(|d| d.0 == n)
                .ok_or_else(|| format!("unknown device '{n}'")),
        }
    }

    fn space(&self, id: Option<u16>) -> Result<MemSpaceId, String> {
        let spaces = self.machine.memory_spaces();
        match id {
            None => spaces
                .first()
                .map(|s| s.id)
                .ok_or_else(|| "machine has no memory spaces".into()),
            Some(id) => spaces
                .iter()
                .find(|s| s.id.0 == id)
                .map(|s| s.id)
                .ok_or_else(|| format!("unknown memory space {id}")),
        }
    }

    fn space_owner(&self, space: MemSpaceId) -> Result<DeviceId, String> {
        self.machine
            .memory_spaces()
            .iter()
            .find(|s| s.id == space)
            .map(|s| s.device)
            .ok_or_else(|| format!("unknown memory space {}", space.0))
    }

    /// The main CPU's own view of memory: its first memory space.
    fn main_space(&self) -> Result<MemSpaceId, String> {
        let main = self.device(None)?;
        self.machine
            .memory_spaces()
            .iter()
            .find(|s| s.device == main)
            .map(|s| s.id)
            .ok_or_else(|| "main CPU has no memory space".into())
    }

    fn main_regs(&mut self) -> Option<RegisterSet> {
        let dev = *self.machine.devices().first()?;
        Some(self.machine.debug_target(dev)?.regs())
    }

    fn main_pc(&mut self) -> Option<u32> {
        self.main_regs()?.get("PC")
    }

    fn disasm_main(&mut self, addr: u32) -> Option<emu_core::Instr> {
        let dev = *self.machine.devices().first()?;
        Some(self.machine.debug_target(dev)?.disassemble(addr))
    }

    fn state_json(&mut self, reason: &StopReason) -> Value {
        let mut reason_json = protocol::stop_json(reason);
        if let StopReason::Breakpoint(id) | StopReason::Watchpoint { id, .. } = reason {
            if let Some(meta) = self.bp_meta.get(id) {
                reason_json["hits"] = json!(meta.hits);
            }
        }
        json!({
            "reason": reason_json,
            "pc": self.main_pc(),
            "beam": protocol::beam_json(self.machine.beam()),
        })
    }

    fn info(&self) -> Value {
        let spaces: Vec<Value> = self
            .machine
            .memory_spaces()
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.0, "name": s.name, "device": s.device.0,
                    "addr_bits": s.addr_bits, "big_endian": s.big_endian,
                })
            })
            .collect();
        let devices: Vec<&str> = self.machine.devices().iter().map(|d| d.0).collect();
        json!({
            "machine": self.machine_id,
            "protocol_version": protocol::PROTOCOL_VERSION,
            "devices": devices,
            "memory_spaces": spaces,
            "running": self.running,
            "beam": protocol::beam_json(self.machine.beam()),
        })
    }

    fn capabilities(&self) -> Value {
        let mut kinds: Vec<&str> = protocol::BREAK_KINDS.to_vec();
        if self.machine.beam().is_none() {
            kinds.retain(|k| *k != "raster");
        }
        json!({
            "protocol": protocol::PROTOCOL_NAME,
            "version": protocol::PROTOCOL_VERSION,
            "commands": protocol::COMMANDS,
            "breakpoint_kinds": kinds,
            "events": protocol::EVENTS,
            "optional_events": protocol::OPTIONAL_EVENTS,
            "beam": self.machine.beam().is_some(),
            // Machine-specific extensions (none yet); generic clients ignore them.
            "extensions": [],
        })
    }
}

#[cfg(test)]
mod tests;
