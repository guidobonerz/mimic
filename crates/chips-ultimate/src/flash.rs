//! The SPI master for the configuration flash and the NOR flash chip
//! behind it (16 MiB).
//!
//! # The controller, I/O `$060200`–`$06020F`
//!
//! Only address bits 3:2 are decoded, so each register answers on four
//! byte addresses (and the four registers would repeat every 16 bytes in
//! a larger window; the block decodes any address it is given that way).
//!
//! | Address | Register | Read | Write |
//! |---|---|---|---|
//! | `$060200`–`$060203` | DATA | shifts `$FF` out, returns the byte shifted in | shifts the value out |
//! | `$060204`–`$060207` | RATE | 1 (the rate is fixed) | ignored |
//! | `$060208`–`$06020B` | CTRL | bit 0 FORCE, bit 1 LEVEL, bit 2 set (card-detect tie-off) | bit 0 FORCE, bit 1 LEVEL |
//! | `$06020C`–`$06020F` | CRC | 0 (no CRC in this instance) | ignored |
//!
//! A 32-bit access of DATA reaches the block as four byte accesses at
//! `+0`…`+3`: four transfers, the least significant byte first, so a word
//! in the processor's memory has the flash's byte order.
//!
//! Chip select: with FORCE = 0 the select line is low only while a byte
//! shifts, so every byte is a frame of its own (the firmware sends its
//! write enable/disable that way). With FORCE = 1 the line is the LEVEL
//! bit: CTRL = 1 opens a frame that lasts until CTRL changes, CTRL = 3
//! holds the chip deselected and bytes shifted then do not reach it.
//! After reset FORCE = 0, LEVEL = 1. There is no interrupt and no busy
//! state: a transfer is complete within the access.
//!
//! # The chips
//!
//! Two boards, two chips ([`FlashChip`]), both 16 MiB with 256-byte
//! pages and 4 KiB sectors:
//!
//! - **Infineon S25FL128L** (JEDEC `01 60 18`), assumed for the Ultimate
//!   64 Elite II. The firmware's driver for it reads pages with `13`,
//!   programs with `12` and erases with `21` (4-byte addresses), reads
//!   single bytes with `03`, unlocks with `65`/`01` and a `66`/`99`
//!   reset, and keeps 24 configuration pages.
//! - **Winbond W25Q128JV** (JEDEC `EF 40 18`) on the Commodore 64
//!   Ultimate. Its driver uses `03`, `02`, `20` (3-byte addresses),
//!   `01`/`31` for the status registers and keeps 16 configuration
//!   pages.
//!
//! Commands of the model (address bytes in parentheses; `S` S25FL128L
//! only, `W` W25Q128JV only):
//!
//! | Opcode | Command |
//! |---|---|
//! | `9F` | JEDEC ID |
//! | `4B` | unique ID: four dummy bytes, then eight ID bytes |
//! | `03` (3), `0B` (3 + dummy), `13` (4) S, `0C` (4 + dummy) S | read, address counts up and wraps at 16 MiB |
//! | `02` (3), `12` (4) S | page program: bits can only go from 1 to 0, the address wraps inside the 256-byte page |
//! | `20` (3), `21` (4) S | erase 4 KiB |
//! | `52` (3), `53` (4) S | erase 32 KiB |
//! | `D8` (3), `DC` (4) S | erase 64 KiB |
//! | `60`, `C7` | erase the chip (only while nothing is protected) |
//! | `06`, `04` | write enable latch set, cleared |
//! | `50` | write enable for the volatile status registers |
//! | `05` | status register 1: bit 7 SRP, 6 SEC, 5 TB, 4–2 BP2–0, 1 WEL, 0 BUSY (always 0) |
//! | `35`, `15` | W: status registers 2 and 3; S: configuration registers 1 and 2 |
//! | `07`, `33` S | status register 2 (bit 5 program error, bit 6 erase error), configuration register 3 |
//! | `01` | write status register 1 and, with more bytes, the following registers |
//! | `31`, `11` W | write status registers 2, 3 |
//! | `65` (3 + dummy), `71` (3) S | read, write any register |
//! | `30` S | clear the error bits |
//! | `66`, `99` | reset enable, reset: volatile registers reload, latch cleared |
//! | `90`, `AB` W | manufacturer/device ID |
//!
//! Everything else does nothing and reads `$FF`. Commands that change
//! something take effect when the frame ends, and only if the frame had
//! exactly the command's length (a program: at least one data byte).
//!
//! Protection: the BP scheme of both chips (they share it) — SEC, TB,
//! BP2–0 in status register 1 and CMP in the second register select a
//! range from the top or bottom, or its complement. A program or erase
//! that touches the range is not carried out. The firmware's updater
//! clears the bits before it writes the FPGA image and the application
//! and afterwards asks for the lowest 4 MiB (`$34`). The lock-down bit
//! (SRL / SRP1) refuses register writes until the next power cycle. The
//! /WP pin is high. Not modelled: one-time locks, individual block
//! locks (WPS), security regions, 4-byte address mode, suspend, QPI.
//!
//! Timing: the firmware polls BUSY in status register 1 after every
//! program, erase and register write and takes "ready" at once; every
//! operation is complete when its frame ends.
//!
//! # What lies where
//!
//! The firmware picks the layout by the FPGA type in the capability
//! register (2: XC7A50T, 3: XC7A100T), not by the chip
//! ([`FlashLayout`]):
//!
//! | | type 2 | type 3 |
//! |---|---|---|
//! | FPGA image | `$000000`, `$220000` long | `$000000`, `$3C0000` long |
//! | application | `$220000`, `$1E0000` long | `$3C0000`, `$1C0000` long |
//! | FAT volume `/flash` (4 KiB sectors, no partition table) | `$400000`, `$BE8000` long | `$580000`, `$A68000` long |
//!
//! The application record starts with three little-endian words: load
//! address, length in bytes, start address; the program follows. A
//! length of `$FFFFFFFF` means "no application". The boot loader copies
//! the program in words and then jumps to the start address ([`AppHeader`]).
//!
//! The configuration pages are the chip's last sectors, one 4 KiB sector
//! per page of which 512 bytes are used: 24 from `$FE8000` on the
//! S25FL128L, 16 from `$FF0000` on the W25Q128JV
//! ([`FlashChip::config_page_addr`]).
//!
//! # Host side
//!
//! The block never touches files. The machine gives it the image
//! ([`SpiFlash::load_image`]), asks which 4 KiB sectors changed
//! ([`SpiFlash::take_dirty`]) and writes those back from
//! [`SpiFlash::image`]. The chip's non-volatile registers are not part
//! of the image; [`SpiFlash::nv_registers`] and
//! [`SpiFlash::set_nv_registers`] let the machine keep them as well.
//!
//! Sources: UE2-C64U-Emulator `docs/hw/06-spi-flash-config.md`,
//! `docs/specs/S06-spi-flash.md`, `docs/specs/S47-c64u-flash-install.md`,
//! `docs/status/storage.md`; 1541 Ultimate firmware
//! `software/io/flash/{flash.h,w25q_flash.h,w25q_flash.cc,s25fl_flash.cc,
//! s25fl_l_flash.h,s25fl_l_flash.cc,prog_flash.cc}`,
//! `software/filesystem/blockdev_flash.cc`,
//! `software/application/update_u2p/{update_u64ii.cc,update_common.h}`,
//! `software/portable/riscv/bootloader_u64ii.c` (all read for facts);
//! command semantics after the public datasheets of the Winbond
//! W25Q128JV and the Infineon S25FL128L.

use crate::io::{IoCtx, IoDevice};
use mimic_core::{StateError, StateReader, StateWriter};

/// First I/O address of the controller.
pub const IO_BASE: u32 = 0x06_0200;
/// Last I/O address of the controller's four registers.
pub const IO_LAST: u32 = 0x06_020F;

/// CTRL bit 0: the select line is forced to [`CTRL_LEVEL`].
pub const CTRL_FORCE: u8 = 0x01;
/// CTRL bit 1: the level of the forced select line (1 = deselected).
pub const CTRL_LEVEL: u8 = 0x02;

/// Size of both chips: 16 MiB.
pub const FLASH_SIZE: usize = 0x100_0000;
/// The smallest erasable unit, also the unit of the dirty tracking.
pub const SECTOR_SIZE: usize = 0x1000;
/// Number of 4 KiB sectors.
pub const SECTOR_COUNT: usize = FLASH_SIZE / SECTOR_SIZE;
/// The unit of a page program.
pub const PAGE_SIZE: usize = 0x100;
/// Bytes of a configuration page that the firmware uses (two flash
/// pages at the start of the page's sector).
pub const CONFIG_PAGE_SIZE: usize = 0x200;

const ADDR_MASK: u32 = 0xFF_FFFF;

/// Status register 1 bits.
const SR1_WEL: u8 = 0x02;
/// Bits of status register 1 a register write can change.
const SR1_WRITABLE: u8 = 0xFC;
/// Second register (W25Q SR2, S25FL CR1): complement protect.
const R2_CMP: u8 = 0x40;
/// Second register: lock-down of the registers until power-off.
const R2_LOCK: u8 = 0x01;
/// S25FL128L status register 2: program error, erase error.
const SR2_P_ERR: u8 = 0x20;
const SR2_E_ERR: u8 = 0x40;

/// The chip on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashChip {
    /// Infineon S25FL128L (Ultimate 64 Elite II).
    S25fl128l,
    /// Winbond W25Q128JV (Commodore 64 Ultimate).
    W25q128jv,
}

impl FlashChip {
    /// Manufacturer, memory type, capacity as `9F` returns them.
    pub fn jedec_id(self) -> [u8; 3] {
        match self {
            FlashChip::S25fl128l => [0x01, 0x60, 0x18],
            FlashChip::W25q128jv => [0xEF, 0x40, 0x18],
        }
    }

    /// Number of configuration pages the firmware's driver for this chip
    /// keeps at the end of the flash.
    pub fn config_pages(self) -> u32 {
        match self {
            FlashChip::S25fl128l => 24,
            FlashChip::W25q128jv => 16,
        }
    }

    /// Flash address of configuration page `page`: the last
    /// [`FlashChip::config_pages`] sectors, `None` beyond them.
    pub fn config_page_addr(self, page: u32) -> Option<u32> {
        let n = self.config_pages();
        (page < n).then(|| (SECTOR_COUNT as u32 - n + page) * SECTOR_SIZE as u32)
    }

    /// Registers after the chip left the factory: status register 1 and
    /// the three that follow it in a register write.
    fn factory_registers(self) -> [u8; 4] {
        match self {
            // CR2: output impedance; CR3: read latency 8, wrap off.
            FlashChip::S25fl128l => [0x00, 0x00, 0x60, 0x78],
            // SR3: driver strength.
            FlashChip::W25q128jv => [0x00, 0x00, 0x60, 0x00],
        }
    }

    /// Bits a write can change, per register.
    fn writable(self) -> [u8; 4] {
        match self {
            FlashChip::S25fl128l => [SR1_WRITABLE, 0x7F, 0xEF, 0x7F],
            FlashChip::W25q128jv => [SR1_WRITABLE, 0x7B, 0x64, 0x00],
        }
    }

    /// One-time lock bits of the second register: once 1 they stay.
    fn sticky(self) -> u8 {
        match self {
            FlashChip::S25fl128l => 0x3C,
            FlashChip::W25q128jv => 0x38,
        }
    }

    fn tag(self) -> u8 {
        match self {
            FlashChip::S25fl128l => 0,
            FlashChip::W25q128jv => 1,
        }
    }
}

/// A region of the flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub len: u32,
}

impl Region {
    /// The first address behind the region.
    pub const fn end(self) -> u32 {
        self.start + self.len
    }
}

/// Where the firmware keeps what, by FPGA type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashLayout {
    /// The FPGA's configuration.
    pub fpga: Region,
    /// The application record ([`AppHeader`] and the program).
    pub application: Region,
    /// The FAT volume mounted as `/flash`: 4 KiB sectors, no partition
    /// table.
    pub flash_disk: Region,
}

/// FPGA type 2 (XC7A50T, Commodore 64 Ultimate).
pub const LAYOUT_50T: FlashLayout = FlashLayout {
    fpga: Region {
        start: 0x00_0000,
        len: 0x22_0000,
    },
    application: Region {
        start: 0x22_0000,
        len: 0x1E_0000,
    },
    flash_disk: Region {
        start: 0x40_0000,
        len: 0xBE_8000,
    },
};

/// FPGA type 3 (XC7A100T, Ultimate 64 Elite II).
pub const LAYOUT_100T: FlashLayout = FlashLayout {
    fpga: Region {
        start: 0x00_0000,
        len: 0x3C_0000,
    },
    application: Region {
        start: 0x3C_0000,
        len: 0x1C_0000,
    },
    flash_disk: Region {
        start: 0x58_0000,
        len: 0xA6_8000,
    },
};

/// The area the layout tables reserve for configuration pages; the
/// W25Q128JV's driver uses only its last 16 sectors.
pub const CONFIG_AREA: Region = Region {
    start: 0xFE_8000,
    len: 0x01_8000,
};

/// The boot loader does not copy an application of this length or more
/// (it still jumps to its start address).
pub const APP_MAX_LEN: u32 = 0x18_0000;

impl FlashLayout {
    /// The layout for the FPGA type of the capability register (bits
    /// 29:28): 3 and above the 100T layout, else the 50T layout.
    pub const fn for_fpga_type(fpga_type: u8) -> Self {
        if fpga_type >= 3 {
            LAYOUT_100T
        } else {
            LAYOUT_50T
        }
    }
}

/// The three words in front of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppHeader {
    /// Where the program is copied to.
    pub load_addr: u32,
    /// Its length in bytes; the boot loader copies whole words.
    pub len: u32,
    /// Where it is started.
    pub run_addr: u32,
}

impl AppHeader {
    /// Bytes of the header; the program follows it.
    pub const SIZE: u32 = 12;

    /// The header of the application in `image` for an FPGA type; `None`
    /// if there is none (length `$FFFFFFFF`) or the image is too short.
    pub fn read(image: &[u8], fpga_type: u8) -> Option<Self> {
        let at = FlashLayout::for_fpga_type(fpga_type).application.start as usize;
        let h = image.get(at..at + Self::SIZE as usize)?;
        let word = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
        let len = word(4);
        (len != u32::MAX).then(|| Self {
            load_addr: word(0),
            len,
            run_addr: word(8),
        })
    }
}

/// What an opcode is, for the chip at hand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    JedecId,
    UniqueId,
    /// A register that is repeated on every byte of the frame.
    Register,
    Read {
        addr_len: u32,
        dummy: u32,
    },
    Program {
        addr_len: u32,
    },
    Erase {
        addr_len: u32,
        size: u32,
    },
    /// S25FL128L: read any register.
    ReadAny,
    /// W25Q128JV: `90` and `AB`.
    DeviceId,
    /// Commands without a data phase of their own; their bytes are
    /// collected and looked at when the frame ends.
    Other,
}

/// The SPI flash controller with its chip.
#[derive(Clone, PartialEq, Eq)]
pub struct SpiFlash {
    chip: FlashChip,
    mem: Vec<u8>,
    /// One bit per 4 KiB sector that changed since [`SpiFlash::take_dirty`].
    dirty: Vec<u64>,
    unique_id: [u8; 8],

    // Controller.
    force: bool,
    level: bool,
    /// The byte shifted in by the last transfer.
    last_rx: u8,

    // Chip registers: status register 1 (without WEL and BUSY) and the
    // three that follow it in a register write.
    regs: [u8; 4],
    regs_nv: [u8; 4],
    /// Write enable latch.
    wel: bool,
    /// `50` was sent: the next register write changes the volatile copy.
    volatile_enable: bool,
    /// `66` was the previous frame.
    reset_armed: bool,
    /// Register writes are refused until power-off.
    locked: bool,
    /// S25FL128L: program and erase error bits of status register 2.
    errors: u8,

    // The open frame.
    selected: bool,
    op: u8,
    /// Bytes of the frame so far, the opcode included.
    count: u32,
    addr: u32,
    /// The bytes after the opcode of a command without data phase.
    args: [u8; 5],
    /// Page program: the data by offset in the page, and which offsets
    /// were given.
    page: [u8; PAGE_SIZE],
    page_given: [u64; 4],
}

impl std::fmt::Debug for SpiFlash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpiFlash")
            .field("chip", &self.chip)
            .field("force", &self.force)
            .field("level", &self.level)
            .field("regs", &self.regs)
            .field("regs_nv", &self.regs_nv)
            .field("wel", &self.wel)
            .field("selected", &self.selected)
            .field("op", &self.op)
            .field("count", &self.count)
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl SpiFlash {
    /// A board with an erased chip.
    pub fn new(chip: FlashChip) -> Self {
        let regs = chip.factory_registers();
        Self {
            chip,
            mem: vec![0xFF; FLASH_SIZE],
            dirty: vec![0; SECTOR_COUNT / 64],
            unique_id: [0x55, 0x15, 0x41, 0x64, 0x00, 0x00, 0x00, 0x01],
            force: false,
            level: true,
            last_rx: 0xFF,
            regs,
            regs_nv: regs,
            wel: false,
            volatile_enable: false,
            reset_armed: false,
            locked: false,
            errors: 0,
            selected: false,
            op: 0,
            count: 0,
            addr: 0,
            args: [0; 5],
            page: [0xFF; PAGE_SIZE],
            page_given: [0; 4],
        }
    }

    pub fn chip(&self) -> FlashChip {
        self.chip
    }

    /// The chip's 64-bit unique number (`4B`). The firmware derives the
    /// board's MAC address, host name and serial number from it.
    pub fn set_unique_id(&mut self, id: [u8; 8]) {
        self.unique_id = id;
    }

    pub fn unique_id(&self) -> [u8; 8] {
        self.unique_id
    }

    /// Replace the chip's contents: shorter images are filled up with
    /// `$FF`, longer ones cut. Nothing counts as changed afterwards.
    pub fn load_image(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(FLASH_SIZE);
        self.mem[..n].copy_from_slice(&bytes[..n]);
        self.mem[n..].fill(0xFF);
        self.dirty.fill(0);
    }

    /// The chip's contents, [`FLASH_SIZE`] bytes.
    pub fn image(&self) -> &[u8] {
        &self.mem
    }

    /// The 4 KiB sectors whose contents changed since the last call,
    /// ascending. Sector `n` is `image()[n * SECTOR_SIZE..][..SECTOR_SIZE]`.
    pub fn take_dirty(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, word) in self.dirty.iter_mut().enumerate() {
            let mut w = std::mem::take(word);
            while w != 0 {
                out.push(i as u32 * 64 + w.trailing_zeros());
                w &= w - 1;
            }
        }
        out
    }

    /// Whether [`SpiFlash::take_dirty`] would return anything.
    pub fn is_dirty(&self) -> bool {
        self.dirty.iter().any(|&w| w != 0)
    }

    /// The chip's non-volatile registers, for the machine to keep beside
    /// the image: status register 1 and, on the W25Q128JV, status
    /// registers 2 and 3, on the S25FL128L configuration registers 1, 2
    /// and 3.
    pub fn nv_registers(&self) -> [u8; 4] {
        self.regs_nv
    }

    /// Set the non-volatile registers as kept by the machine and take
    /// them into use as a power-on does. Bits the chip does not have are
    /// dropped.
    pub fn set_nv_registers(&mut self, regs: [u8; 4]) {
        let mask = self.chip.writable();
        for (i, v) in regs.into_iter().enumerate() {
            self.regs_nv[i] = v & mask[i];
        }
        self.power_cycle();
    }

    /// The chip loses and regains its supply: an open frame is dropped,
    /// the volatile registers reload, the lock-down ends. (The
    /// controller keeps its state; [`IoDevice::reset`] is the FPGA's
    /// side and leaves the chip's registers alone.)
    pub fn power_cycle(&mut self) {
        self.abort_frame();
        self.regs_nv[1] &= !R2_LOCK;
        self.regs = self.regs_nv;
        self.locked = false;
        self.wel = false;
        self.volatile_enable = false;
        self.reset_armed = false;
        self.errors = 0;
    }

    /// The range `lo..hi` that the protection bits guard at present,
    /// `None` if nothing is protected.
    pub fn protected_range(&self) -> Option<(u32, u32)> {
        const ALL: u32 = FLASH_SIZE as u32;
        let sr1 = self.regs[0];
        let bp = (sr1 >> 2) & 7;
        let sec = sr1 & 0x40 != 0;
        let bottom = sr1 & 0x20 != 0;
        let cmp = self.regs[1] & R2_CMP != 0;
        let size = match (bp, sec) {
            (0, _) => 0,
            (7, _) => ALL,
            (n, false) => 0x4_0000 << (n - 1),
            (n @ 1..=3, true) => 0x1000 << (n - 1),
            // 4 and 5 are 32 KiB; the datasheets leave 6 open, taken as
            // 32 KiB too.
            (_, true) => 0x8000,
        };
        let (lo, hi) = match (bottom, cmp) {
            (false, false) => (ALL - size, ALL),
            (true, false) => (0, size),
            (false, true) => (0, ALL - size),
            (true, true) => (size, ALL),
        };
        (lo < hi).then_some((lo, hi))
    }

    fn is_protected(&self, lo: u32, hi: u32) -> bool {
        self.protected_range()
            .is_some_and(|(plo, phi)| lo < phi && plo < hi)
    }

    fn mark_dirty(&mut self, sector: usize) {
        self.dirty[sector / 64] |= 1 << (sector % 64);
    }

    fn class(&self, op: u8) -> Class {
        let s25 = self.chip == FlashChip::S25fl128l;
        match (op, s25) {
            (0x9F, _) => Class::JedecId,
            (0x4B, _) => Class::UniqueId,
            (0x05 | 0x35 | 0x15, _) | (0x07 | 0x33, true) => Class::Register,
            (0x03, _) => Class::Read {
                addr_len: 3,
                dummy: 0,
            },
            (0x0B, _) => Class::Read {
                addr_len: 3,
                dummy: 1,
            },
            (0x13, true) => Class::Read {
                addr_len: 4,
                dummy: 0,
            },
            (0x0C, true) => Class::Read {
                addr_len: 4,
                dummy: 1,
            },
            (0x02, _) => Class::Program { addr_len: 3 },
            (0x12, true) => Class::Program { addr_len: 4 },
            (0x20, _) => Class::Erase {
                addr_len: 3,
                size: 0x1000,
            },
            (0x21, true) => Class::Erase {
                addr_len: 4,
                size: 0x1000,
            },
            (0x52, _) => Class::Erase {
                addr_len: 3,
                size: 0x8000,
            },
            (0x53, true) => Class::Erase {
                addr_len: 4,
                size: 0x8000,
            },
            (0xD8, _) => Class::Erase {
                addr_len: 3,
                size: 0x1_0000,
            },
            (0xDC, true) => Class::Erase {
                addr_len: 4,
                size: 0x1_0000,
            },
            (0x65, true) => Class::ReadAny,
            (0x90 | 0xAB, false) => Class::DeviceId,
            _ => Class::Other,
        }
    }

    /// Status register 1 as read: the protection bits, WEL, BUSY = 0.
    fn status1(&self) -> u8 {
        self.regs[0] | if self.wel { SR1_WEL } else { 0 }
    }

    /// The register a read command returns.
    fn register(&self, op: u8) -> u8 {
        match (op, self.chip) {
            (0x05, _) => self.status1(),
            (0x35, _) => self.regs[1],
            (0x15, _) => self.regs[2],
            (0x07, FlashChip::S25fl128l) => self.errors,
            (0x33, FlashChip::S25fl128l) => self.regs[3],
            _ => 0xFF,
        }
    }

    /// S25FL128L: the register at an address of "read any register":
    /// `$00000x` the non-volatile, `$80000x` the volatile ones; x = 0
    /// status 1, 1 status 2 (volatile only), 2–4 configuration 1–3.
    fn any_register(&self, addr: u32) -> u8 {
        match addr & ADDR_MASK {
            0x00_0000 => self.regs_nv[0],
            0x00_0002 => self.regs_nv[1],
            0x00_0003 => self.regs_nv[2],
            0x00_0004 => self.regs_nv[3],
            0x80_0000 => self.status1(),
            0x80_0001 => self.errors,
            0x80_0002 => self.regs[1],
            0x80_0003 => self.regs[2],
            0x80_0004 => self.regs[3],
            _ => 0,
        }
    }

    fn begin_frame(&mut self) {
        self.selected = true;
        self.count = 0;
    }

    fn abort_frame(&mut self) {
        self.selected = false;
        self.count = 0;
    }

    /// One byte to the selected chip; what it answers.
    fn transfer(&mut self, v: u8) -> u8 {
        let i = self.count;
        self.count = self.count.saturating_add(1);
        if i == 0 {
            self.op = v;
            self.addr = 0;
            self.args = [0; 5];
            self.page_given = [0; 4];
            return 0xFF;
        }
        match self.class(self.op) {
            Class::JedecId => self.chip.jedec_id()[(i as usize - 1) % 3],
            Class::UniqueId => {
                if i <= 4 {
                    0xFF
                } else {
                    self.unique_id[(i as usize - 5) % 8]
                }
            }
            Class::Register => self.register(self.op),
            Class::Read { addr_len, dummy } => {
                if i <= addr_len {
                    self.addr = (self.addr << 8) | u32::from(v);
                    0xFF
                } else if i <= addr_len + dummy {
                    0xFF
                } else {
                    let b = self.mem[(self.addr & ADDR_MASK) as usize];
                    self.addr = self.addr.wrapping_add(1);
                    b
                }
            }
            Class::Program { addr_len } => {
                if i <= addr_len {
                    self.addr = (self.addr << 8) | u32::from(v);
                } else {
                    let off = (self.addr.wrapping_add(i - addr_len - 1) & 0xFF) as usize;
                    self.page[off] = v;
                    self.page_given[off / 64] |= 1 << (off % 64);
                    // Hold the count where it cannot run out, however
                    // long the frame gets.
                    if self.count > addr_len + 1 + 2 * PAGE_SIZE as u32 {
                        self.count -= PAGE_SIZE as u32;
                    }
                }
                0xFF
            }
            Class::Erase { addr_len, .. } => {
                if i <= addr_len {
                    self.addr = (self.addr << 8) | u32::from(v);
                }
                0xFF
            }
            Class::ReadAny => {
                if i <= 3 {
                    self.addr = (self.addr << 8) | u32::from(v);
                    0xFF
                } else if i == 4 {
                    0xFF
                } else {
                    self.any_register(self.addr)
                }
            }
            Class::DeviceId => {
                // Three address or dummy bytes, then for `90`
                // manufacturer and device alternating, for `AB` the
                // device.
                let id = self.chip.jedec_id();
                if i <= 3 {
                    0xFF
                } else if self.op == 0x90 && i.is_multiple_of(2) {
                    id[0]
                } else {
                    id[2] - 1
                }
            }
            Class::Other => {
                if let Some(a) = self.args.get_mut(i as usize - 1) {
                    *a = v;
                }
                0xFF
            }
        }
    }

    /// The select line rose: what the frame asked for happens now.
    fn end_frame(&mut self) {
        if !self.selected {
            return;
        }
        self.selected = false;
        let n = std::mem::take(&mut self.count);
        if n == 0 {
            return;
        }
        let armed = std::mem::take(&mut self.reset_armed);
        let op = self.op;
        match self.class(op) {
            Class::Program { addr_len } => {
                if n > addr_len + 1 {
                    self.program();
                }
            }
            Class::Erase { addr_len, size } => {
                if n == addr_len + 1 {
                    let lo = self.addr & ADDR_MASK & !(size - 1);
                    self.erase(lo, size);
                }
            }
            Class::Other => self.execute(op, n, armed),
            _ => {}
        }
    }

    /// Commands without data phase, at the end of a frame of `n` bytes.
    fn execute(&mut self, op: u8, n: u32, armed: bool) {
        let s25 = self.chip == FlashChip::S25fl128l;
        match (op, n) {
            (0x06, 1) => {
                self.wel = true;
                self.volatile_enable = false;
            }
            (0x04, 1) => {
                self.wel = false;
                self.volatile_enable = false;
            }
            (0x50, 1) => self.volatile_enable = true,
            (0x66, 1) => self.reset_armed = true,
            (0x99, 1) if armed => {
                self.regs = self.regs_nv;
                if self.locked {
                    self.regs[1] |= R2_LOCK;
                }
                self.wel = false;
                self.volatile_enable = false;
                self.errors = 0;
            }
            (0x60 | 0xC7, 1) => {
                if self.wel && self.protected_range().is_none() {
                    self.erase_range(0, FLASH_SIZE as u32);
                    self.wel = false;
                }
            }
            (0x30, 1) if s25 => self.errors = 0,
            // S25FL128L: status 1, configuration 1, 2, 3 as far as
            // bytes follow. W25Q128JV: status 1, and status 2 with it.
            (0x01, 2..=5) if s25 => {
                let args = self.args;
                self.write_registers(0, &args[..n as usize - 1]);
            }
            (0x01, 2..=3) => {
                let args = self.args;
                self.write_registers(0, &args[..n as usize - 1]);
            }
            (0x31, 2) if !s25 => {
                let v = self.args[0];
                self.write_registers(1, &[v]);
            }
            (0x11, 2) if !s25 => {
                let v = self.args[0];
                self.write_registers(2, &[v]);
            }
            // S25FL128L "write any register": three address bytes, one
            // value.
            (0x71, 5) if s25 => {
                let a = u32::from_be_bytes([0, self.args[0], self.args[1], self.args[2]]);
                let v = self.args[3];
                let index = match a & 0x7F_FFFF {
                    0 => Some(0),
                    2 => Some(1),
                    3 => Some(2),
                    4 => Some(3),
                    _ => None,
                };
                if let Some(index) = index {
                    if self.wel && !self.locked {
                        self.set_register(index, v, a & 0x80_0000 == 0);
                    }
                    self.wel = false;
                }
            }
            _ => {}
        }
    }

    /// A register write of `values` starting at register `first`: after
    /// `06` into both copies, after `50` into the volatile one only.
    fn write_registers(&mut self, first: usize, values: &[u8]) {
        let nv = if self.wel {
            true
        } else if self.volatile_enable {
            false
        } else {
            return;
        };
        self.volatile_enable = false;
        if nv {
            self.wel = false;
        }
        if self.locked {
            return;
        }
        for (i, &v) in values.iter().enumerate() {
            self.set_register(first + i, v, nv);
        }
    }

    fn set_register(&mut self, index: usize, v: u8, nv: bool) {
        let mask = self.chip.writable()[index];
        let sticky = if index == 1 { self.chip.sticky() } else { 0 };
        self.regs[index] = (v & mask) | (self.regs[index] & sticky);
        if nv {
            self.regs_nv[index] = (v & mask) | (self.regs_nv[index] & sticky);
        }
        if index == 1 && self.regs[1] & R2_LOCK != 0 {
            self.locked = true;
        }
    }

    /// The page program collected in the frame.
    fn program(&mut self) {
        if !self.wel {
            return;
        }
        let base = self.addr & ADDR_MASK & !0xFF;
        if self.is_protected(base, base + PAGE_SIZE as u32) {
            self.errors |= SR2_P_ERR;
            return;
        }
        let base = base as usize;
        let mut changed = false;
        for off in 0..PAGE_SIZE {
            if self.page_given[off / 64] & (1 << (off % 64)) != 0 {
                let cell = &mut self.mem[base + off];
                let new = *cell & self.page[off];
                changed |= new != *cell;
                *cell = new;
            }
        }
        if changed {
            self.mark_dirty(base / SECTOR_SIZE);
        }
        self.wel = false;
    }

    fn erase(&mut self, lo: u32, size: u32) {
        if !self.wel {
            return;
        }
        if self.is_protected(lo, lo + size) {
            self.errors |= SR2_E_ERR;
            return;
        }
        self.erase_range(lo, size);
        self.wel = false;
    }

    fn erase_range(&mut self, lo: u32, size: u32) {
        let first = lo as usize / SECTOR_SIZE;
        let last = (lo + size) as usize / SECTOR_SIZE;
        for sector in first..last.min(SECTOR_COUNT) {
            let s = &mut self.mem[sector * SECTOR_SIZE..][..SECTOR_SIZE];
            if s.iter().any(|&b| b != 0xFF) {
                s.fill(0xFF);
                self.mark_dirty(sector);
            }
        }
    }

    /// A byte through the controller, as the select line stands.
    fn shift(&mut self, v: u8) -> u8 {
        let rx = if !self.force {
            self.begin_frame();
            let rx = self.transfer(v);
            self.end_frame();
            rx
        } else if self.level {
            0xFF
        } else {
            self.transfer(v)
        };
        self.last_rx = rx;
        rx
    }

    fn ctrl(&self) -> u8 {
        0x04 | if self.level { CTRL_LEVEL } else { 0 } | u8::from(self.force)
    }

    fn set_ctrl(&mut self, v: u8) {
        let was_low = self.force && !self.level;
        self.force = v & CTRL_FORCE != 0;
        self.level = v & CTRL_LEVEL != 0;
        let low = self.force && !self.level;
        if was_low && !low {
            self.end_frame();
        } else if low && !was_low {
            self.begin_frame();
        }
    }
}

impl IoDevice for SpiFlash {
    fn read(&mut self, addr: u32, _ctx: &mut IoCtx<'_>) -> u8 {
        match (addr >> 2) & 3 {
            0 => self.shift(0xFF),
            _ => self.peek(addr),
        }
    }

    /// DATA shows the byte the last transfer shifted in; a real read
    /// would start a new transfer.
    fn peek(&self, addr: u32) -> u8 {
        match (addr >> 2) & 3 {
            0 => self.last_rx,
            1 => 1,
            2 => self.ctrl(),
            _ => 0,
        }
    }

    fn write(&mut self, addr: u32, v: u8, _ctx: &mut IoCtx<'_>) {
        match (addr >> 2) & 3 {
            0 => {
                self.shift(v);
            }
            2 => self.set_ctrl(v),
            _ => {}
        }
    }

    /// The FPGA's side: the controller returns to automatic chip select
    /// and a frame that was open is dropped without effect. The chip
    /// keeps its contents and registers ([`SpiFlash::power_cycle`]).
    fn reset(&mut self) {
        self.abort_frame();
        self.force = false;
        self.level = true;
        self.last_rx = 0xFF;
    }

    /// The image is stored by sector: a bit per sector (set: the sector
    /// follows), then the sectors that are not all `$FF`, ascending.
    fn save(&self, w: &mut StateWriter) {
        w.u8(self.chip.tag());
        w.raw(&self.unique_id);
        w.bool(self.force);
        w.bool(self.level);
        w.u8(self.last_rx);
        w.raw(&self.regs);
        w.raw(&self.regs_nv);
        w.bool(self.wel);
        w.bool(self.volatile_enable);
        w.bool(self.reset_armed);
        w.bool(self.locked);
        w.u8(self.errors);
        w.bool(self.selected);
        w.u8(self.op);
        w.u32(self.count);
        w.u32(self.addr);
        w.raw(&self.args);
        w.raw(&self.page);
        for v in self.page_given {
            w.u64(v);
        }
        let mut map = [0u8; SECTOR_COUNT / 8];
        for (n, s) in self.mem.as_chunks::<SECTOR_SIZE>().0.iter().enumerate() {
            if s.iter().any(|&b| b != 0xFF) {
                map[n / 8] |= 1 << (n % 8);
            }
        }
        w.raw(&map);
        for (n, s) in self.mem.as_chunks::<SECTOR_SIZE>().0.iter().enumerate() {
            if map[n / 8] & (1 << (n % 8)) != 0 {
                w.raw(s);
            }
        }
    }

    /// Nothing changes unless the whole state is good. Sectors that
    /// differ from the present image count as changed afterwards, so the
    /// machine's copy on the host follows.
    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        if r.u8()? != self.chip.tag() {
            return Err(StateError::Invalid("flash chip"));
        }
        let mut unique_id = [0; 8];
        r.raw_into(&mut unique_id)?;
        let force = r.bool()?;
        let level = r.bool()?;
        let last_rx = r.u8()?;
        let mut regs = [0; 4];
        r.raw_into(&mut regs)?;
        let mut regs_nv = [0; 4];
        r.raw_into(&mut regs_nv)?;
        let mask = self.chip.writable();
        if (0..4).any(|i| regs[i] & !mask[i] != 0 || regs_nv[i] & !mask[i] != 0) {
            return Err(StateError::Invalid("flash registers"));
        }
        let wel = r.bool()?;
        let volatile_enable = r.bool()?;
        let reset_armed = r.bool()?;
        let locked = r.bool()?;
        let errors = r.u8()?;
        if errors & !(SR2_P_ERR | SR2_E_ERR) != 0 {
            return Err(StateError::Invalid("flash error bits"));
        }
        let selected = r.bool()?;
        let op = r.u8()?;
        let count = r.u32()?;
        let addr = r.u32()?;
        // An open frame exists only with the select line forced low.
        if selected != (force && !level) || (!selected && count != 0) {
            return Err(StateError::Invalid("flash frame"));
        }
        let mut args = [0; 5];
        r.raw_into(&mut args)?;
        let mut page = [0; PAGE_SIZE];
        r.raw_into(&mut page)?;
        let mut page_given = [0; 4];
        for v in &mut page_given {
            *v = r.u64()?;
        }
        let map = r.raw(SECTOR_COUNT / 8)?;
        let stored = |n: usize| map[n / 8] & (1 << (n % 8)) != 0;
        let sectors = (0..SECTOR_COUNT).filter(|&n| stored(n)).count();
        let data = r.raw(sectors * SECTOR_SIZE)?;

        let mut next = data.as_chunks::<SECTOR_SIZE>().0.iter();
        for n in 0..SECTOR_COUNT {
            let old = &mut self.mem[n * SECTOR_SIZE..][..SECTOR_SIZE];
            let changed = if stored(n) {
                let Some(new) = next.next() else {
                    return Err(StateError::Truncated);
                };
                let c = old != new;
                if c {
                    old.copy_from_slice(new);
                }
                c
            } else {
                let c = old.iter().any(|&b| b != 0xFF);
                if c {
                    old.fill(0xFF);
                }
                c
            };
            if changed {
                self.mark_dirty(n);
            }
        }
        self.unique_id = unique_id;
        self.force = force;
        self.level = level;
        self.last_rx = last_rx;
        self.regs = regs;
        self.regs_nv = regs_nv;
        self.wel = wel;
        self.volatile_enable = volatile_enable;
        self.reset_armed = reset_armed;
        self.locked = locked;
        self.errors = errors;
        self.selected = selected;
        self.op = op;
        self.count = count;
        self.addr = addr;
        self.args = args;
        self.page = page;
        self.page_given = page_given;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::NullC64;
    use crate::irq::Irq;

    const DATA: u32 = IO_BASE;
    const CTRL: u32 = IO_BASE + 8;

    /// The flash with what an access needs, and the accesses the
    /// firmware's drivers are made of.
    struct Bench {
        f: SpiFlash,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
    }

    impl Bench {
        fn new(chip: FlashChip) -> Self {
            Self {
                f: SpiFlash::new(chip),
                ram: vec![0; 16],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
            }
        }

        fn wr(&mut self, addr: u32, v: u8) {
            let mut ctx = IoCtx {
                now: 0,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.f.write(addr, v, &mut ctx);
        }

        fn rd(&mut self, addr: u32) -> u8 {
            let mut ctx = IoCtx {
                now: 0,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage: None,
                net: None,
            };
            self.f.read(addr, &mut ctx)
        }

        fn ctrl(&mut self, v: u8) {
            self.wr(CTRL, v);
        }

        fn out(&mut self, v: u8) {
            self.wr(DATA, v);
        }

        fn inp(&mut self) -> u8 {
            self.rd(DATA)
        }

        /// A 32-bit write of DATA as the bus bridge splits it.
        fn out32(&mut self, v: u32) {
            for (i, b) in v.to_le_bytes().into_iter().enumerate() {
                self.wr(DATA + i as u32, b);
            }
        }

        fn in32(&mut self) -> u32 {
            let mut b = [0; 4];
            for (i, x) in b.iter_mut().enumerate() {
                *x = self.rd(DATA + i as u32);
            }
            u32::from_le_bytes(b)
        }

        /// A forced frame: the bytes out, then `n` bytes in.
        fn frame(&mut self, tx: &[u8], n: usize) -> Vec<u8> {
            self.ctrl(1);
            for &b in tx {
                self.out(b);
            }
            let rx = (0..n).map(|_| self.inp()).collect();
            self.ctrl(3);
            rx
        }

        /// A byte as a frame of its own (automatic select), as the
        /// drivers send write enable and disable.
        fn single(&mut self, op: u8) {
            self.ctrl(0);
            self.out(op);
        }

        fn addr_bytes(&self, op4: bool, addr: u32) -> Vec<u8> {
            let b = addr.to_be_bytes();
            if op4 {
                b.to_vec()
            } else {
                b[1..].to_vec()
            }
        }

        /// The drivers' wait: `05`, then poll bit 0.
        fn wait_ready(&mut self) -> bool {
            self.ctrl(1);
            self.out(0x05);
            let mut ok = false;
            for _ in 0..4 {
                if self.inp() & 1 == 0 {
                    ok = true;
                    break;
                }
            }
            self.ctrl(3);
            ok
        }

        /// The drivers' page write: enable, program in words, wait,
        /// disable.
        fn write_page(&mut self, op: u8, addr: u32, data: &[u8]) -> bool {
            self.single(0x06);
            self.ctrl(1);
            self.out(op);
            for b in self.addr_bytes(op == 0x12, addr) {
                self.out(b);
            }
            for w in data.chunks(4) {
                if w.len() == 4 {
                    self.out32(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
                } else {
                    for &b in w {
                        self.out(b);
                    }
                }
            }
            self.ctrl(3);
            let ok = self.wait_ready();
            self.single(0x04);
            self.ctrl(3);
            ok
        }

        fn erase(&mut self, op: u8, addr: u32) -> bool {
            self.single(0x06);
            let op4 = matches!(op, 0x21 | 0x53 | 0xDC);
            let mut tx = vec![op];
            tx.extend(self.addr_bytes(op4, addr));
            self.frame(&tx, 0);
            let ok = self.wait_ready();
            self.single(0x04);
            self.ctrl(3);
            ok
        }

        fn read(&mut self, op: u8, addr: u32, n: usize) -> Vec<u8> {
            let mut tx = vec![op];
            tx.extend(self.addr_bytes(matches!(op, 0x13 | 0x0C), addr));
            if matches!(op, 0x0B | 0x0C) {
                tx.push(0);
            }
            self.frame(&tx, n)
        }

        /// The probe of the W25Q and the S25FL-K drivers.
        fn probe_plain(&mut self) -> [u8; 3] {
            self.ctrl(3);
            self.out(0xFF);
            self.ctrl(1);
            self.out(0x9F);
            let id = [self.inp(), self.inp(), self.inp()];
            self.ctrl(3);
            id
        }

        /// The probe of the S25FL-L driver, with its resets.
        fn probe_l(&mut self) -> [u8; 3] {
            self.frame(&[0x66, 0x99], 0);
            self.frame(&[0x66], 0);
            self.frame(&[0x99], 0);
            self.frame(&[0xFF], 0);
            let id = self.frame(&[0x9F], 3);
            [id[0], id[1], id[2]]
        }

        fn status(&mut self, op: u8) -> u8 {
            self.frame(&[op], 1)[0]
        }
    }

    fn pattern(seed: u8) -> Vec<u8> {
        (0..PAGE_SIZE)
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn probes_find_the_right_driver() {
        // Order of the testers: W25Q, S25FL-K, S25FL-L.
        let mut b = Bench::new(FlashChip::S25fl128l);
        let id = b.probe_plain();
        assert_ne!(id[0], 0xEF, "the W25Q tester must reject it");
        assert_ne!(id[1], 0x40, "the S25FL-K tester must reject it");
        assert_eq!(b.probe_l(), [0x01, 0x60, 0x18]);

        let mut b = Bench::new(FlashChip::W25q128jv);
        assert_eq!(b.probe_plain(), [0xEF, 0x40, 0x18]);
        // The other probes would not take it.
        assert_ne!(b.probe_l()[0], 0x01);
    }

    #[test]
    fn deselected_bytes_do_not_reach_the_chip() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        // CTRL = 3: a write enable sent now is lost.
        b.ctrl(3);
        b.out(0x06);
        assert_eq!(b.inp(), 0xFF);
        assert_eq!(b.status(0x05) & SR1_WEL, 0);
        // CTRL = 0: every byte is a frame.
        b.single(0x06);
        b.ctrl(3);
        assert_eq!(b.status(0x05), SR1_WEL);
        // Inside a longer frame it is no write enable.
        b.single(0x04);
        b.frame(&[0x06, 0x00], 0);
        assert_eq!(b.status(0x05), 0);
    }

    #[test]
    fn controller_registers() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        // Reset: automatic select, level high; card-detect bit set.
        assert_eq!(b.rd(CTRL), 0x06);
        b.ctrl(1);
        assert_eq!(b.rd(CTRL), 0x05);
        assert_eq!(b.f.peek(CTRL), 0x05);
        b.ctrl(3);
        assert_eq!(b.rd(CTRL), 0x07);
        // RATE reads 1 and ignores writes; CRC reads 0.
        b.wr(IO_BASE + 4, 0x55);
        assert_eq!(b.rd(IO_BASE + 4), 1);
        b.wr(IO_BASE + 12, 0x55);
        assert_eq!(b.rd(IO_BASE + 12), 0);
        // peek of DATA shifts nothing.
        b.ctrl(1);
        b.out(0x9F);
        assert_eq!(b.inp(), 0x01);
        assert_eq!(b.f.peek(DATA), 0x01);
        assert_eq!(b.f.peek(DATA), 0x01);
        assert_eq!(b.inp(), 0x60);
        b.ctrl(3);
        // The FPGA's reset drops an open frame and keeps the chip.
        b.single(0x06);
        b.ctrl(1);
        b.out(0x02);
        b.f.reset();
        assert_eq!(b.rd(CTRL), 0x06);
        assert_eq!(b.status(0x05), SR1_WEL);
    }

    #[test]
    fn s25_page_cycle_with_four_byte_addresses() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        let addr = LAYOUT_100T.flash_disk.start + 0x300;
        let data = pattern(3);
        assert!(b.write_page(0x12, addr, &data));
        // The page read of the driver: `13`, 64 words.
        b.ctrl(1);
        b.out(0x13);
        for x in addr.to_be_bytes() {
            b.out(x);
        }
        let mut back = Vec::new();
        for _ in 0..64 {
            back.extend(b.in32().to_le_bytes());
        }
        b.ctrl(3);
        assert_eq!(back, data);
        // The byte read of the driver: `03` with three address bytes.
        assert_eq!(b.read(0x03, addr + 5, 4), &data[5..9]);
        assert_eq!(b.read(0x0B, addr + 5, 4), &data[5..9]);
        assert_eq!(b.read(0x0C, addr + 5, 4), &data[5..9]);
        // The latch is cleared by the program.
        assert_eq!(b.status(0x05), 0);
        // Bits only go to 0.
        assert!(b.write_page(0x12, addr, &[0x0F; 4]));
        let want: Vec<u8> = data[..4].iter().map(|v| v & 0x0F).collect();
        assert_eq!(b.read(0x13, addr, 4), want);
        // Sector erase by any address inside it.
        assert!(b.erase(0x21, addr + 0x123));
        assert!(b
            .read(0x13, addr & !0xFFF, 0x1000)
            .iter()
            .all(|&v| v == 0xFF));
    }

    #[test]
    fn w25q_page_cycle_and_no_four_byte_commands() {
        let mut b = Bench::new(FlashChip::W25q128jv);
        let addr = LAYOUT_50T.application.start;
        let data = pattern(9);
        assert!(b.write_page(0x02, addr, &data));
        assert_eq!(b.read(0x03, addr, PAGE_SIZE), data);
        assert_eq!(b.read(0x0B, addr + 1, 3), &data[1..4]);
        // The S25FL128L's 4-byte forms are no commands here.
        assert!(b.read(0x13, addr, 4).iter().all(|&v| v == 0xFF));
        b.write_page(0x12, addr + 0x100, &data);
        assert!(b.read(0x03, addr + 0x100, 16).iter().all(|&v| v == 0xFF));
        b.erase(0x21, addr);
        assert_eq!(b.read(0x03, addr, PAGE_SIZE), data);
        assert!(b.erase(0x20, addr));
        assert!(b.read(0x03, addr, PAGE_SIZE).iter().all(|&v| v == 0xFF));
        // Device ID.
        assert_eq!(b.frame(&[0x90, 0, 0, 0], 2), [0xEF, 0x17]);
        assert_eq!(b.frame(&[0xAB, 0, 0, 0], 1), [0x17]);
    }

    #[test]
    fn program_needs_the_latch_and_wraps_in_the_page() {
        for (chip, op) in [(FlashChip::S25fl128l, 0x12), (FlashChip::W25q128jv, 0x02)] {
            let mut b = Bench::new(chip);
            // Without write enable nothing is written.
            let mut tx = vec![op];
            tx.extend(b.addr_bytes(op == 0x12, 0x1000));
            tx.extend([0u8; 8]);
            b.frame(&tx, 0);
            assert!(b.f.image()[0x1000..0x1008].iter().all(|&v| v == 0xFF));
            assert!(b.f.take_dirty().is_empty());
            // Start at offset $F8 of a page: the 16 bytes wrap to the
            // start of the same page.
            let data: Vec<u8> = (0x10..0x20).collect();
            assert!(b.write_page(op, 0x20F8, &data));
            assert_eq!(&b.f.image()[0x20F8..0x2100], &data[..8]);
            assert_eq!(&b.f.image()[0x2000..0x2008], &data[8..]);
            assert!(b.f.image()[0x2100..0x2110].iter().all(|&v| v == 0xFF));
            assert_eq!(b.f.take_dirty(), [2]);
            // More than a page: the last 256 bytes count.
            let mut long = vec![0x00; 16];
            long.extend(pattern(1));
            assert!(b.write_page(op, 0x5000, &long));
            let want = pattern(1);
            assert_eq!(&b.f.image()[0x5010..0x5100], &want[..0xF0]);
            assert_eq!(&b.f.image()[0x5000..0x5010], &want[0xF0..]);
            // A frame that ends after the address programs nothing and
            // keeps the latch.
            b.single(0x06);
            let mut tx = vec![op];
            tx.extend(b.addr_bytes(op == 0x12, 0x7000));
            b.frame(&tx, 0);
            assert_eq!(b.status(0x05), SR1_WEL);
        }
    }

    fn erased(b: &Bench, a: usize) -> bool {
        b.f.image()[a] == 0xFF
    }

    #[test]
    fn erase_sizes() {
        for chip in [FlashChip::S25fl128l, FlashChip::W25q128jv] {
            let s25 = chip == FlashChip::S25fl128l;
            let mut b = Bench::new(chip);
            b.f.load_image(&vec![0x5A; 0x4_0000]);
            // 4 KiB at $1000, 32 KiB at $8000, 64 KiB at $20000.
            assert!(b.erase(0x20, 0x1FFF));
            assert!(b.erase(if s25 { 0x53 } else { 0x52 }, 0x8001));
            assert!(b.erase(if s25 { 0xDC } else { 0xD8 }, 0x2_FFFF));
            assert!(
                !erased(&b, 0x0FFF)
                    && erased(&b, 0x1000)
                    && erased(&b, 0x1FFF)
                    && !erased(&b, 0x2000)
            );
            assert!(
                !erased(&b, 0x7FFF)
                    && erased(&b, 0x8000)
                    && erased(&b, 0xFFFF)
                    && !erased(&b, 0x1_0000)
            );
            assert!(!erased(&b, 0x1_FFFF) && erased(&b, 0x2_0000) && erased(&b, 0x2_FFFF));
            assert!(!erased(&b, 0x3_0000));
            let mut want = vec![1];
            want.extend(8..16);
            want.extend(32..48);
            assert_eq!(b.f.take_dirty(), want);
            // An erase without the latch does nothing.
            b.frame(&[0x20, 0, 0, 0], 0);
            assert!(!erased(&b, 0));
            // A frame of the wrong length does nothing either.
            b.single(0x06);
            b.frame(&[0x20, 0, 0, 0, 0], 0);
            assert!(!erased(&b, 0));
            // Chip erase, both opcodes.
            b.single(0x04);
            b.single(0x06);
            b.single(0xC7);
            assert!(b.f.image().iter().all(|&v| v == 0xFF));
            assert_eq!(b.status(0x05), 0);
            b.f.load_image(&[0; 16]);
            b.single(0x06);
            b.single(0x60);
            assert!(erased(&b, 0));
        }
    }

    #[test]
    fn status_polling_reads_ready_on_every_byte() {
        for chip in [FlashChip::S25fl128l, FlashChip::W25q128jv] {
            let mut b = Bench::new(chip);
            b.single(0x06);
            b.ctrl(1);
            b.out(0x05);
            for _ in 0..5 {
                assert_eq!(b.inp(), SR1_WEL);
            }
            b.ctrl(3);
            b.single(0x04);
            assert!(b.wait_ready());
        }
    }

    #[test]
    fn unique_id_after_four_dummy_bytes() {
        for chip in [FlashChip::S25fl128l, FlashChip::W25q128jv] {
            let mut b = Bench::new(chip);
            let id = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
            b.f.set_unique_id(id);
            assert_eq!(b.f.unique_id(), id);
            // The driver: `4B`, a word of zeros, eight reads.
            b.ctrl(1);
            b.out(0x4B);
            b.out32(0);
            let got: Vec<u8> = (0..8).map(|_| b.inp()).collect();
            b.ctrl(3);
            assert_eq!(got, id);
        }
    }

    #[test]
    fn protection_ranges() {
        let mut f = SpiFlash::new(FlashChip::W25q128jv);
        let mut range = |sr1: u8, cmp: bool| {
            f.regs[0] = sr1;
            f.regs[1] = if cmp { R2_CMP } else { 0 };
            f.protected_range()
        };
        assert_eq!(range(0x00, false), None);
        assert_eq!(range(0x04, false), Some((0xFC_0000, 0x100_0000)));
        assert_eq!(range(0x18, false), Some((0x80_0000, 0x100_0000)));
        assert_eq!(range(0x24, false), Some((0, 0x4_0000)));
        // What the updater asks for: the lowest 4 MiB.
        assert_eq!(range(0x34, false), Some((0, 0x40_0000)));
        assert_eq!(range(0x1C, false), Some((0, 0x100_0000)));
        assert_eq!(range(0x44, false), Some((0xFF_F000, 0x100_0000)));
        assert_eq!(range(0x6C, false), Some((0, 0x4000)));
        assert_eq!(range(0x50, false), Some((0xFF_8000, 0x100_0000)));
        // Complement.
        assert_eq!(range(0x00, true), Some((0, 0x100_0000)));
        assert_eq!(range(0x1C, true), None);
        assert_eq!(range(0x34, true), Some((0x40_0000, 0x100_0000)));
        assert_eq!(range(0x04, true), Some((0, 0xFC_0000)));
    }

    /// The updater on the S25FL128L: unlock, write, lock the lowest
    /// 4 MiB; afterwards the FPGA image refuses writes, the FAT area and
    /// the configuration pages take them.
    #[test]
    fn s25_updater_unlock_write_lock() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        // Unlock: read any register 0, write registers 00 00, wait,
        // disable, reset as two frames.
        assert_eq!(b.frame(&[0x65, 0, 0, 0, 0], 1), [0x00]);
        b.single(0x06);
        b.frame(&[0x01, 0x00, 0x00], 0);
        assert!(b.wait_ready());
        b.single(0x04);
        b.single(0x66);
        b.out(0x99);
        b.ctrl(3);
        // Write and verify a page of the FPGA image.
        let data = pattern(5);
        assert!(b.erase(0x21, 0));
        assert!(b.write_page(0x12, 0, &data));
        assert_eq!(b.read(0x13, 0, PAGE_SIZE), data);
        // Lock: the present value, then write registers 34 00.
        assert_eq!(b.frame(&[0x65, 0, 0, 0, 0], 1), [0x00]);
        b.single(0x06);
        b.frame(&[0x01, 0x34, 0x00], 0);
        assert!(b.wait_ready());
        b.single(0x04);
        b.ctrl(3);
        assert_eq!(b.status(0x05), 0x34);
        assert_eq!(b.f.nv_registers()[0], 0x34);
        assert_eq!(b.f.protected_range(), Some((0, 0x40_0000)));
        b.f.take_dirty();

        // Protected: neither erase nor program change anything; the
        // driver's wait still ends, and the error bits tell.
        assert!(b.erase(0x21, 0));
        assert_eq!(b.status(0x07), SR2_E_ERR);
        assert!(b.write_page(0x12, 0x3F_FF00, &[0; 4]));
        assert_eq!(b.status(0x07), SR2_E_ERR | SR2_P_ERR);
        b.single(0x30);
        assert_eq!(b.status(0x07), 0);
        assert_eq!(b.read(0x13, 0, PAGE_SIZE), data);
        assert_eq!(b.f.image()[0x3F_FF00], 0xFF);
        // A 64 KiB erase that would reach into the range is refused.
        b.erase(0xDC, 0x3F_0000);
        assert!(b.f.take_dirty().is_empty());
        // Chip erase is refused while anything is protected.
        b.single(0x06);
        b.single(0xC7);
        b.single(0x04);
        assert_eq!(b.read(0x13, 0, PAGE_SIZE), data);

        // Above the range everything works: the FAT area of type 2 and
        // a configuration page.
        assert!(b.write_page(0x12, 0x40_0000, &data));
        assert_eq!(b.read(0x13, 0x40_0000, PAGE_SIZE), data);
        let cfg = FlashChip::S25fl128l.config_page_addr(0).unwrap();
        assert!(b.erase(0x21, cfg));
        assert!(b.write_page(0x12, cfg, &data));
        assert_eq!(b.read(0x03, cfg, 4), &data[..4]);

        // The protection is non-volatile: it survives reset and power.
        b.single(0x66);
        b.out(0x99);
        b.ctrl(3);
        assert_eq!(b.status(0x05), 0x34);
        b.f.power_cycle();
        assert_eq!(b.status(0x05), 0x34);
        // The next update unlocks again.
        b.single(0x06);
        b.frame(&[0x01, 0x00, 0x00], 0);
        assert!(b.erase(0x21, 0));
        assert_eq!(b.f.image()[0], 0xFF);
    }

    #[test]
    fn w25q_status_registers() {
        let mut b = Bench::new(FlashChip::W25q128jv);
        // The driver's unlock: `01 00`, then `31 00`, each after `06`.
        b.single(0x06);
        b.frame(&[0x01, 0x00], 0);
        assert!(b.wait_ready());
        b.single(0x06);
        b.frame(&[0x31, 0x00], 0);
        assert!(b.wait_ready());
        b.single(0x04);
        b.ctrl(3);
        assert_eq!(
            [b.status(0x05), b.status(0x35), b.status(0x15)],
            [0, 0, 0x60]
        );

        // Without an enable a write changes nothing.
        b.frame(&[0x01, 0x34], 0);
        assert_eq!(b.status(0x05), 0);
        // After `06`: both copies, and the latch is used up.
        b.single(0x06);
        b.frame(&[0x01, 0x34], 0);
        assert_eq!(b.status(0x05), 0x34);
        assert_eq!(b.f.nv_registers()[0], 0x34);
        // After `50`: the volatile copy only.
        b.single(0x50);
        b.frame(&[0x01, 0x00], 0);
        assert_eq!(b.status(0x05), 0x00);
        assert_eq!(b.f.nv_registers()[0], 0x34);
        // A reset brings the non-volatile value back.
        b.single(0x66);
        b.out(0x99);
        b.ctrl(3);
        assert_eq!(b.status(0x05), 0x34);
        // `99` without `66` right before it is no reset.
        b.single(0x50);
        b.frame(&[0x01, 0x00], 0);
        b.single(0x66);
        b.out(0x05);
        b.out(0x99);
        b.ctrl(3);
        assert_eq!(b.status(0x05), 0x00);

        // Protected range refuses program and erase.
        b.single(0x50);
        b.frame(&[0x01, 0x34], 0);
        assert!(b.write_page(0x02, 0x10_0000, &[0; 4]));
        assert!(b.erase(0x20, 0x10_0000));
        assert_eq!(b.f.image()[0x10_0000], 0xFF);
        assert!(b.write_page(0x02, 0x40_0000, &[0; 4]));
        assert_eq!(b.f.image()[0x40_0000], 0x00);

        // Two bytes after `01` write status 2 as well; read-only and
        // reserved bits stay 0. Status 3 by `11`.
        b.single(0x06);
        b.frame(&[0x01, 0xFF, 0x40], 0);
        assert_eq!([b.status(0x05), b.status(0x35)], [0xFC, 0x40]);
        b.single(0x06);
        b.frame(&[0x11, 0xFF], 0);
        assert_eq!(b.status(0x15), 0x64);

        // Lock-down: no register write until power-off, not even after
        // a reset.
        b.single(0x50);
        b.frame(&[0x31, 0x01], 0);
        assert_eq!(b.status(0x35), 0x01);
        b.single(0x06);
        b.frame(&[0x01, 0x00], 0);
        assert_eq!(b.status(0x05), 0xFC);
        b.single(0x66);
        b.out(0x99);
        b.ctrl(3);
        b.single(0x06);
        b.frame(&[0x01, 0x00], 0);
        assert_eq!(b.status(0x05) & 0xFC, 0xFC);
        b.f.reset();
        b.single(0x06);
        b.frame(&[0x01, 0x00], 0);
        assert_eq!(b.status(0x05) & 0xFC, 0xFC);
        b.f.power_cycle();
        assert_eq!(b.status(0x35), 0x40);
        b.single(0x06);
        b.frame(&[0x01, 0x00], 0);
        assert_eq!(b.status(0x05), 0x00);
    }

    #[test]
    fn s25_any_register_access() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        // Configuration registers by their read commands.
        assert_eq!(
            [b.status(0x35), b.status(0x15), b.status(0x33)],
            [0, 0x60, 0x78]
        );
        // Volatile write of status 1 through `71`.
        b.single(0x06);
        b.frame(&[0x71, 0x80, 0x00, 0x00, 0x24], 0);
        assert_eq!(b.frame(&[0x65, 0x80, 0, 0, 0], 1), [0x24]);
        assert_eq!(b.frame(&[0x65, 0x00, 0, 0, 0], 1), [0x00]);
        // Non-volatile write changes both.
        b.single(0x06);
        b.frame(&[0x71, 0x00, 0x00, 0x00, 0x28], 0);
        assert_eq!(b.frame(&[0x65, 0x00, 0, 0, 0], 2), [0x28, 0x28]);
        assert_eq!(b.status(0x05), 0x28);
        // Four bytes after `01` reach all four registers.
        b.single(0x06);
        b.frame(&[0x01, 0x00, 0x40, 0x60, 0x70], 0);
        assert_eq!(b.f.nv_registers(), [0x00, 0x40, 0x60, 0x70]);
        // The machine's copy comes back as after power-on.
        let mut c = Bench::new(FlashChip::S25fl128l);
        c.f.set_nv_registers([0x34, 0x00, 0x60, 0x78]);
        assert_eq!(c.status(0x05), 0x34);
    }

    #[test]
    fn unknown_opcodes_are_harmless() {
        for chip in [FlashChip::S25fl128l, FlashChip::W25q128jv] {
            let mut b = Bench::new(chip);
            b.f.load_image(&[0x12; 0x2000]);
            let before = b.f.clone();
            for op in 0..=255u8 {
                if matches!(op, 0x06 | 0x50 | 0x66) {
                    continue;
                }
                let rx = b.frame(&[op], 2);
                assert_eq!(rx.len(), 2);
                b.frame(&[op, 0, 0, 0x10, 0, 0, 0, 0, 0], 0);
            }
            // Nothing was enabled, so nothing changed.
            assert_eq!(b.f.image(), before.image());
            assert_eq!(b.f.nv_registers(), before.nv_registers());
            assert!(b.f.take_dirty().is_empty());
            // A very long program frame does not run anything over.
            b.single(0x06);
            b.ctrl(1);
            b.out(0x02);
            for _ in 0..100_000 {
                b.out(0x00);
            }
            b.ctrl(3);
        }
    }

    #[test]
    fn images_and_dirty_sectors() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        assert_eq!(b.f.image().len(), FLASH_SIZE);
        assert!(b.f.image().iter().all(|&v| v == 0xFF));
        // A short image is filled up.
        b.f.load_image(&[1, 2, 3]);
        assert_eq!(&b.f.image()[..4], &[1, 2, 3, 0xFF]);
        assert!(!b.f.is_dirty());
        // A long one is cut.
        b.f.load_image(&vec![0xA5; FLASH_SIZE + 10]);
        assert_eq!(b.f.image().len(), FLASH_SIZE);
        assert_eq!(b.f.image()[FLASH_SIZE - 1], 0xA5);
        b.f.load_image(&[]);
        assert!(b.f.image().iter().all(|&v| v == 0xFF));

        // Programs mark their sector, in ascending order whatever the
        // order of the writes.
        b.write_page(0x12, 0xFF_F000, &[0x00]);
        b.write_page(0x12, 0x00_3000, &[0x00]);
        b.write_page(0x12, 0x40_0100, &[0x00]);
        assert!(b.f.is_dirty());
        assert_eq!(b.f.take_dirty(), [3, 0x400, 0xFFF]);
        assert!(b.f.take_dirty().is_empty());
        // Writing what is there, or erasing what is erased, changes
        // nothing.
        b.write_page(0x12, 0x00_3000, &[0x00]);
        b.write_page(0x12, 0x00_5000, &[0xFF; 16]);
        b.erase(0x21, 0x00_6000);
        assert!(b.f.take_dirty().is_empty());
        b.erase(0x21, 0x00_3000);
        assert_eq!(b.f.take_dirty(), [3]);
        // A new image starts clean.
        b.write_page(0x12, 0x00_3000, &[0x00]);
        b.f.load_image(&[]);
        assert!(b.f.take_dirty().is_empty());
    }

    #[test]
    fn layout_and_application_header() {
        assert_eq!(FlashLayout::for_fpga_type(2), LAYOUT_50T);
        assert_eq!(FlashLayout::for_fpga_type(3), LAYOUT_100T);
        for l in [LAYOUT_50T, LAYOUT_100T] {
            assert_eq!(l.fpga.end(), l.application.start);
            assert_eq!(l.application.end(), l.flash_disk.start);
            assert_eq!(l.flash_disk.end(), CONFIG_AREA.start);
            assert_eq!(l.flash_disk.len as usize % SECTOR_SIZE, 0);
        }
        assert_eq!(CONFIG_AREA.end() as usize, FLASH_SIZE);
        assert_eq!(FlashChip::S25fl128l.config_page_addr(0), Some(0xFE_8000));
        assert_eq!(FlashChip::S25fl128l.config_page_addr(23), Some(0xFF_F000));
        assert_eq!(FlashChip::S25fl128l.config_page_addr(24), None);
        assert_eq!(FlashChip::W25q128jv.config_page_addr(0), Some(0xFF_0000));
        assert_eq!(FlashChip::W25q128jv.config_page_addr(16), None);

        let mut f = SpiFlash::new(FlashChip::W25q128jv);
        assert_eq!(AppHeader::read(f.image(), 2), None);
        let mut img = vec![0xFF; 0x22_0010];
        img[0x22_0000..0x22_000C].copy_from_slice(&[
            0x00, 0x00, 0x01, 0x00, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        ]);
        f.load_image(&img);
        assert_eq!(
            AppHeader::read(f.image(), 2),
            Some(AppHeader {
                load_addr: 0x1_0000,
                len: 0x1234,
                run_addr: 0x1_0000,
            })
        );
        assert_eq!(AppHeader::read(f.image(), 3), None);
        assert_eq!(AppHeader::read(&img[..0x22_0004], 2), None);

        // The boot loader's read: `03`, the address, three words.
        let mut b = Bench::new(FlashChip::W25q128jv);
        b.f.load_image(&img);
        b.ctrl(3);
        b.out(0xFF);
        b.ctrl(1);
        b.out(0xFF);
        b.ctrl(3);
        b.ctrl(1);
        for v in [0x03, 0x22, 0x00, 0x00] {
            b.out(v);
        }
        assert_eq!([b.in32(), b.in32(), b.in32()], [0x1_0000, 0x1234, 0x1_0000]);
        b.ctrl(0);
        assert_eq!(b.rd(CTRL), 0x04);
    }

    #[test]
    fn read_wraps_at_the_end_of_the_chip() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        let mut img = vec![0xFF; FLASH_SIZE];
        img[0] = 0x11;
        img[FLASH_SIZE - 1] = 0x22;
        b.f.load_image(&img);
        assert_eq!(b.read(0x03, 0xFF_FFFF, 2), [0x22, 0x11]);
        // The top address byte of a 4-byte address is not decoded.
        assert_eq!(b.read(0x13, 0x01FF_FFFF, 2), [0x22, 0x11]);
    }

    fn saved(f: &SpiFlash) -> Vec<u8> {
        let mut w = StateWriter::new();
        f.save(&mut w);
        w.into_bytes()
    }

    #[test]
    fn savestate_round_trip() {
        let mut b = Bench::new(FlashChip::S25fl128l);
        b.f.set_unique_id([9, 8, 7, 6, 5, 4, 3, 2]);
        b.write_page(0x12, 0x00_0000, &pattern(1));
        b.write_page(0x12, 0x58_0200, &pattern(2));
        b.write_page(0x12, 0xFF_FF00, &pattern(3));
        b.single(0x06);
        b.frame(&[0x01, 0x34, 0x00], 0);
        // In the middle of a program frame.
        b.single(0x06);
        b.ctrl(1);
        for v in [0x12, 0x00, 0x60, 0x00, 0x10, 0xAA, 0xBB] {
            b.out(v);
        }
        b.f.take_dirty();
        let bytes = saved(&b.f);
        // Three sectors and little else.
        assert!(bytes.len() < 4 * SECTOR_SIZE);

        let mut d = SpiFlash::new(FlashChip::S25fl128l);
        d.load_image(&[0x77; 0x3000]);
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(d.image(), b.f.image());
        // What differs from the image before is to be written back:
        // sectors 0–2 of the old image, and the three of the state.
        assert_eq!(d.take_dirty(), [0, 1, 2, 0x580, 0xFFF]);
        assert_eq!(saved(&d), bytes);
        // The loaded copy goes on where the original stood.
        let mut b2 = Bench::new(FlashChip::S25fl128l);
        b2.f = d;
        for x in [&mut b, &mut b2] {
            x.out(0xCC);
            x.ctrl(3);
            assert_eq!(&x.f.image()[0x60_0010..0x60_0013], &[0xAA, 0xBB, 0xCC]);
            assert_eq!(x.status(0x05), 0x34);
        }
        assert!(b.f == b2.f);
    }

    #[test]
    fn savestate_bad_data_gives_errors() {
        let mut b = Bench::new(FlashChip::W25q128jv);
        b.write_page(0x02, 0x40_0000, &pattern(4));
        b.write_page(0x02, 0x40_1000, &pattern(5));
        let bytes = saved(&b.f);

        // Truncated anywhere: an error, and the flash stays as it was.
        let mut d = SpiFlash::new(FlashChip::W25q128jv);
        d.load_image(&[0x42; 64]);
        let before = d.clone();
        for cut in 0..bytes.len() {
            assert!(d.load(&mut StateReader::new(&bytes[..cut])).is_err());
        }
        assert!(d == before);
        // The other chip's state is refused.
        let mut other = SpiFlash::new(FlashChip::S25fl128l);
        assert!(other.load(&mut StateReader::new(&bytes)).is_err());
        // Register bits the chip does not have.
        let mut bad = bytes.clone();
        bad[1 + 8 + 3] = 0x03;
        assert!(d.load(&mut StateReader::new(&bad)).is_err());
        // A frame that the select line does not allow.
        let mut bad = bytes.clone();
        let selected = 1 + 8 + 3 + 8 + 5;
        assert_eq!(bad[selected], 0);
        bad[selected] = 1;
        assert!(d.load(&mut StateReader::new(&bad)).is_err());
        // Every sector marked, far too little data.
        let mut bad = bytes.clone();
        let map = selected + 1 + 1 + 4 + 4 + 5 + PAGE_SIZE + 32;
        bad[map..map + SECTOR_COUNT / 8].fill(0xFF);
        assert!(d.load(&mut StateReader::new(&bad)).is_err());
        assert!(d == before);
        // Garbage of any kind must not panic.
        for seed in 0..64u32 {
            let junk: Vec<u8> = (0..1200u32)
                .map(|i| (i.wrapping_mul(2_654_435_761).wrapping_add(seed * 97) >> 13) as u8)
                .collect();
            let _ = d.load(&mut StateReader::new(&junk));
        }
        // And the good state still loads.
        d.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(d.image(), b.f.image());
    }
}
