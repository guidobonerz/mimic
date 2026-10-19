//! Machine registry: frontends pick machines by id without depending on any
//! `machine-*` crate. `emu-machines` fills it.

use std::collections::BTreeMap;
use std::fmt;

use crate::machine::Machine;

/// Configuration for creating a machine. Model variants (PAL/NTSC, Kernal
/// revisions, RAM sizes) are configuration, not separate machines. ROMs are
/// passed as bytes: machines never touch the host file system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineConfig {
    /// Model name, `None` for the machine's default.
    pub model: Option<String>,
    /// ROM images by role, e.g. "kernal", "basic", "chargen".
    pub roms: BTreeMap<String, Vec<u8>>,
    pub options: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    UnknownMachine(String),
    UnknownModel(String),
    MissingRom(String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnknownMachine(id) => write!(f, "unknown machine '{id}'"),
            ConfigError::UnknownModel(model) => write!(f, "unknown model '{model}'"),
            ConfigError::MissingRom(role) => write!(f, "missing ROM '{role}'"),
            ConfigError::Invalid(msg) => write!(f, "invalid configuration: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

pub type MachineFactory = fn(&MachineConfig) -> Result<Box<dyn Machine>, ConfigError>;

#[derive(Clone, Copy)]
pub struct MachineEntry {
    /// Stable id used on the command line and in the debug protocol, e.g. "c64".
    pub id: &'static str,
    pub name: &'static str,
    pub models: &'static [&'static str],
    pub create: MachineFactory,
}

impl fmt::Debug for MachineEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineEntry")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("models", &self.models)
            .finish_non_exhaustive()
    }
}

/// Registered machines in registration order (deterministic listing).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    entries: Vec<MachineEntry>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a machine; an entry with the same id is replaced.
    pub fn register(&mut self, entry: MachineEntry) {
        match self.entries.iter_mut().find(|e| e.id == entry.id) {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    pub fn entries(&self) -> &[MachineEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&MachineEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn create(&self, id: &str, config: &MachineConfig) -> Result<Box<dyn Machine>, ConfigError> {
        let entry = self.get(id).ok_or_else(|| ConfigError::UnknownMachine(id.to_owned()))?;
        (entry.create)(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs_kernal(_: &MachineConfig) -> Result<Box<dyn Machine>, ConfigError> {
        Err(ConfigError::MissingRom("kernal".into()))
    }

    fn entry(id: &'static str, name: &'static str) -> MachineEntry {
        MachineEntry { id, name, models: &["pal", "ntsc"], create: needs_kernal }
    }

    #[test]
    fn register_list_and_create() {
        let mut reg = Registry::new();
        reg.register(entry("c64", "Commodore 64"));
        reg.register(entry("vic20", "VIC-20"));
        reg.register(entry("c64", "C64 (replaced)"));

        let ids: Vec<_> = reg.entries().iter().map(|e| e.id).collect();
        assert_eq!(ids, ["c64", "vic20"]);
        assert_eq!(reg.get("c64").map(|e| e.name), Some("C64 (replaced)"));

        let cfg = MachineConfig::default();
        assert_eq!(
            reg.create("c64", &cfg).err(),
            Some(ConfigError::MissingRom("kernal".into()))
        );
        assert_eq!(
            reg.create("amiga", &cfg).err(),
            Some(ConfigError::UnknownMachine("amiga".into()))
        );
    }
}
