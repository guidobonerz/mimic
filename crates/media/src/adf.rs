//! ADF (Amiga Disk File) images and their tracks as Amiga MFM bit streams.
//!
//! An ADF holds the 512-byte sectors of a double-density disk in order:
//! 80 cylinders × 2 heads × 11 sectors (901 120 bytes); track `t` is
//! cylinder `t / 2`, head `t % 2` (head 0 = lower side).
//!
//! **Track format** as the trackdisk.device writes it (Amiga Hardware
//! Reference Manual chapter 8 "Disk"; Laurent Clévy, "The .ADF (Amiga Disk
//! File) format FAQ"): per sector two zero bytes, two sync words $4489
//! (MFM with a missing clock), then as odd/even-split longs (first all
//! odd bits, then all even bits, each with MFM clocks): the info long
//! ($FF, track, sector, sectors until the gap), 16 label bytes, the header
//! checksum (XOR of the encoded info and label longs, masked $55555555),
//! the data checksum (same over the data) and 512 data bytes (odd bits of
//! all 128 longs, then the even bits): 1088 bytes. Eleven sectors follow
//! each other from the index; the rest of the [`TRACK_BYTES`] is gap ($AA,
//! encoded zeros).
//!
//! Tracks are bit streams (MSB first) because the drive writes them from
//! wherever the head is; [`decode_track`] finds sectors at any bit offset.

use emu_core::media::MediaError;

/// Bytes of a double-density image.
pub const ADF_SIZE: usize = 901_120;
pub const TRACKS: usize = 160;
pub const SECTORS: usize = 11;
pub const SECTOR_SIZE: usize = 512;
/// MFM bytes on a track: 2 µs cells at 300 rpm, as commonly used.
pub const TRACK_BYTES: usize = 12_668;
/// MFM bytes of one sector.
pub const SECTOR_MFM: usize = 1088;
pub const SYNC: u16 = 0x4489;

/// Check an image's size.
pub fn check(image: &[u8]) -> Result<(), MediaError> {
    if image.len() == ADF_SIZE {
        Ok(())
    } else {
        Err(MediaError::Format(format!(
            "ADF must be {ADF_SIZE} bytes (double density), got {}",
            image.len()
        )))
    }
}

/// MFM writer: data bits get clock bits (1 between two zeros).
struct Writer {
    out: Vec<u8>,
    last: bool,
}

impl Writer {
    fn raw16(&mut self, w: u16) {
        self.out.extend(w.to_be_bytes());
        self.last = w & 1 != 0;
    }

    /// 16 data bits in the low positions of `d` (mask $5555).
    fn data16(&mut self, d: u16) {
        let d = d & 0x5555;
        let later = d << 1;
        let earlier = (d >> 1) | u16::from(self.last) << 15;
        let clock = !(later | earlier) & 0xaaaa;
        self.raw16(d | clock);
    }

    /// A long's odd or even half (data bits in mask $55555555).
    fn data32(&mut self, d: u32) {
        self.data16((d >> 16) as u16);
        self.data16(d as u16);
    }
}

fn odd(v: u32) -> u32 {
    (v >> 1) & 0x5555_5555
}

fn even(v: u32) -> u32 {
    v & 0x5555_5555
}

fn longs(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Encode track `track` (0–159) of an image.
pub fn encode_track(image: &[u8], track: usize) -> Vec<u8> {
    let mut w = Writer {
        out: Vec::with_capacity(TRACK_BYTES),
        last: false,
    };
    for s in 0..SECTORS {
        let off = (track * SECTORS + s) * SECTOR_SIZE;
        let data = longs(
            image
                .get(off..off + SECTOR_SIZE)
                .unwrap_or(&[0; SECTOR_SIZE]),
        );
        w.data32(0);
        w.raw16(SYNC);
        w.raw16(SYNC);
        let info = 0xff00_0000 | (track as u32) << 16 | (s as u32) << 8 | (SECTORS - s) as u32;
        let label = [0u32; 4];
        let mut hsum = odd(info) ^ even(info);
        for l in label {
            hsum ^= odd(l) ^ even(l);
        }
        let dsum = data.iter().fold(0, |a, &l| a ^ odd(l) ^ even(l));
        w.data32(odd(info));
        w.data32(even(info));
        for l in label {
            w.data32(odd(l));
        }
        for l in label {
            w.data32(even(l));
        }
        for v in [hsum, dsum] {
            w.data32(odd(v));
            w.data32(even(v));
        }
        for &l in &data {
            w.data32(odd(l));
        }
        for &l in &data {
            w.data32(even(l));
        }
    }
    while w.out.len() < TRACK_BYTES {
        w.data16(0);
    }
    w.out.truncate(TRACK_BYTES);
    w.out
}

/// Reads a track bit stream from a bit offset, wrapping around.
struct Reader<'a> {
    bits: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn len(&self) -> usize {
        self.bits.len() * 8
    }

    fn bit(&self, pos: usize) -> u16 {
        let p = pos % self.len();
        u16::from(self.bits[p / 8] >> (7 - p % 8) & 1)
    }

    fn raw16(&mut self) -> u16 {
        let v = (0..16).fold(0, |v, i| v << 1 | self.bit(self.pos + i));
        self.pos += 16;
        v
    }

    /// The data bits of an MFM long (one odd or even half).
    fn half(&mut self) -> u32 {
        let hi = u32::from(self.raw16());
        let lo = u32::from(self.raw16());
        (hi << 16 | lo) & 0x5555_5555
    }

    fn long(&mut self, odd_half: u32) -> u32 {
        let even_half = self.half();
        odd_half << 1 | even_half
    }
}

/// A sector found on a track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sector {
    pub track: u8,
    pub sector: u8,
    pub data: Vec<u8>,
}

/// Find the sectors with valid checksums on a track bit stream.
pub fn decode_track(bits: &[u8]) -> Vec<Sector> {
    let mut found: Vec<Sector> = Vec::new();
    if bits.is_empty() {
        return found;
    }
    let n = bits.len() * 8;
    let r0 = Reader { bits, pos: 0 };
    let mut window: u16 = (0..16).fold(0, |v, i| v << 1 | r0.bit(n - 16 + i));
    for start in 0..n {
        window = window << 1 | r0.bit(start);
        if window != SYNC {
            continue;
        }
        let mut r = Reader {
            bits,
            pos: start + 1,
        };
        // Skip a second sync word.
        let mut first = r.raw16();
        while first == SYNC {
            first = r.raw16();
        }
        let second = u32::from(r.raw16());
        let info_odd = (u32::from(first) << 16 | second) & 0x5555_5555;
        let info = r.long(info_odd);
        let mut label_odd = [0u32; 4];
        for l in &mut label_odd {
            *l = r.half();
        }
        let mut label_even = [0u32; 4];
        for l in &mut label_even {
            *l = r.half();
        }
        let h_odd = r.half();
        let hsum = r.long(h_odd);
        let d_odd = r.half();
        let dsum = r.long(d_odd);
        let mut calc = odd(info) ^ even(info);
        for (o, e) in label_odd.iter().zip(&label_even) {
            calc ^= o ^ e;
        }
        if info >> 24 != 0xff || calc != hsum {
            continue;
        }
        let odds: Vec<u32> = (0..128).map(|_| r.half()).collect();
        let evens: Vec<u32> = (0..128).map(|_| r.half()).collect();
        let csum = odds.iter().chain(&evens).fold(0, |a, &l| a ^ l);
        if csum != dsum {
            continue;
        }
        let data: Vec<u8> = odds
            .iter()
            .zip(&evens)
            .flat_map(|(&o, &e)| (o << 1 | e).to_be_bytes())
            .collect();
        let sector = Sector {
            track: (info >> 16) as u8,
            sector: (info >> 8) as u8,
            data,
        };
        if usize::from(sector.sector) < SECTORS && !found.iter().any(|s| s.sector == sector.sector)
        {
            found.push(sector);
        }
    }
    found
}

/// Store decoded sectors of track `track` into an image. Returns how many
/// sectors were stored (sectors of other tracks are ignored).
pub fn store_track(image: &mut [u8], track: usize, sectors: &[Sector]) -> usize {
    let mut n = 0;
    for s in sectors.iter().filter(|s| usize::from(s.track) == track) {
        let off = (track * SECTORS + usize::from(s.sector)) * SECTOR_SIZE;
        if let Some(dst) = image.get_mut(off..off + SECTOR_SIZE) {
            dst.copy_from_slice(&s.data);
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Vec<u8> {
        (0..ADF_SIZE).map(|i| (i * 7 + i / 512) as u8).collect()
    }

    #[test]
    fn tracks_round_trip_at_any_bit_offset() {
        let img = image();
        for track in [0, 1, 80, 159] {
            let mfm = encode_track(&img, track);
            assert_eq!(mfm.len(), TRACK_BYTES);
            // Starts with $AAAA $AAAA $4489 $4489.
            assert_eq!(&mfm[..8], &[0xaa, 0xaa, 0xaa, 0xaa, 0x44, 0x89, 0x44, 0x89]);
            // Rotate by 5 bits.
            let n = mfm.len() * 8;
            let mut rot = vec![0u8; mfm.len()];
            for i in 0..n {
                let src = (i + 5) % n;
                if mfm[src / 8] >> (7 - src % 8) & 1 != 0 {
                    rot[i / 8] |= 1 << (7 - i % 8);
                }
            }
            let sectors = decode_track(&rot);
            assert_eq!(sectors.len(), SECTORS, "track {track}");
            let mut out = vec![0; ADF_SIZE];
            assert_eq!(store_track(&mut out, track, &sectors), SECTORS);
            let off = track * SECTORS * SECTOR_SIZE;
            assert_eq!(
                out[off..off + SECTORS * SECTOR_SIZE],
                img[off..off + SECTORS * SECTOR_SIZE]
            );
        }
    }

    #[test]
    fn mfm_never_has_two_ones_in_a_row_outside_syncs() {
        let mfm = encode_track(&image(), 3);
        let mut prev = 0;
        for (i, &b) in mfm.iter().enumerate() {
            let v = u16::from(prev) << 8 | u16::from(b);
            assert_eq!(v & (v >> 1) & 0xff, 0, "byte {i}");
            prev = b;
        }
    }

    #[test]
    fn a_damaged_sector_is_skipped() {
        let img = image();
        let mut mfm = encode_track(&img, 0);
        mfm[100] ^= 0x10; // inside sector 0's data
        let sectors = decode_track(&mfm);
        assert_eq!(sectors.len(), SECTORS - 1);
        assert!(sectors.iter().all(|s| s.sector != 0));
    }
}
