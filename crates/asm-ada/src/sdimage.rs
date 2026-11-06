//! SD card images for the Logophor 16: an MBR with one FAT32 partition
//! (type $0C, from block 2048), 512-byte sectors and clusters, 8.3 names
//! without long-name entries. Foundry's boot loader reads
//! `BOOT/BOILER.BIN` from such a card (SPEC.md 11); any FAT32 tool on the
//! host (mtools, the system's own) can fill one too.
//!
//! Layout from Microsoft's "FAT: General Overview of On-Disk Format"
//! (FAT32 File System Specification 1.03, 2000): boot sector and BPB,
//! FSInfo at sector 1, backup boot sector at 6, two FATs, root directory
//! at cluster 2; the MBR partition table at byte 446.

/// The card's first partition block, as most formatters put it.
const PART_START: u32 = 2048;
const RESERVED: u32 = 32;
const ROOT_CLUSTER: u32 = 2;
const EOC: u32 = 0x0fff_ffff;
/// FAT32 needs at least this many clusters (else it is FAT16).
const MIN_CLUSTERS: u32 = 65_525;

/// The smallest card this builder makes, in MiB.
pub const MIN_MIB: u32 = 40;

/// An 8.3 name as stored: 11 bytes, upper case, space-padded.
fn short_name(name: &str) -> Result<[u8; 11], String> {
    let up = name.to_ascii_uppercase();
    let (base, ext) = match up.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (up.as_str(), ""),
    };
    let ok = |s: &str, max: usize| {
        s.len() <= max
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-~!#$%&'()@^{}".contains(&c))
    };
    if base.is_empty() || !ok(base, 8) || !ok(ext, 3) {
        return Err(format!("{name}: not an 8.3 name"));
    }
    let mut n = [b' '; 11];
    n[..base.len()].copy_from_slice(base.as_bytes());
    n[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Ok(n)
}

struct Dir {
    name: [u8; 11],
    entries: Vec<Entry>,
}

enum Entry {
    File { name: [u8; 11], data: Vec<u8> },
    Dir(Dir),
}

impl Dir {
    fn insert(&mut self, path: &[&str], data: Vec<u8>) -> Result<(), String> {
        let name = short_name(path[0])?;
        if path.len() == 1 {
            if self.entries.iter().any(|e| entry_name(e) == name) {
                return Err(format!("{}: twice", path[0]));
            }
            self.entries.push(Entry::File { name, data });
            return Ok(());
        }
        let pos = self.entries.iter().position(|e| entry_name(e) == name);
        let pos = match pos {
            Some(p) => p,
            None => {
                self.entries.push(Entry::Dir(Dir {
                    name,
                    entries: Vec::new(),
                }));
                self.entries.len() - 1
            }
        };
        match &mut self.entries[pos] {
            Entry::Dir(d) => d.insert(&path[1..], data),
            Entry::File { .. } => Err(format!("{}: a file, not a directory", path[0])),
        }
    }
}

fn entry_name(e: &Entry) -> [u8; 11] {
    match e {
        Entry::File { name, .. } => *name,
        Entry::Dir(d) => d.name,
    }
}

struct Volume {
    img: Vec<u8>,
    fat_start: u32,
    fat_sectors: u32,
    data_start: u32,
    clusters: u32,
    next: u32,
}

impl Volume {
    fn sector(&mut self, s: u32) -> &mut [u8] {
        let at = (PART_START + s) as usize * 512;
        &mut self.img[at..at + 512]
    }

    fn set_fat(&mut self, cluster: u32, v: u32) {
        for copy in 0..2 {
            let s = self.fat_start + copy * self.fat_sectors + cluster / 128;
            let at = (cluster % 128) as usize * 4;
            self.sector(s)[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// A chain of `n` (at least 1) clusters holding `data`; its first.
    fn store(&mut self, data: &[u8]) -> Result<u32, String> {
        let n = data.len().div_ceil(512).max(1) as u32;
        if self.next + n > self.clusters + 2 {
            return Err("the card is full".into());
        }
        let first = self.next;
        for k in 0..n {
            let c = first + k;
            self.set_fat(c, if k + 1 == n { EOC } else { c + 1 });
            let chunk =
                &data[(k as usize * 512).min(data.len())..((k as usize + 1) * 512).min(data.len())];
            let s = self.data_start + (c - 2);
            self.sector(s)[..chunk.len()].copy_from_slice(chunk);
        }
        self.next += n;
        Ok(first)
    }

    /// Store directory `d` (its entries first, then itself); its cluster.
    fn store_dir(&mut self, d: &Dir, own: u32, parent: u32) -> Result<(), String> {
        let mut raw = Vec::new();
        if own != ROOT_CLUSTER {
            raw.extend(dir_entry(b".          ", 0x10, own, 0));
            raw.extend(dir_entry(
                b"..         ",
                0x10,
                if parent == ROOT_CLUSTER { 0 } else { parent },
                0,
            ));
        } else {
            raw.extend(dir_entry(b"LOGOPHOR16 ", 0x08, 0, 0));
        }
        // Children: files stored now, directories get their cluster first.
        let mut subdirs = Vec::new();
        for e in &d.entries {
            match e {
                Entry::File { name, data } => {
                    let c = if data.is_empty() {
                        0
                    } else {
                        self.store(data)?
                    };
                    raw.extend(dir_entry(name, 0x20, c, data.len() as u32));
                }
                Entry::Dir(sub) => {
                    // Its size is known before its children are stored.
                    let len = 32 * (sub.entries.len() + 2);
                    let c = self.store(&vec![0; len])?;
                    raw.extend(dir_entry(&sub.name, 0x10, c, 0));
                    subdirs.push((sub, c));
                }
            }
        }
        self.write_chain(own, &raw);
        for (sub, c) in subdirs {
            self.store_dir(sub, c, own)?;
        }
        Ok(())
    }

    /// Write `data` over the chain starting at `c`.
    fn write_chain(&mut self, mut c: u32, data: &[u8]) {
        for chunk in data.chunks(512) {
            let s = self.data_start + (c - 2);
            self.sector(s)[..chunk.len()].copy_from_slice(chunk);
            c = self.fat(c);
            if c >= 0x0fff_fff8 {
                break;
            }
        }
    }

    fn fat(&mut self, cluster: u32) -> u32 {
        let s = self.fat_start + cluster / 128;
        let at = (cluster % 128) as usize * 4;
        u32::from_le_bytes(self.sector(s)[at..at + 4].try_into().unwrap())
    }
}

fn dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    // 2026-01-01 00:00 as the date of every entry.
    let date: u16 = (2026 - 1980) << 9 | 1 << 5 | 1;
    e[16..18].copy_from_slice(&date.to_le_bytes());
    e[24..26].copy_from_slice(&date.to_le_bytes());
    e
}

/// A FAT32 card of `mib` MiB holding `files` (card path with `/`, bytes).
pub fn fat32(mib: u32, files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if mib < MIN_MIB {
        return Err(format!("a FAT32 card needs at least {MIN_MIB} MiB"));
    }
    let total = mib * 2048;
    let part = total - PART_START;
    // FAT size: enough entries for the clusters left after the FATs.
    let mut fat_sectors = 1;
    let clusters = loop {
        let clusters = part - RESERVED - 2 * fat_sectors;
        let need = (clusters + 2).div_ceil(128);
        if need <= fat_sectors {
            break clusters;
        }
        fat_sectors = need;
    };
    if clusters < MIN_CLUSTERS {
        return Err("too few clusters for FAT32".into());
    }
    let mut v = Volume {
        img: vec![0; total as usize * 512],
        fat_start: RESERVED,
        fat_sectors,
        data_start: RESERVED + 2 * fat_sectors,
        clusters,
        next: ROOT_CLUSTER,
    };
    // MBR.
    {
        let m = &mut v.img[..512];
        let p = &mut m[446..462];
        p[4] = 0x0c; // FAT32 with LBA
        p[1..4].copy_from_slice(&[0xfe, 0xff, 0xff]);
        p[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]);
        p[8..12].copy_from_slice(&PART_START.to_le_bytes());
        p[12..16].copy_from_slice(&part.to_le_bytes());
        m[510] = 0x55;
        m[511] = 0xaa;
    }
    // Boot sector with the BPB, its backup, FSInfo.
    let mut bs = [0u8; 512];
    bs[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"LOGOPHOR");
    bs[11..13].copy_from_slice(&512u16.to_le_bytes());
    bs[13] = 1; // sectors a cluster
    bs[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    bs[16] = 2;
    bs[21] = 0xf8;
    bs[24..26].copy_from_slice(&63u16.to_le_bytes());
    bs[26..28].copy_from_slice(&255u16.to_le_bytes());
    bs[28..32].copy_from_slice(&PART_START.to_le_bytes());
    bs[32..36].copy_from_slice(&part.to_le_bytes());
    bs[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    bs[48..50].copy_from_slice(&1u16.to_le_bytes());
    bs[50..52].copy_from_slice(&6u16.to_le_bytes());
    bs[64] = 0x80;
    bs[66] = 0x29;
    bs[67..71].copy_from_slice(&0x4c31_3636u32.to_le_bytes());
    bs[71..82].copy_from_slice(b"LOGOPHOR16 ");
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xaa;
    v.sector(0).copy_from_slice(&bs);
    v.sector(6).copy_from_slice(&bs);
    v.set_fat(0, 0x0fff_fff8);
    v.set_fat(1, EOC);
    // Files and directories.
    let mut root = Dir {
        name: *b"           ",
        entries: Vec::new(),
    };
    for (path, data) in files {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err(format!("{path}: empty path"));
        }
        root.insert(&parts, data.clone())?;
    }
    let root_len = 32 * (root.entries.len() + 1);
    let rc = v.store(&vec![0; root_len])?;
    debug_assert_eq!(rc, ROOT_CLUSTER);
    v.store_dir(&root, ROOT_CLUSTER, 0)?;
    let free = clusters + 2 - v.next;
    let mut fsinfo = [0u8; 512];
    fsinfo[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&free.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&v.next.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
    v.sector(1).copy_from_slice(&fsinfo);
    v.sector(7).copy_from_slice(&fsinfo);
    Ok(v.img)
}

/// Read a file from a FAT32 card made by any formatter (MBR partition 1
/// or a partitionless volume); `None` when it is not there.
pub fn read_file(img: &[u8], path: &str) -> Option<Vec<u8>> {
    let block = |n: u64| img.get(n as usize * 512..n as usize * 512 + 512);
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let le16 = |b: &[u8], at: usize| u16::from_le_bytes(b[at..at + 2].try_into().unwrap());
    let b0 = block(0)?;
    let start = if &b0[82..87] == b"FAT32" {
        0
    } else {
        u64::from(le32(b0, 454))
    };
    let bs = block(start)?;
    let spc = u64::from(bs[13]);
    let fat = start + u64::from(le16(bs, 14));
    let data = fat + u64::from(bs[16]) * u64::from(le32(bs, 36));
    let next = |c: u32| -> u32 {
        let b = block(fat + u64::from(c) / 128).map_or(EOC, |b| le32(b, (c as usize % 128) * 4));
        b & 0x0fff_ffff
    };
    let chain = |mut c: u32| -> Vec<u8> {
        let mut out = Vec::new();
        while (2..0x0fff_fff8).contains(&c) && out.len() < img.len() {
            for k in 0..spc {
                if let Some(b) = block(data + (u64::from(c) - 2) * spc + k) {
                    out.extend_from_slice(b);
                }
            }
            c = next(c);
        }
        out
    };
    let mut dir = chain(le32(bs, 44));
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for (k, part) in parts.iter().enumerate() {
        let want = short_name(part).ok()?;
        let e = dir
            .chunks(32)
            .take_while(|e| e[0] != 0)
            .find(|e| e[0] != 0xe5 && e[11] & 0x0f != 0x0f && e[..11] == want)?;
        let c = u32::from(le16(e, 20)) << 16 | u32::from(le16(e, 26));
        if k + 1 == parts.len() {
            let mut f = chain(c);
            f.truncate(le32(e, 28) as usize);
            return Some(f);
        }
        dir = chain(c);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_card_and_reads_its_files_back() {
        let big: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        let files = vec![
            ("BOOT/BOILER.BIN".to_string(), big.clone()),
            ("readme.txt".to_string(), b"hello".to_vec()),
            ("BOOT/EMPTY".to_string(), Vec::new()),
            ("A/B/C/DEEP.DAT".to_string(), vec![1, 2, 3]),
        ];
        let img = fat32(64, &files).unwrap();
        assert_eq!(img.len(), 64 << 20);
        assert_eq!(&img[510..512], &[0x55, 0xaa]);
        let bs = &img[2048 * 512..];
        assert_eq!(&bs[82..90], b"FAT32   ");
        assert_eq!(read_file(&img, "BOOT/BOILER.BIN").unwrap(), big);
        assert_eq!(read_file(&img, "/README.TXT").unwrap(), b"hello");
        assert_eq!(read_file(&img, "boot/empty").unwrap(), b"");
        assert_eq!(read_file(&img, "A/B/C/DEEP.DAT").unwrap(), [1, 2, 3]);
        assert!(read_file(&img, "BOOT/NONE.BIN").is_none());
        // Clusters of a 5000-byte file: 10, chained.
        let fat = &img[(2048 + 32) as usize * 512..];
        let c = |n: usize| u32::from_le_bytes(fat[4 * n..4 * n + 4].try_into().unwrap());
        assert_eq!((c(0), c(1), c(2)), (0x0fff_fff8, EOC, EOC));
    }

    #[test]
    fn refuses_bad_names_and_small_cards() {
        assert!(fat32(64, &[("TOOLONGNAME.BIN".into(), vec![])]).is_err());
        assert!(fat32(64, &[("A.TOOL".into(), vec![])]).is_err());
        assert!(fat32(64, &[("A".into(), vec![]), ("a".into(), vec![])]).is_err());
        assert!(fat32(16, &[]).is_err());
    }
}
