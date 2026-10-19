//! T64: tape image container (Miha Peternel's C64S format).
//!
//! Layout: 64-byte header (signature "C64", version at $20, max/used
//! entries at $22/$24, tape name at $28), then 32-byte directory entries:
//! +0 entry type (1 = normal file), +1 C64 file type, +2/+4 start/end
//! address, +8 offset of the data in the image, +16 name (16 bytes, padded
//! with $20 or $A0). Many images in the wild carry a wrong end address; the
//! data length is then taken from the next entry's offset or the image end.

use emu_core::media::MediaError;

use crate::prg::Prg;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct T64Entry {
    pub name: Vec<u8>,
    pub prg: Prg,
}

fn trim_name(raw: &[u8]) -> Vec<u8> {
    let end = raw
        .iter()
        .rposition(|&b| b != 0x20 && b != 0xa0 && b != 0)
        .map_or(0, |i| i + 1);
    raw[..end].to_vec()
}

pub fn parse(bytes: &[u8]) -> Result<Vec<T64Entry>, MediaError> {
    let bad = |m: &str| MediaError::Format(format!("T64: {m}"));
    if bytes.len() < 64 || !bytes.starts_with(b"C64") {
        return Err(bad("missing header"));
    }
    let max = usize::from(u16::from_le_bytes([bytes[0x22], bytes[0x23]]));
    let dir_end = 64 + max * 32;
    if bytes.len() < dir_end {
        return Err(bad("directory beyond image end"));
    }
    struct Raw {
        name: Vec<u8>,
        start: u16,
        end: u16,
        offset: usize,
    }
    let mut raws: Vec<Raw> = (0..max)
        .map(|i| &bytes[64 + i * 32..96 + i * 32])
        .filter(|e| e[0] == 1)
        .map(|e| Raw {
            name: trim_name(&e[16..32]),
            start: u16::from_le_bytes([e[2], e[3]]),
            end: u16::from_le_bytes([e[4], e[5]]),
            offset: u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize,
        })
        .collect();
    let mut offsets: Vec<usize> = raws.iter().map(|r| r.offset).collect();
    offsets.push(bytes.len());
    offsets.sort_unstable();
    raws.sort_by_key(|r| r.offset);

    raws.into_iter()
        .map(|r| {
            if r.offset < dir_end || r.offset > bytes.len() {
                return Err(bad("entry data outside the image"));
            }
            let next = offsets
                .iter()
                .copied()
                .find(|&o| o > r.offset)
                .unwrap_or(bytes.len());
            let declared = usize::from(r.end.wrapping_sub(r.start));
            let len = if r.end > r.start && r.offset + declared <= next {
                declared
            } else {
                next - r.offset
            };
            let mut raw = r.start.to_le_bytes().to_vec();
            raw.extend_from_slice(&bytes[r.offset..r.offset + len]);
            Ok(T64Entry {
                name: r.name,
                prg: Prg::parse(&raw)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(end: u16) -> Vec<u8> {
        let mut v = vec![0u8; 64 + 32];
        v[..3].copy_from_slice(b"C64");
        v[0x22] = 1;
        v[0x24] = 1;
        let e = &mut v[64..96];
        e[0] = 1;
        e[1] = 0x82;
        e[2..4].copy_from_slice(&0x0801u16.to_le_bytes());
        e[4..6].copy_from_slice(&end.to_le_bytes());
        e[8..12].copy_from_slice(&96u32.to_le_bytes());
        e[16..32].copy_from_slice(b"GAME            ");
        v.extend_from_slice(&[1, 2, 3, 4]);
        v
    }

    #[test]
    fn reads_entries() {
        let e = parse(&image(0x0805)).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, b"GAME");
        assert_eq!(e[0].prg.load, 0x0801);
        assert_eq!(e[0].prg.data, [1, 2, 3, 4]);
    }

    #[test]
    fn repairs_a_bad_end_address() {
        let e = parse(&image(0xc3c6)).unwrap();
        assert_eq!(e[0].prg.data, [1, 2, 3, 4]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"nope").is_err());
        let mut v = image(0x0805);
        v[0x22] = 200;
        assert!(parse(&v).is_err());
    }
}
