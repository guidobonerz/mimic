//! CAS: Atari cassette image. A sequence of chunks: 4-byte type, 16-bit
//! length and 16-bit aux value (little-endian), then the data.
//!
//! | Type   | Aux                         | Data                         |
//! | ------ | --------------------------- | ---------------------------- |
//! | `FUJI` | –                           | tape description (text)      |
//! | `baud` | baud rate of what follows   | –                            |
//! | `data` | gap before the record, ms   | one record, as POKEY sent it |
//!
//! Other chunk types (`fsk `, `pwm*`: raw signal timings) are skipped.
//! Blank tapes made here are padded with zero bytes after the last chunk
//! so recordings fit into the file; a zero chunk type ends the tape.
//! Source: the CAS format as documented with the A8CAS tools.

use emu_core::media::MediaError;

/// The standard cassette rate.
pub const BAUD: u16 = 600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Gap (inter-record tone) before the record, in ms.
    pub gap_ms: u16,
    pub baud: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cas {
    pub description: Vec<u8>,
    pub records: Vec<Record>,
}

impl Cas {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() < 8 || &bytes[..4] != b"FUJI" {
            return Err(MediaError::Format("not a CAS image".into()));
        }
        let mut description = Vec::new();
        let mut records = Vec::new();
        let mut baud = BAUD;
        let mut i = 0;
        while i + 8 <= bytes.len() {
            let kind = &bytes[i..i + 4];
            if kind == [0; 4] {
                break; // padding of a blank tape
            }
            let len = usize::from(u16::from_le_bytes([bytes[i + 4], bytes[i + 5]]));
            let aux = u16::from_le_bytes([bytes[i + 6], bytes[i + 7]]);
            let data = bytes
                .get(i + 8..i + 8 + len)
                .ok_or_else(|| MediaError::Format("CAS chunk truncated".into()))?;
            match kind {
                b"FUJI" => description = data.to_vec(),
                b"baud" => baud = aux.max(1),
                b"data" => records.push(Record {
                    gap_ms: aux,
                    baud,
                    data: data.to_vec(),
                }),
                _ => {}
            }
            i += 8 + len;
        }
        Ok(Self {
            description,
            records,
        })
    }

    /// The image, with a `baud` chunk wherever the rate changes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut chunk = |kind: &[u8; 4], aux: u16, data: &[u8]| {
            out.extend_from_slice(kind);
            out.extend_from_slice(&(data.len() as u16).to_le_bytes());
            out.extend_from_slice(&aux.to_le_bytes());
            out.extend_from_slice(data);
        };
        chunk(b"FUJI", 0, &self.description);
        let mut baud = 0;
        for r in &self.records {
            if r.baud != baud {
                chunk(b"baud", r.baud, &[]);
                baud = r.baud;
            }
            chunk(b"data", r.gap_ms, &r.data);
        }
        out
    }
}

/// A blank tape: an empty CAS padded to `capacity` bytes.
pub fn blank(capacity: usize) -> Vec<u8> {
    let mut bytes = Cas {
        description: b"blank tape".to_vec(),
        records: Vec::new(),
    }
    .to_bytes();
    bytes.resize(capacity.max(bytes.len()), 0);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_rates_and_padding() {
        let cas = Cas {
            description: b"test".to_vec(),
            records: vec![
                Record {
                    gap_ms: 20_000,
                    baud: 600,
                    data: vec![0x55, 0x55, 0xfc],
                },
                Record {
                    gap_ms: 250,
                    baud: 600,
                    data: vec![1, 2],
                },
                Record {
                    gap_ms: 100,
                    baud: 1200,
                    data: vec![3],
                },
            ],
        };
        let mut bytes = cas.to_bytes();
        assert_eq!(Cas::parse(&bytes).unwrap(), cas);
        bytes.resize(bytes.len() + 100, 0);
        assert_eq!(Cas::parse(&bytes).unwrap(), cas, "padding ends the tape");
        let blank = Cas::parse(&blank(4096)).unwrap();
        assert!(blank.records.is_empty());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Cas::parse(b"RIFF0000").is_err());
        let mut b = Cas {
            description: vec![],
            records: vec![Record {
                gap_ms: 1,
                baud: 600,
                data: vec![1; 10],
            }],
        }
        .to_bytes();
        b.truncate(b.len() - 3);
        assert!(Cas::parse(&b).is_err());
    }
}
