//! CRT: cartridge images as CHIP packets behind a header.
//!
//! Header (big-endian): a 16-byte signature ("C64 CARTRIDGE   ", "VIC20
//! CARTRIDGE ", "C264 CARTRIDGE  " for the C16/Plus/4), the header length
//! (u32, at least `$40`), the version (u16), the hardware type (u16), the
//! EXROM and GAME line levels the cartridge sets (0 = low = active), a
//! sub-type, and the name (32 bytes at `$20`, zero-padded). Then CHIP
//! packets: "CHIP", the packet length (u32, including its 16-byte header),
//! the chip type (u16: 0 ROM, 1 RAM, 2 flash), the bank (u16), the load
//! address (u16) and the image size (u16), then the data. Source: the CRT
//! format as documented with the VICE cartridge tools.

use emu_core::media::MediaError;

/// Which machine family a CRT is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrtMachine {
    C64,
    Vic20,
    /// C16 and Plus/4.
    C264,
}

impl CrtMachine {
    fn signature(self) -> &'static [u8; 16] {
        match self {
            Self::C64 => b"C64 CARTRIDGE   ",
            Self::Vic20 => b"VIC20 CARTRIDGE ",
            Self::C264 => b"C264 CARTRIDGE  ",
        }
    }
}

/// Hardware types of C64 CRTs (the numbering of the format).
pub mod hw {
    pub const NORMAL: u16 = 0;
    pub const ACTION_REPLAY: u16 = 1;
    pub const FINAL_CARTRIDGE_3: u16 = 3;
    pub const OCEAN: u16 = 5;
    pub const FUN_PLAY: u16 = 7;
    pub const SUPER_GAMES: u16 = 8;
    pub const SYSTEM_3: u16 = 15;
    pub const DINAMIC: u16 = 17;
    pub const MAGIC_DESK: u16 = 19;
    pub const EASYFLASH: u16 = 32;
    pub const RETRO_REPLAY: u16 = 36;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    /// 0 ROM, 1 RAM, 2 flash.
    pub kind: u16,
    pub bank: u16,
    pub load: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crt {
    pub machine: CrtMachine,
    pub version: u16,
    pub hardware: u16,
    /// Line levels as the header gives them (true = high = inactive).
    pub exrom: bool,
    pub game: bool,
    pub subtype: u8,
    pub name: String,
    pub chips: Vec<Chip>,
}

impl Crt {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let err = |m: &str| MediaError::Format(format!("CRT: {m}"));
        let machine = [CrtMachine::C64, CrtMachine::Vic20, CrtMachine::C264]
            .into_iter()
            .find(|m| bytes.get(..16) == Some(&m.signature()[..]))
            .ok_or_else(|| err("no CRT signature"))?;
        let be32 = |o: usize| -> Option<u32> {
            bytes
                .get(o..o + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let be16 = |o: usize| -> Option<u16> {
            bytes
                .get(o..o + 2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
        };
        let header = be32(0x10).ok_or_else(|| err("short header"))? as usize;
        if header < 0x40 || header > bytes.len() {
            return Err(err("bad header length"));
        }
        let name_raw = &bytes[0x20..0x40];
        let len = name_raw.iter().position(|&c| c == 0).unwrap_or(32);
        let mut crt = Self {
            machine,
            version: be16(0x14).unwrap_or(0x0100),
            hardware: be16(0x16).unwrap_or(0),
            exrom: bytes[0x18] != 0,
            game: bytes[0x19] != 0,
            subtype: bytes[0x1a],
            name: String::from_utf8_lossy(&name_raw[..len]).into_owned(),
            chips: Vec::new(),
        };
        let mut pos = header;
        while pos + 16 <= bytes.len() {
            if &bytes[pos..pos + 4] != b"CHIP" {
                return Err(err("bad CHIP packet"));
            }
            let packet = be32(pos + 4).unwrap_or(0) as usize;
            let size = usize::from(be16(pos + 14).unwrap_or(0));
            let data = bytes
                .get(pos + 16..pos + 16 + size)
                .ok_or_else(|| err("CHIP data beyond the file"))?;
            crt.chips.push(Chip {
                kind: be16(pos + 8).unwrap_or(0),
                bank: be16(pos + 10).unwrap_or(0),
                load: be16(pos + 12).unwrap_or(0),
                data: data.to_vec(),
            });
            pos += packet.max(16 + size);
        }
        Ok(crt)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.machine.signature().to_vec();
        out.extend_from_slice(&0x40u32.to_be_bytes());
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.hardware.to_be_bytes());
        out.push(u8::from(self.exrom));
        out.push(u8::from(self.game));
        out.push(self.subtype);
        out.resize(0x20, 0);
        let mut name = self.name.as_bytes().to_vec();
        name.resize(32, 0);
        out.extend_from_slice(&name[..32]);
        for c in &self.chips {
            out.extend_from_slice(b"CHIP");
            out.extend_from_slice(&(16 + c.data.len() as u32).to_be_bytes());
            out.extend_from_slice(&c.kind.to_be_bytes());
            out.extend_from_slice(&c.bank.to_be_bytes());
            out.extend_from_slice(&c.load.to_be_bytes());
            out.extend_from_slice(&(c.data.len() as u16).to_be_bytes());
            out.extend_from_slice(&c.data);
        }
        out
    }

    /// The chip of `bank` loaded at `load`, if any.
    pub fn chip(&self, bank: u16, load: u16) -> Option<&Chip> {
        self.chips.iter().find(|c| c.bank == bank && c.load == load)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_errors() {
        let crt = Crt {
            machine: CrtMachine::C64,
            version: 0x0100,
            hardware: hw::OCEAN,
            exrom: false,
            game: true,
            subtype: 0,
            name: "TEST".into(),
            chips: vec![
                Chip {
                    kind: 0,
                    bank: 0,
                    load: 0x8000,
                    data: vec![1; 0x2000],
                },
                Chip {
                    kind: 0,
                    bank: 1,
                    load: 0x8000,
                    data: vec![2; 0x2000],
                },
            ],
        };
        let bytes = crt.to_bytes();
        assert_eq!(bytes.len(), 0x40 + 2 * (16 + 0x2000));
        let back = Crt::parse(&bytes).unwrap();
        assert_eq!(back, crt);
        assert_eq!(back.chip(1, 0x8000).unwrap().data[0], 2);
        assert!(back.chip(2, 0x8000).is_none());
        assert!(Crt::parse(b"NOT A CARTRIDGE.").is_err());
        let mut cut = bytes.clone();
        cut.truncate(0x40 + 100);
        assert!(Crt::parse(&cut).is_err());
        let mut vic = crt.clone();
        vic.machine = CrtMachine::Vic20;
        assert_eq!(
            Crt::parse(&vic.to_bytes()).unwrap().machine,
            CrtMachine::Vic20
        );
    }
}
