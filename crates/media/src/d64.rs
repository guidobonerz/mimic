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
//!
//! D81 (1581): 80 tracks of 40 sectors. Track 40 holds the header (sector
//! 0: name at `$04`, ID at `$16`, DOS type "3D" at `$19`), the BAM
//! (sectors 1 and 2 for tracks 1–40 and 41–80: 6 bytes per track from
//! `$10`, the free count and a 40-bit map) and the directory from sector 3.
//! Files use interleave 1. Source: Commodore 1581 User's Guide.
//!
//! D80 (8050): 77 tracks of 29 (1–39), 27 (40–53), 25 (54–64) and 23
//! (65–77) sectors; D82 (8250) has a second side, tracks 78–154 like 1–77.
//! Track 39 holds the header (sector 0: name at `$06`, ID at `$18`, DOS
//! type "2C" at `$1B`) and the directory from sector 1; the BAM sectors
//! 38/0, 38/3 (and 38/6, 38/9 on a D82) cover 50 tracks each: low and
//! high track at `$04`/`$05`, then 5 bytes per track from `$06`, the free
//! count and a 32-bit map. Only the directory track is left out of BLOCKS
//! FREE (2052, 4133 on a D82). Source: CBM 8050/8250 user's manual.

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

/// The disk layouts of the CBM DOS images.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 1541: 35 (or 40/42) tracks.
    D64,
    /// 1571: 70 tracks, 36–70 on side 1.
    D71,
    /// 1581: 80 tracks of 40 sectors.
    D81,
    /// 8050: 77 tracks.
    D80,
    /// 8250: 154 tracks, 78–154 on side 1.
    D82,
}

/// Sectors per track of the 8050/8250 (one side).
fn sectors_8050(track: u8) -> u8 {
    match track {
        1..=39 => 29,
        40..=53 => 27,
        54..=64 => 25,
        _ => 23,
    }
}

/// A D64, D71, D81, D80 or D82 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct D64 {
    pub tracks: u8,
    pub format: Format,
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
/// Sectors of a D81.
const D81_SECTORS: usize = 80 * 40;
/// Sectors of a D80 (one 8050 side).
const D80_SECTORS: usize = 2083;

impl D64 {
    /// A D64 (35, 40 or 42 tracks), D71, D81, D80 or D82 image, with or
    /// without error bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let layouts = [
            (35u8, Format::D64, total_sectors(35)),
            (40, Format::D64, total_sectors(40)),
            (42, Format::D64, total_sectors(42)),
            (70, Format::D71, D71_SECTORS),
            (80, Format::D81, D81_SECTORS),
            (77, Format::D80, D80_SECTORS),
            (154, Format::D82, 2 * D80_SECTORS),
        ];
        for (tracks, format, n) in layouts {
            if bytes.len() == n * SECTOR_SIZE {
                return Ok(Self {
                    tracks,
                    format,
                    data: bytes.to_vec(),
                    errors: None,
                });
            }
            if bytes.len() == n * (SECTOR_SIZE + 1) {
                let split = n * SECTOR_SIZE;
                return Ok(Self {
                    tracks,
                    format,
                    data: bytes[..split].to_vec(),
                    errors: Some(bytes[split..].to_vec()),
                });
            }
        }
        Err(MediaError::Format(format!(
            "D64/D71/D81/D80/D82: unexpected size {} bytes",
            bytes.len()
        )))
    }

    /// An empty formatted 35-track disk, as the 1541's NEW command leaves
    /// it: BAM with every block free except track 18 sectors 0 and 1.
    pub fn blank(name: &[u8], id: [u8; 2]) -> Self {
        let mut d = Self {
            tracks: 35,
            format: Format::D64,
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
            format: Format::D71,
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

    /// An empty 1581 disk (D81), as its NEW command leaves it: header,
    /// BAM and the first directory block on track 40.
    pub fn blank_d81(name: &[u8], id: [u8; 2]) -> Self {
        let mut d = Self {
            tracks: 80,
            format: Format::D81,
            data: vec![0; D81_SECTORS * SECTOR_SIZE],
            errors: None,
        };
        let n = name.len().min(16);
        if let Some(h) = d.sector_mut(40, 0) {
            h[0] = 40;
            h[1] = 3;
            h[2] = b'D';
            h[0x04..0x1d].fill(0xa0);
            h[0x04..0x04 + n].copy_from_slice(&name[..n]);
            h[0x16] = id[0];
            h[0x17] = id[1];
            h[0x19] = b'3';
            h[0x1a] = b'D';
        }
        for (sector, link) in [(1u8, [40u8, 2u8]), (2, [0, 0xff])] {
            if let Some(b) = d.sector_mut(40, sector) {
                b[0..2].copy_from_slice(&link);
                b[2] = b'D';
                b[3] = !b'D';
                b[4] = id[0];
                b[5] = id[1];
                b[6] = 0xc0;
                for i in 0..40usize {
                    let e = 0x10 + 6 * i;
                    b[e] = 40;
                    b[e + 1..e + 6].fill(0xff);
                }
            }
        }
        if let Some(dir) = d.sector_mut(40, 3) {
            dir[1] = 0xff;
        }
        for s in 0..4 {
            d.allocate(40, s);
        }
        d
    }

    /// An empty 8050 (D80) or, `double_sided`, 8250 (D82) disk, as the
    /// DOS's NEW command leaves it.
    pub fn blank_8050(name: &[u8], id: [u8; 2], double_sided: bool) -> Self {
        let (tracks, format) = if double_sided {
            (154, Format::D82)
        } else {
            (77, Format::D80)
        };
        let sectors = if double_sided { 2 } else { 1 } * D80_SECTORS;
        let mut d = Self {
            tracks,
            format,
            data: vec![0; sectors * SECTOR_SIZE],
            errors: None,
        };
        let n = name.len().min(16);
        if let Some(h) = d.sector_mut(39, 0) {
            h[0] = 38;
            h[1] = 0;
            h[2] = b'C';
            h[0x06..0x1d].fill(0xa0);
            h[0x06..0x06 + n].copy_from_slice(&name[..n]);
            h[0x18] = id[0];
            h[0x19] = id[1];
            h[0x1b] = b'2';
            h[0x1c] = b'C';
        }
        let bams: &[u8] = if double_sided { &[0, 3, 6, 9] } else { &[0, 3] };
        for (i, &sector) in bams.iter().enumerate() {
            let low = 1 + 50 * i as u8;
            let high = (low + 50).min(tracks + 1);
            let link = match bams.get(i + 1) {
                Some(&next) => [38, next],
                None => [39, 1],
            };
            let counts: Vec<u8> = (low..high).map(|t| d.sectors_in(t)).collect();
            if let Some(b) = d.sector_mut(38, sector) {
                b[0..2].copy_from_slice(&link);
                b[2] = b'C';
                b[4] = low;
                b[5] = high;
                for (k, &count) in counts.iter().enumerate() {
                    let e = 6 + 5 * k;
                    b[e] = count;
                    b[e + 1..e + 5].copy_from_slice(&((1u64 << count) - 1).to_le_bytes()[..4]);
                }
            }
        }
        if let Some(dir) = d.sector_mut(39, 1) {
            dir[1] = 0xff;
        }
        for &sector in bams {
            d.allocate(38, sector);
        }
        d.allocate(39, 0);
        d.allocate(39, 1);
        d
    }

    /// The image is a D71 (tracks 36–70 on side 1).
    pub fn double_sided(&self) -> bool {
        self.format == Format::D71
    }

    /// Sectors on a track (side 1 of a D71 repeats tracks 1–35, of a D82
    /// tracks 1–77).
    pub fn sectors_in(&self, track: u8) -> u8 {
        match self.format {
            Format::D81 => 40,
            Format::D80 => sectors_8050(track),
            Format::D82 if track > 77 => sectors_8050(track - 77),
            Format::D82 => sectors_8050(track),
            Format::D71 if track > 35 => sectors_per_track(track - 35),
            _ => sectors_per_track(track),
        }
    }

    /// The 8050 and 8250 layouts.
    fn is_8050(&self) -> bool {
        matches!(self.format, Format::D80 | Format::D82)
    }

    /// Speed zone of a track (see [`speed_zone`]).
    pub fn zone_of(&self, track: u8) -> u8 {
        if self.double_sided() && track > 35 {
            speed_zone(track - 35)
        } else {
            speed_zone(track)
        }
    }

    /// The directory track: 18, 40 on a D81, 39 on a D80/D82.
    pub fn dir_track(&self) -> u8 {
        match self.format {
            Format::D81 => 40,
            Format::D80 | Format::D82 => 39,
            _ => 18,
        }
    }

    /// The first directory block.
    fn dir_start(&self) -> (u8, u8) {
        match self.format {
            Format::D81 => (40, 3),
            Format::D80 | Format::D82 => (39, 1),
            _ => (18, 1),
        }
    }

    /// The header block and the offsets of name, ID and DOS type in it.
    fn header(&self) -> (u8, u8, usize, usize, usize) {
        match self.format {
            Format::D81 => (40, 0, 0x04, 0x16, 0x19),
            Format::D80 | Format::D82 => (39, 0, 0x06, 0x18, 0x1b),
            _ => (18, 0, 0x90, 0xa2, 0xa5),
        }
    }

    fn total(&self, tracks: u8) -> usize {
        (1..=tracks).map(|t| usize::from(self.sectors_in(t))).sum()
    }

    /// Where the BAM keeps a track: the free count and the bitmap.
    fn bam_slot(&self, track: u8) -> Option<(BamByte, BamByte)> {
        if self.is_8050() {
            if track == 0 || track > self.tracks {
                return None;
            }
            let i = (track - 1) / 50;
            let e = 6 + 5 * usize::from((track - 1) % 50);
            return Some(((38, 3 * i, e), (38, 3 * i, e + 1)));
        }
        if self.format == Format::D81 {
            let (sector, i) = match track {
                1..=40 => (1, track - 1),
                41..=80 => (2, track - 41),
                _ => return None,
            };
            let e = 0x10 + 6 * usize::from(i);
            return Some(((40, sector, e), (40, sector, e + 1)));
        }
        match track {
            1..=35 => {
                let e = 4 * usize::from(track);
                Some(((18, 0, e), (18, 0, e + 1)))
            }
            36..=70 if self.double_sided() => {
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

    /// The disk ID bytes from the header, as written into sector headers.
    pub fn id(&self) -> [u8; 2] {
        let (t, s, _, id, _) = self.header();
        self.sector(t, s).map_or([0, 0], |b| [b[id], b[id + 1]])
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
        let (mut t, mut s) = self.dir_start();
        let mut visited = 0;
        while t != 0 {
            visited += 1;
            if visited > 80 {
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
    /// first sector. On a D81 the file is written as by [`D64::write_file`].
    pub fn add_prg(&mut self, name: &[u8], prg: &[u8]) -> Result<(), MediaError> {
        if matches!(self.format, Format::D81 | Format::D80 | Format::D82) {
            return self.write_file(name, 2, prg);
        }
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

    /// The tracks the DOS keeps files on: all but the directory track and,
    /// on a D71, the second BAM (53).
    fn data_tracks(&self) -> impl Iterator<Item = u8> + '_ {
        let last = match self.format {
            Format::D64 => self.tracks.min(35),
            Format::D71 => 70,
            Format::D81 => 80,
            Format::D80 => 77,
            Format::D82 => 154,
        };
        let dir = self.dir_track();
        (1..=last).filter(move |&t| t != dir && !(self.double_sided() && t == 53))
    }

    /// Free blocks as the directory reports them (the directory track, and
    /// track 53 of a D71, not counted).
    pub fn blocks_free(&self) -> u16 {
        self.data_tracks()
            .filter_map(|t| {
                let ((ct, cs, co), _) = self.bam_slot(t)?;
                self.sector(ct, cs).map(|b| u16::from(b[co]))
            })
            .sum()
    }

    /// The next block for a file like the DOS picks it: on the same track
    /// 10 sectors on (interleave; 1 on a D81), else on the next track away
    /// from the directory; the first block on the free track nearest the
    /// directory track (then, on a D71, nearest track 53).
    fn next_free(&self, prev: Option<(u8, u8)>) -> Option<(u8, u8)> {
        let in_track = |t: u8, from: u8| {
            let n = self.sectors_in(t);
            (0..n)
                .map(|i| (from + i) % n)
                .find(|&s| self.is_free(t, s))
                .map(|s| (t, s))
        };
        let usable = |t: u8| self.data_tracks().any(|d| d == t);
        if matches!(self.format, Format::D81 | Format::D80 | Format::D82) {
            // Interleave 1, away from the directory track.
            let dir = self.dir_track();
            let last = self.tracks;
            if let Some((t, s)) = prev {
                if let Some(b) = in_track(t, s + 1) {
                    return Some(b);
                }
                let away: Vec<u8> = if t < dir {
                    (1..t).rev().collect()
                } else {
                    (t + 1..=last).collect()
                };
                if let Some(b) = away
                    .into_iter()
                    .filter(|&t| usable(t))
                    .find_map(|t| in_track(t, 0))
                {
                    return Some(b);
                }
            }
            return (1..last)
                .flat_map(|d| [dir.checked_sub(d), dir.checked_add(d)])
                .flatten()
                .filter(|&t| usable(t))
                .find_map(|t| in_track(t, 0));
        }
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
    /// directory grows by blocks on its track when its sectors are full.
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
    /// block on the directory track (interleave 3, 1 on a D81) when all are
    /// used.
    fn directory_slot(&mut self) -> Result<(u8, u8, usize), MediaError> {
        let (mut t, mut s) = self.dir_start();
        let dir = self.dir_track();
        let step = if matches!(self.format, Format::D64 | Format::D71) {
            3
        } else {
            1
        };
        for _ in 0..80 {
            let b = self
                .sector(t, s)
                .ok_or_else(|| MediaError::Format(format!("D64: bad directory link {t}/{s}")))?;
            if let Some(i) = b.chunks(32).position(|e| e[2] == 0) {
                return Ok((t, s, i * 32));
            }
            if b[0] == 0 {
                let n = self.sectors_in(dir);
                let new = (0..n)
                    .map(|i| (s + step + i) % n)
                    .find(|&x| self.is_free(dir, x))
                    .ok_or_else(|| MediaError::Format("72, DISK FULL".into()))?;
                self.allocate(dir, new);
                let last = self.sector_mut(t, s).unwrap_or_default();
                last[0] = dir;
                last[1] = new;
                let block = self.sector_mut(dir, new).unwrap_or_default();
                block.fill(0);
                block[1] = 0xff;
                return Ok((dir, new, 0));
            }
            (t, s) = (b[0], b[1]);
        }
        Err(MediaError::Format("D64: directory loops".into()))
    }

    /// Delete the files matching `pattern` (as `S:pattern` does); returns
    /// how many.
    pub fn scratch(&mut self, pattern: &[u8]) -> Result<usize, MediaError> {
        let mut count = 0;
        let (mut t, mut s) = self.dir_start();
        let mut visited = 0;
        while t != 0 && visited < 80 {
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
                while ft != 0 && chain < 3200 {
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

    /// The directory as the drive sends it for `LOAD"$"`: a BASIC program at
    /// `$0401` with the header (reversed name, ID, DOS type "2A" or "3D"),
    /// one line per file (blocks, quoted name, type) and BLOCKS FREE.
    pub fn listing(&self) -> Vec<u8> {
        let mut out = vec![0x01, 0x04];
        let mut line = |number: u16, text: &[u8]| {
            out.extend_from_slice(&[0x01, 0x01]);
            out.extend_from_slice(&number.to_le_bytes());
            out.extend_from_slice(text);
            out.push(0);
        };
        let (ht, hs, name, id, dos) = self.header();
        let h = self
            .sector(ht, hs)
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| vec![0; SECTOR_SIZE]);
        let mut header = vec![0x12, b'"'];
        header.extend(
            h[name..name + 16]
                .iter()
                .map(|&c| if c == 0xa0 { b' ' } else { c }),
        );
        header.extend_from_slice(b"\" ");
        header.extend_from_slice(&[h[id], h[id + 1], b' ', h[dos], h[dos + 1]]);
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
        assert!(d.double_sided());
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

    #[test]
    fn d81_layout_and_files() {
        let mut d = D64::blank_d81(b"BIG DISK", *b"81");
        assert_eq!(d.format, Format::D81);
        assert_eq!(d.to_bytes().len(), 819_200);
        assert_eq!(d.blocks_free(), 3160);
        assert_eq!(d.id(), *b"81");
        assert_eq!(&d.sector(40, 0).unwrap()[0x19..0x1b], b"3D");
        assert_eq!(d.sector(40, 1).unwrap()[0x10 + 6 * 39], 36, "track 40");
        // A file starts next to the directory, with interleave 1.
        let data: Vec<u8> = (0..30_000u32).map(|i| (i * 3) as u8).collect();
        d.write_file(b"LARGE", 2, &data).unwrap();
        let (e, back) = d.read_file(b"LARGE").unwrap();
        assert_eq!(back, data);
        assert_eq!((e.track, e.sector, e.blocks), (39, 0, 119));
        assert_eq!(d.sector(39, 0).unwrap()[..2], [39, 1]);
        assert_eq!(d.blocks_free(), 3160 - 119);
        // Tracks 41+ (second BAM sector) hold the rest of a full side.
        let more = vec![0x77u8; 39 * 40 * 254];
        d.write_file(b"MORE", 2, &more).unwrap();
        let (e, _) = d.read_file(b"MORE").unwrap();
        assert!(e.track <= 41);
        assert!(d.sector(40, 2).unwrap()[0x10] < 40, "track 41 used");
        for i in 0..40u8 {
            d.write_file(&[b'F', b'0' + i % 10, b'A' + i / 10], 1, b"x")
                .unwrap();
        }
        assert_eq!(d.directory().unwrap().len(), 42);
        assert_eq!(d.sector(40, 3).unwrap()[..2], [40, 4], "directory grew");
        assert_eq!(d.scratch(b"F*").unwrap(), 40);
        let l = d.listing();
        assert!(l.windows(5).any(|w| w == b"81 3D"));
        assert_eq!(D64::parse(&d.to_bytes()).unwrap(), d);
        d.add_prg(b"PRG", &[0x01, 0x08, 0]).unwrap();
        assert_eq!(d.load(b"PRG").unwrap().1.to_bytes(), [0x01, 0x08, 0]);
    }

    #[test]
    fn d80_and_d82_layouts() {
        let mut d = D64::blank_8050(b"PET DISK", *b"80", false);
        assert_eq!(d.format, Format::D80);
        assert_eq!(d.to_bytes().len(), 533_248);
        assert_eq!(d.blocks_free(), 2052);
        assert_eq!(d.id(), *b"80");
        assert_eq!(
            (d.sectors_in(39), d.sectors_in(53), d.sectors_in(77)),
            (29, 27, 23)
        );
        assert_eq!(d.sector(38, 0).unwrap()[..6], [38, 3, b'C', 0, 1, 51]);
        let data: Vec<u8> = (0..40_000u32).map(|i| (i * 7) as u8).collect();
        d.write_file(b"BIG", 2, &data).unwrap();
        let (e, back) = d.read_file(b"BIG").unwrap();
        assert_eq!(back, data);
        assert_eq!(e.track, 38, "next to the directory");
        assert_eq!(d.blocks_free(), 2052 - 158);
        assert!(d.listing().windows(5).any(|w| w == b"80 2C"));
        assert_eq!(D64::parse(&d.to_bytes()).unwrap(), d);
        // Tracks beyond 50 are in the second BAM sector.
        let more = vec![1u8; 1800 * 254];
        d.write_file(b"MORE", 2, &more).unwrap();
        assert!(d.sector(38, 3).unwrap()[6] < 29, "track 51 used");
        assert_eq!(d.read_file(b"MORE").unwrap().1, more);

        let d82 = D64::blank_8050(b"BIGGER", *b"82", true);
        assert_eq!(d82.to_bytes().len(), 1_066_496);
        assert_eq!(d82.blocks_free(), 4133);
        assert_eq!(d82.sectors_in(78), 29);
        assert_eq!(d82.sector(38, 9).unwrap()[4..6], [151, 155]);
    }
}
