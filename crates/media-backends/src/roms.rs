//! Finding user-supplied ROM images in a host directory.
//!
//! ROMs are never in the repository (golden rule 6). Users point
//! `EMU_ROM_DIR` (or `--rom-dir`) at a folder; files are matched by name
//! prefix and exact size, so common naming schemes work
//! (`kernal.bin`, `kernal-901227-03.bin`, `characters-901225-01.bin`, …).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One ROM a machine needs.
#[derive(Clone, Copy, Debug)]
pub struct RomSpec {
    /// Role name passed in `MachineConfig::roms`.
    pub role: &'static str,
    /// Accepted file name prefixes (lower case).
    pub prefixes: &'static [&'static str],
    pub size: usize,
    /// The machine works without it (e.g. no disk drive).
    pub optional: bool,
}

/// ROMs needed per machine id.
pub fn specs(machine: &str) -> &'static [RomSpec] {
    const C64: &[RomSpec] = &[
        RomSpec {
            role: "kernal",
            prefixes: &["kernal"],
            size: 0x2000,
            optional: false,
        },
        RomSpec {
            role: "basic",
            prefixes: &["basic"],
            size: 0x2000,
            optional: false,
        },
        RomSpec {
            role: "chargen",
            prefixes: &["chargen", "characters", "char"],
            size: 0x1000,
            optional: false,
        },
        RomSpec {
            role: "dos1541",
            prefixes: &["dos1541-", "1541-", "dos1541."],
            size: 0x4000,
            optional: true,
        },
    ];
    match machine {
        "c64" => C64,
        _ => &[],
    }
}

/// The ROM directory from the environment, if set.
pub fn env_dir() -> Option<PathBuf> {
    std::env::var_os("EMU_ROM_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Load every ROM `machine` needs from `dir`. Files are visited in sorted
/// order so the choice is deterministic when several match.
pub fn load(dir: &Path, machine: &str) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut entries: Vec<(String, PathBuf, u64)> = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_file() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            entries.push((name, e.path(), e.metadata()?.len()));
        }
    }
    entries.sort();

    let mut roms = BTreeMap::new();
    for spec in specs(machine) {
        let found = entries.iter().find(|(name, _, len)| {
            *len == spec.size as u64 && spec.prefixes.iter().any(|p| name.starts_with(p))
        });
        match found {
            Some((_, path, _)) => {
                roms.insert(spec.role.to_string(), fs::read(path)?);
            }
            None if spec.optional => {}
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "no {} ROM ({} bytes, name starting with {}) in {}",
                        spec.role,
                        spec.size,
                        spec.prefixes.join("/"),
                        dir.display()
                    ),
                ))
            }
        }
    }
    Ok(roms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_prefix_and_size() {
        let dir = std::env::temp_dir().join(format!("polytrace-roms-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("KERNAL-901227-03.BIN"), vec![1; 0x2000]).unwrap();
        fs::write(dir.join("basic.bin"), vec![2; 0x2000]).unwrap();
        fs::write(dir.join("characters.bin"), vec![3; 0x1000]).unwrap();
        fs::write(dir.join("char-wrong-size.bin"), vec![4; 10]).unwrap();
        let roms = load(&dir, "c64").unwrap();
        assert_eq!(roms["kernal"][0], 1);
        assert_eq!(roms["basic"][0], 2);
        assert_eq!(roms["chargen"][0], 3);

        fs::remove_file(dir.join("basic.bin")).unwrap();
        let err = load(&dir, "c64").unwrap_err();
        assert!(err.to_string().contains("basic"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn machines_without_roms_need_nothing() {
        assert!(specs("sim6502").is_empty());
    }
}
