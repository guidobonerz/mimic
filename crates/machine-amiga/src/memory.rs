//! The address map (HRM appendix "Memory map"; Gary's decoding):
//!
//! | Range | Contents |
//! | --- | --- |
//! | $000000–$1FFFFF | chip RAM (mirrored: 512 KB repeat four times, 1 MB twice); while OVL is high the ROM reads at $000000–$07FFFF (writes still reach chip RAM) |
//! | $200000–$9FFFFF | Zorro II expansion space: fast RAM once its board is configured |
//! | $A00000–$BFFFFF | CIAs: A12 = 0 selects CIA-A (odd bytes, D0–D7), A13 = 0 CIA-B (even bytes, D8–D15); register = A11–A8 |
//! | $C00000–$D7FFFF | slow ("trapdoor") RAM; above it the custom registers appear mirrored (Kickstart finds the end of slow RAM that way) |
//! | $DFF000–$DFFFFF | custom chip registers |
//! | $E80000–$E8FFFF | autoconfig: the next unconfigured expansion board ([`Autoconfig`]) |
//! | $F80000–$FFFFFF | Kickstart ROM (256 KB images repeat); A1000: bootstrap ROM and writable control store ([`Wcs`]) |
//!
//! Everything else reads 0 and ignores writes.
//!
//! **Fast RAM** sits on an autoconfig board (HRM appendix "Expansion"
//! / `libraries/configregs.h`, the Zorro II protocol): at $E80000 the
//! board shows its configuration ROM as nibbles — byte `n` of the ROM at
//! $E80000 + 4n (high nibble) and + 4n + 2 (low nibble), in the upper four
//! data bits; every byte except `er_Type` is stored inverted. The ROM
//! says: `er_Type` $C0 (Zorro II) | $20 (link into the free memory list) |
//! size code (001 = 64 KB … 111 = 4 MB, 000 = 8 MB), product, flags,
//! manufacturer, serial number. Kickstart's expansion.library writes the
//! base address it chose: bits A19–A16 to $E8004A, then A23–A20 to
//! $E80048, which configures the board (it then answers at its base and
//! no longer at $E80000); a write to $E8004C ("shut up") disables it.
//!
//! **A1000.** The Kickstart lives in 256 KB of RAM, the writable control
//! store at $FC0000–$FFFFFF, loaded by the bootstrap ROM from the
//! Kickstart disk. Until the WCS is write-protected the bootstrap ROM
//! answers at $F80000–$FBFFFF (and at address 0 under OVL); writing to the
//! bootstrap ROM's range sets the write-protect latch, after which the WCS
//! shows at $F80000–$FFFFFF like a 256 KB ROM and reboots come from it. A
//! power cycle clears the latch and the WCS.

use chips_amiga::ChipRam;
use drives_other::ata::Ata;

use crate::pcmcia::{self, Pcmcia};
use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// Kickstart image size in the map.
pub const ROM_SIZE: usize = 0x8_0000;
/// Start of the Zorro II expansion space.
pub const EXPANSION_BASE: u32 = 0x20_0000;
pub const EXPANSION_END: u32 = 0xa0_0000;
/// The autoconfig window.
pub const AUTOCONFIG: u32 = 0xe8_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Chip,
    Slow,
    Fast,
    Custom,
    Cia,
    /// A1200: Gayle's IDE and status registers ($DA0000–$DAFFFF) and its
    /// id register ($DE1000).
    Gayle,
    Rom,
    /// The A1000 bootstrap ROM.
    Boot,
    /// The A1000 writable control store, not write-protected yet.
    Wcs,
    Autoconfig,
    Open,
}

impl Region {
    /// Accesses that go over the chip bus and wait for a free DMA slot.
    pub fn chip_bus(self) -> bool {
        matches!(self, Region::Chip | Region::Slow | Region::Custom)
    }
}

/// The A1200's Gayle (IDE interface, PCMCIA, system control).
///
/// **IDE** ($DA0000–$DA7FFF): the ATA task file ([`Ata`]), register = A4–A2,
/// A12 selects the control block (CS1: alternate status / device control at
/// register 6, so $DA101A or $DA301A); register 0 is the 16-bit data
/// register, the others are bytes in the word's upper half (D15–D8). The
/// data bus is wired byte-swapped: the drive's D7–D0 are the CPU's D15–D8,
/// so a block's bytes arrive in their order on the disk (an image made by
/// the Amiga or by WinUAE reads as written) and IDENTIFY's numbers come
/// swapped (scsi.device swaps them back; with straight wiring it set 0
/// sectors and head $F). The
/// ROM's scsi.device uses $DA2000 + 4·reg (+2 for the byte registers),
/// the Linux driver $DA0000 + 2 + 4·reg: both are the same registers. With
/// no drive every IDE register reads $FF, the floating bus (status $FF: no
/// device; with 0 scsi.device waited for a drive to become ready —
/// Kickstart 40.068 never reached insert-disk).
///
/// **Gayle's registers** (bytes, D15–D8): $DA8000 card status (7 the IDE
/// drive's INTRQ, 6 card detect …), $DA9000 interrupt requests (7 IDE: set
/// when INTRQ rises; writing a 0 to bits 7–2 clears them, bits 1–0 are
/// written as given), $DAA000 interrupt enables (7 IDE → INT2), $DAB000
/// configuration (PCMCIA speed and voltage, kept). The IDE request with its
/// enable drives INT2 (Paula's PORTS, level 2) together with CIA-A.
///
/// **Id** at $DE1000: the chip's id ($D0 on the A1200) bit by bit in D7,
/// most significant bit first, a read per bit; a write restarts the
/// sequence. Kickstart 3.x reads it to tell an A1200 from an A500/A2000 and
/// then probes the IDE port.
///
/// **PCMCIA** ([`Pcmcia`]): common memory $600000–$9FFFFF, attribute
/// memory $A00000–$A1FFFF, I/O $A20000–$A3FFFF, reset $A40000. The card
/// status register shows the card: 6 CCDET (a card is in), 5 BVD1 and 4
/// BVD2 (its battery is good), 3 WR (writable), 2 RDY (ready); a card
/// going in or out sets request bit 6, which with its enable is INT6 (the
/// NetBSD driver's level for card detect); the others stay quiet on an
/// SRAM card.
///
/// Sources (`docs/sources.md`): the Linux kernel's `amigayle.h` and Gayle
/// IDE driver (register addresses and bits), NetBSD's Gayle PCMCIA driver
/// (the card detect interrupt's level), Commodore's NDK 3.1 PCMCIA notes
/// (the address spaces), the amiga-dev wiki (Gayle).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gayle {
    /// Bits of the id delivered so far.
    id_bits: u8,
    pub ide: Ata,
    card_status: u8,
    intreq: u8,
    inten: u8,
    config: u8,
    /// INTRQ as last seen (its rising edge sets the request).
    irq_line: bool,
    pub card: Pcmcia,
}

impl Default for Gayle {
    fn default() -> Self {
        let mut ide = Ata::new();
        for d in &mut ide.disks {
            d.prepare = Some(media::hdf::with_rdb);
        }
        Self {
            id_bits: 0,
            ide,
            card_status: 0,
            intreq: 0,
            inten: 0,
            config: 0,
            irq_line: false,
            card: Pcmcia::default(),
        }
    }
}

/// Gayle's id on the A1200.
pub const GAYLE_ID_A1200: u8 = 0xd0;
/// The IDE bit of Gayle's status, request and enable registers.
pub const GAYLE_IDE: u8 = 0x80;
/// Card detect: in the status (a card is in), request and enable (it
/// went in or out).
pub const GAYLE_CCDET: u8 = 0x40;
const GAYLE_BVD1: u8 = 0x20;
const GAYLE_BVD2: u8 = 0x10;
const GAYLE_WR: u8 = 0x08;
const GAYLE_RDY: u8 = 0x04;

/// What a Gayle address is.
enum GayleReg {
    /// IDE register; true: the control block (CS1).
    Ide(u8, bool),
    CardStatus,
    IntReq,
    IntEna,
    Config,
    Id,
    /// PCMCIA common memory, offset.
    Common(u32),
    /// PCMCIA attribute memory, offset.
    Attribute(u32),
    /// PCMCIA I/O and the reset register.
    CardIo,
    None,
}

fn gayle_reg(addr: u32) -> GayleReg {
    let a = addr & 0xff_ffff;
    match a {
        0xda_0000..=0xda_7fff => GayleReg::Ide((a >> 2 & 7) as u8, a & 0x1000 != 0),
        0xda_8000..=0xda_8fff => GayleReg::CardStatus,
        0xda_9000..=0xda_9fff => GayleReg::IntReq,
        0xda_a000..=0xda_afff => GayleReg::IntEna,
        0xda_b000..=0xda_bfff => GayleReg::Config,
        0xde_1000..=0xde_1fff => GayleReg::Id,
        pcmcia::COMMON..=pcmcia::COMMON_END => GayleReg::Common(a - pcmcia::COMMON),
        0xa0_0000..=0xa1_ffff => GayleReg::Attribute(a - pcmcia::ATTRIBUTE),
        0xa2_0000..=0xa5_ffff => GayleReg::CardIo,
        _ => GayleReg::None,
    }
}

impl Gayle {
    /// A byte read from Gayle's space.
    pub fn read(&mut self, addr: u32) -> u8 {
        let v = match gayle_reg(addr) {
            GayleReg::Ide(0, false) => self.ide.read_data() as u8,
            GayleReg::Ide(6, true) => self.ide.alt_status(),
            GayleReg::Ide(r, false) => self.ide.read(r),
            GayleReg::Id => {
                let bit = (GAYLE_ID_A1200 << (self.id_bits & 7)) & 0x80;
                self.id_bits = self.id_bits.wrapping_add(1).min(8);
                bit
            }
            _ => self.peek(addr),
        };
        self.sync();
        v
    }

    /// A word read: the IDE data register gives all 16 bits, the others
    /// their byte in the upper half.
    pub fn read_word(&mut self, addr: u32) -> u16 {
        match gayle_reg(addr) {
            GayleReg::Ide(0, false) => {
                let v = self.ide.read_data().swap_bytes();
                self.sync();
                v
            }
            GayleReg::Common(o) => self.card.common_word(o),
            GayleReg::Attribute(o) => {
                u16::from(self.card.attribute(o)) << 8 | u16::from(self.card.attribute(o | 1))
            }
            _ => u16::from(self.read(addr)) << 8,
        }
    }

    pub fn peek(&self, addr: u32) -> u8 {
        match gayle_reg(addr) {
            GayleReg::Ide(6, true) => self.ide.alt_status(),
            GayleReg::Ide(_, true) => 0xff,
            GayleReg::Ide(r, false) => self.ide.peek(r),
            GayleReg::CardStatus => {
                let ide = if self.ide.irq() { GAYLE_IDE } else { 0 };
                let card = if self.card.present() {
                    let wr = if self.card.writable() { GAYLE_WR } else { 0 };
                    GAYLE_CCDET | GAYLE_BVD1 | GAYLE_BVD2 | GAYLE_RDY | wr
                } else {
                    0
                };
                ide | card
            }
            GayleReg::IntReq => self.intreq,
            GayleReg::IntEna => self.inten,
            GayleReg::Config => self.config,
            GayleReg::Id => (GAYLE_ID_A1200 << (self.id_bits & 7)) & 0x80,
            GayleReg::Common(o) => self.card.common(o),
            GayleReg::Attribute(o) => self.card.attribute(o),
            GayleReg::CardIo => 0xff,
            GayleReg::None => 0,
        }
    }

    pub fn peek_word(&self, addr: u32) -> u16 {
        match gayle_reg(addr) {
            GayleReg::Ide(0, false) => self.ide.peek_data().swap_bytes(),
            GayleReg::Common(o) => self.card.common_word(o),
            GayleReg::Attribute(o) => {
                u16::from(self.card.attribute(o)) << 8 | u16::from(self.card.attribute(o | 1))
            }
            _ => u16::from(self.peek(addr)) << 8,
        }
    }

    pub fn write(&mut self, addr: u32, v: u8) {
        match gayle_reg(addr) {
            GayleReg::Ide(0, false) => self.ide.write_data(u16::from(v) << 8 | u16::from(v)),
            GayleReg::Ide(6, true) => self.ide.write_control(v),
            GayleReg::Ide(_, true) => {}
            GayleReg::Ide(r, false) => self.ide.write(r, v),
            GayleReg::CardStatus => self.card_status = v & 0x7f,
            GayleReg::IntReq => self.intreq = (self.intreq & v & 0xfc) | (v & 3),
            GayleReg::IntEna => self.inten = v,
            GayleReg::Config => self.config = v & 0x0f,
            GayleReg::Id => self.id_bits = 0,
            GayleReg::Common(o) => self.card.set_common(o, v),
            // Attribute memory is the CIS (ROM here); I/O and reset have
            // nothing on an SRAM card.
            GayleReg::Attribute(_) | GayleReg::CardIo | GayleReg::None => {}
        }
        self.sync();
    }

    /// A word write: the data register takes all 16 bits.
    pub fn write_word(&mut self, addr: u32, v: u16) {
        match gayle_reg(addr) {
            GayleReg::Ide(0, false) => {
                self.ide.write_data(v.swap_bytes());
                self.sync();
            }
            GayleReg::Common(o) => {
                self.card.set_common(o, (v >> 8) as u8);
                self.card.set_common(o | 1, v as u8);
            }
            _ => self.write(addr, (v >> 8) as u8),
        }
    }

    /// INTRQ's rising edge sets the IDE request; a card going in or out
    /// the card detect request.
    pub fn sync(&mut self) {
        let line = self.ide.irq();
        if line && !self.irq_line {
            self.intreq |= GAYLE_IDE;
        }
        self.irq_line = line;
        if std::mem::take(&mut self.card.changed) {
            self.intreq |= GAYLE_CCDET;
        }
    }

    /// Gayle's INT2 output (the IDE drive).
    pub fn int2(&self) -> bool {
        self.intreq & self.inten & GAYLE_IDE != 0
    }

    /// Gayle's INT6 output (card detect).
    pub fn int6(&self) -> bool {
        self.intreq & self.inten & GAYLE_CCDET != 0
    }

    /// The system reset line: the IDE drives reset too.
    pub fn reset(&mut self) {
        self.ide.reset();
        self.intreq = 0;
        self.inten = 0;
        self.card_status = 0;
        self.config = 0;
        self.irq_line = false;
        self.id_bits = 0;
        // A card in the slot is no news after a reset.
        self.card.changed = false;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.id_bits);
        w.u8(self.card_status);
        w.u8(self.intreq);
        w.u8(self.inten);
        w.u8(self.config);
        w.bool(self.irq_line);
        self.ide.save(w);
        self.card.save(w);
    }

    /// `version` 6 states hold only the id shift, 7 no card.
    pub fn load(&mut self, r: &mut StateReader<'_>, version: u16) -> Result<(), StateError> {
        self.id_bits = r.u8()?.min(8);
        if version < 7 {
            return Ok(());
        }
        self.card_status = r.u8()? & 0x7f;
        self.intreq = r.u8()?;
        self.inten = r.u8()?;
        self.config = r.u8()? & 0x0f;
        self.irq_line = r.bool()?;
        self.ide.load(r)?;
        if version >= 8 {
            self.card.load(r)?;
        }
        Ok(())
    }
}

/// A Zorro II fast RAM board going through autoconfig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Autoconfig {
    pub ram: Vec<u16>,
    /// Base address once configured.
    pub base: Option<u32>,
    /// Shut up: never configured.
    pub disabled: bool,
    /// A19–A16 nibble written to $4A, waiting for $48.
    low_nibble: u8,
}

impl Autoconfig {
    /// `bytes`: 64 KB … 8 MB (a power of two).
    pub fn new(bytes: usize) -> Self {
        Self {
            ram: vec![0; bytes / 2],
            base: None,
            disabled: false,
            low_nibble: 0,
        }
    }

    pub fn len_bytes(&self) -> usize {
        self.ram.len() * 2
    }

    /// Byte `n` of the configuration ROM.
    fn rom_byte(&self, n: u32) -> u8 {
        let size_code = match self.len_bytes() {
            0x1_0000 => 1,
            0x2_0000 => 2,
            0x4_0000 => 3,
            0x8_0000 => 4,
            0x10_0000 => 5,
            0x20_0000 => 6,
            0x40_0000 => 7,
            _ => 0,
        };
        // Manufacturer 2011: the number reserved for unregistered hardware.
        let plain = match n {
            0x00 => 0xc0 | 0x20 | size_code,
            0x01 => 0x01,
            0x02 => 0x00,
            0x04 => 0x07,
            0x05 => 0xdb,
            0x0b => 0x01,
            _ => 0x00,
        };
        if n == 0 {
            plain
        } else {
            !plain
        }
    }

    /// Does the board answer at $E80000 now?
    pub fn unconfigured(&self) -> bool {
        self.base.is_none() && !self.disabled
    }

    /// A byte read in the autoconfig window.
    pub fn read(&self, offset: u32) -> u8 {
        if !self.unconfigured() {
            return 0xff;
        }
        let n = offset >> 2;
        let b = self.rom_byte(n & 0x3f);
        let nibble = if offset & 2 == 0 { b >> 4 } else { b & 0x0f };
        nibble << 4 | 0x0f
    }

    /// A byte write in the autoconfig window.
    pub fn write(&mut self, offset: u32, v: u8) {
        if !self.unconfigured() {
            return;
        }
        match offset & !1 {
            0x48 => {
                let base = u32::from(v & 0xf0) << 16 | u32::from(self.low_nibble) << 16;
                self.base = Some(base);
            }
            0x4a => self.low_nibble = v >> 4,
            0x4c => self.disabled = true,
            _ => {}
        }
    }

    fn index(&self, addr: u32) -> Option<usize> {
        let base = self.base?;
        let off = addr.checked_sub(base)? as usize;
        (off < self.len_bytes()).then_some(off >> 1)
    }

    pub fn word(&self, addr: u32) -> u16 {
        self.index(addr).map_or(0, |i| self.ram[i])
    }

    pub fn set_word(&mut self, addr: u32, v: u16) {
        if let Some(i) = self.index(addr) {
            self.ram[i] = v;
        }
    }
}

/// The A1000's bootstrap ROM and the write-protect latch of its WCS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wcs {
    /// Bootstrap ROM words (8 KB … 64 KB, repeated over $F80000–$FBFFFF).
    pub boot: Vec<u16>,
    pub protected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub chip: ChipRam,
    /// Slow RAM words (empty: none fitted).
    pub slow: Vec<u16>,
    /// 512 KB of ROM words (A1000: the writable control store, twice).
    pub rom: Vec<u16>,
    /// OVL: the ROM overlays address 0.
    pub overlay: bool,
    pub fast: Option<Autoconfig>,
    /// A1200.
    pub gayle: Option<Gayle>,
    pub a1000: Option<Wcs>,
}

fn words(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u16::from_be_bytes(*w))
        .collect()
}

impl Memory {
    /// `rom`: a 256 or 512 KB Kickstart image.
    pub fn new(chip_bytes: usize, slow_bytes: usize, rom: &[u8]) -> Self {
        let mut words = words(rom);
        if words.is_empty() {
            words.push(0);
        }
        while words.len() < ROM_SIZE / 2 {
            let again = words.clone();
            words.extend(again);
        }
        words.truncate(ROM_SIZE / 2);
        Self {
            chip: ChipRam::new(chip_bytes),
            slow: vec![0; slow_bytes / 2],
            rom: words,
            overlay: true,
            fast: None,
            gayle: None,
            a1000: None,
        }
    }

    /// Fit the A1200's Gayle.
    pub fn with_gayle(mut self) -> Self {
        self.gayle = Some(Gayle::default());
        self
    }

    /// Fit a fast RAM board.
    pub fn with_fast(mut self, bytes: usize) -> Self {
        self.fast = (bytes > 0).then(|| Autoconfig::new(bytes));
        self
    }

    /// A1000: `boot` the bootstrap ROM (the WCS starts empty and
    /// unprotected), or `None` with the Kickstart already in the WCS and
    /// the latch set.
    pub fn with_a1000(mut self, boot: Option<&[u8]>) -> Self {
        self.a1000 = Some(match boot {
            Some(b) => {
                self.rom = vec![0; ROM_SIZE / 2];
                Wcs {
                    boot: words(b),
                    protected: false,
                }
            }
            None => Wcs {
                boot: Vec::new(),
                protected: true,
            },
        });
        self
    }

    /// Address bits the chip RAM takes (19 for 512 KB).
    pub fn chip_bits(&self) -> u8 {
        self.chip.len_bytes().trailing_zeros() as u8
    }

    /// Highest address of the A1000 bootstrap ROM range when it answers.
    fn boot_rom_live(&self) -> bool {
        self.a1000
            .as_ref()
            .is_some_and(|w| !w.protected && !w.boot.is_empty())
    }

    pub fn region(&self, addr: u32, write: bool) -> Region {
        let a = addr & 0xff_ffff;
        match a {
            0x00_0000..=0x07_ffff if self.overlay && !write => {
                if self.boot_rom_live() {
                    Region::Boot
                } else {
                    Region::Rom
                }
            }
            0x00_0000..=0x1f_ffff => Region::Chip,
            // A600/A1200: the PCMCIA slot's common memory where no fast RAM
            // board answers, its attribute, I/O and reset spaces.
            pcmcia::COMMON..=pcmcia::COMMON_END if self.gayle.is_some() => match &self.fast {
                Some(f) if f.index(a).is_some() => Region::Fast,
                _ => Region::Gayle,
            },
            0xa0_0000..=0xa5_ffff if self.gayle.is_some() => Region::Gayle,
            EXPANSION_BASE..=0x9f_ffff => match &self.fast {
                Some(f) if f.index(a).is_some() => Region::Fast,
                _ => Region::Open,
            },
            0xa0_0000..=0xbf_ffff => Region::Cia,
            0xc0_0000..=0xd7_ffff => {
                if ((a - 0xc0_0000) as usize) < self.slow.len() * 2 {
                    Region::Slow
                } else {
                    Region::Custom
                }
            }
            0xda_0000..=0xda_ffff | 0xde_0000..=0xde_ffff if self.gayle.is_some() => Region::Gayle,
            0xdf_f000..=0xdf_ffff => Region::Custom,
            AUTOCONFIG..=0xe8_ffff => match &self.fast {
                Some(f) if f.unconfigured() => Region::Autoconfig,
                _ => Region::Open,
            },
            0xf8_0000..=0xfb_ffff if self.boot_rom_live() => Region::Boot,
            0xf8_0000..=0xff_ffff => match &self.a1000 {
                Some(w) if !w.protected => Region::Wcs,
                _ => Region::Rom,
            },
            _ => Region::Open,
        }
    }

    pub fn rom_word(&self, addr: u32) -> u16 {
        self.rom[(addr as usize >> 1) & (ROM_SIZE / 2 - 1)]
    }

    /// A1000: write into the unprotected WCS.
    pub fn set_wcs_word(&mut self, addr: u32, v: u16) {
        let i = (addr as usize >> 1) & (ROM_SIZE / 2 - 1);
        self.rom[i] = v;
        // The 256 KB appear twice.
        self.rom[i ^ (ROM_SIZE / 4)] = v;
    }

    pub fn boot_word(&self, addr: u32) -> u16 {
        match &self.a1000 {
            Some(w) if !w.boot.is_empty() => w.boot[(addr as usize >> 1) % w.boot.len()],
            _ => 0,
        }
    }

    /// A1000: a write into the bootstrap ROM's range sets the latch.
    pub fn protect_wcs(&mut self) {
        if let Some(w) = &mut self.a1000 {
            w.protected = true;
        }
    }

    pub fn slow_word(&self, addr: u32) -> u16 {
        let i = ((addr & 0xff_ffff) - 0xc0_0000) as usize >> 1;
        self.slow.get(i).copied().unwrap_or(0)
    }

    pub fn set_slow_word(&mut self, addr: u32, v: u16) {
        let i = ((addr & 0xff_ffff) - 0xc0_0000) as usize >> 1;
        if let Some(w) = self.slow.get_mut(i) {
            *w = v;
        }
    }

    pub fn set_slow_byte(&mut self, addr: u32, v: u8) {
        let w = self.slow_word(addr & !1);
        self.set_slow_word(addr & !1, merge_byte(w, addr, v));
    }

    pub fn fast_word(&self, addr: u32) -> u16 {
        self.fast.as_ref().map_or(0, |f| f.word(addr))
    }

    pub fn set_fast_word(&mut self, addr: u32, v: u16) {
        if let Some(f) = &mut self.fast {
            f.set_word(addr, v);
        }
    }

    /// Power-on: RAM cleared, the expansion board unconfigured, the A1000
    /// WCS empty again if a bootstrap ROM loads it.
    pub fn clear(&mut self) {
        self.chip.clear();
        self.slow.fill(0);
        if let Some(f) = &mut self.fast {
            *f = Autoconfig::new(f.len_bytes());
        }
        if let Some(w) = &mut self.a1000 {
            if !w.boot.is_empty() {
                w.protected = false;
                self.rom.fill(0);
            }
        }
    }

    /// The expansion board starts autoconfig again (RESET line).
    pub fn reset_expansion(&mut self) {
        if let Some(f) = &mut self.fast {
            let ram = std::mem::take(&mut f.ram);
            *f = Autoconfig::new(ram.len() * 2);
            f.ram = ram;
        }
    }
}

/// Replace the byte of `w` that `addr`'s low bit selects.
pub fn merge_byte(w: u16, addr: u32, v: u8) -> u16 {
    if addr & 1 == 0 {
        (w & 0x00ff) | u16::from(v) << 8
    } else {
        (w & 0xff00) | u16::from(v)
    }
}

const TAG: [u8; 4] = *b"AMEM";

impl Savestate for Memory {
    fn save(&self, w: &mut StateWriter) {
        // Version 2: fast RAM board and the A1000 WCS.
        w.chunk(TAG, 2, |w| {
            w.bool(self.overlay);
            w.u32(self.slow.len() as u32);
            for v in &self.slow {
                w.u16(*v);
            }
            match &self.fast {
                Some(f) => {
                    w.u32(f.ram.len() as u32);
                    w.bool(f.base.is_some());
                    w.u32(f.base.unwrap_or(0));
                    w.bool(f.disabled);
                    w.u8(f.low_nibble);
                    for v in &f.ram {
                        w.u16(*v);
                    }
                }
                None => w.u32(0),
            }
            match &self.a1000 {
                Some(wcs) => {
                    w.u8(1);
                    w.bool(wcs.protected);
                    let loads = !wcs.boot.is_empty();
                    w.bool(loads);
                    if loads {
                        for v in &self.rom {
                            w.u16(*v);
                        }
                    }
                }
                None => w.u8(0),
            }
        });
        self.chip.save(w);
    }

    /// Atomic; the ROMs and the fitted sizes are configuration.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(TAG, 2)?;
        let overlay = c.bool()?;
        if c.u32()? as usize != self.slow.len() {
            return Err(StateError::Invalid("slow RAM size differs"));
        }
        let mut slow = vec![0; self.slow.len()];
        for v in &mut slow {
            *v = c.u16()?;
        }
        let mut fast = self.fast.clone();
        let mut a1000 = self.a1000.clone();
        let mut rom = None;
        if version >= 2 {
            let n = c.u32()? as usize;
            if n != fast.as_ref().map_or(0, |f| f.ram.len()) {
                return Err(StateError::Invalid("fast RAM size differs"));
            }
            if let Some(f) = &mut fast {
                let configured = c.bool()?;
                let base = c.u32()?;
                f.base = configured.then_some(base);
                f.disabled = c.bool()?;
                f.low_nibble = c.u8()? & 0x0f;
                for v in &mut f.ram {
                    *v = c.u16()?;
                }
            }
            let has_wcs = c.u8()? == 1;
            if has_wcs != a1000.is_some() {
                return Err(StateError::Invalid("A1000 WCS differs"));
            }
            if let Some(w) = &mut a1000 {
                w.protected = c.bool()?;
                if c.bool()? {
                    if w.boot.is_empty() {
                        return Err(StateError::Invalid("A1000 WCS differs"));
                    }
                    let mut words = vec![0; ROM_SIZE / 2];
                    for v in &mut words {
                        *v = c.u16()?;
                    }
                    rom = Some(words);
                }
            }
        }
        let mut chip = self.chip.clone();
        chip.load(r)?;
        self.chip = chip;
        self.slow = slow;
        self.overlay = overlay;
        self.fast = fast;
        self.a1000 = a1000;
        if let Some(rom) = rom {
            self.rom = rom;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoconfig_shows_its_rom_and_takes_a_base_address() {
        let mut m = Memory::new(0x8_0000, 0, &[0; 0x4_0000]).with_fast(0x20_0000);
        assert_eq!(m.region(AUTOCONFIG, false), Region::Autoconfig);
        let f = m.fast.as_mut().unwrap();
        // er_Type: Zorro II, memory list, 2 MB = $E6.
        assert_eq!(f.read(0x00) & 0xf0, 0xe0);
        assert_eq!(f.read(0x02) & 0xf0, 0x60);
        // er_Product 1, inverted: $FE.
        assert_eq!(f.read(0x04) & 0xf0, 0xf0);
        assert_eq!(f.read(0x06) & 0xf0, 0xe0);
        // Manufacturer $07DB inverted: $F8 $24.
        assert_eq!(f.read(0x10) & 0xf0, 0xf0);
        assert_eq!(f.read(0x12) & 0xf0, 0x80);
        assert_eq!(f.read(0x14) & 0xf0, 0x20);
        assert_eq!(f.read(0x16) & 0xf0, 0x40);
        f.write(0x4a, 0x00);
        f.write(0x48, 0x20);
        assert_eq!(f.base, Some(0x20_0000));
        assert!(!f.unconfigured());
        assert_eq!(m.region(AUTOCONFIG, false), Region::Open);
        assert_eq!(m.region(0x20_0000, false), Region::Fast);
        assert_eq!(m.region(0x3f_fffe, true), Region::Fast);
        assert_eq!(m.region(0x40_0000, false), Region::Open);
        m.set_fast_word(0x21_0000, 0x1234);
        assert_eq!(m.fast_word(0x21_0000), 0x1234);
        m.reset_expansion();
        assert!(m.fast.as_ref().unwrap().unconfigured());
        assert_eq!(
            m.fast.as_ref().unwrap().ram[0x8000],
            0x1234,
            "RAM survives a reset"
        );
        m.fast.as_mut().unwrap().write(0x4c, 0);
        assert_eq!(m.region(AUTOCONFIG, false), Region::Open, "shut up");
    }

    #[test]
    fn a1000_bootstrap_rom_until_the_wcs_is_protected() {
        let boot = vec![0x12u8; 0x2000];
        let mut m = Memory::new(0x4_0000, 0, &[0; 0x4_0000]).with_a1000(Some(&boot));
        assert_eq!(
            m.region(0, false),
            Region::Boot,
            "reset vectors from the bootstrap ROM"
        );
        assert_eq!(m.region(0xf8_0000, false), Region::Boot);
        assert_eq!(m.region(0xfc_0000, false), Region::Wcs);
        assert_eq!(m.boot_word(0xf8_2000), 0x1212);
        m.set_wcs_word(0xfc_0100, 0xabcd);
        assert_eq!(m.rom_word(0xfc_0100), 0xabcd);
        assert_eq!(m.rom_word(0xf8_0100), 0xabcd, "the WCS repeats");
        m.protect_wcs();
        assert_eq!(m.region(0xf8_0000, false), Region::Rom);
        assert_eq!(m.region(0, false), Region::Rom);
        m.clear();
        assert_eq!(m.region(0xf8_0000, false), Region::Boot, "power cycle");
        assert_eq!(m.rom_word(0xfc_0100), 0);
        // Preloaded Kickstart: no bootstrap ROM, protected from the start.
        let m = Memory::new(0x4_0000, 0, &[0x33; 0x4_0000]).with_a1000(None);
        assert_eq!(m.region(0xf8_0000, false), Region::Rom);
        assert_eq!(m.rom_word(0xfc_0000), 0x3333);
    }

    #[test]
    fn one_megabyte_chip_ram_mirrors_twice() {
        let m = Memory::new(0x10_0000, 0, &[0; 0x4_0000]);
        assert_eq!(m.chip_bits(), 20);
        let mut m = m;
        m.chip.set_word(0x0f_fffe, 0x5555);
        assert_eq!(m.chip.word(0x1f_fffe), 0x5555);
        assert_eq!(m.chip.word(0x07_fffe), 0);
    }

    #[test]
    fn savestate_round_trip_with_fast_ram_and_wcs() {
        let boot = vec![0u8; 0x2000];
        let mut m = Memory::new(0x8_0000, 0x8_0000, &[0; 0x4_0000])
            .with_fast(0x8_0000)
            .with_a1000(Some(&boot));
        m.fast.as_mut().unwrap().write(0x48, 0x40);
        m.set_fast_word(0x40_0002, 0x4242);
        m.set_wcs_word(0xfc_0000, 0x1111);
        let mut w = StateWriter::new();
        m.save(&mut w);
        let bytes = w.into_bytes();
        let mut back = Memory::new(0x8_0000, 0x8_0000, &[0; 0x4_0000])
            .with_fast(0x8_0000)
            .with_a1000(Some(&boot));
        back.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(back, m);
        let mut other = Memory::new(0x8_0000, 0x8_0000, &[0; 0x4_0000]);
        assert!(other.load(&mut StateReader::new(&bytes)).is_err());
    }

    struct Mem(Vec<u8>);

    impl emu_core::media::MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), emu_core::media::MediaError> {
            b.copy_from_slice(&self.0[o as usize..o as usize + b.len()]);
            Ok(())
        }
        fn write(&mut self, o: u64, d: &[u8]) -> Result<(), emu_core::media::MediaError> {
            self.0[o as usize..o as usize + d.len()].copy_from_slice(d);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), emu_core::media::MediaError> {
            Ok(())
        }
    }

    #[test]
    fn gayle_swaps_the_ide_data_bus_and_raises_int2_on_intrq() {
        use emu_core::media::{Media, MediaKind, MediaSlot};
        let mut g = Gayle::default();
        assert_eq!(g.read(0xda_201c), 0xff, "no drive: the floating bus");
        assert_eq!(g.read_word(0xda_2000), 0xffff);
        // A disk with an RDB stays as it is; block 0 starts "RDSK".
        let mut disk = vec![0u8; 64 * 512];
        disk[..512].copy_from_slice(&media::hdf::rdb_blocks(32, 0x444f_5301)[..512]);
        g.ide.disks[0]
            .insert(Media {
                name: "hd.hdf".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Mem(disk)),
            })
            .unwrap();
        g.write(0xda_a000, GAYLE_IDE);
        // READ SECTORS, LBA 0.
        for (reg, v) in [(2, 1), (3, 0), (4, 0), (5, 0), (6, 0xe0), (7, 0x20)] {
            g.write(0xda_2000 + 4 * reg, v);
        }
        assert!(g.int2(), "INTRQ rose");
        assert_eq!(g.peek(0xda_8000) & GAYLE_IDE, GAYLE_IDE);
        assert_eq!(g.read(0xda_301a) & 0x08, 0x08, "alternate status: DRQ");
        // The block's bytes arrive in their order.
        assert_eq!(g.read_word(0xda_2000), u16::from_be_bytes(*b"RD"));
        assert_eq!(g.read_word(0xda_2000), u16::from_be_bytes(*b"SK"));
        // The status read drops INTRQ; the request stays until written 0.
        g.read(0xda_201c);
        assert_eq!(g.peek(0xda_8000) & GAYLE_IDE, 0);
        assert!(g.int2());
        g.write(0xda_9000, 0x7c | (g.peek(0xda_9000) & 3));
        assert!(!g.int2());
        // The id: $D0, a bit a read.
        g.write(0xde_1000, 0);
        let id = (0..8).fold(0u8, |id, _| id << 1 | g.read(0xde_1000) >> 7);
        assert_eq!(id, GAYLE_ID_A1200);
    }
}
