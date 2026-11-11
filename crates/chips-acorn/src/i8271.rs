//! Intel 8271 floppy disc controller (BBC Micro Model B), at sector level,
//! in non-DMA mode.
//!
//! **Registers** (A1 A0; DACK selects the data register, on the BBC the
//! address bit 2): read 0 status, 1 result; write 0 command, 1 parameter,
//! 2 reset (1 then 0); data read and written with DACK.
//!
//! **Status**: bit 7 command busy, 6 command register full, 5 parameter
//! register full, 4 result register full, 3 interrupt request (the INT
//! pin), 2 non-DMA data request. INT stays until the result is read; in
//! non-DMA mode every data byte raises it too, and moving the byte clears
//! it.
//!
//! **Commands** (bits 7–6 select drive 0 or 1, bits 5–0 the opcode, with
//! their parameter count): 128-byte records `&12` read, `&16` read with
//! deleted data, `&0A` write, `&0E` write deleted, `&1E` verify (track,
//! sector); variable records `&13`, `&17`, `&0B`, `&0F`, `&1F` (track,
//! sector, length bits 7–5 = 128 << n and count bits 4–0); `&1B` read ID
//! (track, 0, count: four bytes C H R N per ID from the index on); `&23`
//! format (track, gap 3, length/count, gap 5, gap 1; then C H R N per
//! sector through data requests, the data fields filled with `&E5`); `&29`
//! seek (track); `&2C` read drive status; `&35` specify (`&0D` step rate,
//! settling time, index count/head load time; `&10`/`&18` bad tracks 1 and
//! 2 and the current track of surface 0/1); `&3A` write special register
//! (register, value); `&3D` read special register (register); `&00`/`&04`
//! scan (not modelled: "scan not met"). Read drive status and read special
//! register give an immediate result without an interrupt; specify and
//! write special register give none.
//!
//! **Results**: bits 4–3 the completion type, 2–1 the code, bit 5 deleted
//! data found: `&00` good, `&08` clock error, `&0A` late DMA (a byte not
//! moved in time), `&0C` ID CRC, `&0E` data CRC, `&10` drive not ready,
//! `&12` write protect, `&14` track 0 not found, `&16` write fault, `&18`
//! sector not found.
//!
//! **Special registers**: `&06` scan sector, `&12`/`&1A` current track of
//! surface 0/1, `&10`/`&11` and `&18`/`&19` bad tracks, `&17` mode, `&22`
//! the drive control input port, `&23` the drive control output port
//! (bit 0 write enable, 1 seek/step, 2 direction, 3 load head, 4 low head
//! current, 5 write fault reset/OPO, 6 select 0, 7 select 1; the BBC takes
//! OPO as the side select). Input port and drive status: bit 1 track 0,
//! 2 ready 0, 3 write protect, 4 index, 5 write fault, 6 ready 1 (the bit
//! order is the commonly documented one; the handbook's figure was not
//! legible in the scan used).
//!
//! **Timing** (2 MHz clocks): single density, a byte every 64 µs; 300 rpm;
//! steps at the specified rate, then the settling time; a sector is found
//! when it turns under the head (sectors evenly spread from the index); a
//! missing sector after two revolutions. Bad tracks shift the physical
//! track a logical one maps to.
//!
//! Source: Intel, *Peripheral Design Handbook* (1979), "8271/8271-6/8271-8
//! Programmable Floppy Disk Controller" (pp. 1-108 to 1-130): registers,
//! status, result codes, command opcodes and parameters, special registers,
//! the drive control ports, track format. The implementation is this
//! project's own.

use crate::wd1772::Floppy;
use emu_core::{StateError, StateReader, StateWriter};

/// 2 MHz clocks per millisecond.
const MS: u32 = 2000;
/// A single-density byte (64 µs).
const BYTE: u32 = 128;
/// One revolution at 300 rpm.
const REVOLUTION: u32 = 400_000;
/// The index hole passes the sensor for about 4 ms.
const INDEX: u32 = 8000;
/// ID field, gap 2 and the data address mark before the first data byte.
const ID_TO_DATA: u32 = 30 * BYTE;

pub const ST_BUSY: u8 = 0x80;
pub const ST_COMMAND_FULL: u8 = 0x40;
pub const ST_PARAMETER_FULL: u8 = 0x20;
pub const ST_RESULT_FULL: u8 = 0x10;
pub const ST_INT: u8 = 0x08;
pub const ST_DATA_REQUEST: u8 = 0x04;

pub const R_GOOD: u8 = 0x00;
pub const R_LATE: u8 = 0x0a;
pub const R_NOT_READY: u8 = 0x10;
pub const R_WRITE_PROTECT: u8 = 0x12;
pub const R_NOT_FOUND: u8 = 0x18;

/// Parameters a command takes (by opcode, bits 5–0).
fn parameters(op: u8) -> usize {
    match op {
        0x00 | 0x04 | 0x23 => 5,
        0x35 => 4,
        0x0b | 0x0f | 0x13 | 0x17 | 0x1f | 0x1b => 3,
        0x0a | 0x0e | 0x12 | 0x16 | 0x1e | 0x3a => 2,
        0x29 | 0x3d => 1,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    /// Collecting parameters.
    Parameters,
    /// Stepping and settling.
    Seek,
    /// Waiting for the sector to come round (or the search to fail).
    Search,
    Read,
    Write,
    Verify,
    ReadId,
    Format,
    /// The result follows when the timer runs out.
    Finish,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct I8271 {
    command: u8,
    params: [u8; 5],
    count_params: usize,
    busy: bool,
    command_full: bool,
    result: u8,
    result_full: bool,
    /// The INT pin.
    pub int: bool,
    drq: bool,
    data: u8,
    phase: Phase,
    timer: u32,
    /// The operation: drive, logical track, sector, sectors left, record
    /// size, bytes moved, the result it ends with.
    drive: usize,
    track: u8,
    sector: u8,
    left: u8,
    size: usize,
    pos: usize,
    buf: Vec<u8>,
    end: u8,
    /// Physical head position per drive (the current track registers).
    head: [u8; 2],
    bad: [[u8; 2]; 2],
    step_rate: u8,
    settle: u8,
    load: u8,
    mode: u8,
    output: u8,
    scan_sector: u8,
    reset_line: bool,
    /// Clocks since the index hole.
    angle: u32,
}

impl I8271 {
    pub fn new() -> Self {
        Self {
            head: [0; 2],
            bad: [[0xff; 2]; 2],
            ..Self::default()
        }
    }

    /// The reset pin or register: the command ends, the flags clear, the
    /// drive outputs go low.
    pub fn reset(&mut self) {
        let keep = (self.head, self.bad, self.angle, self.reset_line);
        *self = Self::new();
        (self.head, self.bad, self.angle, self.reset_line) = keep;
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// The side the board's OPO line selects.
    pub fn side(&self) -> u32 {
        u32::from(self.output & 0x20 != 0)
    }

    /// The head's track of a drive.
    pub fn head(&self, drive: usize) -> u8 {
        self.head[drive & 1]
    }

    /// The selected drive (`None`: neither select line).
    pub fn selected(&self) -> Option<usize> {
        match self.output >> 6 {
            1 | 3 => Some(0),
            2 => Some(1),
            _ => None,
        }
    }

    pub fn status(&self) -> u8 {
        (if self.busy { ST_BUSY } else { 0 })
            | (if self.command_full {
                ST_COMMAND_FULL
            } else {
                0
            })
            | (if self.result_full { ST_RESULT_FULL } else { 0 })
            | (if self.int { ST_INT } else { 0 })
            | (if self.drq { ST_DATA_REQUEST } else { 0 })
    }

    /// A read at `addr` (bit 2: the data register, else bits 1–0).
    pub fn read(&mut self, addr: u16) -> u8 {
        if addr & 4 != 0 {
            self.drq = false;
            self.int = false;
            return self.data;
        }
        match addr & 3 {
            0 => self.status(),
            1 => {
                self.result_full = false;
                self.int = false;
                self.result
            }
            _ => 0,
        }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        if addr & 4 != 0 {
            return self.data;
        }
        match addr & 3 {
            0 => self.status(),
            1 => self.result,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u16, v: u8, disks: &mut [Option<Floppy>; 2]) {
        if addr & 4 != 0 {
            self.data = v;
            self.drq = false;
            self.int = false;
            return;
        }
        match addr & 3 {
            0 => {
                self.command = v;
                self.command_full = true;
                self.busy = true;
                self.result_full = false;
                self.int = false;
                self.drq = false;
                self.count_params = 0;
                self.phase = Phase::Parameters;
                if parameters(v & 0x3f) == 0 {
                    self.start(disks);
                }
            }
            1 => {
                if self.phase == Phase::Parameters && self.count_params < 5 {
                    self.params[self.count_params] = v;
                    self.count_params += 1;
                    if self.count_params >= parameters(self.command & 0x3f) {
                        self.start(disks);
                    }
                }
            }
            2 => {
                let on = v & 1 != 0;
                if on && !self.reset_line {
                    self.reset();
                }
                self.reset_line = on;
            }
            _ => {}
        }
    }

    /// The drive control input port.
    fn input_port(&self, disks: &[Option<Floppy>; 2]) -> u8 {
        let d = self.selected().unwrap_or(self.drive);
        let disk = disks[d].as_ref();
        u8::from(self.head[d] == 0) << 1
            | u8::from(disks[0].is_some()) << 2
            | u8::from(disk.is_some_and(|f| f.write_protected)) << 3
            | u8::from(disk.is_some() && self.angle < INDEX) << 4
            | u8::from(disks[1].is_some()) << 6
    }

    fn read_special(&self, reg: u8, disks: &[Option<Floppy>; 2]) -> u8 {
        match reg {
            0x06 => self.scan_sector,
            0x12 => self.head[0],
            0x1a => self.head[1],
            0x10 | 0x11 => self.bad[0][usize::from(reg & 1)],
            0x18 | 0x19 => self.bad[1][usize::from(reg & 1)],
            0x17 => self.mode,
            0x22 => self.input_port(disks),
            0x23 => self.output,
            _ => 0,
        }
    }

    fn write_special(&mut self, reg: u8, v: u8) {
        match reg {
            0x06 => self.scan_sector = v,
            0x12 => self.head[0] = v,
            0x1a => self.head[1] = v,
            0x10 | 0x11 => self.bad[0][usize::from(reg & 1)] = v,
            0x18 | 0x19 => self.bad[1][usize::from(reg & 1)] = v,
            0x17 => self.mode = v,
            0x23 => self.output = v,
            _ => {}
        }
    }

    /// The physical track a logical one lies on (bad tracks skipped).
    fn physical(&self, drive: usize, track: u8) -> u8 {
        if track == 0 {
            return 0;
        }
        let mut bad = self.bad[drive];
        bad.sort_unstable();
        let mut p = track;
        for b in bad {
            if b != 0xff && b <= p {
                p = p.saturating_add(1);
            }
        }
        p
    }

    fn immediate(&mut self, v: u8) {
        self.result = v;
        self.result_full = true;
        self.busy = false;
        self.command_full = false;
        self.phase = Phase::Idle;
    }

    fn quiet(&mut self) {
        self.busy = false;
        self.command_full = false;
        self.phase = Phase::Idle;
    }

    fn finish(&mut self, result: u8) {
        self.result = result;
        self.result_full = true;
        self.int = true;
        self.drq = false;
        self.busy = false;
        self.command_full = false;
        self.phase = Phase::Idle;
        self.output &= !0x01;
    }

    /// Command and parameters are in: begin.
    fn start(&mut self, disks: &mut [Option<Floppy>; 2]) {
        self.command_full = false;
        let op = self.command & 0x3f;
        let p = self.params;
        // The command's select bits drive the select lines.
        if self.command & 0xc0 != 0 {
            self.output = (self.output & 0x3f) | (self.command & 0xc0);
        }
        self.drive = self.selected().unwrap_or(0);
        match op {
            0x35 => {
                match p[0] {
                    0x10 | 0x18 => {
                        let s = usize::from(p[0] == 0x18);
                        self.bad[s] = [p[1], p[2]];
                        self.head[s] = p[3];
                    }
                    _ => {
                        (self.step_rate, self.settle, self.load) = (p[1], p[2], p[3]);
                    }
                }
                self.quiet();
            }
            0x3a => {
                self.write_special(p[0], p[1]);
                self.quiet();
            }
            0x3d => {
                let v = self.read_special(p[0], disks);
                self.immediate(v);
            }
            0x2c => {
                let v = self.input_port(disks);
                self.immediate(v);
            }
            0x29 => self.seek(p[0]),
            0x0a | 0x0b | 0x0e | 0x0f | 0x12 | 0x13 | 0x16 | 0x17 | 0x1b | 0x1e | 0x1f | 0x23
            | 0x00 | 0x04 => {
                let Some(disk) = disks[self.drive].as_ref() else {
                    self.phase = Phase::Finish;
                    self.timer = BYTE;
                    self.end = R_NOT_READY;
                    return;
                };
                let writes = matches!(op, 0x0a | 0x0b | 0x0e | 0x0f | 0x23);
                if writes && disk.write_protected {
                    self.phase = Phase::Finish;
                    self.timer = BYTE;
                    self.end = R_WRITE_PROTECT;
                    return;
                }
                let (count, size) = match op {
                    0x0a | 0x0e | 0x12 | 0x16 | 0x1e => (1, 128),
                    0x1b => (p[2], 4),
                    _ => (p[2] & 0x1f, 128usize << (p[2] >> 5)),
                };
                self.sector = p[1];
                self.left = count;
                self.size = size;
                self.seek(p[0]);
            }
            _ => self.finish(R_GOOD),
        }
    }

    fn seek(&mut self, track: u8) {
        self.track = track;
        let target = self.physical(self.drive, track);
        let from = self.head[self.drive];
        let steps = u32::from(from.abs_diff(target));
        self.head[self.drive] = target;
        self.timer = steps * u32::from(self.step_rate.max(1)) * MS
            + if steps > 0 {
                u32::from(self.settle) * MS
            } else {
                0
            }
            + u32::from(self.load & 0x0f) * 4 * MS
            + 1;
        self.output |= 0x08;
        self.phase = Phase::Seek;
    }

    /// Clocks until the angle reaches `at`.
    fn until(&self, at: u32) -> u32 {
        (at + REVOLUTION - self.angle) % REVOLUTION + 1
    }

    /// The head is on the track: look for the next sector (or IDs, or the
    /// index for a format).
    fn search(&mut self, disks: &mut [Option<Floppy>; 2]) {
        let op = self.command & 0x3f;
        let Some(disk) = disks[self.drive].as_ref() else {
            self.finish(R_NOT_READY);
            return;
        };
        if self.left == 0 {
            self.phase = Phase::Finish;
            self.timer = BYTE;
            self.end = R_GOOD;
            return;
        }
        match op {
            0x1b => {
                self.buf.clear();
                let n = disk.sectors.max(1);
                for i in 0..u32::from(self.left) {
                    let s = i % n;
                    self.buf.extend([
                        self.head[self.drive],
                        self.side_byte(),
                        (disk.first_sector + s) as u8,
                        size_code(disk.sector_size as usize),
                    ]);
                }
                self.pos = 0;
                self.phase = Phase::ReadId;
                self.timer = self.until(0) + ID_TO_DATA;
                return;
            }
            0x23 => {
                self.buf.clear();
                self.pos = 0;
                self.phase = Phase::Format;
                self.timer = self.until(0);
                self.request();
                return;
            }
            0x00 | 0x04 => {
                self.phase = Phase::Finish;
                self.timer = REVOLUTION;
                self.end = R_GOOD;
                return;
            }
            _ => {}
        }
        let side = self.side();
        let head = u32::from(self.head[self.drive]);
        let found = disk.sector_size as usize == self.size
            && self.head[self.drive] == self.track
            && disk.offset(head, side, u32::from(self.sector)).is_some();
        if !found {
            self.phase = Phase::Finish;
            self.timer = 2 * REVOLUTION;
            self.end = R_NOT_FOUND;
            return;
        }
        let index = u32::from(self.sector).saturating_sub(disk.first_sector);
        let at = index * REVOLUTION / disk.sectors.max(1);
        self.timer = self.until(at) + ID_TO_DATA;
        self.pos = 0;
        let off = disk.offset(head, side, u32::from(self.sector)).unwrap_or(0);
        self.buf = disk.bytes[off..off + self.size].to_vec();
        self.phase = match op {
            0x0a | 0x0b | 0x0e | 0x0f => Phase::Write,
            0x1e | 0x1f => Phase::Verify,
            _ => Phase::Read,
        };
        if self.phase == Phase::Write {
            self.request();
        }
    }

    fn side_byte(&self) -> u8 {
        self.side() as u8
    }

    /// A data request for the next byte to write.
    fn request(&mut self) {
        self.drq = true;
        self.int = true;
    }

    /// The sector is done: the next one, or the end.
    fn next_sector(&mut self, disks: &mut [Option<Floppy>; 2]) {
        self.left = self.left.saturating_sub(1);
        self.sector = self.sector.wrapping_add(1);
        self.scan_sector = self.sector;
        self.search(disks);
    }

    pub fn tick(&mut self, clocks: u32, disks: &mut [Option<Floppy>; 2]) {
        self.angle = (self.angle + clocks) % REVOLUTION;
        if matches!(self.phase, Phase::Idle | Phase::Parameters) {
            return;
        }
        if self.timer > clocks {
            self.timer -= clocks;
            return;
        }
        self.timer = 0;
        match self.phase {
            Phase::Seek => {
                if self.command & 0x3f == 0x29 {
                    self.finish(R_GOOD);
                } else {
                    self.search(disks);
                }
            }
            Phase::Finish => {
                let end = self.end;
                self.finish(end);
            }
            Phase::Read | Phase::ReadId => {
                if self.drq {
                    self.finish(R_LATE);
                    return;
                }
                if self.pos < self.buf.len() {
                    self.data = self.buf[self.pos];
                    self.pos += 1;
                    self.drq = true;
                    self.int = true;
                    self.timer = BYTE;
                } else if self.phase == Phase::ReadId {
                    self.phase = Phase::Finish;
                    self.timer = BYTE;
                    self.end = R_GOOD;
                } else {
                    self.next_sector(disks);
                }
            }
            Phase::Verify => {
                self.timer = 0;
                self.next_sector(disks);
            }
            Phase::Write => {
                if self.drq {
                    self.finish(R_LATE);
                    return;
                }
                self.output |= 0x01;
                self.buf[self.pos] = self.data;
                self.pos += 1;
                if self.pos < self.buf.len() {
                    self.request();
                    self.timer = BYTE;
                    return;
                }
                if let Some(disk) = disks[self.drive].as_mut() {
                    let head = u32::from(self.head[self.drive]);
                    if let Some(off) = disk.offset(head, self.side(), u32::from(self.sector)) {
                        disk.bytes[off..off + self.size].copy_from_slice(&self.buf);
                        disk.dirty = true;
                    }
                }
                self.output &= !0x01;
                self.next_sector(disks);
            }
            Phase::Format => {
                if self.drq {
                    self.finish(R_LATE);
                    return;
                }
                self.buf.push(self.data);
                if self.buf.len() < usize::from(self.left) * 4 {
                    self.request();
                    self.timer = BYTE;
                    return;
                }
                self.format(disks);
            }
            Phase::Idle | Phase::Parameters | Phase::Search => {}
        }
    }

    /// The IDs are in: the data fields of the sectors the image holds get
    /// `&E5`.
    fn format(&mut self, disks: &mut [Option<Floppy>; 2]) {
        let side = self.side();
        let head = u32::from(self.head[self.drive]);
        let size = 128usize << (self.params[2] >> 5);
        if let Some(disk) = disks[self.drive].as_mut() {
            for id in self.buf.chunks(4) {
                if disk.sector_size as usize != size {
                    continue;
                }
                if let Some(off) = disk.offset(head, side, u32::from(id[2])) {
                    disk.bytes[off..off + size].fill(0xe5);
                    disk.dirty = true;
                }
            }
        }
        self.phase = Phase::Finish;
        self.timer = REVOLUTION / 2;
        self.end = R_GOOD;
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.command);
        w.bytes(&self.params);
        w.u8(self.count_params as u8);
        for b in [
            self.busy,
            self.command_full,
            self.result_full,
            self.int,
            self.drq,
            self.reset_line,
        ] {
            w.bool(b);
        }
        w.u8(self.result);
        w.u8(self.data);
        w.u8(self.phase as u8);
        w.u32(self.timer);
        w.u8(self.drive as u8);
        w.u8(self.track);
        w.u8(self.sector);
        w.u8(self.left);
        w.u32(self.size as u32);
        w.u32(self.pos as u32);
        w.bytes(&self.buf);
        w.u8(self.end);
        w.bytes(&self.head);
        w.bytes(&[
            self.bad[0][0],
            self.bad[0][1],
            self.bad[1][0],
            self.bad[1][1],
        ]);
        w.bytes(&[
            self.step_rate,
            self.settle,
            self.load,
            self.mode,
            self.output,
            self.scan_sector,
        ]);
        w.u32(self.angle);
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut n = Self::new();
        n.command = r.u8()?;
        let params = r.bytes()?;
        if params.len() != 5 {
            return Err(StateError::Invalid("8271 parameters"));
        }
        n.params.copy_from_slice(params);
        n.count_params = usize::from(r.u8()?).min(5);
        n.busy = r.bool()?;
        n.command_full = r.bool()?;
        n.result_full = r.bool()?;
        n.int = r.bool()?;
        n.drq = r.bool()?;
        n.reset_line = r.bool()?;
        n.result = r.u8()?;
        n.data = r.u8()?;
        n.phase = match r.u8()? {
            0 => Phase::Idle,
            1 => Phase::Parameters,
            2 => Phase::Seek,
            3 => Phase::Search,
            4 => Phase::Read,
            5 => Phase::Write,
            6 => Phase::Verify,
            7 => Phase::ReadId,
            8 => Phase::Format,
            9 => Phase::Finish,
            _ => return Err(StateError::Invalid("8271 phase")),
        };
        n.timer = r.u32()?;
        n.drive = usize::from(r.u8()? & 1);
        n.track = r.u8()?;
        n.sector = r.u8()?;
        n.left = r.u8()?;
        n.size = (r.u32()? as usize).min(16_384);
        n.buf = r.bytes()?.to_vec();
        n.pos = (r.u32()? as usize).min(n.buf.len());
        n.end = r.u8()?;
        let head = r.bytes()?;
        let bad = r.bytes()?;
        let regs = r.bytes()?;
        if head.len() != 2 || bad.len() != 4 || regs.len() != 6 {
            return Err(StateError::Invalid("8271 registers"));
        }
        n.head = [head[0], head[1]];
        n.bad = [[bad[0], bad[1]], [bad[2], bad[3]]];
        [
            n.step_rate,
            n.settle,
            n.load,
            n.mode,
            n.output,
            n.scan_sector,
        ] = [regs[0], regs[1], regs[2], regs[3], regs[4], regs[5]];
        n.angle = r.u32()? % REVOLUTION;
        *self = n;
        Ok(())
    }
}

fn size_code(size: usize) -> u8 {
    (size / 128).max(1).trailing_zeros() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> Floppy {
        let mut bytes = vec![0; 80 * 10 * 256];
        bytes[0] = 0x11;
        bytes[(3 * 10 + 2) * 256 + 5] = 0x33;
        Floppy {
            bytes,
            sides: 1,
            tracks: 80,
            sectors: 10,
            sector_size: 256,
            first_sector: 0,
            write_protected: false,
            dirty: false,
        }
    }

    fn command(f: &mut I8271, d: &mut [Option<Floppy>; 2], cmd: u8, params: &[u8]) {
        f.write(0, cmd, d);
        for &p in params {
            f.write(1, p, d);
        }
    }

    /// Run until INT, taking data bytes; the bytes and the result.
    fn run(f: &mut I8271, d: &mut [Option<Floppy>; 2], write: Option<u8>) -> (Vec<u8>, u8) {
        let mut got = Vec::new();
        for _ in 0..10_000_000 {
            f.tick(1, d);
            if f.int {
                if f.status() & ST_DATA_REQUEST != 0 {
                    match write {
                        Some(v) => f.write(4, v, d),
                        None => got.push(f.read(4)),
                    }
                } else {
                    return (got, f.read(1));
                }
            }
        }
        panic!("no result");
    }

    #[test]
    fn reads_a_sector_through_data_requests() {
        let mut f = I8271::new();
        let mut d = [Some(disk()), None];
        command(&mut f, &mut d, 0x35, &[0x0d, 6, 8, 0]);
        command(&mut f, &mut d, 0x53, &[3, 2, 0x21]);
        let (bytes, result) = run(&mut f, &mut d, None);
        assert_eq!(result, R_GOOD);
        assert_eq!(bytes.len(), 256);
        assert_eq!(bytes[5], 0x33);
        assert_eq!(f.head(0), 3);
    }

    #[test]
    fn missing_sectors_drives_and_protection_give_their_codes() {
        let mut f = I8271::new();
        let mut d = [Some(disk()), None];
        command(&mut f, &mut d, 0x53, &[0, 12, 0x21]);
        assert_eq!(run(&mut f, &mut d, None).1, R_NOT_FOUND);
        command(&mut f, &mut d, 0x93, &[0, 0, 0x21]);
        assert_eq!(run(&mut f, &mut d, None).1, R_NOT_READY);
        d[0].as_mut().unwrap().write_protected = true;
        command(&mut f, &mut d, 0x4b, &[0, 0, 0x21]);
        assert_eq!(run(&mut f, &mut d, Some(0)).1, R_WRITE_PROTECT);
    }

    #[test]
    fn writes_sectors_and_answers_status_at_once() {
        let mut f = I8271::new();
        let mut d = [Some(disk()), None];
        command(&mut f, &mut d, 0x4b, &[1, 4, 0x22]);
        let (_, result) = run(&mut f, &mut d, Some(0xab));
        assert_eq!(result, R_GOOD);
        let disk = d[0].as_ref().unwrap();
        assert!(disk.dirty);
        assert!(disk.bytes[14 * 256..16 * 256].iter().all(|&b| b == 0xab));
        command(&mut f, &mut d, 0x6c, &[]);
        assert_eq!(
            f.status() & (ST_BUSY | ST_RESULT_FULL | ST_INT),
            ST_RESULT_FULL
        );
        assert_eq!(f.read(1) & 0x04, 0x04, "ready 0");
    }

    #[test]
    fn a_slow_reader_gets_late_dma() {
        let mut f = I8271::new();
        let mut d = [Some(disk()), None];
        command(&mut f, &mut d, 0x53, &[0, 0, 0x21]);
        for _ in 0..1_000_000 {
            f.tick(1, &mut d);
            if f.status() & ST_RESULT_FULL != 0 {
                break;
            }
        }
        assert_eq!(f.read(1), R_LATE);
    }
}
