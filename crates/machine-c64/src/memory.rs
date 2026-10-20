//! C64 memory: 64 KB RAM, ROMs, 1 KB colour RAM, the 6510 port and the PLA.
//!
//! The CPU view is resolved through the PLA table into one of the
//! [`Bank`]s. I/O (`$D000–$DFFF` when mapped) is dispatched by the machine,
//! not here, because it belongs to the chips.

use cpu_6502::IoPort;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

use chips_cbm::pla::{self, Bank, Lines, Map};

pub const KERNAL_SIZE: usize = 0x2000;
pub const BASIC_SIZE: usize = 0x2000;
pub const CHARGEN_SIZE: usize = 0x1000;

/// External levels on the 6510 port inputs: LORAM/HIRAM/CHAREN have
/// pull-ups, bit 4 is the cassette sense line (high = no key pressed, see
/// [`Memory::cassette_sense`]), bit 5
/// (motor) reads low through the motor driver. Bits 3, 6 and 7 float: they
/// keep the level last driven onto them (Lorenz cpuport). The slow fall-off
/// of that charge is not modelled yet.
const PORT_PULLUPS: u8 = 0x17;
const PORT_FLOATING: u8 = 0xc8;

#[derive(Clone)]
pub struct Roms {
    pub kernal: Box<[u8; KERNAL_SIZE]>,
    pub basic: Box<[u8; BASIC_SIZE]>,
    pub chargen: Box<[u8; CHARGEN_SIZE]>,
}

#[derive(Clone)]
pub struct Memory {
    pub ram: Box<[u8; 0x10000]>,
    /// Low nibble only; the high nibble is not stored (open bus on reads).
    pub color: Box<[u8; 0x400]>,
    pub roms: Roms,
    pub port: IoPort,
    /// Cartridge port lines, true = high (no cartridge).
    pub game: bool,
    pub exrom: bool,
    /// Levels held on the floating port bits 3, 6, 7.
    floating: u8,
    /// A datasette key is down: port bit 4 reads low.
    pub cassette_sense: bool,
    /// Current PLA mapping, recomputed when its inputs change.
    map: Map,
}

impl Memory {
    pub fn new(roms: Roms) -> Self {
        let mut m = Self {
            ram: Box::new([0; 0x10000]),
            color: Box::new([0; 0x400]),
            roms,
            port: IoPort::new(0x3f),
            game: true,
            exrom: true,
            floating: 0,
            cassette_sense: false,
            map: pla::TABLE[31],
        };
        m.update_map();
        m
    }

    /// RESET: the 6510 port returns to all inputs; `hard` (power cycle)
    /// also clears RAM and colour RAM.
    pub fn reset(&mut self, hard: bool) {
        self.port = IoPort::new(0x3f);
        if hard {
            self.ram.fill(0);
            self.color.fill(0);
        }
        self.update_map();
    }

    fn update_map(&mut self) {
        let lines = Lines::new(self.port.outputs(), self.game, self.exrom);
        self.map = pla::TABLE[usize::from(lines.mode())];
    }

    /// PLA mode 0–31 currently selected.
    pub fn mode(&self) -> u8 {
        Lines::new(self.port.outputs(), self.game, self.exrom).mode()
    }

    #[inline]
    pub fn bank(&self, addr: u16) -> Bank {
        self.map[usize::from(addr >> 12)]
    }

    /// Ultimax mode: /GAME low, /EXROM high.
    pub fn ultimax(&self) -> bool {
        !self.game && self.exrom
    }

    pub fn set_cartridge_lines(&mut self, game: bool, exrom: bool) {
        self.game = game;
        self.exrom = exrom;
        self.update_map();
    }

    /// The 6510 port read value for `$00`/`$01`.
    pub fn port_read(&self, addr: u16) -> u8 {
        let sense = if self.cassette_sense { 0x10 } else { 0 };
        self.port.read(
            addr as u8,
            (PORT_PULLUPS & !sense) | (self.floating & PORT_FLOATING),
        )
    }

    /// CPU write to `$00`/`$01`: goes to the port and, as on the real board,
    /// also to the RAM cell underneath.
    pub fn port_write(&mut self, addr: u16, v: u8) {
        self.port.write(addr as u8, v);
        let driven = self.port.ddr & PORT_FLOATING;
        self.floating = (self.floating & !driven) | (self.port.data & driven);
        self.ram[usize::from(addr)] = v;
        self.update_map();
    }

    /// CPU-view read of a non-I/O address. `None` for I/O and open bus,
    /// which the machine resolves. Side-effect free.
    #[inline]
    pub fn read(&self, addr: u16) -> Option<u8> {
        let a = usize::from(addr);
        match self.bank(addr) {
            Bank::Ram => Some(if addr < 2 {
                self.port_read(addr)
            } else {
                self.ram[a]
            }),
            Bank::Basic => Some(self.roms.basic[a & 0x1fff]),
            Bank::Kernal => Some(self.roms.kernal[a & 0x1fff]),
            Bank::Char => Some(self.roms.chargen[a & 0x0fff]),
            // The machine asks the cartridge (or leaves the bus open).
            Bank::Io | Bank::RomL | Bank::RomH | Bank::Open => None,
        }
    }

    /// CPU-view write of a non-I/O address. Writes "under" ROM land in RAM,
    /// also under cartridge ROM except in Ultimax mode (no RAM there);
    /// Ultimax open areas ignore them. Returns false for I/O, which the
    /// machine handles. The cartridge sees ROML/ROMH writes through the
    /// machine.
    #[inline]
    pub fn write(&mut self, addr: u16, v: u8) -> bool {
        match self.bank(addr) {
            Bank::Io => false,
            Bank::Open => true,
            Bank::RomL | Bank::RomH if self.ultimax() => true,
            Bank::RomL | Bank::RomH => {
                self.ram[usize::from(addr)] = v;
                true
            }
            Bank::Ram | Bank::Basic | Bank::Kernal | Bank::Char => {
                if addr < 2 {
                    self.port_write(addr, v);
                } else {
                    self.ram[usize::from(addr)] = v;
                }
                true
            }
        }
    }
}

const TAG: [u8; 4] = *b"C64M";
const VERSION: u16 = 2;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram[..]);
            w.raw(&self.color[..]);
            w.bool(self.game);
            w.bool(self.exrom);
            w.u8(self.floating);
        });
        self.port.save(w);
    }

    /// Restores RAM, colour RAM, port and cartridge lines; ROMs come from the
    /// configuration, not the state. On error the memory may be partially
    /// loaded: callers load into a copy.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        if version < VERSION {
            return Err(StateError::UnsupportedVersion { tag: TAG, version });
        }
        c.raw_into(&mut self.ram[..])?;
        c.raw_into(&mut self.color[..])?;
        if self.color.iter().any(|&v| v > 0x0f) {
            return Err(StateError::Invalid("colour RAM"));
        }
        self.game = c.bool()?;
        self.exrom = c.bool()?;
        self.floating = c.u8()? & PORT_FLOATING;
        self.port.load(r)?;
        self.update_map();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn test_roms() -> Roms {
        Roms {
            kernal: Box::new([0xe0; KERNAL_SIZE]),
            basic: Box::new([0xb0; BASIC_SIZE]),
            chargen: Box::new([0xc0; CHARGEN_SIZE]),
        }
    }

    #[test]
    fn power_on_has_all_roms_and_inputs_high() {
        let m = Memory::new(test_roms());
        // DDR = 0 after reset: every port pin is an input, pulled high.
        assert_eq!(m.mode(), 31);
        assert_eq!(m.read(0xa000), Some(0xb0));
        assert_eq!(m.read(0xe000), Some(0xe0));
        assert_eq!(m.read(0xd000), None, "I/O");
        assert_eq!(m.read(0x8000), Some(0));
    }

    #[test]
    fn writes_under_rom_land_in_ram() {
        let mut m = Memory::new(test_roms());
        assert!(m.write(0xa123, 0x42));
        assert_eq!(m.read(0xa123), Some(0xb0), "ROM still visible");
        m.port_write(0, 0x07);
        m.port_write(1, 0x34); // all RAM (mode 28)
        assert_eq!(m.mode(), 28);
        assert_eq!(m.read(0xa123), Some(0x42));
        assert_eq!(m.read(0xd000), Some(0), "RAM under I/O");
    }

    #[test]
    fn port_banking_switches_char_rom_in() {
        let mut m = Memory::new(test_roms());
        m.port_write(0, 0x2f);
        m.port_write(1, 0x33); // CHAREN low
        assert_eq!(m.mode(), 27);
        assert_eq!(m.read(0xd000), Some(0xc0));
        assert!(m.write(0xd000, 9), "char ROM write goes to RAM");
        assert_eq!(m.ram[0xd000], 9);
    }

    #[test]
    fn port_registers_and_ram_underneath() {
        let mut m = Memory::new(test_roms());
        m.write(0, 0x2f);
        m.write(1, 0x37);
        assert_eq!(m.read(0), Some(0x2f));
        // Outputs 0–3 and 5 from the latch, bit 4 input pulled high.
        assert_eq!(m.read(1), Some(0x37));
        assert_eq!(m.ram[0], 0x2f, "port writes also reach RAM");
        assert_eq!(m.ram[1], 0x37);
    }

    #[test]
    fn ultimax_mode_leaves_open_bus() {
        let mut m = Memory::new(test_roms());
        m.set_cartridge_lines(false, true);
        assert_eq!(m.mode(), 23);
        assert_eq!(m.read(0x1000), None);
        assert_eq!(m.read(0x0fff), Some(0));
        assert!(m.write(0x4000, 1));
        assert_eq!(m.ram[0x4000], 0, "open bus swallows writes");
    }

    #[test]
    fn savestate_round_trip() {
        let mut m = Memory::new(test_roms());
        m.write(0x1234, 0x56);
        m.color[5] = 0x0e;
        m.port_write(0, 0x2f);
        m.port_write(1, 0x35);
        let mut w = StateWriter::new();
        m.save(&mut w);
        let data = w.into_bytes();

        let mut n = Memory::new(test_roms());
        n.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(n.ram[..], m.ram[..]);
        assert_eq!(n.color[..], m.color[..]);
        assert_eq!(n.mode(), m.mode());
        assert_eq!(n.port, m.port);
    }
}
