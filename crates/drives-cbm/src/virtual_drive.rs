//! Virtual drive: a serial bus device that speaks the Commodore IEC
//! protocol at line level and serves files from a D64 image directly, with
//! no drive CPU, DOS ROM or disk mechanics. Programs that only use the
//! KERNAL's serial routines (LOAD, SAVE, OPEN/PRINT#/GET#, the command
//! channel) work and load faster; fast loaders and copy protection need the
//! true drive.
//!
//! Protocol (Commodore 64 Programmer's Reference Guide, serial bus; "Inside
//! the 1541"): the host pulls ATN and sends command bytes (LISTEN `$20+n`,
//! UNLISTEN `$3F`, TALK `$40+n`, UNTALK `$5F`, then OPEN `$F0+ch`, CLOSE
//! `$E0+ch` or DATA `$60+ch`). A byte: the talker releases CLK, the
//! listener releases DATA, the talker pulls CLK and sends eight bits, LSB
//! first, each valid while CLK is released (DATA released = 1); the
//! listener acknowledges by pulling DATA. A talker that waits more than
//! 200 µs before the first bit signals EOI (last byte); the listener
//! answers with a 60 µs DATA pulse. After TALK the host hands the bus over
//! ("turnaround"): it holds DATA, releases CLK, and the drive takes CLK.
//!
//! DOS subset: channel 0 loads (`$` = directory listing), channel 1 saves
//! (`@0:` replaces), channels 2–14 open files (`,S`/`,P`, `,R`/`,W`),
//! channel 15 reads the error message and runs `I`, `S:` (scratch), `N:`
//! (new), `UJ`/`U:` (reset), `V` (validate: no-op). Written files go into
//! the image and through the media backend when they are closed.
//!
//! [`VirtualDrive::tick`] is one microsecond.

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{Savestate, StateError, StateReader, StateWriter};
use media::D64;

use crate::iec::IecLines;

/// Bus timing in microseconds. The KERNAL polls the port with a debounce
/// (`LDA`, `CMP` the same address); a VIC-II bad line can stop its CPU for
/// 43 cycles between the two reads, so it may take about 60 µs to see an
/// edge. Both halves of a bit (CLK pulled, then released with the bit
/// valid) therefore last 90 µs, and DATA changes only together with a CLK
/// edge.
const EOI_TIMEOUT: u32 = 200;
const EOI_ACK: u32 = 60;
const BIT_LOW: u32 = 90;
const BIT_VALID: u32 = 90;
const BYTE_GAP: u32 = 60;
const TURNAROUND: u32 = 80;

/// Where the protocol stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Not addressed: lines released.
    #[default]
    Idle,
    /// ATN just asserted: holding DATA until the host pulls CLK (it may
    /// still be released when ATN falls).
    AtnWaitClk,
    /// Listener: holding DATA until the talker releases CLK.
    RxWaitReady,
    /// Listener: DATA released, waiting for the talker to pull CLK.
    RxWaitClkLow,
    /// Listener: answering EOI with a DATA pulse.
    RxEoiAck,
    /// Listener: waiting for CLK to be released (bit valid).
    RxBitHigh,
    /// Listener: waiting for CLK to be pulled again after a bit.
    RxBitLow,
    /// Talker after turnaround: waiting for the host to release CLK.
    TurnWaitClk,
    /// Talker: pause, then signal ready to send.
    TxNextByte,
    /// Talker: CLK released, waiting for the listener to release DATA.
    TxWaitListener,
    /// Talker: EOI, waiting for the listener's DATA pulse to start.
    TxEoiWaitLow,
    /// Talker: EOI, waiting for the pulse to end.
    TxEoiWaitHigh,
    /// Talker: sending bits (phase in `step`).
    TxBits,
    /// Talker: waiting for the listener to acknowledge the byte.
    TxWaitAck,
}

/// An open channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Channel {
    #[default]
    Closed,
    /// Bytes to send, next position.
    Read { data: Vec<u8>, pos: usize },
    /// A file being written.
    Write {
        name: Vec<u8>,
        file_type: u8,
        replace: bool,
        data: Vec<u8>,
    },
}

struct Disk {
    media: Media,
    image: D64,
    protected: bool,
}

pub struct VirtualDrive {
    device: u8,
    disk: Option<Disk>,
    /// Lines this drive pulls.
    out: IecLines,
    state: State,
    timer: u32,
    /// Bit number and phase while sending or receiving.
    bit: u8,
    phase: u8,
    byte: u8,
    eoi: bool,
    under_atn: bool,
    prev_atn: bool,
    listening: bool,
    talking: bool,
    channel: u8,
    /// The secondary address of the current LISTEN was OPEN: data bytes
    /// are the name.
    opening: bool,
    name: Vec<u8>,
    channels: Vec<Channel>,
    /// Command channel text being received.
    command: Vec<u8>,
    status: String,
}

fn status(code: u8, text: &str, track: u8, sector: u8) -> String {
    format!("{code:02},{text},{track:02},{sector:02}\r")
}

impl VirtualDrive {
    pub fn new(device: u8) -> Self {
        Self {
            device: device.clamp(8, 15),
            disk: None,
            out: IecLines::default(),
            state: State::Idle,
            timer: 0,
            bit: 0,
            phase: 0,
            byte: 0,
            eoi: false,
            under_atn: false,
            prev_atn: false,
            listening: false,
            talking: false,
            channel: 0,
            opening: false,
            name: Vec::new(),
            channels: vec![Channel::Closed; 16],
            command: Vec::new(),
            status: status(73, "POLYTRACE VIRTUAL DRIVE", 0, 0),
        }
    }

    /// RESET: channels close, the status shows the DOS version.
    pub fn reset(&mut self) {
        let disk = self.disk.take();
        *self = Self::new(self.device);
        self.disk = disk;
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// The command channel's current message (for tests and tools).
    pub fn status_text(&self) -> &str {
        &self.status
    }

    /// Lines this drive pulls on the bus.
    pub fn iec_out(&self) -> IecLines {
        self.out
    }

    /// One microsecond with the bus levels as they are now (`true` = low).
    pub fn tick(&mut self, bus: IecLines) {
        if bus.atn && !self.prev_atn {
            // ATN: every device answers by pulling DATA and listens.
            self.under_atn = true;
            self.out = IecLines {
                atn: false,
                clk: false,
                data: true,
                srq: false,
            };
            self.enter(State::AtnWaitClk);
        } else if !bus.atn && self.prev_atn {
            self.atn_released();
        }
        self.prev_atn = bus.atn;
        self.timer = self.timer.saturating_add(1);
        self.step(bus);
    }

    fn enter(&mut self, state: State) {
        self.state = state;
        self.timer = 0;
    }

    fn atn_released(&mut self) {
        self.under_atn = false;
        if self.talking {
            self.enter(State::TurnWaitClk);
        } else if self.listening {
            // Keep DATA pulled: ready when the host is.
            self.enter(State::RxWaitReady);
        } else {
            self.out = IecLines::default();
            self.enter(State::Idle);
        }
    }

    fn step(&mut self, bus: IecLines) {
        match self.state {
            State::Idle => {}
            State::AtnWaitClk => {
                if bus.clk {
                    self.enter(State::RxWaitReady);
                }
            }
            State::RxWaitReady => {
                if !bus.clk {
                    self.out.data = false;
                    self.eoi = false;
                    self.enter(State::RxWaitClkLow);
                }
            }
            State::RxWaitClkLow => {
                if bus.clk {
                    self.bit = 0;
                    self.byte = 0;
                    self.enter(State::RxBitHigh);
                } else if !self.eoi && self.timer >= EOI_TIMEOUT {
                    self.eoi = true;
                    self.out.data = true;
                    self.enter(State::RxEoiAck);
                }
            }
            State::RxEoiAck => {
                if self.timer >= EOI_ACK {
                    self.out.data = false;
                    self.enter(State::RxWaitClkLow);
                }
            }
            State::RxBitHigh => {
                if !bus.clk {
                    if !bus.data {
                        self.byte |= 1 << self.bit;
                    }
                    self.bit += 1;
                    self.enter(State::RxBitLow);
                }
            }
            State::RxBitLow => {
                if bus.clk {
                    if self.bit == 8 {
                        self.out.data = true;
                        let (byte, eoi) = (self.byte, self.eoi);
                        self.enter(State::RxWaitReady);
                        self.received(byte, eoi);
                    } else {
                        self.enter(State::RxBitHigh);
                    }
                }
            }
            State::TurnWaitClk => {
                if !bus.clk {
                    self.out.clk = true;
                    self.out.data = false;
                    self.enter(State::TxNextByte);
                    self.timer = BYTE_GAP.saturating_sub(TURNAROUND);
                }
            }
            State::TxNextByte => {
                if self.timer >= BYTE_GAP {
                    match self.next_byte() {
                        Some((byte, last)) => {
                            self.byte = byte;
                            self.eoi = last;
                            self.out.clk = false;
                            self.enter(State::TxWaitListener);
                        }
                        // Nothing to send (file not found, end): let go of
                        // the bus; the host times out.
                        None => {
                            self.out = IecLines::default();
                            self.talking = false;
                            self.enter(State::Idle);
                        }
                    }
                }
            }
            State::TxWaitListener => {
                if !bus.data {
                    if self.eoi {
                        self.enter(State::TxEoiWaitLow);
                    } else {
                        self.start_bits();
                    }
                }
            }
            State::TxEoiWaitLow => {
                if bus.data {
                    self.enter(State::TxEoiWaitHigh);
                }
            }
            State::TxEoiWaitHigh => {
                if !bus.data {
                    self.start_bits();
                }
            }
            State::TxBits => self.send_bits(),
            State::TxWaitAck => {
                if bus.data {
                    self.advance();
                    if self.eoi {
                        // Last byte sent: release CLK and wait for UNTALK.
                        self.out = IecLines::default();
                        self.enter(State::Idle);
                    } else {
                        self.enter(State::TxNextByte);
                    }
                } else if self.timer > 1000 {
                    // No listener: give up.
                    self.out = IecLines::default();
                    self.enter(State::Idle);
                }
            }
        }
    }

    /// Pull CLK and put bit 0 on DATA (released = 1).
    fn start_bits(&mut self) {
        self.out.clk = true;
        self.out.data = self.byte & 1 == 0;
        self.bit = 0;
        self.phase = 0;
        self.enter(State::TxBits);
    }

    /// Bit phases: 0 CLK pulled with the bit on DATA, then release CLK (bit
    /// valid); 1 after the valid time pull CLK and put the next bit on DATA
    /// at the same moment (after the last one, release DATA).
    fn send_bits(&mut self) {
        match self.phase {
            0 if self.timer >= BIT_LOW => {
                self.out.clk = false;
                self.phase = 1;
                self.timer = 0;
            }
            1 if self.timer >= BIT_VALID => {
                self.out.clk = true;
                self.bit += 1;
                self.phase = 0;
                self.timer = 0;
                if self.bit == 8 {
                    self.out.data = false;
                    self.enter(State::TxWaitAck);
                } else {
                    self.out.data = self.byte & (1 << self.bit) == 0;
                }
            }
            _ => {}
        }
    }

    /// The next byte of the talk channel, and whether it is the last.
    fn next_byte(&mut self) -> Option<(u8, bool)> {
        let ch = usize::from(self.channel);
        if ch == 15 {
            let text = self.status.as_bytes();
            let pos = match &self.channels[15] {
                Channel::Read { pos, .. } => *pos,
                _ => 0,
            };
            return text.get(pos).map(|&b| (b, pos + 1 == text.len()));
        }
        match &self.channels[ch] {
            Channel::Read { data, pos } => data.get(*pos).map(|&b| (b, pos + 1 == data.len())),
            _ => None,
        }
    }

    fn advance(&mut self) {
        let ch = usize::from(self.channel);
        if ch == 15 {
            let pos = match &self.channels[15] {
                Channel::Read { pos, .. } => *pos + 1,
                _ => 1,
            };
            if pos >= self.status.len() {
                // Read to the end: the message resets to OK.
                self.channels[15] = Channel::Closed;
                self.status = status(0, " OK", 0, 0);
            } else {
                self.channels[15] = Channel::Read {
                    data: Vec::new(),
                    pos,
                };
            }
            return;
        }
        if let Channel::Read { pos, .. } = &mut self.channels[ch] {
            *pos += 1;
        }
    }

    /// A byte received as listener.
    fn received(&mut self, byte: u8, _eoi: bool) {
        if self.under_atn {
            self.command_byte(byte);
            return;
        }
        if !self.listening {
            return;
        }
        if self.opening || self.channel == 15 {
            if self.channel == 15 && !self.opening {
                self.command.push(byte);
            } else {
                self.name.push(byte);
            }
            return;
        }
        if let Channel::Write { data, .. } = &mut self.channels[usize::from(self.channel)] {
            data.push(byte);
        }
    }

    fn command_byte(&mut self, byte: u8) {
        let dev = byte & 0x1f;
        match byte & 0xf0 {
            0x20 | 0x30 => {
                if byte == 0x3f {
                    if self.listening {
                        self.unlisten();
                    }
                    self.listening = false;
                } else {
                    self.listening = dev == self.device;
                }
            }
            0x40 | 0x50 => {
                if byte == 0x5f {
                    self.talking = false;
                } else {
                    self.talking = dev == self.device;
                }
            }
            0x60 if self.listening || self.talking => {
                self.channel = byte & 0x0f;
                self.opening = false;
            }
            0xe0 if self.listening => {
                self.channel = byte & 0x0f;
                self.close(self.channel);
            }
            0xf0 if self.listening => {
                self.channel = byte & 0x0f;
                self.opening = true;
                self.name.clear();
            }
            _ => {}
        }
    }

    fn unlisten(&mut self) {
        if self.opening {
            self.opening = false;
            let name = std::mem::take(&mut self.name);
            if self.channel == 15 {
                if !name.is_empty() {
                    self.run_command(&name);
                }
            } else {
                self.open(self.channel, &name);
            }
        } else if self.channel == 15 && !self.command.is_empty() {
            let cmd = std::mem::take(&mut self.command);
            self.run_command(&cmd);
        }
    }

    fn set_status(&mut self, code: u8, text: &str) {
        self.status = status(code, text, 0, 0);
        self.channels[15] = Channel::Closed;
    }

    /// OPEN: parse `[@][0:]name[,type][,mode]`.
    fn open(&mut self, ch: u8, raw: &[u8]) {
        let ch_i = usize::from(ch);
        self.channels[ch_i] = Channel::Closed;
        let raw: Vec<u8> = raw.iter().copied().filter(|&c| c != b'\r').collect();
        let (replace, rest) = match raw.strip_prefix(b"@") {
            Some(r) => (true, r),
            None => (false, &raw[..]),
        };
        let rest = match rest.iter().position(|&c| c == b':') {
            Some(i) => &rest[i + 1..],
            None => rest,
        };
        let mut parts = rest.split(|&c| c == b',');
        let name = parts.next().unwrap_or_default().to_vec();
        let mut file_type = 2;
        let mut write = ch == 1;
        for p in parts {
            match p.first() {
                Some(b'S') => file_type = 1,
                Some(b'P') => file_type = 2,
                Some(b'U') => file_type = 3,
                Some(b'W') => write = true,
                Some(b'R') => write = false,
                _ => {}
            }
        }
        let Some(disk) = &self.disk else {
            self.set_status(74, "DRIVE NOT READY");
            return;
        };
        if write {
            if disk.protected {
                self.set_status(26, "WRITE PROTECT ON");
                return;
            }
            if !replace && disk.image.read_file(&name).is_ok() {
                self.set_status(63, "FILE EXISTS");
                return;
            }
            self.channels[ch_i] = Channel::Write {
                name,
                file_type,
                replace,
                data: Vec::new(),
            };
            self.set_status(0, " OK");
            return;
        }
        let data = if ch == 0 && name.first() == Some(&b'$') {
            Ok(disk.image.listing())
        } else {
            disk.image.read_file(&name).map(|(_, d)| d)
        };
        match data {
            Ok(data) if !data.is_empty() => {
                self.channels[ch_i] = Channel::Read { data, pos: 0 };
                self.set_status(0, " OK");
            }
            _ => self.set_status(62, "FILE NOT FOUND"),
        }
    }

    /// CLOSE: a written file goes into the image and the backend.
    fn close(&mut self, ch: u8) {
        let channel = std::mem::take(&mut self.channels[usize::from(ch)]);
        let Channel::Write {
            name,
            file_type,
            replace,
            data,
        } = channel
        else {
            return;
        };
        let Some(disk) = &mut self.disk else {
            return;
        };
        if replace {
            let _ = disk.image.scratch(&name);
        }
        let result = disk.image.write_file(&name, file_type, &data);
        match result.and_then(|()| Self::write_back(disk)) {
            Ok(()) => self.set_status(0, " OK"),
            Err(_) => self.set_status(72, "DISK FULL"),
        }
    }

    fn write_back(disk: &mut Disk) -> Result<(), MediaError> {
        let bytes = disk.image.to_bytes();
        disk.media.backend.write(0, &bytes)
    }

    fn run_command(&mut self, raw: &[u8]) {
        let cmd: Vec<u8> = raw.iter().copied().filter(|&c| c != b'\r').collect();
        let arg = |cmd: &[u8]| -> Vec<u8> {
            match cmd.iter().position(|&c| c == b':') {
                Some(i) => cmd[i + 1..].to_vec(),
                None => Vec::new(),
            }
        };
        match cmd.first() {
            Some(b'I') | Some(b'V') => self.set_status(0, " OK"),
            Some(b'U') if matches!(cmd.get(1), Some(b'J') | Some(b':')) => self.reset(),
            Some(b'S') => {
                let Some(disk) = &mut self.disk else {
                    return self.set_status(74, "DRIVE NOT READY");
                };
                let mut n = 0;
                for pattern in arg(&cmd).split(|&c| c == b',') {
                    n += disk.image.scratch(pattern).unwrap_or(0);
                }
                let _ = Self::write_back(disk);
                self.status = status(1, " FILES SCRATCHED", n.min(99) as u8, 0);
            }
            Some(b'N') => {
                let Some(disk) = &mut self.disk else {
                    return self.set_status(74, "DRIVE NOT READY");
                };
                let a = arg(&cmd);
                let mut parts = a.split(|&c| c == b',');
                let name = parts.next().unwrap_or_default();
                let id = parts
                    .next()
                    .map(|i| {
                        [
                            i.first().copied().unwrap_or(b' '),
                            i.get(1).copied().unwrap_or(b' '),
                        ]
                    })
                    .unwrap_or_else(|| disk.image.id());
                disk.image = D64::blank(name, id);
                let _ = Self::write_back(disk);
                self.set_status(0, " OK");
            }
            _ => self.set_status(31, "SYNTAX ERROR"),
        }
    }
}

impl MediaSlot for VirtualDrive {
    fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = D64::parse(&bytes)?;
        self.disk = Some(Disk {
            media,
            image,
            protected: false,
        });
        Ok(())
    }

    fn eject(&mut self) -> Option<Media> {
        self.disk.take().map(|d| d.media)
    }

    fn write_protect(&mut self, on: bool) {
        if let Some(d) = &mut self.disk {
            d.protected = on;
        }
    }

    /// Files are written back when they are closed; nothing is pending.
    fn flush(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

const TAG: [u8; 4] = *b"VDRV";
const VERSION: u16 = 1;

fn state_code(s: State) -> u8 {
    s as u8
}

fn state_from(v: u8) -> Option<State> {
    use State::*;
    [
        Idle,
        AtnWaitClk,
        RxWaitReady,
        RxWaitClkLow,
        RxEoiAck,
        RxBitHigh,
        RxBitLow,
        TurnWaitClk,
        TxNextByte,
        TxWaitListener,
        TxEoiWaitLow,
        TxEoiWaitHigh,
        TxBits,
        TxWaitAck,
    ]
    .get(usize::from(v))
    .copied()
}

impl Savestate for VirtualDrive {
    /// The image is media, not state; open channels are saved.
    fn save(&self, w: &mut StateWriter) {
        w.chunk(TAG, VERSION, |w| {
            w.u8(state_code(self.state));
            w.u32(self.timer);
            for v in [self.bit, self.phase, self.byte, self.channel] {
                w.u8(v);
            }
            for b in [
                self.out.clk,
                self.out.data,
                self.eoi,
                self.under_atn,
                self.prev_atn,
                self.listening,
                self.talking,
                self.opening,
            ] {
                w.bool(b);
            }
            w.bytes(&self.name);
            w.bytes(&self.command);
            w.bytes(self.status.as_bytes());
            for ch in &self.channels {
                match ch {
                    Channel::Closed => w.u8(0),
                    Channel::Read { data, pos } => {
                        w.u8(1);
                        w.bytes(data);
                        w.u32(*pos as u32);
                    }
                    Channel::Write {
                        name,
                        file_type,
                        replace,
                        data,
                    } => {
                        w.u8(2);
                        w.bytes(name);
                        w.u8(*file_type);
                        w.bool(*replace);
                        w.bytes(data);
                    }
                }
            }
        });
    }

    fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(TAG, VERSION)?;
        let mut d = VirtualDrive::new(self.device);
        d.state = state_from(c.u8()?).ok_or(StateError::Invalid("virtual drive state"))?;
        d.timer = c.u32()?;
        d.bit = c.u8()?.min(8);
        d.phase = c.u8()? % 2;
        d.byte = c.u8()?;
        d.channel = c.u8()? & 0x0f;
        d.out.clk = c.bool()?;
        d.out.data = c.bool()?;
        d.eoi = c.bool()?;
        d.under_atn = c.bool()?;
        d.prev_atn = c.bool()?;
        d.listening = c.bool()?;
        d.talking = c.bool()?;
        d.opening = c.bool()?;
        d.name = c.bytes()?.to_vec();
        d.command = c.bytes()?.to_vec();
        d.status = String::from_utf8_lossy(c.bytes()?).into_owned();
        for ch in &mut d.channels {
            *ch = match c.u8()? {
                0 => Channel::Closed,
                1 => Channel::Read {
                    data: c.bytes()?.to_vec(),
                    pos: c.u32()? as usize,
                },
                2 => Channel::Write {
                    name: c.bytes()?.to_vec(),
                    file_type: c.u8()? & 7,
                    replace: c.bool()?,
                    data: c.bytes()?.to_vec(),
                },
                _ => return Err(StateError::Invalid("virtual drive channel")),
            };
        }
        d.disk = self.disk.take();
        *self = d;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
