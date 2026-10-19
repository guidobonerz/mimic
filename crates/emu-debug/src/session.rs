//! One debugged machine: executes protocol commands. No I/O here; the
//! server feeds requests in and ships responses and events out, and tests
//! drive it directly.

use std::time::Duration;

use serde_json::{json, Value};

use emu_core::{BreakpointKind, DebugHooks, DeviceId, Machine, MemSpaceId, Size, StopReason};

use crate::protocol::{self, Command, Request, MAX_DISASM, MAX_PEEK};

/// Frames a step may run before giving up (e.g. `step_instr` on a CPU held
/// off the bus). Bounded so a command can never hang the session.
const MAX_STEP_FRAMES: u32 = 500;

pub struct Session {
    machine: Box<dyn Machine>,
    machine_id: String,
    hooks: DebugHooks,
    running: bool,
}

impl Session {
    pub fn new(machine: Box<dyn Machine>, machine_id: impl Into<String>) -> Self {
        Self {
            machine,
            machine_id: machine_id.into(),
            hooks: DebugHooks::new(),
            running: false,
        }
    }

    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn set_running(&mut self, running: bool) {
        self.running = running;
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

    /// While running: emulate one frame. Returns a `stopped` event if the
    /// frame ended at a breakpoint, watchpoint, jam or fault.
    pub fn run_slice(&mut self) -> Option<Value> {
        if !self.running {
            return None;
        }
        match self.machine.run_frame(&mut self.hooks) {
            StopReason::FrameDone => None,
            reason => {
                self.running = false;
                let mut ev = self.state_json(&reason);
                ev["event"] = json!("stopped");
                Some(ev)
            }
        }
    }

    pub fn handle(&mut self, req: Request) -> Value {
        match self.execute(req.cmd) {
            Ok(result) => protocol::ok(req.id, result),
            Err(msg) => protocol::err(Some(req.id), msg),
        }
    }

    fn execute(&mut self, cmd: Command) -> Result<Value, String> {
        match cmd {
            Command::Info => Ok(self.info()),
            Command::Pause => {
                self.running = false;
                Ok(self.state_json(&StopReason::StepDone))
            }
            Command::Resume => {
                self.running = true;
                Ok(json!({}))
            }
            Command::StepCycle => {
                self.running = false;
                let reason = self.machine.step_cycle();
                Ok(self.state_json(&reason))
            }
            Command::StepInstr => {
                self.running = false;
                self.hooks.set_instr_step(true);
                let reason = self.run_until_stop();
                self.hooks.set_instr_step(false);
                Ok(self.state_json(&reason))
            }
            Command::StepLine => {
                if self.machine.beam().is_none() {
                    return Err("machine has no video beam".into());
                }
                self.running = false;
                self.hooks.set_line_step(true);
                let reason = self.run_until_stop();
                self.hooks.set_line_step(false);
                Ok(self.state_json(&reason))
            }
            Command::StepFrame => {
                self.running = false;
                let reason = self.machine.run_frame(&mut self.hooks);
                Ok(self.state_json(&reason))
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
            Command::Disasm {
                device,
                addr,
                count,
            } => {
                let count = count.unwrap_or(10).min(MAX_DISASM);
                let dev = self.device(device.as_deref())?;
                let target = self.machine.debug_target(dev).ok_or("no debug target")?;
                let mut addr = match addr {
                    Some(a) => a,
                    None => target.regs().get("PC").ok_or("device has no PC")?,
                };
                let mut lines = Vec::new();
                for _ in 0..count {
                    let ins = target.disassemble(addr);
                    lines.push(json!({
                        "addr": ins.addr,
                        "bytes": ins.bytes,
                        "text": ins.to_string(),
                    }));
                    addr = addr.wrapping_add(ins.bytes.len().max(1) as u32);
                }
                Ok(json!({"device": dev.0, "lines": lines}))
            }
            Command::Beam => Ok(protocol::beam_json(self.machine.beam())),
            Command::BreakAdd { space, addr } => {
                let space = self.space(space)?;
                let id = self
                    .hooks
                    .add_breakpoint(BreakpointKind::Exec { space, addr });
                Ok(json!({"bp": id.0}))
            }
            Command::BreakRemove { bp } => self
                .hooks
                .remove_breakpoint(emu_core::BpId(bp))
                .map(|_| json!({}))
                .ok_or_else(|| format!("no breakpoint {bp}")),
            Command::BreakList => {
                let list: Vec<Value> = self
                    .hooks
                    .breakpoints()
                    .map(|(id, bp)| {
                        let mut v = json!({
                            "bp": id.0, "enabled": bp.enabled, "hits": bp.hits,
                        });
                        match bp.kind {
                            BreakpointKind::Exec { space, addr } => {
                                v["kind"] = json!("exec");
                                v["space"] = json!(space.0);
                                v["addr"] = json!(addr);
                            }
                            other => v["kind"] = json!(format!("{other:?}")),
                        }
                        v
                    })
                    .collect();
                Ok(json!(list))
            }
        }
    }

    /// Run frames until something other than the end of a frame stops us.
    fn run_until_stop(&mut self) -> StopReason {
        for _ in 0..MAX_STEP_FRAMES {
            match self.machine.run_frame(&mut self.hooks) {
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

    fn main_pc(&mut self) -> Option<u32> {
        let dev = *self.machine.devices().first()?;
        self.machine.debug_target(dev)?.regs().get("PC")
    }

    fn state_json(&mut self, reason: &StopReason) -> Value {
        json!({
            "reason": protocol::stop_json(reason),
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
}

#[cfg(test)]
mod tests;
