//! D64: 1541 disk image, the 256-byte sectors of 35 (or 40) tracks in
//! order, optionally followed by one error byte per sector.
//!
//! Sectors per track: 21 (tracks 1–17), 19 (18–24), 18 (25–30), 17 (31+).
//! Track 18 sector 0 is the BAM with the disk name at `$90` and the ID at
//! `$A2`; the directory chain starts at track 18 sector 1. Every sector of a
//! file starts with the track/sector of the next one; a next track of 0
//! means the last sector, with the last used byte in the second byte.
//! Source: Commodore 1541 User's Guide, "Inside the 1541" (Immers/Neufeld).
//!
//! D71 (1571, double-sided) is the same layout for 70 tracks: tracks 36–70
//! are side 1 with the sector counts and speed zones of tracks 1–35. The
//! BAM keeps their free counts in track 18 sector 0 at `$DD–$FF` and their
//! bitmaps (3 bytes per track) in track 53 sector 0, which the 1571 keeps
//! reserved; BAM byte 3 is `$80`. Source: Commodore 1571 User's Guide.

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
    /// A D71: tracks 36–70 are side 1.
    pub double_sided: bool,
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

/// A byte in the BAM: track, sector, offset.
type BamByte = (u8, u8, usize);

/// Sectors of a D71 (both sides).
const D71_SECTORS: usize = 2 * 683;

impl D64 {
    /// A D64 (35, 40 or 42 tracks) or D71 image, with or without error
    /// bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let layouts = [
            (35u8, false, total_sectors(35)),
            (40, false, total_sectors(40)),
            (42, false, total_sectors(42)),
            (70, true, D71_SECTORS),
        ];
        for (tracks, double_sided, n) in layouts {
            if bytes.len() == n * SECTOR_SIZE {
                return Ok(Self {
                    tracks,
                    double_sided,
                    data: bytes.to_vec(),
                    errors: None,
                });
            }
            if bytes.len() == n * (SECTOR_SIZE + 1) {
                let split = n * SECTOR_SIZE;
                return Ok(Self {
                    tracks,
                    double_sided,
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

    /// An empty formatted 35-track disk, as the 1541's NEW command leaves
    /// it: BAM with every block free except track 18 sectors 0 and 1.
    pub fn blank(name: &[u8], id: [u8; 2]) -> Self {
        let mut d = Self {
            tracks: 35,
            double_sided: false,
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
        // BAM entry per track: free count, then a bit per sector (1 = free).
        for t in 1..=35u8 {
            let n = u32::from(sectors_per_track(t));
            let e = 4 * usize::from(t);
            bam[e] = n as u8;
            bam[e + 1..e + 4].copy_from_slice(&((1u32 << n) - 1).to_le_bytes()[..3]);
        }
        let dir = d.sector_mut(18, 1).unwrap_or_else(|| unreachable!());
        dir[1] = 0xff;
        d.allocate(18, 0);
        d.allocate(18, 1);
        d
    }

    /// An empty double-sided 1571 disk (D71): side 1 free except track 53,
    /// which holds the second half of the BAM.
    pub fn blank_d71(name: &[u8], id: [u8; 2]) -> Self {
        let single = Self::blank(name, id);
        let mut data = single.data;
        data.resize(D71_SECTORS * SECTOR_SIZE, 0);
        let mut d = Self {
            tracks: 70,
            double_sided: true,
            data,
            errors: None,
        };
        let mut bitmaps = [0u8; 105];
        for t in 36..=70u8 {
            let n = u32::from(d.sectors_in(t));
            let i = usize::from(t - 36);
            if let Some(bam) = d.sector_mut(18, 0) {
                bam[0xdd + i] = n as u8;
            }
            bitmaps[i * 3..i * 3 + 3].copy_from_slice(&((1u32 << n) - 1).to_le_bytes()[..3]);
        }
        if let Some(bam) = d.sector_mut(18, 0) {
            bam[3] = 0x80;
        }
        if let Some(b) = d.sector_mut(53, 0) {
            b[..105].copy_from_slice(&bitmaps);
        }
        for s in 0..d.sectors_in(53) {
            d.allocate(53, s);
        }
        d
    }

    /// Sectors on a track (side 1 of a D71 repeats tracks 1–35).
    pub fn sectors_in(&self, track: u8) -> u8 {
        if self.double_sided && track > 35 {
            sectors_per_track(track - 35)
        } else {
            sectors_per_track(track)
        }
    }

    /// Speed zone of a track (see [`speed_zone`]).
    pub fn zone_of(&self, track: u8) -> u8 {
        if self.double_sided && track > 35 {
            speed_zone(track - 35)
        } else {
            speed_zone(track)
        }
    }

    fn total(&self, tracks: u8) -> usize {
        (1..=tracks).map(|t| usize::from(self.sectors_in(t))).sum()
    }

    /// Where the BAM keeps a track: the free count and the 3-byte bitmap.
    fn bam_slot(&self, track: u8) -> Option<(BamByte, BamByte)> {
        match track {
            1..=35 => {
                let e = 4 * usize::from(track);
                Some(((18, 0, e), (18, 0, e + 1)))
            }
            36..=70 if self.double_sided => {
                let i = usize::from(track - 36);
                Some(((18, 0, 0xdd + i), (53, 0, i * 3)))
            }
            _ => None,
        }
    }

    /// Mark a block used in the BAM (no-op if it already is).
    fn allocate(&mut self, track: u8, sector: u8) {
        if !self.is_free(track, sector) {
            return;
        }
        let Some(((ct, cs, co), (bt, bs, bo))) = self.bam_slot(track) else {
            return;
        };
        if let Some(b) = self.sector_mut(bt, bs) {
            b[bo + usize::from(sector / 8)] &= !(1 << (sector % 8));
        }
        if let Some(b) = self.sector_mut(ct, cs) {
            b[co] = b[co].saturating_sub(1);
        }
    }

    fn index(&self, track: u8, sector: u8) -> Option<usize> {
        if track == 0 || track > self.tracks || sector >= self.sectors_in(track) {
            return None;
        }
        Some(self.total(track - 1) + usize::from(sector))
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
        let mut seen = vec![false; self.total(self.tracks)];
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
    /// 18 sector 1 and the blocks are marked used in the BAM. A simple writer
    /// for generated test disks: the directory does not grow beyond its
    /// first sector.
    pub fn add_prg(&mut self, name: &[u8], prg: &[u8]) -> Result<(), MediaError> {
        let used = self.used_sectors()?;
        let order = (1..=17u8).rev().chain(19..=self.tracks);
        let free: Vec<(u8, u8)> = order
            .flat_map(|t| (0..self.sectors_in(t)).map(move |s| (t, s)))
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
            self.allocate(t, s);
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
        let entry = self
            .directory()?
            .into_iter()
            .find(|e| e.file_type & 7 == 2 && name_matches(pattern, &e.name))
            .ok_or(MediaError::Format("file not found".into()))?;
        let bytes = self.chain(entry.track, entry.sector)?;
        Ok((entry, Prg::parse(&bytes)?))
    }
}

/// DOS-style file operations: what the 1541 does to the image on SAVE,
/// SCRATCH and `LOAD"$"`, for the virtual drive and host folders.
impl D64 {
    /// Whether a block is free in the BAM.
    fn is_free(&self, track: u8, sector: u8) -> bool {
        if track == 0 || track > self.tracks || sector >= self.sectors_in(track) {
            return false;
        }
        let Some((_, (bt, bs, bo))) = self.bam_slot(track) else {
            return false;
        };
        self.sector(bt, bs)
            .is_some_and(|b| b[bo + usize::from(sector / 8)] & (1 << (sector % 8)) != 0)
    }

    fn release(&mut self, track: u8, sector: u8) {
        if track == 0 || track > self.tracks || sector >= self.sectors_in(track) {
            return;
        }
        if self.is_free(track, sector) {
            return;
        }
        let Some(((ct, cs, co), (bt, bs, bo))) = self.bam_slot(track) else {
            return;
        };
        if let Some(b) = self.sector_mut(bt, bs) {
            b[bo + usize::from(sector / 8)] |= 1 << (sector % 8);
        }
        if let Some(b) = self.sector_mut(ct, cs) {
            b[co] = b[co].saturating_add(1);
        }
    }

    /// The tracks the DOS keeps files on: all but the directory (18) and,
    /// on a D71, the second BAM (53).
    fn data_tracks(&self) -> impl Iterator<Item = u8> + '_ {
        let last = if self.double_sided {
            70
        } else {
            self.tracks.min(35)
        };
        (1..=last).filter(move |&t| t != 18 && !(self.double_sided && t == 53))
    }

    /// Free blocks as the directory reports them (track 18, and track 53 of
    /// a D71, not counted).
    pub fn blocks_free(&self) -> u16 {
        self.data_tracks()
            .filter_map(|t| {
                let ((ct, cs, co), _) = self.bam_slot(t)?;
                self.sector(ct, cs).map(|b| u16::from(b[co]))
            })
            .sum()
    }

    /// The next block for a file like the DOS picks it: on the same track
    /// 10 sectors on (interleave), else on the next track away from the
    /// directory; the first block on the free track nearest track 18 (then,
    /// on a D71, nearest track 53).
    fn next_free(&self, prev: Option<(u8, u8)>) -> Option<(u8, u8)> {
        let in_track = |t: u8, from: u8| {
            let n = self.sectors_in(t);
            (0..n)
                .map(|i| (from + i) % n)
                .find(|&s| self.is_free(t, s))
                .map(|s| (t, s))
        };
        let usable = |t: u8| self.data_tracks().any(|d| d == t);
        if let Some((t, s)) = prev {
            if let Some(b) = in_track(t, s + 10) {
                return Some(b);
            }
            let away: Vec<u8> = match t {
                1..=17 => (1..t).rev().collect(),
                19..=35 => (t + 1..=35).collect(),
                36..=52 => (36..t).rev().collect(),
                _ => (t + 1..=70).collect(),
            };
            if let Some(b) = away
                .into_iter()
                .filter(|&t| usable(t))
                .find_map(|t| in_track(t, 0))
            {
                return Some(b);
            }
        }
        let side0 = (1..18u8).flat_map(|d| [18 - d, 18 + d]);
        let side1 = (1..18u8).flat_map(|d| [53 - d, 53 + d]);
        side0
            .chain(side1)
            .filter(|&t| usable(t))
            .find_map(|t| in_track(t, 0))
    }

    /// Write a file (`file_type` 1 SEQ, 2 PRG, 3 USR) with `data` as stored
    /// (a PRG includes its load address) and add its directory entry; the
    /// directory grows by blocks on track 18 when its sectors are full.
    pub fn write_file(
        &mut self,
        name: &[u8],
        file_type: u8,
        data: &[u8],
    ) -> Result<(), MediaError> {
        let chunks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(254).collect()
        };
        if chunks.len() > usize::from(self.blocks_free()) {
            return Err(MediaError::Format("72, DISK FULL".into()));
        }
        let slot = self.directory_slot()?;
        let mut blocks = Vec::with_capacity(chunks.len());
        let mut prev = None;
        for _ in 0..chunks.len() {
            let b = self
                .next_free(prev)
                .ok_or_else(|| MediaError::Format("72, DISK FULL".into()))?;
            self.allocate(b.0, b.1);
            blocks.push(b);
            prev = Some(b);
        }
        for (i, c) in chunks.iter().enumerate() {
            let (t, s) = blocks[i];
            let next = blocks.get(i + 1).copied();
            let b = self.sector_mut(t, s).unwrap_or_default();
            b.fill(0);
            match next {
                Some((nt, ns)) => {
                    b[0] = nt;
                    b[1] = ns;
                }
                None => b[1] = c.len() as u8 + 1,
            }
            b[2..2 + c.len()].copy_from_slice(c);
        }
        let (dt, ds, offset) = slot;
        let (t0, s0) = blocks[0];
        let dir = self.sector_mut(dt, ds).unwrap_or_default();
        let e = &mut dir[offset..offset + 32];
        e[2..32].fill(0);
        e[2] = 0x80 | (file_type & 7);
        e[3] = t0;
        e[4] = s0;
        e[5..21].fill(0xa0);
        let n = name.len().min(16);
        e[5..5 + n].copy_from_slice(&name[..n]);
        e[30..32].copy_from_slice(&(chunks.len() as u16).to_le_bytes());
        Ok(())
    }

    /// A free directory entry: (track, sector, offset), adding a directory
    /// block on track 18 (interleave 3) when all are used.
    fn directory_slot(&mut self) -> Result<(u8, u8, usize), MediaError> {
        let (mut t, mut s) = (18u8, 1u8);
        for _ in 0..64 {
            let b = self
                .sector(t, s)
                .ok_or_else(|| MediaError::Format(format!("D64: bad directory link {t}/{s}")))?;
            if let Some(i) = b.chunks(32).position(|e| e[2] == 0) {
                return Ok((t, s, i * 32));
            }
            if b[0] == 0 {
                let n = self.sectors_in(18);
                let new = (0..n)
                    .map(|i| (s + 3 + i) % n)
                    .find(|&x| self.is_free(18, x))
                    .ok_or_else(|| MediaError::Format("72, DISK FULL".into()))?;
                self.allocate(18, new);
                let last = self.sector_mut(t, s).unwrap_or_default();
                last[0] = 18;
                last[1] = new;
                let block = self.sector_mut(18, new).unwrap_or_default();
                block.fill(0);
                block[1] = 0xff;
                return Ok((18, new, 0));
            }
            (t, s) = (b[0], b[1]);
        }
        Err(MediaError::Format("D64: directory loops".into()))
    }

    /// Delete the files matching `pattern` (as `S:pattern` does); returns
    /// how many.
    pub fn scratch(&mut self, pattern: &[u8]) -> Result<usize, MediaError> {
        let mut count = 0;
        let (mut t, mut s) = (18u8, 1u8);
        let mut visited = 0;
        while t != 0 && visited < 64 {
            visited += 1;
            let block = self.sector(t, s).map(<[u8]>::to_vec).unwrap_or_default();
            if block.len() < SECTOR_SIZE {
                break;
            }
            for i in 0..8 {
                let e = &block[i * 32..i * 32 + 32];
                let raw = &e[5..21];
                let len = raw.iter().position(|&c| c == 0xa0).unwrap_or(16);
                if e[2] == 0 || !name_matches(pattern, &raw[..len]) {
                    continue;
                }
                let (mut ft, mut fs) = (e[3], e[4]);
                let mut chain = 0;
                while ft != 0 && chain < 800 {
                    chain += 1;
                    let next = self.sector(ft, fs).map(|b| (b[0], b[1]));
                    self.release(ft, fs);
                    let Some(n) = next else { break };
                    (ft, fs) = n;
                }
                if let Some(b) = self.sector_mut(t, s) {
                    b[i * 32 + 2] = 0;
                }
                count += 1;
            }
            (t, s) = (block[0], block[1]);
        }
        Ok(count)
    }

    /// The first file matching `pattern` (any type): its entry and bytes as
    /// stored.
    pub fn read_file(&self, pattern: &[u8]) -> Result<(DirEntry, Vec<u8>), MediaError> {
        let entry = self
            .directory()?
            .into_iter()
            .find(|e| e.file_type & 7 != 0 && name_matches(pattern, &e.name))
            .ok_or(MediaError::Format("62, FILE NOT FOUND".into()))?;
        let bytes = self.chain(entry.track, entry.sector)?;
        Ok((entry, bytes))
    }

    /// The directory as the 1541 sends it for `LOAD"$"`: a BASIC program at
    /// `$0401` with the header (reversed name, ID, "2A"), one line per file
    /// (blocks, quoted name, type) and BLOCKS FREE.
    pub fn listing(&self) -> Vec<u8> {
        let mut out = vec![0x01, 0x04];
        let mut line = |number: u16, text: &[u8]| {
            out.extend_from_slice(&[0x01, 0x01]);
            out.extend_from_slice(&number.to_le_bytes());
            out.extend_from_slice(text);
            out.push(0);
        };
        let bam = self.sector(18, 0).map(<[u8]>::to_vec).unwrap_or_default();
        let mut header = vec![0x12, b'"'];
        header.extend(
            bam[0x90..0xa0]
                .iter()
                .map(|&c| if c == 0xa0 { b' ' } else { c }),
        );
        header.extend_from_slice(b"\" ");
        header.extend_from_slice(&[bam[0xa2], bam[0xa3], b' ', bam[0xa5], bam[0xa6]]);
        line(0, &header);
        for e in self.directory().unwrap_or_default() {
            let mut text = Vec::new();
            let pad = match e.blocks {
                0..=9 => 3,
                10..=99 => 2,
                _ => 1,
            };
            text.extend(std::iter::repeat_n(b' ', pad));
            text.push(b'"');
            text.extend_from_slice(&e.name);
            text.push(b'"');
            text.extend(std::iter::repeat_n(b' ', 17 - e.name.len().min(16)));
            text.push(if e.file_type & 0x80 == 0 { b'*' } else { b' ' });
            text.extend_from_slice(match e.file_type & 7 {
                0 => b"DEL",
                1 => b"SEQ",
                2 => b"PRG",
                3 => b"USR",
                _ => b"REL",
            });
            line(e.blocks, &text);
        }
        line(self.blocks_free(), b"BLOCKS FREE.             ");
        out.extend_from_slice(&[0, 0]);
        out
    }
}

/// CBM DOS name matching: `*` matches any rest, `?` any one character.
pub fn name_matches(pattern: &[u8], name: &[u8]) -> bool {
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

    #[test]
    fn blank_disk_has_664_blocks_free() {
        let mut d = D64::blank(b"FREE", *b"FR");
        let free = |d: &D64| -> u32 {
            let bam = d.sector(18, 0).unwrap();
            (1..=35u8)
                .filter(|&t| t != 18)
                .map(|t| u32::from(bam[4 * usize::from(t)]))
                .sum()
        };
        assert_eq!(free(&d), 664);
        assert_eq!(
            d.sector(18, 0).unwrap()[4 * 18],
            17,
            "BAM and directory used"
        );
        d.add_prg(b"F", &[0x01, 0x08, 0, 0]).unwrap();
        assert_eq!(free(&d), 663);
    }

    #[test]
    fn dos_writes_scratches_and_lists() {
        let mut d = D64::blank(b"WORK", *b"WK");
        let big: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let mut prg = vec![0x01, 0x08];
        prg.extend_from_slice(&big);
        d.write_file(b"BIG", 2, &prg).unwrap();
        assert_eq!(d.blocks_free(), 664 - 12);
        let (e, bytes) = d.read_file(b"B*").unwrap();
        assert_eq!((e.blocks, bytes), (12, prg.clone()));
        // First block nearest the directory, then interleave 10.
        assert_eq!((e.track, e.sector), (17, 0));
        assert_eq!(d.sector(17, 0).unwrap()[..2], [17, 10]);
        // 20 more files fill the first directory block and add another.
        for i in 0..20u8 {
            d.write_file(&[b'F', b'A' + i], 1, b"x").unwrap();
        }
        assert_eq!(d.directory().unwrap().len(), 21);
        assert_eq!(d.sector(18, 1).unwrap()[0], 18, "directory grew");
        assert_eq!(d.scratch(b"F*").unwrap(), 20);
        assert_eq!(d.directory().unwrap().len(), 1);
        assert_eq!(d.blocks_free(), 664 - 12);
        assert!(d.read_file(b"FA").is_err());
        // The listing is a BASIC program at $0401 ending in BLOCKS FREE.
        let l = d.listing();
        assert_eq!(&l[..2], &[0x01, 0x04]);
        assert!(l.windows(5).any(|w| w == b"\"BIG\""));
        assert!(l.windows(11).any(|w| w == b"BLOCKS FREE"));
        assert_eq!(&l[l.len() - 2..], &[0, 0]);
    }

    #[test]
    fn d71_is_double_sided() {
        let mut d = D64::blank_d71(b"SIDES", *b"DS");
        assert!(d.double_sided);
        assert_eq!(d.to_bytes().len(), 349_696);
        assert_eq!(d.blocks_free(), 1328);
        assert_eq!(d.sectors_in(36), 21);
        assert_eq!(d.zone_of(53), 2);
        // Fill side 0, then a file goes to side 1 near track 53.
        let big = vec![0x55u8; 664 * 254];
        d.write_file(b"SIDE0", 2, &big).unwrap();
        assert_eq!(d.blocks_free(), 664);
        d.write_file(b"SIDE1", 2, &[0x01, 0x08, 1]).unwrap();
        let (e, data) = d.read_file(b"SIDE1").unwrap();
        assert_eq!(e.track, 52);
        assert_eq!(data, [0x01, 0x08, 1]);
        let back = D64::parse(&d.to_bytes()).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.read_file(b"SIDE0").unwrap().1, big);
    }
}
