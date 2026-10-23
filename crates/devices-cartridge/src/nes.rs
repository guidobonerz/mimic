//! NES/Famicom cartridges from iNES and NES 2.0 images, with the mappers
//! NROM (0), MMC1 (1), UxROM (2), CNROM (3) and MMC3 (4).
//!
//! **Image.** A 16-byte header ("NES" $1A, PRG ROM in 16 KB units, CHR ROM
//! in 8 KB units, flags 6 and 7, NES 2.0 fields in bytes 8–12), an
//! optional 512-byte trainer (loaded at $7000), the PRG ROM, the CHR ROM.
//! No CHR ROM means 8 KB of CHR RAM. Every cartridge gets PRG RAM at
//! $6000–$7FFF (8 KB, or the size a NES 2.0 header gives).
//!
//! **Nametables.** The console has 2 KB of VRAM for the nametables; the
//! cartridge decides which of its two pages each of the four nametables
//! uses (`nametable`): the header's horizontal or vertical arrangement,
//! one page for all (MMC1), or four-screen boards with 2 KB more VRAM on
//! the cartridge for nametables 2 and 3.
//!
//! **Mappers.**
//! - NROM: 16 or 32 KB PRG (16 KB repeats), 8 KB CHR.
//! - MMC1 (SxROM): a five-bit serial register written through bit 0 of
//!   $8000–$FFFF (bit 7 resets it); the fifth write goes to control
//!   ($8000: mirroring, PRG mode 0/1 32 KB, 2 fixed first, 3 fixed last;
//!   CHR 8 or 4+4 KB), CHR bank 0 ($A000), CHR bank 1 ($C000), PRG bank
//!   ($E000). A write on the cycle right after another is ignored (the
//!   dummy write of read-modify-write instructions). 512 KB boards
//!   (SUROM) take PRG A18 from bit 4 of the CHR bank 0 register.
//! - UxROM: a 16 KB bank at $8000, the last bank fixed at $C000.
//! - CNROM: an 8 KB CHR bank.
//!   UxROM and CNROM have bus conflicts: the written value is ANDed with
//!   the ROM byte at that address.
//! - MMC3 (TxROM): bank select/data ($8000/$8001: two 2 KB and four 1 KB
//!   CHR banks, two 8 KB PRG banks, PRG and CHR mode bits), mirroring
//!   ($A000), the scanline counter (latch $C000, reload $C001, disable
//!   and acknowledge $E000, enable $E001). The counter clocks on rising
//!   edges of PPU A12 after A12 was low for at least 10 dots (about three
//!   CPU cycles; the sprite fetches' short lows do not count); at zero
//!   it reloads, and with the interrupt enabled reaching zero raises
//!   /IRQ (the Sharp chip's behaviour). NES 2.0 submapper 4, the MMC3A:
//!   only a counter that was not zero before the clock, or one reloaded
//!   after a $C001 write, raises /IRQ (reloading 0 again and again does
//!   not).
//!
//! Sources: NESdev wiki, "INES", "NES 2.0", "Mirroring", "NROM", "MMC1",
//! "UxROM", "CNROM", "MMC3", "Bus conflict" (read 2026-10-23). The
//! implementation is this project's own.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

const HEADER: usize = 16;
const TRAINER: usize = 512;
/// A12 low this many PPU dots before a rising edge clocks the MMC3.
const A12_LOW_DOTS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mirroring {
    /// Nametables 0/1 on page 0, 2/3 on page 1 (vertical scrolling games).
    Horizontal,
    /// Nametables 0/2 on page 0, 1/3 on page 1.
    Vertical,
    SingleLow,
    SingleHigh,
    /// Nametables 2 and 3 in the cartridge's own VRAM.
    FourScreen,
}

/// Where a nametable address goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nametable {
    /// Offset into the console's 2 KB.
    Console(u16),
    /// Offset into the cartridge's 2 KB (four-screen).
    Cart(u16),
}

/// The video standard a NES 2.0 header asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Ntsc,
    Pal,
    Multi,
    Dendy,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Mmc1 {
    shift: u8,
    count: u8,
    pub control: u8,
    pub chr0: u8,
    pub chr1: u8,
    pub prg: u8,
    /// CPU cycle of the last write (consecutive writes are ignored).
    last_write: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Mmc3 {
    pub select: u8,
    pub regs: [u8; 8],
    /// $A000 bit 0: horizontal.
    pub horizontal: bool,
    pub ram_ctrl: u8,
    pub irq_latch: u8,
    pub irq_counter: u8,
    irq_reload: bool,
    pub irq_enabled: bool,
    pub irq_pending: bool,
    a12: bool,
    a12_low_since: u64,
    /// MMC3A interrupt behaviour (NES 2.0 submapper 4).
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mapper {
    Nrom,
    Mmc1(Mmc1),
    Uxrom { bank: u8 },
    Cnrom { bank: u8 },
    Mmc3(Mmc3),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NesCart {
    prg: Vec<u8>,
    chr: Vec<u8>,
    chr_ram: bool,
    pub prg_ram: Vec<u8>,
    /// Four-screen boards: 2 KB for nametables 2 and 3.
    vram: Vec<u8>,
    pub mapper_id: u16,
    pub submapper: u8,
    pub battery: bool,
    pub timing: Option<Timing>,
    mirroring: Mirroring,
    trainer: Option<Vec<u8>>,
    pub mapper: Mapper,
}

impl NesCart {
    /// A cartridge from an iNES or NES 2.0 image.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let bad = |m: String| Err(MediaError::Format(m));
        if bytes.len() < HEADER || &bytes[..4] != b"NES\x1a" {
            return bad("not an iNES image (no NES<1A> header)".into());
        }
        let h = &bytes[..HEADER];
        let (f6, f7) = (h[6], h[7]);
        let nes2 = f7 & 0x0c == 0x08;
        let (mut prg_units, mut chr_units) = (usize::from(h[4]), usize::from(h[5]));
        let mut mapper_id = u16::from(f6 >> 4);
        let mut submapper = 0;
        let mut prg_ram_size = 0x2000;
        let mut chr_ram_size = 0x2000;
        let mut timing = None;
        if nes2 {
            if h[9] & 0x0f == 0x0f || h[9] >> 4 == 0x0f {
                return bad("NES 2.0 exponent sizes are not supported".into());
            }
            prg_units |= usize::from(h[9] & 0x0f) << 8;
            chr_units |= usize::from(h[9] >> 4) << 8;
            mapper_id |= u16::from(f7 & 0xf0) | (u16::from(h[8] & 0x0f) << 8);
            submapper = h[8] >> 4;
            let shift = |n: u8| if n == 0 { 0 } else { 64usize << n };
            prg_ram_size = shift(h[10] & 0x0f) + shift(h[10] >> 4);
            chr_ram_size = shift(h[11] & 0x0f) + shift(h[11] >> 4);
            timing = Some(match h[12] & 3 {
                0 => Timing::Ntsc,
                1 => Timing::Pal,
                2 => Timing::Multi,
                _ => Timing::Dendy,
            });
        } else if h[12..16].iter().all(|b| *b == 0) {
            // Old dumpers wrote text into bytes 7–15; then flag 7 is junk.
            mapper_id |= u16::from(f7 & 0xf0);
        }
        let mirroring = if f6 & 8 != 0 {
            Mirroring::FourScreen
        } else if f6 & 1 != 0 {
            Mirroring::Vertical
        } else {
            Mirroring::Horizontal
        };
        let mut at = HEADER;
        let trainer = if f6 & 4 != 0 {
            let t = bytes.get(at..at + TRAINER).map(<[u8]>::to_vec);
            at += TRAINER;
            t
        } else {
            None
        };
        let (prg_len, chr_len) = (prg_units * 0x4000, chr_units * 0x2000);
        if prg_len == 0 {
            return bad("no PRG ROM".into());
        }
        if bytes.len() < at + prg_len + chr_len {
            return bad(format!(
                "image too short: {} bytes, header asks for {}",
                bytes.len(),
                at + prg_len + chr_len
            ));
        }
        let prg = bytes[at..at + prg_len].to_vec();
        let (chr, chr_ram) = if chr_len == 0 {
            (vec![0; chr_ram_size.max(0x2000)], true)
        } else {
            (bytes[at + prg_len..at + prg_len + chr_len].to_vec(), false)
        };
        let mapper = match mapper_id {
            0 => Mapper::Nrom,
            1 => Mapper::Mmc1(Mmc1::default()),
            2 => Mapper::Uxrom { bank: 0 },
            3 => Mapper::Cnrom { bank: 0 },
            4 => Mapper::Mmc3(Mmc3::default()),
            n => return bad(format!("mapper {n} is not supported (0–4)")),
        };
        let mut cart = Self {
            prg,
            chr,
            chr_ram,
            prg_ram: vec![0; prg_ram_size.max(0x2000)],
            vram: vec![
                0;
                if mirroring == Mirroring::FourScreen {
                    0x800
                } else {
                    0
                }
            ],
            mapper_id,
            submapper,
            battery: f6 & 2 != 0,
            timing,
            mirroring,
            trainer,
            mapper,
        };
        cart.reset();
        Ok(cart)
    }

    /// Power-on state of the mapper (the RAM keeps its contents).
    pub fn reset(&mut self) {
        self.mapper = match self.mapper {
            Mapper::Nrom => Mapper::Nrom,
            Mapper::Mmc1(_) => Mapper::Mmc1(Mmc1 {
                control: 0x0c,
                ..Mmc1::default()
            }),
            Mapper::Uxrom { .. } => Mapper::Uxrom { bank: 0 },
            Mapper::Cnrom { .. } => Mapper::Cnrom { bank: 0 },
            Mapper::Mmc3(_) => Mapper::Mmc3(Mmc3 {
                alt: self.submapper == 4,
                ..Mmc3::default()
            }),
        };
        if let Some(t) = &self.trainer {
            self.prg_ram[0x1000..0x1000 + TRAINER].copy_from_slice(t);
        }
    }

    pub fn prg_len(&self) -> usize {
        self.prg.len()
    }

    pub fn chr_len(&self) -> usize {
        self.chr.len()
    }

    pub fn has_chr_ram(&self) -> bool {
        self.chr_ram
    }

    /// Offset into PRG ROM of an 8 KB bank `n` (negative from the end).
    fn prg8(&self, n: i32, addr: u16) -> usize {
        let banks = (self.prg.len() / 0x2000) as i32;
        let b = n.rem_euclid(banks) as usize;
        b * 0x2000 + usize::from(addr & 0x1fff)
    }

    fn prg_offset(&self, addr: u16) -> usize {
        let a = usize::from(addr & 0x7fff);
        match &self.mapper {
            Mapper::Nrom | Mapper::Cnrom { .. } => a % self.prg.len(),
            Mapper::Uxrom { bank } => {
                let banks = self.prg.len() / 0x4000;
                let b = if a < 0x4000 {
                    usize::from(*bank) % banks
                } else {
                    banks - 1
                };
                b * 0x4000 + (a & 0x3fff)
            }
            Mapper::Mmc1(m) => {
                let outer = if self.prg.len() > 0x4_0000 {
                    usize::from(m.chr0 & 0x10) << 14
                } else {
                    0
                };
                let size = self.prg.len().min(0x4_0000);
                let banks = size / 0x4000;
                let bank = usize::from(m.prg & 0x0f);
                let b = match (m.control >> 2) & 3 {
                    0 | 1 => (bank & !1) + usize::from(a >= 0x4000),
                    2 => {
                        if a < 0x4000 {
                            0
                        } else {
                            bank
                        }
                    }
                    _ => {
                        if a < 0x4000 {
                            bank
                        } else {
                            banks - 1
                        }
                    }
                };
                (outer + (b % banks) * 0x4000 + (a & 0x3fff)) % self.prg.len()
            }
            Mapper::Mmc3(m) => {
                let r6 = i32::from(m.regs[6] & 0x3f);
                let r7 = i32::from(m.regs[7] & 0x3f);
                let n = match (a >> 13, m.select & 0x40 != 0) {
                    (0, false) => r6,
                    (0, true) => -2,
                    (1, _) => r7,
                    (2, false) => -2,
                    (2, true) => r6,
                    _ => -1,
                };
                self.prg8(n, addr)
            }
        }
    }

    /// Side-effect-free CPU read of $4020–$FFFF; `None` where the
    /// cartridge does not drive the bus (open bus).
    pub fn cpu_peek(&self, addr: u16) -> Option<u8> {
        match addr {
            0x6000..=0x7fff => Some(self.prg_ram[usize::from(addr & 0x1fff) % self.prg_ram.len()]),
            0x8000..=0xffff => Some(self.prg[self.prg_offset(addr)]),
            _ => None,
        }
    }

    /// A CPU read (no mapper here has read side effects).
    pub fn cpu_read(&mut self, addr: u16) -> Option<u8> {
        self.cpu_peek(addr)
    }

    /// A CPU write to $4020–$FFFF on CPU cycle `cycle`.
    pub fn cpu_write(&mut self, addr: u16, v: u8, cycle: u64) {
        if (0x6000..0x8000).contains(&addr) {
            let n = self.prg_ram.len();
            self.prg_ram[usize::from(addr & 0x1fff) % n] = v;
            return;
        }
        if addr < 0x8000 {
            return;
        }
        let rom = self.prg[self.prg_offset(addr)];
        match &mut self.mapper {
            Mapper::Nrom => {}
            Mapper::Uxrom { bank } => *bank = v & rom,
            Mapper::Cnrom { bank } => *bank = v & rom,
            Mapper::Mmc1(m) => {
                let consecutive = m.last_write.is_some_and(|c| c + 1 == cycle);
                m.last_write = Some(cycle);
                if consecutive {
                    return;
                }
                if v & 0x80 != 0 {
                    m.shift = 0;
                    m.count = 0;
                    m.control |= 0x0c;
                    return;
                }
                m.shift |= (v & 1) << m.count;
                m.count += 1;
                if m.count == 5 {
                    let r = m.shift;
                    match addr >> 13 {
                        4 => m.control = r,
                        5 => m.chr0 = r,
                        6 => m.chr1 = r,
                        _ => m.prg = r,
                    }
                    m.shift = 0;
                    m.count = 0;
                }
            }
            Mapper::Mmc3(m) => match (addr >> 13, addr & 1) {
                (4, 0) => m.select = v,
                (4, _) => m.regs[usize::from(m.select & 7)] = v,
                (5, 0) => m.horizontal = v & 1 != 0,
                (5, _) => m.ram_ctrl = v,
                (6, 0) => m.irq_latch = v,
                (6, _) => {
                    m.irq_counter = 0;
                    m.irq_reload = true;
                }
                (_, 0) => {
                    m.irq_enabled = false;
                    m.irq_pending = false;
                }
                _ => m.irq_enabled = true,
            },
        }
    }

    fn chr_offset(&self, addr: u16) -> usize {
        let a = usize::from(addr & 0x1fff);
        let len = self.chr.len();
        let o = match &self.mapper {
            Mapper::Nrom | Mapper::Uxrom { .. } => a,
            Mapper::Cnrom { bank } => usize::from(*bank) * 0x2000 + a,
            Mapper::Mmc1(m) => {
                if m.control & 0x10 == 0 {
                    usize::from(m.chr0 & 0x1e) * 0x1000 + a
                } else {
                    let r = if a < 0x1000 { m.chr0 } else { m.chr1 };
                    usize::from(r) * 0x1000 + (a & 0xfff)
                }
            }
            Mapper::Mmc3(m) => {
                let a = if m.select & 0x80 != 0 { a ^ 0x1000 } else { a };
                let kb = |r: u8| usize::from(r) * 0x400;
                match a >> 10 {
                    0 => kb(m.regs[0] & 0xfe) + (a & 0x3ff),
                    1 => kb(m.regs[0] | 1) + (a & 0x3ff),
                    2 => kb(m.regs[1] & 0xfe) + (a & 0x3ff),
                    3 => kb(m.regs[1] | 1) + (a & 0x3ff),
                    n => kb(m.regs[n - 2]) + (a & 0x3ff),
                }
            }
        };
        o % len
    }

    /// Pattern table read ($0000–$1FFF).
    pub fn chr_peek(&self, addr: u16) -> u8 {
        self.chr[self.chr_offset(addr)]
    }

    /// Pattern table write: CHR RAM only.
    pub fn chr_write(&mut self, addr: u16, v: u8) {
        if self.chr_ram {
            let o = self.chr_offset(addr);
            self.chr[o] = v;
        }
    }

    /// The current nametable arrangement.
    pub fn mirroring(&self) -> Mirroring {
        if self.mirroring == Mirroring::FourScreen {
            return Mirroring::FourScreen;
        }
        match &self.mapper {
            Mapper::Mmc1(m) => match m.control & 3 {
                0 => Mirroring::SingleLow,
                1 => Mirroring::SingleHigh,
                2 => Mirroring::Vertical,
                _ => Mirroring::Horizontal,
            },
            Mapper::Mmc3(m) => {
                if m.horizontal {
                    Mirroring::Horizontal
                } else {
                    Mirroring::Vertical
                }
            }
            _ => self.mirroring,
        }
    }

    /// Where nametable address `addr` ($2000–$2FFF) goes.
    pub fn nametable(&self, addr: u16) -> Nametable {
        let table = (addr >> 10) & 3;
        let off = addr & 0x3ff;
        let page = match self.mirroring() {
            Mirroring::Horizontal => table >> 1,
            Mirroring::Vertical => table & 1,
            Mirroring::SingleLow => 0,
            Mirroring::SingleHigh => 1,
            Mirroring::FourScreen => {
                if table >= 2 {
                    return Nametable::Cart((table - 2) << 10 | off);
                }
                table
            }
        };
        Nametable::Console(page << 10 | off)
    }

    pub fn vram_peek(&self, offset: u16) -> u8 {
        self.vram.get(usize::from(offset)).copied().unwrap_or(0)
    }

    pub fn vram_write(&mut self, offset: u16, v: u8) {
        if let Some(b) = self.vram.get_mut(usize::from(offset)) {
            *b = v;
        }
    }

    /// The PPU put `addr` on its address bus at PPU dot `dot` (counted
    /// from power-on): the MMC3 watches A12.
    pub fn ppu_address(&mut self, addr: u16, dot: u64) {
        let Mapper::Mmc3(m) = &mut self.mapper else {
            return;
        };
        let a12 = addr & 0x1000 != 0;
        if a12 && !m.a12 && dot.saturating_sub(m.a12_low_since) >= A12_LOW_DOTS {
            let (before, cleared) = (m.irq_counter, m.irq_reload);
            if m.irq_counter == 0 || m.irq_reload {
                m.irq_counter = m.irq_latch;
                m.irq_reload = false;
            } else {
                m.irq_counter -= 1;
            }
            if m.irq_counter == 0 && m.irq_enabled && (!m.alt || before != 0 || cleared) {
                m.irq_pending = true;
            }
        }
        if !a12 && m.a12 {
            m.a12_low_since = dot;
        }
        m.a12 = a12;
    }

    /// The cartridge's /IRQ output.
    pub fn irq(&self) -> bool {
        matches!(&self.mapper, Mapper::Mmc3(m) if m.irq_pending)
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"NESC", 1, |w| {
            w.bytes(&self.prg_ram);
            w.bytes(if self.chr_ram { &self.chr } else { &[] });
            w.bytes(&self.vram);
            match &self.mapper {
                Mapper::Nrom => w.u8(0),
                Mapper::Mmc1(m) => {
                    w.u8(1);
                    for v in [m.shift, m.count, m.control, m.chr0, m.chr1, m.prg] {
                        w.u8(v);
                    }
                    w.bool(m.last_write.is_some());
                    w.u64(m.last_write.unwrap_or(0));
                }
                Mapper::Uxrom { bank } => {
                    w.u8(2);
                    w.u8(*bank);
                }
                Mapper::Cnrom { bank } => {
                    w.u8(3);
                    w.u8(*bank);
                }
                Mapper::Mmc3(m) => {
                    w.u8(4);
                    w.u8(m.select);
                    w.raw(&m.regs);
                    w.bool(m.horizontal);
                    for v in [m.ram_ctrl, m.irq_latch, m.irq_counter] {
                        w.u8(v);
                    }
                    for v in [m.irq_reload, m.irq_enabled, m.irq_pending, m.a12] {
                        w.bool(v);
                    }
                    w.u64(m.a12_low_since);
                }
            }
        });
    }

    /// Atomic: a failed load leaves the cartridge unchanged.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"NESC", 1)?;
        let mut n = self.clone();
        let ram = c.bytes()?;
        if ram.len() != n.prg_ram.len() {
            return Err(StateError::Invalid("PRG RAM size"));
        }
        n.prg_ram.copy_from_slice(ram);
        let chr = c.bytes()?;
        if n.chr_ram {
            if chr.len() != n.chr.len() {
                return Err(StateError::Invalid("CHR RAM size"));
            }
            n.chr.copy_from_slice(chr);
        }
        let vram = c.bytes()?;
        if vram.len() != n.vram.len() {
            return Err(StateError::Invalid("cartridge VRAM size"));
        }
        n.vram.copy_from_slice(vram);
        let id = c.u8()?;
        n.mapper = match (id, &n.mapper) {
            (0, Mapper::Nrom) => Mapper::Nrom,
            (1, Mapper::Mmc1(_)) => {
                let mut m = Mmc1 {
                    shift: c.u8()? & 0x1f,
                    count: c.u8()?,
                    control: c.u8()? & 0x1f,
                    chr0: c.u8()? & 0x1f,
                    chr1: c.u8()? & 0x1f,
                    prg: c.u8()? & 0x1f,
                    last_write: None,
                };
                if m.count > 4 {
                    return Err(StateError::Invalid("MMC1 shift count"));
                }
                let has = c.bool()?;
                let cy = c.u64()?;
                m.last_write = has.then_some(cy);
                Mapper::Mmc1(m)
            }
            (2, Mapper::Uxrom { .. }) => Mapper::Uxrom { bank: c.u8()? },
            (3, Mapper::Cnrom { .. }) => Mapper::Cnrom { bank: c.u8()? },
            (4, Mapper::Mmc3(_)) => {
                let mut m = Mmc3 {
                    select: c.u8()?,
                    alt: n.submapper == 4,
                    ..Mmc3::default()
                };
                c.raw_into(&mut m.regs)?;
                m.horizontal = c.bool()?;
                m.ram_ctrl = c.u8()?;
                m.irq_latch = c.u8()?;
                m.irq_counter = c.u8()?;
                m.irq_reload = c.bool()?;
                m.irq_enabled = c.bool()?;
                m.irq_pending = c.bool()?;
                m.a12 = c.bool()?;
                m.a12_low_since = c.u64()?;
                Mapper::Mmc3(m)
            }
            _ => return Err(StateError::Invalid("mapper differs")),
        };
        *self = n;
        Ok(())
    }
}

/// The NES cartridge slot.
pub fn slot() -> CartSlot<NesCart> {
    CartSlot::new(NesCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image with `prg` 16 KB units (each unit's first byte = its
    /// number) and `chr` 8 KB units (each 1 KB's first byte = its number).
    fn image(mapper: u8, prg: u8, chr: u8, f6: u8) -> Vec<u8> {
        let mut v = vec![
            b'N',
            b'E',
            b'S',
            0x1a,
            prg,
            chr,
            f6 | (mapper << 4),
            mapper & 0xf0,
        ];
        v.resize(HEADER, 0);
        for b in 0..usize::from(prg) {
            let mut bank = vec![0u8; 0x4000];
            bank[0] = b as u8;
            bank[0x2000] = b as u8 | 0x80;
            v.extend(bank);
        }
        for k in 0..usize::from(chr) * 8 {
            let mut kb = vec![0u8; 0x400];
            kb[0] = k as u8;
            v.extend(kb);
        }
        v
    }

    #[test]
    fn header_and_nrom() {
        let c = NesCart::parse(&image(0, 1, 1, 1)).unwrap();
        assert_eq!(c.mapper_id, 0);
        assert_eq!(c.cpu_peek(0x8000), Some(0));
        assert_eq!(c.cpu_peek(0xc000), Some(0), "16 KB repeats");
        assert_eq!(c.mirroring(), Mirroring::Vertical);
        assert_eq!(c.nametable(0x2400), Nametable::Console(0x400));
        assert_eq!(c.nametable(0x2800), Nametable::Console(0));
        assert_eq!(c.cpu_peek(0x5000), None);
        assert!(!c.has_chr_ram());
        let h = NesCart::parse(&image(0, 2, 0, 0)).unwrap();
        assert!(h.has_chr_ram());
        assert_eq!(h.nametable(0x2400), Nametable::Console(0));
        assert_eq!(h.nametable(0x2800), Nametable::Console(0x400));
        assert!(NesCart::parse(b"NES\x1a").is_err());
        assert!(NesCart::parse(&image(9, 1, 1, 0)).is_err(), "mapper 9");
        let mut short = image(0, 2, 1, 0);
        short.truncate(0x6000);
        assert!(NesCart::parse(&short).is_err());
    }

    #[test]
    fn uxrom_and_cnrom_switch_with_bus_conflicts() {
        let mut img = image(2, 8, 0, 0);
        // A bank table so the conflicting ROM byte equals the value.
        let table = HEADER + 7 * 0x4000 + 0x3f00;
        for i in 0..8 {
            img[table + i] = i as u8;
        }
        let mut c = NesCart::parse(&img).unwrap();
        assert_eq!(c.cpu_peek(0xc000), Some(7), "last bank fixed");
        c.cpu_write(0xff03, 3, 0);
        assert_eq!(c.cpu_peek(0x8000), Some(3));
        c.cpu_write(0x8000, 5, 1);
        assert_eq!(c.cpu_peek(0x8000), Some(1), "5 AND the ROM byte 3");
        let mut c = NesCart::parse(&image(3, 2, 4, 0)).unwrap();
        c.cpu_write(0x8001, 2, 0); // ROM byte there is 0: conflict
        assert_eq!(c.chr_peek(0), 0);
        let mut img = image(3, 2, 4, 0);
        img[HEADER + 0x4000 + 0x3fff] = 0xff;
        let mut c = NesCart::parse(&img).unwrap();
        c.cpu_write(0xffff, 2, 0);
        assert_eq!(c.chr_peek(0), 16, "8 KB bank 2 = 1 KB unit 16");
    }

    fn mmc1_write(c: &mut NesCart, addr: u16, v: u8, cycle: &mut u64) {
        for i in 0..5 {
            c.cpu_write(addr, v >> i & 1, *cycle);
            *cycle += 2;
        }
    }

    #[test]
    fn mmc1_serial_registers() {
        let mut c = NesCart::parse(&image(1, 8, 2, 0)).unwrap();
        let mut cy = 0;
        assert_eq!(c.cpu_peek(0xc000), Some(7), "power-on: last bank fixed");
        mmc1_write(&mut c, 0xe000, 2, &mut cy);
        assert_eq!(c.cpu_peek(0x8000), Some(2));
        mmc1_write(&mut c, 0x8000, 0x08, &mut cy); // mode 2, one screen
        assert_eq!(c.cpu_peek(0x8000), Some(0));
        assert_eq!(c.cpu_peek(0xc000), Some(2));
        assert_eq!(c.mirroring(), Mirroring::SingleLow);
        mmc1_write(&mut c, 0x8000, 0x13, &mut cy); // 4 KB CHR, 32 KB PRG
        mmc1_write(&mut c, 0xa000, 3, &mut cy);
        mmc1_write(&mut c, 0xc000, 1, &mut cy);
        assert_eq!(c.chr_peek(0x0000), 12);
        assert_eq!(c.chr_peek(0x1000), 4);
        assert_eq!(c.cpu_peek(0x8000), Some(2));
        assert_eq!(c.cpu_peek(0xc000), Some(3));
        // A write on the next cycle is ignored; bit 7 resets.
        c.cpu_write(0xe000, 1, cy);
        c.cpu_write(0xe000, 1, cy + 1);
        c.cpu_write(0xe000, 0x80, cy + 3);
        assert_eq!((c.cpu_peek(0xc000)), Some(7), "reset fixes the last bank");
        if let Mapper::Mmc1(m) = &c.mapper {
            assert_eq!(m.count, 0);
        }
    }

    #[test]
    fn mmc3_banks_and_scanline_counter() {
        let mut c = NesCart::parse(&image(4, 4, 4, 0)).unwrap();
        // PRG: 8 banks of 8 KB; first byte of each 8 KB: unit (| $80).
        c.cpu_write(0x8000, 6, 0);
        c.cpu_write(0x8001, 3, 0);
        assert_eq!(c.cpu_peek(0x8000), Some(0x81));
        assert_eq!(c.cpu_peek(0xc000), Some(3), "second last");
        assert_eq!(c.cpu_peek(0xe000), Some(0x83), "last");
        c.cpu_write(0x8000, 0x46, 0);
        assert_eq!(c.cpu_peek(0x8000), Some(3));
        assert_eq!(c.cpu_peek(0xc000), Some(0x81));
        // CHR: R2 = 1 KB unit 9 at $1000, inverted to $0000.
        c.cpu_write(0x8000, 2, 0);
        c.cpu_write(0x8001, 9, 0);
        assert_eq!(c.chr_peek(0x1000), 9);
        c.cpu_write(0x8000, 0x82, 0);
        assert_eq!(c.chr_peek(0x0000), 9);
        c.cpu_write(0xa000, 1, 0);
        assert_eq!(c.mirroring(), Mirroring::Horizontal);
        // Counter: latch 2, enabled; one A12 rise a line.
        c.cpu_write(0xc000, 2, 0);
        c.cpu_write(0xc001, 0, 0);
        c.cpu_write(0xe001, 0, 0);
        let mut dot = 100;
        let mut line = |c: &mut NesCart| {
            c.ppu_address(0x0000, dot);
            c.ppu_address(0x1000, dot + 160);
            // A short low in the sprite fetches does not count.
            c.ppu_address(0x2000, dot + 164);
            c.ppu_address(0x1000, dot + 168);
            c.ppu_address(0x0000, dot + 220);
            dot += 341;
        };
        line(&mut c); // reload to 2
        assert!(!c.irq());
        line(&mut c); // 1
        assert!(!c.irq());
        line(&mut c); // 0: interrupt
        assert!(c.irq());
        c.cpu_write(0xe000, 0, 0);
        assert!(!c.irq(), "acknowledged");
        let mut w = StateWriter::new();
        c.save(&mut w);
        let data = w.into_bytes();
        let mut d = NesCart::parse(&image(4, 4, 4, 0)).unwrap();
        d.load(&mut StateReader::new(&data)).unwrap();
        assert_eq!(d, c);
        let mut other = NesCart::parse(&image(1, 4, 4, 0)).unwrap();
        assert!(other.load(&mut StateReader::new(&data)).is_err());
    }

    #[test]
    fn four_screen_and_trainer() {
        let mut img = image(0, 1, 1, 8 | 4);
        let mut t = vec![0u8; TRAINER];
        t[0] = 0x5a;
        img.splice(HEADER..HEADER, t);
        let mut c = NesCart::parse(&img).unwrap();
        assert_eq!(c.cpu_peek(0x7000), Some(0x5a));
        assert_eq!(c.nametable(0x2c05), Nametable::Cart(0x405));
        assert_eq!(c.nametable(0x2405), Nametable::Console(0x405));
        c.vram_write(0x405, 9);
        assert_eq!(c.vram_peek(0x405), 9);
    }
}
