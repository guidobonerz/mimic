//! Expansion modules in the base unit's two slots (`$08` and `$0C`).
//!
//! A module is reached through port `$80` with the slot number in the
//! upper half of the port address (`LD BC,$0880` / `IN A,(C)`): a read
//! gives its structure byte (`$FF` for an empty slot), a write its control
//! byte:
//!
//! | Bit | Meaning |
//! | --- | --- |
//! | 0 | the module's memory is switched on |
//! | 1 | writing allowed (RAM modules) |
//! | 7–5 or 7–6 | the upper address bits of the block it appears at |
//!
//! A module answers only where the base unit has nothing switched on; the
//! lower slot number goes first.
//!
//! | Module | Structure byte | Memory |
//! | --- | --- | --- |
//! | M022 Expander RAM | `$F4` | 16 KB RAM |
//! | M011 64K RAM | `$F6` | 64 KB RAM, the four blocks rotated by bits 7–6 |
//! | M006 BASIC | `$FC` | 16 KB ROM (BASIC for the KC85/2) |
//! | M012 Texor, M026 Forth, M027 Development | `$FB` | 8 KB ROM |
//! | M033 Typestar | `$01` | 16 KB ROM in two 8 KB segments (bit 4) |
//!
//! Source: KC85/3 Systemhandbuch ("Modulsteuerung"); the modules' own
//! manuals (M006, M011, M022, M026, M027, M033) for structure bytes and
//! control bits.

use emu_core::{StateError, StateReader, StateWriter};

pub const SLOTS: [u8; 2] = [0x08, 0x0c];

pub const CONTROL_ON: u8 = 0x01;
pub const CONTROL_WRITE: u8 = 0x02;

/// A module type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    /// Name for options and ROM roles ("m022").
    pub name: &'static str,
    pub id: u8,
    /// The control byte's address bits.
    address_mask: u8,
    /// Size of the block it appears in.
    window: usize,
    /// Size of its memory.
    size: usize,
    ram: bool,
}

pub const KINDS: [Kind; 7] = [
    Kind {
        name: "m022",
        id: 0xf4,
        address_mask: 0xc0,
        window: 0x4000,
        size: 0x4000,
        ram: true,
    },
    Kind {
        name: "m011",
        id: 0xf6,
        address_mask: 0xc0,
        window: 0x1_0000,
        size: 0x1_0000,
        ram: true,
    },
    Kind {
        name: "m006",
        id: 0xfc,
        address_mask: 0xc0,
        window: 0x4000,
        size: 0x4000,
        ram: false,
    },
    Kind {
        name: "m012",
        id: 0xfb,
        address_mask: 0xe0,
        window: 0x2000,
        size: 0x2000,
        ram: false,
    },
    Kind {
        name: "m026",
        id: 0xfb,
        address_mask: 0xe0,
        window: 0x2000,
        size: 0x2000,
        ram: false,
    },
    Kind {
        name: "m027",
        id: 0xfb,
        address_mask: 0xe0,
        window: 0x2000,
        size: 0x2000,
        ram: false,
    },
    Kind {
        name: "m033",
        id: 0x01,
        address_mask: 0xe0,
        window: 0x2000,
        size: 0x4000,
        ram: false,
    },
];

pub fn kind(name: &str) -> Option<Kind> {
    KINDS.iter().copied().find(|k| k.name == name)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub kind: Kind,
    pub control: u8,
    data: Vec<u8>,
}

impl Module {
    /// A module with its ROM image (`None` for RAM modules). `Err` with
    /// the size the image must have.
    pub fn new(kind: Kind, rom: Option<&[u8]>) -> Result<Self, usize> {
        let data = match (kind.ram, rom) {
            (true, _) => vec![0; kind.size],
            (false, Some(r)) if r.len() == kind.size => r.to_vec(),
            _ => return Err(kind.size),
        };
        Ok(Self {
            kind,
            control: 0,
            data,
        })
    }

    fn offset(&self, addr: u16) -> Option<usize> {
        if self.control & CONTROL_ON == 0 {
            return None;
        }
        let k = &self.kind;
        let base = usize::from(self.control & k.address_mask) << 8;
        let o = usize::from(addr).wrapping_sub(base) & 0xffff;
        if o >= k.window {
            return None;
        }
        // The Typestar's second half: control bit 4.
        let segment = if k.size > k.window && self.control & 0x10 != 0 {
            k.window
        } else {
            0
        };
        Some(segment + o)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Modules {
    pub slot: [Option<Module>; 2],
}

impl Modules {
    fn index(slot: u8) -> Option<usize> {
        SLOTS.iter().position(|&s| s == slot)
    }

    /// Port `$80` read: the structure byte of the module in `slot`.
    pub fn id(&self, slot: u8) -> u8 {
        Self::index(slot)
            .and_then(|i| self.slot[i].as_ref())
            .map_or(0xff, |m| m.kind.id)
    }

    /// Port `$80` write: the control byte.
    pub fn control(&mut self, slot: u8, v: u8) {
        if let Some(m) = Self::index(slot).and_then(|i| self.slot[i].as_mut()) {
            m.control = v;
        }
    }

    pub fn read(&self, addr: u16) -> Option<u8> {
        self.slot
            .iter()
            .flatten()
            .find_map(|m| m.offset(addr).map(|o| m.data[o]))
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        for m in self.slot.iter_mut().flatten() {
            if let Some(o) = m.offset(addr) {
                if m.kind.ram && m.control & CONTROL_WRITE != 0 {
                    m.data[o] = v;
                }
                return;
            }
        }
    }

    /// A debugger's write: module RAM even where it is protected.
    pub fn poke(&mut self, addr: u16, v: u8) -> bool {
        for m in self.slot.iter_mut().flatten() {
            if let Some(o) = m.offset(addr) {
                if m.kind.ram {
                    m.data[o] = v;
                }
                return m.kind.ram;
            }
        }
        false
    }

    /// RESET: all modules switched off.
    pub fn reset(&mut self) {
        for m in self.slot.iter_mut().flatten() {
            m.control = 0;
        }
    }

    /// Power-on: module RAM cleared as well.
    pub fn clear(&mut self) {
        for m in self.slot.iter_mut().flatten() {
            m.control = 0;
            if m.kind.ram {
                m.data.fill(0);
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        for m in &self.slot {
            w.bool(m.is_some());
            if let Some(m) = m {
                w.u8(m.kind.id);
                w.u8(m.control);
                if m.kind.ram {
                    w.raw(&m.data);
                }
            }
        }
    }

    /// Loads into the modules that are plugged in; a state with other
    /// modules is refused.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        for m in &mut self.slot {
            if r.bool()? != m.is_some() {
                return Err(StateError::Invalid("KC85 modules differ"));
            }
            if let Some(m) = m {
                if r.u8()? != m.kind.id {
                    return Err(StateError::Invalid("KC85 modules differ"));
                }
                m.control = r.u8()?;
                if m.kind.ram {
                    r.raw_into(&mut m.data)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ram_module_appears_where_its_control_byte_says() {
        let mut m = Modules::default();
        m.slot[0] = Some(Module::new(kind("m022").unwrap(), None).unwrap());
        assert_eq!(m.id(0x08), 0xf4);
        assert_eq!(m.id(0x0c), 0xff);
        assert_eq!(m.read(0x4000), None);
        m.control(0x08, 0x43);
        m.write(0x4123, 0x77);
        assert_eq!(m.read(0x4123), Some(0x77));
        assert_eq!(m.read(0x8123), None);
        m.control(0x08, 0x41);
        m.write(0x4123, 0x11);
        assert_eq!(m.read(0x4123), Some(0x77), "write-protected");
        m.control(0x08, 0x81);
        assert_eq!(m.read(0x8123), Some(0x77));
    }

    #[test]
    fn a_rom_module_needs_its_image_and_the_first_slot_wins() {
        let k = kind("m026").unwrap();
        assert_eq!(Module::new(k, None), Err(0x2000));
        assert_eq!(Module::new(k, Some(&[0; 16])), Err(0x2000));
        let mut m = Modules::default();
        m.slot[0] = Some(Module::new(k, Some(&[0xaa; 0x2000])).unwrap());
        m.slot[1] = Some(Module::new(k, Some(&[0xbb; 0x2000])).unwrap());
        m.control(0x08, 0xc1);
        m.control(0x0c, 0xc1);
        assert_eq!(m.read(0xc000), Some(0xaa));
        assert_eq!(m.read(0xe000), None);
        m.control(0x08, 0xc0);
        assert_eq!(m.read(0xdfff), Some(0xbb));
        m.write(0xc000, 0);
        assert_eq!(m.read(0xc000), Some(0xbb));
    }
}
