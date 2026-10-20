//! Memory freezer, cheat search and cheat files (protocol commands
//! `freeze_*`, `search_*`, `cheats_*`).
//!
//! Freezes live in [`emu_core::DebugHooks`] (same bitmaps as watchpoints,
//! zero cost when none is active). The search narrows a snapshot of
//! candidate addresses taken with peeks between frames; it never halts or
//! slows emulation beyond the copy.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use emu_core::{FreezeId, FreezeMode, MemSpaceId, Size};

use super::Session;

/// Most addresses one search may cover.
pub const MAX_SEARCH: u32 = 1 << 24;
/// Most results one `search_results` returns.
pub const MAX_RESULTS: u32 = 10_000;

/// A cheat search in progress: candidate addresses and their last values.
#[derive(Clone, Debug)]
pub(super) struct Search {
    space: MemSpaceId,
    size: Size,
    candidates: Vec<(u32, u32)>,
}

fn size_from(bytes: Option<u8>) -> Result<Size, String> {
    match bytes.unwrap_or(1) {
        1 => Ok(Size::Byte),
        2 => Ok(Size::Word),
        4 => Ok(Size::Long),
        n => Err(format!("size must be 1, 2 or 4, not {n}")),
    }
}

fn mode_from(name: Option<&str>) -> Result<FreezeMode, String> {
    match name.unwrap_or("block_writes") {
        "block_writes" => Ok(FreezeMode::BlockWrites),
        "reapply_per_frame" => Ok(FreezeMode::ReapplyPerFrame),
        "reapply_on_read" => Ok(FreezeMode::ReapplyOnRead),
        other => Err(format!("unknown freeze mode '{other}'")),
    }
}

fn mode_name(mode: FreezeMode) -> &'static str {
    match mode {
        FreezeMode::BlockWrites => "block_writes",
        FreezeMode::ReapplyPerFrame => "reapply_per_frame",
        FreezeMode::ReapplyOnRead => "reapply_on_read",
    }
}

/// One entry of a cheat file (TOML `[[freeze]]` table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatFreeze {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Memory space by name (`cpu`, `ram` …), so files work across runs.
    pub space: String,
    pub addr: u32,
    #[serde(default = "one")]
    pub size: u8,
    pub value: u32,
    #[serde(default = "block_writes")]
    pub mode: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn one() -> u8 {
    1
}

fn block_writes() -> String {
    "block_writes".into()
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatFile {
    #[serde(default)]
    pub freeze: Vec<CheatFreeze>,
}

/// `freeze_add` arguments.
pub(super) struct FreezeSpec {
    pub space: Option<u16>,
    pub addr: u32,
    pub size: Option<u8>,
    pub value: Option<u32>,
    pub mode: Option<String>,
    pub label: Option<String>,
    pub enabled: Option<bool>,
}

impl Session {
    /// Value of `size` bytes at `addr` in the space's byte order.
    fn peek_value(&mut self, space: MemSpaceId, addr: u32, size: Size) -> Option<u32> {
        let owner = self.space_owner(space).ok()?;
        self.machine.debug_target(owner)?.peek(space, addr, size)
    }

    pub(super) fn freeze_add(&mut self, spec: FreezeSpec) -> Result<Value, String> {
        let space = self.space(spec.space)?;
        let size = size_from(spec.size)?;
        let mode = mode_from(spec.mode.as_deref())?;
        let value = match spec.value {
            Some(v) => v & size.mask(),
            // Freeze what is there now.
            None => self
                .peek_value(space, spec.addr, size)
                .ok_or("address not readable")?,
        };
        if spec.label.as_ref().is_some_and(|l| l.len() > 255) {
            return Err("label longer than 255".into());
        }
        let freeze = emu_core::Freeze {
            mode,
            enabled: spec.enabled.unwrap_or(true),
            label: spec.label,
            ..emu_core::Freeze::new(space, spec.addr, size, value)
        };
        // Put the value in place now, so everything (video fetches too)
        // sees it from the start; read-only cells simply keep theirs.
        if freeze.enabled && mode != FreezeMode::ReapplyOnRead {
            if let Ok(owner) = self.space_owner(space) {
                if let Some(t) = self.machine.debug_target(owner) {
                    let _ = t.poke(space, spec.addr, size, value);
                }
            }
        }
        let id = self.hooks.add_freeze(freeze);
        Ok(json!({"freeze": id.0, "value": value}))
    }

    pub(super) fn freeze_remove(&mut self, id: u32) -> Result<Value, String> {
        self.hooks
            .remove_freeze(FreezeId(id))
            .map(|_| json!({}))
            .ok_or_else(|| format!("no freeze {id}"))
    }

    pub(super) fn freeze_toggle(
        &mut self,
        id: u32,
        enabled: Option<bool>,
    ) -> Result<Value, String> {
        let current = self
            .hooks
            .freezes()
            .find(|(f, _)| f.0 == id)
            .map(|(_, f)| f.enabled)
            .ok_or_else(|| format!("no freeze {id}"))?;
        let enabled = enabled.unwrap_or(!current);
        self.hooks.set_freeze_enabled(FreezeId(id), enabled);
        Ok(json!({"enabled": enabled}))
    }

    pub(super) fn freeze_list(&self) -> Value {
        let list: Vec<Value> = self
            .hooks
            .freezes()
            .map(|(id, f)| {
                json!({
                    "freeze": id.0, "space": f.space.0, "addr": f.addr, "size": f.size.bytes(),
                    "value": f.value, "mode": mode_name(f.mode), "enabled": f.enabled,
                    "label": f.label,
                })
            })
            .collect();
        json!(list)
    }

    /// Offsets in `addr..addr + len` covered by an enabled freeze.
    pub(super) fn frozen_offsets(&self, space: MemSpaceId, addr: u32, len: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for (_, f) in self.hooks.freezes() {
            if !f.enabled || f.space != space {
                continue;
            }
            for i in 0..f.size.bytes() {
                let a = f.addr.wrapping_add(i);
                let off = a.wrapping_sub(addr);
                if off < len {
                    out.push(off);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub(super) fn search_start(
        &mut self,
        space: Option<u16>,
        size: Option<u8>,
        addr: Option<u32>,
        len: Option<u32>,
    ) -> Result<Value, String> {
        let space = self.space(space)?;
        let size = size_from(size)?;
        let bits = self
            .machine
            .memory_spaces()
            .iter()
            .find(|s| s.id == space)
            .map_or(16, |s| s.addr_bits);
        let start = addr.unwrap_or(0);
        let len = len.unwrap_or_else(|| (1u64 << bits).min(u64::from(MAX_SEARCH)) as u32);
        if len == 0 || len > MAX_SEARCH {
            return Err(format!("len must be 1–{MAX_SEARCH:#x}"));
        }
        let owner = self.space_owner(space)?;
        let target = self.machine.debug_target(owner).ok_or("no debug target")?;
        let step = size.bytes();
        let mut candidates = Vec::new();
        let mut off = 0;
        while off + step <= len {
            let a = start.wrapping_add(off);
            if let Some(v) = target.peek(space, a, size) {
                candidates.push((a, v));
            }
            off += step;
        }
        let n = candidates.len();
        self.search = Some(Search {
            space,
            size,
            candidates,
        });
        Ok(json!({"candidates": n}))
    }

    pub(super) fn search_next(&mut self, op: &str, value: Option<u32>) -> Result<Value, String> {
        let mask = self.search.as_ref().ok_or("no search started")?.size.mask();
        let need = || value.ok_or_else(|| format!("'{op}' needs a value"));
        let keep: Box<dyn Fn(u32, u32) -> bool> = match op {
            "eq" => {
                let v = need()?;
                Box::new(move |_, now| now == v)
            }
            "ne" => {
                let v = need()?;
                Box::new(move |_, now| now != v)
            }
            "lt" => {
                let v = need()?;
                Box::new(move |_, now| now < v)
            }
            "gt" => {
                let v = need()?;
                Box::new(move |_, now| now > v)
            }
            "increased" => Box::new(|old, now| now > old),
            "decreased" => Box::new(|old, now| now < old),
            "unchanged" => Box::new(|old, now| now == old),
            "changed" => Box::new(|old, now| now != old),
            "increased_by" => {
                let d = need()?;
                Box::new(move |old, now| now == old.wrapping_add(d) & mask)
            }
            "decreased_by" => {
                let d = need()?;
                Box::new(move |old, now| now == old.wrapping_sub(d) & mask)
            }
            other => return Err(format!("unknown search operation '{other}'")),
        };
        let mut search = self.search.take().ok_or("no search started")?;
        let owner = self.space_owner(search.space)?;
        let target = self.machine.debug_target(owner).ok_or("no debug target")?;
        let (space, size) = (search.space, search.size);
        search.candidates.retain_mut(|(addr, last)| {
            let Some(now) = target.peek(space, *addr, size) else {
                return false;
            };
            let kept = keep(*last, now);
            *last = now;
            kept
        });
        let n = search.candidates.len();
        self.search = Some(search);
        Ok(json!({"candidates": n}))
    }

    pub(super) fn search_results(&mut self, max: Option<u32>) -> Result<Value, String> {
        let search = self.search.as_ref().ok_or("no search started")?;
        let max = max.unwrap_or(100).min(MAX_RESULTS) as usize;
        let (space, size) = (search.space, search.size);
        let shown: Vec<(u32, u32)> = search.candidates.iter().take(max).copied().collect();
        let total = search.candidates.len();
        let owner = self.space_owner(space)?;
        let target = self.machine.debug_target(owner).ok_or("no debug target")?;
        let results: Vec<Value> = shown
            .iter()
            .map(|&(addr, last)| {
                json!({"addr": addr, "value": target.peek(space, addr, size), "last": last})
            })
            .collect();
        Ok(json!({"space": space.0, "size": size.bytes(), "total": total, "results": results}))
    }

    pub(super) fn cheats_save(&self, path: Option<&str>) -> Result<Value, String> {
        let spaces = self.machine.memory_spaces();
        let file = CheatFile {
            freeze: self
                .hooks
                .freezes()
                .filter_map(|(_, f)| {
                    let space = spaces.iter().find(|s| s.id == f.space)?.name.to_string();
                    Some(CheatFreeze {
                        label: f.label.clone(),
                        space,
                        addr: f.addr,
                        size: f.size.bytes() as u8,
                        value: f.value,
                        mode: mode_name(f.mode).into(),
                        enabled: f.enabled,
                    })
                })
                .collect(),
        };
        let text = toml::to_string(&file).map_err(|e| e.to_string())?;
        if let Some(path) = path {
            std::fs::write(path, &text).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(json!({"text": text, "freezes": file.freeze.len()}))
    }

    /// Add the freezes of a cheat file (all or nothing).
    pub(super) fn cheats_load(
        &mut self,
        path: Option<&str>,
        text: Option<String>,
    ) -> Result<Value, String> {
        let text = match (path, text) {
            (Some(path), None) => {
                let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
                if meta.len() > 1 << 20 {
                    return Err(format!("{path}: larger than 1 MB"));
                }
                std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?
            }
            (None, Some(text)) => text,
            _ => return Err("give either path or text".into()),
        };
        let file: CheatFile = toml::from_str(&text).map_err(|e| format!("cheat file: {e}"))?;
        let spaces = self.machine.memory_spaces();
        let mut specs = Vec::with_capacity(file.freeze.len());
        for c in &file.freeze {
            let space = spaces
                .iter()
                .find(|s| s.name == c.space)
                .ok_or_else(|| format!("unknown memory space '{}'", c.space))?;
            size_from(Some(c.size))?;
            mode_from(Some(&c.mode))?;
            specs.push(FreezeSpec {
                space: Some(space.id.0),
                addr: c.addr,
                size: Some(c.size),
                value: Some(c.value),
                mode: Some(c.mode.clone()),
                label: c.label.clone(),
                enabled: Some(c.enabled),
            });
        }
        let mut ids = Vec::new();
        for spec in specs {
            ids.push(self.freeze_add(spec)?["freeze"].clone());
        }
        Ok(json!({"freezes": ids}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheat_files_round_trip_through_toml() {
        let file = CheatFile {
            freeze: vec![
                CheatFreeze {
                    label: Some("Lives".into()),
                    space: "cpu".into(),
                    addr: 0x0810,
                    size: 1,
                    value: 9,
                    mode: "block_writes".into(),
                    enabled: true,
                },
                CheatFreeze {
                    label: None,
                    space: "ram".into(),
                    addr: 0xc000,
                    size: 2,
                    value: 0x1234,
                    mode: "reapply_per_frame".into(),
                    enabled: false,
                },
            ],
        };
        let text = toml::to_string(&file).unwrap();
        assert!(text.contains("[[freeze]]"));
        assert_eq!(toml::from_str::<CheatFile>(&text).unwrap(), file);
        let minimal: CheatFile =
            toml::from_str("[[freeze]]\nspace = \"cpu\"\naddr = 2048\nvalue = 3\n").unwrap();
        assert_eq!(minimal.freeze[0].size, 1);
        assert_eq!(minimal.freeze[0].mode, "block_writes");
        assert!(minimal.freeze[0].enabled);
    }
}
