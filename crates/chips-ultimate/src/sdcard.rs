//! The SD card slot: the FPGA's SPI port at `$060000`–`$0600FF` and an
//! SDHC card in SPI mode behind it.
//!
//! # The SPI port
//!
//! Four registers, each four bytes wide (every byte of a register is the
//! register: only address bits 3–2 are decoded), and the four repeat every
//! 16 bytes through the page. A 32-bit access arrives as four byte
//! accesses at ascending addresses and so runs four transfers, the first
//! byte on the wire in bits 7–0.
//!
//! | Offset | Read | Write |
//! |---|---|---|
//! | `+0` DATA | one transfer with `$FF` out; the byte that came in | one transfer with the byte out; what comes in is lost |
//! | `+4` SPEED | divider bits 7–0 (`$F4` after reset: 500) | divider: bits 7–0 the byte, bit 8 its bit 7 |
//! | `+8` CTRL | bit 0 force select, bit 1 select level, bit 2 card detect, bit 3 write protect | bit 0 force select, bit 1 select level |
//! | `+C` CRC | bits 7–1 the CRC-7 of every bit shifted out since the clear, bit 0 set | any value clears the CRC |
//!
//! The select line: with "force" clear the port lowers it for every byte
//! on its own; with "force" set it follows the level bit, so force and
//! level both set deselect the card. After reset force is clear and level
//! set, so CTRL reads `$02` with no card. CRC-7 uses x⁷ + x³ + 1, the bits
//! MSB first, which is the command CRC of the SD protocol: the firmware
//! clears it before a command and sends what it reads back as the
//! command's last byte. Every access waits for the running transfer, so
//! the firmware never polls a busy flag; the port raises no interrupt.
//!
//! # The card
//!
//! An SDHC card (block addressed, CSD version 2.0) as described by the SD
//! Physical Layer Simplified Specification, SPI mode: power-up clocks,
//! CMD0 into SPI mode, CMD8, CMD55 + ACMD41 until ready, CMD58, CMD9,
//! CMD10, CMD12, CMD13, CMD16, CMD17, CMD18, CMD24, CMD25, CMD59 and
//! ACMD13/22/23/42/51. Its contents are unit [`unit::SD`] of
//! [`IoCtx::storage`]; its size is the medium's (the CSD's C_SIZE counts
//! 512 KB units of it), its write protect switch the medium's
//! [`Storage::write_protected`]. No medium: no card, card detect clear,
//! the data line idles high (`$FF`).
//!
//! Answers come without delay: R1 in the byte after the command's last,
//! the data token in the byte after R1, the data response in the byte
//! after the block's CRC, then a short, fixed busy time. The firmware
//! polls for them in loops of 240 000 and 600 000 reads during which no
//! time may be assumed to pass; nothing here depends on the clock.
//!
//! Changes of medium are seen at each access by
//! [`Storage::generation`]. A medium taken out clears card detect at once,
//! one put in sets it at once; one exchanged for another holds card detect
//! clear for 250 ms first, so that the firmware's poll (every 100 ms) sees
//! the removal and initialises the new card.
//!
//! What the firmware does (U64 build, `software/io/sd_card/`): it looks at
//! CTRL every 100 ms and nothing else while no card is in; with a card it
//! runs CMD0, CMD8 (the fourth byte after R1 must echo `$AA`), CMD55 +
//! ACMD41 with HCS until R1 is 0, CMD58 (bit 6 of the first OCR byte:
//! block addresses), then reads and writes one block at a time with CMD17
//! and CMD24, and reads CID and CSD for the medium's size. It sends the
//! read block's CRC bytes and three `$FF` after a written block (two CRC
//! bytes and the byte carrying the data response, which it never looks
//! at), then reads until `$FF`. It leaves CMD10's two CRC bytes unread;
//! a command's first byte starts the next command at any time and
//! drops what the card had still queued.
//!
//! Not modelled: CMD6 (switch function), erase (CMD32/33/38), CMD27/28/29/30
//! (CSD programming, write protect groups), CMD42 (lock), CMD56, SD
//! (non-SPI) bus mode, SDSC byte addressing. They answer "illegal command".
//!
//! Sources (facts only): UE2-C64U-Emulator `docs/hw/07-sd-card-filesystems.md`
//! (registers, hazards H1–H11), `docs/specs/S09-sd-card.md`,
//! `docs/hw/00-memory-map.md`; 1541 Ultimate
//! `fpga/io/spi/vhdl_source/spi_peripheral_io.vhd` and `spi.vhd` (register
//! decode, reset values, select line, CRC-7), `software/io/sd_card/sdio.h`,
//! `sdio.cc`, `sd_card.cc`, `sdcard_manager.cc` (the driver's sequences and
//! loop bounds); the SD Physical Layer Simplified Specification (SPI mode,
//! responses, tokens, CSD 2.0, CID, OCR, SCR layouts).
#![forbid(unsafe_code)]

use crate::io::{unit, IoCtx, IoDevice, Storage, SECTOR};
use mimic_core::{StateError, StateReader, StateWriter};

/// First I/O address of the slot's page.
pub const SD_BASE: u32 = 0x06_0000;
/// Last I/O address of the slot's page.
pub const SD_LAST: u32 = 0x06_00FF;

/// The divider after reset (9 bits).
const RATE_RESET: u16 = 500;
/// How long card detect stays clear when one medium is exchanged for
/// another (250 ms of the board's clock): longer than the firmware's poll.
const SWAP_HOLD: u64 = 25_000_000;
/// Bytes of clock the card wants after power-up before CMD0 (the
/// specification asks for at least 74 clocks).
const POWER_UP_BYTES: u8 = 10;
/// ACMD41 calls until the card has finished its initialisation.
const OP_COND_POLLS: u8 = 2;
/// Busy bytes (`$00`) after a written block or a stop token.
const WRITE_BUSY_BYTES: usize = 8;
/// Bound of the output queue (a block with R1, token and CRC is 516).
const OUT_MAX: usize = 1024;
/// A received data block: 512 bytes and two CRC bytes.
const BLOCK_RX: usize = SECTOR + 2;

/// R1 bits (SPI mode).
const R1_IDLE: u8 = 0x01;
const R1_ILLEGAL: u8 = 0x04;
const R1_CRC: u8 = 0x08;
const R1_PARAMETER: u8 = 0x40;

/// Data tokens.
const TOKEN_START: u8 = 0xFE;
const TOKEN_MULTI: u8 = 0xFC;
const TOKEN_STOP: u8 = 0xFD;
/// Data response tokens: accepted, CRC error, write error.
const DATA_ACCEPTED: u8 = 0x05;
const DATA_CRC_ERROR: u8 = 0x0B;
const DATA_WRITE_ERROR: u8 = 0x0D;
/// Data error tokens: general error, out of range.
const ERROR_TOKEN_ERROR: u8 = 0x01;
const ERROR_TOKEN_RANGE: u8 = 0x08;

/// OCR once the card is ready: powered up, CCS (block addresses),
/// 2.7–3.6 V.
const OCR_READY: [u8; 4] = [0xC0, 0xFF, 0x80, 0x00];

/// CRC-7 (x⁷ + x³ + 1) of one more byte, MSB first.
fn crc7_byte(mut crc: u8, byte: u8) -> u8 {
    for bit in (0..8).rev() {
        let feedback = ((byte >> bit) ^ (crc >> 6)) & 1;
        crc = (crc << 1) & 0x7F;
        if feedback != 0 {
            crc ^= 0x09;
        }
    }
    crc
}

fn crc7(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |c, &b| crc7_byte(c, b))
}

/// CRC-16 of a data block (x¹⁶ + x¹² + x⁵ + 1, start 0, MSB first).
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in bytes {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A 16-byte card register (CSD, CID) with its CRC-7 in byte 15.
fn with_crc7(mut reg: [u8; 16]) -> [u8; 16] {
    reg[15] = crc7(&reg[..15]) << 1 | 1;
    reg
}

/// The CSD (version 2.0) of a card of `sectors` 512-byte sectors.
/// C_SIZE counts 512 KB less one; a medium smaller than that reports one
/// unit.
fn csd(sectors: u64) -> [u8; 16] {
    let c_size = (sectors / 1024).clamp(1, 0x40_0000) - 1;
    with_crc7([
        0x40,                          // CSD_STRUCTURE 1 (version 2.0)
        0x0E,                          // TAAC 1 ms
        0x00,                          // NSAC
        0x32,                          // TRAN_SPEED 25 MHz
        0x5B,                          // CCC 11–4 (classes 0, 2, 4, 5, 7, 8, 10)
        0x59,                          // CCC 3–0, READ_BL_LEN 9
        0x00,                          // no partial or misaligned blocks, no DSR
        ((c_size >> 16) & 0x3F) as u8, // C_SIZE 21–16
        (c_size >> 8) as u8,           // C_SIZE 15–8
        c_size as u8,                  // C_SIZE 7–0
        0x7F,                          // ERASE_BLK_EN, SECTOR_SIZE 6–1
        0x80,                          // SECTOR_SIZE 0, WP_GRP_SIZE 0
        0x0A,                          // R2W_FACTOR 2, WRITE_BL_LEN 3–2
        0x40,                          // WRITE_BL_LEN 1–0
        0x00,                          // file format, copy, write protect
        0,
    ])
}

/// The CID: manufacturer 0, OEM "MM", product "MIMIC", revision 1.0,
/// serial 1, made November 2026.
fn cid() -> [u8; 16] {
    with_crc7([
        0x00, b'M', b'M', b'M', b'I', b'M', b'I', b'C', 0x10, 0x00, 0x00, 0x00, 0x01, 0x01, 0xAB, 0,
    ])
}

/// The SCR: version 1.0 structure, physical layer 2.00, security SDHC,
/// bus widths 1 and 4, physical layer 3.0.
const SCR: [u8; 8] = [0x02, 0x35, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00];

/// What the card is receiving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rx {
    /// Commands: a byte `01xxxxxx` starts one of six bytes.
    Command,
    /// After CMD24/CMD25: waiting for a data token.
    Token,
    /// A data block's bytes and CRC.
    Data,
}

/// The medium as one transfer sees it.
struct Medium<'s> {
    sectors: u64,
    protect: bool,
    storage: Option<&'s mut dyn Storage>,
}

impl Medium<'_> {
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> bool {
        lba < self.sectors
            && self
                .storage
                .as_mut()
                .is_some_and(|s| s.read(unit::SD, lba, buf))
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> bool {
        lba < self.sectors
            && !self.protect
            && self
                .storage
                .as_mut()
                .is_some_and(|s| s.write(unit::SD, lba, data))
    }
}

/// The card's protocol state.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Card {
    /// Bytes clocked since power-up (saturating).
    power_bytes: u8,
    /// CMD0 has put the card into SPI mode.
    spi: bool,
    /// Initialisation not finished (R1 bit 0).
    idle: bool,
    /// CMD8 accepted since the last reset.
    if_cond: bool,
    /// ACMD41 calls with HCS since the last reset.
    op_polls: u8,
    /// The previous command was CMD55.
    app: bool,
    /// CMD59: CRCs of commands and data are checked.
    crc_on: bool,
    cmd: [u8; 6],
    cmd_len: u8,
    rx: Rx,
    /// The data block in reception (bytes and CRC).
    data: Vec<u8>,
    /// Next block of CMD18 or CMD24/CMD25.
    lba: u64,
    multi_read: bool,
    multi_write: bool,
    /// Blocks written by the last CMD25 (ACMD22).
    written: u32,
    /// What the card sends next.
    out: Vec<u8>,
    out_pos: usize,
}

impl Card {
    fn new() -> Self {
        Self {
            power_bytes: 0,
            spi: false,
            idle: true,
            if_cond: false,
            op_polls: 0,
            app: false,
            crc_on: false,
            cmd: [0; 6],
            cmd_len: 0,
            rx: Rx::Command,
            data: Vec::new(),
            lba: 0,
            multi_read: false,
            multi_write: false,
            written: 0,
            out: Vec::new(),
            out_pos: 0,
        }
    }

    fn out_empty(&self) -> bool {
        self.out_pos >= self.out.len()
    }

    fn next_out(&self) -> u8 {
        self.out.get(self.out_pos).copied().unwrap_or(0xFF)
    }

    fn pop(&mut self) -> u8 {
        let v = self.next_out();
        if !self.out_empty() {
            self.out_pos += 1;
            if self.out_empty() {
                self.out.clear();
                self.out_pos = 0;
            }
        }
        v
    }

    fn drop_out(&mut self) {
        self.out.clear();
        self.out_pos = 0;
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.out.len() + bytes.len() <= OUT_MAX {
            self.out.extend_from_slice(bytes);
        }
    }

    fn r1(&self) -> u8 {
        u8::from(self.idle)
    }

    /// A data block: start token, the bytes, their CRC-16.
    fn push_block(&mut self, bytes: &[u8]) {
        self.push(&[TOKEN_START]);
        self.push(bytes);
        self.push(&crc16(bytes).to_be_bytes());
    }

    /// Block `lba` of the medium as a data block, or an error token (which
    /// also ends a multiple block read).
    fn push_sector(&mut self, lba: u64, m: &mut Medium<'_>) {
        let mut buf = [0u8; SECTOR];
        if lba >= m.sectors {
            self.push(&[ERROR_TOKEN_RANGE]);
            self.multi_read = false;
        } else if m.read(lba, &mut buf) {
            self.push_block(&buf);
        } else {
            self.push(&[ERROR_TOKEN_ERROR]);
            self.multi_read = false;
        }
    }

    /// Back to the state after CMD0 (still in SPI mode).
    fn soft_reset(&mut self) {
        let power_bytes = self.power_bytes;
        *self = Self::new();
        self.power_bytes = power_bytes;
        self.spi = true;
    }

    /// The byte the host sent in this transfer.
    fn receive(&mut self, mosi: u8, m: &mut Medium<'_>) {
        match self.rx {
            Rx::Command => self.receive_command(mosi, m),
            Rx::Token => {
                if mosi & 0xC0 == 0x40 {
                    // The host gives up the write and sends a command.
                    self.rx = Rx::Command;
                    self.multi_write = false;
                    self.receive_command(mosi, m);
                    return;
                }
                match mosi {
                    TOKEN_START if !self.multi_write => self.start_data(),
                    TOKEN_MULTI if self.multi_write => self.start_data(),
                    TOKEN_STOP if self.multi_write => {
                        self.multi_write = false;
                        self.rx = Rx::Command;
                        self.push(&[0xFF]);
                        self.push(&[0x00; WRITE_BUSY_BYTES]);
                    }
                    _ => {}
                }
            }
            Rx::Data => {
                self.data.push(mosi);
                if self.data.len() >= BLOCK_RX {
                    self.finish_block(m);
                }
            }
        }
    }

    fn start_data(&mut self) {
        self.rx = Rx::Data;
        self.data.clear();
    }

    fn receive_command(&mut self, mosi: u8, m: &mut Medium<'_>) {
        if self.cmd_len == 0 {
            if mosi & 0xC0 != 0x40 {
                return;
            }
            // A new command drops what the card still had to say and ends
            // a multiple block read.
            self.drop_out();
            self.multi_read = false;
        }
        self.cmd[usize::from(self.cmd_len)] = mosi;
        self.cmd_len += 1;
        if usize::from(self.cmd_len) == self.cmd.len() {
            self.cmd_len = 0;
            self.execute(m);
        }
    }

    fn finish_block(&mut self, m: &mut Medium<'_>) {
        let (bytes, crc) = self.data.split_at(SECTOR);
        let crc_ok = !self.crc_on || crc16(bytes).to_be_bytes() == crc[..2];
        let response = if !crc_ok {
            DATA_CRC_ERROR
        } else if m.write(self.lba, bytes) {
            self.written = self.written.wrapping_add(1);
            DATA_ACCEPTED
        } else {
            DATA_WRITE_ERROR
        };
        self.lba = self.lba.wrapping_add(1);
        self.data.clear();
        self.push(&[response]);
        if response == DATA_ACCEPTED {
            self.push(&[0x00; WRITE_BUSY_BYTES]);
        }
        self.rx = if self.multi_write {
            Rx::Token
        } else {
            Rx::Command
        };
    }

    fn execute(&mut self, m: &mut Medium<'_>) {
        let index = self.cmd[0] & 0x3F;
        let arg = u32::from_be_bytes([self.cmd[1], self.cmd[2], self.cmd[3], self.cmd[4]]);
        let crc_ok = crc7(&self.cmd[..5]) == self.cmd[5] >> 1;

        if !self.spi {
            // Bus mode: only a correct CMD0 after the power-up clocks
            // brings the card into SPI mode; nothing answers otherwise.
            if index == 0 && crc_ok && self.power_bytes >= POWER_UP_BYTES {
                self.soft_reset();
                self.push(&[R1_IDLE]);
            }
            return;
        }

        let app = std::mem::take(&mut self.app);
        if !crc_ok && (self.crc_on || index == 8) {
            self.push(&[self.r1() | R1_CRC]);
            return;
        }
        let allowed_idle = matches!(index, 0 | 8 | 55 | 58 | 59) || (app && index == 41);
        if self.idle && !allowed_idle {
            self.push(&[R1_IDLE | R1_ILLEGAL]);
            return;
        }

        match (app, index) {
            (_, 0) => {
                self.soft_reset();
                self.push(&[R1_IDLE]);
            }
            (_, 8) => {
                // Voltage 2.7–3.6 V only; otherwise the card stays silent.
                if (arg >> 8) & 0xF == 1 {
                    self.if_cond = true;
                    self.push(&[self.r1(), 0x00, 0x00, 0x01, arg as u8]);
                }
            }
            (_, 9) => {
                self.push(&[self.r1()]);
                self.push_block(&csd(m.sectors));
            }
            (_, 10) => {
                self.push(&[self.r1()]);
                self.push_block(&cid());
            }
            (_, 12) => {
                // A stuff byte, then R1.
                self.multi_read = false;
                self.push(&[0xFF, self.r1()]);
            }
            (true, 13) => {
                self.push(&[self.r1(), 0x00]);
                self.push_block(&[0; 64]);
            }
            (false, 13) => self.push(&[self.r1(), 0x00]),
            // Block length is fixed at 512 on a block-addressed card.
            (_, 16) => self.push(&[self.r1()]),
            (_, 17 | 18) => {
                let lba = u64::from(arg);
                if lba >= m.sectors {
                    self.push(&[self.r1() | R1_PARAMETER]);
                } else {
                    self.push(&[self.r1()]);
                    self.multi_read = index == 18;
                    self.push_sector(lba, m);
                    self.lba = lba + 1;
                }
            }
            (true, 22) => {
                self.push(&[self.r1()]);
                self.push_block(&self.written.to_be_bytes());
            }
            (true, 23 | 42) => self.push(&[self.r1()]),
            (_, 24 | 25) => {
                let lba = u64::from(arg);
                if lba >= m.sectors {
                    self.push(&[self.r1() | R1_PARAMETER]);
                } else {
                    self.push(&[self.r1()]);
                    self.lba = lba;
                    self.rx = Rx::Token;
                    self.multi_write = index == 25;
                    if self.multi_write {
                        self.written = 0;
                    }
                }
            }
            (true, 41) => {
                // A block-addressed card finishes only for a host that sent
                // CMD8 and sets HCS.
                if self.if_cond && arg & 0x4000_0000 != 0 {
                    self.op_polls = self.op_polls.saturating_add(1);
                    if self.op_polls >= OP_COND_POLLS {
                        self.idle = false;
                    }
                }
                self.push(&[self.r1()]);
            }
            (true, 51) => {
                self.push(&[self.r1()]);
                self.push_block(&SCR);
            }
            (_, 55) => {
                self.app = true;
                self.push(&[self.r1()]);
            }
            (_, 58) => {
                let ocr = if self.idle {
                    [0x00, OCR_READY[1], OCR_READY[2], OCR_READY[3]]
                } else {
                    OCR_READY
                };
                self.push(&[self.r1()]);
                self.push(&ocr);
            }
            (_, 59) => {
                self.crc_on = arg & 1 != 0;
                self.push(&[self.r1()]);
            }
            _ => self.push(&[self.r1() | R1_ILLEGAL]),
        }
    }
}

/// The SD card slot: SPI port and card (`$060000`–`$0600FF`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdCard {
    /// Clock divider, 9 bits.
    rate: u16,
    force_ss: bool,
    level_ss: bool,
    crc7: u8,
    /// A medium was in the unit at the last look.
    medium: bool,
    /// [`Storage::generation`] at the last look.
    generation: u32,
    /// The medium's size and write protect at the last look.
    sectors: u64,
    protect: bool,
    /// Card detect stays clear until this clock (a medium exchanged).
    hold_until: u64,
    /// Card detect as of the last access.
    detect: bool,
    card: Card,
}

impl Default for SdCard {
    fn default() -> Self {
        Self::new()
    }
}

impl SdCard {
    /// Power-on state: no card seen yet; the first access looks at the
    /// storage.
    pub fn new() -> Self {
        Self {
            rate: RATE_RESET,
            force_ss: false,
            level_ss: true,
            crc7: 0,
            medium: false,
            generation: 0,
            sectors: 0,
            protect: false,
            hold_until: 0,
            detect: false,
            card: Card::new(),
        }
    }

    /// The I/O addresses of this block.
    pub fn claims(addr: u32) -> bool {
        (SD_BASE..=SD_LAST).contains(&(addr & 0xFF_FFFF))
    }

    /// Card detect as of the last access.
    pub fn card_detect(&self) -> bool {
        self.detect
    }

    /// Board clocks one byte on the SPI wire takes at the present divider
    /// (16 half bits, the end and the gap, each `rate + 1` clocks), for a
    /// bus that wants to stall the processor for it.
    pub fn byte_clocks(&self) -> u64 {
        18 * (u64::from(self.rate) + 1) + 2
    }

    /// Look at the storage: medium in or out, exchanged, its size.
    fn refresh(&mut self, ctx: &IoCtx<'_>) {
        let (sectors, generation, protect) = match ctx.storage.as_deref() {
            Some(s) => (
                s.sectors(unit::SD),
                s.generation(unit::SD),
                s.write_protected(unit::SD),
            ),
            None => (None, 0, false),
        };
        let medium = sectors.is_some();
        if medium != self.medium || generation != self.generation {
            if medium && self.medium {
                self.hold_until = ctx.now.saturating_add(SWAP_HOLD);
            }
            self.medium = medium;
            self.generation = generation;
            self.card = Card::new();
        }
        self.sectors = sectors.unwrap_or(0);
        self.protect = protect;
        self.detect = medium && ctx.now >= self.hold_until;
    }

    fn selected(&self) -> bool {
        !(self.force_ss && self.level_ss)
    }

    /// One byte on the wire: `mosi` out, the card's byte in.
    fn transfer(&mut self, mosi: u8, ctx: &mut IoCtx<'_>) -> u8 {
        self.refresh(ctx);
        self.crc7 = crc7_byte(self.crc7, mosi);
        if !self.detect {
            return 0xFF;
        }
        self.card.power_bytes = self.card.power_bytes.saturating_add(1);
        if !self.selected() {
            return 0xFF;
        }
        let mut m = Medium {
            sectors: self.sectors,
            protect: self.protect,
            storage: ctx.storage.as_deref_mut().map(|s| s as &mut dyn Storage),
        };
        let card = &mut self.card;
        let miso = card.pop();
        card.receive(mosi, &mut m);
        if card.multi_read && card.rx == Rx::Command && card.cmd_len == 0 && card.out_empty() {
            let lba = card.lba;
            card.push_sector(lba, &mut m);
            card.lba = lba.wrapping_add(1);
        }
        miso
    }

    fn ctrl(&self) -> u8 {
        u8::from(self.detect && self.protect) << 3
            | u8::from(self.detect) << 2
            | u8::from(self.level_ss) << 1
            | u8::from(self.force_ss)
    }
}

impl IoDevice for SdCard {
    fn read(&mut self, addr: u32, ctx: &mut IoCtx<'_>) -> u8 {
        match (addr >> 2) & 3 {
            0 => self.transfer(0xFF, ctx),
            2 => {
                self.refresh(ctx);
                self.ctrl()
            }
            _ => self.peek(addr),
        }
    }

    fn peek(&self, addr: u32) -> u8 {
        match (addr >> 2) & 3 {
            0 => {
                if self.detect && self.selected() {
                    self.card.next_out()
                } else {
                    0xFF
                }
            }
            1 => self.rate as u8,
            2 => self.ctrl(),
            _ => self.crc7 << 1 | 1,
        }
    }

    fn write(&mut self, addr: u32, v: u8, ctx: &mut IoCtx<'_>) {
        match (addr >> 2) & 3 {
            0 => {
                self.transfer(v, ctx);
            }
            1 => self.rate = u16::from(v) | u16::from(v & 0x80) << 1,
            2 => {
                self.force_ss = v & 1 != 0;
                self.level_ss = v & 2 != 0;
            }
            _ => self.crc7 = 0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"USDC", 1, |w| {
            w.u16(self.rate);
            w.bool(self.force_ss);
            w.bool(self.level_ss);
            w.u8(self.crc7);
            w.bool(self.medium);
            w.u32(self.generation);
            w.u64(self.sectors);
            w.bool(self.protect);
            w.u64(self.hold_until);
            w.bool(self.detect);

            let c = &self.card;
            w.u8(c.power_bytes);
            w.bool(c.spi);
            w.bool(c.idle);
            w.bool(c.if_cond);
            w.u8(c.op_polls);
            w.bool(c.app);
            w.bool(c.crc_on);
            w.raw(&c.cmd);
            w.u8(c.cmd_len);
            w.u8(match c.rx {
                Rx::Command => 0,
                Rx::Token => 1,
                Rx::Data => 2,
            });
            w.bytes(&c.data);
            w.u64(c.lba);
            w.bool(c.multi_read);
            w.bool(c.multi_write);
            w.u32(c.written);
            w.bytes(&c.out[c.out_pos..]);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut r) = r.chunk(*b"USDC", 1)?;
        let r = &mut r;
        let mut n = Self::new();
        n.rate = r.u16()?;
        if n.rate > 0x1FF {
            return Err(StateError::Invalid("SD SPI divider"));
        }
        n.force_ss = r.bool()?;
        n.level_ss = r.bool()?;
        n.crc7 = r.u8()?;
        if n.crc7 > 0x7F {
            return Err(StateError::Invalid("SD SPI CRC-7"));
        }
        n.medium = r.bool()?;
        n.generation = r.u32()?;
        n.sectors = r.u64()?;
        n.protect = r.bool()?;
        n.hold_until = r.u64()?;
        n.detect = r.bool()?;

        let c = &mut n.card;
        c.power_bytes = r.u8()?;
        c.spi = r.bool()?;
        c.idle = r.bool()?;
        c.if_cond = r.bool()?;
        c.op_polls = r.u8()?;
        c.app = r.bool()?;
        c.crc_on = r.bool()?;
        r.raw_into(&mut c.cmd)?;
        c.cmd_len = r.u8()?;
        if usize::from(c.cmd_len) >= c.cmd.len() {
            return Err(StateError::Invalid("SD command length"));
        }
        c.rx = match r.u8()? {
            0 => Rx::Command,
            1 => Rx::Token,
            2 => Rx::Data,
            _ => return Err(StateError::Invalid("SD receive state")),
        };
        let data = r.bytes()?;
        if data.len() >= BLOCK_RX {
            return Err(StateError::Invalid("SD data block length"));
        }
        c.data = data.to_vec();
        c.lba = r.u64()?;
        c.multi_read = r.bool()?;
        c.multi_write = r.bool()?;
        c.written = r.u32()?;
        let out = r.bytes()?;
        if out.len() > OUT_MAX {
            return Err(StateError::Invalid("SD output queue length"));
        }
        c.out = out.to_vec();
        c.out_pos = 0;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{MemStorage, NullC64};
    use crate::irq::Irq;

    const DATA: u32 = 0x06_0000;
    const SPEED: u32 = 0x06_0004;
    const CTRL: u32 = 0x06_0008;
    const CRC: u32 = 0x06_000C;

    /// The slot with storage and the rest of an access context.
    struct Rig {
        sd: SdCard,
        storage: MemStorage,
        /// `false`: the context has no storage at all.
        with_storage: bool,
        ram: Vec<u8>,
        irq: Irq,
        console: Vec<u8>,
        c64: NullC64,
        now: u64,
        /// DATA reads so far (to check the firmware's loop bounds).
        data_reads: u64,
    }

    /// The firmware's view of what `init` gave.
    #[derive(Debug, PartialEq, Eq)]
    enum Init {
        Ok { sdhc: bool, acmd41: u32 },
        NoInit,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                sd: SdCard::new(),
                storage: MemStorage::default(),
                with_storage: true,
                ram: vec![0; 16],
                irq: Irq::default(),
                console: Vec::new(),
                c64: NullC64::default(),
                now: 0,
                data_reads: 0,
            }
        }

        /// A card with an image of `sectors` sectors, sector `n` filled
        /// with a pattern of `n`.
        fn with_card(sectors: usize) -> Self {
            let mut r = Self::new();
            r.storage.set(unit::SD, Some(pattern_image(sectors)));
            r
        }

        fn rd(&mut self, addr: u32) -> u8 {
            if (addr >> 2) & 3 == 0 {
                self.data_reads += 1;
            }
            let storage: Option<&mut dyn Storage> = if self.with_storage {
                Some(&mut self.storage)
            } else {
                None
            };
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage,
                net: None,
            };
            self.sd.read(addr, &mut ctx)
        }

        fn wr(&mut self, addr: u32, v: u8) {
            let storage: Option<&mut dyn Storage> = if self.with_storage {
                Some(&mut self.storage)
            } else {
                None
            };
            let mut ctx = IoCtx {
                now: self.now,
                ram: &mut self.ram,
                irq: &mut self.irq,
                console: &mut self.console,
                c64: &mut self.c64,
                storage,
                net: None,
            };
            self.sd.write(addr, v, &mut ctx);
        }

        // The firmware's driver, step by step as it touches the port.

        fn sense(&mut self) -> u8 {
            self.rd(CTRL) >> 2
        }

        fn sdio_init(&mut self) {
            self.wr(SPEED, 254);
            self.wr(CTRL, 0x03);
            for _ in 0..100 {
                self.wr(DATA, 0xFF);
            }
            self.wr(CTRL, 0x00);
        }

        fn cmd(&mut self, n: u8, x: u16, y: u16) {
            self.wr(DATA, 0xFF);
            self.wr(CRC, 0);
            self.wr(DATA, 0x40 | n);
            self.wr(DATA, (x >> 8) as u8);
            self.wr(DATA, x as u8);
            self.wr(DATA, (y >> 8) as u8);
            self.wr(DATA, y as u8);
            let crc = self.rd(CRC);
            self.wr(DATA, crc);
        }

        fn resp8b(&mut self) -> u8 {
            let mut resp = 0xFF;
            for _ in 0..8 {
                resp = self.rd(DATA);
                if resp != 0xFF {
                    break;
                }
            }
            resp
        }

        fn init(&mut self) -> Init {
            self.sdio_init();
            self.wr(SPEED, 200);
            let mut resp;
            let mut i = 100;
            loop {
                self.cmd(0, 0, 0);
                resp = self.resp8b();
                if resp == 1 || i == 0 {
                    break;
                }
                i -= 1;
            }
            if resp != 1 {
                return Init::NoInit;
            }
            self.cmd(8, 0, 0x01AA);
            resp = self.resp8b();
            let v2 = resp & 0x04 == 0;
            if v2 {
                self.rd(DATA);
                self.wr(DATA, 0xFF);
                self.rd(DATA);
                if self.rd(DATA) != 0xAA {
                    return Init::NoInit;
                }
            }
            let mut acmd41 = 0;
            let mut i = 32000;
            loop {
                self.cmd(55, 0, 0);
                self.resp8b();
                self.cmd(41, 0x4000, 0);
                acmd41 += 1;
                resp = self.resp8b();
                if resp != 1 || i == 0 {
                    break;
                }
                i -= 1;
            }
            if resp != 0 {
                return Init::NoInit;
            }
            let mut sdhc = false;
            if v2 {
                self.cmd(58, 0, 0);
                self.resp8b();
                let ocr = self.rd(DATA);
                for _ in 0..3 {
                    self.wr(DATA, 0xFF);
                }
                sdhc = ocr & 0x40 != 0;
            }
            self.wr(SPEED, 1);
            Init::Ok { sdhc, acmd41 }
        }

        /// `sdio_read_block` with 32-bit loads (four bytes at +0..+3).
        fn read_block(&mut self, buf: &mut [u8; SECTOR]) -> u8 {
            let mut timeout = 240_000;
            let b = loop {
                let b = self.rd(DATA);
                if b != 0xFF {
                    break b;
                }
                timeout -= 1;
                if timeout == 0 {
                    return 0xFF;
                }
            };
            if b != 0xFE {
                return b;
            }
            for word in buf.chunks_mut(4) {
                for (k, byte) in word.iter_mut().enumerate() {
                    *byte = self.rd(DATA + k as u32);
                }
            }
            self.wr(DATA, 0xFF);
            self.wr(DATA, 0xFF);
            0
        }

        /// `SdCard::read` for one sector (SDHC: the block address).
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR]) -> Result<(), u8> {
            self.cmd(17, (lba >> 16) as u16, lba as u16);
            let r1 = self.resp8b();
            if r1 != 0 {
                return Err(r1);
            }
            match self.read_block(buf) {
                0 => Ok(()),
                e => Err(e),
            }
        }

        /// `sdio_write_block` with 32-bit stores; `Ok` with the byte that
        /// carried the data response.
        fn write_block(&mut self, data: &[u8; SECTOR]) -> Result<u8, ()> {
            self.wr(DATA, 0xFE);
            for word in data.chunks(4) {
                for (k, &byte) in word.iter().enumerate() {
                    self.wr(DATA + k as u32, byte);
                }
            }
            self.wr(DATA, 0xFF);
            self.wr(DATA, 0xFF);
            // The third: the firmware writes it; here it is read to see
            // the data response the firmware never looks at.
            let response = self.rd(DATA);
            let mut timeout = 600_000;
            while self.rd(DATA) != 0xFF {
                timeout -= 1;
                if timeout == 0 {
                    return Err(());
                }
            }
            Ok(response)
        }

        fn write_sector(&mut self, lba: u32, data: &[u8; SECTOR]) -> Result<u8, ()> {
            self.cmd(24, (lba >> 16) as u16, lba as u16);
            self.resp8b();
            self.write_block(data)
        }

        /// `get_drive_size`: CID, then CSD; `None` where the firmware
        /// gives `RES_ERROR`.
        fn drive_size(&mut self) -> Option<(u32, [u8; 16], [u8; 16])> {
            let read_reg = |r: &mut Self, n: u8, crc_bytes: bool| {
                r.cmd(n, 0, 0);
                let mut q = 200;
                loop {
                    let resp = r.resp8b();
                    q -= 1;
                    if resp == 0xFE || q == 0 {
                        break;
                    }
                }
                let mut iob = [0u8; 16];
                if q != 0 {
                    for b in &mut iob {
                        *b = r.rd(DATA);
                    }
                }
                if crc_bytes {
                    r.wr(DATA, 0xFF);
                    r.wr(DATA, 0xFF);
                }
                (q != 0).then_some(iob)
            };
            let cid = read_reg(self, 10, false)?;
            let csd = read_reg(self, 9, true)?;
            if csd[0] & 0xC0 == 0 {
                return None;
            }
            // The firmware keeps C_SIZE in 16 bits.
            let c_size =
                (u32::from(csd[7] & 0x3F) << 16 | u32::from(csd[8]) << 8 | u32::from(csd[9]))
                    & 0xFFFF;
            Some(((c_size + 1) << 10, cid, csd))
        }
    }

    fn pattern(lba: usize) -> Vec<u8> {
        (0..SECTOR)
            .map(|i| (i as u8).wrapping_mul(7) ^ (lba as u8).wrapping_add(0x31))
            .collect()
    }

    fn pattern_image(sectors: usize) -> Vec<u8> {
        (0..sectors).flat_map(pattern).collect()
    }

    fn ready() -> Rig {
        let mut r = Rig::with_card(2048);
        assert!(matches!(r.init(), Init::Ok { sdhc: true, .. }));
        r
    }

    #[test]
    fn claims_the_page() {
        assert!(SdCard::claims(0x06_0000));
        assert!(SdCard::claims(0x06_00FF));
        assert!(SdCard::claims(0x1006_0010));
        assert!(!SdCard::claims(0x06_0100));
        assert!(!SdCard::claims(0x05_FFFF));
    }

    #[test]
    fn h1_no_storage_reads_no_card() {
        let mut r = Rig::new();
        r.with_storage = false;
        assert_eq!(r.rd(CTRL), 0x02);
        assert_eq!(r.sense() & 1, 0);
        assert_eq!(r.rd(DATA), 0xFF);
        assert_eq!(r.init(), Init::NoInit);
    }

    #[test]
    fn h1_empty_slot_is_the_no_card_path() {
        let mut r = Rig::new();
        r.storage.set(unit::SD, None);
        assert_eq!(r.rd(CTRL), 0x02, "reset value: level high, no card");
        // The manager does nothing more than look at the sense bits.
        assert_eq!(r.sense() & 1, 0);
        assert!(!r.sd.card_detect());
        assert_eq!(r.init(), Init::NoInit);
    }

    #[test]
    fn registers_after_reset() {
        let mut r = Rig::new();
        assert_eq!(r.rd(SPEED), 0xF4);
        assert_eq!(r.rd(CRC), 0x01);
        r.wr(SPEED, 0xFE);
        assert_eq!(r.rd(SPEED + 1), 0xFE, "every byte of a register");
        assert_eq!(r.sd.rate, 0x1FE);
        r.wr(SPEED, 0x01);
        assert_eq!(r.sd.rate, 0x001);
        r.wr(CTRL + 0x10, 0x03);
        assert_eq!(r.rd(CTRL + 3), 0x03, "repeats every 16 bytes");
    }

    #[test]
    fn crc_register_gives_the_command_crc() {
        let mut r = Rig::new();
        r.wr(CRC, 0);
        for b in [0x40, 0, 0, 0, 0] {
            r.wr(DATA, b);
        }
        assert_eq!(r.rd(CRC), 0x95);
        r.wr(CRC + 2, 0x55);
        for b in [0x48, 0, 0, 0x01, 0xAA] {
            r.wr(DATA, b);
        }
        assert_eq!(r.rd(CRC), 0x87);
        // Reads shift $FF through the CRC as well.
        r.wr(CRC, 0);
        r.rd(DATA);
        assert_eq!(r.rd(CRC), crc7_byte(0, 0xFF) << 1 | 1);
    }

    #[test]
    fn firmware_init_sequence() {
        let mut r = Rig::with_card(2048);
        assert_eq!(r.sense() & 3, 1, "card detect, not protected");
        assert_eq!(
            r.init(),
            Init::Ok {
                sdhc: true,
                acmd41: u32::from(OP_COND_POLLS)
            }
        );
        assert!(!r.sd.card.idle);
        assert!(r.sd.card.spi);
        assert_eq!(r.sd.rate, 1);
    }

    #[test]
    fn h3_h4_responses_byte_for_byte() {
        let mut r = Rig::with_card(2048);
        r.sdio_init();
        r.cmd(0, 0, 0);
        assert_eq!(r.rd(DATA), 0x01, "R1 on the first read (H3)");
        r.cmd(8, 0, 0x01AA);
        let r7: Vec<u8> = (0..5).map(|_| r.rd(DATA)).collect();
        assert_eq!(r7, [0x01, 0x00, 0x00, 0x01, 0xAA], "H4");
        assert_eq!(r.rd(DATA), 0xFF);
        r.cmd(58, 0, 0);
        let ocr: Vec<u8> = (0..5).map(|_| r.rd(DATA)).collect();
        assert_eq!(ocr, [0x01, 0x00, 0xFF, 0x80, 0x00], "busy before ready");
    }

    #[test]
    fn cmd0_needs_power_up_clocks_and_select() {
        let mut r = Rig::with_card(2048);
        // Without the 100 dummy bytes the first CMD0 comes too early.
        r.cmd(0, 0, 0);
        assert_eq!(r.resp8b(), 0xFF);
        r.cmd(0, 0, 0);
        assert_eq!(r.resp8b(), 0x01, "the firmware's retry gets it");

        // A deselected card hears nothing.
        let mut r = Rig::with_card(2048);
        r.sdio_init();
        r.wr(CTRL, 0x03);
        r.cmd(0, 0, 0);
        assert_eq!(r.resp8b(), 0xFF);
        assert!(!r.sd.card.spi);
        r.wr(CTRL, 0x01); // forced low
        r.cmd(0, 0, 0);
        assert_eq!(r.resp8b(), 0x01);
    }

    #[test]
    fn command_crc_checks() {
        let mut r = Rig::with_card(2048);
        r.sdio_init();
        // CMD0 with a wrong CRC is not heard in bus mode.
        for b in [0xFF, 0x40, 0, 0, 0, 0, 0x01] {
            r.wr(DATA, b);
        }
        assert_eq!(r.resp8b(), 0xFF);
        r.cmd(0, 0, 0);
        assert_eq!(r.resp8b(), 0x01);
        // CMD8 is checked in SPI mode too.
        for b in [0xFF, 0x48, 0, 0, 0x01, 0xAA, 0x01] {
            r.wr(DATA, b);
        }
        assert_eq!(r.resp8b(), R1_IDLE | R1_CRC);
        // Others are not, until CMD59.
        for b in [0xFF, 0x77, 0, 0, 0, 0, 0x01] {
            r.wr(DATA, b);
        }
        assert_eq!(r.resp8b(), 0x01);
        r.cmd(59, 0, 1);
        assert_eq!(r.resp8b(), 0x01);
        for b in [0xFF, 0x77, 0, 0, 0, 0, 0x01] {
            r.wr(DATA, b);
        }
        assert_eq!(r.resp8b(), R1_IDLE | R1_CRC);
    }

    #[test]
    fn idle_card_refuses_reads_and_hcs_is_needed() {
        let mut r = Rig::with_card(2048);
        r.sdio_init();
        r.cmd(0, 0, 0);
        r.resp8b();
        r.cmd(17, 0, 0);
        assert_eq!(r.resp8b(), R1_IDLE | R1_ILLEGAL);
        r.cmd(8, 0, 0x01AA);
        r.resp8b();
        for _ in 0..10 {
            r.cmd(55, 0, 0);
            r.resp8b();
            r.cmd(41, 0, 0);
            assert_eq!(r.resp8b(), 0x01, "no HCS: an SDHC card stays idle");
        }
    }

    #[test]
    fn h7_read_single_block_with_32_bit_loads() {
        let mut r = ready();
        for lba in [0u32, 1, 77, 2047] {
            let before = r.data_reads;
            let mut buf = [0u8; SECTOR];
            assert_eq!(r.read_sector(lba, &mut buf), Ok(()));
            assert_eq!(buf[..], pattern(lba as usize)[..]);
            // R1 + token + 512, no polling.
            assert_eq!(r.data_reads - before, 1 + 1 + 512);
        }
        // Unaligned buffer path: byte reads; also the CRC is real.
        r.cmd(17, 0, 5);
        assert_eq!(r.resp8b(), 0);
        assert_eq!(r.rd(DATA), 0xFE);
        let data: Vec<u8> = (0..SECTOR).map(|_| r.rd(DATA)).collect();
        let crc = u16::from(r.rd(DATA)) << 8 | u16::from(r.rd(DATA));
        assert_eq!(data, pattern(5));
        assert_eq!(crc, crc16(&data));
        assert_eq!(r.rd(DATA), 0xFF);
    }

    #[test]
    fn read_past_the_end_is_a_parameter_error() {
        let mut r = ready();
        let mut buf = [0u8; SECTOR];
        assert_eq!(r.read_sector(2048, &mut buf), Err(R1_PARAMETER));
        assert_eq!(r.read_sector(0xFFFF_FFFF, &mut buf), Err(R1_PARAMETER));
        // The card goes on working.
        assert_eq!(r.read_sector(3, &mut buf), Ok(()));
    }

    #[test]
    fn h8_write_single_block_round_trip() {
        let mut r = ready();
        let block: [u8; SECTOR] = std::array::from_fn(|i| (i as u8) ^ 0xA5);
        let before = r.data_reads;
        assert_eq!(r.write_sector(9, &block), Ok(DATA_ACCEPTED));
        assert!(r.data_reads - before < 600_000);
        let img = r.storage.units[0].as_ref().unwrap();
        assert_eq!(img[9 * SECTOR..10 * SECTOR], block[..]);
        assert_eq!(
            img[8 * SECTOR..9 * SECTOR],
            pattern(8)[..],
            "neighbour kept"
        );
        let mut buf = [0u8; SECTOR];
        assert_eq!(r.read_sector(9, &mut buf), Ok(()));
        assert_eq!(buf, block);
    }

    #[test]
    fn write_data_bytes_do_not_start_commands() {
        let mut r = ready();
        let block = [0x51u8; SECTOR]; // looks like CMD17
        assert_eq!(r.write_sector(0, &block), Ok(DATA_ACCEPTED));
        assert_eq!(r.storage.units[0].as_ref().unwrap()[..SECTOR], block[..]);
    }

    #[test]
    fn write_past_the_end_and_data_crc() {
        let mut r = ready();
        r.cmd(24, 0, 2048);
        assert_eq!(r.resp8b(), R1_PARAMETER);
        // With CRC on, the firmware's $FF $FF is a CRC error.
        r.cmd(59, 0, 1);
        assert_eq!(r.resp8b(), 0);
        let block = [0x11u8; SECTOR];
        assert_eq!(r.write_sector(4, &block), Ok(DATA_CRC_ERROR));
        assert_eq!(
            r.storage.units[0].as_ref().unwrap()[4 * SECTOR],
            pattern(4)[0]
        );
        // The right CRC is accepted (the command's CRC too, now checked).
        r.cmd(24, 0, 4);
        assert_eq!(r.resp8b(), 0);
        r.wr(DATA, 0xFE);
        for &b in &block {
            r.wr(DATA, b);
        }
        for b in crc16(&block).to_be_bytes() {
            r.wr(DATA, b);
        }
        assert_eq!(r.rd(DATA), DATA_ACCEPTED);
        assert_eq!(r.storage.units[0].as_ref().unwrap()[4 * SECTOR], 0x11);
    }

    #[test]
    fn h2_write_protect() {
        let mut r = Rig::with_card(2048);
        r.storage.protected[0] = true;
        assert_eq!(r.rd(CTRL) & 0x0C, 0x0C);
        assert_eq!(r.sense() & 3, 3, "detect and protect");
        assert!(matches!(r.init(), Init::Ok { .. }));
        let block = [0u8; SECTOR];
        assert_eq!(r.write_sector(1, &block), Ok(DATA_WRITE_ERROR));
        assert_eq!(
            r.storage.units[0].as_ref().unwrap()[SECTOR..2 * SECTOR],
            pattern(1)[..]
        );
        let mut buf = [0u8; SECTOR];
        assert_eq!(r.read_sector(1, &mut buf), Ok(()));
    }

    #[test]
    fn multi_block_read_with_stop() {
        let mut r = ready();
        r.cmd(18, 0, 100);
        assert_eq!(r.resp8b(), 0);
        for lba in 100..104 {
            assert_eq!(r.rd(DATA), 0xFE);
            let data: Vec<u8> = (0..SECTOR).map(|_| r.rd(DATA)).collect();
            assert_eq!(data, pattern(lba));
            r.rd(DATA);
            r.rd(DATA);
        }
        r.cmd(12, 0, 0);
        assert_eq!(r.rd(DATA), 0xFF, "stuff byte");
        assert_eq!(r.rd(DATA), 0x00);
        assert_eq!(r.rd(DATA), 0xFF);
        assert!(!r.sd.card.multi_read);
        // Up to the end, then an out-of-range error token.
        r.cmd(18, 0, 2047);
        assert_eq!(r.resp8b(), 0);
        assert_eq!(r.rd(DATA), 0xFE);
        for _ in 0..SECTOR + 2 {
            r.rd(DATA);
        }
        assert_eq!(r.rd(DATA), ERROR_TOKEN_RANGE);
        assert_eq!(r.rd(DATA), 0xFF);
    }

    #[test]
    fn multi_block_write_with_stop_token() {
        let mut r = ready();
        r.cmd(25, 0, 10);
        assert_eq!(r.resp8b(), 0);
        for k in 0..3u8 {
            r.wr(DATA, TOKEN_MULTI);
            for _ in 0..SECTOR {
                r.wr(DATA, 0xC0 + k);
            }
            r.wr(DATA, 0xFF);
            r.wr(DATA, 0xFF);
            assert_eq!(r.rd(DATA), DATA_ACCEPTED);
            let mut n = 0;
            while r.rd(DATA) != 0xFF {
                n += 1;
            }
            assert_eq!(n, WRITE_BUSY_BYTES);
        }
        r.wr(DATA, TOKEN_STOP);
        r.rd(DATA);
        let mut n = 0;
        while r.rd(DATA) != 0xFF {
            n += 1;
        }
        assert_eq!(n, WRITE_BUSY_BYTES);
        let img = r.storage.units[0].as_ref().unwrap();
        for k in 0..3 {
            assert!(img[(10 + k) * SECTOR..(11 + k) * SECTOR]
                .iter()
                .all(|&b| b == 0xC0 + k as u8));
        }
        assert_eq!(img[13 * SECTOR..14 * SECTOR], pattern(13)[..]);
        // ACMD22: blocks written.
        r.cmd(55, 0, 0);
        r.resp8b();
        r.cmd(22, 0, 0);
        assert_eq!(r.resp8b(), 0);
        assert_eq!(r.rd(DATA), 0xFE);
        let n: Vec<u8> = (0..4).map(|_| r.rd(DATA)).collect();
        assert_eq!(n, [0, 0, 0, 3]);
    }

    #[test]
    fn h9_csd_capacity_and_cid() {
        // 64 MB.
        let mut r = Rig::with_card(131_072);
        assert!(matches!(r.init(), Init::Ok { .. }));
        let (sectors, cid, csd) = r.drive_size().expect("CID and CSD");
        assert_eq!(sectors, 131_072);
        assert_eq!(csd[0] >> 6, 1, "CSD version 2.0");
        assert_eq!(csd[15], crc7(&csd[..15]) << 1 | 1);
        assert_eq!(cid[15], crc7(&cid[..15]) << 1 | 1);
        assert_eq!(&cid[3..8], b"MIMIC");
        // CMD10 left its CRC unread; the next command still works.
        let mut buf = [0u8; SECTOR];
        assert_eq!(r.read_sector(2, &mut buf), Ok(()));
        assert_eq!(buf[..], pattern(2)[..]);
    }

    #[test]
    fn csd_c_size() {
        let c = |s: u64| {
            let b = csd(s);
            u32::from(b[7] & 0x3F) << 16 | u32::from(b[8]) << 8 | u32::from(b[9])
        };
        assert_eq!(c(1024), 0);
        assert_eq!(c(100), 0, "below 512 KB: one unit");
        assert_eq!(c(131_072), 127);
        // 32 GB: the largest the firmware's 16-bit C_SIZE holds.
        assert_eq!(c(65_536 * 1024), 0xFFFF);
        assert_eq!(c(u64::MAX), 0x3F_FFFF);
    }

    #[test]
    fn h11_insert_remove_and_exchange() {
        let mut r = Rig::new();
        assert_eq!(r.sense() & 1, 0);
        // Insert: detect at once, the card needs initialising.
        r.storage.set(unit::SD, Some(pattern_image(2048)));
        assert_eq!(r.sense() & 1, 1);
        assert!(matches!(r.init(), Init::Ok { .. }));
        // Remove: detect clear at once, the line idles high.
        r.storage.set(unit::SD, None);
        assert_eq!(r.sense() & 1, 0);
        assert_eq!(r.rd(DATA), 0xFF);
        // Back in, ready again after a new init.
        r.storage.set(unit::SD, Some(pattern_image(2048)));
        assert_eq!(r.sense() & 1, 1);
        assert!(matches!(r.init(), Init::Ok { .. }));
        // Exchanged while in: clear for a while, so a poll sees it.
        r.now = 1_000;
        r.storage.set(unit::SD, Some(vec![0xEE; 1024 * SECTOR]));
        assert_eq!(r.sense() & 1, 0);
        r.now += 10_000_000; // the next 100 ms poll
        assert_eq!(r.sense() & 1, 0);
        r.now = 1_000 + SWAP_HOLD;
        assert_eq!(r.sense() & 1, 1);
        // The new card is fresh: no answer before CMD0.
        r.cmd(17, 0, 0);
        assert_eq!(r.resp8b(), 0xFF);
        assert!(matches!(r.init(), Init::Ok { .. }));
        let mut buf = [0u8; SECTOR];
        assert_eq!(r.read_sector(0, &mut buf), Ok(()));
        assert!(buf.iter().all(|&b| b == 0xEE));
    }

    #[test]
    fn peek_has_no_side_effects() {
        let mut r = ready();
        r.cmd(17, 0, 1);
        assert_eq!(r.sd.peek(DATA), 0x00);
        assert_eq!(r.sd.peek(DATA + 2), 0x00);
        let before = r.sd.clone();
        for a in SD_BASE..=SD_LAST {
            r.sd.peek(a);
        }
        assert_eq!(r.sd, before);
        assert_eq!(r.rd(DATA), 0x00);
        assert_eq!(r.sd.peek(DATA), 0xFE);
    }

    #[test]
    fn every_address_reads_and_writes() {
        let mut r = ready();
        for a in SD_BASE..=SD_LAST {
            r.rd(a);
            r.wr(a, a as u8);
            r.rd(a | 0x1000_0000);
        }
        for a in SD_BASE..=SD_LAST {
            r.wr(a, 0xFF);
        }
        r.wr(CTRL, 0);
        // The card survives the noise.
        assert!(matches!(r.init(), Init::Ok { .. }));
        assert_eq!(r.sd.next_event(), u64::MAX);
    }

    #[test]
    fn reset_is_power_on() {
        let mut r = ready();
        r.sd.reset();
        assert_eq!(r.sd, SdCard::new());
        assert_eq!(r.rd(SPEED), 0xF4);
        assert_eq!(r.rd(CTRL), 0x06);
    }

    #[test]
    fn savestate_round_trip_mid_transfer() {
        let mut r = ready();
        r.cmd(17, 0, 33);
        assert_eq!(r.resp8b(), 0);
        assert_eq!(r.rd(DATA), 0xFE);
        for _ in 0..100 {
            r.rd(DATA);
        }
        let mut w = StateWriter::new();
        r.sd.save(&mut w);
        let bytes = w.into_bytes();

        let mut loaded = SdCard::new();
        loaded.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(loaded.peek(DATA), r.sd.peek(DATA));
        let rest_a: Vec<u8> = (0..412).map(|_| r.rd(DATA)).collect();
        // The loaded copy goes on from the same byte.
        r.sd = loaded;
        let rest_b: Vec<u8> = (0..412).map(|_| r.rd(DATA)).collect();
        assert_eq!(rest_a, rest_b);
        assert_eq!(rest_b[..], pattern(33)[100..]);

        // Mid-write too.
        let mut r = ready();
        r.cmd(24, 0, 7);
        r.resp8b();
        r.wr(DATA, 0xFE);
        for _ in 0..200 {
            r.wr(DATA, 0x77);
        }
        let mut w = StateWriter::new();
        r.sd.save(&mut w);
        let bytes = w.into_bytes();
        let mut loaded = SdCard::new();
        loaded.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(loaded, r.sd);
    }

    #[test]
    fn savestate_truncated_and_invalid() {
        let mut r = ready();
        r.cmd(17, 0, 1);
        let mut w = StateWriter::new();
        r.sd.save(&mut w);
        let bytes = w.into_bytes();
        for cut in 0..bytes.len() {
            let mut s = SdCard::new();
            assert!(
                s.load(&mut StateReader::new(&bytes[..cut])).is_err(),
                "cut {cut}"
            );
        }
        // A receive state out of range.
        let mut bad = SdCard::new();
        bad.card.rx = Rx::Data;
        let mut w = StateWriter::new();
        bad.save(&mut w);
        let mut bytes = w.into_bytes();
        // From the end: out (4), written (4), two flags, lba (8), data (4).
        let at = bytes.len() - 23;
        assert_eq!(bytes[at], 2);
        bytes[at] = 9;
        let mut s = SdCard::new();
        assert!(s.load(&mut StateReader::new(&bytes)).is_err());
    }
}
