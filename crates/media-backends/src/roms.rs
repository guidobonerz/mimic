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
//!
//! Amiga Forever's encrypted ROM files (`amiga-os-310-a1200.rom` …: the
//! header `AMIROMTYPE1`, then the image XORed byte by byte with the
//! repeating contents of `rom.key`) are decrypted when a `rom.key` lies in
//! the same folder (Cloanto, Amiga Forever documentation, "ROM files");
//! without the key they are skipped.

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
    // kick13.rom, "Kickstart 1.3.rom", kick34005.a500, Amiga Forever's
    // amiga-os-130.rom. The A1200's own 3.0/3.1 (39.106, 40.068) are
    // separate roles the A1200 model prefers.
    const AMIGA: &[RomSpec] = &[
        optional(
            "kick30a1200",
            &["amiga-os-300-a1200", "kick30-a1200", "kick39106-a1200"],
            0x8_0000,
        ),
        optional(
            "kick31a1200",
            &["amiga-os-310-a1200", "kick31-a1200", "kick40068-a1200"],
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
                "amiga-os-120",
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
                "amiga-os-120",
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
                "amiga-os-130",
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
                "amiga-os-130",
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
    // ZX Spectrum: one ROM per model (the 128K and +2 files hold both
    // 16 KB ROMs, the 128 editor first).
    const SPECTRUM: &[RomSpec] = &[
        optional(
            "spectrum48",
            &["spec48", "spectrum48", "zx48", "48k", "48-rom", "48"],
            0x4000,
        ),
        optional(
            "spectrum128",
            &[
                "spec128-rom",
                "spec128",
                "spectrum128",
                "zx128",
                "128k",
                "128",
            ],
            0x8000,
        ),
        optional(
            "spectrumplus2",
            &["specp2-rom", "specp2", "spectrumplus2", "plus2", "+2"],
            0x8000,
        ),
        // Multiface 128 and Interface 1 (8 KB each).
        optional("mf128", &["mf128", "multiface128", "multiface-128"], 0x2000),
        optional(
            "if1",
            &[
                "specif1-edition2",
                "specif1",
                "if1-",
                "interface1",
                "interface-1",
            ],
            0x2000,
        ),
    ];
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
    // SNES: the sound CPU's 64-byte boot ROM (optional: an own loader
    // stands in).
    const SNES: &[RomSpec] = &[optional(
        "spc700",
        &["spc700", "ipl", "snes-ipl", "spc700-ipl", "apu"],
        64,
    )];
    // Vectrex: the 8 KB system ROM (executive and Mine Storm); a 4 KB
    // image of the executive alone is accepted too.
    const VECTREX: &[RomSpec] = &[
        optional(
            "vectrex",
            &[
                "vectrex",
                "system",
                "bios",
                "exec",
                "mine-storm",
                "minestorm",
            ],
            0x2000,
        ),
        optional("vectrex", &["vectrex", "system", "bios", "exec"], 0x1000),
    ];
    // Apple II family: every ROM optional, the model picks its set and
    // reports what is missing. Apple part numbers, as dumps are usually
    // named (341-0011 also as "341011d0.bin", 342-0304 as
    // "342_0304_CD.bin").
    const APPLE2: &[RomSpec] = &[
        optional("apple2-e0", &["341-0001", "3410001"], 0x800),
        optional("apple2-e8", &["341-0002", "3410002"], 0x800),
        optional("apple2-f0", &["341-0003", "3410003"], 0x800),
        optional("apple2-f8", &["341-0004", "3410004"], 0x800),
        optional(
            "apple2plus",
            &["apple2plus", "apple2-plus", "apple-ii-plus"],
            0x3000,
        ),
        optional("apple2plus-d0", &["341-0011", "3410011", "341011"], 0x800),
        optional("apple2plus-d8", &["341-0012", "3410012", "341012"], 0x800),
        optional("apple2plus-e0", &["341-0013", "3410013", "341013"], 0x800),
        optional("apple2plus-e8", &["341-0014", "3410014", "341014"], 0x800),
        optional("apple2plus-f0", &["341-0015", "3410015", "341015f0"], 0x800),
        optional("apple2plus-f8", &["341-0020", "3410020", "341020"], 0x800),
        optional("apple2e-cd", &["342-0135", "3420135"], 0x2000),
        optional("apple2e-ef", &["342-0134", "3420134"], 0x2000),
        optional("apple2ee-cd", &["342-0304", "3420304"], 0x2000),
        optional("apple2ee-ef", &["342-0303", "3420303"], 0x2000),
        optional("apple2-video", &["341-0036", "3410036"], 0x800),
        optional(
            "apple2e-video",
            &["342-0133", "3420133", "unenhanced-video-rom"],
            0x1000,
        ),
        optional(
            "apple2ee-video",
            &["342-0265", "3420265", "enhanced-video-rom"],
            0x1000,
        ),
        optional("disk2", &["341-0027", "3410027"], 0x100),
        // The P6 is 341-0028; some dumps are named 342-0028.
        optional("disk2-p6", &["341-0028", "3410028", "3420028"], 0x100),
    ];
    // Macintosh: one image per model (dumps are usually named after the
    // machine) or its two chips, H the even bytes and L the odd (Apple
    // part numbers 342-0220/0221 for the 128K/512K, 342-0341/0342 for the
    // Plus, 342-0701/0702 for the SE). Every ROM is optional: the model
    // picks its own and reports what is missing.
    const MAC: &[RomSpec] = &[
        optional(
            "mac128k",
            &["mac128k", "mac-128k", "macintosh-128", "128k"],
            0x1_0000,
        ),
        // The 512K's ROM is a revision of the 128K's (checksum 28BA4E50).
        optional(
            "mac512k",
            &["mac512k", "mac-512k", "macintosh-512k", "512k"],
            0x1_0000,
        ),
        optional("mac128k-h", &["342-0220"], 0x8000),
        optional("mac128k-l", &["342-0221"], 0x8000),
        optional(
            "macplus",
            &["macplus", "mac-plus", "macintosh-plus", "mac512ke", "plus"],
            0x2_0000,
        ),
        optional("macplus-h", &["342-0341"], 0x1_0000),
        optional("macplus-l", &["342-0342"], 0x1_0000),
        optional(
            "macse",
            &["macse", "mac-se", "macintosh-se", "se-"],
            0x4_0000,
        ),
        optional("macse-h", &["342-0701"], 0x2_0000),
        optional("macse-l", &["342-0702"], 0x2_0000),
    ];
    match machine {
        "mac" => MAC,
        "apple2" => APPLE2,
        "zx81" => ZX81,
        "spectrum" => SPECTRUM,
        "vectrex" => VECTREX,
        "coleco" => COLECO,
        "snes" => SNES,
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
        "spectrum" => &["spectrum", "zxspectrum", "sinclair"],
        "coleco" => &["coleco", "colecovision"],
        "snes" => &["snes", "sfc", "superfamicom"],
        "vectrex" => &["vectrex"],
        "apple2" => &["apple2", "apple", "appleii"],
        "mac" => &["mac", "macintosh", "apple"],
        _ => &[],
    }
}

/// The ROM directory from the environment, if set.
pub fn env_dir() -> Option<PathBuf> {
    std::env::var_os("EMU_ROM_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The header of an Amiga Forever encrypted ROM file.
const CLOANTO_HEADER: &[u8] = b"AMIROMTYPE1";

/// The image in an Amiga Forever encrypted ROM file (`AMIROMTYPE1`, then
/// the image XORed with the repeating key); `None` if `data` is not one or
/// the key is empty.
pub fn decrypt_cloanto(data: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    let body = data.strip_prefix(CLOANTO_HEADER)?;
    if key.is_empty() {
        return None;
    }
    Some(
        body.iter()
            .zip(key.iter().cycle())
            .map(|(b, k)| b ^ k)
            .collect(),
    )
}

/// A file name as prefixes are written: lower case, `-` for `.` and `_`.
fn normalize(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c == '.' || c == '_' { '-' } else { c })
        .collect()
}

/// (normalized name, path, image size) of the files in `dir`, sorted by
/// name. An Amiga Forever encrypted file counts with its image size when
/// the folder has a `rom.key`, and is left out without one.
fn files(dir: &Path) -> io::Result<Vec<(String, PathBuf, u64)>> {
    let mut entries = Vec::new();
    let has_key = dir.join("rom.key").is_file();
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_file() {
            let name = normalize(&e.file_name().to_string_lossy());
            let mut len = e.metadata()?.len();
            let header = CLOANTO_HEADER.len() as u64;
            if len % 1024 == header && is_cloanto(&e.path()) {
                if !has_key {
                    continue;
                }
                len -= header;
            }
            entries.push((name, e.path(), len));
        }
    }
    entries.sort();
    Ok(entries)
}

fn is_cloanto(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0; 11];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head == CLOANTO_HEADER)
}

/// A ROM file's image: Amiga Forever files decrypted with the folder's key.
fn read_rom(path: &Path) -> io::Result<Vec<u8>> {
    let data = fs::read(path)?;
    if data.starts_with(CLOANTO_HEADER) {
        let key = fs::read(path.with_file_name("rom.key"))?;
        return decrypt_cloanto(&data, &key)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "empty rom.key"));
    }
    Ok(data)
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
                roms.insert(spec.role.to_string(), read_rom(path)?);
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

/// [`load`] for files that did not come from a folder on this host (the
/// browser): `files` are (path, bytes) with `/` between folders, e.g.
/// `roms/c64/kernal.bin`. Files in a folder named after the machine come
/// first, then `drives/`, then all others; the rest is as in [`load`].
pub fn match_files(
    files: &[(String, Vec<u8>)],
    machine: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let entry = |(path, data): &(String, Vec<u8>)| {
        let mut parts = path.rsplit(['/', '\\']);
        let name = normalize(parts.next().unwrap_or(""));
        let folder = parts.next().map(str::to_lowercase).unwrap_or_default();
        (folder, name, data.clone())
    };
    let mut all: Vec<(String, String, Vec<u8>)> = files.iter().map(entry).collect();
    // Amiga Forever files: decrypted with a rom.key from the same folder.
    let keys: Vec<(String, Vec<u8>)> = all
        .iter()
        .filter(|(_, name, _)| name == "rom-key")
        .map(|(folder, _, data)| (folder.clone(), data.clone()))
        .collect();
    all.retain_mut(|(folder, _, data)| {
        if !data.starts_with(CLOANTO_HEADER) {
            return true;
        }
        let key = keys.iter().find(|(f, _)| f == folder);
        match key.and_then(|(_, k)| decrypt_cloanto(data, k)) {
            Some(image) => {
                *data = image;
                true
            }
            None => false,
        }
    });
    let mut groups: Vec<Vec<&(String, String, Vec<u8>)>> = Vec::new();
    for sub in folders(machine).iter().chain(&["drives"]) {
        groups.push(all.iter().filter(|(f, _, _)| f == sub).collect());
    }
    let mut rest: Vec<_> = all.iter().collect();
    rest.sort_by(|a, b| a.1.cmp(&b.1));
    groups.push(rest);
    for g in &mut groups {
        g.sort_by(|a, b| a.1.cmp(&b.1));
    }
    let mut roms = BTreeMap::new();
    for spec in specs(machine) {
        if roms.contains_key(spec.role) {
            continue;
        }
        let found = groups.iter().find_map(|g| {
            spec.prefixes.iter().find_map(|p| {
                g.iter()
                    .find(|(_, name, data)| data.len() == spec.size && name.starts_with(p))
            })
        });
        match found {
            Some((_, _, data)) => {
                roms.insert(spec.role.to_string(), data.clone());
            }
            None if spec.optional => {}
            None => {
                return Err(format!(
                    "no {} ROM ({} bytes, name starting with {})",
                    spec.role,
                    spec.size,
                    spec.prefixes.join("/")
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

    /// An Amiga Forever file of `image` under `key`.
    fn cloanto(image: &[u8], key: &[u8]) -> Vec<u8> {
        let mut f = CLOANTO_HEADER.to_vec();
        f.extend(image.iter().zip(key.iter().cycle()).map(|(b, k)| b ^ k));
        f
    }

    #[test]
    fn amiga_forever_roms_are_decrypted_with_rom_key() {
        let key: Vec<u8> = (0..1544u32).map(|i| (i * 7 + 3) as u8).collect();
        let image: Vec<u8> = (0..0x8_0000u32).map(|i| (i >> 3) as u8).collect();
        let dir = temp_dir("cloanto");
        fs::create_dir_all(dir.join("amiga")).unwrap();
        let file = dir.join("amiga/amiga-os-310-a1200.rom");
        fs::write(&file, cloanto(&image, &key)).unwrap();
        // Without the key the file is not offered.
        assert!(!load(&dir, "amiga").unwrap().contains_key("kick31a1200"));
        fs::write(dir.join("amiga/rom.key"), &key).unwrap();
        let roms = load(&dir, "amiga").unwrap();
        assert_eq!(roms.get("kick31a1200"), Some(&image));
        assert!(
            !roms.contains_key("kick31"),
            "the A1200 image is its own role"
        );
        // Uploaded files (browser) the same way.
        let files = vec![
            (
                "roms/amiga/amiga-os-310-a1200.rom".to_string(),
                cloanto(&image, &key),
            ),
            ("roms/amiga/rom.key".to_string(), key.clone()),
        ];
        let roms = match_files(&files, "amiga").unwrap();
        assert_eq!(roms.get("kick31a1200"), Some(&image));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uploaded_files_match_like_a_folder() {
        let files = vec![
            ("roms/KERNAL-901227-03.BIN".to_string(), vec![1; 0x2000]),
            ("roms/basic.bin".to_string(), vec![2; 0x2000]),
            ("roms/characters.bin".to_string(), vec![3; 0x1000]),
            ("roms/c64/kernal.bin".to_string(), vec![9; 0x2000]),
            ("roms/char-wrong-size.bin".to_string(), vec![4; 10]),
        ];
        let roms = match_files(&files, "c64").unwrap();
        assert_eq!(roms["kernal"][0], 9, "the machine's folder first");
        assert_eq!(roms["basic"][0], 2);
        assert_eq!(roms["chargen"][0], 3);
        let err = match_files(&files[..1], "c64").unwrap_err();
        assert!(err.contains("basic"), "{err}");
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
