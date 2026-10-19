//! The I/O area `$D000–$DFFF` and the chips behind it.
//!
//! | Range         | Chip                                   |
//! | ------------- | -------------------------------------- |
//! | `$D000–$D3FF` | VIC-II, 64 registers mirrored          |
//! | `$D400–$D7FF` | SID (not yet emulated)                 |
//! | `$D800–$DBFF` | colour RAM, 4 bits                     |
//! | `$DC00–$DCFF` | CIA1, 16 registers mirrored            |
//! | `$DD00–$DDFF` | CIA2, 16 registers mirrored            |
//! | `$DE00–$DFFF` | I/O1, I/O2 (cartridge port)            |
//!
//! Wiring (C64 schematic 251469): VIC-II and CIA1 IRQ → CPU /IRQ, CIA2 IRQ
//! → CPU /NMI, VIC-II BA → CPU RDY. CIA1 ports scan the keyboard and read
//! the joysticks; CIA2 port A bits 0–1 select the VIC bank (inverted) and
//! drive the IEC bus. Both TOD inputs get the 50/60 Hz power-line signal.

use chips_cbm::cia::{Cia, CiaModel};
use chips_cbm::vic2::{Vic2, VicMemory};
use emu_core::{ClockDivider, Savestate, StateError, StateReader, StateWriter};

use drives_cbm::IecLines;

use crate::keyboard::Keyboard;
use crate::memory::Memory;

/// PAL C64 CPU clock and mains frequency for the TOD input.
const CPU_HZ: u32 = 985_248;
const MAINS_HZ: u32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Io {
    pub vic: Vic2,
    pub cia1: Cia,
    pub cia2: Cia,
    tod: ClockDivider,
    pub keyboard: Keyboard,
    /// Pins pulled low by the devices in control ports 1 and 2
    /// (`PortLines::pulled_low`), refreshed when their input changes.
    pub port_lines: [u32; 2],
    /// Serial bus levels after the last drive update.
    pub iec: IecLines,
}

/// The VIC-II's view of memory: one 16 KB bank of RAM, with the character
/// ROM at `$1000–$1FFF` in banks 0 and 2, plus colour RAM.
pub struct VicView<'a> {
    pub mem: &'a Memory,
    /// Bank base address: `$0000`, `$4000`, `$8000` or `$C000`.
    pub bank: u16,
}

impl VicMemory for VicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        let addr = addr & 0x3fff;
        if self.bank & 0x4000 == 0 && addr & 0x3000 == 0x1000 {
            self.mem.roms.chargen[usize::from(addr & 0x0fff)]
        } else {
            self.mem.ram[usize::from(self.bank | addr)]
        }
    }

    fn color(&self, index: u16) -> u8 {
        self.mem.color[usize::from(index & 0x3ff)]
    }
}

impl Io {
    pub fn new(model: CiaModel) -> Self {
        Self {
            vic: Vic2::new(),
            cia1: Cia::new(model),
            cia2: Cia::new(model),
            tod: ClockDivider::new(MAINS_HZ, CPU_HZ).expect("CPU clock is non-zero"),
            keyboard: Keyboard::default(),
            port_lines: [0; 2],
            iec: IecLines::default(),
        }
    }

    /// VIC bank selected by CIA2 PA0/PA1 (active low).
    pub fn vic_bank(&self) -> u16 {
        u16::from(!self.cia2.port_a_out() & 3) << 14
    }

    /// The byte on the data bus when nothing else drives it: the VIC-II's
    /// last fetch.
    pub fn open_bus(&self) -> u8 {
        self.vic.last_bus()
    }

    /// CIA clocks for one φ2 cycle, before the CPU's access.
    pub fn tick_cias(&mut self) {
        self.cia1.tick();
        self.cia2.tick();
        for _ in 0..self.tod.tick() {
            self.cia1.tod_tick();
            self.cia2.tod_tick();
        }
    }

    /// Levels on CIA1 ports A/B: control port 2 on PA0–4, control port 1 on
    /// PB0–4 (up, down, left, right, fire), then the keyboard matrix.
    fn cia1_ext(&self) -> (u8, u8) {
        let joy = |lines: u32| -> u8 { !((lines & 0x0f) as u8 | ((lines >> 1) & 0x10) as u8) };
        let a = self.cia1.port_a_out() & joy(self.port_lines[1]);
        let b = self.cia1.port_b_out() & joy(self.port_lines[0]);
        self.keyboard.resolve(a, b)
    }

    /// External levels on CIA2 ports: PA6 CLK in and PA7 DATA in read the
    /// serial bus line level (1 = released); the user port is open.
    fn cia2_ext(&self) -> (u8, u8) {
        let pa = 0x3f | if self.iec.clk { 0 } else { 0x40 } | if self.iec.data { 0 } else { 0x80 };
        (pa, 0xff)
    }

    pub fn read(&mut self, mem: &Memory, addr: u16) -> u8 {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd000..=0xd3ff => self.vic.read(addr as u8),
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
            0xd000..=0xd3ff => self.vic.peek(addr as u8),
            // Colour RAM: only 4 bits exist, the upper nibble is open bus.
            0xd800..=0xdbff => (self.open_bus() & 0xf0) | mem.color[usize::from(addr & 0x3ff)],
            0xdc00..=0xdcff => {
                let (a, b) = self.cia1_ext();
                self.cia1.peek(reg, a, b)
            }
            0xdd00..=0xddff => {
                let (a, b) = self.cia2_ext();
                self.cia2.peek(reg, a, b)
            }
            // SID, I/O1, I/O2: not yet emulated.
            _ => self.open_bus(),
        }
    }

    pub fn write(&mut self, mem: &mut Memory, addr: u16, v: u8) {
        let reg = addr as u8 & 0x0f;
        match addr {
            0xd000..=0xd3ff => self.vic.write(addr as u8, v),
            0xd800..=0xdbff => mem.color[usize::from(addr & 0x3ff)] = v & 0x0f,
            0xdc00..=0xdcff => self.cia1.write(reg, v),
            0xdd00..=0xddff => self.cia2.write(reg, v),
            _ => {}
        }
    }
}

impl Savestate for Io {
    fn save(&self, w: &mut StateWriter) {
        self.vic.save(w);
        self.cia1.save(w);
        self.cia2.save(w);
        self.tod.save(w);
        w.chunk(*b"C64K", 1, |w| {
            w.u64(self.keyboard.matrix);
            w.bool(self.keyboard.restore);
            w.u32(self.port_lines[0]);
            w.u32(self.port_lines[1]);
            w.bool(self.iec.atn);
            w.bool(self.iec.clk);
            w.bool(self.iec.data);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.vic.load(r)?;
        self.cia1.load(r)?;
        self.cia2.load(r)?;
        self.tod.load(r)?;
        let (_, mut c) = r.chunk(*b"C64K", 1)?;
        self.keyboard.matrix = c.u64()?;
        self.keyboard.restore = c.bool()?;
        self.port_lines = [c.u32()?, c.u32()?];
        self.iec = IecLines {
            atn: c.bool()?,
            clk: c.bool()?,
            data: c.bool()?,
        };
        Ok(())
    }
}
