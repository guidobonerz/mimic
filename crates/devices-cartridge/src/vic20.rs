//! VIC-20 ROM cartridges: ROM in the expansion blocks BLK1–3 (`$2000`,
//! `$4000`, `$6000`) and BLK5 (`$A000`; a cartridge there with "A0CBM" at
//! `$A004` starts at power-on). From a VIC20 CRT (chips at their load
//! addresses) or a raw dump with its load address in the first two bytes
//! (like a PRG). Source: VIC-20 Programmer's Reference Guide, memory map.

use emu_core::media::MediaError;
use media::crt::{Crt, CrtMachine};

use crate::slot::CartSlot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VicCart {
    /// ROM blocks: start address and contents.
    blocks: Vec<(u16, Vec<u8>)>,
}

impl VicCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let blocks = match Crt::parse(bytes) {
            Ok(crt) if crt.machine == CrtMachine::Vic20 => {
                crt.chips.into_iter().map(|c| (c.load, c.data)).collect()
            }
            Ok(crt) => {
                return Err(MediaError::Format(format!(
                    "a {:?} cartridge does not fit the VIC-20",
                    crt.machine
                )))
            }
            Err(_) if bytes.len() > 2 => {
                vec![(
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes[2..].to_vec(),
                )]
            }
            Err(e) => return Err(e),
        };
        for (start, data) in &blocks {
            let end = usize::from(*start) + data.len();
            let fits = matches!(start, 0x2000..=0x7fff | 0xa000..=0xbfff)
                && (end <= 0x8000 || (*start >= 0xa000 && end <= 0xc000));
            if !fits || data.is_empty() {
                return Err(MediaError::Format(format!(
                    "VIC-20 cartridge: ROM at ${start:04X} is not in BLK1–3 or BLK5"
                )));
            }
        }
        Ok(Self { blocks })
    }

    /// The cartridge byte at `addr`, if a ROM is there.
    pub fn peek(&self, addr: u16) -> Option<u8> {
        self.blocks.iter().find_map(|(start, data)| {
            addr.checked_sub(*start)
                .and_then(|o| data.get(usize::from(o)))
                .copied()
        })
    }
}

pub fn slot() -> CartSlot<VicCart> {
    CartSlot::new(VicCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_dump_and_crt() {
        let mut raw = vec![0x00, 0xa0];
        raw.extend([1, 2, 3]);
        let c = VicCart::parse(&raw).unwrap();
        assert_eq!(
            (c.peek(0xa000), c.peek(0xa002), c.peek(0xa003)),
            (Some(1), Some(3), None)
        );
        assert!(VicCart::parse(&[0x00, 0x90, 1]).is_err(), "I/O area");
        let crt = Crt {
            machine: CrtMachine::Vic20,
            version: 0x0100,
            hardware: 0,
            exrom: true,
            game: true,
            subtype: 0,
            name: String::new(),
            chips: vec![media::crt::Chip {
                kind: 0,
                bank: 0,
                load: 0x6000,
                data: vec![7; 0x2000],
            }],
        };
        let c = VicCart::parse(&crt.to_bytes()).unwrap();
        assert_eq!(c.peek(0x7fff), Some(7));
    }
}
