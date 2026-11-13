//! An ATA (IDE) channel with up to two hard disks (device 0 "master" and
//! device 1 "slave") on hard disk images of 512-byte blocks, at the level
//! of the task file registers a host adapter decodes (the A1200's Gayle,
//! `machine-amiga`).
//!
//! **Registers** (ANSI X3.221-1994 *AT Attachment Interface for Disk
//! Drives* (ATA-1), sections 7 and 8, and X3T10/0948D ATA-2 for READ/WRITE
//! MULTIPLE and LBA; from memory of them):
//!
//! | reg | read                 | write                                 |
//! | --- | -------------------- | ------------------------------------- |
//! | 0   | data (16 bits)       | data                                  |
//! | 1   | error                | features                              |
//! | 2   | sector count         | sector count                          |
//! | 3   | sector number (LBA 7–0) | same                               |
//! | 4   | cylinder low (LBA 15–8) | same                               |
//! | 5   | cylinder high (LBA 23–16) | same                             |
//! | 6   | device/head: 6 LBA, 4 DEV, 3–0 head (LBA 27–24) | same       |
//! | 7   | status (clears INTRQ) | command                              |
//! | alt | alternate status     | device control: 2 SRST, 1 nIEN        |
//!
//! Status: 7 BSY, 6 DRDY, 5 DF, 4 DSC, 3 DRQ, 2 CORR, 1 IDX, 0 ERR. Error:
//! 6 UNC, 4 IDNF, 2 ABRT. Both devices take every task file write; the
//! one DEV selects answers reads. With device 1 missing, device 0 answers
//! its status reads with 0 (ATA-1 7.2.13); a channel without any device
//! reads $FF on every register (the open bus of the host's pull-ups).
//!
//! **Commands:** RECALIBRATE ($1x), READ SECTORS ($20/$21), WRITE SECTORS
//! ($30/$31), READ VERIFY ($40/$41), SEEK ($7x), EXECUTE DEVICE DIAGNOSTIC
//! ($90), INITIALIZE DEVICE PARAMETERS ($91), the power commands ($94–$99,
//! $E0–$E6), READ MULTIPLE ($C4), WRITE MULTIPLE ($C5), SET MULTIPLE MODE
//! ($C6), FLUSH CACHE ($E7), IDENTIFY DEVICE ($EC), SET FEATURES ($EF).
//! Every other command ends with ABRT. Addresses are CHS through the
//! current translation (IDENTIFY's default 16 heads and 63 sectors, or
//! what INITIALIZE DEVICE PARAMETERS set) or LBA. Commands complete at
//! once: BSY is never seen, the data is ready (DRQ) as soon as the command
//! is written. INTRQ rises when a data block is ready for reading, when
//! the device wants the next block of a write, and when a command without
//! data or a write ends; nIEN keeps it off the host. Data words are as on
//! the drive's connector: a block's first byte in the low half (D7–D0),
//! IDENTIFY's words as numbers; a host that wires the bus the other way
//! round (the A600/A1200's Gayle) swaps them itself. Blocks past the image's end but within the CHS
//! capacity read as zeros, writes there are dropped. Writes go through to
//! the image's backend (the copy-on-write overlay by default).

use emu_core::media::{DriveStatus, Media, MediaBackend, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

pub const BLOCK: usize = 512;
/// The most blocks READ/WRITE MULTIPLE move per DRQ (IDENTIFY word 47).
pub const MAX_MULTIPLE: usize = 16;

pub mod status {
    pub const BSY: u8 = 0x80;
    pub const DRDY: u8 = 0x40;
    pub const DSC: u8 = 0x10;
    pub const DRQ: u8 = 0x08;
    pub const ERR: u8 = 0x01;
}

pub mod error {
    pub const IDNF: u8 = 0x10;
    pub const ABRT: u8 = 0x04;
}

const DEFAULT_HEADS: u16 = 16;
const DEFAULT_SECTORS: u16 = 63;

/// Turns an inserted image into what the device shows (an Amiga
/// partition image behind a virtual RDB, `media::hdf::with_rdb`).
pub type Prepare = fn(Box<dyn MediaBackend>) -> Result<Box<dyn MediaBackend>, MediaError>;

/// One hard disk on the channel. A clone has no image (images are media,
/// not state; [`Ata::take_media`] moves them).
#[derive(Debug, Default)]
pub struct AtaDisk {
    media: Option<Media>,
    /// Applied to every image inserted.
    pub prepare: Option<Prepare>,
    protected: bool,
    /// Blocks moved since the frontend last looked (the activity LED).
    activity: u32,
    shown: bool,
    /// The current CHS translation (INITIALIZE DEVICE PARAMETERS).
    heads: u16,
    sectors: u16,
    /// Blocks per DRQ of READ/WRITE MULTIPLE; 0 is off.
    multiple: u8,
    /// A read or write failed in the backend.
    failed: bool,
}

impl Clone for AtaDisk {
    fn clone(&self) -> Self {
        Self {
            media: None,
            prepare: self.prepare,
            protected: self.protected,
            activity: self.activity,
            shown: self.shown,
            heads: self.heads,
            sectors: self.sectors,
            multiple: self.multiple,
            failed: self.failed,
        }
    }
}

impl PartialEq for AtaDisk {
    fn eq(&self, o: &Self) -> bool {
        (self.heads, self.sectors, self.multiple, self.protected)
            == (o.heads, o.sectors, o.multiple, o.protected)
    }
}

impl Eq for AtaDisk {}

impl AtaDisk {
    pub fn has_disk(&self) -> bool {
        self.media.is_some()
    }

    /// The image's size in blocks.
    pub fn blocks(&self) -> u64 {
        self.media
            .as_ref()
            .map_or(0, |m| m.backend.len() / BLOCK as u64)
    }

    /// IDENTIFY's default geometry: 16 heads, 63 sectors and the
    /// cylinders the image needs (at most 16383; larger disks only by LBA).
    pub fn default_cylinders(&self) -> u16 {
        let per_cyl = u64::from(DEFAULT_HEADS * DEFAULT_SECTORS);
        self.blocks().div_ceil(per_cyl).clamp(1, 16383) as u16
    }

    fn reset_translation(&mut self) {
        self.heads = DEFAULT_HEADS;
        self.sectors = DEFAULT_SECTORS;
        self.multiple = 0;
    }

    fn read_block(&mut self, lba: u64, out: &mut [u8]) {
        out.fill(0);
        self.activity += 1;
        if lba >= self.blocks() {
            return;
        }
        if let Some(m) = &mut self.media {
            if m.backend.read(lba * BLOCK as u64, out).is_err() {
                self.failed = true;
            }
        }
    }

    fn write_block(&mut self, lba: u64, data: &[u8]) {
        self.activity += 1;
        if self.protected || lba >= self.blocks() {
            return;
        }
        if let Some(m) = &mut self.media {
            if m.backend.write(lba * BLOCK as u64, data).is_err() {
                self.failed = true;
            }
        }
    }

    /// The 256 words of IDENTIFY DEVICE (ATA-1 8.7, ATA-2 for words 47,
    /// 49, 53–61).
    fn identify(&self, serial: &str) -> [u16; 256] {
        let mut w = [0u16; 256];
        let cyl = self.default_cylinders();
        w[0] = 0x0040; // fixed disk
        w[1] = cyl;
        w[3] = DEFAULT_HEADS;
        w[4] = (BLOCK * DEFAULT_SECTORS as usize) as u16; // unformatted bytes a track
        w[5] = BLOCK as u16;
        w[6] = DEFAULT_SECTORS;
        put_string(&mut w[10..20], serial);
        w[20] = 3; // dual-ported buffer with a cache
        w[21] = 16; // buffer in 512-byte units
        put_string(&mut w[23..27], "1.0");
        put_string(&mut w[27..47], "MIMIC HARD DISK");
        w[47] = 0x8000 | MAX_MULTIPLE as u16;
        w[49] = 0x0200; // LBA
        w[51] = 0x0200; // PIO mode 2 timing
        w[53] = 1; // words 54-58 valid
        w[54] = cyl;
        w[55] = self.heads;
        w[56] = self.sectors;
        let current = u32::from(cyl) * u32::from(self.heads) * u32::from(self.sectors);
        w[57] = current as u16;
        w[58] = (current >> 16) as u16;
        if self.multiple != 0 {
            w[59] = 0x0100 | u16::from(self.multiple);
        }
        let lba = self.blocks().min(0x0fff_ffff) as u32;
        w[60] = lba as u16;
        w[61] = (lba >> 16) as u16;
        w
    }
}

/// An ATA string: two characters a word, the first in the high byte,
/// padded with spaces.
fn put_string(words: &mut [u16], s: &str) {
    let mut bytes = s.bytes().chain(std::iter::repeat(b' '));
    for w in words {
        let hi = bytes.next().unwrap_or(b' ');
        let lo = bytes.next().unwrap_or(b' ');
        *w = u16::from(hi) << 8 | u16::from(lo);
    }
}

impl MediaSlot for AtaDisk {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        if let Some(prepare) = self.prepare {
            let backend = std::mem::replace(&mut media.backend, Box::new(Empty));
            media.backend = prepare(backend)?;
        }
        let len = media.backend.len();
        if len < BLOCK as u64 || !len.is_multiple_of(BLOCK as u64) {
            return Err(MediaError::Format(format!(
                "a hard disk image is whole 512-byte blocks ({len} bytes)"
            )));
        }
        let _ = self.eject();
        self.media = Some(media);
        self.reset_translation();
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    fn dirty(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.backend.dirty())
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.shown,
            error: self.failed,
            track: None,
        }
    }
}

/// A stand-in while an image is being prepared.
struct Empty;

impl MediaBackend for Empty {
    fn len(&self) -> u64 {
        0
    }
    fn read(&mut self, _: u64, _: &mut [u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn write(&mut self, _: u64, _: &[u8]) -> Result<(), MediaError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

/// A transfer in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transfer {
    None,
    /// IDENTIFY's block.
    Identify,
    /// Blocks to the host; `left` counts the command's remaining blocks
    /// including those in the buffer.
    Read {
        left: u32,
    },
    /// Blocks from the host.
    Write {
        left: u32,
    },
}

/// The channel: the task file both devices share, the two devices and the
/// transfer of the selected one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ata {
    pub disks: [AtaDisk; 2],
    features: u8,
    error: u8,
    count: u8,
    sector: u8,
    cyl_low: u8,
    cyl_high: u8,
    device: u8,
    status: u8,
    control: u8,
    intrq: bool,
    transfer: Transfer,
    /// The data buffer (up to MAX_MULTIPLE blocks), its fill and position
    /// in bytes.
    buffer: Vec<u8>,
    len: usize,
    pos: usize,
    /// The device a transfer belongs to.
    owner: usize,
    /// READ/WRITE MULTIPLE: the multiple count of blocks a DRQ.
    multi: bool,
    /// The block the buffer's next read or write continues at.
    next_lba: u64,
}

impl Default for Ata {
    fn default() -> Self {
        Self::new()
    }
}

impl Ata {
    pub fn new() -> Self {
        let mut a = Self {
            disks: Default::default(),
            features: 0,
            error: 0,
            count: 0,
            sector: 0,
            cyl_low: 0,
            cyl_high: 0,
            device: 0,
            status: 0,
            control: 0,
            intrq: false,
            transfer: Transfer::None,
            buffer: vec![0; BLOCK * MAX_MULTIPLE],
            len: 0,
            pos: 0,
            owner: 0,
            multi: false,
            next_lba: 0,
        };
        a.reset();
        a
    }

    /// Power-on or a hardware reset: both devices show their signature.
    pub fn reset(&mut self) {
        for d in &mut self.disks {
            d.reset_translation();
        }
        self.control = 0;
        self.soft_reset();
    }

    fn soft_reset(&mut self) {
        self.error = 1; // diagnostic passed
        self.count = 1;
        self.sector = 1;
        self.cyl_low = 0;
        self.cyl_high = 0;
        self.device = 0;
        self.status = status::DRDY | status::DSC;
        self.transfer = Transfer::None;
        self.intrq = false;
        self.len = 0;
        self.pos = 0;
    }

    /// Move the images from `old` (a savestate was loaded into a clone).
    pub fn take_media(&mut self, old: &mut Ata) {
        for (new, old) in self.disks.iter_mut().zip(&mut old.disks) {
            new.media = old.media.take();
        }
    }

    /// Any device on the channel.
    pub fn present(&self) -> bool {
        self.disks.iter().any(AtaDisk::has_disk)
    }

    fn selected(&self) -> usize {
        usize::from(self.device >> 4 & 1)
    }

    /// The INTRQ line as the host sees it (nIEN off).
    pub fn irq(&self) -> bool {
        self.intrq && self.control & 2 == 0
    }

    /// Register `reg` (0–7) read; reading the status clears INTRQ.
    pub fn read(&mut self, reg: u8) -> u8 {
        let v = self.peek(reg);
        if reg & 7 == 7 && self.disks[self.selected()].has_disk() {
            self.intrq = false;
        }
        v
    }

    /// Register `reg` without side effects (the data register gives the
    /// next word's high byte).
    pub fn peek(&self, reg: u8) -> u8 {
        if !self.present() {
            return 0xff;
        }
        let here = self.disks[self.selected()].has_disk();
        match reg & 7 {
            0 => (self.peek_data() >> 8) as u8,
            1 if here => self.error,
            2 => self.count,
            3 => self.sector,
            4 => self.cyl_low,
            5 => self.cyl_high,
            6 => self.device | 0xa0,
            7 if here => self.status,
            _ => 0,
        }
    }

    /// The alternate status (does not clear INTRQ).
    pub fn alt_status(&self) -> u8 {
        if !self.present() {
            0xff
        } else if self.disks[self.selected()].has_disk() {
            self.status
        } else {
            0
        }
    }

    /// The next data word without taking it.
    pub fn peek_data(&self) -> u16 {
        if self.status & status::DRQ != 0 && self.pos + 1 < self.len {
            u16::from_le_bytes([self.buffer[self.pos], self.buffer[self.pos + 1]])
        } else {
            0
        }
    }

    /// A data register read: the next word of the block to the host (its
    /// first byte in the low half).
    pub fn read_data(&mut self) -> u16 {
        if !self.present() {
            return 0xffff;
        }
        if self.status & status::DRQ == 0
            || !matches!(self.transfer, Transfer::Read { .. } | Transfer::Identify)
        {
            return 0;
        }
        let v = self.peek_data();
        self.pos += 2;
        if self.pos >= self.len {
            self.block_read();
        }
        v
    }

    /// A data register write: the next word of the block from the host.
    pub fn write_data(&mut self, v: u16) {
        if self.status & status::DRQ == 0 || !matches!(self.transfer, Transfer::Write { .. }) {
            return;
        }
        if self.pos + 1 < self.len {
            self.buffer[self.pos..self.pos + 2].copy_from_slice(&v.to_le_bytes());
        }
        self.pos += 2;
        if self.pos >= self.len {
            self.block_written();
        }
    }

    /// A task file register write (1–7; 0 is the data register).
    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 7 {
            0 => self.write_data(u16::from(v) << 8 | u16::from(v)),
            1 => self.features = v,
            2 => self.count = v,
            3 => self.sector = v,
            4 => self.cyl_low = v,
            5 => self.cyl_high = v,
            6 => self.device = v & 0x5f,
            _ => self.command(v),
        }
    }

    /// The device control register: SRST (bit 2) resets both devices when
    /// it goes off again, nIEN (bit 1) masks INTRQ.
    pub fn write_control(&mut self, v: u8) {
        let was = self.control & 4 != 0;
        self.control = v & 6;
        if v & 4 != 0 {
            self.status = status::BSY;
            self.transfer = Transfer::None;
        } else if was {
            self.soft_reset();
            for d in &mut self.disks {
                d.reset_translation();
            }
        }
    }

    /// The block address of the task file: LBA, or CHS through the
    /// device's translation. None for a sector number outside it.
    fn address(&self, dev: usize) -> Option<u64> {
        let head = u64::from(self.device & 15);
        let cyl = u64::from(self.cyl_high) << 8 | u64::from(self.cyl_low);
        if self.device & 0x40 != 0 {
            return Some(head << 24 | cyl << 8 | u64::from(self.sector));
        }
        let d = &self.disks[dev];
        let (heads, sectors) = (u64::from(d.heads), u64::from(d.sectors));
        let s = u64::from(self.sector);
        if s == 0 || s > sectors || head >= heads {
            return None;
        }
        Some((cyl * heads + head) * sectors + s - 1)
    }

    /// The highest block address + 1 the device accepts.
    fn capacity(&self, dev: usize) -> u64 {
        let d = &self.disks[dev];
        let chs = u64::from(d.default_cylinders()) * u64::from(DEFAULT_HEADS * DEFAULT_SECTORS);
        chs.max(d.blocks())
    }

    /// Put `lba` into the task file (after a block, the next one's address).
    fn set_address(&mut self, dev: usize, lba: u64) {
        if self.device & 0x40 != 0 {
            self.sector = lba as u8;
            self.cyl_low = (lba >> 8) as u8;
            self.cyl_high = (lba >> 16) as u8;
            self.device = (self.device & 0xf0) | (lba >> 24 & 15) as u8;
            return;
        }
        let d = &self.disks[dev];
        let (heads, sectors) = (u64::from(d.heads).max(1), u64::from(d.sectors).max(1));
        let cyl = lba / (heads * sectors);
        let rest = lba % (heads * sectors);
        self.cyl_low = cyl as u8;
        self.cyl_high = (cyl >> 8) as u8;
        self.device = (self.device & 0xf0) | (rest / sectors) as u8;
        self.sector = (rest % sectors + 1) as u8;
    }

    fn done(&mut self, irq: bool) {
        self.status = status::DRDY | status::DSC;
        self.transfer = Transfer::None;
        self.len = 0;
        self.pos = 0;
        if irq {
            self.intrq = true;
        }
    }

    fn abort(&mut self, err: u8) {
        self.error = err;
        self.status = status::DRDY | status::DSC | status::ERR;
        self.transfer = Transfer::None;
        self.len = 0;
        self.pos = 0;
        self.intrq = true;
    }

    fn command(&mut self, cmd: u8) {
        let dev = self.selected();
        if !self.disks[dev].has_disk() {
            return;
        }
        self.error = 0;
        self.owner = dev;
        let count = match self.count {
            0 => 256,
            n => u32::from(n),
        };
        match cmd {
            0x10..=0x1f => {
                self.cyl_low = 0;
                self.cyl_high = 0;
                self.done(true);
            }
            0x20 | 0x21 | 0xc4 => {
                if cmd == 0xc4 && self.disks[dev].multiple == 0 {
                    return self.abort(error::ABRT);
                }
                self.transfer = Transfer::Read { left: count };
                self.multi = cmd == 0xc4;
                self.load_read();
            }
            0x30 | 0x31 | 0xc5 => {
                if cmd == 0xc5 && self.disks[dev].multiple == 0 {
                    return self.abort(error::ABRT);
                }
                self.transfer = Transfer::Write { left: count };
                self.multi = cmd == 0xc5;
                self.want_write();
            }
            0x40 | 0x41 => {
                let Some(lba) = self.address(dev) else {
                    return self.abort(error::IDNF | error::ABRT);
                };
                if lba + u64::from(count) > self.capacity(dev) {
                    return self.abort(error::IDNF);
                }
                self.set_address(dev, lba + u64::from(count) - 1);
                self.done(true);
            }
            0x70..=0x7f => match self.address(dev) {
                Some(lba) if lba < self.capacity(dev) => self.done(true),
                _ => self.abort(error::IDNF),
            },
            0x90 => {
                self.soft_reset();
                self.intrq = true;
            }
            0x91 => {
                let d = &mut self.disks[dev];
                d.heads = u16::from(self.device & 15) + 1;
                d.sectors = u16::from(self.count);
                self.done(true);
            }
            0xc6 => {
                let n = self.count;
                if usize::from(n) <= MAX_MULTIPLE && (n == 0 || n.is_power_of_two()) {
                    self.disks[dev].multiple = n;
                    self.done(true);
                } else {
                    self.abort(error::ABRT);
                }
            }
            0x94..=0x99 | 0xe0..=0xe3 | 0xe6 | 0xe7 | 0xef => self.done(true),
            0xe5 => {
                self.count = 0xff; // active or idle
                self.done(true);
            }
            0xec => {
                let words = self.disks[dev].identify(if dev == 0 { "MIMIC0" } else { "MIMIC1" });
                for (i, w) in words.iter().enumerate() {
                    self.buffer[2 * i..2 * i + 2].copy_from_slice(&w.to_le_bytes());
                }
                self.transfer = Transfer::Identify;
                self.len = BLOCK;
                self.pos = 0;
                self.status = status::DRDY | status::DSC | status::DRQ;
                self.intrq = true;
            }
            _ => self.abort(error::ABRT),
        }
    }

    /// Blocks a DRQ moves now: one, or the multiple count.
    fn chunk(&self, left: u32, multi: bool) -> u32 {
        if multi {
            left.min(u32::from(self.disks[self.owner].multiple.max(1)))
        } else {
            1
        }
    }

    /// Fill the buffer with the next blocks of a read and raise DRQ.
    fn load_read(&mut self) {
        let Transfer::Read { left } = self.transfer else {
            return;
        };
        let dev = self.owner;
        let Some(lba) = self.address(dev) else {
            return self.abort(error::IDNF | error::ABRT);
        };
        let n = self.chunk(left, self.multi);
        if lba + u64::from(n) > self.capacity(dev) {
            return self.abort(error::IDNF);
        }
        for i in 0..n as usize {
            let (a, b) = (i * BLOCK, (i + 1) * BLOCK);
            let mut block = [0u8; BLOCK];
            self.disks[dev].read_block(lba + i as u64, &mut block);
            self.buffer[a..b].copy_from_slice(&block);
        }
        self.set_address(dev, lba + u64::from(n) - 1);
        self.next_lba = lba + u64::from(n);
        self.len = n as usize * BLOCK;
        self.pos = 0;
        self.status = status::DRDY | status::DSC | status::DRQ;
        self.intrq = true;
    }

    /// The host took the whole buffer.
    fn block_read(&mut self) {
        match self.transfer {
            Transfer::Read { left } => {
                let n = self.chunk(left, self.multi);
                let left = left - n;
                if left == 0 {
                    self.done(false);
                    return;
                }
                self.transfer = Transfer::Read { left };
                let next = self.next_lba;
                self.set_address(self.owner, next);
                self.load_read();
            }
            _ => self.done(false),
        }
    }

    /// Ask the host for the next blocks of a write.
    fn want_write(&mut self) {
        let Transfer::Write { left } = self.transfer else {
            return;
        };
        let dev = self.owner;
        let Some(lba) = self.address(dev) else {
            return self.abort(error::IDNF | error::ABRT);
        };
        let n = self.chunk(left, self.multi);
        if lba + u64::from(n) > self.capacity(dev) {
            return self.abort(error::IDNF);
        }
        self.next_lba = lba;
        self.len = n as usize * BLOCK;
        self.pos = 0;
        self.status = status::DRDY | status::DSC | status::DRQ;
    }

    /// The host filled the buffer: write it, then want more or end.
    fn block_written(&mut self) {
        let Transfer::Write { left } = self.transfer else {
            return;
        };
        let dev = self.owner;
        let n = self.chunk(left, self.multi);
        let lba = self.next_lba;
        for i in 0..n as usize {
            let block: Vec<u8> = self.buffer[i * BLOCK..(i + 1) * BLOCK].to_vec();
            self.disks[dev].write_block(lba + i as u64, &block);
        }
        let left = left - n;
        self.set_address(dev, lba + u64::from(n) - 1);
        if left == 0 {
            self.done(true);
            return;
        }
        self.transfer = Transfer::Write { left };
        self.set_address(dev, lba + u64::from(n));
        self.want_write();
        self.intrq = true;
    }

    /// Once a frame: the activity LEDs show what moved since the last.
    pub fn end_frame(&mut self) {
        for d in &mut self.disks {
            d.shown = d.activity > 0;
            d.activity = 0;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        for d in &self.disks {
            w.u16(d.heads);
            w.u16(d.sectors);
            w.u8(d.multiple);
        }
        for v in [
            self.features,
            self.error,
            self.count,
            self.sector,
            self.cyl_low,
            self.cyl_high,
            self.device,
            self.status,
            self.control,
        ] {
            w.u8(v);
        }
        w.bool(self.intrq);
        let (kind, left) = match self.transfer {
            Transfer::None => (0, 0),
            Transfer::Identify => (1, 0),
            Transfer::Read { left } => (2, left),
            Transfer::Write { left } => (3, left),
        };
        w.u8(kind);
        w.u32(left);
        w.bool(self.multi);
        w.u64(self.next_lba);
        w.u8(self.owner as u8);
        w.u32(self.len as u32);
        w.u32(self.pos as u32);
        w.raw(&self.buffer[..self.len]);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        for d in &mut self.disks {
            d.heads = r.u16()?.clamp(1, 16);
            d.sectors = r.u16()?.clamp(1, 255);
            d.multiple = r.u8()?.min(MAX_MULTIPLE as u8);
        }
        self.features = r.u8()?;
        self.error = r.u8()?;
        self.count = r.u8()?;
        self.sector = r.u8()?;
        self.cyl_low = r.u8()?;
        self.cyl_high = r.u8()?;
        self.device = r.u8()? & 0x5f;
        self.status = r.u8()?;
        self.control = r.u8()? & 6;
        self.intrq = r.bool()?;
        let kind = r.u8()?;
        let left = r.u32()?.min(256);
        self.transfer = match kind {
            0 => Transfer::None,
            1 => Transfer::Identify,
            2 => Transfer::Read { left },
            3 => Transfer::Write { left },
            _ => return Err(StateError::Invalid("ATA transfer")),
        };
        self.multi = r.bool()?;
        self.next_lba = r.u64()?;
        self.owner = usize::from(r.u8()? & 1);
        let len = r.u32()? as usize;
        let pos = r.u32()? as usize;
        if len > self.buffer.len() || pos > len {
            return Err(StateError::Invalid("ATA buffer"));
        }
        self.len = len;
        self.pos = pos;
        let data = r.raw(len)?;
        self.buffer[..len].copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
