//! WD1772 floppy disk controller (MFM, double density), as in the 1581.
//!
//! Registers (A1/A0): 0 status (read) / command (write), 1 track, 2 sector,
//! 3 data. Commands: type I `$0x` restore, `$1x` seek, `$2x`/`$3x` step,
//! `$4x`/`$5x` step in, `$6x`/`$7x` step out (bit 4 u: update the track
//! register, bit 3 h: no spin-up, bit 2 V: verify, bits 1–0: step rate 6,
//! 12, 2, 3 ms); type II `$8x` read sector, `$Ax` write sector (bit 4 m:
//! multiple sectors, bit 2 E: 15 ms settle delay, bit 0 a0: deleted data
//! mark); type III `$C0` read address, `$E0` read track, `$F0` write track;
//! type IV `$D0` force interrupt.
//!
//! Status after type I: 7 motor on, 6 write protect, 5 spin-up done, 4 seek
//! error, 3 CRC error, 2 track 0, 1 index, 0 busy. After types II/III:
//! 7 motor on, 6 write protect, 5 record type (deleted mark), 4 record not
//! found, 3 CRC error, 2 lost data, 1 DRQ, 0 busy.
//!
//! The disk side is a decoded MFM byte stream per track ([`TRACK_BYTES`]
//! bytes, one every [`BYTE_US`] µs at 300 rpm); sync bytes written with a
//! missing clock (`$F5` → `$A1`, `$F6` → `$C2` in write track) carry
//! [`MARK`]. ID fields are `$A1 $A1 $A1 $FE C H R N CRC CRC`, data fields
//! `$A1 $A1 $A1 $FB data CRC CRC`; the CRC is CCITT (x^16+x^12+x^5+1,
//! preset `$FFFF`) over the marks, the address mark and the field. The
//! 1772 does not compare the side byte. Source: Western Digital WD1770/
//! 1772 data sheet.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

pub use media::mfm::{crc16, MARK, TRACK_BYTES};

/// Microseconds per byte.
pub const BYTE_US: u32 = 32;

const BUSY: u8 = 0x01;
const DRQ: u8 = 0x02;
const LOST: u8 = 0x04;
const CRC_ERR: u8 = 0x08;
const RNF: u8 = 0x10;
const SPIN_UP: u8 = 0x20;
const RECORD_TYPE: u8 = 0x20;
const WRITE_PROTECT: u8 = 0x40;
const MOTOR: u8 = 0x80;

/// Step rates of the 1772 in µs.
const STEP_US: [u32; 4] = [6_000, 12_000, 2_000, 3_000];
const SETTLE_US: u32 = 15_000;
/// Time from the command write to its start; even a command with nothing
/// to do keeps BUSY set this long (the DOS polls for BUSY after writing).
const START_US: u32 = BYTE_US;

/// What the controller sees of the drive.
pub trait FloppyIo {
    /// The byte under the head now; `None` without flux (no disk, motor
    /// stopped, unformatted track).
    fn read(&self) -> Option<u16>;
    /// Write a byte at the head now.
    fn write(&mut self, byte: u16);
    /// One step pulse.
    fn step(&mut self, outward: bool);
    fn track0(&self) -> bool;
    fn protected(&self) -> bool;
}

/// What happens at the head in this microsecond.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pulse {
    /// A new byte is under the head.
    pub byte: bool,
    /// The index hole passes (rising edge of the index pulse).
    pub index: bool,
    /// The index pulse is active (for the type I status).
    pub index_active: bool,
}

/// CRC of the three sync marks and an address mark.
fn crc_after_mark(mark: u8) -> u16 {
    [0xa1, 0xa1, 0xa1, mark].into_iter().fold(0xffff, crc16)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// The command register was written: the controller takes it up after
    /// a short while (BUSY is already set).
    Start,
    /// Waiting for six index pulses after switching the motor on.
    SpinUp,
    /// Type I: decide on the next step.
    Step,
    /// Type I: waiting out the step rate.
    StepWait,
    /// The 15 ms head settle delay (type I verify, E flag).
    Settle,
    /// Searching ID fields (verify, read/write sector, read address).
    FindId,
    /// Collecting the six bytes after an ID address mark.
    IdField,
    /// Read sector: waiting for the data address mark.
    FindData,
    ReadData,
    /// Write sector: the gap between ID and data field.
    WriteGap,
    WriteData,
    /// Read address: handing out the six ID bytes.
    ReadAddress,
    /// Read/write track: waiting for the index pulse.
    WaitIndex,
    ReadTrack,
    WriteTrack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wd177x {
    status: u8,
    pub track: u8,
    pub sector: u8,
    pub data: u8,
    command: u8,
    /// The status register shows the type I bits.
    type1: bool,
    motor: bool,
    /// Index pulses since the controller went idle (motor off after 9).
    idle_index: u8,
    index_active: bool,
    phase: Phase,
    /// µs left in a delay, or bytes counted in a field.
    count: u32,
    /// Index pulses during a search.
    index_count: u8,
    /// Consecutive sync marks just seen.
    marks: u8,
    crc: u16,
    /// The ID field being read (C H R N CRC CRC).
    id: [u8; 6],
    /// Last step direction: towards the hub (inward).
    inward: bool,
    /// Write track: the low CRC byte goes out in the next byte time.
    crc_low_pending: bool,
    /// Step commands: the one step is taken.
    stepped: bool,
}

impl Default for Wd177x {
    fn default() -> Self {
        Self::new()
    }
}

impl Wd177x {
    pub fn new() -> Self {
        Self {
            status: 0,
            track: 0,
            sector: 1,
            data: 0,
            command: 0,
            type1: true,
            motor: false,
            idle_index: 0,
            index_active: false,
            phase: Phase::Idle,
            count: 0,
            index_count: 0,
            marks: 0,
            crc: 0xffff,
            id: [0; 6],
            inward: true,
            crc_low_pending: false,
            stepped: false,
        }
    }

    /// /MR: registers and state to power-on values.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn busy(&self) -> bool {
        self.status & BUSY != 0
    }

    pub fn drq(&self) -> bool {
        !self.type1 && self.status & DRQ != 0
    }

    /// Motor-on output.
    pub fn motor(&self) -> bool {
        self.motor
    }

    fn status_now(&self, io: &dyn FloppyIo) -> u8 {
        let mut s = self.status & !(MOTOR | WRITE_PROTECT);
        if self.motor {
            s |= MOTOR;
        }
        if io.protected() {
            s |= WRITE_PROTECT;
        }
        if self.type1 {
            s &= !(LOST | DRQ);
            if io.track0() {
                s |= LOST;
            }
            if self.index_active {
                s |= DRQ;
            }
        }
        s
    }

    /// Side-effect-free register read.
    pub fn peek(&self, reg: u8, io: &dyn FloppyIo) -> u8 {
        match reg & 3 {
            0 => self.status_now(io),
            1 => self.track,
            2 => self.sector,
            _ => self.data,
        }
    }

    /// Register read: the data register clears DRQ.
    pub fn read(&mut self, reg: u8, io: &dyn FloppyIo) -> u8 {
        let v = self.peek(reg, io);
        if reg & 3 == 3 {
            self.status &= !DRQ;
        }
        v
    }

    pub fn write(&mut self, reg: u8, v: u8) {
        match reg & 3 {
            0 => self.command(v),
            1 if !self.busy() => self.track = v,
            2 if !self.busy() => self.sector = v,
            3 => {
                self.data = v;
                self.status &= !DRQ;
            }
            _ => {}
        }
    }

    fn command(&mut self, cmd: u8) {
        if cmd & 0xf0 == 0xd0 {
            // Force interrupt: stop; when idle the status shows type I bits.
            if !self.busy() {
                self.type1 = true;
            }
            self.status &= !BUSY;
            self.phase = Phase::Idle;
            return;
        }
        if self.busy() {
            return;
        }
        self.command = cmd;
        self.type1 = cmd & 0x80 == 0;
        self.status = BUSY;
        self.idle_index = 0;
        self.count = START_US;
        self.phase = Phase::Start;
    }

    /// The command begins: spin-up first if the motor was off and h is 0.
    fn begin(&mut self, io: &dyn FloppyIo) {
        let spin_up = self.command & 0x08 == 0 && !self.motor;
        self.motor = true;
        if spin_up {
            self.index_count = 0;
            self.phase = Phase::SpinUp;
        } else {
            self.start(io);
        }
    }

    fn start(&mut self, io: &dyn FloppyIo) {
        let cmd = self.command;
        if self.type1 {
            self.status |= SPIN_UP;
            match cmd >> 4 {
                0 => {
                    self.track = 0xff;
                    self.data = 0;
                }
                4 | 5 => self.inward = true,
                6 | 7 => self.inward = false,
                _ => {}
            }
            self.stepped = false;
            self.phase = Phase::Step;
            return;
        }
        let writes = matches!(cmd >> 4, 0xa | 0xb | 0xf);
        if writes && io.protected() {
            self.status |= WRITE_PROTECT;
            self.done();
            return;
        }
        if cmd & 0x04 != 0 {
            self.delay(SETTLE_US);
        } else {
            self.after_settle();
        }
    }

    fn delay(&mut self, us: u32) {
        self.count = us;
        self.phase = Phase::Settle;
    }

    fn after_settle(&mut self) {
        match self.command >> 4 {
            // Type I verify, type II, read address: look for an ID.
            0..=0xc => self.find_id(),
            0xe => self.phase = Phase::WaitIndex,
            0xf => {
                // Write track: the first byte must come within 3 bytes.
                self.status |= DRQ;
                self.count = 3;
                self.phase = Phase::WaitIndex;
            }
            _ => self.done(),
        }
    }

    fn find_id(&mut self) {
        self.index_count = 0;
        self.marks = 0;
        self.phase = Phase::FindId;
    }

    fn done(&mut self) {
        self.status &= !BUSY;
        self.phase = Phase::Idle;
        self.idle_index = 0;
    }

    /// One microsecond.
    pub fn tick(&mut self, io: &mut dyn FloppyIo, pulse: Pulse) {
        self.index_active = pulse.index_active;
        if pulse.index && self.phase == Phase::Idle && self.motor {
            self.idle_index += 1;
            if self.idle_index >= 9 {
                self.motor = false;
            }
        }
        match self.phase {
            Phase::Idle => {}
            Phase::Start => {
                self.count = self.count.saturating_sub(1);
                if self.count == 0 {
                    self.begin(io);
                }
            }
            Phase::SpinUp => {
                if pulse.index {
                    self.index_count += 1;
                    if self.index_count >= 6 {
                        self.start(io);
                    }
                }
            }
            Phase::Step => self.step(io),
            Phase::StepWait | Phase::Settle => {
                self.count = self.count.saturating_sub(1);
                if self.count == 0 {
                    if self.phase == Phase::StepWait {
                        self.phase = Phase::Step;
                    } else {
                        self.after_settle();
                    }
                }
            }
            _ => self.disk_phase(io, pulse),
        }
    }

    /// Type I: the next step, or the end of stepping.
    fn step(&mut self, io: &mut dyn FloppyIo) {
        let cmd = self.command >> 4;
        let seek = cmd <= 1;
        if seek {
            if self.track == self.data {
                return self.end_stepping();
            }
            self.inward = self.data > self.track;
        } else if self.stepped {
            // Step commands move once.
            return self.end_stepping();
        }
        if !self.inward && io.track0() {
            self.track = 0;
            return self.end_stepping();
        }
        if seek || self.command & 0x10 != 0 {
            self.track = if self.inward {
                self.track.wrapping_add(1)
            } else {
                self.track.wrapping_sub(1)
            };
        }
        io.step(!self.inward);
        self.stepped = true;
        self.count = STEP_US[usize::from(self.command & 3)];
        self.phase = Phase::StepWait;
    }

    fn end_stepping(&mut self) {
        if self.command & 0x04 != 0 {
            self.delay(SETTLE_US);
        } else {
            self.done();
        }
    }

    /// Phases that follow the bytes under the head.
    fn disk_phase(&mut self, io: &mut dyn FloppyIo, pulse: Pulse) {
        if pulse.index {
            self.index_count = self.index_count.saturating_add(1);
        }
        match self.phase {
            Phase::FindId => {
                // Five revolutions without a matching ID: record not found.
                if self.index_count >= 6 {
                    self.status |= RNF;
                    return self.done();
                }
                if pulse.byte {
                    self.scan_id(io.read());
                }
            }
            Phase::IdField => {
                if pulse.byte {
                    self.id_byte(io.read().unwrap_or(0) as u8);
                }
            }
            Phase::FindData => {
                if pulse.byte {
                    self.scan_data(io.read());
                }
            }
            Phase::ReadData => {
                if pulse.byte {
                    self.read_data(io.read().unwrap_or(0) as u8);
                }
            }
            Phase::WriteGap => {
                if pulse.byte {
                    self.write_gap(io);
                }
            }
            Phase::WriteData => {
                if pulse.byte {
                    self.write_data(io);
                }
            }
            Phase::ReadAddress => {
                if pulse.byte {
                    self.read_address_byte(io.read().unwrap_or(0) as u8);
                }
            }
            Phase::WaitIndex => {
                if self.command >> 4 == 0xf && pulse.byte && self.count > 0 {
                    self.count -= 1;
                    if self.count == 0 && self.status & DRQ != 0 {
                        self.status |= LOST;
                        return self.done();
                    }
                }
                if pulse.index {
                    self.crc = 0xffff;
                    self.marks = 0;
                    self.crc_low_pending = false;
                    self.phase = if self.command >> 4 == 0xf {
                        Phase::WriteTrack
                    } else {
                        Phase::ReadTrack
                    };
                    // The first byte after the index belongs to the track.
                    if pulse.byte {
                        self.track_byte(io);
                    }
                }
            }
            Phase::ReadTrack | Phase::WriteTrack => {
                if pulse.index {
                    return self.done();
                }
                if pulse.byte {
                    self.track_byte(io);
                }
            }
            _ => {}
        }
    }

    fn scan_id(&mut self, byte: Option<u16>) {
        match byte {
            Some(b) if b == (MARK | 0xa1) => self.marks = self.marks.saturating_add(1),
            Some(0xfe) if self.marks > 0 => {
                self.marks = 0;
                self.crc = crc_after_mark(0xfe);
                self.count = 0;
                // Read address hands the ID bytes out as they pass.
                self.phase = if !self.type1 && self.command >> 4 == 0xc {
                    Phase::ReadAddress
                } else {
                    Phase::IdField
                };
            }
            _ => self.marks = 0,
        }
    }

    fn id_byte(&mut self, b: u8) {
        let i = self.count as usize;
        self.id[i] = b;
        if i < 4 {
            self.crc = crc16(self.crc, b);
        }
        self.count += 1;
        if self.count < 6 {
            return;
        }
        let crc_ok = u16::from_be_bytes([self.id[4], self.id[5]]) == self.crc;
        let cmd = self.command >> 4;
        if self.type1 {
            // Verify: an ID of the track register's track.
            if self.id[0] == self.track {
                if crc_ok {
                    self.status &= !CRC_ERR;
                    return self.done();
                }
                self.status |= CRC_ERR;
            }
            self.phase = Phase::FindId;
            return;
        }
        if self.id[0] != self.track || self.id[2] != self.sector {
            self.phase = Phase::FindId;
            return;
        }
        if !crc_ok {
            self.status |= CRC_ERR;
            self.phase = Phase::FindId;
            return;
        }
        self.status &= !CRC_ERR;
        self.marks = 0;
        self.count = 0;
        self.phase = if cmd <= 9 {
            Phase::FindData
        } else {
            Phase::WriteGap
        };
    }

    fn sector_bytes(&self) -> u32 {
        128 << (self.id[3] & 3)
    }

    fn scan_data(&mut self, byte: Option<u16>) {
        self.count += 1;
        if self.count > 43 {
            // No data mark after the ID: keep looking for the ID.
            return self.find_id_again();
        }
        match byte {
            Some(b) if b == (MARK | 0xa1) => self.marks = self.marks.saturating_add(1),
            Some(m @ (0xfb | 0xf8)) if self.marks > 0 => {
                if m == 0xf8 {
                    self.status |= RECORD_TYPE;
                }
                self.crc = crc_after_mark(m as u8);
                self.count = 0;
                self.phase = Phase::ReadData;
            }
            _ => self.marks = 0,
        }
    }

    fn find_id_again(&mut self) {
        self.marks = 0;
        self.phase = Phase::FindId;
    }

    fn read_data(&mut self, b: u8) {
        let n = self.sector_bytes();
        if self.count < n {
            if self.status & DRQ != 0 {
                self.status |= LOST;
            }
            self.data = b;
            self.status |= DRQ;
            self.crc = crc16(self.crc, b);
        } else if self.count == n {
            self.id[4] = b;
        } else {
            let ok = u16::from_be_bytes([self.id[4], b]) == self.crc;
            if !ok {
                self.status |= CRC_ERR;
                return self.done();
            }
            return self.next_sector();
        }
        self.count += 1;
    }

    /// Multiple-sector commands go on with the next sector number.
    fn next_sector(&mut self) {
        if self.command & 0x10 != 0 {
            self.sector = self.sector.wrapping_add(1);
            self.find_id();
        } else {
            self.done();
        }
    }

    /// Write sector, between the ID field and the data field: DRQ after 2
    /// bytes, the first data byte needed by byte 11, writing from byte 22.
    fn write_gap(&mut self, io: &mut dyn FloppyIo) {
        self.count += 1;
        match self.count {
            2 => self.status |= DRQ,
            11 if self.status & DRQ != 0 => {
                self.status |= LOST;
                self.done();
            }
            22 => {
                // 12 × $00, 3 sync marks, the data mark.
                self.count = 0;
                self.phase = Phase::WriteData;
                self.write_data(io);
            }
            _ => {}
        }
    }

    fn write_data(&mut self, io: &mut dyn FloppyIo) {
        let n = self.sector_bytes();
        let i = self.count;
        let mark = if self.command & 1 != 0 { 0xf8 } else { 0xfb };
        match i {
            0..=11 => io.write(0),
            12..=14 => io.write(MARK | 0xa1),
            15 => {
                io.write(mark);
                self.crc = crc_after_mark(mark as u8);
            }
            _ if i < 16 + n => {
                let b = if self.status & DRQ != 0 {
                    self.status |= LOST;
                    0
                } else {
                    self.data
                };
                io.write(u16::from(b));
                self.crc = crc16(self.crc, b);
                if i + 1 < 16 + n {
                    self.status |= DRQ;
                }
            }
            _ if i == 16 + n => io.write(self.crc >> 8),
            _ if i == 17 + n => io.write(self.crc & 0xff),
            _ => {
                io.write(0x4e);
                return self.next_sector();
            }
        }
        self.count += 1;
    }

    fn read_address_byte(&mut self, b: u8) {
        let i = self.count as usize;
        self.count += 1;
        if i < 6 {
            self.id[i] = b;
            if i < 4 {
                self.crc = crc16(self.crc, b);
            }
            if self.status & DRQ != 0 {
                self.status |= LOST;
            }
            self.data = b;
            self.status |= DRQ;
            return;
        }
        // The command ends a byte time after the last ID byte; the track of
        // the ID goes into the sector register.
        self.sector = self.id[0];
        if u16::from_be_bytes([self.id[4], self.id[5]]) != self.crc {
            self.status |= CRC_ERR;
        }
        self.done();
    }

    /// Read track hands out every byte; write track takes one per byte
    /// time and translates `$F5`–`$F7`.
    fn track_byte(&mut self, io: &mut dyn FloppyIo) {
        if self.command >> 4 == 0xe {
            if self.status & DRQ != 0 {
                self.status |= LOST;
            }
            self.data = io.read().unwrap_or(0) as u8;
            self.status |= DRQ;
            return;
        }
        if self.crc_low_pending {
            self.crc_low_pending = false;
            io.write(self.crc & 0xff);
            self.status |= DRQ;
            return;
        }
        let b = if self.status & DRQ != 0 {
            self.status |= LOST;
            0
        } else {
            self.data
        };
        if b != 0xf5 {
            self.marks = 0;
        }
        match b {
            0xf5 => {
                // The first of a run of marks presets the CRC.
                io.write(MARK | 0xa1);
                if self.marks == 0 {
                    self.crc = 0xffff;
                }
                self.crc = crc16(self.crc, 0xa1);
                self.marks += 1;
                self.status |= DRQ;
                return;
            }
            0xf6 => io.write(MARK | 0xc2),
            0xf7 => {
                io.write(self.crc >> 8);
                self.crc_low_pending = true;
                return;
            }
            b => {
                io.write(u16::from(b));
                self.crc = crc16(self.crc, b);
            }
        }
        self.status |= DRQ;
    }
}

const TAG: [u8; 4] = *b"W177";
const VERSION: u16 = 1;

fn phase_code(p: Phase) -> u8 {
    p as u8
}

fn phase_from(c: u8) -> Option<Phase> {
    use Phase::*;
    [
        Idle,
        Start,
        SpinUp,
        Step,
        StepWait,
        Settle,
        FindId,
        IdField,
        FindData,
        ReadData,
        WriteGap,
        WriteData,
        ReadAddress,
        WaitIndex,
        ReadTrack,
        WriteTrack,
    ]
    .get(usize::from(c))
    .copied()
}

impl Savestate for Wd177x {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(self.status);
            w.u8(self.track);
            w.u8(self.sector);
            w.u8(self.data);
            w.u8(self.command);
            w.bool(self.type1);
            w.bool(self.motor);
            w.u8(self.idle_index);
            w.bool(self.index_active);
            w.u8(phase_code(self.phase));
            w.u32(self.count);
            w.u8(self.index_count);
            w.u8(self.marks);
            w.u16(self.crc);
            w.raw(&self.id);
            w.bool(self.inward);
            w.bool(self.crc_low_pending);
            w.bool(self.stepped);
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = Self::new();
        n.status = c.u8()?;
        n.track = c.u8()?;
        n.sector = c.u8()?;
        n.data = c.u8()?;
        n.command = c.u8()?;
        n.type1 = c.bool()?;
        n.motor = c.bool()?;
        n.idle_index = c.u8()?;
        n.index_active = c.bool()?;
        n.phase = phase_from(c.u8()?).ok_or(StateError::Invalid("WD1772 phase"))?;
        n.count = c.u32()?;
        n.index_count = c.u8()?;
        n.marks = c.u8()?;
        n.crc = c.u16()?;
        c.raw_into(&mut n.id)?;
        n.inward = c.bool()?;
        n.crc_low_pending = c.bool()?;
        n.stepped = c.bool()?;
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
