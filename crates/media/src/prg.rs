//! PRG: a Commodore program file, two-byte little-endian load address
//! followed by the data.

use emu_core::media::MediaError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prg {
    pub load: u16,
    pub data: Vec<u8>,
}

impl Prg {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() < 2 {
            return Err(MediaError::Format(
                "PRG shorter than its load address".into(),
            ));
        }
        let load = u16::from_le_bytes([bytes[0], bytes[1]]);
        let data = bytes[2..].to_vec();
        if usize::from(load) + data.len() > 0x10000 {
            return Err(MediaError::Format("PRG extends beyond 64 KB".into()));
        }
        Ok(Self { load, data })
    }

    /// First address after the program.
    pub fn end(&self) -> u32 {
        u32::from(self.load) + self.data.len() as u32
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.load.to_le_bytes().to_vec();
        v.extend_from_slice(&self.data);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_round_trip() {
        let p = Prg::parse(&[0x01, 0x08, 0xaa, 0xbb]).unwrap();
        assert_eq!((p.load, p.end()), (0x0801, 0x0803));
        assert_eq!(p.to_bytes(), [0x01, 0x08, 0xaa, 0xbb]);
        assert!(Prg::parse(&[1]).is_err());
        assert!(Prg::parse(&[0xff, 0xff, 1, 2]).is_err());
    }
}
