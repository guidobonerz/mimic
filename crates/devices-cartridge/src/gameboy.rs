//! Game Boy cartridges: the ROM, the memory bank controller its header
//! names, and the cartridge RAM (with a battery in many).
//!
//! **Header** (`$0100`–`$014F`): `$0147` the cartridge type, `$0148` the
//! ROM size (32 KB shifted left by it), `$0149` the RAM size (0 none, 1
//! 2 KB, 2 8 KB, 3 32 KB, 4 128 KB, 5 64 KB), `$014D` the header's
//! checksum (the sum of `$0134`–`$014C`, each byte and 1 subtracted from
//! 0), which the boot ROM checks.
//!
//! **Controllers.** Registers are written into the ROM's address space.
//!
//! - No controller (types `$00`, `$08`, `$09`): 32 KB, RAM if any always
//!   on.
//! - **MBC1** (`$01`–`$03`): `$0000` RAM enable (`$A` in the low nibble),
//!   `$2000` five bits of the ROM bank (0 selects 1), `$4000` two more
//!   bits, `$6000` the mode: in mode 1 the two bits also bank
//!   `$0000`–`$3FFF` and the RAM. A **multicart** (several games behind
//!   a menu, 1 MB) wires the two bits one place lower: the bank is
//!   `upper << 4 | low four bits`. The header does not tell; such an
//!   image is known by the logo that each game's header repeats at bank
//!   `$10`.
//! - **MBC2** (`$05`, `$06`): one register over `$0000`–`$3FFF`: with
//!   address bit 8 clear the RAM enable, set the ROM bank (four bits, 0
//!   selects 1); 512 half bytes of RAM in the chip, the upper half reads
//!   as ones.
//! - **MBC3** (`$0F`–`$13`): `$0000` RAM and clock enable, `$2000` seven
//!   bits of the ROM bank (0 selects 1), `$4000` the RAM bank (0–3) or a
//!   register of the clock (`$08` seconds, `$09` minutes, `$0A` hours,
//!   `$0B` the day's low byte, `$0C` its ninth bit, the halt bit 6 and the
//!   carry bit 7), `$6000` latches the clock on a 0 followed by a 1. The
//!   clock runs on emulated time ([`GbCart::clock_second`]).
//! - **MBC5** (`$19`–`$1E`): `$0000` RAM enable, `$2000` the ROM bank's
//!   low byte, `$3000` its ninth bit (bank 0 is bank 0), `$4000` four bits
//!   of the RAM bank.
//!
//! Sources: the Pan Docs (<https://gbdev.io/pandocs/>: the cartridge
//! header, MBC1, MBC2, MBC3, MBC5); Joonas Javanainen, *Game Boy: Complete
//! Technical Reference* (the MBC1's banking in both modes). The
//! implementation is this project's own.

use mimic_core::media::MediaError;
use mimic_core::{Savestate, StateError, StateReader, StateWriter};

use crate::CartSlot;

/// The largest image: 8 MB (MBC5's 512 banks).
pub const MAX_ROM: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mbc {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
}

/// The MBC3's clock: seconds, minutes, hours, days (9 bits), halt, carry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Clock {
    seconds: u8,
    minutes: u8,
    hours: u8,
    days: u16,
    halt: bool,
    carry: bool,
}

impl Clock {
    fn reg(&self, n: u8) -> u8 {
        match n {
            0x08 => self.seconds,
            0x09 => self.minutes,
            0x0a => self.hours,
            0x0b => self.days as u8,
            _ => (self.days >> 8) as u8 & 1 | u8::from(self.halt) << 6 | u8::from(self.carry) << 7,
        }
    }

    fn set_reg(&mut self, n: u8, v: u8) {
        match n {
            0x08 => self.seconds = v & 0x3f,
            0x09 => self.minutes = v & 0x3f,
            0x0a => self.hours = v & 0x1f,
            0x0b => self.days = self.days & 0x100 | u16::from(v),
            _ => {
                self.days = self.days & 0xff | u16::from(v & 1) << 8;
                self.halt = v & 0x40 != 0;
                self.carry = v & 0x80 != 0;
            }
        }
    }

    fn second(&mut self) {
        if self.halt {
            return;
        }
        self.seconds = (self.seconds + 1) & 0x3f;
        if self.seconds != 60 {
            return;
        }
        self.seconds = 0;
        self.minutes = (self.minutes + 1) & 0x3f;
        if self.minutes != 60 {
            return;
        }
        self.minutes = 0;
        self.hours = (self.hours + 1) & 0x1f;
        if self.hours != 24 {
            return;
        }
        self.hours = 0;
        self.days += 1;
        if self.days == 512 {
            self.days = 0;
            self.carry = true;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GbCart {
    rom: Vec<u8>,
    ram: Vec<u8>,
    pub mbc: Mbc,
    pub battery: bool,
    /// The cartridge has the MBC3's clock.
    pub timer: bool,
    ram_on: bool,
    /// The ROM bank register (MBC1: the five low bits).
    bank: u16,
    /// MBC1: the two upper bits; MBC3 and MBC5: the RAM bank or clock
    /// register.
    upper: u8,
    /// MBC1: mode 1.
    mode: bool,
    /// MBC1: wired as a multicart (told from the image, not state).
    multicart: bool,
    clock: Clock,
    latched: Clock,
    /// MBC3: the last byte written to the latch register.
    latch: u8,
}

impl GbCart {
    pub fn parse(bytes: &[u8]) -> Result<Self, MediaError> {
        if bytes.len() < 0x150 {
            return Err(MediaError::Format("too small for a Game Boy ROM".into()));
        }
        if bytes.len() > MAX_ROM {
            return Err(MediaError::Format(format!(
                "a {} byte image is larger than 8 MB",
                bytes.len()
            )));
        }
        let kind = bytes[0x147];
        let (mbc, has_ram, battery, timer) = match kind {
            0x00 => (Mbc::None, false, false, false),
            0x01 => (Mbc::Mbc1, false, false, false),
            0x02 => (Mbc::Mbc1, true, false, false),
            0x03 => (Mbc::Mbc1, true, true, false),
            0x05 => (Mbc::Mbc2, true, false, false),
            0x06 => (Mbc::Mbc2, true, true, false),
            0x08 => (Mbc::None, true, false, false),
            0x09 => (Mbc::None, true, true, false),
            0x0f => (Mbc::Mbc3, false, true, true),
            0x10 => (Mbc::Mbc3, true, true, true),
            0x11 => (Mbc::Mbc3, false, false, false),
            0x12 => (Mbc::Mbc3, true, false, false),
            0x13 => (Mbc::Mbc3, true, true, false),
            0x19 | 0x1c => (Mbc::Mbc5, false, false, false),
            0x1a | 0x1d => (Mbc::Mbc5, true, false, false),
            0x1b | 0x1e => (Mbc::Mbc5, true, true, false),
            other => {
                return Err(MediaError::Format(format!(
                    "cartridge type ${other:02X} is not supported (none, MBC1, MBC2, MBC3, MBC5)"
                )))
            }
        };
        let ram_len = match (mbc, has_ram, bytes[0x149]) {
            (Mbc::Mbc2, ..) => 512,
            (_, false, _) => 0,
            (_, _, 1) => 0x800,
            (_, _, 2) => 0x2000,
            (_, _, 3) => 0x8000,
            (_, _, 4) => 0x2_0000,
            (_, _, 5) => 0x1_0000,
            _ => 0,
        };
        // The image padded to whole banks, a power of two of them.
        let mut rom = bytes.to_vec();
        rom.resize(bytes.len().next_power_of_two().max(0x8000), 0xff);
        let multicart = mbc == Mbc::Mbc1
            && rom.len() == 0x10_0000
            && rom[0x4_0104..0x4_0134] == rom[0x0104..0x0134];
        Ok(Self {
            rom,
            ram: vec![0; ram_len],
            mbc,
            battery: battery && (ram_len > 0 || timer),
            timer,
            ram_on: false,
            bank: 1,
            upper: 0,
            mode: false,
            multicart,
            clock: Clock::default(),
            latched: Clock::default(),
            latch: 0xff,
        })
    }

    /// Power-on: the registers; the RAM keeps its contents.
    pub fn reset(&mut self) {
        self.ram_on = false;
        self.bank = 1;
        self.upper = 0;
        self.mode = false;
        self.latch = 0xff;
    }

    /// The title in the header (`$0134`–`$0143`, up to the first zero).
    pub fn title(&self) -> String {
        self.rom[0x134..0x144]
            .iter()
            .take_while(|b| **b != 0)
            .filter(|b| (0x20..0x7f).contains(*b))
            .map(|b| char::from(*b))
            .collect()
    }

    /// The header says it runs (only) on a Game Boy Color.
    pub fn color(&self) -> bool {
        self.rom[0x143] & 0x80 != 0
    }

    /// The header's checksum is right (the boot ROM stops otherwise).
    pub fn header_ok(&self) -> bool {
        let sum = self.rom[0x134..0x14d]
            .iter()
            .fold(0u8, |s, b| s.wrapping_sub(*b).wrapping_sub(1));
        sum == self.rom[0x14d]
    }

    fn banks(&self) -> usize {
        self.rom.len() / 0x4000
    }

    /// The ROM banks at `$0000` and `$4000`.
    fn rom_banks(&self) -> (usize, usize) {
        let (low, high) = match self.mbc {
            Mbc::None => (0, 1),
            Mbc::Mbc1 if self.multicart => {
                let upper = usize::from(self.upper) << 4;
                (
                    if self.mode { upper } else { 0 },
                    upper | usize::from(self.bank & 0x0f),
                )
            }
            Mbc::Mbc1 => {
                let upper = usize::from(self.upper) << 5;
                (
                    if self.mode { upper } else { 0 },
                    upper | usize::from(self.bank),
                )
            }
            _ => (0, usize::from(self.bank)),
        };
        (low % self.banks(), high % self.banks())
    }

    /// A read of `$0000`–`$7FFF`.
    pub fn read(&self, addr: u16) -> u8 {
        let (low, high) = self.rom_banks();
        let bank = if addr < 0x4000 { low } else { high };
        self.rom[bank * 0x4000 + usize::from(addr & 0x3fff)]
    }

    /// A write to `$0000`–`$7FFF`: the controller's registers.
    pub fn write(&mut self, addr: u16, v: u8) {
        match (self.mbc, addr >> 13) {
            (Mbc::None, _) => {}
            (Mbc::Mbc2, 0 | 1) => {
                if addr & 0x100 == 0 {
                    self.ram_on = v & 0x0f == 0x0a;
                } else {
                    self.bank = u16::from(v & 0x0f).max(1);
                }
            }
            (Mbc::Mbc2, _) => {}
            (_, 0) => self.ram_on = v & 0x0f == 0x0a,
            (Mbc::Mbc1, 1) => self.bank = u16::from(v & 0x1f).max(1),
            (Mbc::Mbc1, 2) => self.upper = v & 3,
            (Mbc::Mbc1, _) => self.mode = v & 1 != 0,
            (Mbc::Mbc3, 1) => self.bank = u16::from(v & 0x7f).max(1),
            (Mbc::Mbc3, 2) => self.upper = v & 0x0f,
            (Mbc::Mbc3, _) => {
                if self.latch == 0 && v == 1 {
                    self.latched = self.clock;
                }
                self.latch = v;
            }
            (Mbc::Mbc5, 1) => {
                if addr & 0x1000 == 0 {
                    self.bank = self.bank & 0x100 | u16::from(v);
                } else {
                    self.bank = self.bank & 0xff | u16::from(v & 1) << 8;
                }
            }
            (Mbc::Mbc5, 2) => self.upper = v & 0x0f,
            (Mbc::Mbc5, _) => {}
        }
    }

    /// The RAM byte `$A000`–`$BFFF` maps to now.
    fn ram_at(&self, addr: u16) -> Option<usize> {
        if self.ram.is_empty() {
            return None;
        }
        let bank = match self.mbc {
            Mbc::Mbc1 if self.mode => usize::from(self.upper),
            Mbc::Mbc3 | Mbc::Mbc5 => usize::from(self.upper),
            _ => 0,
        };
        Some((bank * 0x2000 + usize::from(addr & 0x1fff)) % self.ram.len())
    }

    fn enabled(&self) -> bool {
        self.ram_on || self.mbc == Mbc::None
    }

    /// A read of `$A000`–`$BFFF`; `$FF` with the RAM switched off.
    pub fn ram_read(&self, addr: u16) -> u8 {
        if !self.enabled() {
            return 0xff;
        }
        if self.mbc == Mbc::Mbc3 && self.upper >= 8 {
            return if self.timer {
                self.latched.reg(self.upper)
            } else {
                0xff
            };
        }
        match (self.ram_at(addr), self.mbc) {
            (Some(at), Mbc::Mbc2) => self.ram[at] | 0xf0,
            (Some(at), _) => self.ram[at],
            (None, _) => 0xff,
        }
    }

    pub fn ram_write(&mut self, addr: u16, v: u8) {
        if !self.enabled() {
            return;
        }
        if self.mbc == Mbc::Mbc3 && self.upper >= 8 {
            if self.timer {
                self.clock.set_reg(self.upper, v);
            }
            return;
        }
        if let Some(at) = self.ram_at(addr) {
            self.ram[at] = if self.mbc == Mbc::Mbc2 { v & 0x0f } else { v };
        }
    }

    /// A second of emulated time for the MBC3's clock.
    pub fn clock_second(&mut self) {
        if self.timer {
            self.clock.second();
        }
    }

    /// The battery-backed RAM, if the cartridge has a battery and RAM.
    pub fn battery_ram(&self) -> Option<&[u8]> {
        (self.battery && !self.ram.is_empty()).then_some(&self.ram[..])
    }

    /// Put a saved RAM back (of the RAM's size).
    pub fn set_battery_ram(&mut self, data: &[u8]) {
        if data.len() == self.ram.len() {
            self.ram.copy_from_slice(data);
        }
    }

    /// The ROM and RAM bytes, for the debugger's memory spaces.
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }
}

const STATE_TAG: [u8; 4] = *b"GBCT";

impl Savestate for GbCart {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(STATE_TAG, 1, |w| {
            w.bytes(&self.ram);
            w.bool(self.ram_on);
            w.u16(self.bank);
            w.u8(self.upper);
            w.bool(self.mode);
            w.u8(self.latch);
            for c in [&self.clock, &self.latched] {
                for n in 0x08..=0x0c {
                    w.u8(c.reg(n));
                }
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(STATE_TAG, 1)?;
        let ram = c.bytes()?;
        if ram.len() != self.ram.len() {
            return Err(StateError::Invalid("cartridge RAM size"));
        }
        let mut n = self.clone();
        n.ram = ram.to_vec();
        n.ram_on = c.bool()?;
        n.bank = c.u16()? & 0x1ff;
        n.upper = c.u8()? & 0x0f;
        n.mode = c.bool()?;
        n.latch = c.u8()?;
        for clock in [&mut n.clock, &mut n.latched] {
            for reg in 0x08..=0x0c {
                clock.set_reg(reg, c.u8()?);
            }
        }
        *self = n;
        Ok(())
    }
}

/// The Game Boy's cartridge slot (raw ROM images).
pub fn slot() -> CartSlot<GbCart> {
    CartSlot::new(GbCart::parse, |_| None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An image of `banks` × 16 KB, each bank's first byte its number
    /// (the low byte) and its second the high byte.
    fn image(kind: u8, banks: usize, ram: u8) -> Vec<u8> {
        let mut rom = vec![0u8; banks * 0x4000];
        for b in 0..banks {
            rom[b * 0x4000] = b as u8;
            rom[b * 0x4000 + 1] = (b >> 8) as u8;
        }
        rom[0x134..0x138].copy_from_slice(b"TEST");
        rom[0x147] = kind;
        rom[0x148] = (banks / 2).trailing_zeros() as u8;
        rom[0x149] = ram;
        let sum = rom[0x134..0x14d]
            .iter()
            .fold(0u8, |s, b| s.wrapping_sub(*b).wrapping_sub(1));
        rom[0x14d] = sum;
        rom
    }

    #[test]
    fn the_header_is_read() {
        let c = GbCart::parse(&image(0x13, 4, 3)).unwrap();
        assert_eq!((c.mbc, c.battery, c.timer), (Mbc::Mbc3, true, false));
        assert_eq!(c.ram().len(), 0x8000);
        assert_eq!(c.title(), "TEST");
        assert!(c.header_ok() && !c.color());
        assert!(c.battery_ram().is_some());
        let c = GbCart::parse(&image(0x01, 2, 3)).unwrap();
        assert!(c.ram().is_empty() && c.battery_ram().is_none());
        assert!(GbCart::parse(&image(0x20, 2, 0)).is_err());
        assert!(GbCart::parse(&[0; 0x100]).is_err());
        // Without a controller: 32 KB, RAM always on.
        let mut c = GbCart::parse(&image(0x09, 2, 2)).unwrap();
        c.ram_write(0xa123, 0x55);
        assert_eq!((c.read(0x4000), c.ram_read(0xa123)), (1, 0x55));
    }

    #[test]
    fn an_mbc1_multicart_is_told_by_its_logos() {
        let mut rom = vec![0u8; 0x10_0000];
        for bank in 0..64 {
            rom[bank * 0x4000] = bank as u8;
            rom[bank * 0x4000 + 0x2000] = bank as u8;
        }
        rom[0x147] = 0x01;
        for game in 0..4 {
            for i in 0..0x30 {
                rom[game * 0x4_0000 + 0x104 + i] = i as u8 + 1;
            }
        }
        let mut c = GbCart::parse(&rom).unwrap();
        // The upper bits count sixteen banks; the fifth low bit is not
        // wired, but still keeps the register from being 0.
        c.write(0x4000, 1);
        c.write(0x2000, 0x12);
        assert_eq!(c.read(0x4000), 0x12);
        c.write(0x2000, 0x10);
        assert_eq!(c.read(0x4000), 0x10);
        c.write(0x2000, 0x00);
        assert_eq!(c.read(0x4000), 0x11);
        assert_eq!(c.read(0x0000), 0);
        c.write(0x6000, 1);
        assert_eq!(c.read(0x0000), 0x10);
        // Without the logos the same image is an ordinary MBC1.
        rom[0x4_0104] = 0;
        let mut c = GbCart::parse(&rom).unwrap();
        c.write(0x4000, 1);
        c.write(0x2000, 0x12);
        assert_eq!(c.read(0x4000), 0x32);
    }

    #[test]
    fn mbc1_banks_rom_and_ram_in_both_modes() {
        let mut c = GbCart::parse(&image(0x03, 128, 3)).unwrap();
        assert_eq!((c.read(0x0000), c.read(0x4000)), (0, 1));
        c.write(0x2000, 0x00);
        assert_eq!(c.read(0x4000), 1, "bank 0 selects 1");
        c.write(0x2000, 0xe5);
        assert_eq!(c.read(0x4000), 5);
        c.write(0x4000, 0x02);
        assert_eq!(c.read(0x4000), 0x45);
        // Bank $20 cannot be reached: $21 instead.
        c.write(0x2000, 0x00);
        assert_eq!(c.read(0x4000), 0x41);
        assert_eq!(c.read(0x0000), 0, "mode 0");
        // The RAM is off until enabled; mode 1 banks it and the low ROM.
        assert_eq!(c.ram_read(0xa000), 0xff);
        c.write(0x0000, 0x0a);
        c.ram_write(0xa000, 0x11);
        c.write(0x6000, 0x01);
        assert_eq!(c.read(0x0000), 0x40);
        c.ram_write(0xa000, 0x22);
        c.write(0x4000, 0x00);
        assert_eq!(c.ram_read(0xa000), 0x11);
        c.write(0x4000, 0x02);
        assert_eq!(c.ram_read(0xa000), 0x22);
        c.write(0x0000, 0x00);
        assert_eq!(c.ram_read(0xa000), 0xff);
        // A small ROM wraps its banks.
        let mut c = GbCart::parse(&image(0x01, 4, 0)).unwrap();
        c.write(0x2000, 0x07);
        assert_eq!(c.read(0x4000), 3);
    }

    #[test]
    fn mbc2_has_one_register_and_half_bytes() {
        let mut c = GbCart::parse(&image(0x06, 16, 0)).unwrap();
        assert_eq!(c.ram().len(), 512);
        c.write(0x2100, 0x05);
        assert_eq!(c.read(0x4000), 5);
        c.write(0x0100, 0x00);
        assert_eq!(c.read(0x4000), 1);
        c.write(0x0000, 0x0a);
        c.ram_write(0xa000, 0xab);
        assert_eq!(c.ram_read(0xa000), 0xfb);
        assert_eq!(c.ram_read(0xa200), 0xfb, "mirrored");
    }

    #[test]
    fn mbc3_banks_and_keeps_time() {
        let mut c = GbCart::parse(&image(0x10, 128, 3)).unwrap();
        c.write(0x2000, 0x7f);
        assert_eq!(c.read(0x4000), 0x7f);
        c.write(0x2000, 0x00);
        assert_eq!(c.read(0x4000), 1);
        c.write(0x0000, 0x0a);
        c.write(0x4000, 0x03);
        c.ram_write(0xa000, 0x33);
        c.write(0x4000, 0x00);
        assert_eq!(c.ram_read(0xa000), 0);
        // 59 s + 2 s: a minute; the latch shows it.
        c.write(0x4000, 0x08);
        c.ram_write(0xa000, 59);
        c.clock_second();
        c.clock_second();
        assert_eq!(c.ram_read(0xa000), 0, "not latched yet");
        c.write(0x6000, 0x00);
        c.write(0x6000, 0x01);
        assert_eq!(c.ram_read(0xa000), 1);
        c.write(0x4000, 0x09);
        assert_eq!(c.ram_read(0xa000), 1);
        // Halted: the clock stands.
        c.write(0x4000, 0x0c);
        c.ram_write(0xa000, 0x40);
        c.clock_second();
        c.write(0x6000, 0x00);
        c.write(0x6000, 0x01);
        c.write(0x4000, 0x08);
        assert_eq!(c.ram_read(0xa000), 1);
    }

    #[test]
    fn mbc5_has_nine_bank_bits_and_bank_0() {
        let mut c = GbCart::parse(&image(0x1b, 512, 4)).unwrap();
        c.write(0x2000, 0x00);
        assert_eq!(c.read(0x4000), 0, "bank 0 is bank 0");
        c.write(0x2000, 0x34);
        c.write(0x3000, 0x01);
        assert_eq!((c.read(0x4000), c.read(0x4001)), (0x34, 0x01));
        c.write(0x0000, 0x0a);
        c.write(0x4000, 0x0f);
        c.ram_write(0xbfff, 0x77);
        assert_eq!(c.ram()[0x1_ffff], 0x77);
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = GbCart::parse(&image(0x10, 8, 2)).unwrap();
        c.write(0x0000, 0x0a);
        c.write(0x2000, 0x05);
        c.ram_write(0xa010, 0x99);
        c.clock_second();
        let mut w = StateWriter::new();
        c.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = GbCart::parse(&image(0x10, 8, 2)).unwrap();
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other, c);
        let mut wrong = GbCart::parse(&image(0x10, 8, 3)).unwrap();
        assert!(wrong.load(&mut StateReader::new(&bytes)).is_err());
    }
}
