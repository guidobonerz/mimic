//! C128 memory: two 64 KB RAM banks, the ROMs, 2 KB colour RAM, the MMU
//! and the 8502's I/O port, seen three ways:
//!
//! - **8502, C128 mode:** through the MMU (`chips_cbm::mmu`). `$FF00–$FF04`
//!   is always the MMU; writes to ROM areas go to the RAM underneath.
//! - **8502, C64 mode** (MCR bit 6): the C64 PLA's map from the port bits
//!   LORAM/HIRAM/CHAREN and the cartridge lines GAME/EXROM
//!   (`chips_cbm::pla`), RAM bank 0, the C64 ROM (BASIC `$A000`, KERNAL
//!   `$E000`), the C64 half of the character ROM and the cartridge's
//!   ROML/ROMH (which the machine reads).
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
use chips_cbm::pla::{self, Bank, Lines};
use chips_cbm::vic2::VicMemory;
use cpu_6502::IoPort;
use devices_cartridge::ExpansionCart;
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
    /// The I/O area: the machine's chips (and the cartridge at
    /// `$DE00–$DFFF`) decide.
    Io(u16),
    /// C64 mode: the cartridge's ROML or ROMH.
    RomL(u16),
    RomH(u16),
    /// C64 Ultimax mode: nothing drives the bus.
    Open,
    /// C128 mode: the external function ROM (a cartridge) at this offset
    /// from `$8000`; `$FF` without one.
    Function(u16),
}

/// Where a Z80 access goes ([`Memory::z80_access`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Z80Access {
    Ram {
        bank: u8,
        addr: u16,
    },
    /// The Z80 BIOS in the KERNAL ROM (C128 mode, bank 0, `$0000–$0FFF`).
    Bios(u8),
    /// Another ROM byte (C128 ROMs, C64 mode BASIC/KERNAL/character ROM).
    Rom(u8),
    /// Colour RAM index (both banks).
    Color(usize),
    /// An I/O chip at this address.
    Io(u16),
    /// MMU load configuration register `$FF00 + n`.
    Mmu(u16),
    /// C64 mode, a memory cycle at an I/O address: reads the RAM, writes
    /// the chip at `io`.
    IoUnderRam {
        bank: u8,
        addr: u16,
        io: u16,
    },
}

/// Where a CPU write goes besides memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Done (RAM, MMU, or nowhere).
    Done,
    /// The I/O area.
    Io,
    /// C64 mode: under cartridge ROML/ROMH (also seen by the cartridge).
    RomL,
    RomH,
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
    /// Cartridge line levels (true = high = not pulled), from the machine.
    pub game: bool,
    pub exrom: bool,
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
            game: true,
            exrom: true,
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
    /// The CPU's colour RAM index: port bit 0 picks the bank in C128 mode;
    /// C64 mode stays on bank 1 and no port write switches it (VICE
    /// `c128/c64modemmu` 15, 16; c64modez80-04 for the Z80).
    pub fn color_index(&self, addr: u16) -> usize {
        let bank = if self.mmu.c64_mode() {
            1
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
            // The cartridge's, if it has one.
            Target::Function {
                external: true,
                offset,
            } => return Read::Function(offset),
            Target::Kernal(o) => self.roms.kernal[usize::from(o)],
            Target::Chargen(o) => self.roms.chargen[0x1000 | usize::from(o & 0x0fff)],
            Target::Io(a) => return Read::Io(a),
            Target::Mmu(r) => match r {
                0 => self.mmu.cr,
                r => self.mmu.pcr[usize::from(r - 1)],
            },
        })
    }

    /// The C64-mode PLA page `a` falls in.
    fn c64_bank(&self, a: u16) -> Bank {
        let lines = Lines::new(self.port_out(), self.game, self.exrom);
        pla::TABLE[usize::from(lines.mode())][usize::from(a >> 12)]
    }

    /// C64 mode with GAME low and EXROM high.
    pub fn ultimax(&self) -> bool {
        self.mmu.c64_mode() && !self.game && self.exrom
    }

    /// C64-mode read. RAM is the MMU's as the C128 mode left it: the
    /// configuration register's RAM bank and shared RAM stay in force, and
    /// relocation target pages still land in page 0/1 (the MMU registers
    /// are hidden, not reset; [`chips_cbm::mmu::Mmu::ram_c64`]).
    fn c64_read(&self, a: u16) -> Read {
        let o = usize::from(a & 0x1fff);
        Read::Value(match self.c64_bank(a) {
            Bank::Ram => {
                let (bank, addr) = self.mmu.ram_c64(a);
                self.ram_at(bank, addr)
            }
            Bank::Basic => self.roms.c64[o],
            Bank::Kernal => self.roms.c64[0x2000 + o],
            Bank::Char => self.roms.chargen[o & 0x0fff],
            Bank::Io => return Read::Io(a),
            Bank::RomL => return Read::RomL(a),
            Bank::RomH => return Read::RomH(a),
            Bank::Open => return Read::Open,
        })
    }

    /// 8502 write to the processor port at `$00/$01`: the port takes `v`;
    /// the RAM cell the MMU maps the address to (zero page relocation
    /// included) gets `bus`, the byte the data bus held — the CPU drives
    /// the port, not the bus (VICE `c128/ram0001` test1, `ram0001mmu`
    /// c128modebank0001: $3001 in bank 1 gets the VIC-IIe's last byte).
    pub fn port_write_with_bus(&mut self, a: u16, v: u8, bus: u8) {
        self.port.write(a as u8, v);
        let (bank, addr) = if self.mmu.c64_mode() {
            self.mmu.ram_c64(a)
        } else {
            self.mmu.ram(a)
        };
        self.set_ram(bank, addr, bus);
    }

    /// 8502 write. Writes under ROM land in RAM; in C64 mode also under
    /// cartridge ROM, except in Ultimax mode (no RAM there), whose open
    /// areas ignore them. I/O and the cartridge are the machine's.
    pub fn cpu_write(&mut self, a: u16, v: u8) -> Write {
        if a < 2 {
            // The machine calls `port_write_with_bus` for these.
            self.port_write_with_bus(a, v, v);
            return Write::Done;
        }
        if self.mmu.c64_mode() {
            let bank = self.c64_bank(a);
            match bank {
                Bank::Io => return Write::Io,
                Bank::Open => return Write::Done,
                Bank::RomL | Bank::RomH if self.ultimax() => {}
                _ => {
                    let (bank, addr) = self.mmu.ram_c64(a);
                    self.set_ram(bank, addr, v);
                }
            }
            return match bank {
                Bank::RomL => Write::RomL,
                Bank::RomH => Write::RomH,
                _ => Write::Done,
            };
        }
        match self.mmu.translate(a) {
            Target::Io(_) => Write::Io,
            Target::Mmu(r) => {
                self.mmu.write_lcr(r as u8, v);
                Write::Done
            }
            _ => {
                let (bank, addr) = self.mmu.ram(a);
                self.set_ram(bank, addr, v);
                Write::Done
            }
        }
    }

    /// Whether the Z80 sees its BIOS at `$0000–$0FFF`: in RAM bank 0,
    /// whatever the ROM bits say (the BIOS switches to all-RAM first thing
    /// and keeps running from ROM).
    fn z80_bios(&self) -> bool {
        (self.mmu.cr >> 6) & 3 == 0
    }

    /// Colour RAM index of a Z80 access: the bank as for the 8502.
    fn z80_color_index(&self, addr: u16) -> usize {
        self.color_index(addr)
    }

    /// Where a Z80 access goes: `io` for an IN/OUT cycle. Rules from the
    /// VICE test programs `c128/c64modemmu/c128modez80-*` and
    /// `c64modez80-*`:
    /// - C128 mode: with RAM bank 0 in the CR, `$0000–$0FFF` is moved to
    ///   `$D000–$DFFF`: memory reads see the Z80 BIOS (KERNAL ROM
    ///   `$D000`), writes and I/O reads the RAM there (16, 17, 21, 25;
    ///   not when shared RAM alone makes it bank 0: 18, 20). While the CR
    ///   shows I/O, `$1000–$13FF` is the colour RAM, in memory and I/O
    ///   cycles and over a relocated page (02, 03, 05–08). I/O cycles reach
    ///   the chips at `$D000–$DFFF` whatever the CR says; reads of the MMU's
    ///   own `$D5xx` only while it shows I/O (otherwise RAM, 10), writes
    ///   always (tstz80bk). Other I/O
    ///   cycles read RAM through the MMU, shared RAM and page relocation
    ///   included (12–14); `$FF00–$FF04` are the MMU's in both kinds (11).
    /// - C64 mode: no BIOS; memory and I/O cycles both use the C64 map,
    ///   ROMs included (00, 06); at the I/O addresses memory writes reach
    ///   the chips but memory reads the RAM (01, 02); the colour RAM is
    ///   also at `$1000–$13FF` while I/O is visible (03, 05).
    pub fn z80_access(&self, a: u16, io: bool) -> Z80Access {
        if (0xff00..=0xff04).contains(&a) && !self.mmu.c64_mode() {
            return Z80Access::Mmu(a - 0xff00);
        }
        if self.mmu.c64_mode() {
            // I/O cycles reach the chips even with no I/O in the C64 map
            // (c64modez80-02: $01 = $30).
            if io && (0xd000..=0xdfff).contains(&a) {
                if (0xd800..=0xdbff).contains(&a) {
                    return Z80Access::Color(self.z80_color_index(a));
                }
                return Z80Access::Io(a);
            }
            let io_visible = self.c64_bank(0xd000) == Bank::Io;
            if (0x1000..=0x13ff).contains(&a) && io_visible {
                return Z80Access::Color(self.z80_color_index(a));
            }
            return match self.c64_read(a) {
                Read::Io(a) if (0xd800..=0xdbff).contains(&a) && io => {
                    Z80Access::Color(self.z80_color_index(a))
                }
                Read::Io(a) if io => Z80Access::Io(a),
                // Memory cycles: writes reach the chips, reads the RAM
                // (c64modez80-01, -02).
                Read::Io(_) => {
                    let (bank, addr) = self.mmu.ram_c64(a);
                    Z80Access::IoUnderRam { bank, addr, io: a }
                }
                Read::Value(v) => match self.c64_bank(a) {
                    Bank::Ram => {
                        let (bank, addr) = self.mmu.ram_c64(a);
                        Z80Access::Ram { bank, addr }
                    }
                    _ => Z80Access::Rom(v),
                },
                _ => Z80Access::Rom(0xff),
            };
        }
        let io_enabled = self.mmu.cr & 0x01 == 0;
        if (0x1000..=0x13ff).contains(&a) && io_enabled {
            return Z80Access::Color(self.z80_color_index(a));
        }
        if a < 0x1000 && self.z80_bios() {
            if !io {
                return Z80Access::Bios(self.roms.kernal[0x1000 + usize::from(a)]);
            }
            let (bank, addr) = self.mmu.ram(0xd000 | a);
            return Z80Access::Ram { bank, addr };
        }
        // I/O cycles reach the chips whatever the CR says (the tests
        // report through $D7FF with I/O switched off), except the MMU's own
        // registers, which follow the CR's I/O bit (10).
        if io && (0xd000..=0xdfff).contains(&a) && (io_enabled || !(0xd500..=0xd5ff).contains(&a)) {
            if (0xd800..=0xdbff).contains(&a) {
                return Z80Access::Color(self.z80_color_index(a));
            }
            return Z80Access::Io(a);
        }
        if !io {
            if let Read::Value(v) = self.cpu_read(a) {
                if !matches!(self.mmu.translate(a), Target::Ram { .. } | Target::Io(_)) {
                    return Z80Access::Rom(v);
                }
            }
        }
        let (bank, addr) = self.mmu.ram(a);
        Z80Access::Ram { bank, addr }
    }

    /// A Z80 read that needs no I/O chip (`None` for one).
    pub fn z80_read_value(&self, a: u16, io: bool) -> Option<u8> {
        Some(match self.z80_access(a, io) {
            Z80Access::Ram { bank, addr } | Z80Access::IoUnderRam { bank, addr, .. } => {
                self.ram_at(bank, addr)
            }
            Z80Access::Bios(v) | Z80Access::Rom(v) => v,
            Z80Access::Color(i) => self.color[i] | 0xf0,
            Z80Access::Mmu(0) => self.mmu.cr,
            Z80Access::Mmu(r) => self.mmu.pcr[usize::from(r - 1)],
            Z80Access::Io(_) => return None,
        })
    }

    /// A Z80 write; returns the I/O address for the machine's chips.
    pub fn z80_store(&mut self, a: u16, v: u8, io: bool) -> Option<u16> {
        // OUT reaches the MMU's registers even with I/O switched off in the
        // CR; only reads then see RAM (c128modez80-10, z80/mmu_zp_sp
        // tstz80bk: P0 set back with CR = $BF).
        if io && (0xd500..=0xd5ff).contains(&a) && !self.mmu.c64_mode() {
            return Some(a);
        }
        match self.z80_access(a, io) {
            Z80Access::Ram { bank, addr } => self.set_ram(bank, addr, v),
            Z80Access::Color(i) => self.color[i] = v & 0x0f,
            Z80Access::Mmu(r) => self.mmu.write_lcr(r as u8, v),
            Z80Access::Io(a) | Z80Access::IoUnderRam { io: a, .. } => return Some(a),
            // Under ROM: the RAM there (C128 mode, bank 0: moved to $Dxxx).
            Z80Access::Bios(_) => {
                let (bank, addr) = self.mmu.ram(0xd000 | a);
                self.set_ram(bank, addr, v);
            }
            Z80Access::Rom(_) => {
                let (bank, addr) = if self.mmu.c64_mode() {
                    self.mmu.ram_c64(a)
                } else {
                    self.mmu.ram(a)
                };
                self.set_ram(bank, addr, v);
            }
        }
        None
    }

    /// Memory read as the Z80 sees it (debugger, tests): I/O chips read as
    /// RAM.
    pub fn z80_read(&self, a: u16) -> u8 {
        self.z80_read_value(a, false).unwrap_or_else(|| {
            let (bank, addr) = self.mmu.ram(a);
            self.ram_at(bank, addr)
        })
    }

    /// Memory write as the Z80 does it, I/O left out (debugger pokes).
    pub fn z80_write(&mut self, a: u16, v: u8) {
        let _ = self.z80_store(a, v, false);
    }
}

/// The VIC's view: one 16 KB bank (`base`, from CIA2 port A) of the RAM
/// bank the MMU gives the VIC, the character ROM at `$1000–$1FFF` of banks
/// 0 and 2 (unless hidden in C128 mode), and its colour RAM bank.
pub struct VicView<'a> {
    pub mem: &'a Memory,
    pub base: u16,
    /// The cartridge: in C64 Ultimax mode the VIC-II sees its ROMH at
    /// `$3000` of every bank (and no character ROM).
    pub cart: Option<&'a dyn ExpansionCart>,
    /// The byte of the 8502's last access, for the VIC-IIe's accesses
    /// while the CPU holds the bus ([`VicMemory::cpu_data`]).
    pub cpu_data: Option<u8>,
}

impl VicView<'_> {
    /// The VIC's byte at `addr` with RAM from `ram_bank` (the MMU's choice
    /// in [`VicMemory::read`]; the debugger's graphics views may pick
    /// another).
    #[inline]
    pub fn read_bank(&self, addr: u16, ram_bank: Option<u8>) -> u8 {
        let m = self.mem;
        let addr = addr & 0x3fff;
        if m.ultimax() && addr & 0x3000 == 0x3000 {
            return self
                .cart
                .and_then(|c| c.peek_romh(0xf000 | (addr & 0x0fff)))
                .unwrap_or(0xff);
        }
        let c64 = m.mmu.c64_mode();
        let rom_visible = c64 || m.port_out() & 4 == 0;
        if self.base & 0x4000 == 0 && addr & 0x3000 == 0x1000 && rom_visible {
            let half = if c64 { 0 } else { 0x1000 };
            return m.roms.chargen[half | usize::from(addr & 0x0fff)];
        }
        // The RCR's VIC bank holds in C64 mode too (VICE `c128/vic-mmu`
        // b1c64victest: bank 1, no shared RAM for the VIC).
        let bank = ram_bank.unwrap_or(m.mmu.vic_bank());
        m.ram_at(bank, self.base | addr)
    }

    /// RAM bank the VIC sees now (the RCR's choice, also in C64 mode).
    pub fn ram_bank(&self) -> u8 {
        self.mem.mmu.vic_bank() & 1
    }
}

/// [`VicView`] with a RAM bank of the debugger's choice, for graphics views.
pub struct GfxVicView<'a> {
    pub view: VicView<'a>,
    pub ram_bank: u8,
}

impl VicMemory for GfxVicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        self.view.read_bank(addr, Some(self.ram_bank))
    }

    fn color(&self, index: u16) -> u8 {
        self.view.color(index)
    }
}

impl VicMemory for VicView<'_> {
    fn read(&self, addr: u16) -> u8 {
        self.read_bank(addr, None)
    }

    fn color(&self, index: u16) -> u8 {
        let m = self.mem;
        // C64 mode: bank 1, as for the CPU (see `Memory::color_index`).
        let bank = if m.mmu.c64_mode() {
            1
        } else {
            usize::from((m.port_out() >> 1) & 1)
        };
        m.color[(bank << 10) | usize::from(index & 0x3ff)]
    }

    fn cpu_data(&self) -> Option<u8> {
        self.cpu_data
    }
}

const TAG: [u8; 4] = *b"C8MM";
const VERSION: u16 = 2;

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.raw(&self.ram);
            w.raw(&self.color);
            w.bool(self.caps_lock);
            w.bool(self.game);
            w.bool(self.exrom);
        });
        self.mmu.save(w);
        self.port.save(w);
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, VERSION)?;
        let mut ram = vec![0; self.ram.len()];
        c.raw_into(&mut ram)?;
        let mut color = vec![0; self.color.len()];
        c.raw_into(&mut color)?;
        for v in &mut color {
            *v &= 0x0f;
        }
        let caps_lock = c.bool()?;
        let (game, exrom) = if version >= 2 {
            (c.bool()?, c.bool()?)
        } else {
            (true, true)
        };
        let mut mmu = self.mmu.clone();
        mmu.load(r)?;
        let mut port = self.port;
        port.load(r)?;
        self.ram = ram;
        self.color = color;
        self.caps_lock = caps_lock;
        self.game = game;
        self.exrom = exrom;
        self.mmu = mmu;
        self.port = port;
        Ok(())
    }
}
