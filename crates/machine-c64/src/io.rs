//! The I/O area `$D000–$DFFF` and the chips behind it.
//!
//! | Range         | Chip                                   |
//! | ------------- | -------------------------------------- |
//! | `$D000–$D3FF` | VIC-II (not yet emulated)              |
//! | `$D400–$D7FF` | SID (not yet emulated)                 |
//! | `$D800–$DBFF` | colour RAM, 4 bits                     |
//! | `$DC00–$DCFF` | CIA1, 16 registers mirrored            |
//! | `$DD00–$DDFF` | CIA2, 16 registers mirrored            |
//! | `$DE00–$DFFF` | I/O1, I/O2 (cartridge port)            |
//!
//! Wiring (C64 schematic 251469): CIA1 IRQ → CPU /IRQ, CIA2 IRQ → CPU /NMI.
//! CIA1 ports scan the keyboard and read the joysticks; CIA2 port A selects
//! the VIC bank and drives the IEC bus. Both TOD inputs get the 50/60 Hz
//! power-line signal.

use chips_cbm::cia::{Cia, CiaModel};
use emu_core::{ClockDivider, Savestate, StateError, StateReader, StateWriter};

use crate::memory::Memory;

/// Placeholder for the value on the data bus when nothing drives it. On the
/// real machine it is the byte the VIC-II fetched in the previous half
/// cycle; that arrives with the VIC-II.
pub const OPEN_BUS: u8 = 0xff;

/// PAL C64 CPU clock and mains frequency for the TOD input.
const CPU_HZ: u32 = 985_248;
const MAINS_HZ: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Io {
    pub cia1: Cia,
    pub cia2: Cia,
    tod: ClockDivider,
}

impl Io {
    pub fn new(model: CiaModel) -> Self {
        Self {
            cia1: Cia::new(model),
            cia2: Cia::new(model),
            tod: ClockDivider::new(MAINS_HZ, CPU_HZ).expect("CPU clock is non-zero"),
        }
    }

    /// Chip clocks for one φ2 cycle, before the CPU's access.
    pub fn tick(&mut self) {
        self.cia1.tick();
        self.cia2.tick();
        for _ in 0..self.tod.tick() {
            self.cia1.tod_tick();
            self.cia2.tod_tick();
        }
    }

    /// External pull-downs on CIA1 ports A/B: keyboard matrix and joysticks
    /// (none yet).
    fn cia1_ext(&self) -> (u8, u8) {
        (0xff, 0xff)
    }

    /// External levels on CIA2 ports: IEC bus inputs and user port (none yet).
    fn cia2_ext(&self) -> (u8, u8) {
        (0xff, 0xff)
    }

    pub fn read(&mut self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.read(reg, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.read(reg, a, b)
            }
            _ => self.peek(mem, addr),
        }
    }

    /// Side-effect-free read of an I/O address.
    pub fn peek(&self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            // Colour RAM: only 4 bits exist, the upper nibble is open bus.
            0xd800..=0xdbff => (OPEN_BUS & 0xf0) | mem.color[usize::from(addr & 0x3ff)],
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.peek(reg, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.peek(reg, a, b)
            }
            // VIC-II, SID, I/O1, I/O2: not yet emulated.
            _ => OPEN_BUS,
        }
    }

    pub fn write(&mut self, mem: &mut Memory, addr: u16, v: u8) {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd800..=0xdbff => mem.color[usize::from(addr & 0x3ff)] = v & 0x0f,
            0xdc00..=0xdcff => self.cia1.write(reg, v),
            0xdd00..=0xddff => self.cia2.write(reg, v),
            _ => {}
        }
    }
}

impl Savestate for Io {
    fn save(&self, w: &mut StateWriter) {
        self.cia1.save(w);
        self.cia2.save(w);
        self.tod.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.cia1.load(r)?;
        self.cia2.load(r)?;
        self.tod.load(r)
    }
}
