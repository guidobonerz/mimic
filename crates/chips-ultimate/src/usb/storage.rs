//! A USB stick: a high-speed mass-storage function with the Bulk-Only
//! Transport and the SCSI block commands, on one of the board's storage
//! units.
//!
//! Descriptors: device class 0, control packets of 64 bytes, one
//! configuration (value 1) with one interface of class 8 (mass storage),
//! subclass 6 (SCSI transparent), protocol `$50` (bulk only) and two bulk
//! endpoints of 512 bytes, `$81` IN and `$02` OUT. One logical unit;
//! sectors of 512 bytes ([`SECTOR`]).
//!
//! Bulk-Only Transport (USB Mass Storage Class Bulk-Only Transport 1.0):
//! the host sends a 31-byte command block wrapper ("USBC", tag, data
//! length, flags, LUN, CDB length, CDB) on the OUT endpoint, then the data
//! stage in the direction the wrapper names, then reads the 13-byte status
//! wrapper ("USBS", the tag, residue, status) on the IN endpoint.
//!
//! To keep the firmware's driver safe the data stage always has exactly the
//! length the wrapper announced: an answer shorter than that is padded with
//! zeros (the residue says how much was padding), a longer one is cut, data
//! the command does not want is taken and dropped. A failed command still
//! runs its whole data stage and then reports status 1; the host learns the
//! reason with REQUEST SENSE. The firmware's `bulk_in` loops until all bytes
//! it asked for have come (UE2-C64U-Emulator `docs/hw/09-usb.md` H10), so an
//! IN token here is always answered with data, never with an empty packet,
//! while a stage is under way; outside of one it gets NAK.
//!
//! SCSI commands (SPC/SBC): TEST UNIT READY, REQUEST SENSE (fixed format,
//! 18 bytes), INQUIRY (36 bytes, removable), READ CAPACITY(10), READ(6),
//! READ(10), WRITE(6), WRITE(10), MODE SENSE(6) and (10) (write protect in
//! the device-specific byte), READ FORMAT CAPACITIES, and as no-ops PREVENT
//! ALLOW MEDIUM REMOVAL, START STOP UNIT, SYNCHRONIZE CACHE(10), VERIFY(10).
//! Anything else fails with ILLEGAL REQUEST / invalid command operation
//! code. Sense codes: 05/20 unknown command, 05/21 address out of range,
//! 05/24 invalid field, 05/25 LUN not supported, 07/27 write protected,
//! 03/11 read error, 03/0C write error, 02/3A no medium.
//!
//! Class requests: GET MAX LUN (`$A1 $FE`, one byte: 0) and Bulk-Only
//! Mass Storage Reset (`$21 $FF`).
//!
//! The firmware's driver (1541 Ultimate `software/io/usb/usb_scsi.cc`)
//! sends INQUIRY, REQUEST SENSE, TEST UNIT READY, READ CAPACITY(10),
//! READ(10) and WRITE(10) only, and GET MAX LUN, whose answer it uses
//! uninitialised when it is a STALL (09 H11).

use super::device::{Core, Env, Function, Response, Setup};
use crate::io::SECTOR;
use mimic_core::{StateError, StateReader, StateWriter};

/// The bulk endpoints.
const EP_IN: u8 = 1;
const EP_OUT: u8 = 2;
/// The size of a bulk packet at high speed.
const BULK_PACKET: usize = 512;

/// The CSW status codes.
const PASSED: u8 = 0;
const FAILED: u8 = 1;
const PHASE_ERROR: u8 = 2;

/// Where the Bulk-Only Transport is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for a command block wrapper.
    Command,
    /// Data to the host: `buf[pos..]`, refilled from the medium while
    /// `blocks` are left, then zeros, `remaining` bytes in all.
    DataIn,
    /// Data from the host, `remaining` bytes: whole sectors go to the
    /// medium while `blocks` are left, the rest is dropped.
    DataOut,
    /// The status wrapper waits to be read.
    Status,
}

impl Phase {
    const ALL: [Phase; 4] = [Phase::Command, Phase::DataIn, Phase::DataOut, Phase::Status];
}

/// What a command does in its data stage.
enum Plan {
    None,
    /// These bytes to the host.
    Bytes(Vec<u8>),
    /// Sectors from the medium.
    Read {
        lba: u64,
        count: u32,
    },
    /// Sectors to the medium.
    Write {
        lba: u64,
        count: u32,
    },
    /// Data from the host that is dropped.
    Discard,
}

/// Sense data: key, additional code, qualifier.
type Sense = (u8, u8, u8);
const NO_SENSE: Sense = (0, 0, 0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MassStorage {
    core: Core,
    /// The storage unit behind the stick.
    pub unit: u8,
    /// The unit's generation when the stick was plugged in: another one
    /// means another medium, so another stick.
    pub generation: u32,
    /// The hub port number (1…) for the serial number.
    port: u8,
    phase: Phase,
    tag: u32,
    /// Bytes still to move in the data stage.
    remaining: u32,
    /// The residue the status wrapper reports.
    residue: u32,
    status: u8,
    sense: Sense,
    /// Data to the host not yet sent / data from the host not yet written.
    buf: Vec<u8>,
    pos: usize,
    lba: u64,
    blocks: u32,
}

impl MassStorage {
    pub fn new(unit: u8, generation: u32, port: u8) -> Self {
        Self {
            core: Core::default(),
            unit,
            generation,
            port,
            phase: Phase::Command,
            tag: 0,
            remaining: 0,
            residue: 0,
            status: PASSED,
            sense: NO_SENSE,
            buf: Vec::new(),
            pos: 0,
            lba: 0,
            blocks: 0,
        }
    }

    /// A bus reset: default address and the transport back to its start.
    pub fn bus_reset(&mut self) {
        self.core.reset();
        self.reset_transport();
    }

    fn reset_transport(&mut self) {
        self.phase = Phase::Command;
        self.remaining = 0;
        self.buf.clear();
        self.pos = 0;
        self.blocks = 0;
    }

    fn sectors(&self, env: &Env<'_>) -> Option<u64> {
        env.storage.as_ref()?.sectors(self.unit).filter(|&n| n > 0)
    }

    fn protected(&self, env: &Env<'_>) -> bool {
        env.storage
            .as_ref()
            .is_some_and(|s| s.write_protected(self.unit))
    }

    /// A command block wrapper arrived.
    fn command(&mut self, cbw: &[u8], env: &mut Env<'_>) {
        // An invalid wrapper is not a command; the host's next one is
        // waited for (a real device would stall until a reset recovery).
        if cbw.len() != 31 || cbw[..4] != *b"USBC" {
            return;
        }
        let cb_len = usize::from(cbw[14] & 0x1F);
        if !(1..=16).contains(&cb_len) {
            return;
        }
        self.tag = u32::from_le_bytes([cbw[4], cbw[5], cbw[6], cbw[7]]);
        let host_len = u32::from_le_bytes([cbw[8], cbw[9], cbw[10], cbw[11]]);
        let host_in = cbw[12] & 0x80 != 0;
        let lun = cbw[13] & 0x0F;
        let mut cdb = [0u8; 16];
        cdb[..cb_len].copy_from_slice(&cbw[15..15 + cb_len]);

        let (plan, sense) = if lun != 0 {
            (Plan::None, (5, 0x25, 0))
        } else {
            self.execute(&cdb, env)
        };
        self.status = if sense == NO_SENSE { PASSED } else { FAILED };
        // REQUEST SENSE hands the pending sense over and clears it; every
        // other command leaves its own.
        if cdb[0] != 0x03 || sense != NO_SENSE {
            self.sense = sense;
        }
        self.buf.clear();
        self.pos = 0;
        self.blocks = 0;
        self.remaining = host_len;
        self.residue = 0;

        // The data stage, fitted to what the host announced.
        let (natural, wants_in) = match &plan {
            Plan::None => (0u64, None),
            Plan::Bytes(b) => (b.len() as u64, Some(true)),
            Plan::Read { count, .. } => (u64::from(*count) * SECTOR as u64, Some(true)),
            Plan::Write { count, .. } => (u64::from(*count) * SECTOR as u64, Some(false)),
            Plan::Discard => (0, Some(false)),
        };
        let host = u64::from(host_len);
        // The host announced the other direction than the command has.
        let crossed = wants_in.is_some_and(|d| host > 0 && d != host_in);
        // Or less than the command has (also none at all).
        if crossed || natural > host {
            self.status = PHASE_ERROR;
        }
        if !crossed {
            match plan {
                Plan::Bytes(b) if host_in => self.buf = b,
                Plan::Read { lba, count } if host_in => {
                    self.lba = lba;
                    self.blocks = count;
                }
                Plan::Write { lba, count } if !host_in => {
                    self.lba = lba;
                    self.blocks = count;
                }
                _ => {}
            }
        }
        let moved = if crossed { 0 } else { natural.min(host) };
        self.residue = (host - moved) as u32;
        self.phase = match (host_len, host_in) {
            (0, _) => Phase::Status,
            (_, true) => Phase::DataIn,
            (_, false) => Phase::DataOut,
        };
    }

    /// The command's data stage and its sense (`NO_SENSE`: it succeeds).
    fn execute(&mut self, cdb: &[u8; 16], env: &mut Env<'_>) -> (Plan, Sense) {
        let Some(sectors) = self.sectors(env) else {
            return match cdb[0] {
                0x03 => (Plan::Bytes(self.sense_bytes(cdb[4])), NO_SENSE),
                0x12 => (Plan::Bytes(self.inquiry(cdb)), NO_SENSE),
                _ => (Plan::None, (2, 0x3A, 0)),
            };
        };
        let be16 = |i: usize| u32::from(cdb[i]) << 8 | u32::from(cdb[i + 1]);
        let be32 = |i: usize| u32::from_be_bytes([cdb[i], cdb[i + 1], cdb[i + 2], cdb[i + 3]]);
        let fit = |b: Vec<u8>, alloc: u32| {
            let mut b = b;
            b.truncate(alloc as usize);
            b
        };
        let last = (sectors - 1).min(u64::from(u32::MAX)) as u32;
        match cdb[0] {
            // TEST UNIT READY, PREVENT ALLOW MEDIUM REMOVAL, START STOP
            // UNIT, SYNCHRONIZE CACHE(10), VERIFY(10).
            0x00 | 0x1E | 0x1B | 0x35 | 0x2F => (Plan::None, NO_SENSE),
            0x03 => (Plan::Bytes(self.sense_bytes(cdb[4])), NO_SENSE),
            0x12 => {
                if cdb[1] & 1 != 0 {
                    (Plan::None, (5, 0x24, 0))
                } else {
                    (Plan::Bytes(fit(self.inquiry(cdb), be16(3))), NO_SENSE)
                }
            }
            0x25 => {
                let mut b = last.to_be_bytes().to_vec();
                b.extend_from_slice(&(SECTOR as u32).to_be_bytes());
                (Plan::Bytes(b), NO_SENSE)
            }
            0x23 => {
                let mut b = vec![0, 0, 0, 8];
                b.extend_from_slice(&(last.saturating_add(1)).to_be_bytes());
                b.extend_from_slice(&[2, 0, (SECTOR >> 8) as u8, SECTOR as u8]);
                (Plan::Bytes(fit(b, be16(7))), NO_SENSE)
            }
            0x1A => {
                let wp = if self.protected(env) { 0x80 } else { 0 };
                (
                    Plan::Bytes(fit(vec![3, 0, wp, 0], u32::from(cdb[4]))),
                    NO_SENSE,
                )
            }
            0x5A => {
                let wp = if self.protected(env) { 0x80 } else { 0 };
                let b = vec![0, 6, 0, wp, 0, 0, 0, 0];
                (Plan::Bytes(fit(b, be16(7))), NO_SENSE)
            }
            0x08 | 0x0A | 0x28 | 0x2A => {
                let (lba, count) = if cdb[0] < 0x10 {
                    let lba = u32::from(cdb[1] & 0x1F) << 16 | be16(2);
                    let n = if cdb[4] == 0 { 256 } else { u32::from(cdb[4]) };
                    (lba, n)
                } else {
                    (be32(2), be16(7))
                };
                let write = cdb[0] & 2 != 0;
                let lba = u64::from(lba);
                if lba + u64::from(count) > sectors {
                    let p = if write { Plan::Discard } else { Plan::None };
                    return (p, (5, 0x21, 0));
                }
                if write && self.protected(env) {
                    return (Plan::Discard, (7, 0x27, 0));
                }
                if write {
                    (Plan::Write { lba, count }, NO_SENSE)
                } else {
                    (Plan::Read { lba, count }, NO_SENSE)
                }
            }
            _ => (Plan::None, (5, 0x20, 0)),
        }
    }

    /// Fixed-format sense data (SPC), cut to `alloc`; the pending sense is
    /// cleared by handing it over.
    fn sense_bytes(&mut self, alloc: u8) -> Vec<u8> {
        let (key, asc, ascq) = std::mem::replace(&mut self.sense, NO_SENSE);
        let mut b = vec![0u8; 18];
        b[0] = 0x70;
        b[2] = key;
        b[7] = 10;
        b[12] = asc;
        b[13] = ascq;
        b.truncate(usize::from(alloc));
        b
    }

    /// Standard INQUIRY data: direct access, removable, SPC-2.
    fn inquiry(&self, _cdb: &[u8; 16]) -> Vec<u8> {
        let mut b = vec![0x00, 0x80, 0x04, 0x02, 31, 0, 0, 0];
        b.extend_from_slice(b"mimic   ");
        b.extend_from_slice(b"USB Stick       ");
        b.extend_from_slice(b"1.00");
        b
    }

    fn csw(&self) -> [u8; 13] {
        let mut c = [0u8; 13];
        c[..4].copy_from_slice(b"USBS");
        c[4..8].copy_from_slice(&self.tag.to_le_bytes());
        c[8..12].copy_from_slice(&self.residue.to_le_bytes());
        c[12] = self.status;
        c
    }

    /// Sense after a medium failure in the data stage.
    fn fail(&mut self, sense: Sense) {
        self.status = FAILED;
        self.sense = sense;
        self.blocks = 0;
    }

    fn data_in(&mut self, max: usize, out: &mut Vec<u8>, env: &mut Env<'_>) -> usize {
        let want = max.min(self.remaining as usize);
        while out.len() < want {
            if self.pos < self.buf.len() {
                let n = (self.buf.len() - self.pos).min(want - out.len());
                out.extend_from_slice(&self.buf[self.pos..self.pos + n]);
                self.pos += n;
                continue;
            }
            self.buf.clear();
            self.pos = 0;
            if self.blocks > 0 {
                let mut sector = [0u8; SECTOR];
                let ok = match env.storage.as_mut() {
                    Some(s) => s.read(self.unit, self.lba, &mut sector),
                    None => false,
                };
                if ok {
                    self.buf.extend_from_slice(&sector);
                    self.lba += 1;
                    self.blocks -= 1;
                    continue;
                }
                self.fail((3, 0x11, 0));
            }
            // Padding up to what the host announced.
            out.resize(want, 0);
        }
        self.remaining -= want as u32;
        if self.remaining == 0 {
            self.buf.clear();
            self.pos = 0;
            self.phase = Phase::Status;
        }
        want
    }

    fn data_out(&mut self, data: &[u8], env: &mut Env<'_>) {
        let take = data.len().min(self.remaining as usize);
        self.remaining -= take as u32;
        if self.blocks > 0 {
            self.buf.extend_from_slice(&data[..take]);
            while self.blocks > 0 && self.buf.len() >= SECTOR {
                let ok = match env.storage.as_mut() {
                    Some(s) => s.write(self.unit, self.lba, &self.buf[..SECTOR]),
                    None => false,
                };
                self.buf.drain(..SECTOR);
                if !ok {
                    let protected = self.protected(env);
                    self.fail(if protected {
                        (7, 0x27, 0)
                    } else {
                        (3, 0x0C, 0)
                    });
                    break;
                }
                self.lba += 1;
                self.blocks -= 1;
            }
            if self.blocks == 0 {
                self.buf.clear();
            }
        }
        if self.remaining == 0 {
            self.buf.clear();
            self.phase = Phase::Status;
        }
    }

    pub fn save(&self, w: &mut StateWriter) {
        self.core.save(w);
        w.u8(self.unit);
        w.u32(self.generation);
        w.u8(self.port);
        w.u8(self.phase as u8);
        w.u32(self.tag);
        w.u32(self.remaining);
        w.u32(self.residue);
        w.u8(self.status);
        w.u8(self.sense.0);
        w.u8(self.sense.1);
        w.u8(self.sense.2);
        w.bytes(&self.buf);
        w.u32(self.pos as u32);
        w.u64(self.lba);
        w.u32(self.blocks);
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let core = Core::load(r)?;
        let unit = r.u8()?;
        let generation = r.u32()?;
        let port = r.u8()?;
        let phase = *Phase::ALL
            .get(usize::from(r.u8()?))
            .ok_or(StateError::Invalid("USB storage phase"))?;
        let tag = r.u32()?;
        let remaining = r.u32()?;
        let residue = r.u32()?;
        let status = r.u8()?;
        let sense = (r.u8()?, r.u8()?, r.u8()?);
        let buf = r.bytes()?;
        let pos = r.u32()? as usize;
        let lba = r.u64()?;
        let blocks = r.u32()?;
        if buf.len() > 2 * SECTOR || pos > buf.len() || status > PHASE_ERROR {
            return Err(StateError::Invalid("USB storage buffer"));
        }
        Ok(Self {
            core,
            unit,
            generation,
            port,
            phase,
            tag,
            remaining,
            residue,
            status,
            sense,
            buf: buf.to_vec(),
            pos,
            lba,
            blocks,
        })
    }
}

impl Function for MassStorage {
    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn device_descriptor(&self) -> [u8; 18] {
        [
            18, 1, 0x00, 0x02, // USB 2.0
            0x00, 0x00, 0x00, 64, // class in the interface, EP0 64 bytes
            0x09, 0x12, 0x01, 0x00, // VID $1209, PID $0001 (pid.codes test)
            0x00, 0x01, // device release 1.00
            1, 2, 3, // manufacturer, product, serial
            1, // one configuration
        ]
    }

    fn configuration_descriptor(&self) -> Vec<u8> {
        vec![
            9,
            2,
            32,
            0,
            1,
            1,
            0,
            0x80,
            50, // bus powered, 100 mA
            9,
            4,
            0,
            0,
            2,
            0x08,
            0x06,
            0x50,
            0, // mass storage, SCSI, bulk only
            7,
            5,
            0x80 | EP_IN,
            2,
            0x00,
            0x02,
            0, // bulk IN, 512
            7,
            5,
            EP_OUT,
            2,
            0x00,
            0x02,
            0, // bulk OUT, 512
        ]
    }

    fn string(&self, index: u8) -> Option<String> {
        match index {
            1 => Some("mimic".into()),
            2 => Some("USB Stick".into()),
            3 => Some(format!("MIMIC0000{:02}", self.port)),
            _ => None,
        }
    }

    fn class_in(&mut self, s: &Setup, _env: &mut Env<'_>) -> Option<Vec<u8>> {
        // GET MAX LUN: one logical unit.
        (s.request_type == 0xA1 && s.request == 0xFE).then(|| vec![0])
    }

    fn class_out(&mut self, s: &Setup, _data: &[u8], _env: &mut Env<'_>) -> bool {
        // Bulk-Only Mass Storage Reset.
        if s.request_type == 0x21 && s.request == 0xFF {
            self.reset_transport();
            return true;
        }
        false
    }

    fn endpoint_in(
        &mut self,
        ep: u8,
        max: usize,
        buf: &mut Vec<u8>,
        env: &mut Env<'_>,
    ) -> Response {
        if ep != EP_IN || max == 0 {
            return if ep == EP_IN {
                Response::Nak
            } else {
                Response::Stall
            };
        }
        let max = max.min(BULK_PACKET);
        match self.phase {
            Phase::DataIn => Response::Data(self.data_in(max, buf, env)),
            Phase::Status => {
                let c = self.csw();
                let n = c.len().min(max);
                buf.extend_from_slice(&c[..n]);
                if n == c.len() {
                    self.phase = Phase::Command;
                }
                Response::Data(n)
            }
            Phase::Command | Phase::DataOut => Response::Nak,
        }
    }

    fn endpoint_out(&mut self, ep: u8, data: &[u8], env: &mut Env<'_>) -> Response {
        if ep != EP_OUT {
            return Response::Stall;
        }
        match self.phase {
            Phase::Command => {
                self.command(data, env);
                Response::Ack
            }
            Phase::DataOut => {
                self.data_out(data, env);
                Response::Ack
            }
            Phase::DataIn | Phase::Status => Response::Nak,
        }
    }

    fn configured(&mut self, _value: u8) {
        self.reset_transport();
    }
}
