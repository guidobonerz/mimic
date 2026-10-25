//! G64: a 1541 disk as GCR bit streams, one per half track, as the drive
//! head sees them (for disks a D64 cannot hold: copy protection, odd
//! sector layouts, damaged sectors).
//!
//! Layout (all numbers little-endian): signature `GCR-1541`, version byte
//! (0), number of half tracks (84 for 42 tracks), the largest track size
//! as a 16-bit word (7928 in images of real disks), then one 32-bit file
//! offset per half track (0 = no data; entry 0 is track 1, entry 1 track
//! 1.5 …), then one 32-bit speed entry per half track (0–3, the speed zone
//! of `d64::speed_zone`; larger values point at per-byte speed tables,
//! which this module does not write and reads as zone 0). Each track's
//! data is a 16-bit length followed by that many GCR bytes, the block
//! padded to the largest track size.
//! Source: Peter Schepers, "G64 (Raw GCR binary representation of 1541
//! disks)", G64.TXT; the VICE manual's chapter on emulator file formats.

use emu_core::media::MediaError;

/// The signature at the start of every G64.
pub const SIGNATURE: &[u8; 8] = b"GCR-1541";
/// Half tracks in a standard image (tracks 1–42).
pub const HALF_TRACKS: usize = 84;
/// Track block size used by images of real disks.
pub const MAX_TRACK_SIZE: usize = 7928;

/// One half track's bit stream: one revolution of GCR bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct G64Track {
    pub data: Vec<u8>,
    /// Speed zone 0–3 (3 = tracks 1–17).
    pub speed: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct G64 {
    /// Index 0 = track 1, 1 = track 1.5, 2 = track 2 …
    pub half_tracks: Vec<Option<G64Track>>,
}

impl G64 {
    /// An image with [`HALF_TRACKS`] empty half tracks.
    pub fn new() -> Self {
        Self {
            half_tracks: vec![None; HALF_TRACKS],
        }
    }

    /// Set full track `track` (1-based).
    pub fn set_track(&mut self, track: u8, data: Vec<u8>, speed: u8) {
        let i = usize::from(track.max(1) - 1) * 2;
        if self.half_tracks.len() <= i {
            self.half_tracks.resize(i + 1, None);
        }
        self.half_tracks[i] = Some(G64Track { data, speed });
    }

    /// Full track `track` (1-based).
    pub fn track(&self, track: u8) -> Option<&G64Track> {
        let i = usize::from(track.checked_sub(1)?) * 2;
        self.half_tracks.get(i)?.as_ref()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let n = self.half_tracks.len().min(255);
        let max = self
            .half_tracks
            .iter()
            .flatten()
            .map(|t| t.data.len())
            .max()
            .unwrap_or(0)
            .clamp(MAX_TRACK_SIZE, usize::from(u16::MAX));
        let mut out = Vec::new();
        out.extend_from_slice(SIGNATURE);
        out.push(0);
        out.push(n as u8);
        out.extend_from_slice(&(max as u16).to_le_bytes());
        let header = out.len() + n * 8;
        let mut offset = header;
        for t in &self.half_tracks[..n] {
            let v = if t.is_some() {
                let o = offset;
                offset += 2 + max;
                o as u32
            } else {
                0
            };
            out.extend_from_slice(&v.to_le_bytes());
        }
        for t in &self.half_tracks[..n] {
            let speed = t.as_ref().map_or(0, |t| u32::from(t.speed & 3));
            out.extend_from_slice(&speed.to_le_bytes());
        }
        for t in self.half_tracks[..n].iter().flatten() {
            let len = t.data.len().min(max);
            out.extend_from_slice(&(len as u16).to_le_bytes());
            out.extend_from_slice(&t.data[..len]);
            out.resize(out.len() + max - len, 0);
        }
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let bad = |m: &str| MediaError::Format(format!("G64: {m}"));
        if bytes.len() < 12 || &bytes[..8] != SIGNATURE {
            return Err(bad("no GCR-1541 signature"));
        }
        let n = usize::from(bytes[9]);
        let u32_at = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
        };
        let mut half_tracks = Vec::with_capacity(n);
        for i in 0..n {
            let offset = u32_at(12 + i * 4).ok_or_else(|| bad("truncated table"))? as usize;
            let speed = u32_at(12 + n * 4 + i * 4).ok_or_else(|| bad("truncated table"))?;
            if offset == 0 {
                half_tracks.push(None);
                continue;
            }
            let len = bytes
                .get(offset..offset + 2)
                .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
                .ok_or_else(|| bad("track offset beyond the file"))?;
            let data = bytes
                .get(offset + 2..offset + 2 + len)
                .ok_or_else(|| bad("track data beyond the file"))?
                .to_vec();
            half_tracks.push(Some(G64Track {
                data,
                speed: if speed < 4 { speed as u8 } else { 0 },
            }));
        }
        Ok(Self { half_tracks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_layout() {
        let mut g = G64::new();
        g.set_track(1, vec![0x55; 7692], 3);
        g.set_track(35, vec![0xff; 6250], 0);
        let bytes = g.to_bytes();
        assert_eq!(&bytes[..8], SIGNATURE);
        assert_eq!(bytes[9], 84);
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 7928);
        // The first track block follows the two tables.
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 0x2ac);
        assert_eq!(bytes.len(), 0x2ac + 2 * (2 + 7928));
        let back = G64::parse(&bytes).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.track(35).unwrap().speed, 0);
        assert!(back.track(2).is_none());
        assert!(G64::parse(b"GCR-1541").is_err());
        assert!(G64::parse(&bytes[..700]).is_err());
    }
}
