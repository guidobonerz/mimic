//! The Macintosh floppy controller: Apple's IWM ("Integrated Woz
//! Machine") and the Sony 3.5" drives (400K single-sided, 800K
//! double-sided), at the level of the bit stream under the heads.
//!
//! **IWM** (Mac address $DFE1FF + n·$200, odd bytes; Thomas W.,
//! "Emulating the Macintosh floppy drive", thomasw.dev; Linux/m68k "Mac
//! Plus hardware" notes): accesses n = 0–15 switch eight state bits off
//! (even n) and on (odd n): CA0, CA1, CA2, LSTRB (the drive's phase
//! lines), ENABLE (drive enabled), SELECT (0 internal, 1 external drive),
//! Q6, Q7. The access then reads or writes the register Q6/Q7 select:
//!
//! | Q7 Q6 | read                                      | write (odd access) |
//! | ----- | ----------------------------------------- | ------------------ |
//! | 0 0   | data register (read mode)                 | —                  |
//! | 0 1   | status: 7 sense line, 5 ENABLE, 4–0 mode  | —                  |
//! | 1 0   | handshake: 7 write buffer empty, 6 0 = underrun | —            |
//! | 1 1   | —                                         | ENABLE off: mode register; on: write data |
//!
//! The Mac runs the IWM in latch mode: bits from the head shift in, and
//! when the shift register's bit 7 is set it moves to the data register
//! (a byte not read by then is lost); reading the data register clears
//! it. Writing shifts the data register's byte out one bit per cell; the
//! next byte must be in the buffer when eight bits are out, or the write
//! underruns and stops.
//!
//! **Drive** (thomasw.dev, CiderPress II notes): the drive's registers
//! are addressed by CA2 CA1 CA0 and SEL (the VIA's HEADSEL, PA5). Reads
//! put the selected line on the status register's sense bit (0 = the
//! condition):
//!
//! | CA2 CA1 CA0 SEL | sense                                         |
//! | --------------- | --------------------------------------------- |
//! | 0000 DIRTN      | step direction (0 = towards higher tracks)    |
//! | 0001 CSTIN      | 0 = disk in                                   |
//! | 0010 STEP       | 0 = the head is stepping                      |
//! | 0011 WRTPRT     | 0 = write protected                           |
//! | 0100 MOTORON    | 0 = motor on                                  |
//! | 0101 TK0        | 0 = head at track 0                           |
//! | 0110 EJECT      | 0 = ejecting (always 1 here)                  |
//! | 0111 TACH       | tachometer: 60 pulses a turn                  |
//! | 1000/1001       | RDDATA0/1: the bit under the lower/upper head |
//! | 1100 SIDES      | 1 = two heads (800K drive)                    |
//! | 1101 READY      | 0 = ready (the Plus ROM's driver waits for 0 after a step) |
//! | 1110 INSTALLED  | 0 = drive connected                           |
//! | 1111 NEWINTF    | 1 = 800K drive                                |
//!
//! Commands: with CA0, CA1, SEL picking one and CA2 its value, LSTRB's
//! rising edge sets the direction (000), steps (010, CA2 = 0), turns the
//! motor on (100, CA2 = 0) or off (CA2 = 1) and ejects (110, CA2 = 1).
//! SEL also picks the head for reading and writing. The drive turns at
//! the speed of the head's zone ([`macdisk::TRACK_BITS`]: every track has
//! 2 µs bit cells); the Mac 128K/512K's 400K drive turns as fast as the
//! pulse width the machine feeds it ([`SonyDrive::pwm`]) says, and its
//! ROM finds the settings for each zone by the tachometer. The IWM takes
//! every bit the head passes, whatever the speed. Unrecorded track
//! positions read as zeros.
//!
//! The disk is media, not state: savestates keep the controller and the
//! mechanics.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::macdisk::{self, Image, MacDisk};

/// The disk's position advances by `rate` / `TURN_DEN` bit cells a CPU
/// clock (7.8336 MHz), `rate` = cells a turn × rpm × 1000.
const TURN_DEN: u128 = 60_000 * 7_833_600;
/// A self-regulated drive: 2 µs cells (500 000 a second) at every zone.
const NOMINAL_RATE: u128 = 500_000 * 60_000;
/// How long STEP reads "stepping" after a step command (1 ms), in CPU
/// clocks. The Plus ROM's driver waits about 5 ms after a step and takes a
/// drive still stepping then as a seek error; the head's settling is the
/// driver's own wait.
const STEP_CLOCKS: u64 = 7_834;

struct Disk {
    media: Media,
    image: Image,
    tracks: MacDisk,
    dirty: bool,
    protected: bool,
}

impl Disk {
    fn parse(mut media: Media) -> Result<Self, MediaError> {
        let mut file = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut file)?;
        let image = Image::parse(&file).map_err(MediaError::Format)?;
        let tracks = image.encode();
        Ok(Self {
            media,
            image,
            tracks,
            dirty: false,
            protected: false,
        })
    }

    fn write_back(&mut self) -> Result<(), MediaError> {
        if !std::mem::take(&mut self.dirty) {
            return Ok(());
        }
        let bad = self.image.decode(&self.tracks);
        self.media.backend.write(0, &self.image.to_bytes())?;
        if bad > 0 {
            return Err(MediaError::Format(format!(
                "{bad} sectors could not be decoded"
            )));
        }
        Ok(())
    }
}

/// One Sony drive.
pub struct SonyDrive {
    /// An 800K (double-sided) drive.
    pub double_sided: bool,
    pub track: u8,
    /// DIRTN: true = towards track 0.
    pub dir_down: bool,
    pub motor: bool,
    /// Clock the current step ends.
    step_until: u64,
    /// Bit cell under the head and the fraction of the next one
    /// (in 1/`TURN_DEN` cells), as of `clock`.
    pub pos: u32,
    frac: u64,
    /// The 400K drive's speed control (Mac 128K/512K): the average pulse
    /// width the machine's speed circuit makes, 0–62 in 1/256 steps;
    /// `None` for a drive that sets its own speed.
    pub pwm: Option<u16>,
    clock: u64,
    disk: Option<Disk>,
}

impl std::fmt::Debug for SonyDrive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SonyDrive")
            .field("track", &self.track)
            .field("motor", &self.motor)
            .field("disk", &self.disk.as_ref().map(|d| &d.media.name))
            .finish()
    }
}

impl SonyDrive {
    pub fn new(double_sided: bool) -> Self {
        Self {
            double_sided,
            track: 0,
            dir_down: false,
            motor: false,
            step_until: 0,
            pos: 0,
            frac: 0,
            pwm: None,
            clock: 0,
            disk: None,
        }
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    fn track_bits(&self) -> u32 {
        macdisk::TRACK_BITS[macdisk::zone(usize::from(self.track))]
    }

    /// The stream under head `side`, if the disk has it.
    fn stream(&self, side: usize) -> Option<usize> {
        let d = self.disk.as_ref()?;
        let sides = d.tracks.sides;
        (side < sides && (self.double_sided || side == 0))
            .then_some(usize::from(self.track) * sides + side)
    }

    /// Turn the disk to `now`; returns the bit cells that passed.
    fn spin_to(&mut self, now: u64) -> u64 {
        let elapsed = now.saturating_sub(self.clock);
        self.clock = now.max(self.clock);
        if !self.motor || self.disk.is_none() {
            return 0;
        }
        let bits = u64::from(self.track_bits());
        let total = u128::from(self.frac) + u128::from(elapsed) * self.rate();
        let cells = (total / TURN_DEN) as u64;
        self.frac = (total % TURN_DEN) as u64;
        self.pos = ((u64::from(self.pos) + cells) % bits) as u32;
        cells
    }

    /// Cells a turn × rpm × 1000 (see [`TURN_DEN`]). With pulse-width
    /// control the motor turns at 155 rpm plus 7.95 rpm a step of the
    /// width (this project's linear stand-in for the drive's curve, chosen
    /// so the zones' 394–590 rpm lie inside the range the 128K ROM's
    /// speed values reach; the ROM measures the tachometer at two settings
    /// and interpolates).
    fn rate(&self) -> u128 {
        match self.pwm {
            Some(p) => {
                let rpm_milli = 155_000 + 7_950 * u128::from(p.min(62 << 8)) / 256;
                u128::from(self.track_bits()) * rpm_milli
            }
            None => NOMINAL_RATE,
        }
    }

    /// One bit cell under head `side` (written first if `write` is set).
    fn cell(&mut self, side: usize, write: Option<bool>) -> bool {
        let bits = self.track_bits();
        let pos = self.pos;
        self.pos = (self.pos + 1) % bits;
        let Some(i) = self.stream(side) else {
            return false;
        };
        let Some(d) = &mut self.disk else {
            return false;
        };
        let (Some(track), Some(&n)) = (d.tracks.tracks.get_mut(i), d.tracks.bits.get(i)) else {
            return false;
        };
        if pos >= n {
            return false;
        }
        let byte = &mut track[(pos / 8) as usize];
        let mask = 0x80 >> (pos % 8);
        if let (Some(b), false) = (write, d.protected) {
            if b {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
            d.dirty = true;
        }
        *byte & mask != 0
    }

    /// The drive's sense line for register `reg` (CA2 CA1 CA0 SEL).
    fn sense(&self, reg: u8, now: u64, side: usize) -> bool {
        let disk = self.disk.is_some();
        match reg {
            0x0 => self.dir_down,
            0x1 => !disk,
            0x2 => now >= self.step_until,
            0x3 => !(disk && self.protected()),
            0x4 => !self.motor,
            0x5 => self.track != 0,
            0x6 => true,
            0x7 => {
                let bits = u64::from(self.track_bits());
                (u64::from(self.pos) * 120 / bits) & 1 != 0
            }
            0x8 | 0x9 => self.peek_bit(usize::from(reg & 1 == 1 && side == 1)),
            0xc => self.double_sided,
            0xd => !(self.motor && disk),
            0xe => false,
            0xf => self.double_sided,
            _ => true,
        }
    }

    fn peek_bit(&self, side: usize) -> bool {
        let (Some(i), Some(d)) = (self.stream(side), &self.disk) else {
            return false;
        };
        let pos = self.pos;
        d.tracks.bits.get(i).is_some_and(|&n| pos < n)
            && d.tracks.tracks[i][(pos / 8) as usize] & (0x80 >> (pos % 8)) != 0
    }

    /// A command strobed with LSTRB: `cmd` = CA1 CA0 SEL, `ca2` its value.
    fn command(&mut self, cmd: u8, ca2: bool, now: u64) {
        match cmd {
            0b000 => self.dir_down = ca2,
            0b010 if !ca2 => {
                let t = if self.dir_down {
                    self.track.saturating_sub(1)
                } else {
                    (self.track + 1).min(macdisk::TRACKS as u8 - 1)
                };
                if t != self.track {
                    // The same angle on the new track.
                    let old = u64::from(self.track_bits());
                    self.track = t;
                    let new = u64::from(self.track_bits());
                    self.pos = (u64::from(self.pos) * new / old) as u32 % new as u32;
                }
                self.step_until = now + STEP_CLOCKS;
            }
            0b100 => {
                self.motor = !ca2;
            }
            0b110 if ca2 => {
                let _ = self.eject();
                self.motor = false;
            }
            _ => {}
        }
    }

    pub fn protected(&self) -> bool {
        self.disk.as_ref().is_some_and(|d| d.protected)
    }

    fn save(&self, w: &mut StateWriter) {
        w.u8(self.track);
        w.bool(self.dir_down);
        w.bool(self.motor);
        w.u64(self.step_until);
        w.u32(self.pos);
        w.u64(self.frac);
        w.u16(self.pwm.map_or(0xffff, |p| p.min(63 << 8)));
        w.u64(self.clock);
    }

    fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let track = c.u8()?;
        if usize::from(track) >= macdisk::TRACKS {
            return Err(StateError::Invalid("sony drive track"));
        }
        self.track = track;
        self.dir_down = c.bool()?;
        self.motor = c.bool()?;
        self.step_until = c.u64()?;
        self.pos = c.u32()? % self.track_bits();
        self.frac = (u128::from(c.u64()?) % TURN_DEN) as u64;
        self.pwm = match c.u16()? {
            0xffff => None,
            p => Some(p.min(63 << 8)),
        };
        self.clock = c.u64()?;
        Ok(())
    }
}

impl MediaSlot for SonyDrive {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let disk = Disk::parse(media)?;
        if disk.image.sides == 2 && !self.double_sided {
            return Err(MediaError::Format(
                "an 800K disk does not fit the 400K drive".into(),
            ));
        }
        let _ = self.eject();
        self.disk = Some(disk);
        self.pos %= self.track_bits();
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        let mut d = self.disk.take()?;
        let _ = d.write_back();
        Some(d.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.disk {
            Some(d) => {
                d.write_back()?;
                d.media.backend.commit()
            }
            None => Ok(()),
        }
    }

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.motor && self.disk.is_some(),
            error: false,
            track: Some(self.track),
        }
    }
}

/// The IWM with its drives (internal first, then external; the SE's
/// second internal drive third).
pub struct Iwm {
    pub drives: Vec<SonyDrive>,
    /// State bits CA0, CA1, CA2, LSTRB, ENABLE, SELECT, Q6, Q7 (bit n).
    pub lines: u8,
    pub mode: u8,
    /// HEADSEL from the VIA (PA5).
    pub sel: bool,
    /// SE: the VIA's drive select (PA4) picks the second internal drive.
    pub drive_b: bool,
    pub data: u8,
    shift: u8,
    /// Write: the buffer holds a byte; bits left in the shift register.
    write_full: bool,
    write_bits: u8,
    underrun: bool,
    /// Bits of the read shift register (to stay framed across accesses).
    clock: u64,
}

impl std::fmt::Debug for Iwm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Iwm")
            .field("lines", &self.lines)
            .field("mode", &self.mode)
            .field("drives", &self.drives)
            .finish()
    }
}

pub mod line {
    pub const CA0: u8 = 0x01;
    pub const CA1: u8 = 0x02;
    pub const CA2: u8 = 0x04;
    pub const LSTRB: u8 = 0x08;
    pub const ENABLE: u8 = 0x10;
    pub const SELECT: u8 = 0x20;
    pub const Q6: u8 = 0x40;
    pub const Q7: u8 = 0x80;
}

impl Iwm {
    /// `double_sided` per drive.
    pub fn new(drives: &[bool]) -> Self {
        Self {
            drives: drives.iter().map(|&d| SonyDrive::new(d)).collect(),
            lines: 0,
            mode: 0,
            sel: false,
            drive_b: false,
            data: 0,
            shift: 0,
            write_full: false,
            write_bits: 0,
            underrun: false,
            clock: 0,
        }
    }

    /// RESET: the state bits clear.
    pub fn reset(&mut self) {
        self.lines = 0;
        self.mode = 0;
        self.data = 0;
        self.shift = 0;
        self.write_full = false;
        self.write_bits = 0;
        self.underrun = false;
    }

    fn on(&self, bit: u8) -> bool {
        self.lines & bit != 0
    }

    /// Index of the selected drive, if fitted.
    fn selected(&self) -> Option<usize> {
        let i = if self.on(line::SELECT) {
            1
        } else if self.drive_b {
            2
        } else {
            0
        };
        (i < self.drives.len()).then_some(i)
    }

    fn reg(&self) -> u8 {
        (self.lines & 7) << 1 | u8::from(self.sel)
    }

    fn side(&self) -> usize {
        usize::from(self.sel)
    }

    fn writing(&self) -> bool {
        self.on(line::Q7) && self.on(line::Q6) && self.on(line::ENABLE)
    }

    /// Run the drives and the shift register to `now`.
    pub fn run_to(&mut self, now: u64) {
        let enabled = self.on(line::ENABLE);
        let sel = self.selected();
        let side = self.side();
        let writing = self.writing() && self.write_bits > 0;
        let reading = enabled && !self.on(line::Q7);
        self.clock = now;
        for (i, d) in self.drives.iter_mut().enumerate() {
            if Some(i) != sel || !(reading || writing) {
                d.spin_to(now);
                continue;
            }
            // Bit by bit through the shift register.
            let start_pos = d.pos;
            let cells = d.spin_to(now);
            d.pos = start_pos;
            for _ in 0..cells {
                if writing && self.write_bits > 0 {
                    let bit = self.data & 0x80 != 0;
                    self.data <<= 1;
                    d.cell(side, Some(bit));
                    self.write_bits -= 1;
                    if self.write_bits == 0 {
                        if self.write_full {
                            self.write_full = false;
                            self.data = self.shift;
                            self.write_bits = 8;
                        } else {
                            self.underrun = true;
                        }
                    }
                } else if reading {
                    let bit = d.cell(side, None);
                    self.shift = self.shift << 1 | u8::from(bit);
                    if self.shift & 0x80 != 0 {
                        self.data = self.shift;
                        self.shift = 0;
                    }
                } else {
                    d.cell(side, None);
                }
            }
        }
    }

    fn sense(&self, now: u64) -> bool {
        match self.selected() {
            Some(i) => self.drives[i].sense(self.reg(), now, self.side()),
            None => true,
        }
    }

    /// The register Q6/Q7 select, without side effects.
    pub fn peek(&self, now: u64) -> u8 {
        match (self.on(line::Q7), self.on(line::Q6)) {
            (false, false) => {
                if self.on(line::ENABLE) {
                    self.data
                } else {
                    0xff
                }
            }
            (false, true) => {
                u8::from(self.sense(now)) << 7
                    | u8::from(self.on(line::ENABLE)) << 5
                    | (self.mode & 0x1f)
            }
            (true, false) => {
                let empty = !self.write_full || !self.writing();
                u8::from(empty) << 7 | u8::from(!self.underrun) << 6 | 0x3f
            }
            (true, true) => 0xff,
        }
    }

    /// An access to register `n` (0–15) at clock `now`: `write` is the
    /// byte of a write cycle. Returns the byte read.
    pub fn access(&mut self, n: u8, write: Option<u8>, now: u64) -> u8 {
        self.run_to(now);
        let bit = 1 << (n >> 1);
        let was_writing = self.writing();
        let old = self.lines;
        if n & 1 != 0 {
            self.lines |= bit;
        } else {
            self.lines &= !bit;
        }
        // LSTRB's rising edge strobes a drive command.
        if old & line::LSTRB == 0 && self.on(line::LSTRB) {
            if let Some(i) = self.selected() {
                let cmd = (self.lines & 3) << 1 | u8::from(self.sel);
                let ca2 = self.on(line::CA2);
                self.drives[i].command(cmd, ca2, now);
            }
        }
        if bit == line::ENABLE && !self.on(line::ENABLE) {
            self.write_bits = 0;
        }
        let writing = self.writing();
        if was_writing && !writing {
            self.write_bits = 0;
            self.write_full = false;
        }
        if !was_writing && writing {
            self.underrun = false;
            self.write_bits = 0;
            self.write_full = false;
        }
        match write {
            Some(v) if self.on(line::Q7) && self.on(line::Q6) => {
                if self.on(line::ENABLE) {
                    if self.write_bits == 0 && !self.underrun {
                        self.data = v;
                        self.write_bits = 8;
                    } else {
                        self.shift = v;
                        self.write_full = true;
                    }
                } else {
                    self.mode = v & 0x1f;
                }
                0xff
            }
            Some(_) => 0xff,
            None => {
                let v = self.peek(now);
                if !self.on(line::Q7) && !self.on(line::Q6) && self.on(line::ENABLE) {
                    self.data = 0;
                }
                v
            }
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"IWM ", 1, |w| {
            w.u8(self.lines);
            w.u8(self.mode);
            w.bool(self.sel);
            w.bool(self.drive_b);
            w.u8(self.data);
            w.u8(self.shift);
            w.bool(self.write_full);
            w.u8(self.write_bits);
            w.bool(self.underrun);
            w.u64(self.clock);
            w.u8(self.drives.len() as u8);
            for d in &self.drives {
                d.save(w);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"IWM ", 1)?;
        let lines = c.u8()?;
        let mode = c.u8()? & 0x1f;
        let sel = c.bool()?;
        let drive_b = c.bool()?;
        let data = c.u8()?;
        let shift = c.u8()?;
        let write_full = c.bool()?;
        let write_bits = c.u8()?.min(8);
        let underrun = c.bool()?;
        let clock = c.u64()?;
        if usize::from(c.u8()?) != self.drives.len() {
            return Err(StateError::Invalid("iwm drive count differs"));
        }
        // Mechanics into scratch drives first (atomic).
        let mut mech: Vec<SonyDrive> = self
            .drives
            .iter()
            .map(|d| SonyDrive::new(d.double_sided))
            .collect();
        for d in &mut mech {
            d.load(&mut c)?;
        }
        for (d, m) in self.drives.iter_mut().zip(mech) {
            d.track = m.track;
            d.dir_down = m.dir_down;
            d.motor = m.motor;
            d.step_until = m.step_until;
            d.pos = m.pos;
            d.frac = m.frac;
            d.pwm = m.pwm;
            d.clock = m.clock;
        }
        self.lines = lines;
        self.mode = mode;
        self.sel = sel;
        self.drive_b = drive_b;
        self.data = data;
        self.shift = shift;
        self.write_full = write_full;
        self.write_bits = write_bits;
        self.underrun = underrun;
        self.clock = clock;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;
    use macdisk::{ImageKind, BLOCK, SIDE_BLOCKS, TAG};

    struct Mem(Vec<u8>);

    impl MediaBackend for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), MediaError> {
            let o = offset as usize;
            if self.0.len() < o + data.len() {
                self.0.resize(o + data.len(), 0);
            }
            self.0[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn disk_800k() -> Media {
        let img = Image {
            kind: ImageKind::Raw,
            sides: 2,
            data: (0..2 * SIDE_BLOCKS * BLOCK)
                .map(|i| (i / 512) as u8)
                .collect(),
            tags: vec![0; 2 * SIDE_BLOCKS * TAG],
            name: Vec::new(),
        };
        Media {
            name: "test.dsk".into(),
            kind: MediaKind::Disk,
            backend: Box::new(Mem(img.to_bytes())),
        }
    }

    /// Set the drive register lines CA2 CA1 CA0 / SEL to `reg`.
    fn select(iwm: &mut Iwm, reg: u8, now: u64) {
        for (k, n) in [(1u8, 0u8), (2, 2), (3, 4)] {
            let on = reg >> k & 1;
            iwm.access(n + on, None, now);
        }
        iwm.sel = reg & 1 != 0;
    }

    fn sense(iwm: &mut Iwm, reg: u8, now: u64) -> bool {
        select(iwm, reg, now);
        iwm.access(13, None, now); // Q6 on
        let v = iwm.access(14, None, now); // Q7 off: status
        v & 0x80 != 0
    }

    fn strobe(iwm: &mut Iwm, reg: u8, now: u64) {
        select(iwm, reg, now);
        iwm.access(7, None, now);
        iwm.access(6, None, now);
    }

    #[test]
    fn status_and_commands() {
        let mut iwm = Iwm::new(&[true]);
        iwm.access(9, None, 0); // ENABLE
        assert!(sense(&mut iwm, 0x1, 0), "no disk");
        assert!(!sense(&mut iwm, 0xe, 0), "drive installed");
        assert!(sense(&mut iwm, 0xc, 0), "two heads");
        iwm.drives[0].insert(disk_800k()).unwrap();
        assert!(!sense(&mut iwm, 0x1, 0), "disk in");
        assert!(!sense(&mut iwm, 0x5, 0), "track 0");
        assert!(sense(&mut iwm, 0x4, 0), "motor off");
        strobe(&mut iwm, 0x4, 0); // MOTORON command (CA1 = 1) with CA2 = 0
        assert!(!sense(&mut iwm, 0x4, 0), "motor on");
        strobe(&mut iwm, 0x2, 0); // STEP (CA0 = 1) with CA2 = 0, direction up
        assert_eq!(iwm.drives[0].track, 1);
        assert!(!sense(&mut iwm, 0x2, 10), "stepping");
        assert!(sense(&mut iwm, 0x2, 10 + STEP_CLOCKS), "step done");
        // Mode register: Q6 and Q7 on, ENABLE off, write.
        iwm.access(8, None, 0);
        iwm.access(15, Some(0x1f), 0);
        iwm.access(14, None, 0);
        assert_eq!(iwm.access(13, None, 0) & 0x1f, 0x1f);
    }

    #[test]
    fn reads_address_fields_from_the_disk() {
        let mut iwm = Iwm::new(&[true]);
        iwm.drives[0].insert(disk_800k()).unwrap();
        iwm.access(9, None, 0);
        strobe(&mut iwm, 0x4, 0);
        select(&mut iwm, 0x0, 0);
        iwm.access(12, None, 0); // Q6 off
        iwm.access(14, None, 0); // Q7 off: data
        let mut bytes = Vec::new();
        let mut t = 0;
        while bytes.len() < 2000 {
            t += 20;
            let v = iwm.access(14, None, t);
            if v & 0x80 != 0 {
                bytes.push(v);
            }
        }
        let found = bytes
            .windows(3)
            .filter(|w| *w == [0xd5, 0xaa, 0x96])
            .count();
        assert!(found >= 2, "address prologues in {bytes:02x?}");
    }

    #[test]
    fn savestate_keeps_mechanics() {
        let mut iwm = Iwm::new(&[true, false]);
        iwm.drives[1].track = 7;
        iwm.lines = 0x55;
        let mut w = StateWriter::new();
        iwm.save(&mut w);
        let bytes = w.into_bytes();
        let mut other = Iwm::new(&[true, false]);
        other.load(&mut StateReader::new(&bytes)).unwrap();
        assert_eq!(other.drives[1].track, 7);
        assert_eq!(other.lines, 0x55);
        assert!(Iwm::new(&[true])
            .load(&mut StateReader::new(&bytes))
            .is_err());
    }
}
