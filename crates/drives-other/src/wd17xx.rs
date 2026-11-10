//! Western Digital FD1771 (Model I Expansion Interface) and FD1793 (Model
//! III) floppy disk controllers, at track level: the disks are DMK-style
//! tracks ([`media::dmk`]) that turn under the head at 300 rpm, one byte
//! every 64 µs in single density (FM) and 32 µs in double density (MFM),
//! so commands find their ID fields, data marks and index pulses where the
//! track has them.
//!
//! Registers: 0 command (write) / status (read), 1 track, 2 sector, 3 data.
//! Commands:
//! - Type I: RESTORE `$0x`, SEEK `$1x`, STEP `$2x`/`$3x`, STEP IN
//!   `$4x`/`$5x`, STEP OUT `$6x`/`$7x` (bit 4 update the track register,
//!   bit 3 head load, bit 2 verify, bits 1–0 step rate). Status: 7 not
//!   ready, 6 write protect, 5 head loaded, 4 seek error, 3 CRC error,
//!   2 track 0, 1 index, 0 busy.
//! - Type II: READ SECTOR `$8x`/`$9x`, WRITE SECTOR `$Ax`/`$Bx` (bit 4
//!   multiple sectors; 1793: bit 1 compare side with bit 3; the data mark
//!   written: 1771 bits 1–0 `$FB`/`$FA`/`$F9`/`$F8`, 1793 bit 0 `$FB`/`$F8`).
//! - Type III: READ ADDRESS `$Cx`, READ TRACK `$Ex`, WRITE TRACK `$Fx`.
//!   Status of types II/III: 7 not ready, 6 write protect, 5–6 the record
//!   type (1771: `$FB` 00, `$FA` 01, `$F9` 10, `$F8` 11 in bits 6–5; 1793:
//!   bit 5 = `$F8`), 4 record not found, 3 CRC error, 2 lost data, 1 DRQ,
//!   0 busy.
//! - Type IV: FORCE INTERRUPT `$Dx` (bit 3 interrupt now).
//!
//! WRITE TRACK takes the formatter's bytes from the index pulse to the
//! next: `$F7` writes the two CRC bytes, `$FE` starts an ID field and
//! `$F8–$FB` a data field (both reset the CRC), MFM `$F5` writes a sync
//! `$A1` and `$F6` a `$C2`. A byte not taken in time is Lost Data (reads)
//! or written as `$00` (writes).
//!
//! Sources: Western Digital FD1771 and FD179X data sheets (command set,
//! status bits, write track codes, step rates). The timing model (search,
//! byte stream from the turning track) is this project's own.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::dmk::{decode_sectors, Image, Track, SD_TRACK_BYTES};
use media::mfm::crc16;

pub const ST_BUSY: u8 = 0x01;
pub const ST_DRQ: u8 = 0x02;
pub const ST_INDEX: u8 = 0x02;
pub const ST_LOST: u8 = 0x04;
pub const ST_TRACK0: u8 = 0x04;
pub const ST_CRC: u8 = 0x08;
pub const ST_NOT_FOUND: u8 = 0x10;
pub const ST_SEEK: u8 = 0x10;
pub const ST_HEAD: u8 = 0x20;
pub const ST_WRITE_PROTECT: u8 = 0x40;
pub const ST_NOT_READY: u8 = 0x80;

/// The highest cylinder a drive's head reaches.
const MAX_CYLINDER: u8 = 80;
/// Revolutions a type II/III command looks for its ID field.
const SEARCH_REVOLUTIONS: u64 = 2;
/// The index hole passes the sensor for about 2 % of a turn (4 ms).
const INDEX_PERCENT: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Fd1771,
    Fd1793,
}

/// A drive: its disk and where its head stands. The medium and the
/// bytes it was read from stay here for writing back (see the machine).
#[derive(Default)]
pub struct Drive {
    pub image: Option<Image>,
    pub cylinder: u8,
    pub write_protected: bool,
    pub dirty: bool,
    pub media: Option<emu_core::media::Media>,
    pub original: Vec<u8>,
    /// Images are TI-99/4A sector dumps ([`Image::parse_ti_dsk`]).
    pub ti_dsk: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// A type I command ends at `at`.
    Stepping {
        at: u64,
    },
    /// Bytes flow to the CPU: the next one at `next`.
    Reading {
        bytes: Vec<u8>,
        pos: usize,
        next: u64,
        /// Status bits set when the transfer ends (CRC, record type).
        end: u8,
        /// READ SECTOR with the multiple flag: go on with the next sector.
        multi: bool,
    },
    /// Bytes flow from the CPU into the track at `at` (the data mark).
    Writing {
        track: usize,
        dam_pos: usize,
        dam: u8,
        size: usize,
        pos: usize,
        next: u64,
        buf: Vec<u8>,
        multi: bool,
    },
    /// WRITE TRACK: bytes from the CPU until `end` (the next index pulse).
    Formatting {
        track: usize,
        buf: Vec<u8>,
        next: u64,
        end: u64,
    },
    /// A command fails or ends at `at` with `status` (e.g. not found).
    Ending {
        at: u64,
        status: u8,
    },
}

#[derive(Clone, Debug)]
pub struct Fdc {
    pub chip: Chip,
    /// CPU clocks per second (timing base).
    clock_hz: u64,
    pub status: u8,
    pub track: u8,
    pub sector: u8,
    pub data: u8,
    command: u8,
    /// The last command was type I (status shows index, track 0, head).
    type_one: bool,
    /// Step direction of the last step (true = in, towards the hub).
    step_in: bool,
    pub intrq: bool,
    drq: bool,
    /// Double density (1793 only; the machine selects it).
    pub mfm: bool,
    /// Selected drive and side.
    pub drive: Option<usize>,
    pub side: u8,
    /// The selected drive's motor runs and its door is closed.
    pub ready: bool,
    phase: Phase,
}

impl Fdc {
    pub fn new(chip: Chip, clock_hz: u32) -> Self {
        Self {
            chip,
            clock_hz: u64::from(clock_hz),
            status: 0,
            track: 0,
            sector: 1,
            data: 0,
            command: 0,
            type_one: true,
            step_in: true,
            intrq: false,
            drq: false,
            mfm: false,
            drive: None,
            side: 0,
            ready: false,
            phase: Phase::Idle,
        }
    }

    pub fn reset(&mut self) {
        let (chip, clock) = (self.chip, self.clock_hz as u32);
        *self = Self::new(chip, clock);
    }

    /// CPU clocks per byte on the disk.
    fn byte_clocks(&self) -> u64 {
        let us = if self.mfm { 32 } else { 64 };
        (self.clock_hz * us / 1_000_000).max(1)
    }

    fn clocks_ms(&self, ms: u64) -> u64 {
        self.clock_hz * ms / 1000
    }

    /// Index of the selected drive's current track in its image, if there
    /// is a disk.
    fn track_index(&self, drives: &[Drive]) -> Option<usize> {
        let d = drives.get(self.drive?)?;
        let img = d.image.as_ref()?;
        let sides = usize::from(img.sides.max(1));
        let side = usize::from(self.side).min(sides - 1);
        let i = usize::from(d.cylinder) * sides + side;
        (i < img.tracks.len()).then_some(i)
    }

    fn current_track<'a>(&self, drives: &'a [Drive]) -> Option<&'a Track> {
        let i = self.track_index(drives)?;
        drives[self.drive?].image.as_ref()?.tracks.get(i)
    }

    /// Byte position of the head on a track of `len` bytes at clock `now`.
    fn position(&self, now: u64, len: usize) -> usize {
        ((now / self.byte_clocks()) % len.max(1) as u64) as usize
    }

    /// Clocks from `now` until byte `pos` of a `len`-byte track is under
    /// the head.
    fn until(&self, now: u64, pos: usize, len: usize) -> u64 {
        let here = self.position(now, len);
        let ahead = (pos + len - here) % len;
        ahead as u64 * self.byte_clocks()
    }

    fn revolution(&self, len: usize) -> u64 {
        len as u64 * self.byte_clocks()
    }

    fn index_pulse(&self, now: u64, drives: &[Drive]) -> bool {
        if !self.ready {
            return false;
        }
        let len = self
            .current_track(drives)
            .map_or(SD_TRACK_BYTES, |t| t.data.len());
        let pos = self.position(now, len);
        pos * 100 < len * INDEX_PERCENT as usize
    }

    fn write_protected(&self, drives: &[Drive]) -> bool {
        self.drive
            .and_then(|d| drives.get(d))
            .is_some_and(|d| d.write_protected || d.image.as_ref().is_some_and(|i| i.protected))
    }

    /// The status register as the CPU reads it (also clears INTRQ).
    pub fn read_status(&mut self, now: u64, drives: &[Drive]) -> u8 {
        self.intrq = false;
        self.peek_status(now, drives)
    }

    pub fn peek_status(&self, now: u64, drives: &[Drive]) -> u8 {
        let mut s = self.status;
        if self.ready {
            s &= !ST_NOT_READY;
        } else {
            s |= ST_NOT_READY;
        }
        if self.type_one {
            s &= !(ST_INDEX | ST_TRACK0 | ST_WRITE_PROTECT);
            if self.index_pulse(now, drives) {
                s |= ST_INDEX;
            }
            if self
                .drive
                .and_then(|d| drives.get(d))
                .is_some_and(|d| d.cylinder == 0)
            {
                s |= ST_TRACK0;
            }
            if self.write_protected(drives) {
                s |= ST_WRITE_PROTECT;
            }
        } else if self.drq {
            s |= ST_DRQ;
        } else {
            s &= !ST_DRQ;
        }
        s
    }

    /// The data register as the CPU reads it: takes the byte (DRQ off).
    pub fn read_data(&mut self) -> u8 {
        self.drq = false;
        self.data
    }

    pub fn write_data(&mut self, v: u8) {
        self.data = v;
        self.drq = false;
    }

    pub fn busy(&self) -> bool {
        self.status & ST_BUSY != 0
    }

    pub fn drq(&self) -> bool {
        self.drq
    }

    fn finish(&mut self, status: u8) {
        self.status = status & !ST_BUSY;
        self.drq = false;
        self.intrq = true;
        self.phase = Phase::Idle;
    }

    /// A write to the command register at clock `now`.
    pub fn command(&mut self, v: u8, now: u64, drives: &mut [Drive]) {
        self.intrq = false;
        if v & 0xf0 == 0xd0 {
            // FORCE INTERRUPT: stops any command; bit 3 interrupts now.
            let was_busy = self.busy();
            self.phase = Phase::Idle;
            self.drq = false;
            self.status &= !ST_BUSY;
            if !was_busy {
                self.type_one = true;
                self.status &= !(ST_SEEK | ST_CRC);
            }
            if v & 0x08 != 0 {
                self.intrq = true;
            }
            return;
        }
        if self.busy() {
            return;
        }
        self.command = v;
        self.drq = false;
        self.status = ST_BUSY;
        match v >> 4 {
            0x0..=0x7 => self.type_one_command(v, now, drives),
            0x8..=0xb => {
                self.type_one = false;
                if !self.ready {
                    self.finish(ST_NOT_READY);
                    return;
                }
                if v & 0x20 != 0 && self.write_protected(drives) {
                    self.finish(ST_WRITE_PROTECT);
                    return;
                }
                self.find_sector(now, drives);
            }
            0xc => {
                self.type_one = false;
                if !self.ready {
                    self.finish(ST_NOT_READY);
                    return;
                }
                self.read_address(now, drives);
            }
            0xe => {
                self.type_one = false;
                if !self.ready {
                    self.finish(ST_NOT_READY);
                    return;
                }
                self.read_track(now, drives);
            }
            _ => {
                self.type_one = false;
                if !self.ready {
                    self.finish(ST_NOT_READY);
                    return;
                }
                if self.write_protected(drives) {
                    self.finish(ST_WRITE_PROTECT);
                    return;
                }
                self.write_track(now, drives);
            }
        }
    }

    fn step_ms(&self, rate: u8) -> u64 {
        match (self.chip, rate & 3) {
            (Chip::Fd1771, 0) | (Chip::Fd1771, 1) => 6,
            (Chip::Fd1771, 2) => 10,
            (Chip::Fd1771, _) => 20,
            (Chip::Fd1793, 0) => 6,
            (Chip::Fd1793, 1) => 12,
            (Chip::Fd1793, 2) => 20,
            (Chip::Fd1793, _) => 30,
        }
    }

    fn type_one_command(&mut self, v: u8, now: u64, drives: &mut [Drive]) {
        self.type_one = true;
        let rate = self.step_ms(v);
        let update = v & 0x10 != 0;
        let mut steps = 0u64;
        let head = self.drive.and_then(|d| drives.get_mut(d));
        let mut cyl = head.as_ref().map_or(0, |d| d.cylinder);
        match v >> 4 {
            0 => {
                // RESTORE: step out until track 0 (at most 255 steps).
                while cyl > 0 && steps < 255 {
                    cyl -= 1;
                    steps += 1;
                }
                self.track = 0;
                self.step_in = false;
            }
            1 => {
                // SEEK to the data register's track.
                let target = self.data;
                self.step_in = target > self.track;
                while self.track != target {
                    if self.step_in {
                        self.track = self.track.wrapping_add(1);
                        cyl = (cyl + 1).min(MAX_CYLINDER);
                    } else {
                        self.track = self.track.wrapping_sub(1);
                        cyl = cyl.saturating_sub(1);
                    }
                    steps += 1;
                }
            }
            n => {
                if n >= 4 {
                    self.step_in = n < 6;
                }
                steps = 1;
                if self.step_in {
                    cyl = (cyl + 1).min(MAX_CYLINDER);
                    if update {
                        self.track = self.track.wrapping_add(1);
                    }
                } else {
                    cyl = cyl.saturating_sub(1);
                    if update {
                        self.track = self.track.wrapping_sub(1);
                    }
                }
            }
        }
        if let Some(d) = self.drive.and_then(|d| drives.get_mut(d)) {
            d.cylinder = cyl;
        }
        let mut status = 0;
        if v & 0x08 != 0 || v & 0x04 != 0 {
            status |= ST_HEAD;
        }
        let mut at = now + steps * self.clocks_ms(rate) + 50;
        if v & 0x04 != 0 {
            // Verify: an ID field of this track must pass the head.
            at += self.clocks_ms(10);
            let found = self.current_track(drives).is_some_and(|t| {
                decode_sectors(t)
                    .iter()
                    .any(|s| s.track == self.track && self.id_density_ok(t, s.dam_pos))
            });
            if !found || !self.ready {
                status |= ST_SEEK;
            }
        }
        self.status = ST_BUSY | status;
        self.phase = Phase::Stepping { at };
    }

    /// An ID field's density matches the controller's.
    fn id_density_ok(&self, track: &Track, dam_pos: usize) -> bool {
        // The IDAM before this data mark tells the encoding.
        let mfm = track
            .idam
            .iter()
            .filter(|p| usize::from(*p & 0x3fff) < dam_pos)
            .max_by_key(|p| *p & 0x3fff)
            .is_some_and(|p| p & 0x8000 != 0);
        mfm == self.mfm
    }

    /// The record type bits of the status for a data mark.
    fn record_type(&self, dam: u8) -> u8 {
        match self.chip {
            Chip::Fd1771 => match dam {
                0xfb => 0x00,
                0xfa => 0x20,
                0xf9 => 0x40,
                _ => 0x60,
            },
            Chip::Fd1793 => {
                if dam == 0xf8 {
                    0x20
                } else {
                    0
                }
            }
        }
    }

    /// READ/WRITE SECTOR: wait for the wanted ID field to come round.
    fn find_sector(&mut self, now: u64, drives: &mut [Drive]) {
        let v = self.command;
        let Some(ti) = self.track_index(drives) else {
            self.phase = Phase::Ending {
                at: now + self.revolution(SD_TRACK_BYTES) * SEARCH_REVOLUTIONS,
                status: ST_NOT_FOUND,
            };
            return;
        };
        let drive = self.drive.unwrap_or(0);
        let track = &drives[drive].image.as_ref().unwrap().tracks[ti];
        let len = track.data.len();
        let compare_side = self.chip == Chip::Fd1793 && v & 0x02 != 0;
        let side = (v >> 3) & 1;
        let found = decode_sectors(track)
            .into_iter()
            .filter(|s| {
                s.track == self.track
                    && s.sector == self.sector
                    && (!compare_side || s.side == side)
                    && self.id_density_ok(track, s.dam_pos)
            })
            .min_by_key(|s| self.until(now, s.dam_pos, len));
        let Some(s) = found else {
            self.phase = Phase::Ending {
                at: now + self.revolution(len) * SEARCH_REVOLUTIONS,
                status: ST_NOT_FOUND,
            };
            return;
        };
        let wait = self.until(now, s.dam_pos, len);
        let multi = v & 0x10 != 0;
        if v & 0x20 == 0 {
            let mut end = self.record_type(s.dam);
            if !s.crc_ok {
                end |= ST_CRC;
            }
            self.phase = Phase::Reading {
                bytes: s.data,
                pos: 0,
                next: now + wait + self.byte_clocks(),
                end,
                multi,
            };
        } else {
            let dam = match self.chip {
                Chip::Fd1771 => [0xfb, 0xfa, 0xf9, 0xf8][usize::from(v & 3)],
                Chip::Fd1793 => {
                    if v & 1 != 0 {
                        0xf8
                    } else {
                        0xfb
                    }
                }
            };
            // DRQ asks for the first byte; it must be there before the
            // data mark is written.
            self.drq = true;
            self.phase = Phase::Writing {
                track: ti,
                dam_pos: s.dam_pos,
                dam,
                size: s.size,
                pos: 0,
                next: now + wait,
                buf: Vec::with_capacity(s.size),
                multi,
            };
        }
    }

    /// READ ADDRESS: the next ID field's six bytes.
    fn read_address(&mut self, now: u64, drives: &[Drive]) {
        let Some(track) = self.current_track(drives) else {
            self.phase = Phase::Ending {
                at: now + self.revolution(SD_TRACK_BYTES) * SEARCH_REVOLUTIONS,
                status: ST_NOT_FOUND,
            };
            return;
        };
        let len = track.data.len();
        let at = |i: usize| track.data[i % len];
        let next = track
            .idam
            .iter()
            .filter(|p| (*p & 0x8000 != 0) == self.mfm)
            .map(|p| usize::from(p & 0x3fff))
            .min_by_key(|&p| self.until(now, p, len));
        let Some(p) = next else {
            self.phase = Phase::Ending {
                at: now + self.revolution(len) * SEARCH_REVOLUTIONS,
                status: ST_NOT_FOUND,
            };
            return;
        };
        let bytes: Vec<u8> = (1..=6).map(|k| at(p + k)).collect();
        let mut crc = 0xffffu16;
        if self.mfm {
            crc = (0..3).fold(crc, |c, _| crc16(c, 0xa1));
        }
        crc = (0..5).fold(crc, |c, k| crc16(c, at(p + k)));
        let ok = u16::from_be_bytes([bytes[4], bytes[5]]) == crc;
        // The sector register takes the ID's track number.
        self.sector = bytes[0];
        self.phase = Phase::Reading {
            bytes,
            pos: 0,
            next: now + self.until(now, p, len) + self.byte_clocks(),
            end: if ok { 0 } else { ST_CRC },
            multi: false,
        };
    }

    /// READ TRACK: the raw bytes from one index pulse to the next.
    fn read_track(&mut self, now: u64, drives: &[Drive]) {
        let Some(track) = self.current_track(drives) else {
            self.finish(ST_NOT_FOUND);
            return;
        };
        let len = track.data.len();
        self.phase = Phase::Reading {
            bytes: track.data.clone(),
            pos: 0,
            next: now + self.until(now, 0, len) + self.byte_clocks(),
            end: 0,
            multi: false,
        };
    }

    /// WRITE TRACK: starts at the next index pulse.
    fn write_track(&mut self, now: u64, drives: &[Drive]) {
        let Some(ti) = self.track_index(drives) else {
            self.finish(ST_NOT_FOUND);
            return;
        };
        let len = self
            .current_track(drives)
            .map_or(SD_TRACK_BYTES, |t| t.data.len());
        let start = now + self.until(now, 0, len);
        self.drq = true;
        self.phase = Phase::Formatting {
            track: ti,
            buf: Vec::with_capacity(len),
            next: start,
            end: start + self.revolution(len),
        };
    }

    /// Run the controller up to clock `now`.
    pub fn tick(&mut self, now: u64, drives: &mut [Drive]) {
        let bc = self.byte_clocks();
        loop {
            match &mut self.phase {
                Phase::Idle => return,
                Phase::Stepping { at } => {
                    if now < *at {
                        return;
                    }
                    let status = self.status & !ST_BUSY;
                    self.finish(status);
                }
                Phase::Ending { at, status } => {
                    if now < *at {
                        return;
                    }
                    let s = *status;
                    self.finish(s);
                }
                Phase::Reading {
                    bytes,
                    pos,
                    next,
                    end,
                    multi,
                } => {
                    if now < *next {
                        return;
                    }
                    if *pos >= bytes.len() {
                        let (end, multi) = (*end, *multi);
                        if multi && self.ready {
                            self.sector = self.sector.wrapping_add(1);
                            self.status = ST_BUSY | (self.status & ST_LOST);
                            self.find_sector(now, drives);
                            if matches!(self.phase, Phase::Ending { .. }) {
                                // The last sector of a multiple read: done.
                                let lost = self.status & ST_LOST;
                                self.finish(lost);
                            }
                            continue;
                        }
                        let lost = self.status & ST_LOST;
                        self.finish(end | lost);
                        continue;
                    }
                    if self.drq {
                        // The previous byte was not taken.
                        self.status |= ST_LOST;
                    }
                    self.data = bytes[*pos];
                    *pos += 1;
                    self.drq = true;
                    *next += bc;
                }
                Phase::Writing {
                    track,
                    dam_pos,
                    dam,
                    size,
                    pos,
                    next,
                    buf,
                    multi,
                } => {
                    if now < *next {
                        return;
                    }
                    if *pos == 0 && self.drq {
                        // No first byte by the data mark: nothing written.
                        self.finish(ST_LOST);
                        continue;
                    }
                    if *pos < *size {
                        let b = if self.drq {
                            self.status |= ST_LOST;
                            0
                        } else {
                            self.data
                        };
                        buf.push(b);
                        *pos += 1;
                        *next += bc;
                        if *pos < *size {
                            self.drq = true;
                        }
                        continue;
                    }
                    // The field is complete: put it on the track.
                    let (ti, dp, d, data, multi) = (*track, *dam_pos, *dam, buf.clone(), *multi);
                    let mfm = self.mfm;
                    if let Some(drive) = self.drive.and_then(|i| drives.get_mut(i)) {
                        if let Some(t) = drive.image.as_mut().and_then(|i| i.tracks.get_mut(ti)) {
                            write_field(t, dp, d, &data, mfm);
                            drive.dirty = true;
                        }
                    }
                    if multi {
                        self.sector = self.sector.wrapping_add(1);
                        self.status = ST_BUSY | (self.status & ST_LOST);
                        self.find_sector(now, drives);
                        if matches!(self.phase, Phase::Ending { .. }) {
                            let lost = self.status & ST_LOST;
                            self.finish(lost);
                        }
                        continue;
                    }
                    let lost = self.status & ST_LOST;
                    self.finish(lost);
                }
                Phase::Formatting {
                    track,
                    buf,
                    next,
                    end,
                } => {
                    if now < *next {
                        return;
                    }
                    if *next >= *end {
                        let (ti, raw) = (*track, std::mem::take(buf));
                        let mfm = self.mfm;
                        if let Some(drive) = self.drive.and_then(|i| drives.get_mut(i)) {
                            if let Some(t) = drive.image.as_mut().and_then(|i| i.tracks.get_mut(ti))
                            {
                                *t = format_track(&raw, mfm, t.data.len());
                                drive.dirty = true;
                            }
                        }
                        let lost = self.status & ST_LOST;
                        self.finish(lost);
                        continue;
                    }
                    let b = if self.drq {
                        self.status |= ST_LOST;
                        0
                    } else {
                        self.data
                    };
                    buf.push(b);
                    *next += bc;
                    self.drq = true;
                }
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.status);
        w.u8(self.track);
        w.u8(self.sector);
        w.u8(self.data);
        w.u8(self.command);
        w.bool(self.type_one);
        w.bool(self.step_in);
        w.bool(self.intrq);
        w.bool(self.drq);
        w.bool(self.mfm);
        w.u8(self.drive.map_or(0xff, |d| d as u8));
        w.u8(self.side);
        w.bool(self.ready);
        match &self.phase {
            Phase::Idle => w.u8(0),
            Phase::Stepping { at } => {
                w.u8(1);
                w.u64(*at);
            }
            Phase::Reading {
                bytes,
                pos,
                next,
                end,
                multi,
            } => {
                w.u8(2);
                w.bytes(bytes);
                w.u32(*pos as u32);
                w.u64(*next);
                w.u8(*end);
                w.bool(*multi);
            }
            Phase::Writing {
                track,
                dam_pos,
                dam,
                size,
                pos,
                next,
                buf,
                multi,
            } => {
                w.u8(3);
                w.u32(*track as u32);
                w.u32(*dam_pos as u32);
                w.u8(*dam);
                w.u32(*size as u32);
                w.u32(*pos as u32);
                w.u64(*next);
                w.bytes(buf);
                w.bool(*multi);
            }
            Phase::Formatting {
                track,
                buf,
                next,
                end,
            } => {
                w.u8(4);
                w.u32(*track as u32);
                w.bytes(buf);
                w.u64(*next);
                w.u64(*end);
            }
            Phase::Ending { at, status } => {
                w.u8(5);
                w.u64(*at);
                w.u8(*status);
            }
        }
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        self.status = r.u8()?;
        self.track = r.u8()?;
        self.sector = r.u8()?;
        self.data = r.u8()?;
        self.command = r.u8()?;
        self.type_one = r.bool()?;
        self.step_in = r.bool()?;
        self.intrq = r.bool()?;
        self.drq = r.bool()?;
        self.mfm = r.bool()?;
        self.drive = match r.u8()? {
            0xff => None,
            d if d < 4 => Some(usize::from(d)),
            _ => return Err(StateError::Invalid("FDC drive")),
        };
        self.side = r.u8()? & 1;
        self.ready = r.bool()?;
        let big = |n: u32| -> Result<usize, StateError> {
            if n > 0x10000 {
                Err(StateError::Invalid("FDC buffer"))
            } else {
                Ok(n as usize)
            }
        };
        self.phase = match r.u8()? {
            0 => Phase::Idle,
            1 => Phase::Stepping { at: r.u64()? },
            2 => Phase::Reading {
                bytes: r.bytes()?.to_vec(),
                pos: big(r.u32()?)?,
                next: r.u64()?,
                end: r.u8()?,
                multi: r.bool()?,
            },
            3 => Phase::Writing {
                track: big(r.u32()?)?,
                dam_pos: big(r.u32()?)?,
                dam: r.u8()?,
                size: big(r.u32()?)?,
                pos: big(r.u32()?)?,
                next: r.u64()?,
                buf: r.bytes()?.to_vec(),
                multi: r.bool()?,
            },
            4 => Phase::Formatting {
                track: big(r.u32()?)?,
                buf: r.bytes()?.to_vec(),
                next: r.u64()?,
                end: r.u64()?,
            },
            5 => Phase::Ending {
                at: r.u64()?,
                status: r.u8()?,
            },
            _ => return Err(StateError::Invalid("FDC phase")),
        };
        Ok(())
    }
}

/// Write a data field (mark, bytes, CRC) at `dam_pos` of a track.
fn write_field(t: &mut Track, dam_pos: usize, dam: u8, data: &[u8], mfm: bool) {
    let n = t.data.len();
    let mut crc = 0xffffu16;
    if mfm {
        crc = (0..3).fold(crc, |c, _| crc16(c, 0xa1));
    }
    crc = crc16(crc, dam);
    t.data[dam_pos % n] = dam;
    for (k, &b) in data.iter().enumerate() {
        t.data[(dam_pos + 1 + k) % n] = b;
        crc = crc16(crc, b);
    }
    let [hi, lo] = crc.to_be_bytes();
    t.data[(dam_pos + 1 + data.len()) % n] = hi;
    t.data[(dam_pos + 2 + data.len()) % n] = lo;
}

/// The track a WRITE TRACK leaves: the formatter's bytes with the control
/// codes turned into marks and CRCs, the ID field positions recorded.
pub fn format_track(raw: &[u8], mfm: bool, len: usize) -> Track {
    let mut data = Vec::with_capacity(len.max(raw.len()));
    let mut idam = Vec::new();
    let mut crc = 0xffffu16;
    for &b in raw {
        match b {
            0xf5 if mfm => {
                data.push(0xa1);
                crc = 0xcdb4; // CRC preset after three $A1
            }
            0xf6 if mfm => data.push(0xc2),
            0xf7 => {
                data.extend(crc.to_be_bytes());
            }
            0xfe | 0xf8..=0xfb => {
                if !mfm {
                    crc = 0xffff;
                }
                if b == 0xfe {
                    idam.push(data.len() as u16 | if mfm { 0x8000 } else { 0 });
                }
                data.push(b);
                crc = crc16(crc, b);
            }
            _ => {
                data.push(b);
                crc = crc16(crc, b);
            }
        }
    }
    if data.len() < len {
        data.resize(len, if mfm { 0x4e } else { 0xff });
    }
    Track { data, idam }
}

/// Write a changed image back into its medium.
pub fn write_back(d: &mut Drive) -> Result<(), MediaError> {
    if !std::mem::take(&mut d.dirty) {
        return Ok(());
    }
    let (Some(img), Some(media)) = (&d.image, &mut d.media) else {
        return Ok(());
    };
    let bytes = img.to_bytes(&d.original);
    if media.backend.len() != bytes.len() as u64 {
        media.backend.set_len(bytes.len() as u64)?;
    }
    media.backend.write(0, &bytes)?;
    d.original = bytes;
    Ok(())
}

/// A drive as a media slot: DMK, JV1 or JV3 images (TRS-80), TI-99/4A
/// sector dumps when [`Drive::ti_dsk`] is set.
impl MediaSlot for Drive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = if self.ti_dsk {
            Image::parse_ti_dsk(&bytes)?
        } else {
            Image::parse(&bytes)?
        };
        let _ = self.eject();
        self.image = Some(image);
        self.dirty = false;
        self.media = Some(media);
        self.original = bytes;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let _ = write_back(self);
        self.image = None;
        self.original.clear();
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.write_protected = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        write_back(self)
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: false,
            error: false,
            track: Some(self.cylinder.saturating_add(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use media::dmk::{blank_jv1, find_dam};

    const CLOCK: u32 = 1_774_080;

    fn drives_with_jv1() -> Vec<Drive> {
        let mut bytes = blank_jv1(35);
        bytes[3 * 256] = 0x42; // track 0, sector 3
        vec![
            Drive {
                image: Some(Image::parse(&bytes).unwrap()),
                ..Default::default()
            },
            Drive::default(),
        ]
    }

    fn fdc() -> Fdc {
        let mut f = Fdc::new(Chip::Fd1771, CLOCK);
        f.drive = Some(0);
        f.ready = true;
        f
    }

    /// Run until the command ends, taking bytes as they come.
    fn run(f: &mut Fdc, drives: &mut [Drive], mut now: u64, sink: &mut Vec<u8>) -> u64 {
        for _ in 0..2_000_000 {
            now += 20;
            f.tick(now, drives);
            if f.drq() && !f.busy() {
                break;
            }
            if f.drq() {
                sink.push(f.read_data());
            }
            if !f.busy() {
                break;
            }
        }
        now
    }

    #[test]
    fn read_sector_finds_its_id_and_streams_256_bytes() {
        let mut d = drives_with_jv1();
        let mut f = fdc();
        f.track = 0;
        f.sector = 3;
        f.command(0x8c, 0, &mut d);
        let mut got = Vec::new();
        run(&mut f, &mut d, 0, &mut got);
        assert_eq!(got.len(), 256);
        assert_eq!(got[0], 0x42);
        assert_eq!(f.read_status(0, &d) & 0x7f, 0, "no error, record type $FB");
    }

    #[test]
    fn missing_sector_is_not_found_and_directory_reads_as_fa() {
        let mut d = drives_with_jv1();
        let mut f = fdc();
        f.sector = 12;
        f.command(0x88, 0, &mut d);
        let mut got = Vec::new();
        run(&mut f, &mut d, 0, &mut got);
        assert!(got.is_empty());
        assert_eq!(f.read_status(0, &d) & ST_NOT_FOUND, ST_NOT_FOUND);
        // Seek to the directory track and read: record type $FA.
        f.data = 17;
        f.command(0x10, 0, &mut d);
        let now = run(&mut f, &mut d, 0, &mut got);
        assert_eq!(d[0].cylinder, 17);
        f.sector = 0;
        f.command(0x88, now, &mut d);
        got.clear();
        run(&mut f, &mut d, now, &mut got);
        assert_eq!(got.len(), 256);
        assert_eq!(f.read_status(0, &d) & 0x60, 0x20);
    }

    #[test]
    fn write_sector_then_read_back() {
        let mut d = drives_with_jv1();
        let mut f = fdc();
        f.sector = 5;
        f.command(0xa8, 0, &mut d);
        let mut now = 0;
        let mut n = 0u32;
        while f.busy() {
            now += 20;
            if f.drq() {
                f.write_data(n as u8);
                n += 1;
            }
            f.tick(now, &mut d);
        }
        assert_eq!(f.read_status(now, &d) & (ST_LOST | ST_NOT_FOUND), 0);
        assert!(d[0].dirty);
        f.command(0x88, now, &mut d);
        let mut got = Vec::new();
        run(&mut f, &mut d, now, &mut got);
        assert_eq!(got[..4], [0, 1, 2, 3]);
        assert_eq!(f.read_status(0, &d) & ST_CRC, 0, "CRC written");
    }

    #[test]
    fn format_track_records_ids_and_crcs() {
        let mut raw = vec![0xff; 16];
        for s in 0..2u8 {
            raw.extend([0; 6]);
            raw.extend([0xfe, 0, 0, s, 1, 0xf7]);
            raw.extend([0xff; 11]);
            raw.extend([0; 6]);
            raw.push(0xfb);
            raw.extend([0xe5; 256]);
            raw.push(0xf7);
            raw.extend([0xff; 12]);
        }
        let t = format_track(&raw, false, SD_TRACK_BYTES);
        assert_eq!(t.idam.len(), 2);
        let secs = decode_sectors(&t);
        assert_eq!(secs.len(), 2);
        assert!(secs.iter().all(|s| s.crc_ok && s.data[0] == 0xe5));
        assert!(find_dam(&t, usize::from(t.idam[0]), false).is_some());
    }

    #[test]
    fn restore_and_index() {
        let mut d = drives_with_jv1();
        d[0].cylinder = 5;
        let mut f = fdc();
        f.track = 5;
        f.command(0x03, 0, &mut d);
        let mut sink = Vec::new();
        let now = run(&mut f, &mut d, 0, &mut sink);
        assert_eq!(d[0].cylinder, 0);
        assert_eq!(f.track, 0);
        let s = f.read_status(now, &d);
        assert_eq!(s & ST_TRACK0, ST_TRACK0);
        // The index pulse comes once a turn.
        let rev = f.revolution(SD_TRACK_BYTES);
        let pulses = (0..rev)
            .step_by(500)
            .filter(|&t| f.peek_status(t, &d) & ST_INDEX != 0);
        assert!(pulses.count() > 0);
    }
}
