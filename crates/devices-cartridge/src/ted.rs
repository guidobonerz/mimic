//! C16/Plus/4 ROM cartridges: up to two 32 KB cartridges, C1 and C2, each
//! with a low half (`$8000–$BFFF`) and a high half (`$C000–$FFFF`) that the
//! ROM bank register `$FDD0` switches in (bank 2 = C1, 3 = C2). The KERNAL
//! starts a cartridge whose low ROM has "CBM" at `$8007`. From a C264 CRT
//! (chips at `$8000`/`$C000`, bank 0 = C1, 1 = C2) or a raw dump of 16 KB
//! (C1 low) or 32 KB (C1 low and high). Source: Plus/4 Programmer's
//! Reference Guide (memory banking, cartridge auto-start).

use emu_core::media::MediaError;
use media::crt::{Crt, CrtMachine};

use crate::slot::CartSlot;

const HALF: usize = 0x4000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TedCart {
    /// C1 low, C1 high, C2 low, C2 high.
    roms: [Option<Vec<u8>>; 4],
}

impl TedCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let mut cart = Self::default();
        match Crt::parse(bytes) {
            Ok(crt) if crt.machine == CrtMachine::C264 => {
                for c in crt.chips {
                    let slot = usize::from(c.bank.min(1)) * 2;
                    match c.load {
                        0x8000 => {
                            cart.roms[slot] = Some(c.data[..c.data.len().min(HALF)].to_vec());
                            if c.data.len() > HALF {
                                cart.roms[slot + 1] = Some(c.data[HALF..].to_vec());
                            }
                        }
                        0xc000 => cart.roms[slot + 1] = Some(c.data),
                        other => {
                            return Err(MediaError::Format(format!(
                                "C16/Plus/4 cartridge: chip at ${other:04X}"
                            )))
                        }
                    }
                }
            }
            Ok(crt) => {
                return Err(MediaError::Format(format!(
                    "a {:?} cartridge does not fit the C16/Plus/4",
                    crt.machine
                )))
            }
            Err(_) => match bytes.len() {
                n if n == HALF => cart.roms[0] = Some(bytes.to_vec()),
                n if n == 2 * HALF => {
                    cart.roms[0] = Some(bytes[..HALF].to_vec());
                    cart.roms[1] = Some(bytes[HALF..].to_vec());
                }
                n => {
                    return Err(MediaError::Format(format!(
                        "C16/Plus/4 cartridge: {n} bytes (16 or 32 KB raw, or a CRT)"
                    )))
                }
            },
        }
        Ok(cart)
    }

    /// The byte of ROM bank `bank` (2 = C1, 3 = C2) at `addr` (`$8000–$FFFF`),
    /// if that cartridge half exists.
    pub fn peek(&self, bank: u8, addr: u16) -> Option<u8> {
        let slot = match bank {
            2 => 0,
            3 => 2,
            _ => return None,
        } + usize::from(addr >= 0xc000);
        let rom = self.roms[slot].as_ref()?;
        rom.get(usize::from(addr & 0x3fff) % rom.len().max(1))
            .copied()
    }
}

pub fn slot() -> CartSlot<TedCart> {
    CartSlot::new(TedCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_dumps_and_banks() {
        let mut raw = vec![1u8; HALF];
        raw.extend(vec![2u8; HALF]);
        let c = TedCart::parse(&raw).unwrap();
        assert_eq!(c.peek(2, 0x8000), Some(1));
        assert_eq!(c.peek(2, 0xffff), Some(2));
        assert_eq!(c.peek(3, 0x8000), None, "no C2");
        assert_eq!(c.peek(0, 0x8000), None, "internal ROM");
        assert!(TedCart::parse(&[0; 100]).is_err());
    }
}
