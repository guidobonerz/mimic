//! The DOS of the virtual drives: channels, files and commands served
//! from a CBM disk image, behind a bus protocol (IEC in
//! [`crate::virtual_drive`], IEEE-488 in [`crate::ieee_drive`]).
//!
//! DOS subset: channel 0 loads (`$` = directory listing), channel 1 saves
//! (`@0:` replaces), channels 2–14 open files (`,S`/`,P`, `,R`/`,W`),
//! channel 15 reads the error message and runs `I`, `S:` (scratch), `N:`
//! (new), `UJ`/`U:` (reset), `V` (validate: no-op). Written files go into
//! the image and through the media backend when they are closed. The bus
//! protocol hands over the bytes it receives ([`VirtualDos::received`])
//! and takes the bytes to send ([`VirtualDos::next_byte`]).

use emu_core::media::{Media, MediaError, MediaKind, MediaSlot};
use emu_core::{StateError, StateReader, StateWriter};
use media::{Format, D64};

/// An open channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Channel {
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

pub(crate) struct Disk {
    media: Media,
    image: D64,
    protected: bool,
}

pub struct VirtualDos {
    pub(crate) device: u8,
    pub(crate) disk: Option<Disk>,
    /// The byte being received comes under ATN (a bus command).
    pub(crate) under_atn: bool,
    pub(crate) listening: bool,
    pub(crate) talking: bool,
    pub(crate) channel: u8,
    /// The secondary address of the current LISTEN was OPEN: data bytes
    /// are the name.
    pub(crate) opening: bool,
    pub(crate) name: Vec<u8>,
    pub(crate) channels: Vec<Channel>,
    /// Command channel text being received.
    pub(crate) command: Vec<u8>,
    pub(crate) status: String,
    /// The power-on message (73).
    version: &'static str,
}

pub(crate) fn status(code: u8, text: &str, track: u8, sector: u8) -> String {
    format!("{code:02},{text},{track:02},{sector:02}\r")
}

impl VirtualDos {
    /// A DOS for device number 8–15 that reports `version` at power-on.
    pub fn new(device: u8, version: &'static str) -> Self {
        Self {
            device: device.clamp(8, 15),
            disk: None,
            under_atn: false,
            listening: false,
            talking: false,
            channel: 0,
            opening: false,
            name: Vec::new(),
            channels: vec![Channel::Closed; 16],
            command: Vec::new(),
            status: status(73, version, 0, 0),
            version,
        }
    }

    pub fn device(&self) -> u8 {
        self.device
    }

    /// RESET: channels close, the status shows the DOS version.
    pub fn reset(&mut self) {
        let disk = self.disk.take();
        *self = Self::new(self.device, self.version);
        self.disk = disk;
    }

    pub fn has_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// The command channel's current message (for tests and tools).
    pub fn status_text(&self) -> &str {
        &self.status
    }

    pub fn listening(&self) -> bool {
        self.listening
    }

    pub fn talking(&self) -> bool {
        self.talking
    }

    /// The talker ran out of data (or found no listener).
    pub fn stop_talking(&mut self) {
        self.talking = false;
    }

    /// ATN asserted or released: bytes received meanwhile are commands.
    pub fn set_atn(&mut self, asserted: bool) {
        self.under_atn = asserted;
    }

    /// The next byte of the talk channel, and whether it is the last.
    pub fn next_byte(&mut self) -> Option<(u8, bool)> {
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

    /// The byte from [`VirtualDos::next_byte`] was taken.
    pub fn advance(&mut self) {
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
    /// A byte received as listener (a command under ATN).
    pub fn received(&mut self, byte: u8, _eoi: bool) {
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
                // NEW keeps the disk type: D64, D71 or D81.
                disk.image = match disk.image.format {
                    Format::D64 => D64::blank(name, id),
                    Format::D71 => D64::blank_d71(name, id),
                    Format::D81 => D64::blank_d81(name, id),
                    Format::D80 => D64::blank_8050(name, id, false),
                    Format::D82 => D64::blank_8050(name, id, true),
                };
                let _ = Self::write_back(disk);
                self.set_status(0, " OK");
            }
            _ => self.set_status(31, "SYNTAX ERROR"),
        }
    }

    /// Savestate of the DOS part (the image is media, not state).
    pub fn save_fields(&self, w: &mut StateWriter) {
        w.u8(self.channel);
        for b in [self.under_atn, self.listening, self.talking, self.opening] {
            w.bool(b);
        }
        w.bytes(&self.name);
        w.bytes(&self.command);
        w.bytes(self.status.as_bytes());
        self.save_channels(w);
    }

    pub(crate) fn save_channels(&self, w: &mut StateWriter) {
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
    }

    /// Counterpart of [`VirtualDos::save_fields`] into a fresh DOS.
    pub fn load_fields(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        self.channel = c.u8()? & 0x0f;
        self.under_atn = c.bool()?;
        self.listening = c.bool()?;
        self.talking = c.bool()?;
        self.opening = c.bool()?;
        self.name = c.bytes()?.to_vec();
        self.command = c.bytes()?.to_vec();
        self.status = String::from_utf8_lossy(c.bytes()?).into_owned();
        self.load_channels(c)
    }

    pub(crate) fn load_channels(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        for ch in &mut self.channels {
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
        Ok(())
    }
}

impl MediaSlot for VirtualDos {
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
