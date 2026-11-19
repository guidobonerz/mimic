//! DSK: CPC disk images in the CPCEMU formats, standard ("MV - CPC") and
//! extended ("EXTENDED CPC DSK File"); format description by Marco Vieth,
//! Ulrich Doewich and Kevin Thacker (cpcwiki, "Format:DSK disk image file
//! format").
//!
//! | Offset | Disc information block (256 bytes)                           |
//! | ------ | ------------------------------------------------------------ |
//! | 0x00   | signature                                                    |
//! | 0x22   | creator (14 bytes)                                           |
//! | 0x30   | tracks, 0x31 sides                                           |
//! | 0x32   | standard: size of every track block (16 bits)                |
//! | 0x34   | extended: high byte of each track block's size (0 = unformatted), track by track, side by side |
//!
//! Track blocks follow in the order track 0 side 0, track 0 side 1, …;
//! each starts with a 256-byte header:
//!
//! | Offset | Track information                                            |
//! | ------ | ------------------------------------------------------------ |
//! | 0x00   | "Track-Info\r\n"                                             |
//! | 0x10   | track, 0x11 side, 0x14 sector size N, 0x15 sectors, 0x16 GAP#3, 0x17 filler |
//! | 0x18   | 8 bytes per sector: C, H, R, N, FDC ST1, ST2, data length (extended) |
//!
//! The sector data follows, in the header's order: 128 << N bytes each
//! (the track's N in standard images), the given length in extended ones.
//! Writing changes the bytes in place; formatting rewrites a track inside
//! its block (the new layout must fit).

use mimic_core::media::MediaError;

const STANDARD: &[u8] = b"MV - CPC";
const EXTENDED: &[u8] = b"EXTENDED";
const TRACK_INFO: &[u8] = b"Track-Info\r\n";

fn fmt(msg: impl Into<String>) -> MediaError {
    MediaError::Format(format!("DSK: {}", msg.into()))
}

/// One sector as the controller finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sector {
    /// ID field: cylinder, head, record, size code.
    pub id: [u8; 4],
    /// FDC status bits stored with the sector (errors, deleted data).
    pub st1: u8,
    pub st2: u8,
    /// File offset and length of its data.
    pub offset: usize,
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// File offset and size of the track block.
    pub offset: usize,
    pub size: usize,
    pub sectors: Vec<Sector>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dsk {
    pub extended: bool,
    pub tracks: u8,
    pub sides: u8,
    /// `[track * sides + side]`; `None`: unformatted.
    track_list: Vec<Option<Track>>,
    /// The image file.
    pub bytes: Vec<u8>,
}

/// One sector's ID and size for formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatId {
    pub id: [u8; 4],
}

impl Dsk {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let extended = bytes.starts_with(EXTENDED);
        if !extended && !bytes.starts_with(STANDARD) {
            return Err(fmt("not a CPC disk image"));
        }
        if bytes.len() < 0x100 {
            return Err(fmt("header truncated"));
        }
        let tracks = bytes[0x30];
        let sides = bytes[0x31];
        if !(1..=2).contains(&sides) {
            return Err(fmt(format!("{sides} sides")));
        }
        let count = usize::from(tracks) * usize::from(sides);
        if extended && count > 0xcc {
            return Err(fmt("too many tracks"));
        }
        let std_size = usize::from(u16::from_le_bytes([bytes[0x32], bytes[0x33]]));
        let mut track_list = Vec::with_capacity(count);
        let mut offset = 0x100;
        for i in 0..count {
            let size = if extended {
                usize::from(bytes[0x34 + i]) * 0x100
            } else {
                std_size
            };
            if size == 0 {
                track_list.push(None);
                continue;
            }
            let block = bytes
                .get(offset..offset + size)
                .ok_or_else(|| fmt(format!("track block {i} truncated")))?;
            track_list.push(Self::parse_track(block, offset, extended)?);
            offset += size;
        }
        Ok(Self {
            extended,
            tracks,
            sides,
            track_list,
            bytes: bytes.to_vec(),
        })
    }

    fn parse_track(
        block: &[u8],
        offset: usize,
        extended: bool,
    ) -> Result<Option<Track>, MediaError> {
        if block.len() < 0x100 || !block.starts_with(TRACK_INFO) {
            // Some tools leave unformatted tracks as empty blocks.
            if block.iter().all(|&b| b == 0) {
                return Ok(None);
            }
            return Err(fmt("track header missing"));
        }
        let track_n = block[0x14];
        let count = usize::from(block[0x15]).min(29);
        let mut sectors = Vec::with_capacity(count);
        let mut data = offset + 0x100;
        for k in 0..count {
            let e = &block[0x18 + 8 * k..0x20 + 8 * k];
            let len = if extended {
                usize::from(u16::from_le_bytes([e[6], e[7]]))
            } else {
                128usize << track_n.min(6)
            };
            sectors.push(Sector {
                id: [e[0], e[1], e[2], e[3]],
                st1: e[4],
                st2: e[5],
                offset: data,
                len,
            });
            data += len;
        }
        if data > offset + block.len() {
            return Err(fmt("sector data beyond the track block"));
        }
        Ok(Some(Track {
            offset,
            size: block.len(),
            sectors,
        }))
    }

    /// A track as it is on the disk, if formatted.
    pub fn track(&self, track: u8, side: u8) -> Option<&Track> {
        if track >= self.tracks || side >= self.sides {
            return None;
        }
        self.track_list[usize::from(track) * usize::from(self.sides) + usize::from(side)].as_ref()
    }

    /// The data of a sector.
    pub fn data(&self, s: &Sector) -> &[u8] {
        &self.bytes[s.offset..s.offset + s.len]
    }

    /// Write a sector's data (as much as it holds).
    pub fn write(&mut self, s: &Sector, data: &[u8]) {
        let n = data.len().min(s.len);
        self.bytes[s.offset..s.offset + n].copy_from_slice(&data[..n]);
    }

    /// Format a track in place: new sector IDs, all `filler`. Fails if
    /// the track does not exist in the image or the layout does not fit
    /// its block.
    pub fn format(
        &mut self,
        track: u8,
        side: u8,
        n: u8,
        gap3: u8,
        filler: u8,
        ids: &[FormatId],
    ) -> Result<(), MediaError> {
        if track >= self.tracks || side >= self.sides {
            return Err(MediaError::OutOfRange);
        }
        let i = usize::from(track) * usize::from(self.sides) + usize::from(side);
        let Some(old) = self.track_list[i].clone() else {
            return Err(fmt("cannot format an unformatted track in place"));
        };
        let len = 128usize << n.min(6);
        if ids.len() > 29 || 0x100 + ids.len() * len > old.size {
            return Err(MediaError::OutOfRange);
        }
        let o = old.offset;
        let block = &mut self.bytes[o..o + old.size];
        block.fill(0);
        block[..TRACK_INFO.len()].copy_from_slice(TRACK_INFO);
        block[0x10] = track;
        block[0x11] = side;
        block[0x14] = n;
        block[0x15] = ids.len() as u8;
        block[0x16] = gap3;
        block[0x17] = filler;
        for (k, f) in ids.iter().enumerate() {
            let e = &mut block[0x18 + 8 * k..0x20 + 8 * k];
            e[..4].copy_from_slice(&f.id);
            if self.extended {
                e[6..8].copy_from_slice(&(len as u16).to_le_bytes());
            }
        }
        block[0x100..0x100 + ids.len() * len].fill(filler);
        let block = self.bytes[o..o + old.size].to_vec();
        self.track_list[i] = Self::parse_track(&block, o, self.extended)?;
        Ok(())
    }
}

/// A blank extended image in the CPC's DATA format (AMSDOS): `tracks`
/// tracks of one side, 9 sectors &C1–&C9 of 512 bytes, filled with &E5
/// (an empty directory).
pub fn blank_data(tracks: u8) -> Vec<u8> {
    let track_size = 0x100 + 9 * 512;
    let mut out = vec![0u8; 0x100];
    out[..34].copy_from_slice(b"EXTENDED CPC DSK File\r\nDisk-Info\r\n");
    out[0x22..0x30].copy_from_slice(b"mimic         ");
    out[0x30] = tracks;
    out[0x31] = 1;
    for t in 0..usize::from(tracks) {
        out[0x34 + t] = (track_size / 0x100) as u8;
    }
    for t in 0..tracks {
        let mut block = vec![0u8; track_size];
        block[..TRACK_INFO.len()].copy_from_slice(TRACK_INFO);
        block[0x10] = t;
        block[0x14] = 2;
        block[0x15] = 9;
        block[0x16] = 0x4e;
        block[0x17] = 0xe5;
        // Interleaved like the CPC's FORMAT: C1 C6 C2 C7 C3 C8 C4 C9 C5.
        for (k, r) in [0xc1u8, 0xc6, 0xc2, 0xc7, 0xc3, 0xc8, 0xc4, 0xc9, 0xc5]
            .into_iter()
            .enumerate()
        {
            let e = &mut block[0x18 + 8 * k..0x20 + 8 * k];
            e[..4].copy_from_slice(&[t, 0, r, 2]);
            e[6..8].copy_from_slice(&512u16.to_le_bytes());
        }
        block[0x100..].fill(0xe5);
        out.extend_from_slice(&block);
    }
    out
}

/// The layout of a raw sector dump ("`.img`"): every track of every side
/// in order (track 0 side 0, track 0 side 1, …), the sectors of a track
/// in record order from `first`, all of one size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawGeometry {
    pub tracks: u8,
    pub sides: u8,
    pub sectors: u8,
    /// Size code: 128 << n bytes.
    pub n: u8,
    /// The first record number.
    pub first: u8,
}

impl RawGeometry {
    pub fn sector_len(&self) -> usize {
        128 << self.n.min(6)
    }

    /// Size of the image.
    pub fn len(&self) -> usize {
        usize::from(self.tracks)
            * usize::from(self.sides)
            * usize::from(self.sectors)
            * self.sector_len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Track blocks of images made from raw dumps hold at least this much
/// sector data, so that a track can be formatted in place with another
/// layout (five sectors of 1024 bytes on a track of 16 × 256).
const RAW_TRACK_DATA: usize = 0x1400;

impl Dsk {
    /// An extended image holding a raw dump; `None` if its size does not
    /// match the geometry.
    pub fn from_raw(raw: &[u8], g: RawGeometry) -> Option<Self> {
        if raw.len() != g.len() || g.is_empty() || g.sides > 2 || g.sectors > 29 {
            return None;
        }
        let len = g.sector_len();
        let data = (usize::from(g.sectors) * len).max(RAW_TRACK_DATA);
        let track_size = (0x100 + data).div_ceil(0x100) * 0x100;
        if track_size / 0x100 > 0xff {
            return None;
        }
        let mut out = vec![0u8; 0x100];
        out[..34].copy_from_slice(b"EXTENDED CPC DSK File\r\nDisk-Info\r\n");
        out[0x22..0x30].copy_from_slice(b"mimic         ");
        out[0x30] = g.tracks;
        out[0x31] = g.sides;
        let count = usize::from(g.tracks) * usize::from(g.sides);
        if count > 0xcc {
            return None;
        }
        out[0x34..0x34 + count].fill((track_size / 0x100) as u8);
        let mut src = raw.chunks(len);
        for t in 0..g.tracks {
            for side in 0..g.sides {
                let mut block = vec![0u8; track_size];
                block[..TRACK_INFO.len()].copy_from_slice(TRACK_INFO);
                block[0x10] = t;
                block[0x11] = side;
                block[0x14] = g.n;
                block[0x15] = g.sectors;
                block[0x16] = 0x4e;
                block[0x17] = 0xe5;
                for k in 0..usize::from(g.sectors) {
                    let e = &mut block[0x18 + 8 * k..0x20 + 8 * k];
                    e[..4].copy_from_slice(&[t, side, g.first.wrapping_add(k as u8), g.n]);
                    e[6..8].copy_from_slice(&(len as u16).to_le_bytes());
                    let at = 0x100 + k * len;
                    block[at..at + len].copy_from_slice(src.next()?);
                }
                out.extend_from_slice(&block);
            }
        }
        Self::parse(&out).ok()
    }

    /// The raw dump of the image in that geometry: each sector found by
    /// its ID (cylinder and record; a sector missing or of another size
    /// reads as `$E5`).
    pub fn to_raw(&self, g: RawGeometry) -> Vec<u8> {
        let len = g.sector_len();
        let mut out = vec![0xe5u8; g.len()];
        let mut at = 0;
        for t in 0..g.tracks {
            for side in 0..g.sides {
                let track = self.track(t, side);
                for k in 0..g.sectors {
                    let r = g.first.wrapping_add(k);
                    let found = track.and_then(|tr| {
                        tr.sectors
                            .iter()
                            .find(|s| s.id[0] == t && s.id[2] == r && s.len == len)
                    });
                    if let Some(s) = found {
                        out[at..at + len].copy_from_slice(self.data(s));
                    }
                    at += len;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_data_disk() {
        let d = Dsk::parse(&blank_data(40)).unwrap();
        assert!(d.extended);
        assert_eq!((d.tracks, d.sides), (40, 1));
        let t = d.track(39, 0).unwrap();
        assert_eq!(t.sectors.len(), 9);
        assert_eq!(t.sectors[1].id, [39, 0, 0xc6, 2]);
        assert!(d.data(&t.sectors[0]).iter().all(|&b| b == 0xe5));
        assert!(d.track(40, 0).is_none());
    }

    #[test]
    fn standard_image_and_sector_write() {
        // One track, two sectors of 256 bytes.
        let mut img = vec![0u8; 0x100];
        img[..8].copy_from_slice(STANDARD);
        img[0x30] = 1;
        img[0x31] = 1;
        img[0x32..0x34].copy_from_slice(&(0x100u16 + 512).to_le_bytes());
        let mut t = vec![0u8; 0x300];
        t[..12].copy_from_slice(TRACK_INFO);
        t[0x14] = 1;
        t[0x15] = 2;
        t[0x18..0x1c].copy_from_slice(&[0, 0, 1, 1]);
        t[0x20..0x24].copy_from_slice(&[0, 0, 2, 1]);
        t[0x200] = 0x42; // first byte of sector 2
        img.extend_from_slice(&t);
        let mut d = Dsk::parse(&img).unwrap();
        let s2 = d.track(0, 0).unwrap().sectors[1];
        assert_eq!((s2.len, d.data(&s2)[0]), (256, 0x42));
        d.write(&s2, &[7; 300]);
        assert_eq!(d.data(&s2), &[7; 256]);
        assert_eq!(d.bytes.len(), img.len());
    }

    #[test]
    fn format_in_place() {
        let mut d = Dsk::parse(&blank_data(2)).unwrap();
        let ids: Vec<FormatId> = (1..=8).map(|r| FormatId { id: [1, 0, r, 2] }).collect();
        d.format(1, 0, 2, 0x52, 0x00, &ids).unwrap();
        let t = d.track(1, 0).unwrap();
        assert_eq!(t.sectors.len(), 8);
        assert_eq!(t.sectors[7].id, [1, 0, 8, 2]);
        assert!(d.data(&t.sectors[0]).iter().all(|&b| b == 0));
        // Ten 1 KB sectors do not fit a 4.75 KB track block.
        let big: Vec<FormatId> = (1..=10).map(|r| FormatId { id: [1, 0, r, 3] }).collect();
        assert!(d.format(1, 0, 3, 0x52, 0, &big).is_err());
        // The reparsed image agrees.
        assert_eq!(Dsk::parse(&d.bytes).unwrap(), d);
    }

    #[test]
    fn rejects_other_files() {
        assert!(Dsk::parse(b"ZXTape!").is_err());
        let mut bad = blank_data(1);
        bad.truncate(0x180);
        assert!(Dsk::parse(&bad).is_err());
    }

    #[test]
    fn raw_dump_round_trip() {
        let g = RawGeometry {
            tracks: 80,
            sides: 2,
            sectors: 5,
            n: 3,
            first: 1,
        };
        assert_eq!(g.len(), 819_200);
        let raw: Vec<u8> = (0..g.len()).map(|i| (i / 1024) as u8).collect();
        let mut d = Dsk::from_raw(&raw, g).unwrap();
        assert_eq!((d.tracks, d.sides), (80, 2));
        let t = d.track(1, 1).unwrap();
        assert_eq!(t.sectors[2].id, [1, 1, 3, 3]);
        // Track 1 side 1 sector 3: the (1 * 2 + 1) * 5 + 2 = 17th.
        assert!(d.data(&t.sectors[2]).iter().all(|&b| b == 17));
        assert_eq!(d.to_raw(g), raw);
        assert!(Dsk::from_raw(&raw[1..], g).is_none(), "size must match");
        // Reformatted to 9 × 512, the track reads back as E5 in the old
        // geometry.
        let ids: Vec<FormatId> = (1..=9).map(|r| FormatId { id: [0, 0, r, 2] }).collect();
        d.format(0, 0, 2, 0x4e, 0xe5, &ids).unwrap();
        let back = d.to_raw(g);
        assert!(back[..5 * 1024].iter().all(|&b| b == 0xe5));
        assert_eq!(back[5 * 1024..], raw[5 * 1024..]);
    }
}
