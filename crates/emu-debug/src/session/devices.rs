//! Remote control of ports, media and input (protocol commands from
//! `list_ports` to `replay_input`).

use serde_json::{json, Value};

use emu_core::input::TimedInput;
use emu_core::media::SlotId;
use emu_core::port::PortId;
use emu_core::Cycle;

use super::Session;
use crate::host::{self, Replay};

/// A recording in progress.
#[derive(Clone, Debug)]
pub(super) struct Recording {
    pub(super) state: Vec<u8>,
    pub(super) freezes: Vec<u8>,
    pub(super) events: Vec<TimedInput>,
}

impl Session {
    fn host(&self) -> Result<&dyn host::HostServices, String> {
        self.host
            .as_deref()
            .ok_or_else(|| "this frontend offers no device or media services".to_string())
    }

    pub(super) fn list_ports(&self) -> Value {
        let ports: Vec<Value> = self
            .machine
            .ports()
            .iter()
            .map(|p| {
                let devices = self
                    .host
                    .as_deref()
                    .map(|h| h.device_names(p.kind))
                    .unwrap_or_default();
                json!({
                    "id": p.id.0, "name": p.name, "kind": format!("{:?}", p.kind),
                    "connected": p.connected, "devices": devices,
                })
            })
            .collect();
        json!(ports)
    }

    pub(super) fn connect_device(&mut self, port: u8, device: &str) -> Result<Value, String> {
        let info = self
            .machine
            .ports()
            .into_iter()
            .find(|p| p.id.0 == port)
            .ok_or_else(|| format!("no port {port}"))?;
        let dev = self
            .host()?
            .create_device(device, info.kind)
            .ok_or_else(|| format!("no device '{device}' for a {:?} port", info.kind))?;
        self.machine
            .connect(PortId(port), dev)
            .map_err(|e| e.to_string())?;
        self.events.push(json!({
            "event": "device_connected", "port": port, "device": device,
        }));
        Ok(json!({}))
    }

    pub(super) fn disconnect_device(&mut self, port: u8) -> Result<Value, String> {
        if !self.machine.ports().iter().any(|p| p.id.0 == port) {
            return Err(format!("no port {port}"));
        }
        let was = self.machine.disconnect(PortId(port)).is_some();
        if was {
            self.events.push(json!({
                "event": "device_connected", "port": port, "device": null,
            }));
        }
        Ok(json!({"disconnected": was}))
    }

    pub(super) fn list_media_slots(&self) -> Value {
        let slots: Vec<Value> = self
            .machine
            .media_slots()
            .iter()
            .map(|s| {
                json!({
                    "id": s.id.0, "name": s.name, "kind": format!("{:?}", s.kind),
                    "inserted": s.inserted,
                })
            })
            .collect();
        json!(slots)
    }

    fn slot_name(&self, slot: u8) -> Result<&'static str, String> {
        self.machine
            .media_slots()
            .into_iter()
            .find(|s| s.id.0 == slot)
            .map(|s| s.name)
            .ok_or_else(|| format!("no media slot {slot}"))
    }

    pub(super) fn insert_media(&mut self, slot: u8, path: &str) -> Result<Value, String> {
        let name = self.slot_name(slot)?;
        let media = self.host()?.open_media(path)?;
        let label = media.name.clone();
        self.machine
            .media_slot(SlotId(slot))
            .ok_or_else(|| format!("no media slot {slot}"))?
            .insert(media)
            .map_err(|e| e.to_string())?;
        self.events.push(json!({
            "event": "media_changed", "slot": slot, "name": name, "media": label,
        }));
        Ok(json!({"media": label}))
    }

    pub(super) fn eject_media(&mut self, slot: u8) -> Result<Value, String> {
        let name = self.slot_name(slot)?;
        let ejected = self
            .machine
            .media_slot(SlotId(slot))
            .ok_or_else(|| format!("no media slot {slot}"))?
            .eject()
            .map(|m| m.name);
        self.events.push(json!({
            "event": "media_changed", "slot": slot, "name": name, "media": null,
        }));
        Ok(json!({"ejected": ejected}))
    }

    pub(super) fn flush_media(&mut self, slot: u8) -> Result<Value, String> {
        self.slot_name(slot)?;
        self.machine
            .media_slot(SlotId(slot))
            .ok_or_else(|| format!("no media slot {slot}"))?
            .flush()
            .map_err(|e| e.to_string())?;
        Ok(json!({}))
    }

    /// The drive mode is chosen when the machine is built (option
    /// `drive8` = "1541" or "virtual"); this confirms the current one. A
    /// true drive is a debug device named like its slot ("drive 8" →
    /// "drive8"); a virtual drive is not.
    pub(super) fn set_drive_mode(&mut self, slot: u8, mode: &str) -> Result<Value, String> {
        let name = self.slot_name(slot)?;
        let is_drive = self
            .machine
            .media_slots()
            .iter()
            .any(|s| s.id.0 == slot && s.kind == emu_core::media::MediaKind::Disk);
        if !is_drive {
            return Err(format!("{name} is not a drive"));
        }
        let device: String = name.chars().filter(|c| !c.is_whitespace()).collect();
        let current = if self.machine.devices().iter().any(|d| d.0 == device) {
            "true"
        } else {
            "virtual"
        };
        match mode {
            "true" | "virtual" if mode == current => Ok(json!({"mode": current})),
            "true" | "virtual" => Err(format!(
                "{name} is in {current} mode; the mode is set when the machine starts (option {device}=1541 or virtual)"
            )),
            other => Err(format!("unknown drive mode '{other}'")),
        }
    }

    /// `input`: events at `now + delay` (cycles), in order.
    pub(super) fn input_command(&mut self, events: &[Value]) -> Result<Value, String> {
        let now = self.machine.now();
        let mut parsed = Vec::with_capacity(events.len());
        for e in events {
            let delay = e["delay"].as_u64().unwrap_or(0);
            parsed.push(TimedInput {
                at: Cycle(now.0.saturating_add(delay)),
                target: host::parse_target(&e["target"])?,
                event: host::parse_event(&e["event"])?,
            });
        }
        let n = parsed.len();
        for ev in parsed {
            self.input(ev);
        }
        Ok(json!({"queued": n, "now": now.0}))
    }

    pub(super) fn record_start(&mut self) -> Value {
        self.recording = Some(Recording {
            state: self.machine.save_state(),
            freezes: self.freeze_state(),
            events: Vec::new(),
        });
        json!({"now": self.machine.now().0})
    }

    pub(super) fn record_stop(&mut self) -> Result<Value, String> {
        let rec = self.recording.take().ok_or("not recording")?;
        let replay = Replay {
            machine: self.machine_id.clone(),
            state: rec.state,
            freezes: rec.freezes,
            events: rec.events,
            end: self.machine.now(),
        };
        Ok(replay.to_json())
    }

    /// Restore the recording's start state and queue its events. Runs
    /// afterwards reproduce the recorded one up to `end`.
    pub(super) fn replay_input(&mut self, replay: &Value) -> Result<Value, String> {
        let replay = Replay::from_json(replay)?;
        if replay.machine != self.machine_id {
            return Err(format!(
                "replay is for '{}', this is '{}'",
                replay.machine, self.machine_id
            ));
        }
        self.stop_for_sync();
        self.machine
            .load_state(&replay.state)
            .map_err(|e| format!("replay state rejected: {e:?}"))?;
        if !replay.freezes.is_empty() {
            self.load_freeze_state(&replay.freezes)?;
        }
        for ev in &replay.events {
            self.machine.input(*ev);
        }
        Ok(json!({"events": replay.events.len(), "now": self.machine.now().0, "end": replay.end.0}))
    }
}
