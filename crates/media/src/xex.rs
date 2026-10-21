//! XEX: Atari DOS binary load file. Segments of `start`, `end` (inclusive,
//! little-endian) and data; `$FF $FF` must open the file and may precede
//! any segment. Two addresses have a meaning: a segment writing INITAD
//! (`$02E2`) asks for that routine to be called once the segment is
//! loaded, RUNAD (`$02E0`) names where the program starts after loading.
//! Source: Atari DOS 2.0S Reference Manual, "Binary file format".

use emu_core::media::MediaError;

pub const RUNAD: u16 = 0x02e0;
pub const INITAD: u16 = 0x02e2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: u16,
    pub data: Vec<u8>,
}

impl Segment {
    /// Whether the segment writes the 16-bit vector at `addr`.
    pub fn covers(&self, addr: u16) -> bool {
        let end = u32::from(self.start) + self.data.len() as u32;
        u32::from(self.start) <= u32::from(addr) && u32::from(addr) + 1 < end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xex {
    pub segments: Vec<Segment>,
}

impl Xex {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let err = |m: &str| Err(MediaError::Format(format!("XEX: {m}")));
        if bytes.len() < 2 || bytes[..2] != [0xff, 0xff] {
            return err("missing $FFFF header");
        }
        let word = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let mut segments = Vec::new();
        let mut i = 2;
        while i < bytes.len() {
            if i + 4 > bytes.len() {
                // Trailing junk after the last segment is common; a cut
                // header is not.
                if segments.is_empty() {
                    return err("truncated segment header");
                }
                break;
            }
            let mut start = word(i);
            if start == 0xffff {
                i += 2;
                if i + 4 > bytes.len() {
                    break;
                }
                start = word(i);
            }
            let end = word(i + 2);
            if end < start {
                return err("segment ends before it starts");
            }
            let len = usize::from(end - start) + 1;
            let data_at = i + 4;
            if data_at + len > bytes.len() {
                return err("segment data truncated");
            }
            segments.push(Segment {
                start,
                data: bytes[data_at..data_at + len].to_vec(),
            });
            i = data_at + len;
        }
        if segments.is_empty() {
            return err("no segments");
        }
        Ok(Self { segments })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_and_vectors() {
        let bytes = [
            0xff, 0xff, 0x00, 0x06, 0x01, 0x06, 0xa9, 0x01, // $0600–$0601
            0xff, 0xff, 0xe0, 0x02, 0xe1, 0x02, 0x00, 0x06, // RUNAD = $0600
        ];
        let x = Xex::parse(&bytes).unwrap();
        assert_eq!(x.segments.len(), 2);
        assert_eq!(x.segments[0].start, 0x0600);
        assert_eq!(x.segments[0].data, [0xa9, 0x01]);
        assert!(x.segments[1].covers(RUNAD));
        assert!(!x.segments[1].covers(INITAD));
        assert!(!x.segments[0].covers(RUNAD));
    }

    #[test]
    fn rejects_broken_files() {
        assert!(Xex::parse(&[0x00, 0x06]).is_err());
        assert!(Xex::parse(&[0xff, 0xff]).is_err());
        assert!(Xex::parse(&[0xff, 0xff, 0x00, 0x06, 0x05, 0x06, 1]).is_err());
        assert!(Xex::parse(&[0xff, 0xff, 0x10, 0x06, 0x00, 0x06]).is_err());
        // Junk after a complete segment is ignored.
        assert!(Xex::parse(&[0xff, 0xff, 0x00, 0x06, 0x00, 0x06, 1, 0]).is_ok());
    }
}
