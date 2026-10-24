//! WAV audio files (RIFF/WAVE with PCM samples), as tape recordings.
//!
//! A RIFF file is the tag `RIFF`, a 32-bit little-endian size and the
//! form type `WAVE`, followed by chunks (4-byte id, 32-bit size, data,
//! padded to an even length). `fmt ` holds the format tag (1 = PCM), the
//! channel count, the sample rate, the byte rate, the block alignment and
//! the bits per sample; `data` the interleaved samples, 8-bit unsigned or
//! 16/24/32-bit signed little-endian (Microsoft/IBM, "Multimedia
//! Programming Interface and Data Specifications 1.0", 1991, "WAVE Form").
//! Recordings are read as one mono channel (the channels averaged).

/// A mono recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wav {
    pub rate: u32,
    pub samples: Vec<i16>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

impl Wav {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return Err("not a RIFF/WAVE file".into());
        }
        let mut fmt: Option<(u16, u16, u32, u16)> = None;
        let mut pos = 12;
        while pos + 8 <= data.len() {
            let id = &data[pos..pos + 4];
            let size = u32_at(data, pos + 4).unwrap_or(0) as usize;
            let body = data
                .get(pos + 8..(pos + 8).saturating_add(size).min(data.len()))
                .unwrap_or(&[]);
            match id {
                b"fmt " => {
                    let tag = u16_at(body, 0).ok_or("short fmt chunk")?;
                    let channels = u16_at(body, 2).ok_or("short fmt chunk")?;
                    let rate = u32_at(body, 4).ok_or("short fmt chunk")?;
                    let bits = u16_at(body, 14).ok_or("short fmt chunk")?;
                    fmt = Some((tag, channels, rate, bits));
                }
                b"data" => {
                    let (tag, channels, rate, bits) = fmt.ok_or("data before fmt")?;
                    // 0xFFFE: WAVE_FORMAT_EXTENSIBLE, PCM in practice.
                    if tag != 1 && tag != 0xfffe {
                        return Err(format!("format tag {tag} is not PCM"));
                    }
                    if channels == 0 || rate == 0 || !matches!(bits, 8 | 16 | 24 | 32) {
                        return Err(format!(
                            "unsupported format: {channels} channels, {bits} bits"
                        ));
                    }
                    let width = usize::from(bits / 8);
                    let frame = width * usize::from(channels);
                    let samples = body
                        .chunks_exact(frame)
                        .map(|f| {
                            let sum: i32 = f
                                .chunks_exact(width)
                                .map(|s| match width {
                                    1 => (i32::from(s[0]) - 128) << 8,
                                    _ => {
                                        i32::from(i16::from_le_bytes([s[width - 2], s[width - 1]]))
                                    }
                                })
                                .sum();
                            (sum / i32::from(channels)) as i16
                        })
                        .collect();
                    return Ok(Self { rate, samples });
                }
                _ => {}
            }
            pos += 8 + size + (size & 1);
        }
        Err("no data chunk".into())
    }

    /// A 16-bit mono PCM file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let data_len = self.samples.len() as u32 * 2;
        let mut v = Vec::with_capacity(44 + data_len as usize);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&self.rate.to_le_bytes());
        v.extend_from_slice(&(self.rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_formats() {
        let w = Wav {
            rate: 22_050,
            samples: vec![0, 1000, -1000, i16::MAX, i16::MIN],
        };
        assert_eq!(Wav::parse(&w.to_bytes()).unwrap(), w);
        // 8-bit stereo: the channels are averaged.
        let mut b = w.to_bytes();
        b[22] = 2; // channels
        b[34] = 8; // bits
        b[32] = 2; // block align
        let p = Wav::parse(&b).unwrap();
        assert_eq!(p.samples.len(), 5);
        assert!(Wav::parse(b"RIFF\0\0\0\0WAVE").is_err());
        assert!(Wav::parse(b"nope").is_err());
    }
}
