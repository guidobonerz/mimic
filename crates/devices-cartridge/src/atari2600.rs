//! Atari 2600 cartridges. The cartridge sees A0–A12 of the 6507; it
//! answers in the upper half (A12 = 1, the 4 KB window `$1000–$1FFF`) and
//! watches every bus cycle for its bank-switching hotspots ([`Cart2600::access`],
//! [`Cart2600::snoop`]).
//!
//! | Scheme | Size        | Switching                                                        |
//! | ------ | ----------- | ---------------------------------------------------------------- |
//! | 2K, 4K | 2 / 4 KB    | none (2 KB mirrored twice)                                       |
//! | F8     | 8 KB        | access `$1FF8`/`$1FF9` selects 4 KB bank 0/1                     |
//! | F6     | 16 KB       | `$1FF6–$1FF9`, banks 0–3                                         |
//! | F4     | 32 KB       | `$1FF4–$1FFB`, banks 0–7                                         |
//! | FA     | 12 KB       | `$1FF8–$1FFA`, banks 0–2; 256 bytes RAM (write `$1000`, read `$1100`) |
//! | FE     | 8 KB        | after an access to `$01FE` (JSR/RTS with the stack at `$01FF`), D5 of the next byte on the bus: 1 = bank 0, 0 = bank 1 |
//! | E0     | 8 KB        | four 1 KB segments; `$1FE0–$1FE7`, `$1FE8–$1FEF`, `$1FF0–$1FF7` pick the slice of segments 0–2, segment 3 is the last slice |
//! | E7     | 16 KB       | `$1FE0–$1FE6`: 2 KB slice at `$1000`; `$1FE7`: 1 KB RAM there instead (write `$1000`, read `$1400`); `$1FE8–$1FEB`: 256-byte RAM block at `$1800` (write) / `$1900` (read); the last 2 KB slice fixed at `$1800` |
//! | 3F     | 8 KB–512 KB | a write to `$00–$3F` selects the 2 KB slice at `$1000` (data mod slices); the last slice fixed at `$1800` |
//! | DPC    | 10 KB       | F8 banking of the 8 KB program; the Display Processor Chip's registers at `$1000–$107F` with 2 KB display data ([`dpc`]) |
//!
//! Superchip ("SC", on F8/F6/F4): 128 bytes RAM, write port `$1000–$107F`,
//! read port `$1080–$10FF`. The cartridge has no R/W line: a write to a
//! read port is ignored here, and a read of a write port returns what is
//! on the data bus.
//!
//! Power-up: F8/F6/F4/FA/FE/DPC in their last bank, E0 with slices 0, 1, 2,
//! E7 and 3F with slice 0.
//!
//! Images are raw ROM dumps (`.a26`, `.bin`). [`detect`] picks the scheme
//! from the size and from code signatures: `STA $3F` for 3F, absolute
//! accesses to the E0/E7 hotspots, RAM ports (the first 256 bytes of every
//! bank one repeated value, or frequent stores into the write port) for
//! the Superchip, reset vectors with D5 = 1 in bank 0 and D5 = 0 in the
//! last bank for FE; DPC by its size (10 240 bytes, or 10 495 with the
//! dump of the chip's random number sequence appended, which is ignored);
//! [`Cart2600::with_scheme`]
//! overrides it.
//!
//! Source: Kevin Horton, "Cart Information" v6
//! (<https://www.qotile.net/minidig/docs/sizes.txt>), sections "Extra RAM
//! in Carts" and "Descriptions of the Various Bankswitch Modes"; the FE
//! data-bit rule is this project's reading of its JSR/RTS description.

use emu_core::media::MediaError;
use emu_core::{StateError, StateReader, StateWriter};

use crate::slot::CartSlot;

pub mod dpc;

use dpc::Dpc;

/// DPC images: program and display data, optionally followed by the
/// 255-byte random number dump.
const DPC_LEN: usize = 0x2800;
const DPC_LEN_WITH_RNG: usize = 0x28ff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// 2 KB or 4 KB, no bank switching.
    Plain,
    F8,
    F6,
    F4,
    Fa,
    Fe,
    E0,
    E7,
    Tigervision,
    /// Activision's Display Processor Chip (Pitfall II).
    Dpc,
}

impl Scheme {
    /// Name as used in options and messages.
    pub fn name(self) -> &'static str {
        match self {
            Scheme::Plain => "4k",
            Scheme::F8 => "f8",
            Scheme::F6 => "f6",
            Scheme::F4 => "f4",
            Scheme::Fa => "fa",
            Scheme::Fe => "fe",
            Scheme::E0 => "e0",
            Scheme::E7 => "e7",
            Scheme::Tigervision => "3f",
            Scheme::Dpc => "dpc",
        }
    }

    /// A scheme by option name; `sc` suffix (f8sc …) adds the Superchip.
    pub fn from_name(name: &str) -> Option<(Self, bool)> {
        let (base, sc) = match name.strip_suffix("sc") {
            Some(b) => (b, true),
            None => (name, false),
        };
        let s = match base {
            "2k" | "4k" => Scheme::Plain,
            "f8" => Scheme::F8,
            "f6" => Scheme::F6,
            "f4" => Scheme::F4,
            "fa" => Scheme::Fa,
            "fe" => Scheme::Fe,
            "e0" => Scheme::E0,
            "e7" => Scheme::E7,
            "3f" => Scheme::Tigervision,
            "dpc" => Scheme::Dpc,
            _ => return None,
        };
        (!sc || matches!(s, Scheme::F8 | Scheme::F6 | Scheme::F4)).then_some((s, sc))
    }

    /// ROM sizes the scheme works with.
    fn fits(self, len: usize) -> bool {
        match self {
            Scheme::Plain => len == 0x800 || len == 0x1000,
            Scheme::F8 | Scheme::Fe | Scheme::E0 => len == 0x2000,
            Scheme::F6 | Scheme::E7 => len == 0x4000,
            Scheme::F4 => len == 0x8000,
            Scheme::Fa => len == 0x3000,
            Scheme::Tigervision => len >= 0x1000 && len.is_multiple_of(0x800) && len <= 0x80000,
            Scheme::Dpc => len == DPC_LEN || len == DPC_LEN_WITH_RNG,
        }
    }
}

/// Times `pattern` occurs in `rom`.
fn count(rom: &[u8], pattern: impl Fn(&[u8]) -> bool, len: usize) -> usize {
    rom.windows(len).filter(|w| pattern(w)).count()
}

/// Absolute reads/writes (LDA, STA, BIT, LDX, LDY, CMP, STX, STY) of a
/// hotspot address `$xFlo` with `lo` in `range` (any mirror of `$1Fxx`).
fn hotspot_accesses(rom: &[u8], range: std::ops::RangeInclusive<u8>) -> usize {
    const ABS: [u8; 8] = [0xad, 0x8d, 0x2c, 0xae, 0xac, 0xcd, 0x8e, 0x8c];
    count(
        rom,
        |w| ABS.contains(&w[0]) && range.contains(&w[1]) && w[2] & 0x1f == 0x1f,
        3,
    )
}

/// The scheme and Superchip flag for a ROM image, from its size and
/// contents.
pub fn detect(rom: &[u8]) -> Result<(Scheme, bool), MediaError> {
    let sta_3f = count(rom, |w| w == [0x85, 0x3f], 2);
    // Superchip: RAM ports left empty in every bank, or code that writes
    // into the write port (STA/STX/STY abs[,X/Y] to $x000–$x07F) often.
    let superchip = |banks: usize| {
        let empty = (0..banks).all(|b| {
            let ports = &rom[b * 0x1000..b * 0x1000 + 0x100];
            ports.iter().all(|&x| x == ports[0])
        });
        let stores = count(
            rom,
            |w| {
                [0x8d, 0x9d, 0x99, 0x8e, 0x8c].contains(&w[0]) && w[1] < 0x80 && w[2] & 0x1f == 0x10
            },
            3,
        );
        empty || stores >= 10
    };
    // FE: bank 0 runs at $Fxxx (D5 = 1), the power-up bank at an address
    // with D5 = 0 ($Cxxx/$Dxxx): their reset vectors say so.
    let fe = rom.len() == 0x2000 && rom[0x0ffd] & 0x20 != 0 && rom[0x1ffd] & 0x20 == 0;
    Ok(match rom.len() {
        0x800 | 0x1000 => (Scheme::Plain, false),
        // E0 first: its code may contain `85 3F` byte pairs by chance, 3F
        // code has no E0 hotspot accesses.
        0x2000 if hotspot_accesses(rom, 0xe0..=0xf7) >= 2 => (Scheme::E0, false),
        0x2000 if sta_3f >= 2 => (Scheme::Tigervision, false),
        0x2000 if fe => (Scheme::Fe, false),
        0x2000 => (Scheme::F8, superchip(2)),
        0x3000 => (Scheme::Fa, false),
        DPC_LEN | DPC_LEN_WITH_RNG => (Scheme::Dpc, false),
        0x4000 if hotspot_accesses(rom, 0xe0..=0xe7) >= 2 => (Scheme::E7, false),
        0x4000 => (Scheme::F6, superchip(4)),
        0x8000 if sta_3f >= 2 => (Scheme::Tigervision, false),
        0x8000 => (Scheme::F4, superchip(8)),
        n if Scheme::Tigervision.fits(n) && sta_3f >= 1 => (Scheme::Tigervision, false),
        n => {
            return Err(MediaError::Format(format!(
                "Atari 2600 cartridge of {n} bytes: no known bank-switching scheme"
            )))
        }
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cart2600 {
    pub scheme: Scheme,
    pub superchip: bool,
    rom: Vec<u8>,
    /// Selected 4 KB bank (F8/F6/F4/FA/FE) or 2 KB slice (E7, 3F).
    bank: usize,
    /// E0: slices of segments 0–2.
    slices: [u8; 3],
    /// Cartridge RAM: Superchip 128, FA 256, E7 2048 bytes.
    ram: Vec<u8>,
    /// E7: 256-byte block at `$1800`, and the 1 KB RAM at `$1000`.
    ram_block: u8,
    ram_low: bool,
    /// FE: `$01FE` was accessed; the next bus byte picks the bank.
    fe_armed: bool,
    /// The Display Processor Chip of a DPC cartridge.
    dpc: Option<Box<Dpc>>,
}

impl Cart2600 {
    /// A cartridge with the detected scheme.
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        let (scheme, superchip) = detect(bytes)?;
        Self::new(bytes.to_vec(), scheme, superchip)
    }

    fn new(rom: Vec<u8>, scheme: Scheme, superchip: bool) -> Result<Self, MediaError> {
        if !scheme.fits(rom.len()) {
            return Err(MediaError::Format(format!(
                "Atari 2600 cartridge of {} bytes does not fit scheme {}",
                rom.len(),
                scheme.name()
            )));
        }
        let ram = match scheme {
            _ if superchip => 128,
            Scheme::Fa => 256,
            Scheme::E7 => 2048,
            _ => 0,
        };
        let mut c = Self {
            scheme,
            superchip,
            rom,
            bank: 0,
            slices: [0; 3],
            ram: vec![0; ram],
            ram_block: 0,
            ram_low: false,
            fe_armed: false,
            dpc: None,
        };
        c.power_on();
        Ok(c)
    }

    /// The same ROM with another scheme (an option overriding detection).
    pub fn with_scheme(&self, scheme: Scheme, superchip: bool) -> Result<Self, MediaError> {
        Self::new(self.rom.clone(), scheme, superchip)
    }

    /// Power-up banking (RAM cleared).
    pub fn power_on(&mut self) {
        self.bank = match self.scheme {
            Scheme::F8 | Scheme::F6 | Scheme::F4 | Scheme::Fa | Scheme::Fe => {
                self.rom.len() / 0x1000 - 1
            }
            Scheme::Dpc => 1,
            _ => 0,
        };
        self.dpc = (self.scheme == Scheme::Dpc).then(Box::default);
        self.slices = [0, 1, 2];
        self.ram.fill(0);
        self.ram_block = 0;
        self.ram_low = false;
        self.fe_armed = false;
    }

    fn banks(&self) -> usize {
        match self.scheme {
            Scheme::E7 | Scheme::Tigervision => self.rom.len() / 0x800,
            Scheme::Dpc => 2,
            _ => self.rom.len().div_ceil(0x1000),
        }
    }

    /// What the cartridge drives for a read at `addr` in its window
    /// (A12 = 1), without side effects; `None`: nothing (a write port).
    fn read_at(&self, addr: u16) -> Option<u8> {
        let off = usize::from(addr & 0x0fff);
        let last2k = self.rom.len() - 0x800;
        if self.superchip && off < 0x100 {
            return (off >= 0x80).then(|| self.ram[off - 0x80]);
        }
        match self.scheme {
            Scheme::Fa if off < 0x200 => return (off >= 0x100).then(|| self.ram[off - 0x100]),
            Scheme::E7 if self.ram_low && off < 0x800 => {
                return (off >= 0x400).then(|| self.ram[off - 0x400]);
            }
            Scheme::E7 if (0x800..0xa00).contains(&off) => {
                let i = 0x400 + usize::from(self.ram_block) * 0x100 + (off & 0xff);
                return (off >= 0x900).then(|| self.ram[i]);
            }
            Scheme::Dpc if off < 0x40 => {
                return self.dpc.as_ref().map(|d| d.peek(off, self.display()));
            }
            _ => {}
        }
        Some(match self.scheme {
            Scheme::Plain => self.rom[off & (self.rom.len() - 1)],
            Scheme::F8 | Scheme::F6 | Scheme::F4 | Scheme::Fa | Scheme::Fe | Scheme::Dpc => {
                self.rom[self.bank * 0x1000 + off]
            }
            Scheme::E0 => {
                let seg = off >> 10;
                let slice = if seg == 3 {
                    7
                } else {
                    usize::from(self.slices[seg])
                };
                self.rom[slice * 0x400 + (off & 0x3ff)]
            }
            Scheme::E7 | Scheme::Tigervision => {
                if off < 0x800 {
                    self.rom[self.bank * 0x800 + off]
                } else {
                    self.rom[last2k + off - 0x800]
                }
            }
        })
    }

    /// The byte at `addr` (A12 = 1) without side effects; a write port
    /// reads as 0.
    pub fn peek(&self, addr: u16) -> u8 {
        self.read_at(addr).unwrap_or(0)
    }

    /// One bus cycle at `addr` (A0–A12); `write` carries the data of a
    /// write. Switches banks on hotspots; returns what the cartridge
    /// drives onto the data bus.
    pub fn access(&mut self, addr: u16, write: Option<u8>) -> Option<u8> {
        let a = addr & 0x1fff;
        match self.scheme {
            Scheme::Tigervision => {
                if let (Some(v), true) = (write, a < 0x40) {
                    self.bank = usize::from(v) % self.banks();
                }
            }
            Scheme::Fe if a == 0x01fe => self.fe_armed = true,
            _ => {}
        }
        if a & 0x1000 == 0 {
            return None;
        }
        let hot = |lo: u16, hi: u16| (lo..=hi).contains(&a).then(|| usize::from(a - lo));
        match self.scheme {
            Scheme::F8 | Scheme::Dpc => self.bank = hot(0x1ff8, 0x1ff9).unwrap_or(self.bank),
            Scheme::F6 => self.bank = hot(0x1ff6, 0x1ff9).unwrap_or(self.bank),
            Scheme::F4 => self.bank = hot(0x1ff4, 0x1ffb).unwrap_or(self.bank),
            Scheme::Fa => self.bank = hot(0x1ff8, 0x1ffa).unwrap_or(self.bank),
            Scheme::E0 => {
                if let Some(i) = hot(0x1fe0, 0x1ff7) {
                    self.slices[i / 8] = (i % 8) as u8;
                }
            }
            Scheme::E7 => match hot(0x1fe0, 0x1feb) {
                Some(i @ 0..=6) => {
                    self.bank = i;
                    self.ram_low = false;
                }
                Some(7) => self.ram_low = true,
                Some(i) => self.ram_block = (i - 8) as u8,
                None => {}
            },
            _ => {}
        }
        if let Some(d) = &mut self.dpc {
            let reg = usize::from(a & 0x0fff);
            match write {
                Some(v) if (0x40..0x80).contains(&reg) => d.write(reg, v),
                None if reg < 0x40 => {
                    let display = &self.rom[0x2000..0x2000 + dpc::DISPLAY_LEN];
                    return Some(d.read(reg, display));
                }
                _ => {}
            }
        }
        match write {
            Some(v) => {
                self.write_ram(a, v);
                None
            }
            None => self.read_at(a),
        }
    }

    /// DPC: the 2 KB display data after the program.
    fn display(&self) -> &[u8] {
        self.rom
            .get(0x2000..0x2000 + dpc::DISPLAY_LEN)
            .unwrap_or(&[])
    }

    /// One CPU cycle of a CPU clocked at `cpu_hz` (the DPC's music
    /// oscillator runs on emulated time); nothing for other schemes.
    #[inline]
    pub fn tick(&mut self, cpu_hz: u32) {
        if let Some(d) = &mut self.dpc {
            d.tick(cpu_hz);
        }
    }

    /// The DPC's registers for a debugger view; `None` for other schemes.
    pub fn dpc_registers(&self) -> Option<Vec<(&'static str, u32, u8)>> {
        self.dpc.as_ref().map(|d| d.registers())
    }

    /// A write into a RAM write port.
    fn write_ram(&mut self, addr: u16, v: u8) {
        let off = usize::from(addr & 0x0fff);
        let slot = if self.superchip {
            (off < 0x80).then_some(off)
        } else {
            match self.scheme {
                Scheme::Fa => (off < 0x100).then_some(off),
                Scheme::E7 if self.ram_low && off < 0x400 => Some(off),
                Scheme::E7 if (0x800..0x900).contains(&off) => {
                    Some(0x400 + usize::from(self.ram_block) * 0x100 + (off & 0xff))
                }
                _ => None,
            }
        };
        if let Some(i) = slot {
            self.ram[i] = v;
        }
    }

    /// The byte on the data bus at the end of a cycle at `addr` (FE reads
    /// its bank from the byte after a `$01FE` access).
    pub fn snoop(&mut self, addr: u16, data: u8) {
        if self.fe_armed && addr & 0x1fff != 0x01fe {
            self.fe_armed = false;
            self.bank = if data & 0x20 != 0 { 0 } else { 1 };
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"C26 ", 1, |w| {
            w.u8(self.scheme as u8);
            w.bool(self.superchip);
            w.u32(self.rom.len() as u32);
            w.u32(self.bank as u32);
            w.raw(&self.slices);
            w.bytes(&self.ram);
            w.u8(self.ram_block);
            w.bool(self.ram_low);
            w.bool(self.fe_armed);
        });
        if let Some(d) = &self.dpc {
            d.save(w);
        }
    }

    /// Banking state and RAM; the ROM is the inserted image.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"C26 ", 1)?;
        let scheme = c.u8()?;
        let superchip = c.bool()?;
        let len = c.u32()? as usize;
        if scheme != self.scheme as u8 || superchip != self.superchip || len != self.rom.len() {
            return Err(StateError::Invalid("cartridge differs"));
        }
        let bank = c.u32()? as usize;
        let mut slices = [0; 3];
        c.raw_into(&mut slices)?;
        let ram = c.bytes()?;
        let ram_block = c.u8()?;
        let ram_low = c.bool()?;
        let fe_armed = c.bool()?;
        if bank >= self.banks().max(1)
            || slices.iter().any(|&s| s > 7)
            || ram.len() != self.ram.len()
            || ram_block > 3
        {
            return Err(StateError::Invalid("cartridge banking"));
        }
        self.bank = bank;
        self.slices = slices;
        self.ram.copy_from_slice(ram);
        self.ram_block = ram_block;
        if let Some(d) = &mut self.dpc {
            d.load(r)?;
        }
        self.ram_low = ram_low;
        self.fe_armed = fe_armed;
        Ok(())
    }
}

/// The 2600's cartridge slot.
pub fn slot() -> CartSlot<Cart2600> {
    CartSlot::new(Cart2600::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM whose bytes name their `unit`-sized slice.
    fn rom(size: usize, unit: usize) -> Vec<u8> {
        (0..size).map(|i| (i / unit) as u8).collect()
    }

    fn cart(size: usize, unit: usize, scheme: Scheme, sc: bool) -> Cart2600 {
        Cart2600::new(rom(size, unit), scheme, sc).unwrap()
    }

    fn read(c: &mut Cart2600, a: u16) -> Option<u8> {
        c.access(a, None)
    }

    #[test]
    fn plain_images_mirror_into_the_window() {
        let mut r = vec![0u8; 0x800];
        r[0] = 0x11;
        r[0x7ff] = 0x22;
        let mut c = Cart2600::parse(&r).unwrap();
        assert_eq!(read(&mut c, 0x1000), Some(0x11));
        assert_eq!(read(&mut c, 0x1800), Some(0x11), "2K mirrored");
        assert_eq!(read(&mut c, 0x1fff), Some(0x22));
        assert_eq!(read(&mut c, 0x0080), None, "not in the cartridge window");
        assert_eq!(c.access(0x1000, Some(5)), None);
        assert!(Cart2600::parse(&[0; 100]).is_err());
    }

    #[test]
    fn f8_f6_f4_hotspots() {
        let mut c = cart(0x2000, 0x1000, Scheme::F8, false);
        assert_eq!(read(&mut c, 0x1000), Some(1), "starts in the last bank");
        read(&mut c, 0x1ff8);
        assert_eq!(read(&mut c, 0x1000), Some(0));
        c.access(0x1ff9, Some(0));
        assert_eq!(read(&mut c, 0x1000), Some(1), "writes switch too");
        let mut c = cart(0x4000, 0x1000, Scheme::F6, false);
        read(&mut c, 0x1ff7);
        assert_eq!(read(&mut c, 0x1abc), Some(1));
        let mut c = cart(0x8000, 0x1000, Scheme::F4, false);
        read(&mut c, 0x1ff4 + 5);
        assert_eq!(read(&mut c, 0x1100), Some(5));
        assert_eq!(c.peek(0x1ffb), 5, "peek has no side effects");
        assert_eq!(read(&mut c, 0x1100), Some(5));
    }

    #[test]
    fn superchip_and_fa_ram_ports() {
        let mut c = cart(0x2000, 0x1000, Scheme::F8, true);
        c.access(0x1005, Some(0x42));
        assert_eq!(read(&mut c, 0x1085), Some(0x42));
        assert_eq!(read(&mut c, 0x1005), None, "write port drives nothing");
        read(&mut c, 0x1ff8);
        assert_eq!(read(&mut c, 0x1085), Some(0x42), "RAM in every bank");
        let mut c = cart(0x3000, 0x1000, Scheme::Fa, false);
        assert_eq!(read(&mut c, 0x1200), Some(2));
        c.access(0x10ff, Some(9));
        assert_eq!(read(&mut c, 0x11ff), Some(9));
        read(&mut c, 0x1ff9);
        assert_eq!(read(&mut c, 0x1200), Some(1));
    }

    #[test]
    fn e0_segments() {
        let mut c = cart(0x2000, 0x400, Scheme::E0, false);
        assert_eq!(read(&mut c, 0x1c00), Some(7), "segment 3: last slice");
        read(&mut c, 0x1fe5); // segment 0 ← slice 5
        read(&mut c, 0x1fea); // segment 1 ← slice 2
        read(&mut c, 0x1ff6); // segment 2 ← slice 6
        let segs: Vec<_> = [0x1000, 0x1400, 0x1800, 0x1c00]
            .iter()
            .map(|&a| c.peek(a))
            .collect();
        assert_eq!(segs, [5, 2, 6, 7]);
    }

    #[test]
    fn e7_slices_and_ram() {
        let mut c = cart(0x4000, 0x800, Scheme::E7, false);
        assert_eq!(read(&mut c, 0x1a00), Some(7), "last slice fixed");
        read(&mut c, 0x1fe3);
        assert_eq!(read(&mut c, 0x1000), Some(3));
        read(&mut c, 0x1fe7); // 1 KB RAM at $1000
        c.access(0x1010, Some(0x77));
        assert_eq!(read(&mut c, 0x1410), Some(0x77));
        read(&mut c, 0x1fea); // 256-byte block 2
        c.access(0x1801, Some(0x55));
        assert_eq!(read(&mut c, 0x1901), Some(0x55));
        read(&mut c, 0x1fe8);
        assert_eq!(read(&mut c, 0x1901), Some(0), "another block");
        read(&mut c, 0x1fe1);
        assert_eq!(read(&mut c, 0x1410), Some(1), "ROM slice again");
    }

    #[test]
    fn tigervision_writes_below_0x40() {
        let mut c = cart(0x2000, 0x800, Scheme::Tigervision, false);
        assert_eq!(read(&mut c, 0x1800), Some(3), "last slice fixed");
        c.access(0x003f, Some(2));
        assert_eq!(read(&mut c, 0x1000), Some(2));
        c.access(0x0020, Some(3));
        assert_eq!(read(&mut c, 0x1000), Some(3), "the last slice too");
        c.access(0x003f, Some(2));
        c.access(0x0040, Some(1));
        assert_eq!(read(&mut c, 0x1000), Some(2), "above $3F: TIA only");
        read(&mut c, 0x003f);
        assert_eq!(read(&mut c, 0x1000), Some(2), "reads do not switch");
    }

    #[test]
    fn fe_follows_jsr_and_rts() {
        let mut c = cart(0x2000, 0x1000, Scheme::Fe, false);
        assert_eq!(c.peek(0x1000), 1);
        // JSR $F000-bank routine: push PCH ($01FF), PCL ($01FE), fetch
        // the target's high byte $F0 (D5 = 1).
        c.access(0x01ff, Some(0xd0));
        c.snoop(0x01ff, 0xd0);
        c.access(0x01fe, Some(0x12));
        c.snoop(0x01fe, 0x12);
        c.access(0x1002, None);
        c.snoop(0x1002, 0xf0);
        assert_eq!(c.peek(0x1000), 0);
        // RTS: pull PCL ($01FE), then PCH $D0 (D5 = 0).
        c.access(0x01fe, None);
        c.snoop(0x01fe, 0x12);
        c.access(0x01ff, None);
        c.snoop(0x01ff, 0xd0);
        assert_eq!(c.peek(0x1000), 1);
    }

    #[test]
    fn detection() {
        let mut r = vec![0xeau8; 0x2000];
        assert_eq!(detect(&r).unwrap(), (Scheme::F8, true), "uniform RAM ports");
        r[0x200] = 1;
        r[0x1100] = 2;
        r[0x1050] = 3;
        assert_eq!(detect(&r).unwrap(), (Scheme::F8, false));
        r[0x0ffd] = 0xf0;
        r[0x1ffd] = 0xf0;
        for i in 0..10 {
            r[0x400 + 3 * i..0x403 + 3 * i].copy_from_slice(&[0x8d, i as u8, 0xf0]);
        }
        assert_eq!(
            detect(&r).unwrap(),
            (Scheme::F8, true),
            "stores into the RAM"
        );
        r[0x400..0x41e].fill(0xea);
        r[0x300..0x304].copy_from_slice(&[0x85, 0x3f, 0x85, 0x3f]);
        assert_eq!(detect(&r).unwrap().0, Scheme::Tigervision);
        let mut e0 = vec![0u8; 0x2000];
        e0[0x10..0x16].copy_from_slice(&[0xad, 0xe0, 0x1f, 0x8d, 0xf3, 0xff]);
        assert_eq!(detect(&e0).unwrap().0, Scheme::E0);
        let mut e7 = vec![1u8; 0x4000];
        e7[0x10..0x16].copy_from_slice(&[0xad, 0xe2, 0xff, 0x2c, 0xe7, 0x1f]);
        assert_eq!(detect(&e7).unwrap().0, Scheme::E7);
        assert_eq!(detect(&vec![0u8; 0x3000]).unwrap().0, Scheme::Fa);
        let mut fe = vec![0u8; 0x2000];
        fe[0x0ffc..0x0ffe].copy_from_slice(&[0x00, 0xf0]);
        fe[0x1ffc..0x1ffe].copy_from_slice(&[0x4e, 0xcc]);
        assert_eq!(detect(&fe).unwrap().0, Scheme::Fe);
        assert!(detect(&[0u8; 0x1800]).is_err());
        assert_eq!(Scheme::from_name("f6sc"), Some((Scheme::F6, true)));
        assert_eq!(Scheme::from_name("e0sc"), None);
    }

    #[test]
    fn dpc_banks_registers_and_detection() {
        let mut r = rom(0x2800, 0x1000);
        r[0x2000 + 0x7ff] = 0xab; // display byte at counter 0
        r.extend((0..255).map(|i| i as u8)); // the random number dump
        assert_eq!(detect(&r).unwrap(), (Scheme::Dpc, false));
        assert_eq!(detect(&r[..0x2800]).unwrap(), (Scheme::Dpc, false));
        let mut c = Cart2600::parse(&r).unwrap();
        assert_eq!(read(&mut c, 0x1100), Some(1), "starts in bank 1");
        read(&mut c, 0x1ff8);
        assert_eq!(read(&mut c, 0x1100), Some(0), "F8 hotspots");
        assert_eq!(read(&mut c, 0x1040), Some(0), "write registers read as ROM");
        c.access(0x1053, Some(0)); // DF3 counter 0
        assert_eq!(c.peek(0x100b), 0xab);
        assert_eq!(read(&mut c, 0x100b), Some(0xab), "DF3 data");
        assert_eq!(read(&mut c, 0x1000), Some(1), "random number");
        assert_eq!(Scheme::from_name("dpc"), Some((Scheme::Dpc, false)));
        // Music: DF5 in music mode with top 1 toggles at 10 kHz.
        c.access(0x1045, Some(1));
        c.access(0x105d, Some(0x10));
        let before = c.peek(0x1004);
        let mut changes = 0;
        let mut last = before;
        for _ in 0..1_193 {
            c.tick(1_193_182);
            let v = c.peek(0x1004);
            changes += u32::from(v != last);
            last = v;
        }
        assert!(
            (19..=21).contains(&changes),
            "{changes} amplitude changes in 1 ms"
        );
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = Cart2600::parse(&r).unwrap();
        back.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = cart(0x4000, 0x800, Scheme::E7, false);
        read(&mut c, 0x1fe7);
        c.access(0x1001, Some(9));
        read(&mut c, 0x1fe9);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = cart(0x4000, 0x800, Scheme::E7, false);
        back.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, c);
        let mut other = cart(0x4000, 0x1000, Scheme::F6, false);
        assert!(other.load(&mut StateReader::new(&bytes)).is_err());
    }
}
