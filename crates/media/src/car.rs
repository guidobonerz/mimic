//! CAR: Atari 8-bit cartridge image. A 16-byte header — `CART`, the
//! cartridge type and a checksum (sum of the data bytes) as 32-bit
//! big-endian values, 4 unused bytes — then the ROM. Raw dumps without a
//! header are accepted by size (8 KB standard, 16 KB standard 16K).
//! Source: the CAR format as published with Atari800 and Altirra's
//! documentation (type numbers).

use emu_core::media::MediaError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Car {
    /// The cartridge type number.
    pub kind: u32,
    pub rom: Vec<u8>,
}

impl Car {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() >= 16 && &bytes[..4] == b"CART" {
            let word =
                |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
            let kind = word(4);
            let sum = word(8);
            let rom = bytes[16..].to_vec();
            let actual = rom.iter().fold(0u32, |a, &b| a.wrapping_add(u32::from(b)));
            if actual != sum {
                return Err(MediaError::Format(format!(
                    "CAR checksum {actual:08X}, header says {sum:08X}"
                )));
            }
            return Ok(Self { kind, rom });
        }
        let kind = match bytes.len() {
            0x2000 => 1,
            0x4000 => 2,
            n => {
                return Err(MediaError::Format(format!(
                    "raw cartridge of {n} bytes (8 or 16 KB without a CAR header)"
                )))
            }
        };
        Ok(Self {
            kind,
            rom: bytes.to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let sum = self
            .rom
            .iter()
            .fold(0u32, |a, &b| a.wrapping_add(u32::from(b)));
        let mut out = b"CART".to_vec();
        out.extend_from_slice(&self.kind.to_be_bytes());
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.rom);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_checksum_and_raw_dumps() {
        let car = Car {
            kind: 12,
            rom: (0..0x8000).map(|i| i as u8).collect(),
        };
        let bytes = car.to_bytes();
        assert_eq!(Car::parse(&bytes).unwrap(), car);
        let mut bad = bytes.clone();
        bad[100] ^= 1;
        assert!(Car::parse(&bad).is_err());
        assert_eq!(Car::parse(&[0; 0x2000]).unwrap().kind, 1);
        assert_eq!(Car::parse(&[0; 0x4000]).unwrap().kind, 2);
        assert!(Car::parse(&[0; 100]).is_err());
    }
}
