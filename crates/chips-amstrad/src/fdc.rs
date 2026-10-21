//! NEC µPD765A floppy disc controller as the CPC uses it (NEC µPD765
//! data sheet; cpcwiki, "765 FDC"): no DMA (the CPU moves every byte
//! through the data register in the execution phase), no interrupt line,
//! TC not connected, only US0 wired (two drives).
//!
//! Ports: &FA7E motor (bit 0, both drives), &FB7E main status register,
//! &FB7F data.
//!
//! | MSR bit | Meaning                                         |
//! | ------- | ----------------------------------------------- |
//! | 7 RQM   | the data register wants a transfer             |
//! | 6 DIO   | direction: 1 = to the CPU                      |
//! | 5 EXM   | execution phase                                |
//! | 4 CB    | a command is in progress                       |
//! | 1–0     | drive B / A seeking                            |
//!
//! Commands (bits 7–5 of the first byte: MT, MF, SK): read data (06),
//! read deleted data (0C), write data (05), write deleted data (09), read
//! track (02), read ID (0A), format track (0D), seek (0F), recalibrate
//! (07), sense interrupt status (08), sense drive status (04), specify
//! (03); everything else is invalid (ST0 = &80). Scan commands are not
//! supported.
//!
//! **Timing.** The disc turns at 300 rpm (200 ms); a track's sectors are
//! spread evenly over the turn, and a sector is found when it comes under
//! the head. Data moves at one byte per 32 µs (250 kbit/s MFM); a byte the
//! CPU does not take (or give) in time is an overrun. Without TC, a
//! multi-sector transfer ends after the sector EOT with "end of cylinder"
//! (ST0 abnormal termination, ST1 EN), as AMSDOS expects. A missing sector
//! is reported after two index pulses. The FDC on the CPC runs at 4 MHz,
//! so step times are twice the SPECIFY values.

use emu_core::{Savestate, StateError, StateReader, StateWriter};

/// One revolution in microseconds.
pub const REV_US: u32 = 200_000;
/// Microseconds per data byte.
const BYTE_US: u32 = 32;

// MSR bits.
const RQM: u8 = 0x80;
const DIO: u8 = 0x40;
const EXM: u8 = 0x20;
const CB: u8 = 0x10;

// ST0 – ST3 bits.
const IC_ABNORMAL: u8 = 0x40;
const IC_INVALID: u8 = 0x80;
const SE: u8 = 0x20;
const EC: u8 = 0x10;
const NR: u8 = 0x08;
const EN: u8 = 0x80;
const DE: u8 = 0x20;
const OR: u8 = 0x10;
const ND: u8 = 0x04;
const NW: u8 = 0x02;
const MA: u8 = 0x01;
const CM: u8 = 0x40;
const DD: u8 = 0x20;
const WC: u8 = 0x10;

/// A sector's ID field and the status stored with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectorInfo {
    pub id: [u8; 4],
    pub st1: u8,
    pub st2: u8,
}

/// What the controller needs from a drive.
pub trait FloppyDrive {
    /// A disc is in and turning.
    fn ready(&self) -> bool;
    fn write_protected(&self) -> bool;
    fn two_sided(&self) -> bool;
    /// The head's cylinder.
    fn cylinder(&self) -> u8;
    /// One step outward (towards cylinder 0) or inward.
    fn step(&mut self, outward: bool);
    /// The sectors of the track under the head on `side`, in the order
    /// they pass the head from the index hole.
    fn sectors(&self, side: u8) -> Vec<SectorInfo>;
    /// Data of the sector at `index` in that order.
    fn read(&self, side: u8, index: usize) -> Vec<u8>;
    fn write(&mut self, side: u8, index: usize, data: &[u8]) -> bool;
    fn format(&mut self, side: u8, n: u8, gap3: u8, filler: u8, ids: &[[u8; 4]]) -> bool;
}

/// Command bytes (including the first) by command code.
fn command_len(code: u8) -> usize {
    match code {
        0x02 | 0x05 | 0x06 | 0x09 | 0x0c | 0x11 | 0x19 | 0x1d => 9,
        0x03 | 0x0f => 3,
        0x04 | 0x07 | 0x0a => 2,
        0x0d => 6,
        _ => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Read,
    Write,
    ReadTrack,
    ReadId,
    Format,
}

impl Kind {
    fn id(self) -> u8 {
        self as u8
    }

    fn from_id(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Read,
            1 => Self::Write,
            2 => Self::ReadTrack,
            3 => Self::ReadId,
            4 => Self::Format,
            _ => return None,
        })
    }
}

/// An execution phase in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Exec {
    kind: Kind,
    drive: u8,
    side: u8,
    /// C, H, R, N, EOT, GPL, DTL (format: N, SC, GPL, D in 3–6).
    p: [u8; 7],
    deleted: bool,
    skip: bool,
    /// Microseconds until the wanted sector arrives (0: transferring).
    wait: u32,
    /// Index of that sector on the track (none: not found / error).
    sector: Option<u16>,
    buffer: Vec<u8>,
    pos: u16,
    /// Microseconds to the next byte; whether a byte waits for the CPU
    /// (read) or the controller waits for one (write).
    timer: u32,
    pending: bool,
    st: [u8; 3],
    /// Stop after this sector (deleted mark, data error).
    last: bool,
    /// Sectors read so far by read track.
    count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fdc {
    pub motor: bool,
    /// Command bytes received, result bytes still to be read.
    command: Vec<u8>,
    result: Vec<u8>,
    exec: Option<Exec>,
    /// Present cylinder per drive.
    pcn: [u8; 2],
    /// Seek in progress: target cylinder, steps to go, µs to the next step.
    seek: [Option<(u8, u8, u32)>; 2],
    /// Interrupts waiting for SENSE INTERRUPT STATUS (ST0 per drive).
    interrupt: [Option<u8>; 2],
    /// SPECIFY: step rate (µs per step).
    step_us: u32,
    /// Position in the revolution.
    rotation: u32,
    /// The last data byte (what a read of the data register returns).
    data: u8,
}

impl Default for Fdc {
    fn default() -> Self {
        Self::new()
    }
}

impl Fdc {
    pub fn new() -> Self {
        Self {
            motor: false,
            command: Vec::new(),
            result: Vec::new(),
            exec: None,
            pcn: [0; 2],
            seek: [None; 2],
            interrupt: [None; 2],
            step_us: 32_000,
            rotation: 0,
            data: 0xff,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// The present cylinder number of drive `i` (debugger).
    pub fn cylinder(&self, i: usize) -> u8 {
        self.pcn[i & 1]
    }

    /// Main status register (no side effects).
    pub fn status(&self) -> u8 {
        let seeking = u8::from(self.seek[0].is_some()) | u8::from(self.seek[1].is_some()) << 1;
        if !self.result.is_empty() {
            return RQM | DIO | CB | seeking;
        }
        if let Some(e) = &self.exec {
            let dir = if matches!(e.kind, Kind::Write | Kind::Format) {
                0
            } else {
                DIO
            };
            let rqm = if e.pending { RQM } else { 0 };
            return rqm | dir | EXM | CB | seeking;
        }
        let busy = if self.command.is_empty() { 0 } else { CB };
        RQM | busy | seeking
    }

    /// What a read of the data register would return (no side effects).
    pub fn peek_data(&self) -> u8 {
        match (&self.exec, self.result.first()) {
            (_, Some(&v)) => v,
            (Some(e), None) if e.pending && e.kind != Kind::Write => self.data,
            _ => 0xff,
        }
    }

    /// Read the data register.
    pub fn read_data(&mut self) -> u8 {
        if !self.result.is_empty() {
            return self.result.remove(0);
        }
        if let Some(e) = &mut self.exec {
            if e.pending && matches!(e.kind, Kind::Read | Kind::ReadTrack) {
                e.pending = false;
                e.pos += 1;
                return self.data;
            }
        }
        0xff
    }

    /// Write the data register.
    pub fn write_data<D: FloppyDrive>(&mut self, v: u8, drives: &mut [D; 2]) {
        if let Some(e) = &mut self.exec {
            if e.pending && matches!(e.kind, Kind::Write | Kind::Format) {
                e.pending = false;
                if usize::from(e.pos) < e.buffer.len() {
                    e.buffer[usize::from(e.pos)] = v;
                }
                e.pos += 1;
            }
            return;
        }
        if !self.result.is_empty() {
            return;
        }
        self.command.push(v);
        if self.command.len() >= command_len(self.command[0] & 0x1f) {
            let cmd = std::mem::take(&mut self.command);
            self.start(&cmd, drives);
        }
    }

    fn st0(drive: u8, side: u8) -> u8 {
        side << 2 | drive
    }

    fn start<D: FloppyDrive>(&mut self, cmd: &[u8], drives: &mut [D; 2]) {
        let code = cmd[0] & 0x1f;
        let drive = cmd.get(1).map_or(0, |b| b & 1);
        let side = cmd.get(1).map_or(0, |b| (b >> 2) & 1);
        let d = &drives[usize::from(drive)];
        match code {
            0x03 => {
                // SRT in the high nibble; 4 MHz clock: 2 ms units.
                self.step_us = (16 - u32::from(cmd[1] >> 4)) * 2000;
            }
            0x04 => {
                let st3 = if d.write_protected() { 0x40 } else { 0 }
                    | if d.ready() { 0x20 } else { 0 }
                    | if d.cylinder() == 0 { 0x10 } else { 0 }
                    | if d.two_sided() { 0x08 } else { 0 }
                    | Self::st0(drive, side);
                self.result = vec![st3];
            }
            0x07 | 0x0f => {
                let target = if code == 0x07 { 0 } else { cmd[2] };
                let steps = if code == 0x07 {
                    77
                } else {
                    target.abs_diff(self.pcn[usize::from(drive)])
                };
                self.seek[usize::from(drive)] = Some((target, steps, 0));
                if code == 0x07 {
                    // Recalibrate counts physical steps until track 0.
                    self.pcn[usize::from(drive)] = 0xff;
                }
            }
            0x08 => {
                self.result = match (0..2).find_map(|i| self.interrupt[i].take().map(|s| (i, s))) {
                    Some((i, st0)) => vec![st0, self.pcn[i]],
                    None => vec![IC_INVALID],
                };
            }
            0x02 | 0x05 | 0x06 | 0x09 | 0x0a | 0x0c | 0x0d => {
                let kind = match code {
                    0x05 | 0x09 => Kind::Write,
                    0x02 => Kind::ReadTrack,
                    0x0a => Kind::ReadId,
                    0x0d => Kind::Format,
                    _ => Kind::Read,
                };
                let mut p = [0u8; 7];
                for (k, v) in p.iter_mut().enumerate() {
                    *v = cmd.get(2 + k).copied().unwrap_or(0);
                }
                let mut e = Exec {
                    kind,
                    drive,
                    side,
                    p,
                    deleted: code == 0x09 || code == 0x0c,
                    skip: cmd[0] & 0x20 != 0,
                    wait: 0,
                    sector: None,
                    buffer: Vec::new(),
                    pos: 0,
                    timer: 0,
                    pending: false,
                    st: [Self::st0(drive, side), 0, 0],
                    last: false,
                    count: 0,
                };
                if !d.ready() {
                    e.st[0] |= IC_ABNORMAL | NR;
                    self.finish(e);
                    return;
                }
                if matches!(kind, Kind::Write | Kind::Format) && d.write_protected() {
                    e.st[0] |= IC_ABNORMAL;
                    e.st[1] |= NW;
                    self.finish(e);
                    return;
                }
                match kind {
                    Kind::Format => {
                        // Starts at the index hole; then 4 bytes per sector.
                        e.wait = (REV_US - self.rotation) % REV_US + 1;
                        e.buffer = vec![0; 4 * usize::from(e.p[1])];
                    }
                    Kind::ReadTrack => self.find_by_index(&mut e, d, 0),
                    _ => self.find(&mut e, d),
                }
                self.exec = Some(e);
            }
            _ => self.result = vec![IC_INVALID],
        }
    }

    /// Microseconds from now until sector `i` of `n` passes the head.
    fn latency(&self, i: usize, n: usize) -> u32 {
        let at = (i as u64 * u64::from(REV_US) / n as u64) as u32;
        (at + REV_US - self.rotation) % REV_US + 1
    }

    /// Look for the sector the parameters name (C, H, R, N).
    fn find<D: FloppyDrive>(&self, e: &mut Exec, d: &D) {
        let sectors = d.sectors(e.side);
        let want = [e.p[0], e.p[1], e.p[2], e.p[3]];
        let found = (0..sectors.len())
            .filter(|&i| sectors[i].id == want)
            .min_by_key(|&i| self.latency(i, sectors.len()));
        match found {
            Some(i) => {
                e.sector = Some(i as u16);
                e.wait = self.latency(i, sectors.len());
            }
            None => {
                // Two index pulses, then "no data" (or "missing address
                // mark" on an unformatted track).
                e.sector = None;
                e.wait = 2 * REV_US;
                e.st[0] |= IC_ABNORMAL;
                if sectors.is_empty() {
                    e.st[1] |= MA;
                } else {
                    e.st[1] |= ND;
                    if sectors
                        .iter()
                        .any(|s| s.id[2] == want[2] && s.id[0] != want[0])
                    {
                        e.st[2] |= WC;
                    }
                }
            }
        }
        if e.kind == Kind::ReadId {
            // READ ID takes the next sector, whatever it is.
            let next = (0..sectors.len()).min_by_key(|&i| self.latency(i, sectors.len()));
            e.sector = next.map(|i| i as u16);
            e.st = [e.st[0] & 7, 0, 0];
            match next {
                Some(i) => e.wait = self.latency(i, sectors.len()),
                None => {
                    e.wait = 2 * REV_US;
                    e.st[0] |= IC_ABNORMAL;
                    e.st[1] |= MA;
                }
            }
        }
    }

    /// Read track: sector `i` from the index.
    fn find_by_index<D: FloppyDrive>(&self, e: &mut Exec, d: &D, i: usize) {
        let n = d.sectors(e.side).len();
        if n == 0 {
            e.sector = None;
            e.wait = 2 * REV_US;
            e.st[0] |= IC_ABNORMAL;
            e.st[1] |= MA;
        } else {
            e.sector = Some((i % n) as u16);
            e.wait = if i == 0 {
                (REV_US - self.rotation) % REV_US + 1
            } else {
                self.latency(i % n, n)
            };
        }
    }

    /// One microsecond.
    pub fn tick<D: FloppyDrive>(&mut self, drives: &mut [D; 2]) {
        if self.motor {
            self.rotation = (self.rotation + 1) % REV_US;
        }
        for (i, d) in drives.iter_mut().enumerate() {
            let Some((target, steps, timer)) = self.seek[i] else {
                continue;
            };
            if timer > 1 {
                self.seek[i] = Some((target, steps, timer - 1));
                continue;
            }
            let recal = self.pcn[i] == 0xff;
            let done = if recal {
                d.cylinder() == 0 || steps == 0
            } else {
                self.pcn[i] == target
            };
            if done {
                self.seek[i] = None;
                let mut st0 = SE | i as u8;
                if recal {
                    if d.cylinder() != 0 {
                        st0 |= IC_ABNORMAL | EC;
                    }
                    self.pcn[i] = 0;
                }
                self.interrupt[i] = Some(st0);
                continue;
            }
            let outward = if recal { true } else { target < self.pcn[i] };
            d.step(outward);
            if !recal {
                self.pcn[i] = if outward {
                    self.pcn[i] - 1
                } else {
                    self.pcn[i] + 1
                };
            }
            self.seek[i] = Some((target, steps.saturating_sub(1), self.step_us));
        }
        let Some(mut e) = self.exec.take() else {
            return;
        };
        self.run(&mut e, drives);
        if self.exec.is_none() && self.result.is_empty() {
            self.exec = Some(e);
        }
    }

    fn run<D: FloppyDrive>(&mut self, e: &mut Exec, drives: &mut [D; 2]) {
        let d = &mut drives[usize::from(e.drive)];
        if e.wait > 0 {
            e.wait -= 1;
            if e.wait > 0 {
                return;
            }
            if e.kind == Kind::Format {
                e.pending = true;
                e.timer = BYTE_US;
                return;
            }
            let Some(i) = e.sector else {
                self.finish(e.clone());
                return;
            };
            let info = d.sectors(e.side)[usize::from(i)];
            if e.kind == Kind::ReadId {
                e.p[..4].copy_from_slice(&info.id);
                self.finish(e.clone());
                return;
            }
            self.begin_sector(e, d, usize::from(i), info);
            return;
        }
        // Transferring.
        if e.timer > 1 {
            e.timer -= 1;
            return;
        }
        let size = e.buffer.len();
        if e.pending {
            // The CPU missed the byte.
            e.st[0] |= IC_ABNORMAL;
            e.st[1] |= OR;
            self.finish(e.clone());
            return;
        }
        if usize::from(e.pos) < size {
            if matches!(e.kind, Kind::Read | Kind::ReadTrack) {
                self.data = e.buffer[usize::from(e.pos)];
            }
            e.pending = true;
            e.timer = BYTE_US;
            return;
        }
        // The sector is done.
        match e.kind {
            Kind::Format => {
                let ids: Vec<[u8; 4]> = e
                    .buffer
                    .chunks(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect();
                if !d.format(e.side, e.p[0], e.p[2], e.p[3], &ids) {
                    e.st[0] |= IC_ABNORMAL;
                    e.st[1] |= NW;
                }
                if let Some(last) = ids.last() {
                    e.p[..4].copy_from_slice(last);
                }
                self.finish(e.clone());
            }
            Kind::Write => {
                if let Some(i) = e.sector {
                    d.write(e.side, usize::from(i), &e.buffer);
                }
                self.next_sector(e, d);
            }
            Kind::Read => self.next_sector(e, d),
            Kind::ReadTrack => {
                e.count += 1;
                if e.count >= e.p[4] {
                    e.st[0] |= IC_ABNORMAL;
                    e.st[1] |= EN;
                    self.finish(e.clone());
                } else {
                    let i = usize::from(e.sector.unwrap_or(0)) + 1;
                    self.find_by_index(e, d, i);
                }
            }
            Kind::ReadId => {}
        }
    }

    /// The sector arrived: load the buffer, check its marks.
    fn begin_sector<D: FloppyDrive>(&mut self, e: &mut Exec, d: &D, i: usize, info: SectorInfo) {
        let n = if e.kind == Kind::ReadTrack {
            e.p[3]
        } else {
            info.id[3]
        };
        let size = if n == 0 {
            usize::from(e.p[6]).max(1)
        } else {
            128usize << n.min(6)
        };
        let deleted = info.st2 & CM != 0;
        if e.kind != Kind::Write && e.kind != Kind::ReadTrack && deleted != e.deleted {
            e.st[2] |= CM;
            if e.skip {
                self.next_sector(e, d);
                return;
            }
            e.last = true;
        }
        e.buffer = vec![0; size];
        if e.kind != Kind::Write {
            let data = d.read(e.side, i);
            let k = data.len().min(size);
            e.buffer[..k].copy_from_slice(&data[..k]);
            if info.st1 & DE != 0 || info.st2 & DD != 0 {
                e.st[1] |= info.st1 & DE;
                e.st[2] |= info.st2 & DD;
                e.last = true;
            }
        }
        e.pos = 0;
        e.timer = 1;
        e.pending = false;
    }

    /// After a sector of a read or write: the next one, or the end.
    fn next_sector<D: FloppyDrive>(&mut self, e: &mut Exec, d: &D) {
        if e.st[1] & (DE) != 0 || e.st[2] & DD != 0 {
            e.st[0] |= IC_ABNORMAL;
            self.finish(e.clone());
            return;
        }
        if e.last {
            self.finish(e.clone());
            return;
        }
        if e.p[2] == e.p[4] {
            // No TC on the CPC: end of cylinder; C + 1, R = 1.
            e.st[0] |= IC_ABNORMAL;
            e.st[1] |= EN;
            e.p[0] = e.p[0].wrapping_add(1);
            e.p[2] = 1;
            self.finish(e.clone());
            return;
        }
        e.p[2] = e.p[2].wrapping_add(1);
        e.buffer.clear();
        self.find(e, d);
    }

    fn finish(&mut self, e: Exec) {
        self.exec = None;
        self.result = vec![e.st[0], e.st[1], e.st[2], e.p[0], e.p[1], e.p[2], e.p[3]];
    }
}

const TAG: [u8; 4] = *b"F765";
const VERSION: u16 = 1;

impl Savestate for Fdc {
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.bool(self.motor);
            w.bytes(&self.command);
            w.bytes(&self.result);
            w.raw(&self.pcn);
            for s in self.seek {
                w.bool(s.is_some());
                let (t, n, timer) = s.unwrap_or((0, 0, 0));
                w.u8(t);
                w.u8(n);
                w.u32(timer);
            }
            for i in self.interrupt {
                w.bool(i.is_some());
                w.u8(i.unwrap_or(0));
            }
            w.u32(self.step_us);
            w.u32(self.rotation);
            w.u8(self.data);
            w.bool(self.exec.is_some());
            if let Some(e) = &self.exec {
                w.u8(e.kind.id());
                w.u8(e.drive);
                w.u8(e.side);
                w.raw(&e.p);
                w.bool(e.deleted);
                w.bool(e.skip);
                w.u32(e.wait);
                w.bool(e.sector.is_some());
                w.u16(e.sector.unwrap_or(0));
                w.bytes(&e.buffer);
                w.u16(e.pos);
                w.u32(e.timer);
                w.bool(e.pending);
                w.raw(&e.st);
                w.bool(e.last);
                w.u8(e.count);
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut n = Self::new();
        n.motor = c.bool()?;
        n.command = c.bytes()?.to_vec();
        n.result = c.bytes()?.to_vec();
        c.raw_into(&mut n.pcn)?;
        for s in n.seek.iter_mut() {
            let some = c.bool()?;
            let v = (c.u8()?, c.u8()?, c.u32()?);
            *s = some.then_some(v);
        }
        for i in n.interrupt.iter_mut() {
            let some = c.bool()?;
            let v = c.u8()?;
            *i = some.then_some(v);
        }
        n.step_us = c.u32()?;
        n.rotation = c.u32()? % REV_US;
        n.data = c.u8()?;
        if c.bool()? {
            let kind = Kind::from_id(c.u8()?).ok_or(StateError::Invalid("FDC phase"))?;
            let drive = c.u8()? & 1;
            let side = c.u8()? & 1;
            let mut p = [0; 7];
            c.raw_into(&mut p)?;
            let deleted = c.bool()?;
            let skip = c.bool()?;
            let wait = c.u32()?;
            let has = c.bool()?;
            let sector = c.u16()?;
            let buffer = c.bytes()?.to_vec();
            let pos = c.u16()?;
            let timer = c.u32()?;
            let pending = c.bool()?;
            let mut st = [0; 3];
            c.raw_into(&mut st)?;
            let last = c.bool()?;
            let count = c.u8()?;
            if usize::from(pos) > buffer.len() + 1 {
                return Err(StateError::Invalid("FDC buffer"));
            }
            n.exec = Some(Exec {
                kind,
                drive,
                side,
                p,
                deleted,
                skip,
                wait,
                sector: has.then_some(sector),
                buffer,
                pos,
                timer,
                pending,
                st,
                last,
                count,
            });
        }
        if n.command.len() > 9 || n.result.len() > 7 {
            return Err(StateError::Invalid("FDC phase"));
        }
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
