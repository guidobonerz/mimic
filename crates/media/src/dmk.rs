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
//! JV images are built into tracks on load (gaps, marks and CRCs as the
//! TRSDOS formatter writes them) and written back sector by sector.

use emu_core::media::MediaError;

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
        if let Some(img) = Self::parse_dmk(bytes)? {
            return Ok(img);
        }
        if let Some(img) = Self::parse_jv3(bytes)? {
            return Ok(img);
        }
        Self::parse_jv1(bytes)
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
        }))
    }

    /// The image as a file in its own format. JV images get their sectors
    /// back from the tracks; sectors a track no longer holds keep their
    /// old bytes (`original`).
    pub fn to_bytes(&self, original: &[u8]) -> Vec<u8> {
        match self.format {
            Format::Dmk => self.to_dmk(),
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
    let len = if mfm { DD_TRACK_BYTES } else { SD_TRACK_BYTES };
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

/// A blank JV1-style single-density image of `tracks` tracks (tests).
pub fn blank_jv1(tracks: usize) -> Vec<u8> {
    vec![0xe5; tracks * 2560]
}

#[cfg(test)]
mod tests {
    use super::*;

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
