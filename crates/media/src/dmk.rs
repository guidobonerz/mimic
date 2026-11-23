//! TRS-80 disk images: DMK (raw tracks), JV1 and JV3 (sectors), all kept
//! as DMK-style tracks in memory so the floppy controller works on the
//! bytes a real track carries.
//!
//! **DMK** (David Keil's format, as documented by its author and in the
//! TRS-80 emulation community; see `docs/sources.md`): a 16-byte header —
//! byte 0 `$FF` = write protected, byte 1 the number of tracks, bytes 2–3
//! the track length (little endian, including the IDAM table), byte 4 the
//! flags (bit 4 single-sided, bit 6 single density written once per byte,
//! bit 7 ignore density) — then the tracks, side 0 and side 1 of each
//! cylinder in turn for double-sided images. A track starts with 64
//! little-endian pointers to its ID address marks (`$FE` bytes; offset from
//! the track start, bit 15 set = double density MFM, bit 14 unused), then
//! the track bytes. Single-density (FM) bytes are stored twice unless the
//! header says otherwise.
//!
//! **JV1**: 256-byte single-density sectors 0–9 of each track in order,
//! single-sided; the directory track (17) uses the `$FA` data mark.
//!
//! **JV3**: 2901 three-byte sector descriptors (track, sector, flags: bits
//! 1–0 size — 0 = 256, 1 = 128, 2 = 1024, 3 = 512 bytes —, bit 2 "non-IBM",
//! bit 3 CRC error, bit 4 side 1, bits 6–5 the data mark, bit 7 double
//! density; `$FF` track = unused), one write-protect byte (`0` =
//! protected), then the sector data in descriptor order.
//!
//! **IMD** (Dave Dunfield's ImageDisk, as its documentation describes the
//! file): the text `IMD v.vv: date time`, a comment, the byte `$1A`, then
//! one record a track: the mode (0–2 FM at 500, 300 and 250 kbit/s, 3–5
//! MFM at the same rates), the cylinder, the head (bit 0; bit 7 a
//! cylinder map follows, bit 6 a head map), the number of sectors, the
//! size code (128 << n), the sector numbers in the order they lie on the
//! track, the optional maps (the cylinder and head each ID carries), and
//! for each sector a type byte and its data: 0 no data, 1 the bytes, 2
//! one byte that fills the sector; 3 and 4 the same with the deleted data
//! mark, 5 to 8 the same read with a data error (taken as good here).
//! Tracks at 500 kbit/s are an 8-inch disk's ([`Image::eight_inch`]):
//! 10416 bytes a turn at 360 rpm in MFM, where a 5.25-inch track has 6250
//! at 300 rpm.
//!
//! JV and IMD images are built into tracks on load (gaps, marks and CRCs as the
//! TRSDOS formatter writes them) and written back sector by sector.
//!
//! **TI-99/4A DSK** (V9T9/PC99 sector dumps, [`Image::parse_ti_dsk`]):
//! 256-byte single-density sectors 0–8 of each track, 40 tracks; a
//! double-sided image continues with side 1 from track 39 down to track 0
//! (the TI's logical sector numbering). Its own call, as a single-sided
//! TI image has the size of a 36-track JV1. On the track the sectors lie
//! interleaved as the TI disk controller formats them (0 7 5 3 1 8 6 4 2).
//! [`blank_ti_dsk`] makes an initialised blank disk (the volume information
//! block and an empty file directory; Thierry Nouspikel, "The TI-99/4A
//! Tech Pages", disk structure).

use mimic_core::media::MediaError;

use crate::mfm::crc16;

pub const DMK_HEADER: usize = 16;
pub const IDAM_TABLE: usize = 128;
/// Bytes on a double-density track (300 rpm, 250 kbit/s).
pub const DD_TRACK_BYTES: usize = 6250;
/// Bytes on a single-density track (125 kbit/s).
pub const SD_TRACK_BYTES: usize = 3125;
/// The DMK track length TRS-80 emulators use (6400 + the IDAM table),
/// enough for a double-density track or a doubled single-density one.
pub const DEFAULT_TRACK_LEN: usize = 0x1900;

/// Data mark bits of a JV3 descriptor and the mark bytes they stand for.
const JV3_DAM_FB: u8 = 0x00;
const JV3_DAM_FA: u8 = 0x20;
const JV3_DAM_F9: u8 = 0x40;
const JV3_SECTORS: usize = 2901;

/// One track as the controller sees it: the raw bytes (single density
/// stored once per byte here) and where the ID address marks start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub data: Vec<u8>,
    /// Offsets of the `$FE` bytes into `data`, with bit 15 = MFM.
    pub idam: Vec<u16>,
}

impl Track {
    /// A blank track of `len` bytes.
    pub fn blank(len: usize, mfm: bool) -> Self {
        Self {
            data: vec![if mfm { 0x4e } else { 0xff }; len],
            idam: Vec::new(),
        }
    }

    pub fn is_mfm(&self) -> bool {
        self.idam.first().is_some_and(|p| p & 0x8000 != 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Dmk,
    Jv1,
    Jv3,
    TiDsk,
    MsxDsk,
    Imd,
}

/// Bytes on an 8-inch track (360 rpm, 500 kbit/s MFM; half in FM).
pub const DD8_TRACK_BYTES: usize = 10416;
/// The highest cylinder an IMD image may name.
const IMD_CYLINDERS: usize = 85;

/// TI disks: sectors per track, tracks per side, their order on a track.
const TI_SECTORS: usize = 9;
const TI_TRACKS: usize = 40;
const TI_INTERLEAVE: [u8; 9] = [0, 7, 5, 3, 1, 8, 6, 4, 2];
const TI_SIDE: usize = TI_SECTORS * TI_TRACKS * 256;

/// Where cylinder `t`, side `side`, sector `s` lies in a TI image.
fn ti_offset(t: usize, side: usize, s: usize) -> usize {
    let logical_track = if side == 0 { t } else { 2 * TI_TRACKS - 1 - t };
    (logical_track * TI_SECTORS + s) * 256
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub format: Format,
    pub protected: bool,
    pub sides: u8,
    /// Tracks in DMK order: cylinder-major, side-minor.
    pub tracks: Vec<Track>,
    /// DMK: single-density bytes stored once (header bit 6).
    sd_single: bool,
    /// DMK: the header's track length.
    track_len: usize,
    /// JV3: the descriptors as read (for writing back).
    jv3: Vec<(u8, u8, u8)>,
    /// The tracks are an 8-inch disk's: twice the data rate, 360 rpm.
    pub eight_inch: bool,
}

fn err(msg: impl Into<String>) -> MediaError {
    MediaError::Format(msg.into())
}

impl Image {
    pub fn cylinders(&self) -> usize {
        self.tracks.len() / usize::from(self.sides.max(1))
    }

    pub fn track(&self, cylinder: u8, side: u8) -> Option<&Track> {
        self.tracks
            .get(usize::from(cylinder) * usize::from(self.sides) + usize::from(side))
    }

    pub fn track_mut(&mut self, cylinder: u8, side: u8) -> Option<&mut Track> {
        let i = usize::from(cylinder) * usize::from(self.sides) + usize::from(side);
        self.tracks.get_mut(i)
    }

    /// Recognise and parse an image: DMK by its header, JV3 by its
    /// descriptor table, JV1 by a size that is a whole number of tracks.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if let Some(img) = Self::parse_imd(bytes)? {
            return Ok(img);
        }
        if let Some(img) = Self::parse_dmk(bytes)? {
            return Ok(img);
        }
        if let Some(img) = Self::parse_jv3(bytes)? {
            return Ok(img);
        }
        Self::parse_jv1(bytes)
    }

    /// A TI-99/4A sector dump: 92 160 bytes single-sided, 184 320
    /// double-sided (see the module doc).
    pub fn parse_ti_dsk(bytes: &[u8]) -> Result<Self, MediaError> {
        let sides = match bytes.len() {
            TI_SIDE => 1,
            n if n == 2 * TI_SIDE => 2,
            n => {
                return Err(err(format!(
                "not a TI-99/4A single-density disk image: {n} bytes ({TI_SIDE} or {} expected)",
                2 * TI_SIDE
            )))
            }
        };
        let mut tracks = Vec::new();
        for t in 0..TI_TRACKS {
            for side in 0..sides {
                let sectors: Vec<SectorSpec> = TI_INTERLEAVE
                    .iter()
                    .map(|&n| SectorSpec {
                        track: t as u8,
                        side: side as u8,
                        sector: n,
                        size: 256,
                        dam: 0xfb,
                        data: bytes[ti_offset(t, side, usize::from(n))..][..256].to_vec(),
                        mfm: false,
                    })
                    .collect();
                tracks.push(build_track(&sectors, false));
            }
        }
        Ok(Self {
            format: Format::TiDsk,
            protected: false,
            sides: sides as u8,
            tracks,
            sd_single: true,
            track_len: DEFAULT_TRACK_LEN,
            jv3: Vec::new(),
            eight_inch: false,
        })
    }

    /// An MSX (MS-DOS style) sector dump: 512-byte sectors in double
    /// density, numbered from 1, cylinder by cylinder and side by side.
    /// The geometry comes from the boot sector's parameter block (total
    /// sectors at $13, sectors per track at $18, sides at $1A) when that
    /// fits the image's size, else from the size: 720 KB and 360 KB are 80
    /// cylinders of 9 sectors with two sides and one, 640 KB and 320 KB
    /// the same with 8 sectors, 180 KB and 160 KB 40 cylinders, one side.
    pub fn parse_msx_dsk(bytes: &[u8]) -> Result<Self, MediaError> {
        let (cylinders, sides, spt) = msx_geometry(bytes).ok_or_else(|| {
            err(format!(
                "not an MSX disk image: {} bytes (720, 360, 640, 320, 180 or 160 KB expected)",
                bytes.len()
            ))
        })?;
        let mut tracks = Vec::new();
        for t in 0..cylinders {
            for side in 0..sides {
                let sectors: Vec<SectorSpec> = (0..spt)
                    .map(|n| SectorSpec {
                        track: t as u8,
                        side: side as u8,
                        sector: n as u8 + 1,
                        size: 512,
                        dam: 0xfb,
                        data: bytes[((t * sides + side) * spt + n) * 512..][..512].to_vec(),
                        mfm: true,
                    })
                    .collect();
                tracks.push(build_track(&sectors, true));
            }
        }
        Ok(Self {
            format: Format::MsxDsk,
            protected: false,
            sides: sides as u8,
            tracks,
            sd_single: true,
            track_len: DEFAULT_TRACK_LEN,
            jv3: Vec::new(),
            eight_inch: false,
        })
    }

    fn parse_dmk(bytes: &[u8]) -> Result<Option<Self>, MediaError> {
        if bytes.len() < DMK_HEADER {
            return Ok(None);
        }
        let tracks = usize::from(bytes[1]);
        let track_len = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
        let flags = bytes[4];
        let sides = if flags & 0x10 != 0 { 1 } else { 2 };
        if tracks == 0 || !(IDAM_TABLE + 128..=0x4000).contains(&track_len) {
            return Ok(None);
        }
        if bytes.len() != DMK_HEADER + tracks * usize::from(sides) * track_len {
            return Ok(None);
        }
        let sd_single = flags & 0x40 != 0;
        let mut out = Vec::new();
        for t in 0..tracks * usize::from(sides) {
            let start = DMK_HEADER + t * track_len;
            let raw = &bytes[start..start + track_len];
            let idam: Vec<u16> = (0..64)
                .map(|i| u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]))
                .filter(|&p| p & 0x3fff >= IDAM_TABLE as u16)
                .collect();
            let mfm = idam.first().is_some_and(|p| p & 0x8000 != 0);
            let body = &raw[IDAM_TABLE..];
            // Single density stored twice: keep every other byte.
            let (data, scale) = if !mfm && !sd_single {
                (body.iter().step_by(2).copied().collect::<Vec<_>>(), 2)
            } else {
                (body.to_vec(), 1)
            };
            let idam = idam
                .iter()
                .map(|&p| {
                    let off = (usize::from(p & 0x3fff) - IDAM_TABLE) / scale;
                    off as u16 | (p & 0x8000)
                })
                .collect();
            out.push(Track { data, idam });
        }
        Ok(Some(Self {
            format: Format::Dmk,
            protected: bytes[0] == 0xff,
            sides,
            tracks: out,
            sd_single,
            track_len,
            jv3: Vec::new(),
            eight_inch: false,
        }))
    }

    fn parse_jv1(bytes: &[u8]) -> Result<Self, MediaError> {
        const TRACK: usize = 10 * 256;
        if bytes.is_empty() || !bytes.len().is_multiple_of(TRACK) || bytes.len() / TRACK > 96 {
            return Err(err(format!(
                "not a TRS-80 disk image: {} bytes (DMK header, JV3 table or JV1 tracks of 2560 bytes expected)",
                bytes.len()
            )));
        }
        let tracks = bytes.len() / TRACK;
        let mut out = Vec::new();
        for t in 0..tracks {
            let sectors: Vec<SectorSpec> = (0..10u8)
                .map(|s| SectorSpec {
                    track: t as u8,
                    side: 0,
                    sector: s,
                    size: 256,
                    dam: if t == 17 { 0xfa } else { 0xfb },
                    data: bytes[t * TRACK + usize::from(s) * 256..][..256].to_vec(),
                    mfm: false,
                })
                .collect();
            out.push(build_track(&sectors, false));
        }
        Ok(Self {
            format: Format::Jv1,
            protected: false,
            sides: 1,
            tracks: out,
            sd_single: true,
            track_len: DEFAULT_TRACK_LEN,
            jv3: Vec::new(),
            eight_inch: false,
        })
    }

    fn parse_jv3(bytes: &[u8]) -> Result<Option<Self>, MediaError> {
        let header = JV3_SECTORS * 3 + 1;
        if bytes.len() < header {
            return Ok(None);
        }
        let mut descs = Vec::new();
        let mut total = 0usize;
        let mut sides = 1u8;
        for i in 0..JV3_SECTORS {
            let (t, s, f) = (bytes[3 * i], bytes[3 * i + 1], bytes[3 * i + 2]);
            if t == 0xff {
                continue;
            }
            descs.push((t, s, f));
            total += jv3_size(f);
            if f & 0x10 != 0 {
                sides = 2;
            }
        }
        if descs.is_empty() || bytes.len() < header + total {
            return Ok(None);
        }
        // Group by cylinder and side, keeping the descriptor order.
        let cylinders = descs.iter().map(|d| usize::from(d.0)).max().unwrap_or(0) + 1;
        let mut specs: Vec<Vec<SectorSpec>> = vec![Vec::new(); cylinders * usize::from(sides)];
        let mut pos = header;
        for &(t, s, f) in &descs {
            let size = jv3_size(f);
            let data = bytes[pos..pos + size].to_vec();
            pos += size;
            let side = u8::from(f & 0x10 != 0);
            let mfm = f & 0x80 != 0;
            let dam = match (mfm, f & 0x60) {
                (false, JV3_DAM_FB) | (true, JV3_DAM_FB) => 0xfb,
                (false, JV3_DAM_FA) => 0xfa,
                (false, JV3_DAM_F9) => 0xf9,
                // $60 single density, $20/$40/$60 double density: $F8.
                _ => 0xf8,
            };
            specs[usize::from(t) * usize::from(sides) + usize::from(side)].push(SectorSpec {
                track: t,
                side,
                sector: s,
                size,
                dam,
                data,
                mfm,
            });
        }
        let tracks = specs
            .iter()
            .map(|s| {
                let mfm = s.first().is_some_and(|x| x.mfm);
                if s.is_empty() {
                    Track::blank(SD_TRACK_BYTES, false)
                } else {
                    build_track(s, mfm)
                }
            })
            .collect();
        Ok(Some(Self {
            format: Format::Jv3,
            protected: bytes[header - 1] == 0,
            sides,
            tracks,
            sd_single: true,
            track_len: DEFAULT_TRACK_LEN,
            jv3: descs,
            eight_inch: false,
        }))
    }

    /// An ImageDisk file (see the module doc), told by its first bytes.
    fn parse_imd(bytes: &[u8]) -> Result<Option<Self>, MediaError> {
        if !bytes.starts_with(b"IMD ") {
            return Ok(None);
        }
        let short = || err("IMD image ends inside a track");
        let mut i = bytes
            .iter()
            .position(|&b| b == 0x1a)
            .ok_or_else(|| err("IMD image without the end of its comment"))?
            + 1;
        // (cylinder, head, MFM, sectors)
        let mut found: Vec<(usize, usize, bool, Vec<SectorSpec>)> = Vec::new();
        let mut eight_inch = false;
        while i < bytes.len() {
            let head = bytes.get(i..i + 5).ok_or_else(short)?;
            let (mode, cyl, flags, count, code) = (head[0], head[1], head[2], head[3], head[4]);
            if mode > 5 || flags & 0x3e != 0 || code > 3 || usize::from(cyl) >= IMD_CYLINDERS {
                return Err(err(format!(
                    "IMD track header: mode {mode}, cylinder {cyl}, head {flags:#04x}, size code {code}"
                )));
            }
            i += 5;
            let n = usize::from(count);
            let size = 128usize << code;
            let mut map = |on: bool| -> Result<Option<&[u8]>, MediaError> {
                if !on {
                    return Ok(None);
                }
                let m = bytes.get(i..i + n).ok_or_else(short)?;
                i += n;
                Ok(Some(m))
            };
            let numbers = map(true)?.unwrap_or(&[]);
            let cylinders = map(flags & 0x80 != 0)?;
            let heads = map(flags & 0x40 != 0)?;
            let mfm = mode >= 3;
            let mut sectors = Vec::with_capacity(n);
            for s in 0..n {
                let kind = *bytes.get(i).ok_or_else(short)?;
                i += 1;
                let data = match kind {
                    0 => vec![0xe5; size],
                    1 | 3 | 5 | 7 => {
                        let d = bytes.get(i..i + size).ok_or_else(short)?;
                        i += size;
                        d.to_vec()
                    }
                    2 | 4 | 6 | 8 => {
                        let b = *bytes.get(i).ok_or_else(short)?;
                        i += 1;
                        vec![b; size]
                    }
                    _ => return Err(err(format!("IMD sector type {kind}"))),
                };
                sectors.push(SectorSpec {
                    track: cylinders.map_or(cyl, |m| m[s]),
                    side: heads.map_or(flags & 1, |m| m[s]),
                    sector: numbers[s],
                    size,
                    dam: if matches!(kind, 3 | 4 | 7 | 8) {
                        0xf8
                    } else {
                        0xfb
                    },
                    data,
                    mfm,
                });
            }
            if n == 0 {
                continue;
            }
            eight_inch |= mode % 3 == 0;
            found.push((usize::from(cyl), usize::from(flags & 1), mfm, sectors));
        }
        if found.is_empty() {
            return Err(err("IMD image without tracks"));
        }
        let sides = if found.iter().any(|t| t.1 == 1) { 2 } else { 1 };
        let cylinders = found.iter().map(|t| t.0 + 1).max().unwrap_or(0);
        let full = if eight_inch {
            DD8_TRACK_BYTES
        } else {
            DD_TRACK_BYTES
        };
        let mut tracks = vec![Track::blank(full, true); cylinders * sides];
        for (cyl, head, mfm, sectors) in &found {
            let len = if *mfm { full } else { full / 2 };
            tracks[cyl * sides + head] = build_track_len(sectors, *mfm, len);
        }
        Ok(Some(Self {
            format: Format::Imd,
            protected: false,
            sides: sides as u8,
            tracks,
            sd_single: true,
            track_len: DEFAULT_TRACK_LEN,
            jv3: Vec::new(),
            eight_inch,
        }))
    }

    /// The tracks as an ImageDisk file: the sectors each track holds now,
    /// under the comment `original` had. A track's sectors of another size
    /// than its first are left out (the format has one size a track).
    fn to_imd(&self, original: &[u8]) -> Vec<u8> {
        let mut out = match original.iter().position(|&b| b == 0x1a) {
            Some(end) if original.starts_with(b"IMD ") => original[..=end].to_vec(),
            _ => b"IMD 1.18: 01/01/1980 00:00:00\r\n\x1a".to_vec(),
        };
        let sides = usize::from(self.sides.max(1));
        for (t, track) in self.tracks.iter().enumerate() {
            let (cyl, head) = ((t / sides) as u8, (t % sides) as u8);
            let mut sectors = decode_sectors(track);
            let Some(size) = sectors.first().map(|s| s.size) else {
                continue;
            };
            sectors.retain(|s| s.size == size);
            sectors.truncate(255);
            let mode = match (track.is_mfm(), self.eight_inch) {
                (false, true) => 0,
                (false, false) => 2,
                (true, true) => 3,
                (true, false) => 5,
            };
            let cylinders = sectors.iter().any(|s| s.track != cyl);
            let heads = sectors.iter().any(|s| s.side != head);
            out.extend([
                mode,
                cyl,
                head | u8::from(cylinders) << 7 | u8::from(heads) << 6,
                sectors.len() as u8,
                size_code(size),
            ]);
            out.extend(sectors.iter().map(|s| s.sector));
            if cylinders {
                out.extend(sectors.iter().map(|s| s.track));
            }
            if heads {
                out.extend(sectors.iter().map(|s| s.side));
            }
            for s in &sectors {
                let deleted = if s.dam == 0xf8 { 2 } else { 0 };
                if s.data.iter().all(|&b| b == s.data[0]) {
                    out.extend([2 + deleted, s.data[0]]);
                } else {
                    out.push(1 + deleted);
                    out.extend(&s.data);
                }
            }
        }
        out
    }

    /// The image as a file in its own format. JV images get their sectors
    /// back from the tracks; sectors a track no longer holds keep their
    /// old bytes (`original`).
    pub fn to_bytes(&self, original: &[u8]) -> Vec<u8> {
        match self.format {
            Format::Dmk => self.to_dmk(),
            Format::Imd => self.to_imd(original),
            Format::Jv1 => {
                let mut out = original.to_vec();
                for (t, track) in self.tracks.iter().enumerate() {
                    for s in decode_sectors(track) {
                        if s.size == 256 && usize::from(s.sector) < 10 {
                            let off = t * 2560 + usize::from(s.sector) * 256;
                            if let Some(dst) = out.get_mut(off..off + 256) {
                                dst.copy_from_slice(&s.data);
                            }
                        }
                    }
                }
                out
            }
            Format::TiDsk => {
                let mut out = original.to_vec();
                let sides = usize::from(self.sides.max(1));
                for (i, track) in self.tracks.iter().enumerate() {
                    let (t, side) = (i / sides, i % sides);
                    for sec in decode_sectors(track) {
                        if sec.size == 256 && usize::from(sec.sector) < TI_SECTORS {
                            let off = ti_offset(t, side, usize::from(sec.sector));
                            if let Some(dst) = out.get_mut(off..off + 256) {
                                dst.copy_from_slice(&sec.data);
                            }
                        }
                    }
                }
                out
            }
            Format::MsxDsk => {
                let mut out = original.to_vec();
                let spt = out.len() / 512 / self.tracks.len().max(1);
                for (i, track) in self.tracks.iter().enumerate() {
                    for sec in decode_sectors(track) {
                        let n = usize::from(sec.sector);
                        if sec.size == 512 && (1..=spt).contains(&n) {
                            let off = (i * spt + n - 1) * 512;
                            if let Some(dst) = out.get_mut(off..off + 512) {
                                dst.copy_from_slice(&sec.data);
                            }
                        }
                    }
                }
                out
            }
            Format::Jv3 => {
                let mut out = original.to_vec();
                let header = JV3_SECTORS * 3 + 1;
                let mut pos = header;
                for &(t, s, f) in &self.jv3 {
                    let size = jv3_size(f);
                    let side = u8::from(f & 0x10 != 0);
                    if let Some(track) = self.track(t, side) {
                        if let Some(sec) = decode_sectors(track)
                            .into_iter()
                            .find(|x| x.sector == s && x.size == size)
                        {
                            out[pos..pos + size].copy_from_slice(&sec.data);
                        }
                    }
                    pos += size;
                }
                out
            }
        }
    }

    fn to_dmk(&self) -> Vec<u8> {
        let mut out = vec![0u8; DMK_HEADER];
        out[0] = if self.protected { 0xff } else { 0 };
        out[1] = self.cylinders() as u8;
        out[2..4].copy_from_slice(&(self.track_len as u16).to_le_bytes());
        out[4] = if self.sides == 1 { 0x10 } else { 0 } | if self.sd_single { 0x40 } else { 0 };
        for track in &self.tracks {
            let mfm = track.is_mfm();
            let scale = if !mfm && !self.sd_single { 2 } else { 1 };
            let mut raw = vec![0u8; self.track_len];
            for (i, &p) in track.idam.iter().take(64).enumerate() {
                let off = (usize::from(p & 0x3fff) * scale + IDAM_TABLE) as u16 | (p & 0x8000);
                raw[2 * i..2 * i + 2].copy_from_slice(&off.to_le_bytes());
            }
            for (i, &b) in track.data.iter().enumerate() {
                for k in 0..scale {
                    if let Some(d) = raw.get_mut(IDAM_TABLE + i * scale + k) {
                        *d = b;
                    }
                }
            }
            out.extend(raw);
        }
        out
    }
}

/// Cylinders, sides and sectors per track of an MSX sector dump.
fn msx_geometry(bytes: &[u8]) -> Option<(usize, usize, usize)> {
    let len = bytes.len();
    if len >= 512 {
        let word = |at: usize| usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]));
        let (total, spt, sides) = (word(0x13), word(0x18), word(0x1a));
        if total * 512 == len && matches!(spt, 8 | 9) && matches!(sides, 1 | 2) {
            let cylinders = total / (spt * sides);
            if matches!(cylinders, 40 | 80) && cylinders * spt * sides == total {
                return Some((cylinders, sides, spt));
            }
        }
    }
    Some(match len {
        737_280 => (80, 2, 9),
        368_640 => (80, 1, 9),
        655_360 => (80, 2, 8),
        327_680 => (80, 1, 8),
        184_320 => (40, 1, 9),
        163_840 => (40, 1, 8),
        _ => return None,
    })
}

fn jv3_size(flags: u8) -> usize {
    match flags & 3 {
        0 => 256,
        1 => 128,
        2 => 1024,
        _ => 512,
    }
}

/// A sector to lay out on a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectorSpec {
    pub track: u8,
    pub side: u8,
    pub sector: u8,
    pub size: usize,
    /// Data address mark: $FB, $FA, $F9 or $F8.
    pub dam: u8,
    pub data: Vec<u8>,
    pub mfm: bool,
}

/// Size code of a sector length (128 << n).
fn size_code(size: usize) -> u8 {
    match size {
        128 => 0,
        256 => 1,
        512 => 2,
        _ => 3,
    }
}

/// Lay out sectors on a track the way the TRS-80 DOSes format them
/// (IBM-style gaps; FM: 6 × $00 before the marks, MFM: 12 × $00 and three
/// $A1 sync bytes).
pub fn build_track(sectors: &[SectorSpec], mfm: bool) -> Track {
    build_track_len(
        sectors,
        mfm,
        if mfm { DD_TRACK_BYTES } else { SD_TRACK_BYTES },
    )
}

/// [`build_track`] for a track of `len` bytes (an 8-inch disk's are
/// longer).
pub fn build_track_len(sectors: &[SectorSpec], mfm: bool, len: usize) -> Track {
    let gap = if mfm { 0x4e } else { 0xff };
    let mut data = Vec::with_capacity(len);
    let mut idam = Vec::new();
    data.extend(std::iter::repeat_n(gap, if mfm { 32 } else { 16 }));
    for s in sectors {
        // ID field.
        data.extend(std::iter::repeat_n(0u8, if mfm { 12 } else { 6 }));
        let mut crc = 0xffffu16;
        if mfm {
            data.extend([0xa1; 3]);
            crc = (0..3).fold(crc, |c, _| crc16(c, 0xa1));
        }
        let idam_pos = data.len();
        for b in [0xfe, s.track, s.side, s.sector, size_code(s.size)] {
            data.push(b);
            crc = crc16(crc, b);
        }
        data.extend(crc.to_be_bytes());
        idam.push(idam_pos as u16 | if mfm { 0x8000 } else { 0 });
        // Gap 2, data field.
        data.extend(std::iter::repeat_n(gap, if mfm { 22 } else { 11 }));
        data.extend(std::iter::repeat_n(0u8, if mfm { 12 } else { 6 }));
        let mut crc = 0xffffu16;
        if mfm {
            data.extend([0xa1; 3]);
            crc = (0..3).fold(crc, |c, _| crc16(c, 0xa1));
        }
        data.push(s.dam);
        crc = crc16(crc, s.dam);
        for i in 0..s.size {
            let b = s.data.get(i).copied().unwrap_or(0);
            data.push(b);
            crc = crc16(crc, b);
        }
        data.extend(crc.to_be_bytes());
        data.extend(std::iter::repeat_n(gap, if mfm { 24 } else { 12 }));
    }
    if data.len() > len {
        // More than fits: the track is simply longer (the drive turns a
        // little slower in our model); keep everything.
    } else {
        data.resize(len, gap);
    }
    Track { data, idam }
}

/// A sector found on a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sector {
    pub track: u8,
    pub side: u8,
    pub sector: u8,
    pub size: usize,
    pub dam: u8,
    pub data: Vec<u8>,
    /// Offset of the data mark in the track.
    pub dam_pos: usize,
    pub crc_ok: bool,
}

/// The data mark position for the ID at `idam`: the first $F8–$FB within
/// the gap after the ID field (MFM: after $A1 sync bytes).
pub fn find_dam(track: &Track, idam: usize, mfm: bool) -> Option<usize> {
    let n = track.data.len();
    let from = idam + 7;
    let limit = if mfm { 43 } else { 30 };
    (from..from + limit).find(|&i| {
        let b = track.data[i % n];
        (0xf8..=0xfb).contains(&b) && (!mfm || track.data[(i + n - 1) % n] == 0xa1)
    })
}

/// Decode the sectors of a track from its ID address marks.
pub fn decode_sectors(track: &Track) -> Vec<Sector> {
    let n = track.data.len();
    if n == 0 {
        return Vec::new();
    }
    let at = |i: usize| track.data[i % n];
    let mut out = Vec::new();
    for &p in &track.idam {
        let mfm = p & 0x8000 != 0;
        let i = usize::from(p & 0x3fff);
        if at(i) != 0xfe {
            continue;
        }
        let (t, side, sector, code) = (at(i + 1), at(i + 2), at(i + 3), at(i + 4));
        let size = 128usize << (code & 3);
        let Some(dam_pos) = find_dam(track, i, mfm) else {
            continue;
        };
        let dam = at(dam_pos);
        let data: Vec<u8> = (0..size).map(|k| at(dam_pos + 1 + k)).collect();
        let mut crc = 0xffffu16;
        if mfm {
            for _ in 0..3 {
                crc = crc16(crc, 0xa1);
            }
        }
        crc = crc16(crc, dam);
        for &b in &data {
            crc = crc16(crc, b);
        }
        let stored = u16::from_be_bytes([at(dam_pos + 1 + size), at(dam_pos + 2 + size)]);
        out.push(Sector {
            track: t,
            side,
            sector,
            size,
            dam,
            data,
            dam_pos,
            crc_ok: stored == crc,
        });
    }
    out
}

/// An initialised blank TI-99/4A disk (single density, 40 tracks, one or
/// two sides) named `name`: sector 0 the volume information block (name,
/// total sectors, 9 sectors a track, "DSK", tracks and sides, the
/// allocation bitmap with sectors 0 and 1 in use), sector 1 the empty
/// file directory.
pub fn blank_ti_dsk(name: &str, sides: u8) -> Vec<u8> {
    let sides = usize::from(sides.clamp(1, 2));
    let total = TI_SECTORS * TI_TRACKS * sides;
    let mut img = vec![0xe5; total * 256];
    img[..512].fill(0);
    let vib = &mut img[..256];
    let mut n: Vec<u8> = name.bytes().take(10).collect();
    n.resize(10, b' ');
    vib[..10].copy_from_slice(&n);
    vib[10..12].copy_from_slice(&(total as u16).to_be_bytes());
    vib[12] = TI_SECTORS as u8;
    vib[13..16].copy_from_slice(b"DSK");
    vib[16] = b' ';
    vib[17] = TI_TRACKS as u8;
    vib[18] = sides as u8;
    vib[19] = 1;
    // The bitmap from byte >38: one bit per sector; past the disk: used.
    for (i, b) in vib[0x38..0x100].iter_mut().enumerate() {
        let mut v = 0u8;
        for bit in 0..8 {
            let sector = i * 8 + bit;
            if sector < 2 || sector >= total {
                v |= 1 << bit;
            }
        }
        *b = v;
    }
    img
}

/// A blank JV1-style single-density image of `tracks` tracks (tests).
pub fn blank_jv1(tracks: usize) -> Vec<u8> {
    vec![0xe5; tracks * 2560]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An IMD image: cylinder 0 in FM with 128-byte sectors, cylinder 1
    /// in MFM with 256-byte ones numbered from 5, one of them filled, one
    /// with the deleted mark, the IDs naming cylinder 7.
    fn imd(rate: u8) -> Vec<u8> {
        let mut b = b"IMD 1.18: 01/02/2026 03:04:05\r\na comment\r\n\x1a".to_vec();
        b.extend([rate, 0, 0, 2, 0, 1, 2]);
        b.extend([2, 0x11, 2, 0x22]);
        b.extend([rate + 3, 1, 0x80, 3, 1, 5, 7, 6, 7, 7, 7]);
        b.push(1);
        b.extend((0..=255u8).collect::<Vec<u8>>());
        b.extend([2, 0xe5]);
        b.extend([4, 0x33]);
        b
    }

    #[test]
    fn imd_images_become_tracks_and_go_back() {
        let bytes = imd(2);
        let img = Image::parse(&bytes).unwrap();
        assert_eq!(img.format, Format::Imd);
        assert!(!img.eight_inch);
        assert_eq!((img.sides, img.cylinders()), (1, 2));
        assert_eq!(img.tracks[0].data.len(), SD_TRACK_BYTES);
        assert_eq!(img.tracks[1].data.len(), DD_TRACK_BYTES);
        assert!(!img.tracks[0].is_mfm() && img.tracks[1].is_mfm());
        let s = decode_sectors(&img.tracks[1]);
        // The order on the track and the cylinder the IDs carry.
        assert_eq!(
            s.iter().map(|s| (s.track, s.sector)).collect::<Vec<_>>(),
            [(7, 5), (7, 7), (7, 6)]
        );
        assert!(s.iter().all(|s| s.crc_ok && s.size == 256));
        assert_eq!(s[0].data[200], 200);
        assert!(s[1].data.iter().all(|&b| b == 0xe5));
        assert_eq!((s[1].dam, s[2].dam), (0xfb, 0xf8));
        // Written back: the same file, comment and all.
        assert_eq!(img.to_bytes(&bytes), bytes);

        // 500 kbit/s: an 8-inch disk with its longer tracks.
        let fast = Image::parse(&imd(0)).unwrap();
        assert!(fast.eight_inch);
        assert_eq!(fast.tracks[1].data.len(), DD8_TRACK_BYTES);
        assert_eq!(fast.tracks[0].data.len(), DD8_TRACK_BYTES / 2);
        assert_eq!(fast.to_bytes(&imd(0)), imd(0));

        // A file cut short is refused, never read past.
        for cut in [44, 50, 60, bytes.len() - 1] {
            assert!(Image::parse(&bytes[..cut]).is_err(), "{cut}");
        }
        let mut bad = bytes.clone();
        bad[43] = 9; // the mode
        assert!(Image::parse(&bad).is_err());
    }

    #[test]
    fn msx_sector_dumps_round_trip() {
        let mut bytes = vec![0u8; 737_280];
        for (i, s) in bytes.chunks_mut(512).enumerate() {
            s[0] = i as u8;
            s[511] = (i >> 8) as u8;
        }
        let mut img = Image::parse_msx_dsk(&bytes).unwrap();
        assert_eq!(
            (img.format, img.sides, img.cylinders()),
            (Format::MsxDsk, 2, 80)
        );
        // Cylinder 1, side 1, sector 3 is the 30th sector of the image.
        let secs = decode_sectors(img.track(1, 1).unwrap());
        assert_eq!(secs.len(), 9);
        let s = secs.iter().find(|s| s.sector == 3).unwrap();
        assert_eq!((s.size, s.data[0], s.track, s.side), (512, 29, 1, 1));
        assert_eq!(img.to_bytes(&bytes), bytes);
        // A changed sector goes back to its place.
        let t = img.track_mut(1, 1).unwrap();
        let idam = usize::from(t.idam[2] & 0x3fff);
        let dam = find_dam(t, idam, true).unwrap();
        t.data[dam + 1] = 0xee;
        let out = img.to_bytes(&bytes);
        assert_eq!(out[29 * 512], 0xee);
        assert_eq!(out.len(), bytes.len());
        // The parameter block decides between 40 double-sided and 80
        // single-sided cylinders; without one the size does.
        let mut small = vec![0u8; 368_640];
        assert_eq!(Image::parse_msx_dsk(&small).unwrap().sides, 1);
        small[0x13..0x15].copy_from_slice(&720u16.to_le_bytes());
        small[0x18] = 9;
        small[0x1a] = 2;
        let img = Image::parse_msx_dsk(&small).unwrap();
        assert_eq!((img.sides, img.cylinders()), (2, 40));
        assert!(Image::parse_msx_dsk(&bytes[..1000]).is_err());
    }

    #[test]
    fn ti_disks_keep_their_sector_order() {
        let mut bytes = blank_ti_dsk("TEST", 2);
        assert_eq!(bytes.len(), 184_320);
        assert_eq!(&bytes[13..16], b"DSK");
        assert_eq!(bytes[0x38], 0x03, "VIB and FDIR in use");
        // Logical sector 361 is side 1, track 39, sector 1.
        bytes[361 * 256] = 0x77;
        let img = Image::parse_ti_dsk(&bytes).unwrap();
        assert_eq!((img.sides, img.cylinders()), (2, 40));
        let t = decode_sectors(img.track(39, 1).unwrap());
        let s1 = t.iter().find(|s| s.sector == 1).unwrap();
        assert_eq!((s1.data[0], s1.side, s1.track), (0x77, 1, 39));
        let order: Vec<u8> = decode_sectors(img.track(0, 0).unwrap())
            .iter()
            .map(|s| s.sector)
            .collect();
        assert_eq!(order, TI_INTERLEAVE);
        // Back to the sector dump.
        let mut img2 = img.clone();
        let tr = img2.track_mut(2, 0).unwrap();
        let s = decode_sectors(tr)
            .into_iter()
            .find(|s| s.sector == 4)
            .unwrap();
        tr.data[s.dam_pos + 1] = 0x55;
        let out = img2.to_bytes(&bytes);
        assert_eq!(out[(2 * 9 + 4) * 256], 0x55);
        assert_eq!(out[361 * 256], 0x77);
        assert!(Image::parse_ti_dsk(&bytes[..1000]).is_err());
    }

    #[test]
    fn jv1_tracks_decode_back() {
        let mut bytes = blank_jv1(35);
        bytes[17 * 2560 + 3 * 256] = 0x42; // track 17, sector 3
        let img = Image::parse(&bytes).unwrap();
        assert_eq!(
            (img.format, img.sides, img.cylinders()),
            (Format::Jv1, 1, 35)
        );
        let sectors = decode_sectors(img.track(17, 0).unwrap());
        assert_eq!(sectors.len(), 10);
        let s3 = sectors.iter().find(|s| s.sector == 3).unwrap();
        assert_eq!(
            (s3.dam, s3.data[0], s3.crc_ok, s3.size),
            (0xfa, 0x42, true, 256)
        );
        assert_eq!(decode_sectors(img.track(0, 0).unwrap())[0].dam, 0xfb);
        // Writing back keeps the layout.
        let mut img2 = img.clone();
        let t = img2.track_mut(5, 0).unwrap();
        let s = decode_sectors(t)[0].clone();
        t.data[s.dam_pos + 1] = 0x99;
        let out = img2.to_bytes(&bytes);
        assert_eq!(out.len(), bytes.len());
        assert_eq!(out[5 * 2560 + usize::from(s.sector) * 256], 0x99);
    }

    #[test]
    fn dmk_round_trip_single_and_double_density() {
        let sd = build_track(
            &[SectorSpec {
                track: 0,
                side: 0,
                sector: 1,
                size: 256,
                dam: 0xfb,
                data: vec![0x11; 256],
                mfm: false,
            }],
            false,
        );
        let dd = build_track(
            &[SectorSpec {
                track: 0,
                side: 1,
                sector: 2,
                size: 512,
                dam: 0xf8,
                data: vec![0x22; 512],
                mfm: true,
            }],
            true,
        );
        let img = Image {
            format: Format::Dmk,
            protected: true,
            sides: 2,
            tracks: vec![sd.clone(), dd.clone()],
            sd_single: false,
            track_len: DEFAULT_TRACK_LEN,
            jv3: Vec::new(),
            eight_inch: false,
        };
        let bytes = img.to_dmk();
        assert_eq!(bytes.len(), DMK_HEADER + 2 * DEFAULT_TRACK_LEN);
        assert_eq!(bytes[0], 0xff);
        let back = Image::parse(&bytes).unwrap();
        assert_eq!(back.format, Format::Dmk);
        assert!(back.protected);
        let s = decode_sectors(back.track(0, 0).unwrap());
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].sector, s[0].size, s[0].data[5], s[0].crc_ok),
            (1, 256, 0x11, true)
        );
        let d = decode_sectors(back.track(0, 1).unwrap());
        assert_eq!(
            (d[0].sector, d[0].size, d[0].dam, d[0].crc_ok),
            (2, 512, 0xf8, true)
        );
        assert!(back.track(0, 1).unwrap().is_mfm());
        assert!(!back.track(0, 0).unwrap().is_mfm());
    }

    #[test]
    fn jv3_descriptors_and_write_back() {
        let mut bytes = vec![0xffu8; JV3_SECTORS * 3 + 1];
        // Track 0 sectors 0 and 1 (256 bytes, SD), track 1 sector 5 DD 512, side 1.
        for (i, d) in [(0u8, 0u8, 0x00u8), (0, 1, 0x20), (1, 5, 0x80 | 0x10 | 0x03)]
            .iter()
            .enumerate()
        {
            bytes[3 * i..3 * i + 3].copy_from_slice(&[d.0, d.1, d.2]);
        }
        bytes[JV3_SECTORS * 3] = 0xff; // writable
        bytes.extend(vec![0x01; 256]);
        bytes.extend(vec![0x02; 256]);
        bytes.extend(vec![0x03; 512]);
        let img = Image::parse(&bytes).unwrap();
        assert_eq!(
            (img.format, img.sides, img.protected),
            (Format::Jv3, 2, false)
        );
        let t0 = decode_sectors(img.track(0, 0).unwrap());
        assert_eq!(t0.len(), 2);
        assert_eq!((t0[1].sector, t0[1].dam, t0[1].data[0]), (1, 0xfa, 0x02));
        let t1 = decode_sectors(img.track(1, 1).unwrap());
        assert_eq!((t1[0].sector, t1[0].size, t1[0].data[0]), (5, 512, 0x03));
        assert!(img.track(1, 1).unwrap().is_mfm());
        let mut img2 = img.clone();
        let t = img2.track_mut(1, 1).unwrap();
        let s = decode_sectors(t)[0].clone();
        t.data[s.dam_pos + 1] = 0x77;
        let out = img2.to_bytes(&bytes);
        assert_eq!(out[JV3_SECTORS * 3 + 1 + 512], 0x77);
        assert!(Image::parse(&[0u8; 100]).is_err());
    }
}
