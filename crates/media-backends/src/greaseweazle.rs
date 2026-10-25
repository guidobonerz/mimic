//! Greaseweazle: a USB flux reader for floppy drives, as a USB CDC serial
//! device (on Windows a COM port, `/dev/ttyACM*` on Linux).
//!
//! Protocol (host side, written from the firmware's protocol header):
//! a command is `[cmd, length, args…]` (multi-byte numbers little-endian),
//! answered by `[cmd, ack]` with ack 0 = OK; commands do not pipeline.
//! Used here: GET_INFO (0; index 0 returns 32 bytes: firmware major/minor,
//! main-firmware flag, max command, sample frequency u32, hardware model
//! …), SEEK (2; signed cylinder, 8 or 16 bit), HEAD (3), MOTOR (6; unit,
//! on), READ_FLUX (7; ticks u32 = 0, index pulses u16: the stream ends
//! after that many), GET_FLUX_STATUS (9; the read's result in the ack),
//! SELECT (12), DESELECT (13), SET_BUS_TYPE (14; 1 = IBM PC, 2 = Shugart).
//! Communications are reset by setting the line to 10000 baud, then 9600.
//!
//! The flux stream: bytes 1–249 are an interval of that many ticks;
//! 250–254 with a following byte `b` are `250 + (x - 250) · 255 + b - 1`;
//! 255 starts an opcode: 1 = INDEX, 2 = SPACE, each with a 28-bit number in
//! four bytes (bit 0 set, seven data bits each, least significant first):
//! INDEX marks an index pulse that many ticks after the current position,
//! SPACE adds ticks without a transition. A 0 byte ends the stream.
//!
//! Sources: Greaseweazle firmware `inc/cdc_acm_protocol.h` and host tools
//! `src/greaseweazle/usb.py` (Keir Fraser, released into the public domain
//! under the Unlicense), <https://github.com/keirf/greaseweazle-firmware>,
//! <https://github.com/keirf/greaseweazle>. No code was copied.

use std::io::{self, Read, Write};
use std::time::Duration;

use emu_core::media::MediaError;

use crate::flux::disk::FluxSource;
use crate::flux::Flux;

pub const CMD_GET_INFO: u8 = 0;
pub const CMD_SEEK: u8 = 2;
pub const CMD_HEAD: u8 = 3;
pub const CMD_MOTOR: u8 = 6;
pub const CMD_READ_FLUX: u8 = 7;
pub const CMD_GET_FLUX_STATUS: u8 = 9;
pub const CMD_SELECT: u8 = 12;
pub const CMD_DESELECT: u8 = 13;
pub const CMD_SET_BUS_TYPE: u8 = 14;

pub const BUS_IBMPC: u8 = 1;
pub const BUS_SHUGART: u8 = 2;

pub const FLUXOP_INDEX: u8 = 1;
pub const FLUXOP_SPACE: u8 = 2;

/// USB vendor and product id of the Greaseweazle (pid.codes).
pub const USB_VID: u16 = 0x1209;
pub const USB_PID: u16 = 0x4d69;

/// The firmware's name for an ack code.
pub fn ack_name(code: u8) -> &'static str {
    match code {
        0 => "okay",
        1 => "bad command",
        2 => "no index pulse (no disk, or the drive motor is off)",
        3 => "track 0 not found",
        4 => "flux overflow (USB too slow)",
        5 => "flux underflow",
        6 => "disk is write-protected",
        7 => "no drive unit selected",
        8 => "no bus type set",
        9 => "bad drive unit",
        10 => "bad pin",
        11 => "bad cylinder",
        12 => "out of SRAM",
        13 => "out of flash",
        _ => "unknown error",
    }
}

/// The byte channel to the device: a serial port, or a test double.
pub trait Transport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Read at least one byte (waiting up to the transport's timeout).
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    /// Reset communications (the 10000-baud request).
    fn clear_comms(&mut self) -> io::Result<()>;

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let mut at = 0;
        while at < buf.len() {
            match self.read(&mut buf[at..])? {
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                n => at += n,
            }
        }
        Ok(())
    }
}

/// What GET_INFO reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub fw_major: u8,
    pub fw_minor: u8,
    pub is_main_firmware: bool,
    pub max_cmd: u8,
    pub sample_freq: u32,
    pub hw_model: u8,
    pub hw_submodel: u8,
}

impl Info {
    fn parse(b: &[u8; 32]) -> Self {
        Self {
            fw_major: b[0],
            fw_minor: b[1],
            is_main_firmware: b[2] != 0,
            max_cmd: b[3],
            sample_freq: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            hw_model: b[8],
            hw_submodel: b[9],
        }
    }
}

fn io_err(e: io::Error) -> MediaError {
    MediaError::Backend(format!("Greaseweazle: {e}"))
}

/// Decode a flux stream (without its final 0) into intervals and index
/// pulse positions (ticks from the start).
pub fn decode_stream(data: &[u8], sample_freq: u32) -> Result<Flux, MediaError> {
    let bad = |m: &str| MediaError::Backend(format!("Greaseweazle flux stream: {m}"));
    let mut flux = Flux {
        sample_freq,
        intervals: Vec::with_capacity(data.len()),
        index: Vec::new(),
    };
    let mut now: u64 = 0;
    let mut pending: u64 = 0;
    let mut i = 0;
    let n28 = |at: usize| -> Option<u64> {
        let b = data.get(at..at + 4)?;
        Some(
            u64::from(b[0] >> 1)
                | u64::from(b[1] >> 1) << 7
                | u64::from(b[2] >> 1) << 14
                | u64::from(b[3] >> 1) << 21,
        )
    };
    while i < data.len() {
        let b = data[i];
        match b {
            0 => break,
            255 => {
                let op = *data.get(i + 1).ok_or_else(|| bad("truncated opcode"))?;
                let v = n28(i + 2).ok_or_else(|| bad("truncated opcode argument"))?;
                match op {
                    FLUXOP_INDEX => flux.index.push(now + pending + v),
                    FLUXOP_SPACE => pending += v,
                    _ => return Err(bad(&format!("unknown opcode {op}"))),
                }
                i += 6;
            }
            250..=254 => {
                let next = *data.get(i + 1).ok_or_else(|| bad("truncated interval"))?;
                let v = 250 + u64::from(b - 250) * 255 + u64::from(next) - 1;
                pending += v;
                now += pending;
                flux.intervals
                    .push(u32::try_from(pending).unwrap_or(u32::MAX));
                pending = 0;
                i += 2;
            }
            _ => {
                pending += u64::from(b);
                now += pending;
                flux.intervals
                    .push(u32::try_from(pending).unwrap_or(u32::MAX));
                pending = 0;
                i += 1;
            }
        }
    }
    Ok(flux)
}

fn push_n28(out: &mut Vec<u8>, v: u64) {
    for shift in [0, 7, 14, 21] {
        out.push(1 | ((v >> shift) as u8) << 1);
    }
}

/// Encode flux as the device sends it (with the final 0): the reverse of
/// [`decode_stream`], for test doubles.
pub fn encode_stream(flux: &Flux) -> Vec<u8> {
    let mut out = Vec::with_capacity(flux.intervals.len() + 16);
    let mut index = flux.index.iter().peekable();
    let mut now: u64 = 0;
    for &d in &flux.intervals {
        let end = now + u64::from(d);
        while let Some(&&at) = index.peek() {
            if at >= end {
                break;
            }
            out.extend_from_slice(&[255, FLUXOP_INDEX]);
            push_n28(&mut out, at.saturating_sub(now));
            index.next();
        }
        let mut v = u64::from(d);
        // Longer than the two-byte form: SPACE for the rest.
        const MAX2: u64 = 250 + 4 * 255 + 254;
        if v > MAX2 {
            out.extend_from_slice(&[255, FLUXOP_SPACE]);
            push_n28(&mut out, v - 249);
            v = 249;
        }
        if v < 250 {
            out.push(v.max(1) as u8);
        } else {
            let x = v - 250;
            out.push(250 + (x / 255) as u8);
            out.push((x % 255) as u8 + 1);
        }
        now = end;
    }
    for &at in index {
        out.extend_from_slice(&[255, FLUXOP_INDEX]);
        push_n28(&mut out, at.saturating_sub(now));
    }
    out.push(0);
    out
}

/// Which drive on which bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveSelect {
    pub bus: u8,
    pub unit: u8,
}

impl DriveSelect {
    /// `a`/`b` = IBM PC bus (drive after/before the cable twist, units 0
    /// and 1), `0`–`2` = Shugart bus units.
    pub fn parse(s: &str) -> Result<Self, MediaError> {
        match s.to_ascii_lowercase().as_str() {
            "a" => Ok(Self {
                bus: BUS_IBMPC,
                unit: 0,
            }),
            "b" => Ok(Self {
                bus: BUS_IBMPC,
                unit: 1,
            }),
            n @ ("0" | "1" | "2") => Ok(Self {
                bus: BUS_SHUGART,
                unit: n.parse().unwrap_or(0),
            }),
            other => Err(MediaError::Control(format!(
                "drive '{other}': a or b (IBM PC cable), 0, 1 or 2 (Shugart)"
            ))),
        }
    }
}

/// A Greaseweazle on a transport.
pub struct Greaseweazle<T: Transport> {
    transport: T,
    pub info: Info,
    drive: Option<DriveSelect>,
}

impl<T: Transport> Greaseweazle<T> {
    /// Reset communications and read the firmware information.
    pub fn new(mut transport: T) -> Result<Self, MediaError> {
        transport.clear_comms().map_err(io_err)?;
        let mut gw = Self {
            transport,
            info: Info::parse(&[0; 32]),
            drive: None,
        };
        gw.command(&[CMD_GET_INFO, 3, 0])?;
        let mut b = [0u8; 32];
        gw.transport.read_exact(&mut b).map_err(io_err)?;
        gw.info = Info::parse(&b);
        if !gw.info.is_main_firmware {
            return Err(MediaError::Backend(
                "Greaseweazle is in update (bootloader) mode: run `gw update` first".into(),
            ));
        }
        if gw.info.fw_major < 1 || gw.info.sample_freq == 0 {
            return Err(MediaError::Backend(format!(
                "Greaseweazle firmware {}.{} is too old (needs 1.0 or later)",
                gw.info.fw_major, gw.info.fw_minor
            )));
        }
        Ok(gw)
    }

    /// Send a command and check its ack.
    fn command(&mut self, cmd: &[u8]) -> Result<(), MediaError> {
        self.transport.write_all(cmd).map_err(io_err)?;
        let mut ack = [0u8; 2];
        self.transport.read_exact(&mut ack).map_err(io_err)?;
        if ack[0] != cmd[0] {
            return Err(MediaError::Backend(format!(
                "Greaseweazle: answer to command {} was for {}",
                cmd[0], ack[0]
            )));
        }
        if ack[1] != 0 {
            return Err(MediaError::Backend(format!(
                "Greaseweazle command {}: {} ({})",
                cmd[0],
                ack_name(ack[1]),
                ack[1]
            )));
        }
        Ok(())
    }

    /// Select a drive and start its motor.
    pub fn select(&mut self, drive: DriveSelect) -> Result<(), MediaError> {
        self.command(&[CMD_SET_BUS_TYPE, 3, drive.bus])?;
        self.command(&[CMD_SELECT, 3, drive.unit])?;
        self.command(&[CMD_MOTOR, 4, drive.unit, 1])?;
        self.drive = Some(drive);
        Ok(())
    }

    /// Stop the motor and deselect.
    pub fn release(&mut self) -> Result<(), MediaError> {
        if let Some(d) = self.drive.take() {
            self.command(&[CMD_MOTOR, 4, d.unit, 0])?;
            self.command(&[CMD_DESELECT, 2])?;
        }
        Ok(())
    }

    pub fn seek(&mut self, cyl: i16, head: u8) -> Result<(), MediaError> {
        match i8::try_from(cyl) {
            Ok(c) => self.command(&[CMD_SEEK, 3, c as u8])?,
            Err(_) => {
                let [lo, hi] = cyl.to_le_bytes();
                self.command(&[CMD_SEEK, 4, lo, hi])?;
            }
        }
        self.command(&[CMD_HEAD, 3, head])
    }

    /// Read `revs` whole revolutions from the current track: the stream
    /// runs until the `revs + 1`-th index pulse.
    pub fn read(&mut self, revs: u8) -> Result<Flux, MediaError> {
        let max_index = u16::from(revs) + 1;
        let [i0, i1] = max_index.to_le_bytes();
        self.command(&[CMD_READ_FLUX, 8, 0, 0, 0, 0, i0, i1])?;
        let mut data = Vec::with_capacity(256 * 1024);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = self.transport.read(&mut buf).map_err(io_err)?;
            if n == 0 {
                return Err(MediaError::Backend("Greaseweazle: stream ended".into()));
            }
            if let Some(end) = buf[..n].iter().position(|&b| b == 0) {
                data.extend_from_slice(&buf[..end]);
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        self.command(&[CMD_GET_FLUX_STATUS, 2])?;
        decode_stream(&data, self.info.sample_freq)
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T: Transport> FluxSource for Greaseweazle<T> {
    fn read_flux(&mut self, cyl: u8, head: u8, revs: u8) -> Result<Flux, MediaError> {
        self.seek(i16::from(cyl), head)?;
        // A flux overflow (USB busy) is worth another try.
        let mut last = None;
        for _ in 0..3 {
            match self.read(revs) {
                Ok(f) => return Ok(f),
                Err(e) if e.to_string().contains("overflow") => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| MediaError::Backend("Greaseweazle read failed".into())))
    }
}

/// The Greaseweazle's serial port.
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    pub fn open(name: &str) -> Result<Self, MediaError> {
        let port = serialport::new(name, 9600)
            .timeout(Duration::from_secs(5))
            .open()
            .map_err(|e| MediaError::Backend(format!("{name}: {e}")))?;
        Ok(Self { port })
    }
}

impl Transport for SerialTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        Write::write_all(&mut self.port, data)?;
        self.port.flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Read::read(&mut self.port, buf)
    }

    fn clear_comms(&mut self) -> io::Result<()> {
        let e = |e: serialport::Error| io::Error::other(e.to_string());
        self.port
            .clear(serialport::ClearBuffer::Output)
            .map_err(e)?;
        self.port.set_baud_rate(10_000).map_err(e)?;
        self.port.set_baud_rate(9600).map_err(e)?;
        self.port.clear(serialport::ClearBuffer::Input).map_err(e)
    }
}

/// Serial ports that look like a Greaseweazle (USB id 1209:4d69).
pub fn find_ports() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            matches!(&p.port_type, serialport::SerialPortType::UsbPort(u)
                if u.vid == USB_VID && u.pid == USB_PID)
        })
        .map(|p| p.port_name)
        .collect()
}

/// Open the Greaseweazle on `port` (`auto`: the only one found).
pub fn open(port: &str) -> Result<Greaseweazle<SerialTransport>, MediaError> {
    let name = if port.eq_ignore_ascii_case("auto") {
        let found = find_ports();
        match found.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(MediaError::Backend(
                    "no Greaseweazle found (USB 1209:4d69); give its port, e.g. COM5".into(),
                ))
            }
            many => {
                return Err(MediaError::Backend(format!(
                    "several Greaseweazles: {}; pick one",
                    many.join(", ")
                )))
            }
        }
    } else {
        port.to_string()
    };
    Greaseweazle::new(SerialTransport::open(&name)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flux::disk::{import, DiskFormat, ImportOptions, Output, Step};
    use crate::flux::{encode_bits, unpack, Rng};
    use std::collections::VecDeque;

    #[test]
    fn stream_round_trip() {
        let flux = Flux {
            sample_freq: 72_000_000,
            intervals: vec![1, 100, 249, 250, 251, 504, 505, 1524, 1525, 99_999, 3],
            index: vec![50, 1000, 100_000, 200_000],
        };
        let bytes = encode_stream(&flux);
        assert_eq!(*bytes.last().unwrap(), 0);
        assert!(!bytes[..bytes.len() - 1].contains(&0));
        let back = decode_stream(&bytes[..bytes.len() - 1], 72_000_000).unwrap();
        assert_eq!(back, flux);
        // Known encodings: 250 → [250, 1]; 504 → [250, 255]; 505 → [251, 1].
        assert_eq!(
            encode_stream(&Flux {
                sample_freq: 1,
                intervals: vec![250, 504, 505],
                index: vec![],
            }),
            [250, 1, 250, 255, 251, 1, 0]
        );
        assert!(decode_stream(&[255, 9, 1, 1, 1, 1], 1).is_err());
        assert!(decode_stream(&[251], 1).is_err());
    }

    #[test]
    fn drive_names() {
        assert_eq!(
            DriveSelect::parse("A").unwrap(),
            DriveSelect {
                bus: BUS_IBMPC,
                unit: 0
            }
        );
        assert_eq!(DriveSelect::parse("2").unwrap().bus, BUS_SHUGART);
        assert!(DriveSelect::parse("c").is_err());
    }

    /// A Greaseweazle in software: answers commands, plays flux of a
    /// 1541 disk at 72 MHz in a 40-track drive.
    struct FakeGw {
        input: VecDeque<u8>,
        cmd: Vec<u8>,
        log: Vec<Vec<u8>>,
        cyl: i16,
        head: u8,
        selected: bool,
        motor: bool,
        bus: u8,
        tracks: Vec<Vec<bool>>,
        rng: Rng,
        cleared: usize,
        /// Fail READ_FLUX once with this ack.
        fail_read: Option<u8>,
    }

    impl FakeGw {
        fn new(disk: &media::D64) -> Self {
            Self {
                input: VecDeque::new(),
                cmd: Vec::new(),
                log: Vec::new(),
                cyl: 0,
                head: 0,
                selected: false,
                motor: false,
                bus: 0,
                tracks: (1..=35u8)
                    .map(|t| unpack(&media::gcr::encode_track(disk, t)))
                    .collect(),
                rng: Rng::new(99),
                cleared: 0,
                fail_read: None,
            }
        }

        fn ack(&mut self, cmd: u8, code: u8) {
            self.input.extend([cmd, code]);
        }

        fn run(&mut self, c: &[u8]) {
            self.log.push(c.to_vec());
            let ok = |b: bool| if b { 0 } else { 1 };
            match c[0] {
                CMD_GET_INFO => {
                    self.ack(c[0], 0);
                    let mut info = [0u8; 32];
                    info[..4].copy_from_slice(&[1, 4, 1, 22]);
                    info[4..8].copy_from_slice(&72_000_000u32.to_le_bytes());
                    info[8] = 4;
                    self.input.extend(info);
                }
                CMD_SET_BUS_TYPE => {
                    self.bus = c[2];
                    self.ack(c[0], ok(c[2] == 1 || c[2] == 2));
                }
                CMD_SELECT => {
                    self.selected = self.bus != 0;
                    self.ack(c[0], if self.bus == 0 { 8 } else { 0 });
                }
                CMD_MOTOR => {
                    self.motor = c[3] != 0;
                    self.ack(c[0], 0);
                }
                CMD_DESELECT => {
                    self.selected = false;
                    self.ack(c[0], 0);
                }
                CMD_SEEK => {
                    self.cyl = if c[1] == 3 {
                        i16::from(c[2] as i8)
                    } else {
                        i16::from_le_bytes([c[2], c[3]])
                    };
                    self.ack(c[0], if self.selected { 0 } else { 7 });
                }
                CMD_HEAD => {
                    self.head = c[2];
                    self.ack(c[0], 0);
                }
                CMD_READ_FLUX => {
                    if !self.motor {
                        self.ack(c[0], 2);
                        return;
                    }
                    self.ack(c[0], 0);
                    let max_index = u16::from_le_bytes([c[6], c[7]]);
                    let track = self
                        .tracks
                        .get(self.cyl as usize)
                        .filter(|_| self.head == 0);
                    let flux = match track {
                        Some(bits) => {
                            let t = self.cyl as u8 + 1;
                            let zone = media::d64::speed_zone(t);
                            let cell = crate::flux::gcr::cell_seconds(zone) * 72e6;
                            let lead = (self.rng.next_u64() % 3000) as usize;
                            encode_bits(
                                bits,
                                cell,
                                72_000_000,
                                usize::from(max_index - 1),
                                0.08,
                                lead,
                                &mut self.rng,
                            )
                        }
                        None => Flux {
                            sample_freq: 72_000_000,
                            intervals: vec![14_400_000; 2],
                            index: vec![0, 14_400_000],
                        },
                    };
                    let stream = encode_stream(&flux);
                    self.input.extend(stream);
                }
                CMD_GET_FLUX_STATUS => match self.fail_read.take() {
                    Some(code) => self.ack(c[0], code),
                    None => self.ack(c[0], 0),
                },
                other => self.ack(other, 1),
            }
        }
    }

    impl Transport for FakeGw {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            for &b in data {
                self.cmd.push(b);
                if self.cmd.len() >= 2 && self.cmd.len() == usize::from(self.cmd[1]) {
                    let c = std::mem::take(&mut self.cmd);
                    self.run(&c);
                }
            }
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.input.is_empty() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            // Deliver in uneven chunks, like USB.
            let n = buf.len().min(self.input.len()).min(4093);
            for b in buf.iter_mut().take(n) {
                *b = self.input.pop_front().unwrap_or(0);
            }
            Ok(n)
        }

        fn clear_comms(&mut self) -> io::Result<()> {
            self.cleared += 1;
            self.input.clear();
            self.cmd.clear();
            Ok(())
        }
    }

    #[test]
    fn commands_and_framing() {
        let d = media::D64::blank(b"GW", *b"GW");
        let mut gw = Greaseweazle::new(FakeGw::new(&d)).unwrap();
        assert_eq!(gw.info.sample_freq, 72_000_000);
        assert_eq!((gw.info.fw_major, gw.info.fw_minor), (1, 4));
        // Reading before selecting: the device refuses.
        let e = gw.seek(3, 0).unwrap_err();
        assert!(e.to_string().contains("no drive unit"), "{e}");
        gw.select(DriveSelect::parse("a").unwrap()).unwrap();
        gw.seek(-1, 0).unwrap();
        gw.seek(300, 1).unwrap();
        let t = gw.into_transport();
        assert_eq!(t.cleared, 1);
        assert_eq!(t.log[0], [CMD_GET_INFO, 3, 0]);
        assert!(t.log.contains(&vec![CMD_SET_BUS_TYPE, 3, BUS_IBMPC]));
        assert!(t.log.contains(&vec![CMD_MOTOR, 4, 0, 1]));
        assert!(t.log.contains(&vec![CMD_SEEK, 3, 0xff]));
        assert!(t.log.contains(&vec![CMD_SEEK, 4, 0x2c, 0x01]));
        assert_eq!(t.log.last().unwrap(), &vec![CMD_HEAD, 3, 1]);
    }

    #[test]
    fn whole_d64_through_the_protocol() {
        let mut d = media::D64::blank(b"PROTOCOL", *b"PR");
        let data: Vec<u8> = (0..30_000u32).map(|i| (i * 7 % 256) as u8).collect();
        d.write_file(b"FILE", 2, &data).unwrap();
        let mut fake = FakeGw::new(&d);
        fake.fail_read = Some(4);
        let mut gw = Greaseweazle::new(fake).unwrap();
        gw.select(DriveSelect::parse("a").unwrap()).unwrap();
        let out = import(
            &mut gw,
            DiskFormat::Cbm1541 { tracks: 35 },
            Output::D64,
            ImportOptions::default(),
            &mut |_| {},
        )
        .unwrap();
        gw.release().unwrap();
        assert_eq!(out.step, Step::Single);
        assert!(out.missing.is_empty());
        assert_eq!(out.image, d.to_bytes());
        let t = gw.into_transport();
        assert!(!t.motor && !t.selected);
        // The read with the overflow was repeated.
        let reads = t.log.iter().filter(|c| c[0] == CMD_READ_FLUX).count();
        assert_eq!(reads, 1 + 35 + 1);
        assert!(t
            .log
            .iter()
            .any(|c| c == &[CMD_READ_FLUX, 8, 0, 0, 0, 0, 3, 0]));
    }
}
