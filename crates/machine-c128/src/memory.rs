//! C128 memory: two 64 KB RAM banks, the ROMs, 2 KB colour RAM, the MMU
//! and the 8502's I/O port, seen three ways:
//!
//! - **8502, C128 mode:** through the MMU (`chips_cbm::mmu`). `$FF00–$FF04`
//!   is always the MMU; writes to ROM areas go to the RAM underneath.
//! - **8502, C64 mode** (MCR bit 6): the C64 PLA's map from the port bits
//!   LORAM/HIRAM/CHAREN, RAM bank 0, the C64 ROM (BASIC `$A000`, KERNAL
//!   `$E000`) and the C64 half of the character ROM.
//! - **Z80:** the MMU's RAM and ROM map, except that `$0000–$0FFF` shows
//!   the Z80 BIOS (KERNAL ROM `$D000–$DFFF`) in RAM bank 0, and
//!   `$D000–$DFFF` is always RAM (the Z80 reaches I/O through its I/O
//!   instructions).
//!
//! 8502 port (`$00/$01`, 7 bits): 0 LORAM, 1 HIRAM, 2 CHAREN in C64 mode;
//! in C128 mode bit 0 selects the colour RAM bank the CPU sees, bit 1 the
//! one the VIC sees, bit 2 hides the character ROM from the VIC; 3–5
//! datasette, 6 CAPS LOCK (input, 0 = down). Source: Commodore 128
//! Programmer's Reference Guide, chapter 13 (memory map, `$01`).

use chips_cbm::mmu::{Mmu, Target};
use chips_cbm::vic2::VicMemory;
use cpu_6502::IoPort;
use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub const ROM_16K: usize = 0x4000;
pub const CHARGEN_SIZE: usize = 0x2000;
pub const FUNCTION_SIZE: usize = 0x8000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roms {
    pub kernal: Box<[u8; ROM_16K]>,
    pub basic_lo: Box<[u8; ROM_16K]>,
    pub basic_hi: Box<[u8; ROM_16K]>,
    /// 4 KB C64 set, then 4 KB C128 set.
    pub chargen: Box<[u8; CHARGEN_SIZE]>,
    /// C64 mode: 8 KB BASIC, then 8 KB KERNAL.
    pub c64: Box<[u8; ROM_16K]>,
    /// Internal function ROM socket (`$8000–$FFFF`), if fitted.
    pub function: Option<Box<[u8; FUNCTION_SIZE]>>,
}

/// What a CPU read resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Value(u8),
    /// The I/O area: the machine's chips decide.
    Io(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Banks 0 and 1 (banks 2 and 3 mirror them).
    pub ram: Vec<u8>,
    /// 2 × 1 KB, 4 bits.
    pub color: Vec<u8>,
    pub roms: Roms,
    pub mmu: Mmu,
    pub port: IoPort,
    /// CAPS LOCK down (8502 port bit 6 input).
    pub caps_lock: bool,
    /// A datasette key is down (port bit 4 reads low).
    pub cassette_sense: bool,
}

impl Memory {
    pub fn new(roms: Roms) -> Self {
        Self {
            ram: vec![0; 0x20000],
            color: vec![0; 0x800],
            roms,
            mmu: Mmu::new(),
            port: IoPort::new(0x7f),
            caps_lock: false,
            cassette_sense: false,
        }
    }

    pub fn reset(&mut self, hard: bool) {
        if hard {
            self.ram.fill(0);
            self.color.fill(0);
        }
        self.mmu.reset();
        self.port = IoPort::new(0x7f);
    }

    pub fn ram_at(&self, bank: u8, addr: u16) -> u8 {
        self.ram[(usize::from(bank & 1) << 16) | usize::from(addr)]
    }

    fn set_ram(&mut self, bank: u8, addr: u16, v: u8) {
        self.ram[(usize::from(bank & 1) << 16) | usize::from(addr)] = v;
    }

    /// External levels on the 8502 port's inputs: CAPS LOCK on bit 6, the
    /// datasette sense line on bit 4.
    fn port_pins(&self) -> u8 {
        let mut pins = 0x7f;
        if self.caps_lock {
            pins &= !0x40;
        }
        if self.cassette_sense {
            pins &= !0x10;
        }
        pins
    }

    fn port_out(&self) -> u8 {
        self.port.outputs()
    }

    /// Colour RAM index for a CPU access to `$D800–$DBFF`.
    pub fn color_index(&self, addr: u16) -> usize {
        let bank = if self.mmu.c64_mode() {
            0
        } else {
            usize::from(self.port_out() & 1)
        };
        (bank << 10) | usize::from(addr & 0x3ff)
    }

    /// Side-effect-free 8502 read (I/O left to the machine).
    pub fn cpu_read(&self, a: u16) -> Read {
        if a < 2 {
            return Read::Value(self.port.read(a as u8, self.port_pins()));
        }
        if self.mmu.c64_mode() {
            return self.c64_read(a);
        }
        Read::Value(match self.mmu.translate(a) {
            Target::Ram { bank, addr } => self.ram_at(bank, addr),
            Target::BasicLo(o) => self.roms.basic_lo[usize::from(o)],
            Target::BasicHi(o) => self.roms.basic_hi[usize::from(o)],
            Target::Function {
                external: false,
                offset,
            } => self
                .roms
                .function
                .as_ref()
                .map_or(0xff, |f| f[usize::from(offset)]),
            // No external function ROM (cartridge) fitted.
            Target::Function { external: true, .. } => 0xff,
            Target::Kernal(o) => self.roms.kernal[usize::from(o)],
            Target::Chargen(o) => self.roms.chargen[0x1000 | usize::from(o & 0x0fff)],
            Target::Io(a) => return Read::Io(a),
            Target::Mmu(r) => match r {
                0 => self.mmu.cr,
                r => self.mmu.pcr[usize::from(r - 1)],
            },
        })
    }

    fn c64_read(&self, a: u16) -> Read {
        let p = self.port_out();
        let (lo, hi, charen) = (p & 1 != 0, p & 2 != 0, p & 4 != 0);
        Read::Value(match a {
            0xa000..=0xbfff if lo && hi => self.roms.c64[usize::from(a - 0xa000)],
            0xd000..=0xdfff if lo || hi => {
                if charen {
                    return Read::Io(a);
                }
                self.roms.chargen[usize::from(a & 0x0fff)]
            }
            0xe000..=0xffff if hi => self.roms.c64[0x2000 + usize::from(a - 0xe000)],
            _ => self.ram_at(0, a),
        })
    }

    /// 8502 write; returns false if it goes to I/O (the machine's chips).
    pub fn cpu_write(&mut self, a: u16, v: u8) -> bool {
        if a < 2 {
            self.port.write(a as u8, v);
            // The written cell of RAM gets the value too (as on the C64).
            self.set_ram(0, a, v);
            return true;
        }
        if self.mmu.c64_mode() {
            let p = self.port_out();
            let io = (p & 3 != 0) && p & 4 != 0;
            if (0xd000..=0xdfff).contains(&a) && io {
                return false;
            }
            self.set_ram(0, a, v);
            return true;
        }
        match self.mmu.translate(a) {
            Target::Io(_) => false,
            Target::Mmu(r) => {
                self.mmu.write_lcr(r as u8, v);
                true
            }
            _ => {
                let (bank, addr) = self.mmu.ram(a);
                self.set_ram(bank, addr, v);
                true
            }
        }
    }

    /// Whether the Z80 sees its BIOS at `$0000–$0FFF`: in RAM bank 0,
    /// whatever the ROM bits say (the BIOS switches to all-RAM first thing
    /// and keeps running from ROM).
    fn z80_bios(&self) -> bool {
        (self.mmu.cr >> 6) & 3 == 0
    }

    pub fn z80_read(&self, a: u16) -> u8 {
        if a < 0x1000 && self.z80_bios() {
            return self.roms.kernal[0x1000 + usize::from(a)];
        }
        match self.mmu.translate(a) {
            Target::Io(a) => {
                let (bank, addr) = self.mmu.ram(a);
                self.ram_at(bank, addr)
            }
            _ => match self.cpu_read_c128(a) {
                Read::Value(v) => v,
                Read::Io(_) => 0xff,
            },
        }
    }

    /// The C128-mode map regardless of the C64 mode bit (for the Z80).
    fn cpu_read_c128(&self, a: u16) -> Read {
        if self.mmu.c64_mode() {
            let (bank, addr) = self.mmu.ram(a);
            return Read::Value(self.ram_at(bank, addr));
        }
        self.cpu_read(a)
    }

    pub fn z80_write(&mut self, a: u16, v: u8) {
        if let Target::Mmu(r) = self.mmu.translate(a) {
            self.mmu.write_lcr(r as u8, v);
            return;
        }
        let (bank, addr) = self.mmu.ram(a);
        self.set_ram(bank, addr, v);
    }
}

/// The VIC's view: one 16 KB bank (`base`, from CIA2 port A) of the RAM
/// bank the MMU gives the VIC, the character ROM at `$1000–$1FFF` of banks
/// 0 and 2 (unless hidden in C128 mode), and its colour RAM bank.
pub struct VicView<'a> {
    pub mem: &'a Memory,
    pub base: u16,
}

impl VicMemory for VicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        let m = self.mem;
        let addr = addr & 0x3fff;
        let c64 = m.mmu.c64_mode();
        let rom_visible = c64 || m.port_out() & 4 == 0;
        if self.base & 0x4000 == 0 && addr & 0x3000 == 0x1000 && rom_visible {
            let half = if c64 { 0 } else { 0x1000 };
            return m.roms.chargen[half | usize::from(addr & 0x0fff)];
        }
        let bank = if c64 { 0 } else { m.mmu.vic_bank() };
        m.ram_at(bank, self.base | addr)
    }

    fn color(&self, index: u16) -> u8 {
        let m = self.mem;
        let bank = if m.mmu.c64_mode() {
            0
        } else {
            usize::from((m.port_out() >> 1) & 1)
        };
        m.color[(bank << 10) | usize::from(index & 0x3ff)]
    }
}

const TAG: [u8; 4] = *b"C8MM";
const VERSION: u16 = 1;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram);
            w.raw(&self.color);
            w.bool(self.caps_lock);
        });
        self.mmu.save(w);
        self.port.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut ram = vec![0; self.ram.len()];
        c.raw_into(&mut ram)?;
        let mut color = vec![0; self.color.len()];
        c.raw_into(&mut color)?;
        for v in &mut color {
            *v &= 0x0f;
        }
        let caps_lock = c.bool()?;
        let mut mmu = self.mmu.clone();
        mmu.load(r)?;
        let mut port = self.port;
        port.load(r)?;
        self.ram = ram;
        self.color = color;
        self.caps_lock = caps_lock;
        self.mmu = mmu;
        self.port = port;
        Ok(())
    }
}
