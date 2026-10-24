//! SCSI on the Mac Plus and SE: the NCR 5380 controller as initiator and
//! hard disks as targets (IDs 0–6; the Mac is ID 7), at the level of the
//! bus signals.
//!
//! **NCR 5380** (NCR 5380 SCSI Interface Chip data sheet; Linux/m68k "Mac
//! Plus hardware" notes): base $580000, register r at $580000 + r·$10,
//! read at even and write at odd addresses; A9 ($580200) is DACK for
//! pseudo-DMA. Its interrupt is not wired; the SCSI Manager polls.
//!
//! | r | read                                   | write                    |
//! | - | -------------------------------------- | ------------------------ |
//! | 0 | current SCSI data                      | output data              |
//! | 1 | initiator command: 7 RST, 6 arbitration in progress, 5 lost arbitration, 4 ACK, 3 BSY, 2 SEL, 1 ATN, 0 assert data bus | same (bits 7, 4–0) |
//! | 2 | mode: 0 arbitrate, 1 DMA mode, 2 monitor busy, 6 target mode | same |
//! | 3 | target command: 3 REQ, 2 MSG, 1 C/D, 0 I/O | same           |
//! | 4 | bus status: 7 RST, 6 BSY, 5 REQ, 4 MSG, 3 C/D, 2 I/O, 1 SEL, 0 parity | select enable |
//! | 5 | bus and status: 7 end of DMA, 6 DMA request, 4 IRQ, 3 phase match, 1 ATN, 0 ACK | start DMA send |
//! | 6 | input data                             | start DMA target receive |
//! | 7 | reset parity/interrupt                 | start DMA initiator receive |
//!
//! With DMA mode on and a DMA started, a DACK read takes the byte on the
//! bus and a DACK write puts one there, each with the REQ/ACK handshake
//! done by the chip. Arbitration always wins (the Mac is the only
//! initiator).
//!
//! **Targets** (ANSI X3.131-1986 SCSI-1): a disk answers selection with
//! BSY, takes an identify message if ATN was set, then goes through the
//! command, data, status and message-in phases, every byte handshaken by
//! REQ/ACK as the initiator drives its lines. Commands: TEST UNIT READY,
//! REZERO, REQUEST SENSE, FORMAT UNIT, READ(6), WRITE(6), SEEK, INQUIRY,
//! MODE SELECT, MODE SENSE, START/STOP, SEND DIAGNOSTIC, PREVENT/ALLOW,
//! READ CAPACITY, READ(10), WRITE(10), VERIFY; others end with CHECK
//! CONDITION, ILLEGAL REQUEST. Blocks are 512 bytes, read from and
//! written through to the image's backend (the copy-on-write overlay by
//! default). Bus timing is not modelled: the target reacts as soon as
//! the initiator's lines change.

use emu_core::media::{DriveStatus, Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};

pub const BLOCK: usize = 512;
/// Targets on the bus (IDs 0–6).
pub const TARGETS: usize = 7;

pub mod icr {
    pub const RST: u8 = 0x80;
    pub const AIP: u8 = 0x40;
    pub const LA: u8 = 0x20;
    pub const ACK: u8 = 0x10;
    pub const BSY: u8 = 0x08;
    pub const SEL: u8 = 0x04;
    pub const ATN: u8 = 0x02;
    pub const DATA: u8 = 0x01;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Free,
    /// Selected: BSY asserted, waiting for SEL to drop.
    Selected,
    MsgOut,
    Command,
    DataIn,
    DataOut,
    Status,
    MsgIn,
}

impl Phase {
    /// MSG, C/D, I/O.
    fn lines(self) -> (bool, bool, bool) {
        match self {
            Phase::MsgOut => (true, true, false),
            Phase::Command => (false, true, false),
            Phase::DataIn => (false, false, true),
            Phase::DataOut => (false, false, false),
            Phase::Status => (false, true, true),
            Phase::MsgIn => (true, true, true),
            Phase::Free | Phase::Selected => (false, false, false),
        }
    }

    fn code(self) -> u8 {
        match self {
            Phase::Free => 0,
            Phase::Selected => 1,
            Phase::MsgOut => 2,
            Phase::Command => 3,
            Phase::DataIn => 4,
            Phase::DataOut => 5,
            Phase::Status => 6,
            Phase::MsgIn => 7,
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            0 => Phase::Free,
            1 => Phase::Selected,
            2 => Phase::MsgOut,
            3 => Phase::Command,
            4 => Phase::DataIn,
            5 => Phase::DataOut,
            6 => Phase::Status,
            7 => Phase::MsgIn,
            _ => return None,
        })
    }
}

/// A hard disk on the bus.
#[derive(Default)]
pub struct ScsiDisk {
    media: Option<Media>,
    protected: bool,
    /// Sense key and additional sense code of the last error.
    sense: (u8, u8),
    busy: bool,
}

impl std::fmt::Debug for ScsiDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScsiDisk")
            .field("media", &self.media.as_ref().map(|m| &m.name))
            .finish()
    }
}

impl ScsiDisk {
    pub fn has_disk(&self) -> bool {
        self.media.is_some()
    }

    pub fn blocks(&self) -> u32 {
        self.media
            .as_ref()
            .map_or(0, |m| (m.backend.len() / BLOCK as u64) as u32)
    }

    fn read_blocks(&mut self, lba: u32, n: u32) -> Option<Vec<u8>> {
        if u64::from(lba) + u64::from(n) > u64::from(self.blocks()) {
            return None;
        }
        let m = self.media.as_mut()?;
        let mut buf = vec![0; n as usize * BLOCK];
        m.backend
            .read(u64::from(lba) * BLOCK as u64, &mut buf)
            .ok()?;
        Some(buf)
    }

    fn write_blocks(&mut self, lba: u32, data: &[u8]) -> bool {
        let n = (data.len() / BLOCK) as u64;
        if u64::from(lba) + n > u64::from(self.blocks()) || self.protected {
            return false;
        }
        match self.media.as_mut() {
            Some(m) => m.backend.write(u64::from(lba) * BLOCK as u64, data).is_ok(),
            None => false,
        }
    }
}

impl MediaSlot for ScsiDisk {
    fn insert(&mut self, media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let len = media.backend.len();
        if len < BLOCK as u64 || !len.is_multiple_of(BLOCK as u64) {
            return Err(MediaError::Format(format!(
                "a hard disk image is whole 512-byte blocks ({len} bytes)"
            )));
        }
        let _ = self.eject();
        self.media = Some(media);
        self.sense = (0, 0);
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

    fn status(&self) -> DriveStatus {
        DriveStatus {
            active: self.busy,
            error: false,
            track: None,
        }
    }
}

/// The controller, the bus and its targets.
pub struct Scsi {
    pub disks: Vec<ScsiDisk>,
    pub odr: u8,
    pub icr: u8,
    pub mode: u8,
    pub tcr: u8,
    /// A DMA transfer was started (send / initiator receive).
    dma_send: bool,
    dma_recv: bool,
    // The selected target and its transfer.
    target: Option<usize>,
    phase: Phase,
    req: bool,
    /// The target took the byte of the current handshake.
    acked: bool,
    data: u8,
    cdb: Vec<u8>,
    buf: Vec<u8>,
    pos: usize,
    /// DataOut: what to do with the bytes when they are in.
    out_lba: Option<u32>,
    out_expected: usize,
    status: u8,
}

impl std::fmt::Debug for Scsi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scsi")
            .field("icr", &self.icr)
            .field("mode", &self.mode)
            .field("phase", &self.phase)
            .finish()
    }
}

impl Default for Scsi {
    fn default() -> Self {
        Self::new()
    }
}

impl Scsi {
    pub fn new() -> Self {
        Self {
            disks: (0..TARGETS).map(|_| ScsiDisk::default()).collect(),
            odr: 0,
            icr: 0,
            mode: 0,
            tcr: 0,
            dma_send: false,
            dma_recv: false,
            target: None,
            phase: Phase::Free,
            req: false,
            acked: false,
            data: 0,
            cdb: Vec::new(),
            buf: Vec::new(),
            pos: 0,
            out_lba: None,
            out_expected: 0,
            status: 0,
        }
    }

    pub fn reset(&mut self) {
        self.odr = 0;
        self.icr = 0;
        self.mode = 0;
        self.tcr = 0;
        self.dma_send = false;
        self.dma_recv = false;
        self.bus_free();
    }

    fn bus_free(&mut self) {
        if let Some(t) = self.target.take() {
            self.disks[t].busy = false;
        }
        self.phase = Phase::Free;
        self.req = false;
        self.acked = false;
        self.cdb.clear();
        self.buf.clear();
        self.pos = 0;
        self.out_lba = None;
        self.out_expected = 0;
    }

    fn arbitrating(&self) -> bool {
        self.mode & 1 != 0
    }

    /// The byte on the data lines.
    fn bus_data(&self) -> u8 {
        let mut v = 0;
        if self.icr & icr::DATA != 0 || self.arbitrating() {
            v |= self.odr;
        }
        if self.target.is_some() && self.phase.lines().2 {
            v |= self.data;
        }
        v
    }

    fn target_bsy(&self) -> bool {
        self.target.is_some()
    }

    fn csr(&self) -> u8 {
        let (msg, cd, io) = self.phase.lines();
        let t = self.target.is_some() && self.phase != Phase::Selected;
        u8::from(self.icr & icr::RST != 0) << 7
            | u8::from(self.target_bsy() || self.icr & icr::BSY != 0) << 6
            | u8::from(self.req) << 5
            | u8::from(t && msg) << 4
            | u8::from(t && cd) << 3
            | u8::from(t && io) << 2
            | u8::from(self.icr & icr::SEL != 0) << 1
    }

    fn phase_match(&self) -> bool {
        let (msg, cd, io) = self.phase.lines();
        self.tcr & 7 == u8::from(msg) << 2 | u8::from(cd) << 1 | u8::from(io)
    }

    fn bsr(&self) -> u8 {
        let drq = self.mode & 2 != 0 && (self.dma_send || self.dma_recv) && self.req;
        u8::from(drq) << 6
            | u8::from(self.phase_match()) << 3
            | u8::from(self.icr & icr::ATN != 0) << 1
            | u8::from(self.icr & icr::ACK != 0)
    }

    fn icr_read(&self) -> u8 {
        let aip = self.arbitrating() && self.target.is_none();
        (self.icr & 0x9f) | u8::from(aip) << 6
    }

    pub fn peek(&self, reg: u8) -> u8 {
        match reg & 7 {
            0 | 6 => self.bus_data(),
            1 => self.icr_read(),
            2 => self.mode,
            3 => self.tcr,
            4 => self.csr(),
            5 => self.bsr(),
            _ => 0,
        }
    }

    /// A register read; `dack` for a pseudo-DMA access.
    pub fn read(&mut self, reg: u8, dack: bool) -> u8 {
        if dack && self.dma_recv && self.mode & 2 != 0 {
            let v = self.bus_data();
            self.handshake();
            return v;
        }
        // Register 7 (reset parity/interrupt) has nothing latched here.
        self.peek(reg)
    }

    pub fn write(&mut self, reg: u8, v: u8, dack: bool) {
        if dack && self.dma_send && self.mode & 2 != 0 {
            self.odr = v;
            let keep = self.icr;
            self.icr |= icr::DATA;
            self.handshake();
            self.icr = keep;
            return;
        }
        match reg & 7 {
            0 => self.odr = v,
            1 => self.icr = v & 0x9f,
            2 => {
                self.mode = v;
                if v & 2 == 0 {
                    self.dma_send = false;
                    self.dma_recv = false;
                }
            }
            3 => self.tcr = v & 0x0f,
            4 => {}
            5 => self.dma_send = true,
            6 => {}
            _ => self.dma_recv = true,
        }
        self.settle();
    }

    /// A REQ/ACK handshake done by the chip in DMA mode.
    fn handshake(&mut self) {
        if !self.req {
            return;
        }
        self.icr |= icr::ACK;
        self.settle();
        self.icr &= !icr::ACK;
        self.settle();
    }

    /// Let the targets react to the bus until nothing changes.
    fn settle(&mut self) {
        if self.icr & icr::RST != 0 {
            self.bus_free();
            return;
        }
        for _ in 0..8 {
            if !self.step() {
                return;
            }
        }
    }

    fn step(&mut self) -> bool {
        let sel = self.icr & icr::SEL != 0;
        let ack = self.icr & icr::ACK != 0;
        match self.phase {
            Phase::Free => {
                if !sel || self.icr & icr::BSY != 0 {
                    return false;
                }
                let ids = self.bus_data() & 0x7f;
                let Some(t) =
                    (0..TARGETS).find(|&t| ids & (1 << t) != 0 && self.disks[t].has_disk())
                else {
                    return false;
                };
                self.target = Some(t);
                self.disks[t].busy = true;
                self.phase = Phase::Selected;
                true
            }
            Phase::Selected => {
                if sel {
                    return false;
                }
                let next = if self.icr & icr::ATN != 0 {
                    Phase::MsgOut
                } else {
                    Phase::Command
                };
                self.enter(next);
                true
            }
            _ => {
                if self.req && ack && !self.acked {
                    self.req = false;
                    self.acked = true;
                    self.take_byte();
                    true
                } else if !self.req && !ack && self.acked {
                    self.acked = false;
                    self.next_byte();
                    true
                } else {
                    false
                }
            }
        }
    }

    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.pos = 0;
        match phase {
            Phase::Status => self.data = self.status,
            Phase::MsgIn => self.data = 0,
            Phase::DataIn => self.data = self.buf.first().copied().unwrap_or(0),
            _ => {}
        }
        self.req = true;
    }

    /// The initiator acknowledged the byte of this REQ.
    fn take_byte(&mut self) {
        let byte = self.bus_data();
        match self.phase {
            Phase::MsgOut => {}
            Phase::Command => self.cdb.push(byte),
            Phase::DataOut => self.buf.push(byte),
            _ => {}
        }
        self.pos += 1;
    }

    /// ACK went away: the next byte or phase.
    fn next_byte(&mut self) {
        match self.phase {
            Phase::MsgOut => {
                if self.icr & icr::ATN != 0 {
                    self.req = true;
                } else {
                    self.enter(Phase::Command);
                }
            }
            Phase::Command => {
                if self.cdb.len() < cdb_len(self.cdb[0]) {
                    self.req = true;
                } else {
                    self.execute();
                }
            }
            Phase::DataIn => {
                if self.pos < self.buf.len() {
                    self.data = self.buf[self.pos];
                    self.req = true;
                } else {
                    self.enter(Phase::Status);
                }
            }
            Phase::DataOut => {
                if self.pos < self.out_len() {
                    self.req = true;
                } else {
                    self.finish_out();
                }
            }
            Phase::Status => self.enter(Phase::MsgIn),
            Phase::MsgIn => self.bus_free(),
            Phase::Free | Phase::Selected => {}
        }
    }

    fn out_len(&self) -> usize {
        self.out_expected
    }

    fn check(&mut self, key: u8, asc: u8) {
        if let Some(t) = self.target {
            self.disks[t].sense = (key, asc);
        }
        self.status = 2;
        self.enter(Phase::Status);
    }

    fn good(&mut self) {
        self.status = 0;
        self.enter(Phase::Status);
    }

    fn data_in(&mut self, data: Vec<u8>, alloc: usize) {
        let mut data = data;
        data.truncate(alloc);
        self.status = 0;
        if data.is_empty() {
            self.enter(Phase::Status);
        } else {
            self.buf = data;
            self.enter(Phase::DataIn);
        }
    }

    fn execute(&mut self) {
        let Some(t) = self.target else {
            return self.bus_free();
        };
        let c = self.cdb.clone();
        let op = c[0];
        let lba6 = || u32::from(c[1] & 0x1f) << 16 | u32::from(c[2]) << 8 | u32::from(c[3]);
        let len6 = || if c[4] == 0 { 256 } else { u32::from(c[4]) };
        let lba10 = || u32::from_be_bytes([c[2], c[3], c[4], c[5]]);
        let len10 = || u32::from(c[7]) << 8 | u32::from(c[8]);
        let (sense, blocks, protected) = {
            let d = &self.disks[t];
            (d.sense, d.blocks(), d.protected)
        };
        match op {
            0x00 | 0x01 | 0x04 | 0x0b | 0x16 | 0x17 | 0x1b | 0x1d | 0x1e | 0x2f => {
                self.disks[t].sense = (0, 0);
                self.good();
            }
            0x03 => {
                let (key, asc) = sense;
                self.disks[t].sense = (0, 0);
                let mut s = vec![0u8; 18];
                s[0] = 0x70;
                s[2] = key;
                s[7] = 10;
                s[12] = asc;
                let alloc = if c[4] == 0 { 4 } else { usize::from(c[4]) };
                self.data_in(s, alloc);
            }
            0x08 | 0x28 => {
                let (lba, n) = if op == 0x08 {
                    (lba6(), len6())
                } else {
                    (lba10(), len10())
                };
                match self.disks[t].read_blocks(lba, n) {
                    Some(d) => self.data_in(d, usize::MAX),
                    None => self.check(5, 0x21),
                }
            }
            0x0a | 0x2a => {
                let (lba, n) = if op == 0x0a {
                    (lba6(), len6())
                } else {
                    (lba10(), len10())
                };
                if u64::from(lba) + u64::from(n) > u64::from(blocks) {
                    return self.check(5, 0x21);
                }
                if protected {
                    return self.check(7, 0x27);
                }
                self.begin_out(Some(lba), n as usize * BLOCK);
            }
            0x15 => self.begin_out(None, usize::from(c[4])),
            0x12 => {
                let mut d = vec![0u8; 36];
                d[2] = 1;
                d[3] = 1;
                d[4] = 31;
                d[8..16].copy_from_slice(b"QUANTUM ");
                d[16..32].copy_from_slice(b"MIMIC SCSI DISK ");
                d[32..36].copy_from_slice(b"1.0 ");
                self.data_in(d, usize::from(c[4]));
            }
            0x1a => {
                let mut d = vec![11, 0, if protected { 0x80 } else { 0 }, 8, 0];
                d.extend_from_slice(&blocks.to_be_bytes()[1..]);
                d.extend_from_slice(&[0, 0, (BLOCK >> 8) as u8, 0]);
                self.data_in(d, usize::from(c[4]));
            }
            0x25 => {
                let last = blocks.saturating_sub(1);
                let mut d = last.to_be_bytes().to_vec();
                d.extend_from_slice(&(BLOCK as u32).to_be_bytes());
                self.data_in(d, 8);
            }
            _ => self.check(5, 0x20),
        }
    }

    fn begin_out(&mut self, lba: Option<u32>, len: usize) {
        self.out_lba = lba;
        self.buf.clear();
        self.out_expected = len;
        if len == 0 {
            self.good();
        } else {
            self.enter(Phase::DataOut);
        }
    }

    fn finish_out(&mut self) {
        let ok = match (self.out_lba, self.target) {
            (Some(lba), Some(t)) => {
                let data = std::mem::take(&mut self.buf);
                self.disks[t].write_blocks(lba, &data)
            }
            _ => true,
        };
        if ok {
            self.good();
        } else {
            self.check(3, 0x0c);
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"5380", 1, |w| {
            for b in [self.odr, self.icr, self.mode, self.tcr] {
                w.u8(b);
            }
            w.bool(self.dma_send);
            w.bool(self.dma_recv);
            w.u8(self.target.map_or(0xff, |t| t as u8));
            w.u8(self.phase.code());
            w.bool(self.req);
            w.bool(self.acked);
            w.u8(self.data);
            w.bytes(&self.cdb);
            w.bytes(&self.buf);
            w.u32(self.pos as u32);
            w.bool(self.out_lba.is_some());
            w.u32(self.out_lba.unwrap_or(0));
            w.u32(self.out_expected as u32);
            w.u8(self.status);
            for d in &self.disks {
                w.u8(d.sense.0);
                w.u8(d.sense.1);
            }
        });
    }

    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"5380", 1)?;
        let odr = c.u8()?;
        let icr_v = c.u8()?;
        let mode = c.u8()?;
        let tcr = c.u8()?;
        let dma_send = c.bool()?;
        let dma_recv = c.bool()?;
        let target = match c.u8()? {
            0xff => None,
            t if usize::from(t) < TARGETS => Some(usize::from(t)),
            _ => return Err(StateError::Invalid("scsi target")),
        };
        let phase = Phase::from_code(c.u8()?).ok_or(StateError::Invalid("scsi phase"))?;
        let req = c.bool()?;
        let acked = c.bool()?;
        let data = c.u8()?;
        let cdb = c.bytes()?.to_vec();
        let buf = c.bytes()?.to_vec();
        let pos = c.u32()? as usize;
        let has_lba = c.bool()?;
        let lba = c.u32()?;
        let out_expected = c.u32()? as usize;
        let status = c.u8()?;
        let mut sense = [(0u8, 0u8); TARGETS];
        for s in &mut sense {
            *s = (c.u8()?, c.u8()?);
        }
        if cdb.len() > 16 || pos > buf.len().max(out_expected).max(16) {
            return Err(StateError::Invalid("scsi transfer"));
        }
        self.odr = odr;
        self.icr = icr_v & 0x9f;
        self.mode = mode;
        self.tcr = tcr & 0x0f;
        self.dma_send = dma_send;
        self.dma_recv = dma_recv;
        self.target = target;
        self.phase = phase;
        self.req = req;
        self.acked = acked;
        self.data = data;
        self.cdb = cdb;
        self.buf = buf;
        self.pos = pos;
        self.out_lba = has_lba.then_some(lba);
        self.out_expected = out_expected;
        self.status = status;
        for (d, s) in self.disks.iter_mut().zip(sense) {
            d.sense = s;
            d.busy = false;
        }
        if let Some(t) = target {
            self.disks[t].busy = true;
        }
        Ok(())
    }
}

/// Bytes in a command descriptor block by its group.
fn cdb_len(op: u8) -> usize {
    match op >> 5 {
        1 | 2 => 10,
        5 => 12,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::media::MediaBackend;

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
            self.0[o..o + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn commit(&mut self) -> Result<(), MediaError> {
            Ok(())
        }
    }

    fn bus() -> Scsi {
        let mut s = Scsi::new();
        let data: Vec<u8> = (0..64 * BLOCK).map(|i| (i / BLOCK) as u8).collect();
        s.disks[0]
            .insert(Media {
                name: "hd".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Mem(data)),
            })
            .unwrap();
        s
    }

    /// Select target 0 the way the SCSI Manager does, without ATN.
    fn select(s: &mut Scsi) {
        s.write(0, 0x80, false);
        s.write(2, 1, false); // arbitrate
        assert_ne!(s.read(1, false) & icr::AIP, 0);
        s.write(1, icr::DATA | icr::BSY | icr::SEL, false);
        s.write(2, 0, false);
        s.write(0, 0x81, false);
        s.write(1, icr::DATA | icr::SEL, false);
        assert_ne!(s.read(4, false) & 0x40, 0, "target busy");
        s.write(1, 0, false);
    }

    /// One byte by REQ/ACK: send `out` or read.
    fn byte(s: &mut Scsi, out: Option<u8>) -> u8 {
        assert_ne!(s.read(4, false) & 0x20, 0, "REQ");
        let v = match out {
            Some(b) => {
                s.write(0, b, false);
                s.write(1, icr::DATA | icr::ACK, false);
                b
            }
            None => {
                let v = s.read(0, false);
                s.write(1, icr::ACK, false);
                v
            }
        };
        s.write(1, 0, false);
        v
    }

    fn phase(s: &mut Scsi) -> u8 {
        s.read(4, false) >> 2 & 7
    }

    fn command(s: &mut Scsi, cdb: &[u8]) -> (Vec<u8>, u8) {
        select(s);
        assert_eq!(phase(s), 0b010, "command phase");
        for &b in cdb {
            byte(s, Some(b));
        }
        let mut data = Vec::new();
        while phase(s) == 0b001 {
            data.push(byte(s, None));
        }
        assert_eq!(phase(s), 0b011, "status phase");
        let status = byte(s, None);
        assert_eq!(phase(s), 0b111, "message in");
        assert_eq!(byte(s, None), 0);
        assert_eq!(s.read(4, false) & 0x40, 0, "bus free");
        (data, status)
    }

    #[test]
    fn inquiry_read_and_write() {
        let mut s = bus();
        // No unit attention after power-on (the Mac ROM does not ask for
        // sense and would retry forever).
        assert_eq!(command(&mut s, &[0, 0, 0, 0, 0, 0]).1, 0);
        let (sense, st) = command(&mut s, &[3, 0, 0, 0, 18, 0]);
        assert_eq!((sense[2], st), (0, 0));
        let (inq, st) = command(&mut s, &[0x12, 0, 0, 0, 36, 0]);
        assert_eq!(st, 0);
        assert_eq!(&inq[8..15], b"QUANTUM");
        let (cap, _) = command(&mut s, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(cap, [0, 0, 0, 63, 0, 0, 2, 0]);
        let (blk, st) = command(&mut s, &[8, 0, 0, 5, 1, 0]);
        assert_eq!((blk.len(), blk[0], st), (BLOCK, 5, 0));
        // WRITE(6) one block at 2 with pseudo-DMA, then read it back.
        select(&mut s);
        for b in [0x0a, 0, 0, 2, 1, 0] {
            byte(&mut s, Some(b));
        }
        assert_eq!(phase(&mut s), 0b000);
        s.write(3, 0, false);
        s.write(2, 2, false);
        s.write(5, 0, false);
        for i in 0..BLOCK {
            s.write(0, 0xa0 ^ i as u8, true);
        }
        s.write(2, 0, false);
        assert_eq!(phase(&mut s), 0b011);
        assert_eq!(byte(&mut s, None), 0);
        byte(&mut s, None);
        let (blk, _) = command(&mut s, &[8, 0, 0, 2, 1, 0]);
        assert_eq!(blk[3], 0xa3);
        // Out of range.
        assert_eq!(command(&mut s, &[8, 0, 0, 64, 1, 0]).1, 2);
    }

    #[test]
    fn savestate_round_trip() {
        let mut s = bus();
        select(&mut s);
        byte(&mut s, Some(0x12));
        let mut w = StateWriter::new();
        s.save(&mut w);
        let b = w.into_bytes();
        let mut t = bus();
        t.load(&mut StateReader::new(&b)).unwrap();
        assert_eq!(t.cdb, [0x12]);
        assert_eq!(t.phase, Phase::Command);
    }
}
