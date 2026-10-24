//! Macintosh 3.5" GCR disks (400K single-sided, 800K double-sided): raw
//! images and DiskCopy 4.2 images, encoded into the bit streams the Sony
//! drive's heads read, and decoded back.
//!
//! **Geometry.** 80 tracks a side in five zones of 16 tracks with 12,
//! 11, 10, 9 and 8 sectors (800 sectors a side); the drive turns faster
//! the further in the head is, so every track is written with 2 µs bit
//! cells: [`TRACK_BITS`] per zone (about 402, 438, 482, 536 and 603 rpm).
//! Logical blocks run sector by sector, then side, then track (block 0
//! is track 0 side 0 sector 0, a double-sided disk's block 12 track 0 side
//! 1 sector 0). The sectors of a track are laid out with 2:1 interleave.
//!
//! **Sector** (bytes are 8-bit GCR disk bytes with bit 7 set; "sync" is a
//! self-sync byte: `$FF` followed by two zero bits):
//!
//! | Field          | Contents                                              |
//! | -------------- | ----------------------------------------------------- |
//! | gap            | 36 sync bytes (5 before the data field)               |
//! | address        | `D5 AA 96`, track & $3F, sector, side·$20 + track ≥ 64, format ($02 one side, $22 two), checksum = XOR of the four, `DE AA`, pad `FF` |
//! | data           | `D5 AA AD`, sector, 699 bytes for the 12 tag and 512 data bytes, 4 checksum bytes, `DE AA`, pad `FF` |
//!
//! Every value is a six-bit one written through the Apple II's 6-and-2
//! table ([`a2disk::read_nibble`]). The 524 bytes go in 175 groups of three
//! (the last has two): three rolling checksums c1, c2, c3 — before each
//! group c1 rotates left by one bit, then the first byte adds into c3
//! (plus the bit rotated out), the second into c2 (plus c3's carry), the
//! third into c1 (plus c2's carry); the bytes are written XOR c1, c3 and
//! c2 respectively, and the four values of a group are the bytes' top two bits packed together, then
//! their low six bits. The checksum follows as one more group (c3, c2,
//! c1). Sources: CiderPress II, "Nibble notes" (format byte, address and
//! data field layout, gaps, interleave tables, ciderpress2.com/formatdoc);
//! the decoder of Clock Signal (MIT, commit 5a9f3cf on
//! git.applefritter.com/6502/CLK) for the checksum byte order; Thomas W.,
//! "Emulating the Macintosh floppy drive" (thomasw.dev) for the zones.
//! The XOR pairing of the bytes with c1/c3/c2 is this project's reading
//! of those descriptions; it is checked when a real ROM reads a disk.
//!
//! **DiskCopy 4.2** (big-endian; CiderPress II, "DiskCopy notes"): $00
//! the image name (Pascal string, 64 bytes), $40 data size, $44 tag size,
//! $48 data checksum, $4C tag checksum, $50 disk format (0 = 400K, 1 =
//! 800K), $51 format byte ($12 400K, $22 800K), $52 $0100, $54 the data
//! blocks, then the tags (12 bytes a block). A checksum adds each
//! big-endian word to a 32-bit sum and rotates it right by one bit; the
//! tag checksum leaves out the first 12 bytes.

use crate::a2disk;

/// Bits on a track per zone (16 tracks each).
pub const TRACK_BITS: [u32; 5] = [74_640, 68_240, 62_200, 55_980, 49_760];
pub const TRACKS: usize = 80;
pub const BLOCK: usize = 512;
pub const TAG: usize = 12;
pub const SIDE_BLOCKS: usize = 800;
/// Raw image sizes.
pub const BYTES_400K: usize = SIDE_BLOCKS * BLOCK;
pub const BYTES_800K: usize = 2 * SIDE_BLOCKS * BLOCK;
const DC42_HEADER: usize = 0x54;

pub fn zone(track: usize) -> usize {
    (track / 16).min(4)
}

pub fn sectors(track: usize) -> usize {
    12 - zone(track)
}

/// The first block of a track side.
pub fn first_block(track: usize, side: usize, sides: usize) -> usize {
    let before: usize = (0..track).map(sectors).sum();
    before * sides + side * sectors(track)
}

/// Physical sector order of a track: 2:1 interleave.
pub fn interleave(n: usize) -> Vec<usize> {
    let mut table = vec![usize::MAX; n];
    let mut at = 0;
    for i in 0..n {
        while table[at] != usize::MAX {
            at = (at + 1) % n;
        }
        table[at] = i;
        at = (at + 2) % n;
    }
    table
}

/// A disk as bit streams: track `t`, side `s` is `tracks[t * sides + s]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacDisk {
    pub sides: usize,
    pub tracks: Vec<Vec<u8>>,
    pub bits: Vec<u32>,
}

/// What an image file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Raw,
    DiskCopy42,
}

/// Blocks and tags of an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub kind: ImageKind,
    pub sides: usize,
    pub data: Vec<u8>,
    /// 12 bytes a block, zeros when the image has none.
    pub tags: Vec<u8>,
    /// DiskCopy name.
    pub name: Vec<u8>,
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// The DiskCopy 4.2 checksum.
pub fn dc42_checksum(data: &[u8]) -> u32 {
    data.chunks(2).fold(0u32, |sum, w| {
        let word = u32::from(w[0]) << 8 | u32::from(*w.get(1).unwrap_or(&0));
        sum.wrapping_add(word).rotate_right(1)
    })
}

impl Image {
    pub fn parse(file: &[u8]) -> Result<Self, String> {
        match file.len() {
            BYTES_400K | BYTES_800K => {
                let sides = file.len() / BYTES_400K;
                return Ok(Self {
                    kind: ImageKind::Raw,
                    sides,
                    data: file.to_vec(),
                    tags: vec![0; sides * SIDE_BLOCKS * TAG],
                    name: Vec::new(),
                });
            }
            _ => {}
        }
        if file.len() < DC42_HEADER || file[0x52..0x54] != [0x01, 0x00] {
            return Err(format!(
                "not a Mac 400K/800K image ({} bytes; raw {BYTES_400K}/{BYTES_800K} or DiskCopy 4.2)",
                file.len()
            ));
        }
        let data_len = be32(file, 0x40) as usize;
        let tag_len = be32(file, 0x44) as usize;
        let sides = match data_len {
            BYTES_400K => 1,
            BYTES_800K => 2,
            n => return Err(format!("DiskCopy image of {n} bytes is not 400K or 800K")),
        };
        if file.len() < DC42_HEADER + data_len + tag_len {
            return Err("DiskCopy image is shorter than its header says".into());
        }
        let data = file[DC42_HEADER..DC42_HEADER + data_len].to_vec();
        let mut tags = vec![0; sides * SIDE_BLOCKS * TAG];
        let stored = &file[DC42_HEADER + data_len..DC42_HEADER + data_len + tag_len];
        let n = stored.len().min(tags.len());
        tags[..n].copy_from_slice(&stored[..n]);
        let name_len = usize::from(file[0]).min(63);
        Ok(Self {
            kind: ImageKind::DiskCopy42,
            sides,
            data,
            tags,
            name: file[1..1 + name_len].to_vec(),
        })
    }

    /// The image file again, in its own format.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self.kind {
            ImageKind::Raw => self.data.clone(),
            ImageKind::DiskCopy42 => {
                let has_tags = self.tags.iter().any(|&b| b != 0);
                let tags: &[u8] = if has_tags { &self.tags } else { &[] };
                let mut v = vec![0u8; DC42_HEADER];
                let n = self.name.len().min(63);
                v[0] = n as u8;
                v[1..1 + n].copy_from_slice(&self.name[..n]);
                v[0x40..0x44].copy_from_slice(&(self.data.len() as u32).to_be_bytes());
                v[0x44..0x48].copy_from_slice(&(tags.len() as u32).to_be_bytes());
                v[0x48..0x4c].copy_from_slice(&dc42_checksum(&self.data).to_be_bytes());
                let tag_sum = if tags.len() > TAG {
                    dc42_checksum(&tags[TAG..])
                } else {
                    0
                };
                v[0x4c..0x50].copy_from_slice(&tag_sum.to_be_bytes());
                v[0x50] = (self.sides - 1) as u8;
                v[0x51] = if self.sides == 2 { 0x22 } else { 0x12 };
                v[0x52] = 0x01;
                v.extend_from_slice(&self.data);
                v.extend_from_slice(tags);
                v
            }
        }
    }

    pub fn blocks(&self) -> usize {
        self.sides * SIDE_BLOCKS
    }

    pub fn encode(&self) -> MacDisk {
        let sides = self.sides;
        let mut tracks = Vec::with_capacity(TRACKS * sides);
        let mut bits = Vec::with_capacity(TRACKS * sides);
        for t in 0..TRACKS {
            for s in 0..sides {
                let first = first_block(t, s, sides);
                let secs: Vec<[u8; 524]> = (0..sectors(t))
                    .map(|n| {
                        let b = first + n;
                        let mut sec = [0u8; 524];
                        sec[..TAG].copy_from_slice(&self.tags[b * TAG..(b + 1) * TAG]);
                        sec[TAG..].copy_from_slice(&self.data[b * BLOCK..(b + 1) * BLOCK]);
                        sec
                    })
                    .collect();
                let (bytes, n) = encode_track(t, s, sides, &secs);
                tracks.push(bytes);
                bits.push(n);
            }
        }
        MacDisk {
            sides,
            tracks,
            bits,
        }
    }

    /// Store the sectors read from `disk` into the image. Returns the
    /// number of sectors that could not be decoded (they keep their old
    /// contents).
    pub fn decode(&mut self, disk: &MacDisk) -> usize {
        let mut bad = 0;
        for t in 0..TRACKS {
            for s in 0..self.sides {
                let i = t * self.sides + s;
                let (Some(track), Some(&n)) = (disk.tracks.get(i), disk.bits.get(i)) else {
                    bad += sectors(t);
                    continue;
                };
                let found = decode_track(track, n, t, s);
                let first = first_block(t, s, self.sides);
                for (k, sec) in found.iter().enumerate() {
                    match sec {
                        Some(sec) => {
                            let b = first + k;
                            self.tags[b * TAG..(b + 1) * TAG].copy_from_slice(&sec[..TAG]);
                            self.data[b * BLOCK..(b + 1) * BLOCK].copy_from_slice(&sec[TAG..]);
                        }
                        None => bad += 1,
                    }
                }
            }
        }
        bad
    }
}

/// Bit writer for a track.
struct Bits {
    out: Vec<u8>,
    n: u32,
}

impl Bits {
    fn push(&mut self, bit: bool) {
        let i = self.n as usize;
        if i / 8 >= self.out.len() {
            self.out.push(0);
        }
        if bit {
            self.out[i / 8] |= 0x80 >> (i % 8);
        }
        self.n += 1;
    }

    fn byte(&mut self, b: u8) {
        for k in (0..8).rev() {
            self.push(b >> k & 1 != 0);
        }
    }

    fn sync(&mut self, count: usize) {
        for _ in 0..count {
            self.byte(0xff);
            self.push(false);
            self.push(false);
        }
    }

    fn nibble(&mut self, v: u8) {
        self.byte(a2disk::write_table()[usize::from(v & 0x3f)]);
    }
}

/// The 699 + 4 six-bit values of a data field.
pub fn encode_data(src: &[u8; 524]) -> Vec<u8> {
    let (mut c1, mut c2, mut c3) = (0u32, 0u32, 0u32);
    let mut out = Vec::with_capacity(703);
    for g in 0..175 {
        let a = u32::from(src[3 * g]);
        let b = u32::from(src[3 * g + 1]);
        let last = g == 174;
        c1 = (c1 & 0xff) << 1;
        if c1 & 0x100 != 0 {
            c1 |= 1;
        }
        c3 += a;
        if c1 > 0xff {
            c3 += 1;
            c1 &= 0xff;
        }
        let ea = a ^ c1;
        c2 += b;
        if c3 > 0xff {
            c2 += 1;
            c3 &= 0xff;
        }
        let eb = b ^ c3;
        let ec = if last {
            0
        } else {
            let c = u32::from(src[3 * g + 2]);
            c1 += c;
            if c2 > 0xff {
                c1 += 1;
                c2 &= 0xff;
            }
            c ^ c2
        };
        out.push((((ea >> 2) & 0x30) | ((eb >> 4) & 0x0c) | ((ec >> 6) & 0x03)) as u8);
        out.push((ea & 0x3f) as u8);
        out.push((eb & 0x3f) as u8);
        if !last {
            out.push((ec & 0x3f) as u8);
        }
    }
    let (c1, c2, c3) = (c1 & 0xff, c2 & 0xff, c3 & 0xff);
    out.push((((c3 >> 2) & 0x30) | ((c2 >> 4) & 0x0c) | ((c1 >> 6) & 0x03)) as u8);
    out.push((c3 & 0x3f) as u8);
    out.push((c2 & 0x3f) as u8);
    out.push((c1 & 0x3f) as u8);
    out
}

/// The 524 bytes of a data field from its 703 six-bit values; `None`
/// with a bad checksum.
pub fn decode_data(v: &[u8]) -> Option<[u8; 524]> {
    if v.len() < 703 {
        return None;
    }
    let mut out = [0u8; 524];
    let (mut c1, mut c2, mut c3) = (0u32, 0u32, 0u32);
    let mut i = 0;
    for g in 0..175 {
        let last = g == 174;
        let top = u32::from(v[i]);
        let ea = u32::from(v[i + 1]) | (top << 2 & 0xc0);
        let eb = u32::from(v[i + 2]) | (top << 4 & 0xc0);
        let ec = if last {
            0
        } else {
            u32::from(v[i + 3]) | (top << 6 & 0xc0)
        };
        i += if last { 3 } else { 4 };
        c1 = (c1 & 0xff) << 1;
        if c1 & 0x100 != 0 {
            c1 |= 1;
        }
        let a = ea ^ (c1 & 0xff);
        c3 += a;
        if c1 > 0xff {
            c3 += 1;
            c1 &= 0xff;
        }
        let b = eb ^ (c3 & 0xff);
        c2 += b;
        if c3 > 0xff {
            c2 += 1;
            c3 &= 0xff;
        }
        out[3 * g] = a as u8;
        out[3 * g + 1] = b as u8;
        if !last {
            let c = ec ^ (c2 & 0xff);
            c1 += c;
            if c2 > 0xff {
                c1 += 1;
                c2 &= 0xff;
            }
            out[3 * g + 2] = c as u8;
        }
    }
    let top = u32::from(v[i]);
    let s3 = u32::from(v[i + 1]) | (top << 2 & 0xc0);
    let s2 = u32::from(v[i + 2]) | (top << 4 & 0xc0);
    let s1 = u32::from(v[i + 3]) | (top << 6 & 0xc0);
    ((c1 & 0xff, c2 & 0xff, c3 & 0xff) == (s1, s2, s3)).then_some(out)
}

/// One track side as a bit stream and its length in bits.
pub fn encode_track(track: usize, side: usize, sides: usize, secs: &[[u8; 524]]) -> (Vec<u8>, u32) {
    let total = TRACK_BITS[zone(track)];
    let mut w = Bits {
        out: Vec::with_capacity(total as usize / 8 + 1),
        n: 0,
    };
    let format = if sides == 2 { 0x22 } else { 0x02 };
    let side_byte = (side as u8) << 5 | (track >> 6) as u8;
    for &s in &interleave(secs.len()) {
        w.sync(36);
        for b in [0xd5, 0xaa, 0x96] {
            w.byte(b);
        }
        let hdr = [(track & 0x3f) as u8, s as u8, side_byte, format];
        for v in hdr {
            w.nibble(v);
        }
        w.nibble(hdr.iter().fold(0, |x, v| x ^ v) & 0x3f);
        for b in [0xde, 0xaa, 0xff] {
            w.byte(b);
        }
        w.sync(5);
        for b in [0xd5, 0xaa, 0xad] {
            w.byte(b);
        }
        w.nibble(s as u8);
        for v in encode_data(&secs[s]) {
            w.nibble(v);
        }
        for b in [0xde, 0xaa, 0xff] {
            w.byte(b);
        }
    }
    // The rest of the turn: self-sync, then single one bits.
    while w.n + 10 <= total {
        w.sync(1);
    }
    while w.n < total {
        w.push(true);
    }
    w.out.truncate(total.div_ceil(8) as usize);
    (w.out, total)
}

/// The disk bytes of a track as the IWM's latch frames them, starting at
/// bit 0 and going round twice (a field across the end is read whole).
fn latch_bytes(track: &[u8], bits: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut latch = 0u8;
    for i in 0..2 * bits {
        let j = i % bits;
        latch = latch << 1 | (track[(j / 8) as usize] >> (7 - j % 8) & 1);
        if latch & 0x80 != 0 {
            out.push(latch);
            latch = 0;
        }
    }
    out
}

/// The sectors found on a track side (by sector number).
pub fn decode_track(track: &[u8], bits: u32, t: usize, side: usize) -> Vec<Option<[u8; 524]>> {
    let n = sectors(t);
    let mut found = vec![None; n];
    if bits == 0 || track.len() * 8 < bits as usize {
        return found;
    }
    let bytes = latch_bytes(track, bits);
    let six = |b: u8| a2disk::read_nibble(b);
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i..i + 3] != [0xd5, 0xaa, 0x96] {
            i += 1;
            continue;
        }
        i += 3;
        let Some(hdr) = bytes
            .get(i..i + 5)
            .and_then(|h| h.iter().map(|&b| six(b)).collect::<Option<Vec<u8>>>())
        else {
            continue;
        };
        if hdr[..4].iter().fold(0, |x, v| x ^ v) != hdr[4] {
            continue;
        }
        let trk = usize::from(hdr[0]) | usize::from(hdr[2] & 0x1f) << 6;
        let sec = usize::from(hdr[1]);
        if trk != t || usize::from(hdr[2] >> 5 & 1) != side || sec >= n {
            continue;
        }
        // The data field follows within a few dozen bytes.
        let Some(at) = (i + 5..(i + 60).min(bytes.len().saturating_sub(3)))
            .find(|&j| bytes[j..j + 3] == [0xd5, 0xaa, 0xad])
        else {
            continue;
        };
        let body = at + 3;
        let vals: Option<Vec<u8>> = bytes
            .get(body..body + 704)
            .and_then(|d| d.iter().map(|&b| six(b)).collect());
        if let Some(vals) = vals {
            if usize::from(vals[0]) == sec && found[sec].is_none() {
                found[sec] = decode_data(&vals[1..]);
            }
        }
        i = body;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(sides: usize, kind: ImageKind) -> Image {
        let blocks = sides * SIDE_BLOCKS;
        Image {
            kind,
            sides,
            data: (0..blocks * BLOCK)
                .map(|i| (i * 7 + i / 512) as u8)
                .collect(),
            tags: (0..blocks * TAG).map(|i| (i * 3) as u8).collect(),
            name: b"Test".to_vec(),
        }
    }

    #[test]
    fn data_field_round_trip_and_checksum() {
        let mut sec = [0u8; 524];
        for (i, b) in sec.iter_mut().enumerate() {
            *b = (i * 31 + 5) as u8;
        }
        let v = encode_data(&sec);
        assert_eq!(v.len(), 703);
        assert!(v.iter().all(|&x| x < 64));
        assert_eq!(decode_data(&v), Some(sec));
        let mut bad = v.clone();
        bad[100] ^= 1;
        assert_eq!(decode_data(&bad), None);
        // All zeros encode to all zeros: the checksums stay 0.
        assert!(encode_data(&[0; 524]).iter().all(|&x| x == 0));
    }

    #[test]
    fn geometry() {
        assert_eq!((0..TRACKS).map(sectors).sum::<usize>(), SIDE_BLOCKS);
        assert_eq!(first_block(1, 0, 2), 24);
        assert_eq!(first_block(0, 1, 2), 12);
        assert_eq!(first_block(16, 0, 1), 192);
        assert_eq!(interleave(12), [0, 6, 1, 7, 2, 8, 3, 9, 4, 10, 5, 11]);
        assert_eq!(interleave(8), [0, 4, 1, 5, 2, 6, 3, 7]);
    }

    #[test]
    fn disks_round_trip_through_tracks() {
        for sides in [1, 2] {
            let img = image(sides, ImageKind::Raw);
            let disk = img.encode();
            assert_eq!(disk.tracks.len(), TRACKS * sides);
            for (t, &n) in disk.bits.iter().enumerate() {
                assert_eq!(n, TRACK_BITS[zone(t / sides)]);
            }
            let mut back = image(sides, ImageKind::Raw);
            back.data.fill(0);
            back.tags.fill(0);
            assert_eq!(back.decode(&disk), 0);
            assert_eq!(back, img);
        }
    }

    #[test]
    fn diskcopy_round_trip() {
        let img = image(2, ImageKind::DiskCopy42);
        let file = img.to_bytes();
        assert_eq!(file.len(), DC42_HEADER + BYTES_800K + 2 * SIDE_BLOCKS * TAG);
        assert_eq!(file[0x51], 0x22);
        assert_eq!(Image::parse(&file).unwrap(), img);
        let raw = image(1, ImageKind::Raw);
        assert_eq!(Image::parse(&raw.to_bytes()).unwrap().sides, 1);
        assert!(Image::parse(&[0; 1000]).is_err());
        // The checksum: words added, rotated right.
        assert_eq!(dc42_checksum(&[0x00, 0x02]), 1);
        assert_eq!(dc42_checksum(&[0x00, 0x01]), 0x8000_0000);
    }
}
