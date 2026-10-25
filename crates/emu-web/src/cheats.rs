//! Cheats in the web frontend: the running machine's freezes, imports,
//! the cheat database from uploaded files (`cheats/<machine>/<crc>.toml`,
//! matched by the inserted images' CRC-32) and the cheat search.

use serde_json::{json, Value};

use emu_cheats::search::{Op, Search};
use emu_core::{FreezeId, Size};

use crate::app::WebEmu;

impl WebEmu {
    fn main_space(&self) -> Result<(emu_core::MemSpaceId, String, emu_core::DeviceId), String> {
        let r = self.running.as_ref().ok_or("no machine running")?;
        let s = r
            .machine
            .memory_spaces()
            .into_iter()
            .next()
            .ok_or("the machine has no memory space")?;
        Ok((s.id, s.name.to_string(), s.device))
    }

    /// The freezes of the running machine.
    pub fn cheats(&self) -> Value {
        let Some(r) = &self.running else {
            return json!([]);
        };
        let list: Vec<Value> = r
            .hooks
            .freezes()
            .map(|(id, f)| {
                json!({"id": id.0, "addr": f.addr, "size": f.size.bytes(), "value": f.value,
                       "mode": emu_cheats::mode_name(f.mode), "enabled": f.enabled,
                       "label": f.label})
            })
            .collect();
        json!(list)
    }

    /// Import cheats (`toml`, `pokes`, `vice`, `codes`) into the machine.
    pub fn cheats_import(&mut self, format: &str, text: &str) -> Result<Value, String> {
        let (_, space, _) = self.main_space()?;
        let file = emu_cheats::import::import(format, text, &space)?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        let applied = emu_cheats::apply(&file, r.machine.as_mut(), &mut r.hooks)?;
        Ok(json!({"freezes": applied.freezes.len(), "patches": applied.patches, "game": file.game}))
    }

    pub fn cheat_toggle(&mut self, id: u32, enabled: bool) -> bool {
        self.running
            .as_mut()
            .is_some_and(|r| r.hooks.set_freeze_enabled(FreezeId(id), enabled))
    }

    pub fn cheat_remove(&mut self, id: u32) -> bool {
        self.running
            .as_mut()
            .is_some_and(|r| r.hooks.remove_freeze(FreezeId(id)).is_some())
    }

    /// The freezes as a cheat file (TOML), to keep in the database.
    pub fn cheats_export(&self) -> Result<String, String> {
        let r = self.running.as_ref().ok_or("no machine running")?;
        Ok(emu_cheats::from_hooks(r.machine.as_ref(), &r.hooks).to_toml())
    }

    /// Apply the database's cheats for an image's bytes from the uploaded
    /// files: `…/<machine>/<crc>.toml` or `<crc>.toml`. The game, if found.
    pub(crate) fn cheats_for_image(&mut self, data: &[u8]) -> Result<Option<String>, String> {
        let r = self.running.as_ref().ok_or("no machine running")?;
        let key = emu_cheats::media_key(data);
        let with_machine = format!("{}/{key}.toml", r.id);
        let alone = format!("{key}.toml");
        let found = self
            .files
            .iter()
            .find(|(p, _)| {
                let p = p.replace('\\', "/").to_lowercase();
                p.ends_with(&with_machine) || p == alone || p.ends_with(&format!("/{alone}"))
            })
            .map(|(_, d)| d.clone());
        let Some(bytes) = found else {
            return Ok(None);
        };
        let text = String::from_utf8(bytes).map_err(|_| format!("{alone}: not UTF-8"))?;
        let file = emu_cheats::CheatFile::from_toml(&text)?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        emu_cheats::apply(&file, r.machine.as_mut(), &mut r.hooks)?;
        Ok(Some(file.game.unwrap_or(key)))
    }

    /// Start a cheat search over the main CPU's space (`size` 1, 2 or 4).
    pub fn search_start(&mut self, size: u8) -> Result<Value, String> {
        let (space, _, dev) = self.main_space()?;
        let size = emu_cheats::size_from(size)?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        let bits = r
            .machine
            .memory_spaces()
            .first()
            .map_or(16, |s| s.addr_bits);
        let len = (1u64 << bits).min(u64::from(emu_cheats::search::MAX_SEARCH)) as u32;
        let t = r.machine.debug_target(dev).ok_or("no debug target")?;
        let s = Search::start(space, size, 0, len, |a| t.peek(space, a, size))?;
        let n = s.candidates.len();
        self.search = Some(s);
        Ok(json!({"candidates": n}))
    }

    pub fn search_next(&mut self, op: &str, value: Option<u32>) -> Result<Value, String> {
        let op = Op::parse(op, value)?;
        let (_, _, dev) = self.main_space()?;
        let mut s = self.search.take().ok_or("no search started")?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        let n = match r.machine.debug_target(dev) {
            Some(t) => {
                let (space, size) = (s.space, s.size);
                s.next(op, |a| t.peek(space, a, size))
            }
            None => 0,
        };
        self.search = Some(s);
        Ok(json!({"candidates": n}))
    }

    pub fn search_results(&self, max: usize) -> Value {
        let Some(s) = &self.search else {
            return json!({"candidates": 0, "results": []});
        };
        let results: Vec<Value> = s
            .candidates
            .iter()
            .take(max)
            .map(|&(a, v)| json!({"addr": a, "value": v}))
            .collect();
        json!({"candidates": s.candidates.len(), "size": s.size.bytes(), "results": results})
    }

    /// Freeze a search result at its current value.
    pub fn search_freeze(&mut self, addr: u32, label: Option<String>) -> Result<Value, String> {
        let size = self.search.as_ref().map_or(Size::Byte, |s| s.size);
        let (space, name, dev) = self.main_space()?;
        let r = self.running.as_mut().ok_or("no machine running")?;
        let value = r
            .machine
            .debug_target(dev)
            .and_then(|t| t.peek(space, addr, size))
            .ok_or("address not readable")?;
        let file = emu_cheats::CheatFile {
            game: None,
            freeze: vec![emu_cheats::CheatFreeze {
                label,
                space: name,
                addr,
                size: size.bytes() as u8,
                value,
                mode: "block_writes".into(),
                enabled: true,
            }],
            patch: Vec::new(),
        };
        let applied = emu_cheats::apply(&file, r.machine.as_mut(), &mut r.hooks)?;
        Ok(json!({"id": applied.freezes.first().map(|f| f.0), "value": value}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_search_and_freeze_in_the_browser_core() {
        let mut emu = WebEmu::new();
        assert!(
            emu.cheats_import("pokes", "POKE 2,3").is_err(),
            "no machine"
        );
        emu.start(&json!({"machine": "c64", "options": {"roms": "stub"}}))
            .unwrap();
        let r = emu
            .cheats_import("pokes", "POKE 53280,2 : REM red border")
            .unwrap();
        assert_eq!(r["freezes"], 1);
        let list = emu.cheats();
        assert_eq!(list[0]["label"], "red border");
        let id = list[0]["id"].as_u64().unwrap() as u32;
        assert!(emu.cheat_toggle(id, false));
        assert_eq!(emu.cheats()[0]["enabled"], false);
        assert!(emu.cheats_export().unwrap().contains("[[freeze]]"));
        assert!(emu.cheat_remove(id));
        assert!(emu.cheats().as_array().unwrap().is_empty());
        // Search: the whole 64 KB, then narrow to bytes equal to their old value.
        assert_eq!(emu.search_start(1).unwrap()["candidates"], 65536);
        emu.run_frame().unwrap();
        let n = emu.search_next("unchanged", None).unwrap()["candidates"]
            .as_u64()
            .unwrap();
        assert!(n > 1000 && n < 65536, "{n}");
        let first = emu.search_results(5)["results"][0]["addr"]
            .as_u64()
            .unwrap() as u32;
        assert!(emu.search_freeze(first, Some("x".into())).unwrap()["id"].is_u64());
        assert!(emu.search_next("fly", None).is_err());
    }

    #[test]
    fn the_database_comes_from_uploaded_files() {
        let mut emu = WebEmu::new();
        let image = vec![0x01, 0x08, 0x60];
        emu.add_file("game.prg", image.clone());
        emu.add_file(
            &format!("cheats/c64/{}.toml", emu_cheats::media_key(&image)),
            b"game = \"Game\"\n[[freeze]]\nspace = \"cpu\"\naddr = 53280\nvalue = 1\n".to_vec(),
        );
        let info = emu
            .start(&json!({"machine": "c64", "options": {"roms": "stub"},
                           "media": [{"file": "game.prg", "kind": "tape"}]}))
            .unwrap();
        assert_eq!(info["cheats"], json!(["Game"]));
        assert_eq!(emu.cheats()[0]["addr"], 53280);
    }
}
