//! WD1772 floppy disc controller, at sector level.
//!
//! Registers: 0 status/command, 1 track, 2 sector, 3 data. Bytes move at the
//! double-density rate of 32 µs; a program that does not take one in time
//! gets Lost Data. Sectors come from an image with a fixed geometry, so
//! formats are accepted and thrown away, and copy protection is out of
//! reach.
//!
//! Source: Western Digital, "WD1770/1772/1773 Floppy Disk Controller" data
//! sheet: command set, status bits, DRQ/INTRQ.

use emu_core::{StateError, StateReader, StateWriter};

const BYTE_CLOCKS: u32 = 256;
const FIRST_BYTE_CLOCKS: u32 = 8000;
/// Five revolutions, a guess of the sector search's end.
const NOT_FOUND_CLOCKS: u32 = 400_000;
const STEP_CLOCKS: u32 = 24_000;
/// 300 rpm on the 8 MHz bus.
const REVOLUTION_CLOCKS: u32 = 1_600_000;
/// The index hole passes the sensor for about 4 ms.
const INDEX_CLOCKS: u32 = 32_000;
/// The motor stops ten revolutions after the last command.
const MOTOR_REVOLUTIONS: u32 = 10;

pub const ST_BUSY: u8 = 0x01;
/// DRQ after a type II or III command, the index pulse after a type I.
pub const ST_DRQ: u8 = 0x02;
pub const ST_INDEX: u8 = 0x02;
pub const ST_TRACK0: u8 = 0x04;
pub const ST_LOST: u8 = 0x04;
pub const ST_NOT_FOUND: u8 = 0x10;
/// After a type I command: the motor is up to speed.
pub const ST_SPIN_UP: u8 = 0x20;
pub const ST_WRITE_PROTECT: u8 = 0x40;
pub const ST_MOTOR: u8 = 0x80;

/// A disc in a drive: an image and how its sectors lie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Floppy {
    pub bytes: Vec<u8>,
    pub sides: u32,
    pub tracks: u32,
    pub sectors: u32,
    pub sector_size: u32,
    pub write_protected: bool,
    pub dirty: bool,
}

impl Floppy {
    /// ADFS images by their size: L (640K), D/E (800K), F (1.6 MB); `None`
    /// for another.
    pub fn from_image(bytes: Vec<u8>) -> Option<Self> {
        let (sides, tracks, sectors, sector_size) = match bytes.len() {
            163_840 => (1, 40, 16, 256),
            327_680 => (1, 80, 16, 256),
            655_360 => (2, 80, 16, 256),
            819_200 => (2, 80, 5, 1024),
            1_638_400 => (2, 80, 10, 1024),
            _ => return None,
        };
        Some(Self {
            bytes,
            sides,
            tracks,
            sectors,
            sector_size,
            write_protected: false,
            dirty: false,
        })
    }

    /// Byte offset of a sector, if it exists.
    fn offset(&self, track: u32, side: u32, sector: u32) -> Option<usize> {
        if track >= self.tracks || side >= self.sides || sector >= self.sectors {
            return None;
        }
        let per_track = self.sectors * self.sector_size;
        Some(((track * self.sides + side) * per_track + sector * self.sector_size) as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// A seek ends when the timer does.
    Seek,
    ReadSector,
    WriteSector,
    ReadAddress,
    WriteTrack,
    /// The sector search runs out.
    NotFound,
}

#[derive(Clone, Debug)]
pub struct Wd1772 {
    status: u8,
    track: u8,
    sector: u8,
    data: u8,
    command: u8,
    phase: Phase,
    timer: u32,
    pub intrq: bool,
    pub drq: bool,
    /// Head position per drive.
    head: [u8; 4],
    /// Selected drive and side (from the board's latch).
    pub drive: Option<usize>,
    pub side: u32,
    /// Direction of the last step: true inwards.
    inwards: bool,
    pos: usize,
    left: usize,
    id: [u8; 6],
    /// Clocks since the index hole, while a disc is in the selected drive.
    angle: u32,
    /// Revolutions since the last command ended.
    idle_revolutions: u32,
    /// The last command was type I: bit 1 of the status shows the index.
    type_one: bool,
    index: bool,
}

impl Default for Wd1772 {
    fn default() -> Self {
        Self::new()
    }
}

impl Wd1772 {
    pub fn new() -> Self {
        Self {
            status: 0,
            track: 0,
            sector: 1,
            data: 0,
            command: 0,
            phase: Phase::Idle,
            timer: 0,
            intrq: false,
            drq: false,
            head: [0; 4],
            drive: None,
            side: 0,
            inwards: true,
            pos: 0,
            left: 0,
            id: [0; 6],
            angle: 0,
            idle_revolutions: 0,
            type_one: true,
            index: false,
        }
    }

    pub fn reset(&mut self) {
        let (head, drive, side) = (self.head, self.drive, self.side);
        *self = Self::new();
        self.head = head;
        self.drive = drive;
        self.side = side;
    }

    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    pub fn peek(&self, reg: u32) -> u8 {
        match reg & 3 {
            0 if self.type_one && self.index => self.status | ST_INDEX,
            0 => self.status,
            1 => self.track,
            2 => self.sector,
            _ => self.data,
        }
    }

    pub fn read(&mut self, reg: u32) -> u8 {
        let v = self.peek(reg);
        match reg & 3 {
            0 => self.intrq = false,
            3 => {
                self.drq = false;
                self.status &= !ST_DRQ;
            }
            _ => {}
        }
        v
    }

    fn head_now(&self) -> u8 {
        self.drive.map_or(0, |d| self.head[d & 3])
    }

    fn finish(&mut self) {
        self.phase = Phase::Idle;
        self.status &= !(ST_BUSY | ST_DRQ);
        self.drq = false;
        self.intrq = true;
    }

    fn status_type1(&mut self) {
        let track0 = if self.head_now() == 0 { ST_TRACK0 } else { 0 };
        self.status = (self.status & (ST_BUSY | ST_MOTOR)) | track0 | ST_SPIN_UP;
    }

    fn find_sector(&mut self, disks: &[Option<Floppy>]) -> Option<usize> {
        let d = disks.get(self.drive?)?.as_ref()?;
        if self.head_now() != self.track {
            return None;
        }
        d.offset(u32::from(self.track), self.side, u32::from(self.sector))
    }

    fn sector_size(&self, disks: &[Option<Floppy>]) -> usize {
        self.drive
            .and_then(|d| disks.get(d)?.as_ref())
            .map_or(256, |d| d.sector_size as usize)
    }

    fn start_sector(&mut self, write: bool, disks: &[Option<Floppy>]) {
        match self.find_sector(disks) {
            Some(pos) => {
                self.pos = pos;
                self.left = self.sector_size(disks);
                self.phase = if write {
                    Phase::WriteSector
                } else {
                    Phase::ReadSector
                };
                self.timer = FIRST_BYTE_CLOCKS;
            }
            None => {
                self.phase = Phase::NotFound;
                self.timer = NOT_FOUND_CLOCKS;
            }
        }
    }

    pub fn write(&mut self, reg: u32, v: u8, disks: &[Option<Floppy>]) {
        match reg & 3 {
            0 => self.command(v, disks),
            1 => self.track = v,
            2 => self.sector = v,
            _ => {
                self.data = v;
                self.drq = false;
                self.status &= !ST_DRQ;
            }
        }
    }

    fn command(&mut self, v: u8, disks: &[Option<Floppy>]) {
        if v & 0xf0 == 0xd0 {
            let was_busy = self.busy();
            self.phase = Phase::Idle;
            self.drq = false;
            self.status &= !(ST_BUSY | ST_DRQ);
            if !was_busy {
                self.type_one = true;
                self.status_type1();
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
        self.intrq = false;
        self.drq = false;
        self.status = ST_BUSY | ST_MOTOR;
        self.idle_revolutions = 0;
        self.type_one = v < 0x80;
        match v >> 4 {
            0 => {
                if let Some(d) = self.drive {
                    self.head[d & 3] = 0;
                }
                self.track = 0;
                self.phase = Phase::Seek;
                self.timer = STEP_CLOCKS;
            }
            1 => {
                let steps = u32::from(self.track.abs_diff(self.data));
                self.inwards = self.data > self.track;
                if let Some(d) = self.drive {
                    self.head[d & 3] = self.head[d & 3]
                        .wrapping_add(self.data)
                        .wrapping_sub(self.track);
                }
                self.track = self.data;
                self.phase = Phase::Seek;
                self.timer = STEP_CLOCKS * (steps + 1);
            }
            2..=7 => {
                let code = v >> 5;
                if code == 2 {
                    self.inwards = true;
                } else if code == 3 {
                    self.inwards = false;
                }
                if let Some(d) = self.drive {
                    let h = &mut self.head[d & 3];
                    *h = if self.inwards {
                        h.saturating_add(1)
                    } else {
                        h.saturating_sub(1)
                    };
                }
                if v & 0x10 != 0 {
                    self.track = if self.inwards {
                        self.track.wrapping_add(1)
                    } else {
                        self.track.wrapping_sub(1)
                    };
                }
                self.phase = Phase::Seek;
                self.timer = STEP_CLOCKS;
            }
            8 | 9 => self.start_sector(false, disks),
            10 | 11 => {
                let protected = self
                    .drive
                    .and_then(|d| disks.get(d)?.as_ref())
                    .is_some_and(|d| d.write_protected);
                if protected {
                    self.status |= ST_WRITE_PROTECT;
                    self.finish();
                } else {
                    self.start_sector(true, disks);
                }
            }
            12 => {
                let found = self.drive.and_then(|d| disks.get(d)?.as_ref());
                match found {
                    Some(d) => {
                        let size = match d.sector_size {
                            1024 => 3,
                            512 => 2,
                            _ => 1,
                        };
                        // The next header to pass under the head.
                        let slot = REVOLUTION_CLOCKS / d.sectors;
                        let next = self.angle / slot + 1;
                        let wait = next * slot - self.angle;
                        let head = self.head_now();
                        self.id = [head, self.side as u8, (next % d.sectors) as u8, size, 0, 0];
                        self.left = 6;
                        self.phase = Phase::ReadAddress;
                        self.timer = wait;
                        self.track = head;
                    }
                    None => {
                        self.phase = Phase::NotFound;
                        self.timer = NOT_FOUND_CLOCKS;
                    }
                }
            }
            14 => {
                // Read track has no track image behind it.
                self.phase = Phase::NotFound;
                self.timer = NOT_FOUND_CLOCKS;
            }
            _ => {
                self.left = 6250;
                self.phase = Phase::WriteTrack;
                self.timer = FIRST_BYTE_CLOCKS;
            }
        }
    }

    /// `clocks` of the 8 MHz bus went by.
    pub fn tick(&mut self, clocks: u32, disks: &mut [Option<Floppy>]) {
        self.rotate(clocks, disks);
        if self.phase == Phase::Idle {
            return;
        }
        let mut left = clocks;
        while left > 0 && self.phase != Phase::Idle {
            if self.timer > left {
                self.timer -= left;
                return;
            }
            left -= self.timer;
            self.timer = 0;
            self.event(disks);
        }
    }

    /// The disc turns; without one no light passes the index sensor.
    fn rotate(&mut self, clocks: u32, disks: &[Option<Floppy>]) {
        let angle = u64::from(self.angle) + u64::from(clocks);
        let turns = (angle / u64::from(REVOLUTION_CLOCKS)) as u32;
        self.angle = (angle % u64::from(REVOLUTION_CLOCKS)) as u32;
        let disc = self.drive.and_then(|d| disks.get(d)?.as_ref()).is_some();
        self.index = disc && self.angle < INDEX_CLOCKS;
        if self.phase == Phase::Idle {
            self.idle_revolutions = self.idle_revolutions.saturating_add(turns);
            if self.idle_revolutions >= MOTOR_REVOLUTIONS {
                self.status &= !ST_MOTOR;
            }
        }
    }

    fn request(&mut self) {
        if self.drq {
            self.status |= ST_LOST;
        }
        self.drq = true;
        self.status |= ST_DRQ;
        self.timer = BYTE_CLOCKS;
    }

    fn event(&mut self, disks: &mut [Option<Floppy>]) {
        match self.phase {
            Phase::Seek => {
                self.phase = Phase::Idle;
                self.status_type1();
                self.status &= !ST_BUSY;
                self.intrq = true;
            }
            Phase::NotFound => {
                self.status |= ST_NOT_FOUND;
                self.finish();
            }
            Phase::ReadSector => {
                if self.left == 0 {
                    self.sector_done(false, disks);
                    return;
                }
                let disk = self.drive.and_then(|d| disks.get(d)?.as_ref());
                self.data = disk
                    .and_then(|d| d.bytes.get(self.pos).copied())
                    .unwrap_or(0);
                self.pos += 1;
                self.left -= 1;
                self.request();
            }
            Phase::WriteSector => {
                if self.left < self.sector_size(disks) {
                    // The byte asked for last goes down now.
                    if let Some(d) = self.drive.and_then(|d| disks.get_mut(d)?.as_mut()) {
                        if let Some(b) = d.bytes.get_mut(self.pos - 1) {
                            *b = self.data;
                            d.dirty = true;
                        }
                    }
                }
                if self.left == 0 {
                    self.sector_done(true, disks);
                    return;
                }
                self.pos += 1;
                self.left -= 1;
                self.request();
            }
            Phase::ReadAddress => {
                if self.left == 0 {
                    self.finish();
                    return;
                }
                self.data = self.id[6 - self.left];
                self.left -= 1;
                if self.left == 5 {
                    self.sector = self.id[0];
                }
                self.request();
            }
            Phase::WriteTrack => {
                if self.left == 0 {
                    self.finish();
                    return;
                }
                self.left -= 1;
                self.request();
            }
            Phase::Idle => {}
        }
    }

    fn sector_done(&mut self, write: bool, disks: &mut [Option<Floppy>]) {
        if self.command & 0x10 != 0 {
            self.sector = self.sector.wrapping_add(1);
            // Multiple sectors end at the track's last with Not Found.
            self.start_sector(write, disks);
        } else {
            self.finish();
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"1772", 1, |w| {
            for v in [
                self.status,
                self.track,
                self.sector,
                self.data,
                self.command,
            ] {
                w.u8(v);
            }
            w.u8(self.phase as u8);
            w.u32(self.timer);
            w.bool(self.intrq);
            w.bool(self.drq);
            w.raw(&self.head);
            w.u8(self.drive.map_or(0xff, |d| d as u8));
            w.u32(self.side);
            w.bool(self.inwards);
            w.u32(self.pos as u32);
            w.u32(self.left as u32);
            w.raw(&self.id);
            w.u32(self.angle);
            w.u32(self.idle_revolutions);
            w.bool(self.type_one);
            w.bool(self.index);
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"1772", 1)?;
        self.status = c.u8()?;
        self.track = c.u8()?;
        self.sector = c.u8()?;
        self.data = c.u8()?;
        self.command = c.u8()?;
        self.phase = match c.u8()? {
            0 => Phase::Idle,
            1 => Phase::Seek,
            2 => Phase::ReadSector,
            3 => Phase::WriteSector,
            4 => Phase::ReadAddress,
            5 => Phase::WriteTrack,
            6 => Phase::NotFound,
            _ => return Err(StateError::Invalid("1772 phase")),
        };
        self.timer = c.u32()?;
        self.intrq = c.bool()?;
        self.drq = c.bool()?;
        c.raw_into(&mut self.head)?;
        let d = c.u8()?;
        self.drive = (d != 0xff).then_some(usize::from(d));
        self.side = c.u32()?;
        self.inwards = c.bool()?;
        self.pos = c.u32()? as usize;
        self.left = c.u32()? as usize;
        c.raw_into(&mut self.id)?;
        self.angle = c.u32()?;
        self.idle_revolutions = c.u32()?;
        self.type_one = c.bool()?;
        self.index = c.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc() -> Vec<Option<Floppy>> {
        let mut bytes = vec![0; 819_200];
        // Track 1, side 1, sector 2: (1*2+1)*5120 + 2*1024.
        bytes[3 * 5120 + 2048] = 0xa5;
        vec![Some(Floppy::from_image(bytes).unwrap())]
    }

    fn seek(c: &mut Wd1772, track: u8, d: &mut [Option<Floppy>]) {
        c.write(3, track, d);
        c.write(0, 0x10, d);
        c.tick(10_000_000, d);
        assert!(c.intrq);
        c.read(0);
    }

    #[test]
    fn reads_a_sector_with_drq_per_byte() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        c.side = 1;
        seek(&mut c, 1, &mut d);
        c.write(2, 2, &d);
        c.write(0, 0x80, &d);
        c.tick(FIRST_BYTE_CLOCKS, &mut d);
        assert!(c.drq);
        assert_eq!(c.read(3), 0xa5);
        let mut n = 1;
        while c.busy() {
            c.tick(BYTE_CLOCKS, &mut d);
            if c.drq {
                c.read(3);
                n += 1;
            }
        }
        assert_eq!(n, 1024);
        assert!(c.intrq);
        assert_eq!(c.peek(0) & (ST_LOST | ST_NOT_FOUND), 0);
    }

    #[test]
    fn writes_a_sector() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        c.write(2, 0, &d);
        c.write(0, 0xa0, &d);
        for i in 0..1024u32 {
            c.tick(
                if i == 0 {
                    FIRST_BYTE_CLOCKS
                } else {
                    BYTE_CLOCKS
                },
                &mut d,
            );
            assert!(c.drq);
            c.write(3, i as u8, &d);
        }
        c.tick(BYTE_CLOCKS * 2, &mut d);
        assert!(c.intrq);
        let f = d[0].as_ref().unwrap();
        assert_eq!(&f.bytes[..4], &[0, 1, 2, 3]);
        assert!(f.dirty);
    }

    #[test]
    fn missing_sectors_and_lost_data() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        c.write(2, 9, &d);
        c.write(0, 0x80, &d);
        c.tick(NOT_FOUND_CLOCKS, &mut d);
        assert_ne!(c.peek(0) & ST_NOT_FOUND, 0);
        c.write(2, 0, &d);
        c.write(0, 0x80, &d);
        c.tick(FIRST_BYTE_CLOCKS + BYTE_CLOCKS * 2, &mut d);
        assert_ne!(c.peek(0) & ST_LOST, 0);
    }

    #[test]
    fn read_address_reports_the_size() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        c.write(0, 0xc0, &d);
        let mut id = vec![];
        c.tick(FIRST_BYTE_CLOCKS - BYTE_CLOCKS, &mut d);
        while c.busy() {
            c.tick(BYTE_CLOCKS, &mut d);
            if c.drq {
                id.push(c.read(3));
            }
        }
        assert_eq!(id[3], 3, "1024 bytes");
    }

    #[test]
    fn read_address_walks_round_the_track() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        let mut seen = vec![];
        for _ in 0..5 {
            c.write(0, 0xc0, &d);
            let mut id = vec![];
            while c.busy() {
                c.tick(BYTE_CLOCKS, &mut d);
                if c.drq {
                    id.push(c.read(3));
                }
            }
            seen.push(id[2]);
        }
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn force_interrupt_and_track0() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        c.write(0, 0xd8, &d);
        assert!(c.intrq);
        c.read(0);
        c.write(0, 0xd0, &d);
        assert_ne!(c.peek(0) & ST_TRACK0, 0);
        seek(&mut c, 3, &mut d);
        assert_eq!(c.peek(0) & ST_TRACK0, 0);
    }

    #[test]
    fn index_pulses_show_after_a_type_one_command_and_the_motor_stops() {
        let mut d = disc();
        let mut c = Wd1772::new();
        c.drive = Some(0);
        seek(&mut c, 0, &mut d);
        assert_ne!(c.peek(0) & ST_SPIN_UP, 0);
        c.tick(REVOLUTION_CLOCKS - c.angle, &mut d);
        assert_ne!(c.peek(0) & ST_INDEX, 0);
        c.tick(INDEX_CLOCKS, &mut d);
        assert_eq!(c.peek(0) & ST_INDEX, 0);
        c.tick(REVOLUTION_CLOCKS * MOTOR_REVOLUTIONS, &mut d);
        assert_eq!(c.peek(0) & ST_MOTOR, 0);

        let mut empty = vec![None];
        c.tick(REVOLUTION_CLOCKS - c.angle, &mut empty);
        assert_eq!(c.peek(0) & ST_INDEX, 0, "no disc, no pulse");
    }

    #[test]
    fn savestate_round_trip() {
        let mut c = Wd1772::new();
        c.write(1, 7, &[]);
        let mut w = StateWriter::new();
        c.save(&mut w);
        let b = w.into_bytes();
        let mut n = Wd1772::new();
        n.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(n.peek(1), 7);
    }
}
