//! TI-99/4A cartridges. A cartridge is up to three dumps, by the usual
//! naming: `NAMEC.BIN` the ROM at `>6000` (8 KB, or several 8 KB banks),
//! `NAMED.BIN` its second bank, `NAMEG.BIN` the GROMs from `>6000` of the
//! GROM space (up to 40 KB). [`TiCart`] holds them together; as one file
//! it is this project's bundle ([`TiCart::to_bytes`]): the magic
//! `TI99CART`, then for each part a letter (`C`, `D`, `G`), its length as
//! a 32-bit big-endian number and the bytes.

use emu_core::media::MediaError;

const MAGIC: &[u8; 8] = b"TI99CART";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TiCart {
    /// The ROM at `>6000` (bank 0 first), empty without one.
    pub rom: Vec<u8>,
    /// The second bank (a `D` file).
    pub rom2: Vec<u8>,
    /// The GROMs from GROM address `>6000`.
    pub grom: Vec<u8>,
}

/// The part a dump's file name names: `C`, `D` or `G` before the
/// extension (`PARSECG.BIN` → `G`), `None` otherwise.
pub fn part_of(name: &str) -> Option<char> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    match stem.chars().last()?.to_ascii_uppercase() {
        c @ ('C' | 'D' | 'G') if stem.len() > 1 => Some(c),
        _ => None,
    }
}

impl TiCart {
    /// A bundle, or a single dump named after its part (a name without a
    /// part letter is taken as the ROM).
    pub fn parse(name: &str, data: &[u8]) -> Result<Self, MediaError> {
        if let Some(mut rest) = data.strip_prefix(MAGIC.as_slice()) {
            let mut cart = Self::default();
            while let [letter, a, b, c, d, tail @ ..] = rest {
                let len = u32::from_be_bytes([*a, *b, *c, *d]) as usize;
                let bytes = tail
                    .get(..len)
                    .ok_or_else(|| MediaError::Format("TI cartridge bundle cut short".into()))?;
                match letter {
                    b'C' => cart.rom = bytes.to_vec(),
                    b'D' => cart.rom2 = bytes.to_vec(),
                    b'G' => cart.grom = bytes.to_vec(),
                    _ => return Err(MediaError::Format("TI cartridge bundle part".into())),
                }
                rest = &tail[len..];
            }
            if !rest.is_empty() {
                return Err(MediaError::Format("TI cartridge bundle cut short".into()));
            }
            return cart.check();
        }
        let mut cart = Self::default();
        match part_of(name) {
            Some('G') => cart.grom = data.to_vec(),
            Some('D') => cart.rom2 = data.to_vec(),
            _ => cart.rom = data.to_vec(),
        }
        cart.check()
    }

    fn check(self) -> Result<Self, MediaError> {
        if self.rom.is_empty() && self.rom2.is_empty() && self.grom.is_empty() {
            return Err(MediaError::Format("empty TI cartridge".into()));
        }
        if self.grom.len() > 0xa000 {
            return Err(MediaError::Format("TI cartridge GROMs over 40 KB".into()));
        }
        if self.rom.len() + self.rom2.len() > 0x20_0000 {
            return Err(MediaError::Format("TI cartridge ROM over 2 MB".into()));
        }
        Ok(self)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for (letter, part) in [(b'C', &self.rom), (b'D', &self.rom2), (b'G', &self.grom)] {
            if !part.is_empty() {
                out.push(letter);
                out.extend_from_slice(&(part.len() as u32).to_be_bytes());
                out.extend_from_slice(part);
            }
        }
        out
    }

    /// The 8 KB ROM banks: the C dump's banks, then the D dump's.
    pub fn banks(&self) -> Vec<Vec<u8>> {
        let mut banks: Vec<Vec<u8>> = self
            .rom
            .chunks(0x2000)
            .chain(self.rom2.chunks(0x2000))
            .map(|b| {
                let mut b = b.to_vec();
                b.resize(0x2000, 0);
                b
            })
            .collect();
        if banks.is_empty() {
            banks.push(Vec::new());
        }
        banks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_the_part() {
        assert_eq!(part_of("PARSECG.BIN"), Some('G'));
        assert_eq!(part_of("dir/MunchMnC.Bin"), Some('C'));
        assert_eq!(part_of("FATHOMD.BIN"), Some('D'));
        assert_eq!(part_of("GAME.BIN"), None);
        assert_eq!(part_of("C.BIN"), None);
    }

    #[test]
    fn bundles_round_trip() {
        let cart = TiCart {
            rom: vec![1; 0x2000],
            rom2: vec![2; 0x2000],
            grom: vec![3; 0x1800],
        };
        let bytes = cart.to_bytes();
        assert_eq!(TiCart::parse("x.bin", &bytes).unwrap(), cart);
        assert_eq!(cart.banks().len(), 2);
        assert!(TiCart::parse("x", &bytes[..bytes.len() - 1]).is_err());
        let g = TiCart::parse("ALPINERG.BIN", &[7; 100]).unwrap();
        assert_eq!((g.grom.len(), g.rom.len()), (100, 0));
    }
}
