//! Atari SIO bus with disk drives D1:–D4: (810, 1050, XF551) at protocol
//! level: the drives answer command frames like the real ones, with their
//! timing, but run no firmware.
//!
//! Protocol (Atari 400/800 OS Manual, "Serial I/O"; Altirra Hardware
//! Reference Manual, "SIO"): the computer pulls COMMAND low (PIA CB2) and
//! sends a 5-byte frame — device ID ($31–$34 = D1:–D4:), command, AUX1,
//! AUX2, checksum (sum with end-around carry) — then releases COMMAND. The
//! device answers ACK `A` or NAK `N`; for data to the device the computer
//! then sends a data frame (data + checksum) which the device ACKs. The
//! device ends with COMPLETE `C` or ERROR `E`, followed by its data frame
//! for commands that return data. All bytes run at 19 200 baud, 8N1.
//!
//! Commands: `R` read sector, `W` write with verify, `P` put (write), `S`
//! status (4 bytes: drive status, inverted FDC status, format timeout, 0),
//! `!` format, `"` format enhanced density (1050). Status bits: 7 enhanced
//! density, 5 double density, 4 motor on, 3 write protect, 2 write error,
//! 1 data frame error, 0 command frame error.
//!
//! Timing (approximations of the mechanics; constants per model): ACK 1 ms
//! after COMMAND rises; seek = tracks × step rate + 10 ms settle; then the
//! wait for the sector to reach the head on a 288 rpm disk with the
//! drives' 2:1 sector interleave, plus one sector to read it; writes with
//! verify take an extra revolution; a status answers in 1 ms.
//!
//! A drive without a disk does not answer at all, like a drive that is
//! switched off: the OS then boots on without it.

use std::collections::VecDeque;

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::atr::{self, Atr};

pub const BAUD: u64 = 19_200;
const ACK: u8 = b'A';
const NAK: u8 = b'N';
const COMPLETE: u8 = b'C';
const ERROR: u8 = b'E';

/// SIO checksum: byte sum with end-around carry.
pub fn checksum(data: &[u8]) -> u8 {
    let mut sum: u16 = 0;
    for &b in data {
        sum += u16::from(b);
        if sum > 0xff {
            sum = (sum & 0xff) + 1;
        }
    }
    sum as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveModel {
    /// Atari 810: single density only.
    A810,
    /// Atari 1050: single and enhanced (medium) density.
    A1050,
    /// Atari XF551: single and double density.
    Xf551,
}

impl DriveModel {
    pub fn from_option(v: &str) -> Option<Self> {
        match v {
            "810" => Some(Self::A810),
            "1050" => Some(Self::A1050),
            "xf551" => Some(Self::Xf551),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::A810 => "810",
            Self::A1050 => "1050",
            Self::Xf551 => "xf551",
        }
    }

    /// Track-to-track step time in ms.
    fn step_ms(self) -> u64 {
        match self {
            Self::A810 => 10,
            Self::A1050 => 20,
            Self::Xf551 => 6,
        }
    }

    fn reads(self, atr: &Atr) -> bool {
        match self {
            Self::A810 => atr.single_density(),
            Self::A1050 => atr.sector_size == 128,
            Self::Xf551 => atr.single_density() || atr.sector_size == 256,
        }
    }
}

/// A disk drive on the bus.
pub struct SioDrive {
    model: DriveModel,
    media: Option<Media>,
    atr: Option<Atr>,
    write_protect: bool,
    /// Track under the head.
    track: u16,
    /// The last command's result for the status byte.
    last_error: u8,
    /// The bus is answering a command for this drive (the computer's data
    /// frame or the drive's reply still to come); kept by [`Sio`].
    busy: bool,
}

impl SioDrive {
    pub fn new(model: DriveModel) -> Self {
        Self {
            model,
            media: None,
            atr: None,
            write_protect: false,
            track: 0,
            last_error: 0,
            busy: false,
        }
    }

    pub fn model(&self) -> DriveModel {
        self.model
    }

    pub fn has_disk(&self) -> bool {
        self.media.is_some()
    }

    /// A readable disk for this drive.
    fn disk(&self) -> Option<Atr> {
        self.atr.filter(|a| self.model.reads(a))
    }

    fn sectors_per_track(&self) -> u16 {
        match self.atr {
            Some(a) if a.enhanced_density() => 26,
            _ => 18,
        }
    }

    /// The 4 bytes of a status command's data frame.
    fn status_frame(&self) -> [u8; 4] {
        let mut s = self.last_error;
        if self.write_protect {
            s |= 0x08;
        }
        if let Some(a) = self.atr {
            if a.sector_size == 256 {
                s |= 0x20;
            }
            if a.enhanced_density() {
                s |= 0x80;
            }
        }
        // Inverted FDC status ($FF = no error), format timeout.
        [s, 0xff, 0xe0, 0x00]
    }

    fn read_sector(&mut self, n: u16) -> Option<Vec<u8>> {
        let atr = self.disk()?;
        let off = atr.offset(u32::from(n))?;
        let mut buf = vec![0; atr.size_of(u32::from(n))];
        self.media.as_mut()?.backend.read(off, &mut buf).ok()?;
        Some(buf)
    }

    fn write_sector(&mut self, n: u16, data: &[u8]) -> bool {
        let Some(atr) = self.disk() else { return false };
        if self.write_protect {
            return false;
        }
        let Some(off) = atr.offset(u32::from(n)) else {
            return false;
        };
        let size = atr.size_of(u32::from(n)).min(data.len());
        self.media
            .as_mut()
            .is_some_and(|m| m.backend.write(off, &data[..size]).is_ok())
    }

    /// Bytes per sector of the data frame for sector `n` (128 without a
    /// readable disk).
    fn frame_size(&self, n: u16) -> usize {
        self.disk().map_or(128, |a| a.size_of(u32::from(n)))
    }
}

impl MediaSlot for SioDrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut header = [0u8; atr::HEADER];
        media.backend.read(0, &mut header)?;
        let atr = Atr::parse(&header, media.backend.len())?;
        self.media = Some(media);
        self.atr = Some(atr);
        self.track = 0;
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.atr = None;
        self.media.take()
    }

    fn write_protect(&mut self, on: bool) {
        self.write_protect = on;
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.media {
            Some(m) => m.backend.commit(),
            None => Ok(()),
        }
    }

    /// Busy while the bus serves a command for this drive; an error while
    /// the last command failed (status bits 0–2: bad frame or sector, write
    /// error, unreadable disk); the track of the last sector accessed.
    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.busy,
            error: self.last_error & 0x07 != 0,
            track: u8::try_from(self.track.saturating_add(1)).ok(),
        }
    }
}

/// What the bus drives onto the computer's serial input this cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SerialIn {
    /// Line level (true = mark/idle).
    pub level: bool,
    /// A byte is being shifted in.
    pub busy: bool,
    /// A byte completed (its stop bit ended) this cycle.
    pub byte: Option<u8>,
}

/// Bytes from a device to the computer, each with the cycle its start bit
/// begins; 8N1 at [`BAUD`] (SIO) or another rate (cassette).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SerialTx {
    hz: u64,
    baud: u64,
    queue: VecDeque<(u64, u8)>,
}

impl SerialTx {
    pub fn new(hz: u32) -> Self {
        Self::with_baud(hz, BAUD as u32)
    }

    pub fn with_baud(hz: u32, baud: u32) -> Self {
        Self {
            hz: u64::from(hz),
            baud: u64::from(baud.max(1)),
            queue: VecDeque::new(),
        }
    }

    /// Change the rate for bytes queued from now on (only while idle).
    pub fn set_baud(&mut self, baud: u32) {
        self.baud = u64::from(baud.max(1));
    }

    fn byte_cycles(&self) -> u64 {
        10 * self.hz / self.baud
    }

    /// Queued bytes with their start cycles (for savestates).
    pub fn queued(&self) -> impl Iterator<Item = (u64, u8)> + '_ {
        self.queue.iter().copied()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Queue `bytes` back to back from `at` (or after what is queued);
    /// returns the cycle the last stop bit ends.
    pub fn send(&mut self, at: u64, bytes: &[u8]) -> u64 {
        let len = self.byte_cycles();
        let mut t = self.queue.back().map_or(at, |&(s, _)| (s + len).max(at));
        for &b in bytes {
            self.queue.push_back((t, b));
            t += len;
        }
        t
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn tick(&mut self, now: u64) -> SerialIn {
        let idle = SerialIn {
            level: true,
            busy: false,
            byte: None,
        };
        let Some(&(start, b)) = self.queue.front() else {
            return idle;
        };
        if now < start {
            return idle;
        }
        let len = self.byte_cycles();
        if now - start + 1 >= len {
            self.queue.pop_front();
            return SerialIn {
                level: true,
                busy: false,
                byte: Some(b),
            };
        }
        let bit = (now - start) * self.baud / self.hz;
        let level = match bit {
            0 => false,
            1..=8 => b >> (bit - 1) & 1 != 0,
            _ => true,
        };
        SerialIn {
            level,
            busy: true,
            byte: None,
        }
    }
}

/// SIO savestate chunk version (2: per-drive busy flag).
const SIO_VERSION: u16 = 2;

/// A data frame the bus waits for from the computer (write commands).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    unit: usize,
    command: u8,
    sector: u16,
    need: usize,
    got: Vec<u8>,
}

/// The SIO bus: drives D1:–D4: and the serial lines.
pub struct Sio {
    hz: u32,
    pub drives: [Option<SioDrive>; 4],
    /// A one-sector boot disk D1: serves instead of its disk (program
    /// loading).
    boot: Option<Vec<u8>>,
    command: bool,
    frame: Vec<u8>,
    pending: Option<Pending>,
    tx: SerialTx,
}

impl Sio {
    pub fn new(hz: u32) -> Self {
        Self {
            hz,
            drives: [None, None, None, None],
            boot: None,
            command: false,
            frame: Vec::new(),
            pending: None,
            tx: SerialTx::new(hz),
        }
    }

    fn ms(&self, ms: u64) -> u64 {
        ms * u64::from(self.hz) / 1000
    }

    /// Serve `sector1` as D1:'s only sector (and a status) until cleared.
    pub fn set_boot_sector(&mut self, sector1: Option<Vec<u8>>) {
        self.boot = sector1;
    }

    /// RESET does not reach the drives; a power cycle stops transfers.
    pub fn power_on(&mut self) {
        self.command = false;
        self.frame.clear();
        self.pending = None;
        self.tx = SerialTx::new(self.hz);
        self.end_busy();
    }

    /// The COMMAND line (true = asserted, low). The frame is processed when
    /// it rises.
    pub fn set_command(&mut self, asserted: bool, now: u64) {
        if asserted && !self.command {
            self.frame.clear();
            self.pending = None;
            self.end_busy();
        }
        if !asserted && self.command {
            let frame = std::mem::take(&mut self.frame);
            self.command_frame(&frame, now);
        }
        self.command = asserted;
    }

    /// A byte the computer finished sending.
    pub fn byte_from_computer(&mut self, v: u8, now: u64) {
        if self.command {
            if self.frame.len() < 5 {
                self.frame.push(v);
            }
            return;
        }
        let Some(p) = &mut self.pending else { return };
        p.got.push(v);
        if p.got.len() == p.need {
            let p = self.pending.take().expect("pending");
            self.data_frame(p, now);
        }
    }

    pub fn tick(&mut self, now: u64) -> SerialIn {
        let r = self.tx.tick(now);
        if r.byte.is_some() {
            self.end_busy();
        }
        r
    }

    /// Clears the drives' busy flags once the bus is idle.
    fn end_busy(&mut self) {
        if !self.busy() {
            for d in self.drives.iter_mut().flatten() {
                d.busy = false;
            }
        }
    }

    fn command_frame(&mut self, f: &[u8], now: u64) {
        if f.len() != 5 || !(0x31..=0x34).contains(&f[0]) {
            return;
        }
        let unit = usize::from(f[0] - 0x31);
        let boot = unit == 0 && self.boot.is_some();
        // Nobody answers without a drive; an empty drive counts as switched
        // off (a real one left on makes the OS retry "BOOT ERROR" forever).
        if !boot && !self.drives[unit].as_ref().is_some_and(SioDrive::has_disk) {
            return;
        }
        let ack_at = now + self.ms(1);
        if checksum(&f[..4]) != f[4] {
            self.tx.send(ack_at, &[NAK]);
            return;
        }
        let (command, sector) = (f[1], u16::from_le_bytes([f[2], f[3]]));
        let rev = self.rev_cycles();
        let hz = u64::from(self.hz);
        if boot {
            self.boot_command(command, sector, ack_at);
            return;
        }
        let Some(drive) = self.drives[unit].as_mut() else {
            return;
        };
        drive.busy = true;
        match command {
            b'S' => {
                let status = drive.status_frame();
                let done = self.tx.send(ack_at, &[ACK]);
                self.send_frame(done + self.ms(1), COMPLETE, &status);
            }
            b'R' => {
                let valid = drive
                    .atr
                    .is_some_and(|a| u32::from(sector) >= 1 && u32::from(sector) <= a.sectors);
                if drive.has_disk() && !valid {
                    drive.last_error = 0x01;
                    self.tx.send(ack_at, &[NAK]);
                    return;
                }
                let done = self.tx.send(ack_at, &[ACK]);
                let ready = done + self.seek_and_rotate(unit, sector, done, false);
                let drive = self.drives[unit].as_mut().expect("drive");
                match drive.read_sector(sector) {
                    Some(data) => {
                        drive.last_error = 0;
                        self.send_frame(ready, COMPLETE, &data);
                    }
                    None => {
                        drive.last_error = 0x04;
                        let size = drive.frame_size(sector);
                        self.send_frame(ready, ERROR, &vec![0; size]);
                    }
                }
            }
            b'W' | b'P' => {
                let need = drive.frame_size(sector) + 1;
                self.tx.send(ack_at, &[ACK]);
                self.pending = Some(Pending {
                    unit,
                    command,
                    sector,
                    need,
                    got: Vec::new(),
                });
            }
            b'!' | b'"' => {
                let enhanced = command == b'"';
                if enhanced && drive.model != DriveModel::A1050 {
                    self.tx.send(ack_at, &[NAK]);
                    return;
                }
                let done = self.tx.send(ack_at, &[ACK]);
                let ok = drive.has_disk() && !drive.write_protect && Self::format(drive, enhanced);
                drive.last_error = if ok { 0 } else { 0x04 };
                let size = drive.frame_size(4);
                // 40 tracks: a revolution each plus the step.
                let step = drive.model.step_ms() * hz / 1000;
                let t = done + 40 * (rev + step);
                // Bad sector list: none ($FFFF).
                self.send_frame(t, if ok { COMPLETE } else { ERROR }, &vec![0xff; size]);
            }
            _ => {
                drive.last_error = 0x01;
                self.tx.send(ack_at, &[NAK]);
            }
        }
    }

    fn boot_command(&mut self, command: u8, sector: u16, ack_at: u64) {
        let data = self.boot.clone().unwrap_or_default();
        match (command, sector) {
            (b'S', _) => {
                let done = self.tx.send(ack_at, &[ACK]);
                self.send_frame(done + self.ms(1), COMPLETE, &[0x00, 0xff, 0xe0, 0x00]);
            }
            (b'R', 1) => {
                let done = self.tx.send(ack_at, &[ACK]);
                let mut sector = data;
                sector.resize(128, 0);
                self.send_frame(done + self.ms(1), COMPLETE, &sector);
            }
            _ => {
                self.tx.send(ack_at, &[NAK]);
            }
        }
    }

    /// Clear a disk to zeros in the new density (same image size).
    fn format(drive: &mut SioDrive, _enhanced: bool) -> bool {
        let Some(m) = &mut drive.media else {
            return false;
        };
        let len = m.backend.len();
        let zeros = vec![0u8; (len as usize).saturating_sub(atr::HEADER)];
        m.backend.write(atr::HEADER as u64, &zeros).is_ok()
    }

    fn data_frame(&mut self, p: Pending, now: u64) {
        let ack_at = now + self.ms(1);
        let (data, sum) = p.got.split_at(p.need - 1);
        if checksum(data) != sum[0] {
            self.tx.send(ack_at, &[NAK]);
            return;
        }
        let done = self.tx.send(ack_at, &[ACK]);
        let ready = done + self.seek_and_rotate(p.unit, p.sector, done, p.command == b'W');
        let drive = self.drives[p.unit].as_mut().expect("drive");
        let ok = drive.write_sector(p.sector, data);
        drive.last_error = if ok { 0 } else { 0x04 };
        self.tx.send(ready, &[if ok { COMPLETE } else { ERROR }]);
    }

    fn rev_cycles(&self) -> u64 {
        // 288 rpm.
        u64::from(self.hz) * 60 / 288
    }

    /// Cycles from `at` until sector `n` has passed under the head: seek,
    /// settle, rotational wait and the sector itself (plus a revolution to
    /// verify a write).
    fn seek_and_rotate(&mut self, unit: usize, n: u16, at: u64, verify: bool) -> u64 {
        let rev = self.rev_cycles();
        let (spt, step, from) = {
            let d = self.drives[unit].as_ref().expect("drive");
            (d.sectors_per_track(), d.model.step_ms(), d.track)
        };
        let s = n.max(1) - 1;
        let track = s / spt;
        let in_track = s % spt;
        let seek = if track == from {
            0
        } else {
            self.ms(step * u64::from(track.abs_diff(from)) + 10)
        };
        if let Some(d) = self.drives[unit].as_mut() {
            d.track = track;
        }
        // 2:1 interleave: odd sectors on the first half of the track.
        let half = spt.div_ceil(2);
        let slot = if in_track.is_multiple_of(2) {
            in_track / 2
        } else {
            half + in_track / 2
        };
        let slot_cycles = rev / u64::from(spt);
        let ready = at + seek;
        let target = u64::from(slot) * slot_cycles;
        let wait = (target + rev - ready % rev) % rev;
        seek + wait + slot_cycles + if verify { rev } else { 0 }
    }

    fn send_frame(&mut self, at: u64, status: u8, data: &[u8]) {
        let mut bytes = Vec::with_capacity(data.len() + 2);
        bytes.push(status);
        bytes.extend_from_slice(data);
        bytes.push(checksum(data));
        self.tx.send(at, &bytes);
    }

    /// Whether a transfer is under way (for a drive-activity display).
    pub fn busy(&self) -> bool {
        !self.tx.is_idle() || self.pending.is_some()
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"SIO ", SIO_VERSION, |w| {
            w.bool(self.command);
            w.bytes(&self.frame);
            w.bool(self.pending.is_some());
            if let Some(p) = &self.pending {
                w.u8(p.unit as u8);
                w.u8(p.command);
                w.u16(p.sector);
                w.u16(p.need as u16);
                w.bytes(&p.got);
            }
            w.u32(self.tx.queue.len() as u32);
            for &(t, b) in &self.tx.queue {
                w.u64(t);
                w.u8(b);
            }
            w.bool(self.boot.is_some());
            w.bytes(self.boot.as_deref().unwrap_or_default());
            for d in &self.drives {
                w.bool(d.is_some());
                if let Some(d) = d {
                    w.str(d.model.name());
                    w.u16(d.track);
                    w.u8(d.last_error);
                    w.bool(d.write_protect);
                    w.bool(d.busy);
                }
            }
        });
    }

    /// Restores bus and drive mechanics; the disks stay as inserted. The
    /// drive configuration must match.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (version, mut c) = r.chunk(*b"SIO ", SIO_VERSION)?;
        let command = c.bool()?;
        let frame = c.bytes()?.to_vec();
        let pending = if c.bool()? {
            let unit = usize::from(c.u8()?);
            let command = c.u8()?;
            let sector = c.u16()?;
            let need = usize::from(c.u16()?);
            let got = c.bytes()?.to_vec();
            if unit > 3 || need == 0 || got.len() >= need {
                return Err(StateError::Invalid("SIO data frame"));
            }
            Some(Pending {
                unit,
                command,
                sector,
                need,
                got,
            })
        } else {
            None
        };
        let n = c.u32()? as usize;
        if n > 1024 {
            return Err(StateError::Invalid("SIO transmit queue"));
        }
        let mut queue = VecDeque::with_capacity(n);
        for _ in 0..n {
            queue.push_back((c.u64()?, c.u8()?));
        }
        let has_boot = c.bool()?;
        let boot = c.bytes()?.to_vec();
        let mut mech = Vec::new();
        for d in &self.drives {
            let present = c.bool()?;
            if present != d.is_some() {
                return Err(StateError::Invalid("SIO drive configuration differs"));
            }
            if let Some(d) = d {
                let model = c.string()?;
                if model != d.model.name() {
                    return Err(StateError::Invalid("SIO drive model differs"));
                }
                let (track, err, wp) = (c.u16()?, c.u8()?, c.bool()?);
                // Version 1 has no busy flag: busy while a data frame is due.
                let busy = if version >= 2 {
                    c.bool()?
                } else {
                    pending.as_ref().is_some_and(|p| p.unit == mech.len())
                };
                mech.push(Some((track, err, wp, busy)));
            } else {
                mech.push(None);
            }
        }
        for (d, m) in self.drives.iter_mut().zip(mech) {
            if let (Some(d), Some((track, err, wp, busy))) = (d, m) {
                d.track = track;
                d.last_error = err;
                d.write_protect = wp;
                d.busy = busy;
            }
        }
        self.command = command;
        self.frame = frame;
        self.pending = pending;
        self.tx.queue = queue;
        self.boot = has_boot.then_some(boot);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
