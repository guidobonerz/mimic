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
/// The 1571 DOS (the C128D's 1571CR ROM is named `dos1571cr`).
const DOS1571: RomSpec = optional("dos1571", &["dos1571-", "1571-"], 0x8000);
const DOS1581: RomSpec = optional("dos1581", &["dos1581-", "1581-"], 0x8000);

/// ROMs needed per machine id.
pub fn specs(machine: &str) -> &'static [RomSpec] {
    const C64: &[RomSpec] = &[
        rom("kernal", &["kernal"], 0x2000),
        rom("basic", &["basic"], 0x2000),
        rom("chargen", &["chargen", "characters", "char"], 0x1000),
        DOS1541,
        DOS1571,
        DOS1581,
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
        DOS1571,
        DOS1581,
    ];
    // Every PET ROM is optional here: the model picks its set and reports
    // what is missing. Names follow the common `basic-4.901465-…` scheme.
    const PET: &[RomSpec] = &[
        optional("basic1", &["basic-1-", "basic1"], 0x2000),
        optional("basic2", &["basic-2-", "basic2"], 0x2000),
        optional("basic4", &["basic-4-", "basic4"], 0x3000),
        optional("kernal1", &["kernal-1-", "kernal1"], 0x1000),
        optional("kernal2", &["kernal-2-", "kernal2"], 0x1000),
        optional("kernal4", &["kernal-4-", "kernal4"], 0x1000),
        optional("edit1n", &["edit-1-n", "edit1n"], 0x800),
        optional("edit2n", &["edit-2-n", "edit2n"], 0x800),
        optional("edit2b", &["edit-2-b", "edit2b"], 0x800),
        optional("edit4-40n-50", &["edit-4-40-n-50hz", "edit4-40n-50"], 0x800),
        optional("edit4-40n-60", &["edit-4-40-n-60hz", "edit4-40n-60"], 0x800),
        optional("edit4-40b-50", &["edit-4-40-b-50hz", "edit4-40b-50"], 0x800),
        optional("edit4-40b-60", &["edit-4-40-b-60hz", "edit4-40b-60"], 0x800),
        // 901474-04 is the original; a patched copy often sits next to it.
        optional(
            "edit4-80b-50",
            &[
                "edit-4-80-b-50hz-901474-04-bin",
                "edit-4-80-b-50hz",
                "edit4-80b-50",
            ],
            0x800,
        ),
        optional("edit4-80b-60", &["edit-4-80-b-60hz", "edit4-80b-60"], 0x800),
        optional("chargen1", &["characters-1-", "chargen1"], 0x800),
        optional("chargen2", &["characters-2-", "chargen2"], 0x800),
    ];
    // C16 and Plus/4: kernal 318004 is PAL, 318005 NTSC; BASIC 3.5 is
    // 318006; the Plus/4's 3-plus-1 ROM is 317053 (low) and 317054 (high).
    const PLUS4: &[RomSpec] = &[
        rom(
            "kernal",
            &["kernal-318004-05", "kernal-318004", "kernal"],
            0x4000,
        ),
        optional(
            "kernal-ntsc",
            &["kernal-318005-05", "kernal-318005"],
            0x4000,
        ),
        rom("basic", &["basic-318006", "basic"], 0x4000),
        rom(
            "function-lo",
            &["3plus1-317053", "3plus1-lo", "function-lo"],
            0x4000,
        ),
        rom(
            "function-hi",
            &["3plus1-317054", "3plus1-hi", "function-hi"],
            0x4000,
        ),
        DOS1541,
        DOS1571,
        DOS1581,
    ];
    const C16: &[RomSpec] = &[PLUS4[0], PLUS4[1], PLUS4[2], DOS1541, DOS1571, DOS1581];
    // C128: KERNAL 318020, BASIC 318018 (low) / 318019 (high), character
    // ROM 390059; the C64-mode ROM as one 16 KB 251913 or as the C64's two
    // 8 KB ROMs (VICE names them basic64/kernal64).
    const C128: &[RomSpec] = &[
        rom(
            "kernal",
            &["kernal-318020", "kernal-c128", "kernal"],
            0x4000,
        ),
        rom("basic-lo", &["basiclo", "basic-lo", "basic-318018"], 0x4000),
        rom("basic-hi", &["basichi", "basic-hi", "basic-318019"], 0x4000),
        rom(
            "chargen",
            &["chargen-390059", "chargen", "characters"],
            0x2000,
        ),
        optional("c64", &["251913", "c64"], 0x4000),
        optional("basic64", &["basic64", "basic-901226"], 0x2000),
        // 901227-03 is the standard KERNAL; 325179/325182 are national ones.
        optional(
            "kernal64",
            &[
                "kernal64-901227-03",
                "kernal64-901227",
                "kernal64",
                "kernal-901227",
            ],
            0x2000,
        ),
        optional("function", &["function", "funcrom"], 0x8000),
        DOS1541,
        DOS1571,
        DOS1581,
    ];
    // Atari 800XL: XL/XE OS (Rev. 2 is CO61598B) and BASIC Rev. C
    // (CO24947A). Common names: atarixl.rom, atari-xl-os-rev2.rom,
    // ataribas.rom, basic-revc.rom.
    const ATARI800XL: &[RomSpec] = &[
        rom(
            "os",
            &[
                "co61598", "atarixl", "atari-xl", "xl-os", "xlos", "os-xl", "os",
            ],
            0x4000,
        ),
        optional(
            "basic",
            &["co24947", "ataribas", "atari-basic", "basic-revc", "basic"],
            0x2000,
        ),
        // 400/800 OS-A/OS-B (10 KB): ATARIOSB.ROM, atariosa.rom, co12499 …
        optional(
            "osb",
            &[
                "atariosb", "atariosa", "osb", "osa", "os-b", "os-a", "co12499", "co14599",
                "co12399", "co14699", "os400", "os800",
            ],
            0x2800,
        ),
    ];
    // Amstrad CPC 464/664/6128 (one machine, the model picks the ROMs):
    // 16 KB OS and BASIC per model (BASIC 1.0 on the 464, 1.1 on the
    // 6128), or the common 32 KB files with both (OS first); AMSDOS for
    // the disk models; ParaDOS as an optional AMSDOS replacement.
    const CPC: &[RomSpec] = &[
        optional("os464", &["os464", "os-464", "cpc464-os"], 0x4000),
        optional(
            "basic464",
            &[
                "basic1-0",
                "basic10",
                "basic-1-0",
                "basic464",
                "cpc464-basic",
            ],
            0x4000,
        ),
        optional("os664", &["os664", "os-664", "cpc664-os"], 0x4000),
        optional(
            "basic664",
            &["basic664", "basic-664", "cpc664-basic"],
            0x4000,
        ),
        optional("os6128", &["os6128", "os-6128", "cpc6128-os"], 0x4000),
        optional(
            "basic6128",
            &[
                "basic1-1",
                "basic11",
                "basic-1-1",
                "basic6128",
                "cpc6128-basic",
            ],
            0x4000,
        ),
        optional("rom464", &["cpc464"], 0x8000),
        optional("rom664", &["cpc664"], 0x8000),
        optional("rom6128", &["cpc6128"], 0x8000),
        optional("amsdos", &["amsdos"], 0x4000),
        optional("parados", &["parados"], 0x4000),
    ];
    // Amiga Kickstart: 1.2 (33.180), 1.3 (34.5), 2.04 (37.175), 3.1 (40.63),
    // as 256 KB images or 512 KB files that hold one twice. Common names:
    // kick13.rom, "Kickstart 1.3.rom", kick34005.a500.
    const AMIGA: &[RomSpec] = &[
        optional(
            "kick12",
            &[
                "kick12",
                "kick-1-2",
                "kickstart 1-2",
                "kickstart-1-2",
                "kick33180",
            ],
            0x8_0000,
        ),
        optional(
            "kick12",
            &[
                "kick12",
                "kick-1-2",
                "kickstart 1-2",
                "kickstart-1-2",
                "kick33180",
            ],
            0x4_0000,
        ),
        optional(
            "kick13",
            &[
                "kick13",
                "kick-1-3",
                "kickstart 1-3",
                "kickstart-1-3",
                "kick34005",
            ],
            0x8_0000,
        ),
        optional(
            "kick13",
            &[
                "kick13",
                "kick-1-3",
                "kickstart 1-3",
                "kickstart-1-3",
                "kick34005",
            ],
            0x4_0000,
        ),
        optional(
            "kick20",
            &[
                "kick20",
                "kick-2-0",
                "kickstart 2-0",
                "kickstart-2-0",
                "kick37175",
            ],
            0x8_0000,
        ),
        optional(
            "kick30",
            &[
                "kick30",
                "kick-3-0",
                "kickstart 3-0",
                "kickstart-3-0",
                "kick39106",
            ],
            0x8_0000,
        ),
        optional(
            "kick31",
            &[
                "kick31",
                "kick-3-1",
                "kickstart 3-1",
                "kickstart-3-1",
                "kick40063",
            ],
            0x8_0000,
        ),
        // A1000 bootstrap ROM (8 KB; loads the Kickstart disk into the WCS).
        optional(
            "a1000boot",
            &["a1000boot", "a1000-boot", "a1000 boot", "bootstrap"],
            0x2000,
        ),
    ];
    // Sinclair ZX81: the 8 KB ROM; the "improved" third edition first.
    const ZX81: &[RomSpec] = &[rom(
        "zx81",
        &[
            "zx81-edition3",
            "zx81-edition2",
            "zx81-edition1",
            "zx81",
            "zx-81",
            "sinclair-zx81",
        ],
        0x2000,
    )];
    // ColecoVision: the 8 KB BIOS.
    const COLECO: &[RomSpec] = &[rom(
        "coleco",
        &[
            "coleco",
            "colecovision",
            "colecobios",
            "coleco-bios",
            "bios-coleco",
        ],
        0x2000,
    )];
    match machine {
        "zx81" => ZX81,
        "coleco" => COLECO,
        "amiga" => AMIGA,
        "atari8" => ATARI800XL,
        "cpc" => CPC,
        "c64" => C64,
        "c128" => C128,
        "vic20" => VIC20,
        "pet" => PET,
        "plus4" => PLUS4,
        "c16" => C16,
        _ => &[],
    }
}

/// The machine's own subfolders of the ROM directory, most specific first.
fn folders(machine: &str) -> &'static [&'static str] {
    match machine {
        "c64" => &["c64"],
        "vic20" => &["vic20"],
        "pet" => &["pet"],
        "c128" => &["c128"],
        "plus4" => &["plus4"],
        "c16" => &["c16", "plus4"],
        "atari8" => &["atari8", "atari800xl", "atari"],
        "cpc" => &["cpc", "amstrad"],
        "amiga" => &["amiga"],
        "zx81" => &["zx81", "sinclair"],
        "coleco" => &["coleco", "colecovision"],
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
        // A role may be listed once per accepted size; the first found wins.
        if roms.contains_key(spec.role) {
            continue;
        }
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
        let dir = std::env::temp_dir().join(format!("mimic-roms-{name}-{}", std::process::id()));
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
    fn cpc_roms_per_model_and_combined() {
        let dir = temp_dir("cpc");
        fs::create_dir_all(dir.join("cpc")).unwrap();
        for (name, v) in [
            ("OS464.ROM", 1),
            ("BASIC1-0.ROM", 2),
            ("OS6128.ROM", 3),
            ("BASIC1-1.ROM", 4),
            ("AMSDOS.ROM", 5),
            ("BASIC664.ROM", 6),
        ] {
            fs::write(dir.join("cpc").join(name), vec![v; 0x4000]).unwrap();
        }
        fs::write(dir.join("cpc/cpc664.rom"), vec![7; 0x8000]).unwrap();
        let roms = load(&dir, "cpc").unwrap();
        assert_eq!(roms["os464"][0], 1);
        assert_eq!(roms["basic464"][0], 2);
        assert_eq!(roms["os6128"][0], 3);
        assert_eq!(roms["basic6128"][0], 4);
        assert_eq!(roms["amsdos"][0], 5);
        assert_eq!(roms["basic664"][0], 6);
        assert_eq!(roms["rom664"][0], 7);
        assert!(!roms.contains_key("os664"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn atari_xl_roms_by_common_names() {
        let dir = temp_dir("atari");
        fs::create_dir_all(dir.join("atari")).unwrap();
        fs::write(dir.join("atari/ATARIXL.ROM"), vec![1; 0x4000]).unwrap();
        // The 400/800 OS-B has another size and must not match.
        fs::write(dir.join("atari/ATARIOSB.ROM"), vec![2; 0x2800]).unwrap();
        fs::write(dir.join("atari/ATARIBAS.ROM"), vec![3; 0x2000]).unwrap();
        let roms = load(&dir, "atari8").unwrap();
        assert_eq!(roms["os"][0], 1);
        assert_eq!(roms["basic"][0], 3);
        fs::remove_file(dir.join("atari/ATARIBAS.ROM")).unwrap();
        let roms = load(&dir, "atari8").unwrap();
        assert!(!roms.contains_key("basic"), "BASIC is optional");
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
