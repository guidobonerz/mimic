//! 1581 disks as decoded MFM byte streams: one stream per track and side,
//! [`TRACK_BYTES`] bytes from the index hole, sync bytes written with a
//! missing clock flagged with [`MARK`].
//!
//! Layout as the 1581 DOS formats (its write-track routine): 32 × `$4E`,
//! then per sector 12 × `$00`, 3 × `$A1` (mark), `$FE`, track (0–79), side
//! (0–1), sector (1–10), size 2 (512 bytes), CRC, 22 × `$4E`, 12 × `$00`,
//! 3 × `$A1` (mark), `$FB`, 512 data bytes, CRC, 35 × `$4E`; `$4E` up to
//! the index. Logical track `t`, sector `s` of a D81 lies on track `t - 1`,
//! side `s / 20`, physical sector `(s % 20) / 2 + 1`, first or second half.
//! CRC: CCITT, preset `$FFFF`, over the marks, the address mark and the
//! field. Sources: Commodore 1581 User's Guide; WD1772 data sheet.

use crate::d64::{Format, D64};

/// Bytes on a double-density track at 300 rpm (250 kbit/s).
pub const TRACK_BYTES: usize = 6250;
/// Flag on a stream byte written with a missing clock (a sync mark).
pub const MARK: u16 = 0x100;
/// 512-byte sectors per track and side.
pub const SECTORS: u8 = 10;
const GAP3: usize = 35;

/// CRC-CCITT of one byte.
pub fn crc16(crc: u16, byte: u8) -> u16 {
    let mut c = crc ^ (u16::from(byte) << 8);
    for _ in 0..8 {
        c = if c & 0x8000 != 0 {
            (c << 1) ^ 0x1021
        } else {
            c << 1
        };
    }
    c
}

/// A sector found in a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sector {
    pub track: u8,
    pub side: u8,
    pub sector: u8,
    pub data: Vec<u8>,
}

fn field(out: &mut Vec<u16>, mark: u8, bytes: &[u8]) {
    out.extend(std::iter::repeat_n(0u16, 12));
    out.extend(std::iter::repeat_n(MARK | 0xa1, 3));
    out.push(u16::from(mark));
    let mut crc = [0xa1, 0xa1, 0xa1, mark].into_iter().fold(0xffff, crc16);
    for &b in bytes {
        out.push(u16::from(b));
        crc = crc16(crc, b);
    }
    out.push(crc >> 8);
    out.push(crc & 0xff);
}

/// A formatted track with the given 512-byte sectors (1–10).
pub fn format_track(track: u8, side: u8, sectors: &[[u8; 512]]) -> Vec<u16> {
    let mut out = vec![0x4e; 32];
    for (i, data) in sectors.iter().enumerate() {
        field(&mut out, 0xfe, &[track, side, i as u8 + 1, 2]);
        out.extend(std::iter::repeat_n(0x4e, 22));
        field(&mut out, 0xfb, data);
        out.extend(std::iter::repeat_n(0x4e, GAP3));
    }
    out.resize(TRACK_BYTES, 0x4e);
    out
}

/// The stream of a D81's track (0–79) and side.
pub fn encode_track(image: &D64, track: u8, side: u8) -> Vec<u16> {
    let sectors: Vec<[u8; 512]> = (0..SECTORS)
        .map(|r| {
            let first = side * 20 + r * 2;
            let mut buf = [0u8; 512];
            for half in 0..2u8 {
                if let Some(s) = image.sector(track + 1, first + half) {
                    buf[usize::from(half) * 256..][..256].copy_from_slice(s);
                }
            }
            buf
        })
        .collect();
    format_track(track, side, &sectors)
}

/// The sectors with intact ID and data fields in a stream.
pub fn decode_track(stream: &[u16]) -> Vec<Sector> {
    let n = stream.len();
    let at = |i: usize| stream[i % n];
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        // An ID address mark after at least one sync mark.
        if at(i) == MARK | 0xa1 && at(i + 1) == 0xfe {
            let id: Vec<u8> = (0..6).map(|k| at(i + 2 + k) as u8).collect();
            let crc = [0xa1, 0xa1, 0xa1, 0xfe]
                .into_iter()
                .chain(id[..4].iter().copied())
                .fold(0xffff, crc16);
            if u16::from_be_bytes([id[4], id[5]]) == crc {
                if let Some(data) = data_field(stream, i + 8, 128 << (id[3] & 3)) {
                    out.push(Sector {
                        track: id[0],
                        side: id[1],
                        sector: id[2],
                        data,
                    });
                }
            }
            i += 8;
        } else {
            i += 1;
        }
    }
    out
}

/// The data field within 43 bytes from `from`, if its CRC is right.
fn data_field(stream: &[u16], from: usize, len: usize) -> Option<Vec<u8>> {
    let n = stream.len();
    let at = |i: usize| stream[i % n];
    let start =
        (from..from + 43).find(|&i| at(i) == MARK | 0xa1 && matches!(at(i + 1), 0xfb | 0xf8))?;
    let mark = at(start + 1) as u8;
    let data: Vec<u8> = (0..len).map(|k| at(start + 2 + k) as u8).collect();
    let crc = [0xa1, 0xa1, 0xa1, mark]
        .into_iter()
        .chain(data.iter().copied())
        .fold(0xffff, crc16);
    let stored = u16::from_be_bytes([at(start + 2 + len) as u8, at(start + 3 + len) as u8]);
    (stored == crc).then_some(data)
}

/// Put the 512-byte sectors of a stream back into a D81 (sectors whose ID
/// does not fit the image are left out).
pub fn store_sectors(image: &mut D64, sectors: &[Sector]) {
    if image.format != Format::D81 {
        return;
    }
    for s in sectors {
        if s.data.len() != 512 || s.side > 1 || !(1..=SECTORS).contains(&s.sector) {
            continue;
        }
        let first = s.side * 20 + (s.sector - 1) * 2;
        for half in 0..2u8 {
            if let Some(dst) = image.sector_mut(s.track + 1, first + half) {
                dst.copy_from_slice(&s.data[usize::from(half) * 256..][..256]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d81_tracks_round_trip() {
        let mut d = D64::blank_d81(b"MFM", *b"MF");
        d.write_file(
            b"DATA",
            2,
            &(0..5000u32).map(|i| i as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        let mut back = D64::parse(&vec![0; 819_200]).unwrap();
        for t in 0..80 {
            for side in 0..2 {
                let stream = encode_track(&d, t, side);
                assert_eq!(stream.len(), TRACK_BYTES);
                let sectors = decode_track(&stream);
                assert_eq!(sectors.len(), 10, "track {t} side {side}");
                store_sectors(&mut back, &sectors);
            }
        }
        assert_eq!(back, d);
    }

    #[test]
    fn bad_crc_drops_the_sector() {
        let d = D64::blank_d81(b"MFM", *b"MF");
        let mut stream = encode_track(&d, 39, 0);
        // Corrupt a data byte of the first sector.
        stream[32 + 12 + 3 + 1 + 6 + 22 + 12 + 3 + 1 + 5] ^= 0xff;
        assert_eq!(decode_track(&stream).len(), 9);
        assert_eq!(crc16(0xffff, 0xa1), 0x443b);
    }
}
