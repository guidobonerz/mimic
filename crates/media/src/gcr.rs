//! Group Code Recording as written by the 1541: every 4 bits become 5, so
//! no more than two 0 bits follow each other and ten 1 bits (a SYNC mark)
//! never occur inside data.
//!
//! A sector on disk: SYNC (5 × `$FF`), header block `$08, checksum, sector,
//! track, ID2, ID1, $0F, $0F` (GCR: 10 bytes), gap (9 × `$55`), SYNC, data
//! block `$07, 256 bytes, checksum, $00, $00` (GCR: 325 bytes), tail gap.
//! Track lengths follow the four speed zones (7692/7142/6666/6250 bytes per
//! revolution at 300 rpm). Source: "Inside the 1541", chapter on GCR.

use crate::d64::{sectors_per_track, speed_zone, D64};

const GCR: [u8; 16] = [
    0b01010, 0b01011, 0b10010, 0b10011, 0b01110, 0b01111, 0b10110, 0b10111, 0b01001, 0b11001,
    0b11010, 0b11011, 0b01101, 0b11101, 0b11110, 0b10101,
];

/// Bytes per track revolution for speed zone 0..=3.
pub const TRACK_BYTES: [usize; 4] = [6250, 6666, 7142, 7692];

/// Encode 4 bytes into 5 GCR bytes.
fn encode4(input: [u8; 4], out: &mut Vec<u8>) {
    let mut bits: u64 = 0;
    for b in input {
        bits = bits << 10
            | u64::from(GCR[usize::from(b >> 4)]) << 5
            | u64::from(GCR[usize::from(b & 15)]);
    }
    for i in (0..5).rev() {
        out.push((bits >> (i * 8)) as u8);
    }
}

fn encode(data: &[u8], out: &mut Vec<u8>) {
    for c in data.chunks(4) {
        let mut q = [0u8; 4];
        q[..c.len()].copy_from_slice(c);
        encode4(q, out);
    }
}

/// Decode 5 GCR bytes into 4 bytes; `None` on an invalid code.
pub fn decode5(input: &[u8; 5]) -> Option<[u8; 4]> {
    let bits = input.iter().fold(0u64, |a, &b| a << 8 | u64::from(b));
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let shift = (3 - i) * 10;
        let hi = GCR
            .iter()
            .position(|&g| u64::from(g) == (bits >> (shift + 5)) & 31)?;
        let lo = GCR
            .iter()
            .position(|&g| u64::from(g) == (bits >> shift) & 31)?;
        *o = (hi << 4 | lo) as u8;
    }
    Some(out)
}

/// The GCR bit stream of one track as bytes (MSB first), one revolution.
pub fn encode_track(disk: &D64, track: u8) -> Vec<u8> {
    let len = TRACK_BYTES[usize::from(speed_zone(track))];
    let n = sectors_per_track(track);
    let [id1, id2] = disk.id();
    let mut out = Vec::with_capacity(len);
    // Sector size on disk: 2 × SYNC, GCR header, header gap, GCR data block.
    const SECTOR_BYTES: usize = 5 + 10 + 9 + 5 + 325;
    let tail_gap = (len - usize::from(n) * SECTOR_BYTES) / usize::from(n);
    for sector in 0..n {
        let error = disk.error(track, sector);
        let data = disk.sector(track, sector).unwrap_or(&[0; 256]);
        let sync = if error == 3 { 0x55 } else { 0xff };

        // Header (error 2 = header block not found, 9 = bad checksum,
        // 0x0B = ID mismatch).
        out.extend_from_slice(&[sync; 5]);
        let mut checksum = sector ^ track ^ id2 ^ id1;
        if error == 9 {
            checksum ^= 0xff;
        }
        let (h1, h2) = if error == 0x0b {
            (id1 ^ 0xff, id2)
        } else {
            (id1, id2)
        };
        let header = [
            if error == 2 { 0x00 } else { 0x08 },
            checksum,
            sector,
            track,
            h2,
            h1,
            0x0f,
            0x0f,
        ];
        encode(&header, &mut out);
        out.extend_from_slice(&[0x55; 9]);

        // Data (error 4 = data block not found, 5 = bad checksum).
        out.extend_from_slice(&[sync; 5]);
        let mut block = Vec::with_capacity(260);
        block.push(if error == 4 { 0x00 } else { 0x07 });
        block.extend_from_slice(data);
        let mut sum = data.iter().fold(0u8, |a, &b| a ^ b);
        if error == 5 {
            sum ^= 0xff;
        }
        block.extend_from_slice(&[sum, 0, 0]);
        encode(&block, &mut out);

        out.extend(std::iter::repeat_n(0x55, tail_gap));
    }
    out.resize(len, 0x55);
    out
}

/// One sector read back from a GCR track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedSector {
    pub track: u8,
    pub sector: u8,
    pub data: [u8; 256],
}

/// The sectors on one revolution of a GCR track (byte-aligned, as the 1541
/// writes it): each SYNC followed by a header block (`$08`) whose next SYNC
/// starts a data block (`$07`) with a valid checksum. The track is circular;
/// a sector that wraps around its end is found too. Sectors with a damaged
/// header or data block are left out.
pub fn decode_track(track: &[u8]) -> Vec<DecodedSector> {
    let n = track.len();
    if n < 400 {
        return Vec::new();
    }
    let at = |i: usize| track[i % n];
    let group = |start: usize| -> [u8; 5] { std::array::from_fn(|k| at(start + k)) };
    // The first byte after a SYNC (two or more $FF) at or after `from`,
    // searching at most `within` bytes.
    let after_sync = |from: usize, within: usize| -> Option<usize> {
        let mut i = from;
        while i < from + within {
            if at(i) == 0xff && at(i + 1) == 0xff {
                let mut j = i;
                while at(j) == 0xff && j < i + n {
                    j += 1;
                }
                return Some(j);
            }
            i += 1;
        }
        None
    };
    let mut out: Vec<DecodedSector> = Vec::new();
    let mut i = 0;
    while let Some(h) = after_sync(i, n.saturating_sub(i)) {
        i = h + 1;
        let Some(header) = decode5(&group(h)) else {
            continue;
        };
        if header[0] != 0x08 {
            continue;
        }
        let (sector, track_no) = (header[2], header[3]);
        // The data block's SYNC follows within the header gap.
        let Some(d) = after_sync(h + 10, 40) else {
            continue;
        };
        let mut block = Vec::with_capacity(260);
        for g in 0..65 {
            match decode5(&group(d + g * 5)) {
                Some(q) => block.extend_from_slice(&q),
                None => break,
            }
        }
        if block.len() < 258 || block[0] != 0x07 {
            continue;
        }
        let data: [u8; 256] = block[1..257].try_into().unwrap_or([0; 256]);
        if data.iter().fold(0u8, |a, &b| a ^ b) != block[257] {
            continue;
        }
        if !out.iter().any(|s| s.sector == sector) {
            out.push(DecodedSector {
                track: track_no,
                sector,
                data,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let mut out = Vec::new();
        encode4([0x08, 0x1f, 0xa5, 0xff], &mut out);
        assert_eq!(
            decode5(&out.clone().try_into().unwrap()),
            Some([0x08, 0x1f, 0xa5, 0xff])
        );
        // Never more than two zero bits in a row, no ten ones.
        let mut all = Vec::new();
        encode(&(0..=255).collect::<Vec<u8>>(), &mut all);
        let bits: String = all.iter().map(|b| format!("{b:08b}")).collect();
        assert!(!bits.contains("000"));
        assert!(!bits.contains(&"1".repeat(10)));
    }

    #[test]
    fn track_has_zone_length_and_all_headers() {
        let d = D64::blank(b"X", *b"AB");
        for (track, len) in [(1, 7692), (18, 7142), (25, 6666), (35, 6250)] {
            let t = encode_track(&d, track);
            assert_eq!(t.len(), len);
            // Find each header: SYNC then GCR($08 checksum sector track).
            let mut found = 0;
            for i in 0..t.len() - 10 {
                if t[i..i + 5] == [0xff; 5] && t[i + 5] != 0xff {
                    let g: [u8; 5] = t[i + 5..i + 10].try_into().unwrap();
                    if let Some([0x08, _, s, tr]) = decode5(&g) {
                        assert_eq!(tr, track);
                        assert!(s < sectors_per_track(track));
                        found += 1;
                    }
                }
            }
            assert_eq!(found, usize::from(sectors_per_track(track)));
        }
    }

    #[test]
    fn whole_tracks_decode_back() {
        let mut d = D64::blank(b"X", *b"AB");
        for s in 0..21 {
            d.sector_mut(1, s).unwrap()[0] = s * 3;
            d.sector_mut(1, s).unwrap()[255] = 0xa5;
        }
        let mut t = encode_track(&d, 1);
        let sectors = decode_track(&t);
        assert_eq!(sectors.len(), 21);
        for s in &sectors {
            assert_eq!(s.track, 1);
            assert_eq!(s.data[0], s.sector * 3);
            assert_eq!(s.data[255], 0xa5);
        }
        // Rotated so a sector wraps around the end: still all found.
        t.rotate_left(100);
        assert_eq!(decode_track(&t).len(), 21);
        // A damaged data block drops that sector only (sector 0 now starts
        // 100 bytes before the end).
        let in_data = t.len() - 100 + 5 + 10 + 9 + 5 + 20;
        t[in_data] ^= 0xff;
        assert_eq!(decode_track(&t).len(), 20);
    }

    #[test]
    fn data_block_decodes_back() {
        let mut d = D64::blank(b"X", *b"AB");
        d.sector_mut(1, 0).unwrap()[..4].copy_from_slice(b"DATA");
        let t = encode_track(&d, 1);
        // Second SYNC of sector 0 starts the data block.
        let start = 5 + 10 + 9 + 5;
        let g: [u8; 5] = t[start..start + 5].try_into().unwrap();
        assert_eq!(decode5(&g), Some([0x07, b'D', b'A', b'T']));
    }
}
