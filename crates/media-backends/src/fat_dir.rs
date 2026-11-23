//! A host folder as a block device holding a FAT32 file system, for
//! emulated USB sticks and SD cards (the Ultimate 64 Elite II's firmware
//! reads and writes them like real ones). The volume is built when the
//! folder is opened; what the guest writes goes back into the folder on
//! [`commit`](MediaBackend::commit).
//!
//! # Volume layout
//!
//! 512-byte sectors. Sector 0 is an MBR with one partition of type `0x0C`
//! (FAT32, LBA) from sector 2048 to the end (CHS fields as 255 heads and
//! 63 sectors a track, `FE FF FF` past cylinder 1023; disk signature = the
//! volume serial). The partition holds:
//! - the boot sector with the BPB (OEM name `MIMIC`, media `F8`, label
//!   `MIMIC`, not bootable: the code halts), FSInfo in sector 1, a third
//!   boot-region sector with the `55 AA` signature in sector 2, their
//!   backups in sectors 6–8;
//! - 32 reserved sectors, plus as many as align the data area to the
//!   cluster size;
//! - two FATs, each `ceil((sectors − 32) / ((256 · spc + 2) / 2))` sectors
//!   (the specification's FAT size formula);
//! - the data area. Sectors per cluster follow the specification's table
//!   for FAT32: up to 260 MB 1, up to 8 GB 8, up to 16 GB 16, up to 32 GB
//!   32, above 64; halved while the volume would have fewer than 65 525
//!   clusters (it would not be FAT32 then), and a volume too small even
//!   with one sector a cluster grows in 1 MiB steps until it is FAT32
//!   (about 34 MiB).
//!
//! The root directory is cluster 2 and starts with the volume label entry;
//! then come all directories (pre-order, sorted by name), then all files
//! in the same order, each in one contiguous run. Directory entries carry
//! VFAT long names (with the short name's checksum) whenever the name is
//! not a plain upper-case 8.3 name, and unique 8.3 short names (basis name
//! plus `~n` tail, the specification's procedure). Times are the host
//! files' modification times in UTC (clamped to 1980–2107; dates of
//! creation, access and writing all set to it).
//!
//! The volume serial number (BPB and MBR disk signature) is the 32-bit
//! FNV-1a hash of the tree: each object's relative path, kind, size and
//! FAT time stamp in pre-order, then the volume's sector count. The same
//! folder (names, sizes, times, contents) gives byte-identical volumes.
//!
//! Size: `size` rounded up to a MiB, else max(256 MiB, 2 × the content
//! rounded up to a MiB), at most 2 TiB (what the MBR can describe). Files
//! that do not fit are left out.
//!
//! # Memory
//!
//! Only the MBR, the boot region and the directories are in memory; FAT
//! sectors are generated from the list of runs, file data is read from the
//! host files when the guest reads it (each run maps to a file and an
//! offset). Sectors the guest writes are kept in an overlay (sector →
//! bytes). A 2 GiB volume over a few files needs a few kilobytes.
//!
//! # Write-back
//!
//! [`commit`](MediaBackend::commit) reads the guest's file system as it is
//! now (partition table, BPB, the active FAT, the directory tree with long
//! names) and checks it first: every chain stays inside the volume, ends
//! with an end mark, uses no cluster twice (loops, cross links), a file's
//! chain has exactly the clusters its size needs, directories nest at most
//! 64 deep and hold at most 65 536 entries, names are unique (ignoring
//! case) and usable on the host (no `"*/:<>?\|` or control characters, no
//! trailing dot or space, no device names such as `CON` or `LPT1`). If
//! anything fails, the commit is refused with an error and the folder is
//! not touched. Otherwise it compares the tree with the one of the last
//! open or commit:
//! - a file whose first cluster is that of a known file, under the same
//!   path, is that file; under another path it is the same file renamed or
//!   moved, if its contents are unchanged. Then a file at the same path is
//!   the same file. Everything else is new.
//! - New files and folders are created; changed files are rewritten (only
//!   when their bytes differ); renamed and moved files are renamed.
//! - Files that are gone are never deleted: they move into
//!   `<dir>/.mimic-trash/<n>/<their relative path>`, with `n` the first
//!   unused number (1, 2, …). Folders that are gone are removed only when
//!   they are empty after that (they keep anything the host put there).
//! - A file the host changed since the last open or commit (size or
//!   modification time differs, or it is gone) is never overwritten,
//!   moved or put in the trash: the guest's version is written beside it
//!   as `name (mimic).ext` (`name (mimic 2).ext`, … when that exists too),
//!   which is where later changes of that file go. A new file or a renamed
//!   one whose target exists on the host gets such a name as well.
//! - Contents are first written into `<dir>/.mimic-stage`, then moved into
//!   place, so the guest's data is read before any host file changes.
//!   Neither `.mimic-trash` nor `.mimic-stage` appears in the volume.
//!
//! After a commit the guest's tree is the reference for the next one:
//! file runs now map to the host files that hold them, and overlay sectors
//! that the host now holds byte for byte are dropped. The guest's view
//! does not change, except for bytes past a file's end in its last sector
//! and for sectors that held a rewritten file's old contents that the
//! guest has not written since (free space): they read as zero.
//!
//! # Limits
//!
//! Left out when opening (listed by [`FatDirBackend::skipped`]): symbolic
//! links and other non-regular files, files larger than 4 GiB − 1, names
//! FAT cannot hold (see above; also names that are not valid Unicode or
//! that differ from a sibling's only in case), folders deeper than 64
//! levels, entries beyond 65 536 slots a folder, files that do not fit.
//! Host files are read when the guest reads them: changing them on the host
//! while the volume is in use gives the guest inconsistent data (a commit
//! then treats them as changed on the host). A host change that keeps size
//! and modification time is not noticed. An I/O error in the middle of a
//! commit leaves the earlier steps done; no host data is lost by it.
//!
//! Source: Microsoft, *Microsoft Extensible Firmware Initiative FAT32 File
//! System Specification*, version 1.03 (2000): BPB and FSInfo layouts,
//! cluster size table, FAT size formula, FAT entry values, directory
//! entries, date and time format, long name entries and their checksum,
//! the short name generation; MBR partition entries from the IBM PC
//! partition table convention (type `0x0C`); the civil date from a day
//! count after Howard Hinnant, "chrono-Compatible Low-Level Date
//! Algorithms" (public domain).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mimic_core::media::{Media, MediaBackend, MediaError, MediaKind};

const SECTOR: usize = 512;
const SECTOR_U64: u64 = 512;
const MIB: u64 = 1 << 20;
/// First sector of the FAT32 partition.
pub const PARTITION_START: u64 = 2048;
const RESERVED_MIN: u64 = 32;
const MIN_CLUSTERS: u64 = 65_525;
const MAX_CLUSTERS: u64 = 0x0FFF_FFF5;
/// The MBR's 32-bit sector count, after the partition's start.
const MAX_SECTORS: u64 = PARTITION_START + u32::MAX as u64;
/// Smallest default volume size.
pub const DEFAULT_MIN_SIZE: u64 = 256 * MIB;
const MAX_FILE: u64 = 0xFFFF_FFFF;
const MAX_DEPTH: usize = 64;
const MAX_DIR_SLOTS: usize = 65_536;
const EOC: u32 = 0x0FFF_FFFF;
/// Where files the guest deleted go (relative to the folder).
pub const TRASH_DIR: &str = ".mimic-trash";
/// Where a commit puts contents before moving them into place.
pub const STAGE_DIR: &str = ".mimic-stage";
const LABEL: &[u8; 11] = b"MIMIC      ";
const DOT: &[u8; 11] = b".          ";
const DOTDOT: &[u8; 11] = b"..         ";
/// Character positions of a long name entry (13 UTF-16 units).
const LFN_POS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

fn io_err(path: &Path, e: std::io::Error) -> MediaError {
    MediaError::Backend(format!("{}: {e}", path.display()))
}

fn damaged(msg: impl fmt::Display) -> MediaError {
    MediaError::Format(format!(
        "guest FAT file system is damaged, nothing written back: {msg}"
    ))
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// A host object left out of the volume, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    /// Sectors of the whole volume (MBR included).
    sectors: u64,
    spc: u64,
    reserved: u64,
    fat_size: u64,
    clusters: u64,
}

impl Geometry {
    fn new(bytes: u64) -> Result<Self, MediaError> {
        let mut sectors = bytes.div_ceil(MIB) * (MIB / SECTOR_U64);
        loop {
            if sectors > MAX_SECTORS {
                return Err(MediaError::Backend(format!(
                    "a FAT volume of {bytes} bytes is larger than an MBR can describe (2 TiB)"
                )));
            }
            let part = sectors.saturating_sub(PARTITION_START);
            let mut spc = if part <= 532_480 {
                1
            } else if part <= 16_777_216 {
                8
            } else if part <= 33_554_432 {
                16
            } else if part <= 67_108_864 {
                32
            } else {
                64
            };
            loop {
                if part > RESERVED_MIN {
                    let per = (256 * spc + 2) / 2;
                    let fat_size = (part - RESERVED_MIN).div_ceil(per);
                    // The data area starts on a cluster boundary (2048 is a
                    // multiple of every cluster size).
                    let reserved = RESERVED_MIN + (spc - (RESERVED_MIN + 2 * fat_size) % spc) % spc;
                    let data = part.saturating_sub(reserved + 2 * fat_size);
                    let clusters = (data / spc).min(MAX_CLUSTERS);
                    if clusters >= MIN_CLUSTERS && fat_size * 128 >= clusters + 2 {
                        return Ok(Self {
                            sectors,
                            spc,
                            reserved,
                            fat_size,
                            clusters,
                        });
                    }
                }
                if spc == 1 {
                    break;
                }
                spc /= 2;
            }
            sectors += MIB / SECTOR_U64;
        }
    }

    fn part_sectors(&self) -> u64 {
        self.sectors - PARTITION_START
    }

    fn fat_start(&self) -> u64 {
        PARTITION_START + self.reserved
    }

    fn data_start(&self) -> u64 {
        self.fat_start() + 2 * self.fat_size
    }

    fn cluster_sector(&self, c: u64) -> u64 {
        self.data_start() + (c - 2) * self.spc
    }

    fn cluster_bytes(&self) -> u64 {
        self.spc * SECTOR_U64
    }
}

/// Where a run of data sectors comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// `files[idx]` from byte `offset` on.
    File { idx: usize, offset: u64 },
    /// `dirs[idx]` from byte `offset` on.
    Dir { idx: usize, offset: u64 },
}

impl Source {
    fn advance(self, sectors: u64) -> Self {
        let by = sectors * SECTOR_U64;
        match self {
            Source::File { idx, offset } => Source::File {
                idx,
                offset: offset + by,
            },
            Source::Dir { idx, offset } => Source::Dir {
                idx,
                offset: offset + by,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Extent {
    sectors: u64,
    source: Source,
}

#[derive(Debug)]
struct HostFile {
    path: PathBuf,
}

/// What a host file looked like when the reference was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| Stamp {
            len: m.len(),
            modified: m.modified().ok(),
        })
}

/// An object of the reference tree (the last open or commit).
#[derive(Clone, Debug)]
struct RefNode {
    is_dir: bool,
    first: u32,
    size: u32,
    /// Its clusters as (first sector, sectors).
    runs: Vec<(u64, u64)>,
    file: Option<usize>,
    /// Relative host path.
    host: PathBuf,
    stamp: Option<Stamp>,
}

/// An object of the guest's tree as read at commit.
#[derive(Clone, Debug)]
struct GuestNode {
    path: Vec<String>,
    is_dir: bool,
    first: u32,
    size: u32,
    runs: Vec<(u64, u64)>,
}

/// The guest's FAT32 geometry as its BPB tells it.
#[derive(Clone, Copy, Debug)]
struct GuestGeo {
    spc: u64,
    fat_start: u64,
    data_start: u64,
    clusters: u64,
    root: u32,
}

impl GuestGeo {
    fn cluster_sector(&self, c: u32) -> u64 {
        self.data_start + (u64::from(c) - 2) * self.spc
    }
}

struct Walk {
    g: GuestGeo,
    cache: Option<(u64, [u8; SECTOR])>,
    used: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteKind {
    /// Replace the matched host file.
    Overwrite,
    /// A new host file at the desired path, or a `(mimic)` name beside it.
    Fresh,
}

struct FilePlan {
    guest: usize,
    matched: Option<Vec<String>>,
    rename: bool,
    write: Option<WriteKind>,
    desired: PathBuf,
}

struct Plan {
    files: Vec<FilePlan>,
    trash: Vec<Vec<String>>,
    dirs: Vec<(usize, PathBuf)>,
    remove_dirs: Vec<PathBuf>,
}

/// A host folder as a FAT32 volume (see the module documentation).
pub struct FatDirBackend {
    dir: PathBuf,
    geo: Geometry,
    serial: u32,
    mbr: Box<[u8; SECTOR]>,
    boot: Box<[u8; SECTOR]>,
    fsinfo: Box<[u8; SECTOR]>,
    /// The FAT as built: (first cluster, clusters), each one contiguous.
    chains: BTreeMap<u32, u32>,
    /// Data sectors backed by host files or directories: first sector →
    /// run.
    extents: BTreeMap<u64, Extent>,
    dirs: Vec<Vec<u8>>,
    files: Vec<HostFile>,
    overlay: BTreeMap<u64, Box<[u8; SECTOR]>>,
    reference: BTreeMap<Vec<String>, RefNode>,
    skipped: Vec<Skipped>,
    dirty: bool,
    handle: Option<(usize, File)>,
}

impl fmt::Debug for FatDirBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FatDirBackend")
            .field("dir", &self.dir)
            .field("bytes", &self.len())
            .field("sectors_per_cluster", &self.geo.spc)
            .field("serial", &format_args!("{:08X}", self.serial))
            .field("overlay_sectors", &self.overlay.len())
            .field("dirty", &self.dirty)
            .finish()
    }
}

/// A host object found when scanning.
struct Node {
    name: String,
    path: Vec<String>,
    short: [u8; 11],
    lfn: bool,
    time: (u16, u16, u8),
    is_dir: bool,
    size: u32,
    stamp: Option<Stamp>,
    parent: usize,
    children: Vec<usize>,
    /// Directory entries a directory needs.
    slots: usize,
    first: u32,
    clusters: u64,
    skip: bool,
}

fn rel_of(path: &[String]) -> PathBuf {
    path.iter().collect()
}

/// Why `name` cannot be a FAT long name or a host name, if it cannot.
fn name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Some("not a file name");
    }
    if name.encode_utf16().count() > 255 {
        return Some("name longer than 255 characters");
    }
    if name.chars().any(|c| {
        (c as u32) < 0x20 || matches!(c, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|')
    }) {
        return Some("name has a character FAT does not allow");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("name ends with a dot or a space");
    }
    let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    if device {
        return Some("name is a reserved device name");
    }
    None
}

/// A character allowed in short names, upper-cased.
fn short_char(c: char) -> Option<u8> {
    let c = c.to_ascii_uppercase();
    (c.is_ascii_uppercase() || c.is_ascii_digit() || "$%'-_@~`!(){}^#&".contains(c))
        .then_some(c as u8)
}

fn pack_short(base: &[u8], ext: &[u8]) -> [u8; 11] {
    let mut sn = [b' '; 11];
    sn[..base.len().min(8)].copy_from_slice(&base[..base.len().min(8)]);
    sn[8..8 + ext.len().min(3)].copy_from_slice(&ext[..ext.len().min(3)]);
    sn
}

/// `name` itself as a short name: 1–8 characters, optionally a dot and
/// 1–3 more, all allowed in short names and none lower case.
fn exact_short(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    let ok = |s: &str, max: usize| {
        s.len() <= max
            && s.chars()
                .all(|c| !c.is_ascii_lowercase() && short_char(c).is_some())
    };
    if base.is_empty() || !ok(base, 8) || !ok(ext, 3) || (name.contains('.') && ext.is_empty()) {
        return None;
    }
    Some(pack_short(base.as_bytes(), ext.as_bytes()))
}

/// A unique short name for `name` in a directory whose names are `taken`,
/// and whether a long name is needed beside it.
fn make_short(name: &str, taken: &BTreeSet<[u8; 11]>) -> Option<([u8; 11], bool)> {
    if let Some(sn) = exact_short(name) {
        if !taken.contains(&sn) {
            return Some((sn, false));
        }
    }
    let trimmed = name.trim_start_matches('.');
    let mut lossy = trimmed.len() != name.len();
    let (base_s, ext_s) = match trimmed.rfind('.') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => (trimmed, ""),
    };
    let mut convert = |s: &str| -> Vec<u8> {
        s.chars()
            .filter_map(|c| {
                if c == ' ' || c == '.' {
                    lossy = true;
                    None
                } else {
                    Some(short_char(c).unwrap_or_else(|| {
                        lossy = true;
                        b'_'
                    }))
                }
            })
            .collect()
    };
    let mut base = convert(base_s);
    let ext = convert(ext_s);
    if base.len() > 8 || ext.len() > 3 {
        lossy = true;
    }
    if base.is_empty() {
        base.push(b'_');
        lossy = true;
    }
    base.truncate(8);
    if !lossy {
        let sn = pack_short(&base, &ext);
        if !taken.contains(&sn) {
            return Some((sn, true));
        }
    }
    (1..=999_999u32).find_map(|n| {
        let tail = format!("~{n}");
        let mut b = base[..base.len().min(8 - tail.len())].to_vec();
        b.extend_from_slice(tail.as_bytes());
        let sn = pack_short(&b, &ext);
        (!taken.contains(&sn)).then_some((sn, true))
    })
}

/// The long-name checksum of a short name.
fn lfn_checksum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}

/// Long-name entries for `name`, in the order they are stored (the last
/// part first).
fn lfn_entries(name: &str, short: &[u8; 11]) -> Vec<[u8; 32]> {
    let mut units: Vec<u16> = name.encode_utf16().collect();
    let n = units.len().div_ceil(13);
    if !units.len().is_multiple_of(13) {
        units.push(0);
    }
    units.resize(n * 13, 0xFFFF);
    let chk = lfn_checksum(short);
    (0..n)
        .rev()
        .map(|i| {
            let mut e = [0u8; 32];
            e[0] = (i as u8 + 1) | if i == n - 1 { 0x40 } else { 0 };
            e[11] = 0x0F;
            e[13] = chk;
            for (k, &pos) in LFN_POS.iter().enumerate() {
                put16(&mut e, pos, units[i * 13 + k]);
            }
            e
        })
        .collect()
}

fn short_entry(short: &[u8; 11], attr: u8, first: u32, size: u32, t: (u16, u16, u8)) -> [u8; 32] {
    let (date, time, tenth) = t;
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(short);
    e[11] = attr;
    e[13] = tenth;
    put16(&mut e, 14, time);
    put16(&mut e, 16, date);
    put16(&mut e, 18, date);
    put16(&mut e, 20, (first >> 16) as u16);
    put16(&mut e, 22, time);
    put16(&mut e, 24, date);
    put16(&mut e, 26, first as u16);
    put32(&mut e, 28, size);
    e
}

/// Year, month, day of a day count since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// FAT date, time and tenths for a host time (UTC), clamped to 1980–2107.
fn fat_time(t: Option<SystemTime>) -> (u16, u16, u8) {
    const MIN: (u16, u16, u8) = ((1 << 5) | 1, 0, 0);
    const MAX: (u16, u16, u8) = (
        (127 << 9) | (12 << 5) | 31,
        (23 << 11) | (59 << 5) | 29,
        100,
    );
    let Some(secs) = t
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
    else {
        return MIN;
    };
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    if y < 1980 {
        return MIN;
    }
    if y > 2107 {
        return MAX;
    }
    let s = secs % 86_400;
    let date = (((y - 1980) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let time =
        (((s / 3600) as u16) << 11) | ((((s / 60) % 60) as u16) << 5) | ((s % 60) / 2) as u16;
    (date, time, ((s % 2) * 100) as u8)
}

/// CHS of a sector for the MBR (255 heads, 63 sectors a track).
fn chs(lba: u64) -> [u8; 3] {
    let c = lba / (255 * 63);
    if c > 1023 {
        return [0xFE, 0xFF, 0xFF];
    }
    let h = (lba / 63) % 255;
    let s = lba % 63 + 1;
    [h as u8, (s as u8) | (((c >> 2) & 0xC0) as u8), c as u8]
}

fn fnv1a(hash: &mut u32, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u32::from(b);
        *hash = hash.wrapping_mul(0x0100_0193);
    }
}

/// Read the host folder under `nodes[at]` into `nodes`.
fn scan(
    base: &Path,
    at: usize,
    nodes: &mut Vec<Node>,
    skipped: &mut Vec<Skipped>,
    depth: usize,
) -> Result<(), MediaError> {
    let abs = base.join(rel_of(&nodes[at].path));
    let rd = match fs::read_dir(&abs) {
        Ok(rd) => rd,
        Err(e) if at == 0 => return Err(io_err(&abs, e)),
        Err(e) => {
            nodes[at].skip = true;
            skipped.push(Skipped {
                path: abs,
                reason: e.to_string(),
            });
            return Ok(());
        }
    };
    let mut entries = Vec::new();
    for e in rd {
        match e {
            Ok(e) => match e.file_name().into_string() {
                Ok(name) => entries.push((name, e.path())),
                Err(_) => skipped.push(Skipped {
                    path: e.path(),
                    reason: "name is not valid Unicode".into(),
                }),
            },
            Err(e) => skipped.push(Skipped {
                path: abs.clone(),
                reason: e.to_string(),
            }),
        }
    }
    entries.sort();
    let mut folded: BTreeMap<String, String> = BTreeMap::new();
    let mut taken = BTreeSet::new();
    for (name, path) in entries {
        if at == 0 && (name.eq_ignore_ascii_case(TRASH_DIR) || name.eq_ignore_ascii_case(STAGE_DIR))
        {
            continue;
        }
        let mut skip = |reason: String| {
            skipped.push(Skipped {
                path: path.clone(),
                reason,
            })
        };
        if let Some(why) = name_problem(&name) {
            skip(why.into());
            continue;
        }
        let lower = name.to_lowercase();
        if let Some(other) = folded.get(&lower) {
            skip(format!("differs from '{other}' only in case"));
            continue;
        }
        let md = match fs::symlink_metadata(&path) {
            Ok(md) => md,
            Err(e) => {
                skip(e.to_string());
                continue;
            }
        };
        let ft = md.file_type();
        if ft.is_symlink() {
            skip("symbolic link".into());
            continue;
        }
        let is_dir = ft.is_dir();
        if !is_dir && !ft.is_file() {
            skip("not a regular file or folder".into());
            continue;
        }
        if is_dir && depth + 1 > MAX_DEPTH {
            skip(format!("nested deeper than {MAX_DEPTH} folders"));
            continue;
        }
        if !is_dir && md.len() > MAX_FILE {
            skip("larger than 4 GiB - 1".into());
            continue;
        }
        let Some((short, lfn)) = make_short(&name, &taken) else {
            skip("no free short name".into());
            continue;
        };
        let slots = 1 + if lfn {
            name.encode_utf16().count().div_ceil(13)
        } else {
            0
        };
        if nodes[at].slots + slots > MAX_DIR_SLOTS {
            skip("folder has too many entries for FAT".into());
            continue;
        }
        taken.insert(short);
        folded.insert(lower, name.clone());
        nodes[at].slots += slots;
        let mut p = nodes[at].path.clone();
        p.push(name.clone());
        let idx = nodes.len();
        nodes.push(Node {
            name,
            path: p,
            short,
            lfn,
            time: fat_time(md.modified().ok()),
            is_dir,
            size: if is_dir { 0 } else { md.len() as u32 },
            stamp: (!is_dir).then(|| Stamp {
                len: md.len(),
                modified: md.modified().ok(),
            }),
            parent: at,
            children: Vec::new(),
            slots: 2,
            first: 0,
            clusters: 0,
            skip: false,
        });
        nodes[at].children.push(idx);
        if is_dir {
            scan(base, idx, nodes, skipped, depth + 1)?;
        }
    }
    Ok(())
}

/// Nodes in pre-order (children sorted by name), skipped subtrees left
/// out.
fn preorder(nodes: &[Node]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![0];
    while let Some(i) = stack.pop() {
        if nodes[i].skip {
            continue;
        }
        out.push(i);
        stack.extend(nodes[i].children.iter().rev());
    }
    out
}

/// `rel`, or `name (mimic).ext`, `name (mimic 2).ext` … beside it when
/// something exists there.
fn free_name(dir: &Path, rel: &Path) -> PathBuf {
    if fs::symlink_metadata(dir.join(rel)).is_err() {
        return rel.to_path_buf();
    }
    let name = rel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let parent = rel.parent().map(Path::to_path_buf).unwrap_or_default();
    (1u32..)
        .map(|n| {
            let tag = if n == 1 {
                " (mimic)".to_string()
            } else {
                format!(" (mimic {n})")
            };
            parent.join(format!("{stem}{tag}{ext}"))
        })
        .find(|c| fs::symlink_metadata(dir.join(c)).is_err())
        .unwrap_or_else(|| rel.to_path_buf())
}

fn make_parents(path: &Path) -> Result<(), MediaError> {
    match path.parent() {
        Some(p) => fs::create_dir_all(p).map_err(|e| io_err(p, e)),
        None => Ok(()),
    }
}

fn move_file(from: &Path, to: &Path) -> Result<(), MediaError> {
    make_parents(to)?;
    fs::rename(from, to).map_err(|e| io_err(from, e))
}

/// A name from a short entry (`case`: the NT lower-case bits of byte 12).
fn short_display(short: &[u8; 11], case: u8) -> String {
    let part = |b: &[u8], lower: bool| -> String {
        let end = b.iter().rposition(|&c| c != b' ').map_or(0, |i| i + 1);
        b[..end]
            .iter()
            .map(|&c| match c {
                0x20..=0x7E if lower => (c as char).to_ascii_lowercase(),
                0x20..=0x7E => c as char,
                _ => '_',
            })
            .collect()
    };
    let base = part(&short[..8], case & 0x08 != 0);
    let ext = part(&short[8..], case & 0x10 != 0);
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

impl FatDirBackend {
    /// Build the volume for `dir` (see the module documentation); `size`
    /// in bytes, `None` for the default.
    pub fn open(dir: &Path, size: Option<u64>) -> Result<Self, MediaError> {
        let md = fs::metadata(dir).map_err(|e| io_err(dir, e))?;
        if !md.is_dir() {
            return Err(MediaError::Backend(format!(
                "{}: not a folder",
                dir.display()
            )));
        }
        let mut nodes = vec![Node {
            name: String::new(),
            path: Vec::new(),
            short: [b' '; 11],
            lfn: false,
            time: (0, 0, 0),
            is_dir: true,
            size: 0,
            stamp: None,
            parent: 0,
            children: Vec::new(),
            slots: 1,
            first: 0,
            clusters: 0,
            skip: false,
        }];
        let mut skipped = Vec::new();
        scan(dir, 0, &mut nodes, &mut skipped, 0)?;

        let round = |v: u64| v.div_ceil(4096) * 4096;
        let content: u64 = preorder(&nodes)
            .iter()
            .map(|&i| {
                let n = &nodes[i];
                if n.is_dir {
                    round(n.slots as u64 * 32)
                } else {
                    round(u64::from(n.size))
                }
            })
            .sum();
        let bytes = size.unwrap_or_else(|| {
            DEFAULT_MIN_SIZE
                .max(content.saturating_mul(2).div_ceil(MIB) * MIB)
                .min(MAX_SECTORS * SECTOR_U64 / MIB * MIB)
        });
        let geo = Geometry::new(bytes)?;

        // Directories first, then files, each contiguous.
        let order = preorder(&nodes);
        let cb = geo.cluster_bytes();
        let limit = geo.clusters + 2;
        let mut next = 2u64;
        for &i in &order {
            if nodes[i].is_dir {
                let n = (nodes[i].slots as u64 * 32).div_ceil(cb).max(1);
                if next + n > limit {
                    return Err(MediaError::Backend(format!(
                        "{}: the folders do not fit into a volume of {} bytes",
                        dir.display(),
                        geo.sectors * SECTOR_U64
                    )));
                }
                nodes[i].first = next as u32;
                nodes[i].clusters = n;
                next += n;
            }
        }
        for &i in &order {
            if nodes[i].is_dir {
                continue;
            }
            let n = u64::from(nodes[i].size).div_ceil(cb);
            if n == 0 {
                continue;
            }
            if next + n > limit {
                nodes[i].skip = true;
                skipped.push(Skipped {
                    path: dir.join(rel_of(&nodes[i].path)),
                    reason: "does not fit into the volume".into(),
                });
                continue;
            }
            nodes[i].first = next as u32;
            nodes[i].clusters = n;
            next += n;
        }

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let mut extents = BTreeMap::new();
        let mut chains = BTreeMap::new();
        let mut reference = BTreeMap::new();
        let mut serial = 0x811C_9DC5u32;
        for &i in &order {
            let n = &nodes[i];
            if n.skip {
                continue;
            }
            fnv1a(&mut serial, n.path.join("/").as_bytes());
            fnv1a(&mut serial, &[0, u8::from(n.is_dir)]);
            fnv1a(&mut serial, &n.size.to_le_bytes());
            fnv1a(&mut serial, &n.time.0.to_le_bytes());
            fnv1a(&mut serial, &n.time.1.to_le_bytes());
            if n.is_dir {
                let mut buf = vec![0u8; (n.clusters * cb) as usize];
                let mut entries = Vec::new();
                if i == 0 {
                    entries.push(short_entry(LABEL, 0x08, 0, 0, (0, 0, 0)));
                } else {
                    let up = if n.parent == 0 {
                        0
                    } else {
                        nodes[n.parent].first
                    };
                    entries.push(short_entry(DOT, 0x10, n.first, 0, n.time));
                    entries.push(short_entry(DOTDOT, 0x10, up, 0, n.time));
                }
                for &c in &n.children {
                    let ch = &nodes[c];
                    if ch.skip {
                        continue;
                    }
                    if ch.lfn {
                        entries.extend(lfn_entries(&ch.name, &ch.short));
                    }
                    let attr = if ch.is_dir { 0x10 } else { 0x20 };
                    entries.push(short_entry(&ch.short, attr, ch.first, ch.size, ch.time));
                }
                for (k, e) in entries.iter().enumerate() {
                    buf[k * 32..k * 32 + 32].copy_from_slice(e);
                }
                let idx = dirs.len();
                dirs.push(buf);
                let start = geo.cluster_sector(u64::from(n.first));
                let sectors = n.clusters * geo.spc;
                extents.insert(
                    start,
                    Extent {
                        sectors,
                        source: Source::Dir { idx, offset: 0 },
                    },
                );
                chains.insert(n.first, n.clusters as u32);
                if i != 0 {
                    reference.insert(
                        n.path.clone(),
                        RefNode {
                            is_dir: true,
                            first: n.first,
                            size: 0,
                            runs: vec![(start, sectors)],
                            file: None,
                            host: rel_of(&n.path),
                            stamp: None,
                        },
                    );
                }
            } else {
                let idx = files.len();
                files.push(HostFile {
                    path: dir.join(rel_of(&n.path)),
                });
                let mut runs = Vec::new();
                if n.first != 0 {
                    let start = geo.cluster_sector(u64::from(n.first));
                    extents.insert(
                        start,
                        Extent {
                            sectors: u64::from(n.size).div_ceil(SECTOR_U64),
                            source: Source::File { idx, offset: 0 },
                        },
                    );
                    chains.insert(n.first, n.clusters as u32);
                    runs.push((start, n.clusters * geo.spc));
                }
                reference.insert(
                    n.path.clone(),
                    RefNode {
                        is_dir: false,
                        first: n.first,
                        size: n.size,
                        runs,
                        file: Some(idx),
                        host: rel_of(&n.path),
                        stamp: n.stamp,
                    },
                );
            }
        }
        fnv1a(&mut serial, &geo.sectors.to_le_bytes());

        let free = geo.clusters + 2 - next;
        let mut be = Self {
            dir: dir.to_path_buf(),
            geo,
            serial,
            mbr: Box::new([0; SECTOR]),
            boot: Box::new([0; SECTOR]),
            fsinfo: Box::new([0; SECTOR]),
            chains,
            extents,
            dirs,
            files,
            overlay: BTreeMap::new(),
            reference,
            skipped,
            dirty: false,
            handle: None,
        };
        be.build_boot_region(free as u32, next as u32);
        Ok(be)
    }

    fn build_boot_region(&mut self, free: u32, next_free: u32) {
        let g = self.geo;
        let m = &mut self.mbr;
        put32(&mut m[..], 440, self.serial);
        let p = 446;
        m[p] = 0x00;
        m[p + 1..p + 4].copy_from_slice(&chs(PARTITION_START));
        m[p + 4] = 0x0C;
        m[p + 5..p + 8].copy_from_slice(&chs(g.sectors - 1));
        put32(&mut m[..], p + 8, PARTITION_START as u32);
        put32(&mut m[..], p + 12, g.part_sectors() as u32);
        m[510] = 0x55;
        m[511] = 0xAA;

        let b = &mut self.boot;
        b[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
        b[3..11].copy_from_slice(b"MIMIC   ");
        put16(&mut b[..], 11, SECTOR as u16);
        b[13] = g.spc as u8;
        put16(&mut b[..], 14, g.reserved as u16);
        b[16] = 2;
        b[21] = 0xF8;
        put16(&mut b[..], 24, 63);
        put16(&mut b[..], 26, 255);
        put32(&mut b[..], 28, PARTITION_START as u32);
        put32(&mut b[..], 32, g.part_sectors() as u32);
        put32(&mut b[..], 36, g.fat_size as u32);
        put32(&mut b[..], 44, 2);
        put16(&mut b[..], 48, 1);
        put16(&mut b[..], 50, 6);
        b[64] = 0x80;
        b[66] = 0x29;
        put32(&mut b[..], 67, self.serial);
        b[71..82].copy_from_slice(LABEL);
        b[82..90].copy_from_slice(b"FAT32   ");
        // Not bootable: halt.
        b[90..93].copy_from_slice(&[0xF4, 0xEB, 0xFD]);
        b[510] = 0x55;
        b[511] = 0xAA;

        let f = &mut self.fsinfo;
        put32(&mut f[..], 0, 0x4161_5252);
        put32(&mut f[..], 484, 0x6141_7272);
        put32(&mut f[..], 488, free);
        put32(&mut f[..], 492, next_free);
        put32(&mut f[..], 508, 0xAA55_0000);
    }

    /// Host objects left out of the volume.
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// The host folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The volume serial number.
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// Bytes held in memory for the volume (directories, overlay, runs),
    /// roughly.
    pub fn memory_use(&self) -> usize {
        self.dirs.iter().map(Vec::len).sum::<usize>()
            + self.overlay.len() * (SECTOR + 32)
            + (self.extents.len() + self.chains.len()) * 48
            + self.reference.len() * 128
            + 3 * SECTOR
    }

    fn sector(&mut self, s: u64) -> Result<[u8; SECTOR], MediaError> {
        if let Some(b) = self.overlay.get(&s) {
            return Ok(**b);
        }
        let mut out = [0u8; SECTOR];
        self.base_sector(s, &mut out)?;
        Ok(out)
    }

    /// A sector as built (without the guest's writes).
    fn base_sector(&mut self, s: u64, out: &mut [u8; SECTOR]) -> Result<(), MediaError> {
        out.fill(0);
        let run = self
            .extents
            .range(..=s)
            .next_back()
            .filter(|(&start, e)| s < start + e.sectors)
            .map(|(&start, e)| e.source.advance(s - start));
        match run {
            Some(Source::File { idx, offset }) => return self.read_host(idx, offset, out),
            Some(Source::Dir { idx, offset }) => {
                let d = &self.dirs[idx];
                let o = offset as usize;
                if o < d.len() {
                    let n = (d.len() - o).min(SECTOR);
                    out[..n].copy_from_slice(&d[o..o + n]);
                }
                return Ok(());
            }
            None => {}
        }
        let g = self.geo;
        if s == 0 {
            out.copy_from_slice(&self.mbr[..]);
        } else if s >= PARTITION_START && s < g.fat_start() {
            match s - PARTITION_START {
                0 | 6 => out.copy_from_slice(&self.boot[..]),
                1 | 7 => out.copy_from_slice(&self.fsinfo[..]),
                2 | 8 => {
                    out[510] = 0x55;
                    out[511] = 0xAA;
                }
                _ => {}
            }
        } else if s >= g.fat_start() && s < g.data_start() {
            let first = ((s - g.fat_start()) % g.fat_size) * 128;
            for k in 0..128u64 {
                let c = first + k;
                let v = match c {
                    0 => 0x0FFF_FFF8,
                    1 => EOC,
                    _ if c >= g.clusters + 2 => 0,
                    _ => self
                        .chains
                        .range(..=(c as u32))
                        .next_back()
                        .filter(|(&st, &n)| c < u64::from(st) + u64::from(n))
                        .map_or(0, |(&st, &n)| {
                            if c + 1 == u64::from(st) + u64::from(n) {
                                EOC
                            } else {
                                c as u32 + 1
                            }
                        }),
                };
                put32(out, (k * 4) as usize, v);
            }
        }
        Ok(())
    }

    /// 512 bytes of `files[idx]` from `offset`; past its end (or when it is
    /// gone) zero.
    fn read_host(
        &mut self,
        idx: usize,
        offset: u64,
        out: &mut [u8; SECTOR],
    ) -> Result<(), MediaError> {
        if !matches!(&self.handle, Some((i, _)) if *i == idx) {
            self.handle = None;
            let path = &self.files[idx].path;
            match File::open(path) {
                Ok(f) => self.handle = Some((idx, f)),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(io_err(path, e)),
            }
        }
        let Some((_, f)) = self.handle.as_mut() else {
            return Ok(());
        };
        let path = &self.files[idx].path;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| io_err(path, e))?;
        let mut got = 0;
        while got < SECTOR {
            match f.read(&mut out[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(io_err(path, e)),
            }
        }
        Ok(())
    }

    fn check(&self, offset: u64, n: usize) -> Result<(), MediaError> {
        match offset.checked_add(n as u64) {
            Some(end) if end <= self.len() => Ok(()),
            _ => Err(MediaError::OutOfRange),
        }
    }

    /// Point `count` sectors from `start` at `source`, cutting what was
    /// there.
    fn set_extent(&mut self, start: u64, count: u64, source: Source) {
        let end = start + count;
        if let Some((&s, &e)) = self.extents.range(..start).next_back() {
            if s + e.sectors > start {
                self.extents.insert(
                    s,
                    Extent {
                        sectors: start - s,
                        source: e.source,
                    },
                );
                if s + e.sectors > end {
                    self.extents.insert(
                        end,
                        Extent {
                            sectors: s + e.sectors - end,
                            source: e.source.advance(end - s),
                        },
                    );
                }
            }
        }
        let inside: Vec<u64> = self.extents.range(start..end).map(|(&s, _)| s).collect();
        for s in inside {
            if let Some(e) = self.extents.remove(&s) {
                if s + e.sectors > end {
                    self.extents.insert(
                        end,
                        Extent {
                            sectors: s + e.sectors - end,
                            source: e.source.advance(end - s),
                        },
                    );
                }
            }
        }
        self.extents.insert(
            start,
            Extent {
                sectors: count,
                source,
            },
        );
    }

    // ---- reading the guest's file system -------------------------------

    fn guest_geometry(&mut self) -> Result<GuestGeo, MediaError> {
        let total = self.geo.sectors;
        let s0 = self.sector(0)?;
        let is_bpb = |b: &[u8; SECTOR]| {
            b[510] == 0x55
                && b[511] == 0xAA
                && &b[82..87] == b"FAT32"
                && le16(b, 11) == 512
                && b[13].is_power_of_two()
        };
        let (start, count) = if is_bpb(&s0) {
            (0, total)
        } else {
            if s0[510] != 0x55 || s0[511] != 0xAA {
                return Err(damaged("no partition table"));
            }
            let entry = (0..4)
                .map(|i| 446 + 16 * i)
                .find(|&p| matches!(s0[p + 4], 0x0B | 0x0C))
                .ok_or_else(|| damaged("no FAT32 partition"))?;
            let start = u64::from(le32(&s0, entry + 8));
            let count = u64::from(le32(&s0, entry + 12));
            if start == 0 || start + count > total {
                return Err(damaged("partition outside the volume"));
            }
            (start, count)
        };
        let b = self.sector(start)?;
        if !is_bpb(&b) {
            return Err(damaged("no FAT32 boot sector"));
        }
        let spc = u64::from(b[13]);
        let reserved = u64::from(le16(&b, 14));
        let nfats = u64::from(b[16]);
        let tot = u64::from(le32(&b, 32));
        let fat_size = u64::from(le32(&b, 36));
        let ext = le16(&b, 40);
        let root = le32(&b, 44);
        if reserved == 0 || !(1..=2).contains(&nfats) || le16(&b, 17) != 0 || le16(&b, 22) != 0 {
            return Err(damaged("boot sector is not FAT32"));
        }
        if tot == 0 || tot > count || fat_size == 0 {
            return Err(damaged("boot sector sizes outside the partition"));
        }
        let meta = reserved + nfats * fat_size;
        if meta >= tot {
            return Err(damaged("no data area"));
        }
        let clusters = (tot - meta) / spc;
        if !(MIN_CLUSTERS..=MAX_CLUSTERS).contains(&clusters) {
            return Err(damaged(format!("{clusters} clusters is not FAT32")));
        }
        if fat_size * 128 < clusters + 2 {
            return Err(damaged("FAT too small for the clusters"));
        }
        let active = if ext & 0x80 != 0 {
            u64::from(ext & 0x0F)
        } else {
            0
        };
        if active >= nfats {
            return Err(damaged("active FAT does not exist"));
        }
        if root < 2 || u64::from(root) >= clusters + 2 {
            return Err(damaged("root directory cluster outside the volume"));
        }
        Ok(GuestGeo {
            spc,
            fat_start: start + reserved + active * fat_size,
            data_start: start + meta,
            clusters,
            root,
        })
    }

    fn fat_entry(&mut self, w: &mut Walk, c: u32) -> Result<u32, MediaError> {
        let s = w.g.fat_start + u64::from(c) / 128;
        if w.cache.as_ref().is_none_or(|(cs, _)| *cs != s) {
            w.cache = Some((s, self.sector(s)?));
        }
        let Some((_, b)) = &w.cache else {
            return Err(damaged("FAT unreadable"));
        };
        Ok(le32(b, (c as usize % 128) * 4) & 0x0FFF_FFFF)
    }

    /// The runs (first sector, sectors) and cluster count of the chain from
    /// `first`, marking its clusters used.
    fn chain(
        &mut self,
        w: &mut Walk,
        first: u32,
        what: &str,
    ) -> Result<(Vec<(u64, u64)>, u64), MediaError> {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut c = first;
        let mut n = 0u64;
        loop {
            if c < 2 || u64::from(c) >= w.g.clusters + 2 {
                return Err(damaged(format!("{what}: cluster {c} outside the volume")));
            }
            let (word, bit) = (c as usize / 64, c % 64);
            if w.used[word] & (1 << bit) != 0 {
                return Err(damaged(format!("{what}: cluster {c} used twice")));
            }
            w.used[word] |= 1 << bit;
            n += 1;
            let s = w.g.cluster_sector(c);
            match runs.last_mut() {
                Some((start, count)) if *start + *count == s => *count += w.g.spc,
                _ => runs.push((s, w.g.spc)),
            }
            let v = self.fat_entry(w, c)?;
            if v >= 0x0FFF_FFF8 {
                return Ok((runs, n));
            }
            if v == 0x0FFF_FFF7 {
                return Err(damaged(format!("{what}: bad cluster in its chain")));
            }
            c = v;
        }
    }

    /// The entries of a directory: (name, is_dir, first cluster, size).
    fn dir_entries(
        &mut self,
        runs: &[(u64, u64)],
    ) -> Result<Vec<(String, bool, u32, u32)>, MediaError> {
        struct Long {
            next: u8,
            chk: u8,
            units: Vec<u16>,
        }
        let mut out = Vec::new();
        let mut long: Option<Long> = None;
        for &(start, count) in runs {
            for s in start..start + count {
                let sec = self.sector(s)?;
                for e in sec.as_chunks::<32>().0 {
                    if e[0] == 0 {
                        return Ok(out);
                    }
                    if e[0] == 0xE5 {
                        long = None;
                        continue;
                    }
                    let attr = e[11];
                    if attr & 0x3F == 0x0F {
                        let ord = e[0] & 0x1F;
                        if e[0] & 0x40 != 0 {
                            long = (1..=20).contains(&ord).then(|| Long {
                                next: ord,
                                chk: e[13],
                                units: vec![0; usize::from(ord) * 13],
                            });
                        }
                        match &mut long {
                            Some(l) if ord > 0 && l.next == ord && l.chk == e[13] => {
                                let at = usize::from(ord - 1) * 13;
                                for (k, &pos) in LFN_POS.iter().enumerate() {
                                    l.units[at + k] = le16(e, pos);
                                }
                                l.next -= 1;
                            }
                            _ => long = None,
                        }
                        continue;
                    }
                    let l = long.take();
                    if attr & 0x08 != 0 {
                        continue;
                    }
                    let mut short = [0u8; 11];
                    short.copy_from_slice(&e[..11]);
                    if short[0] == 0x05 {
                        short[0] = 0xE5;
                    }
                    if &short == DOT || &short == DOTDOT {
                        continue;
                    }
                    let name = l
                        .filter(|l| l.next == 0 && l.chk == lfn_checksum(&short))
                        .and_then(|l| {
                            let end = l
                                .units
                                .iter()
                                .position(|&u| u == 0)
                                .unwrap_or(l.units.len());
                            String::from_utf16(&l.units[..end]).ok()
                        })
                        .unwrap_or_else(|| short_display(&short, e[12]));
                    let first = (u32::from(le16(e, 20)) << 16) | u32::from(le16(e, 26));
                    out.push((name, attr & 0x10 != 0, first, le32(e, 28)));
                }
            }
        }
        Ok(out)
    }

    /// The guest's tree, sorted by path; refuses a damaged one.
    fn parse_guest(&mut self) -> Result<Vec<GuestNode>, MediaError> {
        let g = self.guest_geometry()?;
        let mut w = Walk {
            g,
            cache: None,
            used: vec![0; ((g.clusters + 2) as usize).div_ceil(64)],
        };
        let cb = g.spc * SECTOR_U64;
        let mut out = Vec::new();
        let mut stack = vec![(g.root, Vec::<String>::new(), 0usize)];
        while let Some((first, path, depth)) = stack.pop() {
            let shown = if path.is_empty() {
                "/".to_string()
            } else {
                path.join("/")
            };
            let (runs, n) = self.chain(&mut w, first, &shown)?;
            if n * cb > (MAX_DIR_SLOTS * 32) as u64 {
                return Err(damaged(format!(
                    "{shown}: directory larger than FAT allows"
                )));
            }
            let mut names = BTreeSet::new();
            for (name, is_dir, first, size) in self.dir_entries(&runs)? {
                if path.is_empty()
                    && (name.eq_ignore_ascii_case(TRASH_DIR)
                        || name.eq_ignore_ascii_case(STAGE_DIR))
                {
                    continue;
                }
                let mut p = path.clone();
                p.push(name.clone());
                let shown = p.join("/");
                if let Some(why) = name_problem(&name) {
                    return Err(MediaError::Format(format!(
                        "guest file '{shown}': {why}; nothing written back"
                    )));
                }
                if !names.insert(name.to_lowercase()) {
                    return Err(damaged(format!("'{shown}' appears twice")));
                }
                if is_dir {
                    if depth + 1 > MAX_DEPTH {
                        return Err(damaged(format!("'{shown}' nested too deeply")));
                    }
                    if first < 2 {
                        return Err(damaged(format!("'{shown}' has no cluster")));
                    }
                    stack.push((first, p.clone(), depth + 1));
                    out.push(GuestNode {
                        path: p,
                        is_dir: true,
                        first,
                        size: 0,
                        runs: Vec::new(),
                    });
                } else {
                    let (runs, n) = if first == 0 {
                        (Vec::new(), 0)
                    } else {
                        self.chain(&mut w, first, &shown)?
                    };
                    if size > 0 && n != u64::from(size).div_ceil(cb) {
                        return Err(damaged(format!("'{shown}': {n} clusters for {size} bytes")));
                    }
                    out.push(GuestNode {
                        path: p,
                        is_dir: false,
                        first,
                        size,
                        runs,
                    });
                }
            }
        }
        // Directory runs for the reference.
        for node in out.iter_mut().filter(|n| n.is_dir) {
            let mut c = node.first;
            let mut runs: Vec<(u64, u64)> = Vec::new();
            loop {
                let s = g.cluster_sector(c);
                match runs.last_mut() {
                    Some((start, count)) if *start + *count == s => *count += g.spc,
                    _ => runs.push((s, g.spc)),
                }
                let v = self.fat_entry(&mut w, c)?;
                if !(2..0x0FFF_FFF8).contains(&v) {
                    break;
                }
                c = v;
            }
            node.runs = runs;
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    // ---- write-back ----------------------------------------------------

    fn runs_clean(&self, runs: &[(u64, u64)]) -> bool {
        runs.iter()
            .all(|&(s, n)| self.overlay.range(s..s + n).next().is_none())
    }

    /// The guest file holds the bytes of the reference file's host file.
    fn same_content(&mut self, g: &GuestNode, r: &RefNode) -> Result<bool, MediaError> {
        if g.size != r.size {
            return Ok(false);
        }
        if g.runs == r.runs && self.runs_clean(&g.runs) {
            return Ok(true);
        }
        let Some(idx) = r.file else {
            return Ok(false);
        };
        let path = self.files[idx].path.clone();
        let Ok(mut f) = File::open(&path) else {
            return Ok(false);
        };
        if f.metadata().map(|m| m.len()).ok() != Some(u64::from(g.size)) {
            return Ok(false);
        }
        let mut left = u64::from(g.size);
        let mut host = [0u8; SECTOR];
        for &(start, count) in &g.runs {
            for s in start..start + count {
                if left == 0 {
                    break;
                }
                let n = left.min(SECTOR_U64) as usize;
                let sec = self.sector(s)?;
                if f.read_exact(&mut host[..n]).is_err() || sec[..n] != host[..n] {
                    return Ok(false);
                }
                left -= n as u64;
            }
        }
        Ok(left == 0)
    }

    fn host_ok(&self, r: &RefNode) -> bool {
        r.stamp.is_some() && stamp_of(&self.dir.join(&r.host)) == r.stamp
    }

    fn plan(&mut self, guest: &[GuestNode]) -> Result<Plan, MediaError> {
        let mut by_cluster: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for (k, r) in &self.reference {
            if !r.is_dir && r.first != 0 {
                by_cluster.insert(r.first, k.clone());
            }
        }
        let mut matched: Vec<Option<Vec<String>>> = vec![None; guest.len()];
        let mut equal = vec![false; guest.len()];
        let mut taken: BTreeSet<Vec<String>> = BTreeSet::new();
        // Same first cluster: the same file, moved only when unchanged.
        for (i, g) in guest.iter().enumerate() {
            if g.is_dir || g.first == 0 {
                continue;
            }
            let Some(k) = by_cluster.get(&g.first).cloned() else {
                continue;
            };
            if taken.contains(&k) {
                continue;
            }
            let r = self.reference[&k].clone();
            if k == g.path || self.same_content(g, &r)? {
                equal[i] = k != g.path;
                matched[i] = Some(k.clone());
                taken.insert(k);
            }
        }
        // Same path.
        for (i, g) in guest.iter().enumerate() {
            if g.is_dir || matched[i].is_some() || taken.contains(&g.path) {
                continue;
            }
            if self.reference.get(&g.path).is_some_and(|r| !r.is_dir) {
                matched[i] = Some(g.path.clone());
                taken.insert(g.path.clone());
            }
        }
        let ok: BTreeMap<Vec<String>, bool> = self
            .reference
            .iter()
            .filter(|(_, r)| !r.is_dir)
            .map(|(k, r)| (k.clone(), self.host_ok(r)))
            .collect();
        let mut trash = Vec::new();
        let mut vacated = BTreeSet::new();
        for (k, r) in &self.reference {
            if !r.is_dir && !taken.contains(k) && ok[k] {
                trash.push(k.clone());
                vacated.insert(r.host.clone());
            }
        }
        for (g, m) in guest.iter().zip(&matched) {
            if let Some(k) = m {
                if *k != g.path && ok[k] {
                    vacated.insert(self.reference[k].host.clone());
                }
            }
        }

        let mut dir_host: BTreeMap<Vec<String>, PathBuf> = BTreeMap::new();
        dir_host.insert(Vec::new(), PathBuf::new());
        let mut dirs = Vec::new();
        for (i, g) in guest.iter().enumerate() {
            if !g.is_dir {
                continue;
            }
            let (name, parent) = g
                .path
                .split_last()
                .ok_or_else(|| damaged("unnamed folder"))?;
            let parent = dir_host
                .get(parent)
                .cloned()
                .ok_or_else(|| damaged("folder without parent"))?;
            let host = match self.reference.get(&g.path) {
                Some(r) if r.is_dir => r.host.clone(),
                _ => {
                    let cand = parent.join(name);
                    match fs::symlink_metadata(self.dir.join(&cand)) {
                        Ok(m) if !m.is_dir() && !vacated.contains(&cand) => {
                            free_name(&self.dir, &cand)
                        }
                        _ => cand,
                    }
                }
            };
            dir_host.insert(g.path.clone(), host.clone());
            dirs.push((i, host));
        }

        let mut files = Vec::new();
        for (i, g) in guest.iter().enumerate() {
            if g.is_dir {
                continue;
            }
            let (name, parent) = g.path.split_last().ok_or_else(|| damaged("unnamed file"))?;
            let parent = dir_host
                .get(parent)
                .cloned()
                .ok_or_else(|| damaged("file without folder"))?;
            let (rename, write, desired) = match matched[i].clone() {
                Some(k) => {
                    let r = self.reference[&k].clone();
                    let host_ok = ok[&k];
                    let desired = if k == g.path {
                        r.host.clone()
                    } else {
                        parent.join(name)
                    };
                    let changed = !(equal[i] || self.same_content(g, &r)?);
                    if desired != r.host {
                        if host_ok {
                            (true, changed.then_some(WriteKind::Overwrite), desired)
                        } else {
                            (false, Some(WriteKind::Fresh), desired)
                        }
                    } else if !changed {
                        (false, None, desired)
                    } else if host_ok {
                        (false, Some(WriteKind::Overwrite), desired)
                    } else {
                        (false, Some(WriteKind::Fresh), desired)
                    }
                }
                None => (false, Some(WriteKind::Fresh), parent.join(name)),
            };
            files.push(FilePlan {
                guest: i,
                matched: matched[i].clone(),
                rename,
                write,
                desired,
            });
        }

        let kept: BTreeSet<&PathBuf> = dirs.iter().map(|(_, h)| h).collect();
        let mut remove_dirs: Vec<PathBuf> = self
            .reference
            .iter()
            .filter(|(k, r)| r.is_dir && !dir_host.contains_key(*k) && !kept.contains(&r.host))
            .map(|(_, r)| r.host.clone())
            .collect();
        remove_dirs.sort_by_key(|p| Reverse(p.components().count()));
        Ok(Plan {
            files,
            trash,
            dirs,
            remove_dirs,
        })
    }

    /// Write a guest file's bytes into `path`.
    fn stage_file(&mut self, g: &GuestNode, path: &Path) -> Result<(), MediaError> {
        let mut f = File::create(path).map_err(|e| io_err(path, e))?;
        let mut left = u64::from(g.size);
        for &(start, count) in &g.runs {
            for s in start..start + count {
                if left == 0 {
                    break;
                }
                let n = left.min(SECTOR_U64) as usize;
                let sec = self.sector(s)?;
                f.write_all(&sec[..n]).map_err(|e| io_err(path, e))?;
                left -= n as u64;
            }
        }
        f.flush().map_err(|e| io_err(path, e))
    }

    fn apply(&mut self, guest: &[GuestNode], plan: Plan) -> Result<(), MediaError> {
        let stage = self.dir.join(STAGE_DIR);
        let staging = plan.files.iter().any(|f| f.rename || f.write.is_some());
        if staging {
            fs::create_dir_all(&stage).map_err(|e| io_err(&stage, e))?;
        }
        let count = plan.files.len();
        // 1. Every new content into the stage, before anything moves.
        let mut staged: Vec<Option<PathBuf>> = vec![None; count];
        for (n, fp) in plan.files.iter().enumerate() {
            if fp.write.is_some() {
                let p = stage.join(format!("w{n}"));
                self.stage_file(&guest[fp.guest], &p)?;
                staged[n] = Some(p);
            }
        }
        self.handle = None;
        // 2. Deleted files into the trash.
        if !plan.trash.is_empty() {
            let base = self.dir.join(TRASH_DIR);
            let bin = (1u32..)
                .map(|n| base.join(n.to_string()))
                .find(|p| fs::symlink_metadata(p).is_err())
                .unwrap_or_else(|| base.join("0"));
            fs::create_dir_all(&bin).map_err(|e| io_err(&bin, e))?;
            for k in &plan.trash {
                let r = self.reference[k].clone();
                let to = bin.join(&r.host);
                move_file(&self.dir.join(&r.host), &to)?;
                if let Some(idx) = r.file {
                    self.files[idx].path = to;
                }
            }
        }
        // 3. Renames, through the stage (swaps and moves into vacated
        // places work).
        let mut final_host: Vec<Option<PathBuf>> = vec![None; count];
        let mut moving = Vec::new();
        for (n, fp) in plan.files.iter().enumerate() {
            if !fp.rename {
                continue;
            }
            let Some(r) = fp
                .matched
                .as_ref()
                .and_then(|k| self.reference.get(k))
                .cloned()
            else {
                continue;
            };
            let tmp = stage.join(format!("r{n}"));
            move_file(&self.dir.join(&r.host), &tmp)?;
            if let Some(idx) = r.file {
                self.files[idx].path = tmp.clone();
            }
            moving.push((n, r.file, tmp));
        }
        for (n, idx, tmp) in moving {
            let target = free_name(&self.dir, &plan.files[n].desired);
            let abs = self.dir.join(&target);
            move_file(&tmp, &abs)?;
            if let Some(idx) = idx {
                self.files[idx].path = abs;
            }
            final_host[n] = Some(target);
        }
        // 4. Folders.
        for (_, h) in &plan.dirs {
            let abs = self.dir.join(h);
            fs::create_dir_all(&abs).map_err(|e| io_err(&abs, e))?;
        }
        // 5. Contents into place.
        let mut new_idx: Vec<Option<usize>> = vec![None; count];
        let mut overwritten = BTreeSet::new();
        for (n, fp) in plan.files.iter().enumerate() {
            let (Some(kind), Some(src)) = (fp.write, staged[n].take()) else {
                continue;
            };
            let old = fp
                .matched
                .as_ref()
                .and_then(|k| self.reference.get(k))
                .cloned();
            let target = match (kind, &old) {
                (WriteKind::Overwrite, Some(r)) => {
                    if let Some(idx) = r.file {
                        overwritten.insert(idx);
                    }
                    final_host[n].clone().unwrap_or_else(|| r.host.clone())
                }
                _ => free_name(&self.dir, &fp.desired),
            };
            let abs = self.dir.join(&target);
            move_file(&src, &abs)?;
            new_idx[n] = Some(self.files.len());
            self.files.push(HostFile { path: abs });
            final_host[n] = Some(target);
        }
        // 6. Folders that are gone, when empty.
        for d in &plan.remove_dirs {
            let _ = fs::remove_dir(self.dir.join(d));
        }
        if staging {
            let _ = fs::remove_dir(&stage);
        }
        self.rebase(guest, &plan, &final_host, &new_idx, &overwritten);
        Ok(())
    }

    /// The guest's tree becomes the reference.
    fn rebase(
        &mut self,
        guest: &[GuestNode],
        plan: &Plan,
        final_host: &[Option<PathBuf>],
        new_idx: &[Option<usize>],
        overwritten: &BTreeSet<usize>,
    ) {
        self.handle = None;
        self.extents.retain(
            |_, e| !matches!(e.source, Source::File { idx, .. } if overwritten.contains(&idx)),
        );
        let mut reference = BTreeMap::new();
        for (i, h) in &plan.dirs {
            let g = &guest[*i];
            reference.insert(
                g.path.clone(),
                RefNode {
                    is_dir: true,
                    first: g.first,
                    size: 0,
                    runs: g.runs.clone(),
                    file: None,
                    host: h.clone(),
                    stamp: None,
                },
            );
        }
        for (n, fp) in plan.files.iter().enumerate() {
            let g = &guest[fp.guest];
            let old = fp
                .matched
                .as_ref()
                .and_then(|k| self.reference.get(k))
                .cloned();
            let (file, host, stamp) = match (new_idx[n], &final_host[n], old) {
                (Some(idx), Some(h), _) => (Some(idx), h.clone(), stamp_of(&self.dir.join(h))),
                (None, Some(h), Some(r)) => (r.file, h.clone(), stamp_of(&self.dir.join(h))),
                (None, None, Some(r)) => (r.file, r.host, r.stamp),
                _ => continue,
            };
            if let Some(idx) = file {
                let mut left = u64::from(g.size).div_ceil(SECTOR_U64);
                let mut off = 0;
                for &(start, count) in &g.runs {
                    if left == 0 {
                        break;
                    }
                    let k = count.min(left);
                    self.set_extent(
                        start,
                        k,
                        Source::File {
                            idx,
                            offset: off * SECTOR_U64,
                        },
                    );
                    off += k;
                    left -= k;
                }
            }
            reference.insert(
                g.path.clone(),
                RefNode {
                    is_dir: false,
                    first: g.first,
                    size: g.size,
                    runs: g.runs.clone(),
                    file,
                    host,
                    stamp,
                },
            );
        }
        self.reference = reference;
        // Overlay sectors the host now holds byte for byte.
        let keys: Vec<u64> = self.overlay.keys().copied().collect();
        let mut base = [0u8; SECTOR];
        for s in keys {
            if self.base_sector(s, &mut base).is_ok()
                && self.overlay.get(&s).is_some_and(|o| **o == base)
            {
                self.overlay.remove(&s);
            }
        }
        self.handle = None;
    }
}

impl MediaBackend for FatDirBackend {
    fn len(&self) -> u64 {
        self.geo.sectors * SECTOR_U64
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
        self.check(offset, buf.len())?;
        let mut pos = offset;
        let mut done = 0;
        while done < buf.len() {
            let within = (pos % SECTOR_U64) as usize;
            let n = (SECTOR - within).min(buf.len() - done);
            let sec = self.sector(pos / SECTOR_U64)?;
            buf[done..done + n].copy_from_slice(&sec[within..within + n]);
            pos += n as u64;
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
        self.check(offset, data.len())?;
        let mut pos = offset;
        let mut done = 0;
        while done < data.len() {
            let s = pos / SECTOR_U64;
            let within = (pos % SECTOR_U64) as usize;
            let n = (SECTOR - within).min(data.len() - done);
            let mut sec = if n == SECTOR {
                [0u8; SECTOR]
            } else {
                self.sector(s)?
            };
            sec[within..within + n].copy_from_slice(&data[done..done + n]);
            self.overlay.insert(s, Box::new(sec));
            pos += n as u64;
            done += n;
        }
        if !data.is_empty() {
            self.dirty = true;
        }
        Ok(())
    }

    /// Write the guest's changes back into the folder (see the module
    /// documentation); a damaged file system is refused untouched.
    fn commit(&mut self) -> Result<(), MediaError> {
        if !self.dirty {
            return Ok(());
        }
        self.handle = None;
        let guest = self.parse_guest()?;
        let plan = self.plan(&guest)?;
        self.apply(&guest, plan)?;
        self.dirty = false;
        Ok(())
    }

    fn dirty(&self) -> bool {
        self.dirty
    }
}

/// A host folder as a FAT32 disk (`size` in bytes, `None` for the
/// default).
pub fn open(dir: &Path, size: Option<u64>) -> Result<Media, MediaError> {
    let backend = FatDirBackend::open(dir, size)?;
    Ok(Media {
        name: dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        kind: MediaKind::Disk,
        backend: Box::new(backend),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("mimic-fatdir-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn put(&self, rel: &str, data: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, data).unwrap();
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pattern(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761) | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    type Tree = BTreeMap<String, Option<Vec<u8>>>;

    /// The host folder: path → contents (`None`: a folder); the trash only
    /// with `trash`.
    fn host_tree(dir: &Path, trash: bool) -> Tree {
        fn walk(base: &Path, rel: &str, out: &mut Tree, trash: bool) {
            for e in fs::read_dir(base.join(rel)).unwrap() {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                if rel.is_empty() && name == TRASH_DIR && !trash {
                    continue;
                }
                let r = if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                };
                if e.file_type().unwrap().is_dir() {
                    out.insert(r.clone(), None);
                    walk(base, &r, out, trash);
                } else {
                    out.insert(r, Some(fs::read(e.path()).unwrap()));
                }
            }
        }
        let mut out = Tree::new();
        walk(dir, "", &mut out, trash);
        out
    }

    const POS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

    fn checksum(short: &[u8]) -> u8 {
        let mut sum = 0u8;
        for &b in short {
            sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(b);
        }
        sum
    }

    fn split(path: &str) -> (&str, &str) {
        path.rsplit_once('/').unwrap_or(("", path))
    }

    #[derive(Clone, Debug)]
    struct Ent {
        name: String,
        short: [u8; 11],
        attr: u8,
        first: u32,
        size: u32,
        date: u16,
        time: u16,
        /// Byte offsets of its entries, long-name entries first.
        slots: Vec<u64>,
    }

    /// A small FAT32 driver of the tests' own, working on the volume the
    /// way an operating system would: both FATs, long names, first free
    /// clusters.
    struct Fs<'a> {
        be: &'a mut FatDirBackend,
        part: u64,
        spc: u64,
        rsvd: u64,
        nfats: u64,
        fatsz: u64,
        root: u32,
        clusters: u32,
        seq: u32,
    }

    impl<'a> Fs<'a> {
        fn new(be: &'a mut FatDirBackend) -> Self {
            let mut mbr = vec![0; 512];
            be.read(0, &mut mbr).unwrap();
            assert_eq!(&mbr[510..], &[0x55, 0xAA]);
            assert_eq!(mbr[446 + 4], 0x0C);
            let part = u64::from(u32::from_le_bytes(mbr[454..458].try_into().unwrap()));
            let mut b = vec![0; 512];
            be.read(part * 512, &mut b).unwrap();
            let r16 = |o: usize| u64::from(u16::from_le_bytes([b[o], b[o + 1]]));
            let r32 = |o: usize| u64::from(u32::from_le_bytes(b[o..o + 4].try_into().unwrap()));
            assert_eq!(r16(11), 512);
            assert_eq!(&b[82..90], b"FAT32   ");
            assert_eq!(&b[510..], &[0x55, 0xAA]);
            let (spc, rsvd, nfats) = (u64::from(b[13]), r16(14), u64::from(b[16]));
            let (tot, fatsz, root) = (r32(32), r32(36), r32(44) as u32);
            let clusters = ((tot - rsvd - nfats * fatsz) / spc) as u32;
            Self {
                be,
                part,
                spc,
                rsvd,
                nfats,
                fatsz,
                root,
                clusters,
                seq: 0,
            }
        }

        fn rd(&mut self, off: u64, n: usize) -> Vec<u8> {
            let mut v = vec![0; n];
            self.be.read(off, &mut v).unwrap();
            v
        }

        fn wr(&mut self, off: u64, data: &[u8]) {
            self.be.write(off, data).unwrap()
        }

        fn cb(&self) -> u64 {
            self.spc * 512
        }

        fn coff(&self, c: u32) -> u64 {
            (self.part + self.rsvd + self.nfats * self.fatsz + (u64::from(c) - 2) * self.spc) * 512
        }

        fn fat_off(&self, i: u64, c: u32) -> u64 {
            (self.part + self.rsvd + i * self.fatsz) * 512 + u64::from(c) * 4
        }

        fn fat(&mut self, c: u32) -> u32 {
            let o = self.fat_off(0, c);
            u32::from_le_bytes(self.rd(o, 4).try_into().unwrap()) & 0x0FFF_FFFF
        }

        fn set_fat(&mut self, c: u32, v: u32) {
            for i in 0..self.nfats {
                let o = self.fat_off(i, c);
                self.wr(o, &v.to_le_bytes());
            }
        }

        fn chain(&mut self, first: u32) -> Vec<u32> {
            let mut out = Vec::new();
            let mut c = first;
            while (2..self.clusters + 2).contains(&c) {
                out.push(c);
                assert!(out.len() <= self.clusters as usize, "chain loops");
                c = self.fat(c);
            }
            assert!(first == 0 || c >= 0x0FFF_FFF8, "chain ends in {c:#x}");
            out
        }

        fn entries(&mut self, dir: u32) -> Vec<Ent> {
            let mut out = Vec::new();
            let mut lfn: Vec<(u8, u8, Vec<u16>)> = Vec::new();
            let mut slots = Vec::new();
            let cb = self.cb() as usize;
            'dir: for c in self.chain(dir) {
                let base = self.coff(c);
                let data = self.rd(base, cb);
                for (k, e) in data.chunks(32).enumerate() {
                    let off = base + (k * 32) as u64;
                    if e[0] == 0 {
                        break 'dir;
                    }
                    if e[0] == 0xE5 {
                        lfn.clear();
                        slots.clear();
                        continue;
                    }
                    slots.push(off);
                    if e[11] == 0x0F {
                        let u = POS
                            .iter()
                            .map(|&p| u16::from_le_bytes([e[p], e[p + 1]]))
                            .collect();
                        lfn.push((e[0], e[13], u));
                        continue;
                    }
                    let short: [u8; 11] = e[..11].try_into().unwrap();
                    let name = if lfn.is_empty() {
                        let base = String::from_utf8_lossy(&short[..8]).trim_end().to_string();
                        let ext = String::from_utf8_lossy(&short[8..]).trim_end().to_string();
                        if ext.is_empty() {
                            base
                        } else {
                            format!("{base}.{ext}")
                        }
                    } else {
                        let n = lfn.len();
                        assert_eq!(lfn[0].0, n as u8 | 0x40, "first long-name entry");
                        let sum = checksum(&short);
                        let mut units = Vec::new();
                        for (i, (ord, chk, u)) in lfn.iter().enumerate().rev() {
                            assert_eq!(ord & 0x1F, (n - i) as u8, "long-name order");
                            assert_eq!(*chk, sum, "long-name checksum");
                            units.extend(u);
                        }
                        let end = units.iter().position(|&x| x == 0).unwrap_or(units.len());
                        String::from_utf16(&units[..end]).unwrap()
                    };
                    let w = |o: usize| u16::from_le_bytes([e[o], e[o + 1]]);
                    out.push(Ent {
                        name,
                        short,
                        attr: e[11],
                        first: (u32::from(w(20)) << 16) | u32::from(w(26)),
                        size: u32::from_le_bytes(e[28..32].try_into().unwrap()),
                        date: w(24),
                        time: w(22),
                        slots: std::mem::take(&mut slots),
                    });
                    lfn.clear();
                }
            }
            out
        }

        fn list(&mut self, dir: u32) -> Vec<Ent> {
            self.entries(dir)
                .into_iter()
                .filter(|e| e.attr & 0x08 == 0 && e.name != "." && e.name != "..")
                .collect()
        }

        fn read_file(&mut self, e: &Ent) -> Vec<u8> {
            let cb = self.cb() as usize;
            let mut out = Vec::new();
            for c in self.chain(e.first) {
                let o = self.coff(c);
                out.extend(self.rd(o, cb));
            }
            assert_eq!(out.len(), (e.size as usize).div_ceil(cb) * cb, "{}", e.name);
            out.truncate(e.size as usize);
            out
        }

        fn tree(&mut self) -> Tree {
            let mut out = Tree::new();
            let root = self.root;
            self.walk(root, "", &mut out);
            out
        }

        fn walk(&mut self, dir: u32, rel: &str, out: &mut Tree) {
            let mut shorts = BTreeSet::new();
            for e in self.list(dir) {
                assert!(shorts.insert(e.short), "short name twice in {rel}");
                let r = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if e.attr & 0x10 != 0 {
                    let dots = self.entries(e.first);
                    assert_eq!(&dots[0].short, b".          ");
                    assert_eq!(dots[0].first, e.first);
                    assert_eq!(&dots[1].short, b"..         ");
                    assert_eq!(dots[1].first, if dir == self.root { 0 } else { dir });
                    out.insert(r.clone(), None);
                    self.walk(e.first, &r, out);
                } else {
                    let d = self.read_file(&e);
                    out.insert(r, Some(d));
                }
            }
        }

        fn find(&mut self, path: &str) -> (u32, Ent) {
            let (parent, name) = split(path);
            let d = self.dir_of(parent);
            let e = self.list(d).into_iter().find(|e| e.name == name);
            (d, e.unwrap_or_else(|| panic!("{path} not found")))
        }

        fn dir_of(&mut self, path: &str) -> u32 {
            if path.is_empty() {
                return self.root;
            }
            let (_, e) = self.find(path);
            assert!(e.attr & 0x10 != 0);
            e.first
        }

        fn alloc(&mut self, n: usize) -> u32 {
            if n == 0 {
                return 0;
            }
            let mut got = Vec::new();
            let mut c = 2;
            while got.len() < n {
                assert!(c < self.clusters + 2, "volume full");
                if self.fat(c) == 0 {
                    got.push(c);
                }
                c += 1;
            }
            for w in got.windows(2) {
                self.set_fat(w[0], w[1]);
            }
            self.set_fat(got[n - 1], 0x0FFF_FFFF);
            got[0]
        }

        fn free(&mut self, first: u32) {
            for c in self.chain(first) {
                self.set_fat(c, 0);
            }
        }

        fn put_data(&mut self, first: u32, data: &[u8]) {
            let cb = self.cb() as usize;
            for (i, c) in self.chain(first).into_iter().enumerate() {
                let o = self.coff(c);
                self.wr(o, &data[i * cb..data.len().min((i + 1) * cb)]);
            }
        }

        fn add_entry(&mut self, dir: u32, name: &str, attr: u8, first: u32, size: u32) {
            self.seq += 1;
            let mut short = *b"           ";
            short[..8].copy_from_slice(format!("Q{:07}", self.seq).as_bytes());
            let sum = checksum(&short);
            let mut units: Vec<u16> = name.encode_utf16().collect();
            let n = units.len().div_ceil(13);
            if !units.len().is_multiple_of(13) {
                units.push(0);
            }
            units.resize(n * 13, 0xFFFF);
            let mut ents: Vec<[u8; 32]> = Vec::new();
            for i in (0..n).rev() {
                let mut e = [0u8; 32];
                e[0] = (i + 1) as u8 | if i == n - 1 { 0x40 } else { 0 };
                e[11] = 0x0F;
                e[13] = sum;
                for (k, &p) in POS.iter().enumerate() {
                    e[p..p + 2].copy_from_slice(&units[i * 13 + k].to_le_bytes());
                }
                ents.push(e);
            }
            let mut e = [0u8; 32];
            e[..11].copy_from_slice(&short);
            e[11] = attr;
            e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            e[24..26].copy_from_slice(&((41u16 << 9) | (3 << 5) | 4).to_le_bytes());
            e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
            e[28..32].copy_from_slice(&size.to_le_bytes());
            ents.push(e);
            loop {
                let cb = self.cb();
                let chain = self.chain(dir);
                let offs: Vec<u64> = chain
                    .iter()
                    .flat_map(|&c| (0..cb / 32).map(move |k| (c, k)))
                    .map(|(c, k)| self.coff(c) + k * 32)
                    .collect();
                let mut run = 0;
                let mut found = None;
                for (i, &o) in offs.iter().enumerate() {
                    let b = self.rd(o, 1)[0];
                    if b == 0 || b == 0xE5 {
                        run += 1;
                        if run == ents.len() {
                            found = Some(i + 1 - run);
                            break;
                        }
                    } else {
                        run = 0;
                    }
                }
                if let Some(at) = found {
                    for (k, e) in ents.iter().enumerate() {
                        self.wr(offs[at + k], e);
                    }
                    return;
                }
                let c = self.alloc(1);
                let o = self.coff(c);
                self.wr(o, &vec![0; cb as usize]);
                self.set_fat(chain[chain.len() - 1], c);
            }
        }

        fn create(&mut self, path: &str, data: &[u8]) {
            let (parent, name) = split(path);
            let d = self.dir_of(parent);
            let first = self.alloc(data.len().div_ceil(self.cb() as usize));
            self.put_data(first, data);
            self.add_entry(d, name, 0x20, first, data.len() as u32);
        }

        fn dot_entry(name: &[u8; 11], c: u32) -> [u8; 32] {
            let mut e = [0u8; 32];
            e[..11].copy_from_slice(name);
            e[11] = 0x10;
            e[20..22].copy_from_slice(&((c >> 16) as u16).to_le_bytes());
            e[26..28].copy_from_slice(&(c as u16).to_le_bytes());
            e
        }

        fn mkdir(&mut self, path: &str) {
            let (parent, name) = split(path);
            let d = self.dir_of(parent);
            let c = self.alloc(1);
            let o = self.coff(c);
            let cb = self.cb() as usize;
            self.wr(o, &vec![0; cb]);
            let up = if d == self.root { 0 } else { d };
            self.wr(o, &Self::dot_entry(b".          ", c));
            self.wr(o + 32, &Self::dot_entry(b"..         ", up));
            self.add_entry(d, name, 0x10, c, 0);
        }

        fn remove(&mut self, path: &str) {
            let (_, e) = self.find(path);
            if e.attr & 0x10 != 0 {
                for child in self.list(e.first) {
                    self.remove(&format!("{path}/{}", child.name));
                }
            }
            self.free(e.first);
            for &o in &e.slots {
                self.wr(o, &[0xE5]);
            }
        }

        fn rename(&mut self, from: &str, to: &str) {
            let (_, e) = self.find(from);
            for &o in &e.slots {
                self.wr(o, &[0xE5]);
            }
            let (parent, name) = split(to);
            let d = self.dir_of(parent);
            self.add_entry(d, name, e.attr, e.first, e.size);
            if e.attr & 0x10 != 0 {
                let up = if d == self.root { 0 } else { d };
                let o = self.coff(e.first) + 32;
                self.wr(o, &Self::dot_entry(b"..         ", up));
            }
        }

        fn overwrite(&mut self, path: &str, data: &[u8]) {
            let (_, e) = self.find(path);
            self.free(e.first);
            let first = self.alloc(data.len().div_ceil(self.cb() as usize));
            self.put_data(first, data);
            let o = e.slots[e.slots.len() - 1];
            let mut s = self.rd(o, 32);
            s[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
            s[26..28].copy_from_slice(&(first as u16).to_le_bytes());
            s[28..32].copy_from_slice(&(data.len() as u32).to_le_bytes());
            self.wr(o, &s);
        }
    }

    #[test]
    fn short_names_follow_the_rules() {
        let mut taken = BTreeSet::new();
        assert_eq!(
            make_short("README.TXT", &taken),
            Some((*b"README  TXT", false))
        );
        assert_eq!(
            make_short("readme.txt", &taken),
            Some((*b"README  TXT", true))
        );
        assert_eq!(
            make_short(".gitignore", &taken),
            Some((*b"GITIGN~1   ", true))
        );
        assert_eq!(make_short("a+b.c", &taken), Some((*b"A_B~1   C  ", true)));
        assert_eq!(
            make_short("Long file name.html", &taken),
            Some((*b"LONGFI~1HTM", true))
        );
        taken.insert(*b"LONGFI~1HTM");
        assert_eq!(
            make_short("Long file names.html", &taken),
            Some((*b"LONGFI~2HTM", true))
        );
        taken.insert(*b"README  TXT");
        assert_eq!(
            make_short("README.TXT", &taken),
            Some((*b"README~1TXT", true))
        );
        assert_eq!(lfn_entries("a", b"A          ").len(), 1);
        assert_eq!(lfn_entries("exactly13char", b"EXACTL~1   ").len(), 1);
        assert_eq!(lfn_entries("fourteen chars", b"FOURTE~1   ").len(), 2);
        assert_eq!(fat_time(None), ((1 << 5) | 1, 0, 0));
        assert_eq!(
            fat_time(Some(UNIX_EPOCH + Duration::from_secs(1_589_723_131))).2,
            100
        );
    }

    #[test]
    fn folder_becomes_a_fat32_volume() {
        let t = Temp::new("build");
        t.put("readme.txt", b"hello");
        t.put("README2.TXT", b"upper");
        t.put("A long file name with spaces.text", &pattern(100, 1));
        t.put("empty.bin", b"");
        t.put("sub/deeper/big.bin", &pattern(5000, 2));
        t.put(
            "sub/\u{dc}n\u{ef}c\u{f6}d\u{e9} name.txt",
            "gr\u{fc}\u{df}e".as_bytes(),
        );
        t.put(".hidden", b"dot");
        for i in 0..20 {
            t.put(
                &format!("many/file number {i:02}.dat"),
                &pattern(i * 100, i as u32),
            );
        }
        fs::create_dir_all(t.0.join("empty dir")).unwrap();
        let when = UNIX_EPOCH + Duration::from_secs(1_589_723_130); // 2020-05-17 13:45:30
        File::options()
            .write(true)
            .open(t.0.join("readme.txt"))
            .unwrap()
            .set_modified(when)
            .unwrap();

        let mut be = FatDirBackend::open(&t.0, None).unwrap();
        assert_eq!(be.len(), DEFAULT_MIN_SIZE);
        assert!(be.skipped().is_empty(), "{:?}", be.skipped());
        assert!(!be.dirty());
        let mut f = Fs::new(&mut be);
        assert_eq!(f.part, PARTITION_START);
        assert_eq!(f.spc, 1);
        assert!(f.clusters >= 65_525);
        assert_eq!(f.tree(), host_tree(&t.0, false));

        let (_, e) = f.find("readme.txt");
        assert_eq!(&e.short, b"README  TXT");
        assert_eq!(e.date, (40 << 9) | (5 << 5) | 17);
        assert_eq!(e.time, (13 << 11) | (45 << 5) | 15);
        let (_, e) = f.find("README2.TXT");
        assert_eq!(e.slots.len(), 1, "a plain 8.3 name needs no long name");
        let (_, e) = f.find("empty.bin");
        assert_eq!((e.first, e.size), (0, 0));

        // Both FATs alike, FSInfo's free count right, backup boot sector.
        let n = f.fatsz as usize * 512;
        let f0 = f.rd((f.part + f.rsvd) * 512, n);
        let f1 = f.rd((f.part + f.rsvd + f.fatsz) * 512, n);
        assert!(f0 == f1);
        assert_eq!(le32(&f0, 0), 0x0FFF_FFF8);
        let free = (2..f.clusters as usize + 2)
            .filter(|&c| f0[c * 4..c * 4 + 4] == [0; 4])
            .count();
        let info = f.rd((f.part + 1) * 512, 512);
        assert_eq!(le32(&info, 0), 0x4161_5252);
        assert_eq!(le32(&info, 484), 0x6141_7272);
        assert_eq!(le32(&info, 488) as usize, free);
        let boot = f.rd(f.part * 512, 512);
        assert_eq!(boot, f.rd((f.part + 6) * 512, 512));
        assert_eq!((f.part + f.rsvd + 2 * f.fatsz) % f.spc, 0);
    }

    #[test]
    fn same_folder_gives_identical_volumes() {
        let t = Temp::new("determinism");
        t.put("a/b/c.txt", b"abc");
        t.put("x.bin", &pattern(70_000, 3));
        t.put("Long Name Here.txt", b"x");
        let mut a = FatDirBackend::open(&t.0, Some(48 * MIB)).unwrap();
        let mut b = FatDirBackend::open(&t.0, Some(48 * MIB)).unwrap();
        assert_eq!(a.len(), 48 * MIB);
        assert_eq!(a.serial(), b.serial());
        let mut ba = vec![0; MIB as usize];
        let mut bb = vec![0; MIB as usize];
        for off in (0..a.len()).step_by(MIB as usize) {
            a.read(off, &mut ba).unwrap();
            b.read(off, &mut bb).unwrap();
            assert!(ba == bb, "volumes differ in the MiB at {off}");
        }
        t.put("new.txt", b"n");
        let c = FatDirBackend::open(&t.0, Some(48 * MIB)).unwrap();
        assert_ne!(c.serial(), a.serial());
    }

    #[test]
    fn guest_changes_come_back_into_the_folder() {
        let t = Temp::new("writeback");
        t.put("keep.txt", b"keep");
        t.put("change.txt", b"old content");
        t.put("delete.txt", b"delete me");
        t.put("rename_me.txt", b"rename me");
        t.put("conflict.txt", b"original");
        t.put("olddir/inner.txt", b"inner");
        t.put("deldir/x.txt", b"x data");
        t.put("deldir/sub/y.txt", b"y");
        let big = pattern(3000, 7);
        let mut be = FatDirBackend::open(&t.0, None).unwrap();
        let before = {
            let mut f = Fs::new(&mut be);
            f.create("new file.txt", b"brand new");
            f.mkdir("newdir");
            f.mkdir("newdir/deep");
            f.create("newdir/deep/n.bin", &big);
            f.overwrite("change.txt", b"new content that is longer");
            f.remove("delete.txt");
            f.rename("rename_me.txt", "newdir/renamed.txt");
            f.rename("olddir", "newname");
            f.remove("deldir");
            f.overwrite("conflict.txt", b"guest version");
            f.tree()
        };
        t.put("conflict.txt", b"the host changed it");
        assert!(be.dirty());
        be.commit().unwrap();
        assert!(!be.dirty());

        let host = host_tree(&t.0, true);
        let file = |p: &str| host.get(p).cloned().flatten();
        assert_eq!(file("keep.txt"), Some(b"keep".to_vec()));
        assert_eq!(
            file("change.txt"),
            Some(b"new content that is longer".to_vec())
        );
        assert_eq!(file("new file.txt"), Some(b"brand new".to_vec()));
        assert_eq!(file("newdir/deep/n.bin"), Some(big.clone()));
        assert_eq!(file("newdir/renamed.txt"), Some(b"rename me".to_vec()));
        assert_eq!(file("newname/inner.txt"), Some(b"inner".to_vec()));
        assert_eq!(file("conflict.txt"), Some(b"the host changed it".to_vec()));
        assert_eq!(
            file("conflict (mimic).txt"),
            Some(b"guest version".to_vec())
        );
        assert_eq!(
            file(".mimic-trash/1/delete.txt"),
            Some(b"delete me".to_vec())
        );
        assert_eq!(
            file(".mimic-trash/1/deldir/x.txt"),
            Some(b"x data".to_vec())
        );
        assert_eq!(file(".mimic-trash/1/deldir/sub/y.txt"), Some(b"y".to_vec()));
        for gone in ["delete.txt", "rename_me.txt", "olddir", "deldir", STAGE_DIR] {
            assert!(!host.contains_key(gone), "{gone} still there");
        }
        assert!(
            !host.keys().any(|k| k.contains("rename_me")),
            "a rename is no deletion"
        );
        // The guest's view is the same after the commit.
        assert_eq!(Fs::new(&mut be).tree(), before);

        // A second round on the re-based volume.
        {
            let mut f = Fs::new(&mut be);
            f.overwrite("new file.txt", b"second");
            f.create("newdir/second.txt", b"2");
            f.overwrite("conflict.txt", b"guest again");
            f.remove("keep.txt");
        }
        be.commit().unwrap();
        let host = host_tree(&t.0, true);
        let file = |p: &str| host.get(p).cloned().flatten();
        assert_eq!(file("new file.txt"), Some(b"second".to_vec()));
        assert_eq!(file("newdir/second.txt"), Some(b"2".to_vec()));
        assert_eq!(file("conflict.txt"), Some(b"the host changed it".to_vec()));
        assert_eq!(file("conflict (mimic).txt"), Some(b"guest again".to_vec()));
        assert_eq!(file(".mimic-trash/2/keep.txt"), Some(b"keep".to_vec()));
        assert!(!host.contains_key("keep.txt"));
        assert!(!host.contains_key("conflict (mimic 2).txt"));

        // Nothing changed: nothing happens.
        be.commit().unwrap();
        assert_eq!(host_tree(&t.0, true), host);

        // A fresh volume shows the folder, without the trash.
        let mut fresh = FatDirBackend::open(&t.0, None).unwrap();
        assert_eq!(Fs::new(&mut fresh).tree(), host_tree(&t.0, false));
    }

    #[test]
    fn damaged_guest_file_system_is_refused() {
        for case in 0..4 {
            let t = Temp::new(&format!("damaged{case}"));
            t.put("a.bin", &pattern(3000, 9));
            t.put("dir/b.txt", b"b");
            let before = host_tree(&t.0, true);
            let mut be = FatDirBackend::open(&t.0, None).unwrap();
            {
                let mut f = Fs::new(&mut be);
                f.create("new.txt", b"would be written");
                f.remove("dir/b.txt");
                let (_, a) = f.find("a.bin");
                match case {
                    // A loop.
                    0 => f.set_fat(a.first + 2, a.first),
                    // A cross link.
                    1 => {
                        let (_, n) = f.find("new.txt");
                        f.set_fat(n.first, a.first + 1);
                    }
                    // A chain running into a free cluster.
                    2 => f.set_fat(a.first + 1, 0),
                    // A size the chain does not have.
                    _ => {
                        let o = a.slots[a.slots.len() - 1];
                        f.wr(o + 28, &100_000u32.to_le_bytes());
                    }
                }
            }
            let r = be.commit();
            assert!(
                matches!(r, Err(MediaError::Format(_))),
                "case {case}: {r:?}"
            );
            assert!(be.dirty());
            assert_eq!(host_tree(&t.0, true), before, "case {case}");
            assert!(!t.0.join(TRASH_DIR).exists() && !t.0.join(STAGE_DIR).exists());
        }
    }

    #[test]
    fn guest_names_the_host_cannot_hold_are_refused() {
        let t = Temp::new("badname");
        t.put("a.txt", b"a");
        let before = host_tree(&t.0, true);
        let mut be = FatDirBackend::open(&t.0, None).unwrap();
        {
            let mut f = Fs::new(&mut be);
            f.create("aux.txt", b"device");
            f.remove("a.txt");
        }
        assert!(matches!(be.commit(), Err(MediaError::Format(_))));
        assert_eq!(host_tree(&t.0, true), before);
    }

    #[test]
    fn access_past_the_end_fails() {
        let t = Temp::new("range");
        t.put("a.txt", b"a");
        let mut be = FatDirBackend::open(&t.0, Some(40 * MIB)).unwrap();
        let len = be.len();
        assert_eq!(len, 40 * MIB);
        let mut buf = [0u8; 2];
        assert_eq!(be.read(len - 1, &mut buf), Err(MediaError::OutOfRange));
        assert_eq!(be.write(len - 1, &buf), Err(MediaError::OutOfRange));
        assert_eq!(be.read(u64::MAX, &mut buf), Err(MediaError::OutOfRange));
        be.read(len - 2, &mut buf).unwrap();
        be.read(len, &mut []).unwrap();
        assert!(!be.dirty());
        // Unaligned writes across sectors read back.
        let data = pattern(1500, 5);
        be.write(1000, &data).unwrap();
        let mut back = vec![0; 1500];
        be.read(1000, &mut back).unwrap();
        assert_eq!(back, data);
        assert!(be.dirty());
    }

    #[test]
    fn big_sparse_volume_stays_small_in_memory() {
        let t = Temp::new("sparse");
        t.put("one.txt", b"1");
        t.put("d/two.bin", &pattern(100_000, 4));
        let mut be = FatDirBackend::open(&t.0, Some(2 << 30)).unwrap();
        assert_eq!(be.len(), 2 << 30);
        assert!(be.memory_use() < 64 * 1024, "{} bytes", be.memory_use());
        {
            let mut f = Fs::new(&mut be);
            assert_eq!(f.spc, 8);
            assert_eq!(f.tree(), host_tree(&t.0, false));
            let last_fat = (f.part + f.rsvd + 2 * f.fatsz - 1) * 512;
            assert!(f.rd(last_fat, 512).iter().all(|&b| b == 0));
            let len = f.be.len();
            assert!(f.rd(len - 512, 512).iter().all(|&b| b == 0));
            f.create("added.txt", b"added");
        }
        assert!(be.memory_use() < 64 * 1024, "{} bytes", be.memory_use());
        be.commit().unwrap();
        assert_eq!(fs::read(t.0.join("added.txt")).unwrap(), b"added");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped() {
        let t = Temp::new("symlink");
        t.put("real.txt", b"r");
        std::os::unix::fs::symlink(t.0.join("real.txt"), t.0.join("link.txt")).unwrap();
        let be = FatDirBackend::open(&t.0, None).unwrap();
        assert_eq!(be.skipped().len(), 1);
        assert_eq!(be.skipped()[0].reason, "symbolic link");
    }
}
