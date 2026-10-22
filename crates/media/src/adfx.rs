//! Extended ADF images: tracks stored as raw MFM bit streams where a plain
//! ADF cannot hold them (copy protections, non-DOS formats), next to
//! ordinary AmigaDOS tracks.
//!
//! Two layouts exist (both big endian), as documented for the UAE family of
//! emulators (see `docs/sources.md`):
//!
//! - **`UAE--ADF`** (the old one): the 8-byte signature, then 160 entries of
//!   `u32 sync, u32 length`; a track with `sync == 0` is `length` bytes of
//!   AmigaDOS sectors (11 × 512), else `length` bytes of raw MFM. The data
//!   follow in track order.
//! - **`UAE-1ADF`**: the signature, `u16 reserved`, `u16 tracks`, then per
//!   track `u16 reserved, u16 type, u32 length (bytes), u32 length (bits)`
//!   with type 0 = AmigaDOS sectors and 1 = raw MFM; the data follow in
//!   track order.
//!
//! Raw tracks are kept bit-exact; writes to them go back into the same
//! bytes (their length cannot change), writes to sector tracks go back as
//! sectors. [`Image`] remembers where every track lies in the file.

use emu_core::media::MediaError;

use crate::adf;

pub const SIGNATURE_V1: &[u8; 8] = b"UAE--ADF";
pub const SIGNATURE_V2: &[u8; 8] = b"UAE-1ADF";

/// One track of an extended ADF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Raw MFM bits (else AmigaDOS sectors).
    pub raw: bool,
    /// Offset of the track's bytes in the file.
    pub offset: usize,
    pub len: usize,
    /// Bits on the track (raw tracks).
    pub bits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub tracks: Vec<Track>,
}

/// Is this an extended ADF?
pub fn is_extended(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && (&bytes[..8] == SIGNATURE_V1 || &bytes[..8] == SIGNATURE_V2)
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, MediaError> {
    b.get(o..o + 2)
        .map(|v| u16::from_be_bytes([v[0], v[1]]))
        .ok_or_else(|| MediaError::Format("extended ADF truncated".into()))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, MediaError> {
    b.get(o..o + 4)
        .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
        .ok_or_else(|| MediaError::Format("extended ADF truncated".into()))
}

impl Image {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if !is_extended(bytes) {
            return Err(MediaError::Format("not an extended ADF".into()));
        }
        let v2 = &bytes[..8] == SIGNATURE_V2;
        let (count, entry_len, first) = if v2 {
            (usize::from(u16_at(bytes, 10)?), 12, 12)
        } else {
            (adf::TRACKS, 8, 8)
        };
        if count == 0 || count > adf::TRACKS {
            return Err(MediaError::Format(format!(
                "extended ADF with {count} tracks"
            )));
        }
        let mut tracks = Vec::with_capacity(count);
        let mut offset = first + count * entry_len;
        for t in 0..count {
            let e = first + t * entry_len;
            let (raw, len, bits) = if v2 {
                let kind = u16_at(bytes, e + 2)?;
                let len = u32_at(bytes, e + 4)? as usize;
                let bits = u32_at(bytes, e + 8)?;
                if kind > 1 {
                    return Err(MediaError::Format(format!(
                        "extended ADF track {t}: type {kind}"
                    )));
                }
                (kind == 1, len, bits)
            } else {
                let sync = u32_at(bytes, e)?;
                let len = u32_at(bytes, e + 4)? as usize;
                (sync != 0, len, 8 * len as u32)
            };
            if offset + len > bytes.len() {
                return Err(MediaError::Format(format!(
                    "extended ADF track {t} runs past the file"
                )));
            }
            if raw && (bits == 0 || bits as usize > 8 * len) {
                return Err(MediaError::Format(format!(
                    "extended ADF track {t}: {bits} bits in {len} bytes"
                )));
            }
            if !raw && len != adf::SECTORS * adf::SECTOR_SIZE && len != 0 {
                return Err(MediaError::Format(format!(
                    "extended ADF track {t}: {len} bytes of sectors"
                )));
            }
            tracks.push(Track {
                raw,
                offset,
                len,
                bits: if raw { bits } else { 0 },
            });
            offset += len;
        }
        Ok(Self { tracks })
    }

    /// The MFM bit stream of track `t` (an empty sector track is zeros).
    pub fn track_bits(&self, bytes: &[u8], t: usize) -> (Vec<u8>, u32) {
        let Some(track) = self.tracks.get(t) else {
            return (vec![0; adf::TRACK_BYTES], 8 * adf::TRACK_BYTES as u32);
        };
        let data = &bytes[track.offset..track.offset + track.len];
        if track.raw {
            (data.to_vec(), track.bits)
        } else {
            // Sectors: encode like a plain ADF's track.
            let mut img = vec![0u8; adf::ADF_SIZE];
            let off = t * adf::SECTORS * adf::SECTOR_SIZE;
            if data.len() == adf::SECTORS * adf::SECTOR_SIZE {
                img[off..off + data.len()].copy_from_slice(data);
            }
            (adf::encode_track(&img, t), 8 * adf::TRACK_BYTES as u32)
        }
    }

    /// Put a written track back: raw tracks byte for byte, sector tracks
    /// as decoded sectors. Returns the file range that changed.
    pub fn store_track(&self, bytes: &mut [u8], t: usize, mfm: &[u8]) -> Option<(usize, usize)> {
        let track = self.tracks.get(t)?;
        let dst = &mut bytes[track.offset..track.offset + track.len];
        if track.raw {
            let n = dst.len().min(mfm.len());
            dst[..n].copy_from_slice(&mfm[..n]);
        } else {
            let sectors = adf::decode_track(mfm);
            for s in sectors
                .iter()
                .filter(|s| usize::from(s.track) == t && usize::from(s.sector) < adf::SECTORS)
            {
                let off = usize::from(s.sector) * adf::SECTOR_SIZE;
                if let Some(d) = dst.get_mut(off..off + adf::SECTOR_SIZE) {
                    d.copy_from_slice(&s.data);
                }
            }
        }
        Some((track.offset, track.len))
    }
}

/// A track for [`build`]: `Some((bytes, bits))` a raw track, `None` a
/// sector track; the second element its sectors (11 × 512 bytes, or empty).
pub type BuildTrack<'a> = (Option<(&'a [u8], u32)>, &'a [u8]);

/// Build a version 2 image from tracks (tests, converters).
pub fn build(tracks: &[BuildTrack<'_>]) -> Vec<u8> {
    let mut out = SIGNATURE_V2.to_vec();
    out.extend(0u16.to_be_bytes());
    out.extend((tracks.len() as u16).to_be_bytes());
    for (raw, sectors) in tracks {
        out.extend(0u16.to_be_bytes());
        match raw {
            Some((data, bits)) => {
                out.extend(1u16.to_be_bytes());
                out.extend((data.len() as u32).to_be_bytes());
                out.extend(bits.to_be_bytes());
            }
            None => {
                out.extend(0u16.to_be_bytes());
                out.extend((sectors.len() as u32).to_be_bytes());
                out.extend(0u32.to_be_bytes());
            }
        }
    }
    for (raw, sectors) in tracks {
        match raw {
            Some((data, _)) => out.extend_from_slice(data),
            None => out.extend_from_slice(sectors),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_round_trip_with_raw_and_sector_tracks() {
        let raw: Vec<u8> = (0..100u32).map(|i| (i * 37) as u8).collect();
        let sectors: Vec<u8> = (0..adf::SECTORS * 512).map(|i| i as u8).collect();
        let file = build(&[(Some((&raw, 795)), &[]), (None, &sectors), (None, &[])]);
        assert!(is_extended(&file));
        let img = Image::parse(&file).unwrap();
        assert_eq!(img.tracks.len(), 3);
        let (bits, n) = img.track_bits(&file, 0);
        assert_eq!((bits.as_slice(), n), (raw.as_slice(), 795));
        let (bits, n) = img.track_bits(&file, 1);
        assert_eq!(n, 8 * adf::TRACK_BYTES as u32);
        let decoded = adf::decode_track(&bits);
        assert_eq!(decoded.len(), adf::SECTORS);
        assert_eq!(
            decoded.iter().find(|s| s.sector == 3).unwrap().data,
            sectors[3 * 512..4 * 512]
        );
        let (bits, _) = img.track_bits(&file, 2);
        assert_eq!(
            adf::decode_track(&bits).len(),
            adf::SECTORS,
            "an empty sector track reads as zeros"
        );
        // Write back: raw bytes in place, sectors decoded.
        let mut file2 = file.clone();
        let new_raw: Vec<u8> = raw.iter().map(|b| !b).collect();
        img.store_track(&mut file2, 0, &new_raw);
        let mut img2 = vec![0u8; adf::ADF_SIZE];
        img2[adf::SECTORS * 512..adf::SECTORS * 512 + 512].fill(0xee);
        let mfm = adf::encode_track(&img2, 1);
        img.store_track(&mut file2, 1, &mfm);
        let back = Image::parse(&file2).unwrap();
        assert_eq!(back.track_bits(&file2, 0).0, new_raw);
        let t1 = &file2[back.tracks[1].offset..][..512];
        assert!(t1.iter().all(|&b| b == 0xee));
    }

    #[test]
    fn v1_layout_and_bad_files() {
        let mut file = SIGNATURE_V1.to_vec();
        for t in 0..adf::TRACKS {
            let (sync, len) = if t == 0 { (0x4489u32, 16u32) } else { (0, 0) };
            file.extend(sync.to_be_bytes());
            file.extend(len.to_be_bytes());
        }
        file.extend([0xaa; 16]);
        let img = Image::parse(&file).unwrap();
        assert!(img.tracks[0].raw);
        assert_eq!(img.tracks[0].bits, 128);
        assert!(!img.tracks[1].raw);
        file.truncate(file.len() - 1);
        assert!(Image::parse(&file).is_err(), "truncated");
        assert!(Image::parse(b"UAE-1ADF\0\0\0\0").is_err(), "no tracks");
        assert!(Image::parse(&[0; 100]).is_err());
    }
}
