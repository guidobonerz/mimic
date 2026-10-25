//! Commodore GCR (1541, 1571) from flux.
//!
//! A 1541 records at four bit rates by speed zone: 16 MHz divided by
//! 13, 14, 15 or 16, then by 4 — bit cells of 3.25, 3.5, 3.75 and 4 µs
//! at 300 rpm for zones 3 (tracks 1–17) down to 0 (tracks 31 and up). A 1
//! bit is a flux transition. The read logic counts ten or more 1 bits as
//! SYNC and starts a new byte at the first 0 after it; this module frames
//! the bits the same way, so [`media::gcr::decode_track`] finds the
//! sectors. Sources: "Inside the 1541" (Immers/Neufeld), chapter on the
//! read/write electronics and GCR; Commodore 1541 service manual (bit
//! rate divider).

use media::gcr::{decode5, decode_track, DecodedSector, TRACK_BYTES};

use super::{decode_bits, Bits, Flux};

/// Bit cell in seconds at 300 rpm for speed zone 0–3.
pub fn cell_seconds(zone: u8) -> f64 {
    f64::from(16 - zone.min(3)) / 4.0 * 1e-6
}

/// Frame bits into bytes like the 1541's read logic: byte boundaries
/// restart at the end of each SYNC. Returns the bytes and the byte
/// position of each index pulse.
pub fn frame_bytes(bits: &Bits) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::with_capacity(bits.bits.len() / 8 + 1);
    let mut index = Vec::with_capacity(bits.index.len());
    let mut next_index = bits.index.iter().peekable();
    let (mut ones, mut byte, mut n) = (0u32, 0u8, 0u8);
    for (i, &b) in bits.bits.iter().enumerate() {
        while next_index.peek().is_some_and(|&&at| at <= i) {
            index.push(out.len());
            next_index.next();
        }
        if b {
            ones += 1;
        } else {
            if ones >= 10 {
                // End of SYNC: this 0 starts a byte.
                byte = 0;
                n = 0;
            }
            ones = 0;
        }
        byte = byte << 1 | u8::from(b);
        n += 1;
        if n == 8 {
            out.push(byte);
            n = 0;
        }
    }
    for _ in next_index {
        index.push(out.len());
    }
    (out, index)
}

/// What one read of a GCR track gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CbmTrack {
    /// Sectors with a good data block.
    pub sectors: Vec<DecodedSector>,
    /// (track, sector) of every header block seen.
    pub headers: Vec<(u8, u8)>,
    /// One revolution of framed GCR bytes (from the first index pulse).
    pub revolution: Vec<u8>,
}

impl CbmTrack {
    /// The track number most headers carry.
    pub fn track_id(&self) -> Option<u8> {
        let mut counts = [0usize; 256];
        for &(t, _) in &self.headers {
            counts[usize::from(t)] += 1;
        }
        let (t, &n) = counts.iter().enumerate().max_by_key(|&(_, n)| *n)?;
        (n > 0).then_some(t as u8)
    }
}

/// The header blocks (track, sector) after every SYNC.
fn headers(bytes: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 1;
    while i + 5 < bytes.len() {
        if bytes[i - 1] == 0xff && bytes[i] != 0xff {
            let group: [u8; 5] = [
                bytes[i],
                bytes[i + 1],
                bytes[i + 2],
                bytes[i + 3],
                bytes[i + 4],
            ];
            if let Some([0x08, _, sector, track]) = decode5(&group) {
                if !out.contains(&(track, sector)) {
                    out.push((track, sector));
                }
            }
        }
        i += 1;
    }
    out
}

/// Decode a read of a GCR track recorded in speed zone `zone`. The drive's
/// speed comes from the index pulses (300 rpm without them), so a 360 rpm
/// drive works too.
pub fn decode(flux: &Flux, zone: u8) -> CbmTrack {
    let rpm = flux.rpm().unwrap_or(300.0);
    let cell = cell_seconds(zone) * f64::from(flux.sample_freq) * 300.0 / rpm;
    let bits = decode_bits(flux, cell);
    let (bytes, index) = frame_bytes(&bits);
    let revolution = match index.as_slice() {
        [a, b, ..] if a < b => bytes[*a..*b].to_vec(),
        [a] => bytes[*a..].to_vec(),
        _ => bytes.clone(),
    };
    let mut revolution = revolution;
    revolution.truncate(media::g64::MAX_TRACK_SIZE);
    if revolution.len() < 400 {
        revolution.resize(TRACK_BYTES[usize::from(zone.min(3))], 0x55);
    }
    CbmTrack {
        sectors: decode_track(&bytes),
        headers: headers(&bytes),
        revolution,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{encode_bits, unpack, Rng};
    use super::*;
    use media::D64;

    #[test]
    fn cells_by_zone() {
        assert!((cell_seconds(3) - 3.25e-6).abs() < 1e-12);
        assert!((cell_seconds(0) - 4e-6).abs() < 1e-12);
    }

    #[test]
    fn framing_restarts_after_sync() {
        // Two bits of junk, a SYNC, then $52 (GCR of $08's first nibbles).
        let mut bits = vec![false, true];
        bits.extend(std::iter::repeat_n(true, 16));
        bits.extend(unpack(&[0x52, 0x54]));
        let (bytes, _) = frame_bytes(&Bits {
            bits,
            index: vec![],
        });
        assert_eq!(&bytes[bytes.len() - 2..], &[0x52, 0x54]);
    }

    #[test]
    fn track_from_flux_at_360_rpm() {
        let mut d = D64::blank(b"FLUX", *b"FX");
        for s in 0..21u8 {
            d.sector_mut(1, s).unwrap().fill(s ^ 0x5a);
        }
        let gcr = media::gcr::encode_track(&d, 1);
        let bits = unpack(&gcr);
        let mut rng = Rng::new(1);
        // 72 MHz, zone 3 at 360 rpm: cells 300/360 as long.
        let cell = 3.25e-6 * 72e6 * 300.0 / 360.0;
        let flux = encode_bits(&bits, cell, 72_000_000, 2, 0.1, 5000, &mut rng);
        let t = decode(&flux, 3);
        assert_eq!(t.sectors.len(), 21);
        assert!(t.sectors.iter().all(|s| s.data[0] == s.sector ^ 0x5a));
        assert_eq!(t.track_id(), Some(1));
        assert_eq!(t.headers.len(), 21);
        assert!((t.revolution.len() as i64 - 7692).abs() < 4);
    }
}
