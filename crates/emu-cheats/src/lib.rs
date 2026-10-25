//! Cheats: cheat files (TOML), imports of common formats, the per-game
//! cheat database and the cheat search (memory finder).
//!
//! Host-neutral and wasm-compatible: the debug server, the desktop and the
//! web frontend share it. Freezes go into [`emu_core::DebugHooks`] (golden
//! rule: zero cost while none is active); code patches are written once
//! through the machine's debug targets.
//!
//! Cheat files per game live in `cheats/<machine>/<key>.toml`, where the key
//! is the CRC-32 of the inserted image ([`media_key`]), so they apply
//! automatically when the image goes in ([`db_path`]).

#![forbid(unsafe_code)]

pub mod import;
pub mod search;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use emu_core::{DebugHooks, FreezeId, FreezeMode, Machine, MemSpaceId, Size};

/// One frozen value (TOML `[[freeze]]`).
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

/// Bytes written once when the cheat is applied (TOML `[[patch]]`), e.g. a
/// `NOP` over the instruction that loses a life.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub space: String,
    pub addr: u32,
    pub bytes: Vec<u8>,
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

/// A cheat file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatFile {
    /// The game's name, for people reading the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub freeze: Vec<CheatFreeze>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch: Vec<CheatPatch>,
}

impl CheatFile {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("cheat file: {e}"))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    /// The other file's cheats added after these.
    pub fn extend(&mut self, other: CheatFile) {
        self.game = self.game.take().or(other.game);
        self.freeze.extend(other.freeze);
        self.patch.extend(other.patch);
    }

    pub fn is_empty(&self) -> bool {
        self.freeze.is_empty() && self.patch.is_empty()
    }
}

pub fn size_from(bytes: u8) -> Result<Size, String> {
    match bytes {
        1 => Ok(Size::Byte),
        2 => Ok(Size::Word),
        4 => Ok(Size::Long),
        n => Err(format!("size must be 1, 2 or 4, not {n}")),
    }
}

pub fn mode_from(name: &str) -> Result<FreezeMode, String> {
    match name {
        "block_writes" => Ok(FreezeMode::BlockWrites),
        "reapply_per_frame" => Ok(FreezeMode::ReapplyPerFrame),
        "reapply_on_read" => Ok(FreezeMode::ReapplyOnRead),
        other => Err(format!("unknown freeze mode '{other}'")),
    }
}

pub fn mode_name(mode: FreezeMode) -> &'static str {
    match mode {
        FreezeMode::BlockWrites => "block_writes",
        FreezeMode::ReapplyPerFrame => "reapply_per_frame",
        FreezeMode::ReapplyOnRead => "reapply_on_read",
    }
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub freezes: Vec<FreezeId>,
    pub patches: usize,
}

fn space_of(m: &dyn Machine, name: &str) -> Result<(MemSpaceId, emu_core::DeviceId), String> {
    m.memory_spaces()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| (s.id, s.device))
        .ok_or_else(|| format!("unknown memory space '{name}'"))
}

/// Add the file's freezes to `hooks` and write its enabled patches (all or
/// nothing: every entry is checked first). A freeze's value is also put in
/// place at once, so video fetches see it from the start.
pub fn apply(
    file: &CheatFile,
    m: &mut dyn Machine,
    hooks: &mut DebugHooks,
) -> Result<Applied, String> {
    let mut freezes = Vec::with_capacity(file.freeze.len());
    for c in &file.freeze {
        let (space, dev) = space_of(m, &c.space)?;
        let size = size_from(c.size)?;
        let mode = mode_from(&c.mode)?;
        if c.label.as_ref().is_some_and(|l| l.len() > 255) {
            return Err("label longer than 255".into());
        }
        freezes.push((
            emu_core::Freeze {
                mode,
                enabled: c.enabled,
                label: c.label.clone(),
                ..emu_core::Freeze::new(space, c.addr, size, c.value)
            },
            dev,
        ));
    }
    let mut patches = Vec::new();
    for p in file.patch.iter().filter(|p| p.enabled) {
        let (space, dev) = space_of(m, &p.space)?;
        patches.push((space, dev, p));
    }
    let mut applied = Applied::default();
    for (f, dev) in freezes {
        if f.enabled && f.mode != FreezeMode::ReapplyOnRead {
            if let Some(t) = m.debug_target(dev) {
                let _ = t.poke(f.space, f.addr, f.size, f.value);
            }
        }
        applied.freezes.push(hooks.add_freeze(f));
    }
    for (space, dev, p) in patches {
        let t = m
            .debug_target(dev)
            .ok_or_else(|| format!("no debug target for space '{}'", p.space))?;
        for (i, &b) in p.bytes.iter().enumerate() {
            t.poke(
                space,
                p.addr.wrapping_add(i as u32),
                Size::Byte,
                u32::from(b),
            );
        }
        applied.patches += 1;
    }
    Ok(applied)
}

/// The freezes in `hooks` as a cheat file (spaces by name).
pub fn from_hooks(m: &dyn Machine, hooks: &DebugHooks) -> CheatFile {
    let spaces = m.memory_spaces();
    CheatFile {
        game: None,
        freeze: hooks
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
        patch: Vec::new(),
    }
}

/// CRC-32 (IEEE 802.3, the one ROM catalogues use) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The database key of an image: its CRC-32 as eight lower-case hex digits.
pub fn media_key(data: &[u8]) -> String {
    format!("{:08x}", crc32(data))
}

/// Where the cheat file of an image lives: `<dir>/<machine>/<key>.toml`.
pub fn db_path(dir: &Path, machine: &str, key: &str) -> PathBuf {
    dir.join(machine).join(format!("{key}.toml"))
}

/// The cheat file for `data` in the database at `dir`, if there is one.
pub fn lookup(dir: &Path, machine: &str, data: &[u8]) -> Result<Option<CheatFile>, String> {
    let path = db_path(dir, machine, &media_key(data));
    match std::fs::read_to_string(&path) {
        Ok(text) => CheatFile::from_toml(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A cheat file's format from its file name: `.pokes`/`.bas` POKE lists,
/// `.mon`/`.vice` VICE monitor commands, `.txt`/`.codes` address/value
/// codes, anything else TOML.
pub fn format_for_path(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pokes" | "bas" => "pokes",
        "mon" | "vice" => "vice",
        "txt" | "codes" => "codes",
        _ => "toml",
    }
}

/// The cheat database folder a frontend uses: `EMU_CHEATS_DIR`, else
/// `cheats/` in the working directory if it exists.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("EMU_CHEATS_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let local = PathBuf::from("cheats");
    local.is_dir().then_some(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(media_key(b""), "00000000");
    }

    #[test]
    fn cheat_files_round_trip_through_toml() {
        let file = CheatFile {
            game: Some("Test".into()),
            freeze: vec![CheatFreeze {
                label: Some("Lives".into()),
                space: "cpu".into(),
                addr: 0x0810,
                size: 1,
                value: 9,
                mode: "block_writes".into(),
                enabled: true,
            }],
            patch: vec![CheatPatch {
                label: Some("no collision".into()),
                space: "cpu".into(),
                addr: 0x1000,
                bytes: vec![0xea, 0xea],
                enabled: true,
            }],
        };
        let text = file.to_toml();
        assert!(text.contains("[[freeze]]") && text.contains("[[patch]]"));
        assert_eq!(CheatFile::from_toml(&text).unwrap(), file);
        let minimal =
            CheatFile::from_toml("[[freeze]]\nspace = \"cpu\"\naddr = 2048\nvalue = 3\n").unwrap();
        assert_eq!(minimal.freeze[0].size, 1);
        assert_eq!(minimal.freeze[0].mode, "block_writes");
        assert!(minimal.freeze[0].enabled);
        assert!(CheatFile::from_toml("[[freeze]]\naddr = 1").is_err());
    }

    #[test]
    fn database_lookup_by_image_crc() {
        let dir = std::env::temp_dir().join(format!("mimic-cheats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let image = b"a game image";
        let path = db_path(&dir, "c64", &media_key(image));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "game = \"Game\"\n[[freeze]]\nspace = \"cpu\"\naddr = 1\nvalue = 2\n",
        )
        .unwrap();
        assert_eq!(
            lookup(&dir, "c64", image).unwrap().unwrap().game.as_deref(),
            Some("Game")
        );
        assert!(lookup(&dir, "c64", b"other").unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
