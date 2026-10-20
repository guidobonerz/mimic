//! Finding user-supplied ROM images in a host directory.
//!
//! ROMs are never in the repository (golden rule 6). Users point
//! `EMU_ROM_DIR` (or `--rom-dir`) at a folder. It is searched in this
//! order: the machine's own subfolders (`c64/`, `vic20/`, …), then
//! `drives/`, then the folder itself, so both a flat folder and one
//! subfolder per machine work.
//!
//! Files are matched by name prefix and exact size, so common naming
//! schemes work (`kernal.bin`, `kernal-901227-03.bin`,
//! `kernal.901486-07.bin`, `characters-901225-01.bin`, …). Names are
//! compared in lower case with `.` and `_` read as `-`. Within a folder
//! the first prefix of a spec that matches any file wins, so a spec
//! lists the preferred part number before the generic name.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One ROM a machine needs.
#[derive(Clone, Copy, Debug)]
pub struct RomSpec {
    /// Role name passed in `MachineConfig::roms`.
    pub role: &'static str,
    /// Accepted file name prefixes (lower case, `-` for `.` and `_`),
    /// preferred first.
    pub prefixes: &'static [&'static str],
    pub size: usize,
    /// The machine works without it (e.g. no disk drive).
    pub optional: bool,
}

const fn rom(role: &'static str, prefixes: &'static [&'static str], size: usize) -> RomSpec {
    RomSpec {
        role,
        prefixes,
        size,
        optional: false,
    }
}

const fn optional(role: &'static str, prefixes: &'static [&'static str], size: usize) -> RomSpec {
    RomSpec {
        role,
        prefixes,
        size,
        optional: true,
    }
}

/// The 1541 DOS (not the 1541-II, whose name also starts with `dos1541`).
const DOS1541: RomSpec = optional("dos1541", &["dos1541-", "1541-"], 0x4000);

/// ROMs needed per machine id.
pub fn specs(machine: &str) -> &'static [RomSpec] {
    const C64: &[RomSpec] = &[
        rom("kernal", &["kernal"], 0x2000),
        rom("basic", &["basic"], 0x2000),
        rom("chargen", &["chargen", "characters", "char"], 0x1000),
        DOS1541,
    ];
    // Kernal 901486-07 is the PAL one, -06 and -02 are NTSC; character
    // ROM 901460-03 is the standard set (-02 is the Japanese VIC-1001).
    const VIC20: &[RomSpec] = &[
        rom("kernal", &["kernal-901486-07", "kernal"], 0x2000),
        optional(
            "kernal-ntsc",
            &["kernal-901486-06", "kernal-901486-02"],
            0x2000,
        ),
        rom("basic", &["basic"], 0x2000),
        rom(
            "chargen",
            &[
                "chargen-901460-03",
                "characters-901460-03",
                "chargen",
                "characters",
                "char",
            ],
            0x1000,
        ),
        DOS1541,
    ];
    match machine {
        "c64" => C64,
        "vic20" => VIC20,
        _ => &[],
    }
}

/// The machine's own subfolders of the ROM directory, most specific first.
fn folders(machine: &str) -> &'static [&'static str] {
    match machine {
        "c64" => &["c64"],
        "vic20" => &["vic20"],
        _ => &[],
    }
}

/// The ROM directory from the environment, if set.
pub fn env_dir() -> Option<PathBuf> {
    std::env::var_os("EMU_ROM_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// A file name as prefixes are written: lower case, `-` for `.` and `_`.
fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c == '.' || c == '_' { '-' } else { c })
        .collect()
}

/// (normalized name, path, size) of the files in `dir`, sorted by name.
fn files(dir: &Path) -> io::Result<Vec<(String, PathBuf, u64)>> {
    let mut entries = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_file() {
            let name = normalize(&e.file_name().to_string_lossy());
            entries.push((name, e.path(), e.metadata()?.len()));
        }
    }
    entries.sort();
    Ok(entries)
}

/// Load every ROM `machine` needs from `dir` (see the module docs for the
/// search order). The choice is deterministic when several files match.
pub fn load(dir: &Path, machine: &str) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut search = Vec::new();
    for sub in folders(machine).iter().chain(&["drives"]) {
        let path = dir.join(sub);
        if path.is_dir() {
            search.push(files(&path)?);
        }
    }
    search.push(files(dir)?);

    let mut roms = BTreeMap::new();
    for spec in specs(machine) {
        let found = search.iter().find_map(|entries| {
            spec.prefixes.iter().find_map(|p| {
                entries
                    .iter()
                    .find(|(name, _, len)| *len == spec.size as u64 && name.starts_with(p))
            })
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
                        "no {} ROM ({} bytes, name starting with {}) in {} or its {} folders",
                        spec.role,
                        spec.size,
                        spec.prefixes.join("/"),
                        dir.display(),
                        folders(machine)
                            .iter()
                            .chain(&["drives"])
                            .copied()
                            .collect::<Vec<_>>()
                            .join("/"),
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("polytrace-roms-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn matches_by_prefix_and_size() {
        let dir = temp_dir("flat");
        fs::write(dir.join("KERNAL-901227-03.BIN"), vec![1; 0x2000]).unwrap();
        fs::write(dir.join("basic.bin"), vec![2; 0x2000]).unwrap();
        fs::write(dir.join("characters.bin"), vec![3; 0x1000]).unwrap();
        fs::write(dir.join("char-wrong-size.bin"), vec![4; 10]).unwrap();
        let roms = load(&dir, "c64").unwrap();
        assert_eq!(roms["kernal"][0], 1);
        assert_eq!(roms["basic"][0], 2);
        assert_eq!(roms["chargen"][0], 3);
        assert!(!roms.contains_key("dos1541"));

        fs::remove_file(dir.join("basic.bin")).unwrap();
        let err = load(&dir, "c64").unwrap_err();
        assert!(err.to_string().contains("basic"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn machine_and_drive_subfolders_come_first() {
        let dir = temp_dir("tree");
        for sub in ["c64", "vic20", "drives"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        // A stray C64 kernal at the top must not shadow the VIC-20 folder.
        fs::write(dir.join("kernal.bin"), vec![9; 0x2000]).unwrap();
        fs::write(dir.join("c64/kernal-901227-03.bin"), vec![1; 0x2000]).unwrap();
        fs::write(dir.join("c64/basic-901226-01.bin"), vec![2; 0x2000]).unwrap();
        fs::write(dir.join("c64/chargen-901225-01.bin"), vec![3; 0x1000]).unwrap();
        fs::write(dir.join("vic20/kernal.901486-02.bin"), vec![0x02; 0x2000]).unwrap();
        fs::write(dir.join("vic20/kernal.901486-06.bin"), vec![0x06; 0x2000]).unwrap();
        fs::write(dir.join("vic20/kernal.901486-07.bin"), vec![0x07; 0x2000]).unwrap();
        fs::write(dir.join("vic20/basic-901486-01.bin"), vec![4; 0x2000]).unwrap();
        fs::write(dir.join("vic20/chargen-901460-02.bin"), vec![0x02; 0x1000]).unwrap();
        fs::write(dir.join("vic20/chargen-901460-03.bin"), vec![0x03; 0x1000]).unwrap();
        fs::write(
            dir.join("drives/dos1541ii-251968-03.bin"),
            vec![0xee; 0x4000],
        )
        .unwrap();
        fs::write(
            dir.join("drives/dos1541-325302-01+901229-05.bin"),
            vec![5; 0x4000],
        )
        .unwrap();

        let c64 = load(&dir, "c64").unwrap();
        assert_eq!(c64["kernal"][0], 1);
        assert_eq!(c64["dos1541"][0], 5, "1541, not the 1541-II");

        let vic = load(&dir, "vic20").unwrap();
        assert_eq!(vic["kernal"][0], 0x07, "PAL kernal preferred");
        assert_eq!(vic["kernal-ntsc"][0], 0x06);
        assert_eq!(vic["chargen"][0], 0x03, "standard character set");
        assert_eq!(vic["basic"][0], 4);
        assert_eq!(vic["dos1541"][0], 5);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn machines_without_roms_need_nothing() {
        assert!(specs("sim6502").is_empty());
    }
}
