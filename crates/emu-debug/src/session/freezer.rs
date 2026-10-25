//! Memory freezer, cheat search and cheat files (protocol commands
//! `freeze_*`, `search_*`, `cheats_*`).
//!
//! Freezes live in [`emu_core::DebugHooks`] (same bitmaps as watchpoints,
//! zero cost when none is active). The search narrows a snapshot of
//! candidate addresses taken with peeks between frames; it never halts or
//! slows emulation beyond the copy.

use serde_json::{json, Value};

use emu_cheats::search::{Op, Search};
use emu_core::{FreezeId, FreezeMode, MemSpaceId, Size};

use emu_cheats::CheatFile;

use super::Session;

/// Most addresses one search may cover.
pub const MAX_SEARCH: u32 = emu_cheats::search::MAX_SEARCH;
/// Most results one `search_results` returns.
pub const MAX_RESULTS: u32 = 10_000;

fn size_from(bytes: Option<u8>) -> Result<Size, String> {
    emu_cheats::size_from(bytes.unwrap_or(1))
}

fn mode_from(name: Option<&str>) -> Result<FreezeMode, String> {
    emu_cheats::mode_from(name.unwrap_or("block_writes"))
}

fn mode_name(mode: FreezeMode) -> &'static str {
    emu_cheats::mode_name(mode)
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
        let owner = self.space_owner(space)?;
        let target = self.machine.debug_target(owner).ok_or("no debug target")?;
        let search = Search::start(space, size, start, len, |a| target.peek(space, a, size))?;
        let n = search.candidates.len();
        self.search = Some(search);
        Ok(json!({"candidates": n}))
    }

    pub(super) fn search_next(&mut self, op: &str, value: Option<u32>) -> Result<Value, String> {
        let op = Op::parse(op, value)?;
        let mut search = self.search.take().ok_or("no search started")?;
        let result = self.space_owner(search.space).and_then(|owner| {
            let target = self
                .machine
                .debug_target(owner)
                .ok_or_else(|| "no debug target".to_string())?;
            let (space, size) = (search.space, search.size);
            Ok(search.next(op, |a| target.peek(space, a, size)))
        });
        self.search = Some(search);
        Ok(json!({"candidates": result?}))
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
        let file = emu_cheats::from_hooks(self.machine.as_ref(), &self.hooks);
        let text = file.to_toml();
        if let Some(path) = path {
            std::fs::write(path, &text).map_err(|e| format!("{path}: {e}"))?;
        }
        Ok(json!({"text": text, "freezes": file.freeze.len()}))
    }

    /// Add the cheats of a file (all or nothing). `format`: `toml`
    /// (default), `pokes`, `vice` or `codes` (see `emu_cheats::import`);
    /// imported addresses refer to the main CPU's space.
    pub(super) fn cheats_load(
        &mut self,
        path: Option<&str>,
        text: Option<String>,
        format: Option<&str>,
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
        let space = self
            .machine
            .memory_spaces()
            .first()
            .map_or("cpu", |s| s.name)
            .to_string();
        let file = emu_cheats::import::import(format.unwrap_or("toml"), &text, &space)?;
        self.apply_cheats(&file)
    }

    /// Apply a cheat file a frontend loaded (all or nothing).
    pub fn apply_cheat_file(&mut self, file: &CheatFile) -> Result<(), String> {
        self.apply_cheats(file).map(|_| ())
    }

    pub(super) fn apply_cheats(&mut self, file: &CheatFile) -> Result<Value, String> {
        let applied = emu_cheats::apply(file, self.machine.as_mut(), &mut self.hooks)?;
        let ids: Vec<u32> = applied.freezes.iter().map(|f| f.0).collect();
        Ok(json!({"freezes": ids, "patches": applied.patches, "game": file.game}))
    }

    /// Folder of the per-game cheat database (`cheats/<machine>/<crc>.toml`).
    pub fn set_cheats_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.cheats_dir = dir;
    }

    /// Load the database's cheats for an image just inserted, if any; the
    /// game name. Frontends call this for images inserted at start.
    pub fn cheats_for_image(&mut self, data: &[u8]) -> Result<Option<String>, String> {
        let Some(dir) = self.cheats_dir.clone() else {
            return Ok(None);
        };
        let machine = self.machine_id.clone();
        match emu_cheats::lookup(&dir, &machine, data)? {
            Some(file) => {
                let game = file
                    .game
                    .clone()
                    .unwrap_or_else(|| emu_cheats::media_key(data));
                self.apply_cheats(&file)?;
                self.events
                    .push(json!({"event": "cheats_loaded", "game": game}));
                Ok(Some(game))
            }
            None => Ok(None),
        }
    }
}
