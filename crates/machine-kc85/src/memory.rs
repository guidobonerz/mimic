//! The KC85's memory: RAM, the IRM (video RAM), the ROMs and what the
//! PIO and the output ports switch.
//!
//! **KC85/2 and /3** (16 KB RAM, 16 KB IRM):
//!
//! | Address | Block | Switched by |
//! | --- | --- | --- |
//! | `$0000–$3FFF` | RAM0 | PIO A bit 1 (write: bit 3) |
//! | `$4000–$7FFF` | modules only | |
//! | `$8000–$BFFF` | IRM: pixels `$8000`, colours `$A800`, system `$B200` | PIO A bit 2 |
//! | `$C000–$DFFF` | BASIC ROM (KC85/3) | PIO A bit 7 |
//! | `$E000–$FFFF` | CAOS ROM | PIO A bit 0 |
//!
//! **KC85/4 and /5** (RAM in 16 KB blocks: block 0 is RAM0, block 1 RAM4,
//! the rest RAM8; the KC85/4 has 4 blocks, the KC85/5 16):
//!
//! | Address | Block | Switched by |
//! | --- | --- | --- |
//! | `$0000–$3FFF` | RAM0 | PIO A bit 1 (write: bit 3) |
//! | `$4000–$7FFF` | RAM4 | port `$86` bit 0 (write: bit 1) |
//! | `$8000–$BFFF` | IRM, four 16 KB planes (pixels 0, colours 0, pixels 1, colours 1) | PIO A bit 2; plane below `$A800`: port `$84` bits 1 (colours) and 2 (image); from `$A800` up always pixels 0 (system variables) |
//! | `$8000–$BFFF` | RAM8, under the IRM | PIO B bit 5 (write: bit 6); block: port `$84` bits 4–7 (KC85/4: bit 4, blocks 2 and 3) |
//! | `$C000–$DFFF` | CAOS ROM C (KC85/4: 4 KB, to `$CFFF`) | port `$86` bit 7 |
//! | `$C000–$DFFF` | BASIC / USER ROM, under CAOS C (KC85/5: 32 KB, 8 KB segment by port `$86` bits 5–6) | PIO A bit 7 |
//! | `$E000–$FFFF` | CAOS ROM E | PIO A bit 0 |
//!
//! Where the base unit has nothing switched on, the modules answer
//! ([`crate::modules`]); else the bus floats (`$FF`).
//!
//! Sources: KC85/3 Systemhandbuch and KC85/4 Systemhandbuch (VEB
//! Mikroelektronik Mühlhausen; memory map, PIO and port assignment);
//! "KC85/5 – CAOS 4.x Systembeschreibung" (KC-Club) for the KC85/5's RAM
//! segments and USER ROM.

use mimic_core::{StateError, StateReader, StateWriter};

use crate::modules::Modules;
use crate::Model;

pub const BLOCK: usize = 0x4000;

/// PIO A lines.
pub const A_CAOS_E: u8 = 0x01;
pub const A_RAM0: u8 = 0x02;
pub const A_IRM: u8 = 0x04;
pub const A_RAM0_WRITE: u8 = 0x08;
pub const A_TAPE_LED: u8 = 0x20;
pub const A_TAPE_MOTOR: u8 = 0x40;
pub const A_BASIC: u8 = 0x80;
/// PIO B lines (KC85/4).
pub const B_RAM8: u8 = 0x20;
pub const B_RAM8_WRITE: u8 = 0x40;
pub const B_BLINK: u8 = 0x80;
/// Port `$84` (KC85/4).
pub const P84_IMAGE_SHOWN: u8 = 0x01;
pub const P84_COLOURS: u8 = 0x02;
pub const P84_IMAGE: u8 = 0x04;
pub const P84_LOW_COLOUR: u8 = 0x08;
/// Port `$86` (KC85/4).
pub const P86_RAM4: u8 = 0x01;
pub const P86_RAM4_WRITE: u8 = 0x02;
pub const P86_CAOS_C: u8 = 0x80;

#[derive(Clone, Debug, Default)]
pub struct Roms {
    /// 8 KB at `$E000`.
    pub caos_e: Vec<u8>,
    /// KC85/4: 4 KB, KC85/5: 8 KB at `$C000`; else empty.
    pub caos_c: Vec<u8>,
    /// 8 KB (KC85/3, /4), 32 KB (KC85/5) at `$C000`; empty on the KC85/2.
    pub basic: Vec<u8>,
}

#[derive(Clone)]
pub struct Memory {
    model: Model,
    roms: Roms,
    /// RAM blocks of 16 KB.
    pub ram: Vec<u8>,
    /// The IRM: one plane (KC85/2, /3) or four.
    pub irm: Vec<u8>,
    /// The PIO's lines as the memory logic sees them.
    pub pio_a: u8,
    pub pio_b: u8,
    pub port84: u8,
    pub port86: u8,
    pub modules: Modules,
}

/// What an address reaches.
enum Target {
    Ram(usize),
    /// RAM that is write-protected now.
    RamReadOnly(usize),
    Irm(usize),
    CaosE(usize),
    CaosC(usize),
    Basic(usize),
    /// Nothing in the base unit: the modules' turn.
    Outside,
}

impl Memory {
    pub fn new(model: Model, roms: Roms) -> Self {
        let blocks = match model {
            Model::Kc852 | Model::Kc853 => 1,
            Model::Kc854 => 4,
            Model::Kc855 => 16,
        };
        Self {
            model,
            roms,
            ram: vec![0; blocks * BLOCK],
            irm: vec![0; if model.four() { 4 * BLOCK } else { BLOCK }],
            pio_a: 0xff,
            pio_b: 0xff,
            port84: 0,
            port86: 0,
            modules: Modules::default(),
        }
    }

    /// Power-on: RAM and IRM cleared, the output ports at 0.
    pub fn clear(&mut self) {
        self.ram.fill(0);
        self.irm.fill(0);
        self.reset();
        self.modules.clear();
    }

    /// RESET: the KC85/4's output ports go to 0 (the PIO's lines follow
    /// the PIO).
    pub fn reset(&mut self) {
        self.port84 = 0;
        self.port86 = 0;
        self.modules.reset();
    }

    fn ram(&self, offset: usize, writable: bool) -> Target {
        if writable {
            Target::Ram(offset)
        } else {
            Target::RamReadOnly(offset)
        }
    }

    fn target(&self, addr: u16) -> Target {
        let a = usize::from(addr);
        let low = a & (BLOCK - 1);
        let four = self.model.four();
        match addr >> 14 {
            0 if self.pio_a & A_RAM0 != 0 => self.ram(low, self.pio_a & A_RAM0_WRITE != 0),
            1 if four && self.port86 & P86_RAM4 != 0 => {
                self.ram(BLOCK + low, self.port86 & P86_RAM4_WRITE != 0)
            }
            2 if self.pio_a & A_IRM != 0 => {
                let plane = if four && addr < 0xa800 {
                    usize::from(self.port84 >> 1 & 3)
                } else {
                    0
                };
                Target::Irm(plane * BLOCK + low)
            }
            2 if four && self.pio_b & B_RAM8 != 0 => {
                let block = match self.model {
                    Model::Kc854 => 2 + usize::from(self.port84 >> 4 & 1),
                    _ => usize::from(self.port84 >> 4),
                };
                self.ram(block * BLOCK + low, self.pio_b & B_RAM8_WRITE != 0)
            }
            3 if addr >= 0xe000 => {
                if self.pio_a & A_CAOS_E != 0 {
                    Target::CaosE(a - 0xe000)
                } else {
                    Target::Outside
                }
            }
            3 if four && self.port86 & P86_CAOS_C != 0 => Target::CaosC(a - 0xc000),
            3 if self.pio_a & A_BASIC != 0 && !self.roms.basic.is_empty() => {
                let segment = if self.roms.basic.len() > 0x2000 {
                    usize::from(self.port86 >> 5 & 3)
                } else {
                    0
                };
                Target::Basic(segment * 0x2000 + (a - 0xc000))
            }
            _ => Target::Outside,
        }
    }

    /// The byte at `addr`. No side effects.
    pub fn read(&self, addr: u16) -> u8 {
        let rom = |r: &[u8], o: usize| r.get(o).copied().unwrap_or(0xff);
        match self.target(addr) {
            Target::Ram(o) | Target::RamReadOnly(o) => self.ram[o],
            Target::Irm(o) => self.irm[o],
            Target::CaosE(o) => rom(&self.roms.caos_e, o),
            Target::CaosC(o) => rom(&self.roms.caos_c, o),
            Target::Basic(o) => rom(&self.roms.basic, o),
            Target::Outside => self.modules.read(addr).unwrap_or(0xff),
        }
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match self.target(addr) {
            Target::Ram(o) => self.ram[o] = v,
            Target::Irm(o) => self.irm[o] = v,
            Target::Outside => self.modules.write(addr, v),
            _ => {}
        }
    }

    /// A debugger's write: RAM even where it is protected, never ROM.
    pub fn poke(&mut self, addr: u16, v: u8) -> bool {
        match self.target(addr) {
            Target::Ram(o) | Target::RamReadOnly(o) => self.ram[o] = v,
            Target::Irm(o) => self.irm[o] = v,
            Target::Outside => return self.modules.poke(addr, v),
            _ => return false,
        }
        true
    }

    /// The IRM as the system sees it at `addr` (`$8000–$BFFF`), whatever
    /// is switched: pixel plane 0, which holds the system variables and
    /// the character buffer.
    pub fn irm0(&self, addr: u16) -> u8 {
        self.irm[usize::from(addr) & (BLOCK - 1)]
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"KCME", 2, |w| {
            w.raw(&self.ram);
            w.raw(&self.irm);
            for b in [self.pio_a, self.pio_b, self.port84, self.port86] {
                w.u8(b);
            }
            self.modules.save(w);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        // 1: no slot $FC.
        let (version, mut r) = r.chunk(*b"KCME", 2)?;
        let mut n = self.clone();
        r.raw_into(&mut n.ram)?;
        r.raw_into(&mut n.irm)?;
        n.pio_a = r.u8()?;
        n.pio_b = r.u8()?;
        n.port84 = r.u8()?;
        n.port86 = r.u8()?;
        n.modules.load(&mut r, if version < 2 { 2 } else { 3 })?;
        *self = n;
        Ok(())
    }
}
