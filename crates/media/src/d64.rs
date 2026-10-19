//! D64: 1541 disk image, the 256-byte sectors of 35 (or 40) tracks in
//! order, optionally followed by one error byte per sector.
//!
//! Sectors per track: 21 (tracks 1–17), 19 (18–24), 18 (25–30), 17 (31+).
//! Track 18 sector 0 is the BAM with the disk name at `$90` and the ID at
//! `$A2`; the directory chain starts at track 18 sector 1. Every sector of a
//! file starts with the track/sector of the next one; a next track of 0
//! means the last sector, with the last used byte in the second byte.
//! Source: Commodore 1541 User's Guide, "Inside the 1541" (Immers/Neufeld).

use emu_core::media::MediaError;

use crate::prg::Prg;

pub const SECTOR_SIZE: usize = 256;

pub fn sectors_per_track(track: u8) -> u8 {
    match track {
        1..=17 => 21,
        18..=24 => 19,
        25..=30 => 18,
        _ => 17,
    }
}

/// Speed zone of a track (3 = outermost, fastest), as set in VIA2 PB5/6.
pub fn speed_zone(track: u8) -> u8 {
    match track {
        1..=17 => 3,
        18..=24 => 2,
        25..=30 => 1,
        _ => 0,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct D64 {
    pub tracks: u8,
    data: Vec<u8>,
    /// One code per sector; 1 = no error (absent in most images).
    errors: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// C64 file type byte (bit 7 = closed, low bits: 0 DEL 1 SEQ 2 PRG 3 USR 4 REL).
    pub file_type: u8,
    pub track: u8,
    pub sector: u8,
    /// PETSCII name without padding.
    pub name: Vec<u8>,
    pub blocks: u16,
}

fn total_sectors(tracks: u8) -> usize {
    (1..=tracks)
        .map(|t| usize::from(sectors_per_track(t)))
        .sum()
}

impl D64 {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        for tracks in [35u8, 40, 42] {
            let n = total_sectors(tracks);
            if bytes.len() == n * SECTOR_SIZE {
                return Ok(Self {
                    tracks,
                    data: bytes.to_vec(),
                    errors: None,
                });
            }
            if bytes.len() == n * (SECTOR_SIZE + 1) {
                let split = n * SECTOR_SIZE;
                return Ok(Self {
                    tracks,
                    data: bytes[..split].to_vec(),
                    errors: Some(bytes[split..].to_vec()),
                });
            }
        }
        Err(MediaError::Format(format!(
            "D64: unexpected size {} bytes",
            bytes.len()
        )))
    }

    /// An empty formatted 35-track disk (for tests).
    pub fn blank(name: &[u8], id: [u8; 2]) -> Self {
        let mut d = Self {
            tracks: 35,
            data: vec![0; total_sectors(35) * SECTOR_SIZE],
            errors: None,
        };
        let bam = d.sector_mut(18, 0).unwrap_or_else(|| unreachable!());
        bam[0] = 18;
        bam[1] = 1;
        bam[2] = 0x41;
        bam[0x90..0xab].fill(0xa0);
        bam[0x90..0x90 + name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
        bam[0xa2] = id[0];
        bam[0xa3] = id[1];
        bam[0xa5] = b'2';
        bam[0xa6] = b'A';
        let dir = d.sector_mut(18, 1).unwrap_or_else(|| unreachable!());
        dir[1] = 0xff;
        d
    }

    fn index(&self, track: u8, sector: u8) -> Option<usize> {
        if track == 0 || track > self.tracks || sector >= sectors_per_track(track) {
            return None;
        }
        Some(total_sectors(track - 1) + usize::from(sector))
    }

    pub fn sector(&self, track: u8, sector: u8) -> Option<&[u8]> {
        let i = self.index(track, sector)?;
        Some(&self.data[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE])
    }

    pub fn sector_mut(&mut self, track: u8, sector: u8) -> Option<&mut [u8]> {
        let i = self.index(track, sector)?;
        Some(&mut self.data[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE])
    }

    /// Error code of a sector (1 = none).
    pub fn error(&self, track: u8, sector: u8) -> u8 {
        match (&self.errors, self.index(track, sector)) {
            (Some(e), Some(i)) => e[i],
            _ => 1,
        }
    }

    /// The disk ID bytes from the BAM, as written into sector headers.
    pub fn id(&self) -> [u8; 2] {
        self.sector(18, 0).map_or([0, 0], |b| [b[0xa2], b[0xa3]])
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.data.clone();
        if let Some(e) = &self.errors {
            v.extend_from_slice(e);
        }
        v
    }

    /// Follow a sector chain; at most one visit per sector (no loops).
    fn chain(&self, track: u8, sector: u8) -> Result<Vec<u8>, MediaError> {
        let mut out = Vec::new();
        let (mut t, mut s) = (track, sector);
        let mut seen = vec![false; total_sectors(self.tracks)];
        loop {
            let i = self
                .index(t, s)
                .ok_or_else(|| MediaError::Format(format!("D64: bad link {t}/{s}")))?;
            if std::mem::replace(&mut seen[i], true) {
                return Err(MediaError::Format("D64: sector chain loops".into()));
            }
            let b = self.sector(t, s).unwrap_or_default();
            if b[0] == 0 {
                let last = usize::from(b[1]).max(1);
                out.extend_from_slice(&b[2..=last]);
                return Ok(out);
            }
            out.extend_from_slice(&b[2..]);
            (t, s) = (b[0], b[1]);
        }
    }

    pub fn directory(&self) -> Result<Vec<DirEntry>, MediaError> {
        let mut entries = Vec::new();
        let (mut t, mut s) = (18u8, 1u8);
        let mut visited = 0;
        while t != 0 {
            visited += 1;
            if visited > 64 {
                return Err(MediaError::Format("D64: directory loops".into()));
            }
            let b = self
                .sector(t, s)
                .ok_or_else(|| MediaError::Format(format!("D64: bad directory link {t}/{s}")))?;
            for e in b.chunks(32) {
                if e[2] == 0 {
                    continue;
                }
                let raw = &e[5..21];
                let len = raw.iter().position(|&c| c == 0xa0).unwrap_or(16);
                entries.push(DirEntry {
                    file_type: e[2],
                    track: e[3],
                    sector: e[4],
                    name: raw[..len].to_vec(),
                    blocks: u16::from_le_bytes([e[30], e[31]]),
                });
            }
            (t, s) = (b[0], b[1]);
        }
        Ok(entries)
    }

    /// Add a PRG file (load address + data in `prg`): sectors are taken in
    /// order from track 17 downwards, then from 19 upwards, skipping sectors
    /// already linked by other files; the directory gets an entry in track
    /// 18 sector 1. A simple writer for generated test disks: the BAM is not
    /// updated and the directory does not grow beyond its first sector.
    pub fn add_prg(&mut self, name: &[u8], prg: &[u8]) -> Result<(), MediaError> {
        let used = self.used_sectors()?;
        let order = (1..=17u8).rev().chain(19..=self.tracks);
        let free: Vec<(u8, u8)> = order
            .flat_map(|t| (0..sectors_per_track(t)).map(move |s| (t, s)))
            .filter(|ts| !used.contains(ts))
            .collect();
        let chunks: Vec<&[u8]> = prg.chunks(254).collect();
        if chunks.len() > free.len() {
            return Err(MediaError::Format("D64: disk full".into()));
        }
        let slot = self
            .sector(18, 1)
            .and_then(|d| d.chunks(32).position(|e| e[2] == 0))
            .ok_or_else(|| MediaError::Format("D64: directory full".into()))?;
        for (i, c) in chunks.iter().enumerate() {
            let (t, s) = free[i];
            let next = free.get(i + 1).filter(|_| i + 1 < chunks.len());
            let b = self.sector_mut(t, s).unwrap_or_default();
            b.fill(0);
            match next {
                Some(&(nt, ns)) => {
                    b[0] = nt;
                    b[1] = ns;
                }
                None => b[1] = c.len() as u8 + 1,
            }
            b[2..2 + c.len()].copy_from_slice(c);
        }
        let (t0, s0) = free[0];
        let dir = self.sector_mut(18, 1).unwrap_or_default();
        let e = &mut dir[slot * 32..slot * 32 + 32];
        e[2] = 0x82;
        e[3] = t0;
        e[4] = s0;
        e[5..21].fill(0xa0);
        let n = name.len().min(16);
        e[5..5 + n].copy_from_slice(&name[..n]);
        e[30..32].copy_from_slice(&(chunks.len() as u16).to_le_bytes());
        Ok(())
    }

    fn used_sectors(&self) -> Result<Vec<(u8, u8)>, MediaError> {
        let mut used = Vec::new();
        for e in self.directory()? {
            let (mut t, mut s) = (e.track, e.sector);
            while t != 0 && !used.contains(&(t, s)) {
                used.push((t, s));
                let b = self.sector(t, s).unwrap_or_default();
                (t, s) = (b[0], b[1]);
            }
        }
        Ok(used)
    }

    /// The first PRG matching `pattern` (`*` = any rest, `?` = any char),
    /// like `LOAD"name",8`.
    pub fn load(&self, pattern: &[u8]) -> Result<(DirEntry, Prg), MediaError> {
        let matches = |name: &[u8]| {
            let mut i = 0;
            for (k, &p) in pattern.iter().enumerate() {
                match p {
                    b'*' => return true,
                    b'?' if i < name.len() => i += 1,
                    _ if i < name.len() && name[i] == p => i += 1,
                    _ => return k == pattern.len(),
                }
            }
            i == name.len()
        };
        let entry = self
            .directory()?
            .into_iter()
            .find(|e| e.file_type & 7 == 2 && matches(&e.name))
            .ok_or(MediaError::Format("file not found".into()))?;
        let bytes = self.chain(entry.track, entry.sector)?;
        Ok((entry, Prg::parse(&bytes)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a PRG into consecutive sectors of track 17 and a directory entry.
    pub(crate) fn disk_with(name: &[u8], prg: &[u8]) -> D64 {
        let mut d = D64::blank(b"TEST DISK", *b"AB");
        let mut chunks: Vec<&[u8]> = prg.chunks(254).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        let n = chunks.len();
        for (i, c) in chunks.iter().enumerate() {
            let s = d.sector_mut(17, i as u8).unwrap();
            if i + 1 < n {
                s[0] = 17;
                s[1] = i as u8 + 1;
            } else {
                s[0] = 0;
                s[1] = c.len() as u8 + 1;
            }
            s[2..2 + c.len()].copy_from_slice(c);
        }
        let dir = d.sector_mut(18, 1).unwrap();
        dir[2] = 0x82;
        dir[3] = 17;
        dir[4] = 0;
        dir[5..21].fill(0xa0);
        dir[5..5 + name.len()].copy_from_slice(name);
        dir[30] = n as u8;
        d
    }

    #[test]
    fn geometry() {
        assert_eq!(total_sectors(35), 683);
        assert_eq!(total_sectors(35) * 256, 174_848);
        assert_eq!(speed_zone(18), 2);
        let d = D64::parse(&vec![0; 174_848 + 683]).unwrap();
        assert_eq!(d.tracks, 35);
        assert!(d.sector(36, 0).is_none() && d.sector(18, 19).is_none());
        assert!(D64::parse(&[0; 1000]).is_err());
    }

    #[test]
    fn directory_and_load() {
        let prg: Vec<u8> = [0x01, 0x08]
            .into_iter()
            .chain((0..600).map(|i| i as u8))
            .collect();
        let d = disk_with(b"HELLO", &prg);
        let dir = d.directory().unwrap();
        assert_eq!(dir.len(), 1);
        assert_eq!(dir[0].name, b"HELLO");
        assert_eq!(dir[0].blocks, 3);
        let (_, p) = d.load(b"*").unwrap();
        assert_eq!(p.to_bytes(), prg);
        assert!(d.load(b"HEL*").is_ok());
        assert!(d.load(b"H?LLO").is_ok());
        assert!(d.load(b"HELL").is_err());
        assert_eq!(d.id(), *b"AB");
        assert_eq!(D64::parse(&d.to_bytes()).unwrap(), d);
    }

    #[test]
    fn add_prg_writes_a_loadable_file() {
        let mut d = D64::blank(b"GEN", *b"ID");
        let big: Vec<u8> = [0x01, 0x08]
            .into_iter()
            .chain((0..1000).map(|i| (i * 7) as u8))
            .collect();
        d.add_prg(b"ONE", &[0x00, 0xc0, 0x60]).unwrap();
        d.add_prg(b"TWO", &big).unwrap();
        assert_eq!(d.load(b"ONE").unwrap().1.to_bytes(), [0x00, 0xc0, 0x60]);
        assert_eq!(d.load(b"TWO").unwrap().1.to_bytes(), big);
        let dir = d.directory().unwrap();
        assert_eq!(dir.len(), 2);
        assert_eq!(dir[1].blocks, 4);
    }

    #[test]
    fn looping_chains_are_errors_not_hangs() {
        let mut d = disk_with(b"X", &[1, 8, 0]);
        let s = d.sector_mut(17, 0).unwrap();
        s[0] = 17;
        s[1] = 0;
        assert!(d.load(b"X").is_err());
    }
}
