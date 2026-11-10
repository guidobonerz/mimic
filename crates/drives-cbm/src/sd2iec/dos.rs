//! The SD2IEC's DOS: channels, files and commands on the host folder (the
//! "SD card", [`FileHost`] from the machine's [`HostServices`]) or on a
//! mounted disk image.
//!
//! [`HostServices`]: emu_core::host::HostServices

use emu_core::host::{lock, FileHost, FsError, OpenMode, SharedFiles};
use emu_core::media::{DriveStatus, Media, MediaError, MediaKind};
use emu_core::{StateError, StateReader, StateWriter};
use media::{Format, D64};

use super::names::{self, PRG, REL, SEQ, USR};
use crate::dos::status;
use crate::virtual_drive::IecDos;

/// What the drive reports at power-on and after `UI`. Programs detect an
/// SD2IEC by "SD2IEC" in this message (sd2iec README, "Introduction").
pub const VERSION: &str = "SD2IEC V1.0.0/MIMIC";

/// A DOS error: code and text.
type DosError = (u8, &'static str);
type DosResult<T> = Result<T, DosError>;

const OK: DosError = (0, " OK");
const WRITE_ERROR: DosError = (25, "WRITE ERROR");
const PROTECTED: DosError = (26, "WRITE PROTECT ON");
const SYNTAX_30: DosError = (30, "SYNTAX ERROR");
const SYNTAX: DosError = (31, "SYNTAX ERROR");
const BAD_NAME: DosError = (33, "SYNTAX ERROR");
const PATH: DosError = (39, "PATH NOT FOUND");
const NOT_FOUND: DosError = (62, "FILE NOT FOUND");
const EXISTS: DosError = (63, "FILE EXISTS");
const TYPE_MISMATCH: DosError = (64, "FILE TYPE MISMATCH");
const NO_CHANNEL: DosError = (70, "NO CHANNEL");
const DISK_FULL: DosError = (72, "DISK FULL");
const NOT_READY: DosError = (74, "DRIVE NOT READY");
const PARTITION: DosError = (77, "SELECTED PARTITION ILLEGAL");

fn fs_error(e: FsError) -> DosError {
    match e {
        FsError::NotFound => NOT_FOUND,
        FsError::Exists | FsError::NotEmpty => EXISTS,
        FsError::Denied => PROTECTED,
        FsError::Unavailable => NOT_READY,
        FsError::InvalidPath | FsError::NotADirectory | FsError::IsADirectory => PATH,
        _ => WRITE_ERROR,
    }
}

/// Where a written file goes when its channel closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// A file in the folder; `prefix` goes in front of the data (an x00
    /// header, or the old content when appending).
    Host { path: String, prefix: Vec<u8> },
    /// A file in the mounted image.
    Image {
        name: Vec<u8>,
        file_type: u8,
        replace: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Channel {
    #[default]
    Closed,
    /// Bytes to send, next position. On channel 15 with data: the answer
    /// to `M-R` instead of the status.
    Read {
        data: Vec<u8>,
        pos: usize,
    },
    Write {
        target: Target,
        data: Vec<u8>,
    },
}

/// Where the mounted image comes from.
pub(crate) enum Source {
    /// A file in the folder (mounted with `CD`).
    Host(String),
    /// A disk inserted into the SD2IEC's media slot.
    Media(Media),
}

pub(crate) struct Mount {
    pub(crate) source: Source,
    /// Read from the folder when first needed (after a savestate load).
    image: Option<D64>,
}

/// A directory entry of the folder as the C64 sees it.
#[derive(Clone, Debug)]
struct Entry {
    /// The host file name in its directory.
    host: String,
    /// The PETSCII name (at most 16 characters).
    name: Vec<u8>,
    dir: bool,
    file_type: u8,
    /// Data bytes (without an x00 header).
    size: u64,
    x00: bool,
    /// The extension hidden by `XE+`, kept on rename.
    hidden_ext: Option<String>,
    /// A name starting with `.`: left out of listings unless asked for.
    hidden: bool,
    mountable: bool,
}

pub struct Sd2iecDos {
    pub(crate) device: u8,
    files: Option<SharedFiles>,
    pub(crate) under_atn: bool,
    pub(crate) listening: bool,
    pub(crate) talking: bool,
    pub(crate) channel: u8,
    pub(crate) opening: bool,
    pub(crate) name: Vec<u8>,
    pub(crate) command: Vec<u8>,
    pub(crate) channels: Vec<Channel>,
    pub(crate) status: String,
    /// The current directory in the folder (canonical path).
    pub(crate) cwd: String,
    pub(crate) mount: Option<Mount>,
    /// `XE0`–`XE4`: how written files are stored (default 1: x00 for
    /// SEQ/USR/REL, PRG without).
    pub(crate) ext_mode: u8,
    /// `XE+`/`XE-`: `.PRG`/`.SEQ`/`.USR`/`.REL` hidden and taken as type.
    pub(crate) hide_ext: bool,
    /// `X*+`/`X*-`: 1581-style `*` matching (default on).
    pub(crate) star_1581: bool,
    /// The card's write protect switch.
    pub(crate) protected: bool,
}

impl Sd2iecDos {
    pub fn new(device: u8) -> Self {
        Self {
            device: device.clamp(4, 30),
            files: None,
            under_atn: false,
            listening: false,
            talking: false,
            channel: 0,
            opening: false,
            name: Vec::new(),
            command: Vec::new(),
            channels: vec![Channel::Closed; 16],
            status: status(73, VERSION, 0, 0),
            cwd: "/".into(),
            mount: None,
            ext_mode: 1,
            hide_ext: false,
            star_1581: true,
            protected: false,
        }
    }

    pub fn device(&self) -> u8 {
        self.device
    }

    /// The folder the SD2IEC works on (`None`: no card).
    pub fn attach_files(&mut self, files: Option<SharedFiles>) {
        self.files = files;
    }

    /// The command channel's current message.
    pub fn status_text(&self) -> &str {
        &self.status
    }

    /// The current directory in the folder.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// A disk image is mounted (by `CD` or in the media slot).
    pub fn mounted(&self) -> bool {
        self.mount.is_some()
    }

    /// A disk is inserted in the media slot.
    pub fn has_media(&self) -> bool {
        matches!(
            self.mount,
            Some(Mount {
                source: Source::Media(_),
                ..
            })
        )
    }

    fn status_code(&self) -> Option<u8> {
        match self.status.as_bytes() {
            [a @ b'0'..=b'9', b @ b'0'..=b'9', ..] => Some((a - b'0') * 10 + (b - b'0')),
            _ => None,
        }
    }

    /// RESET from the bus or `UJ`: channels close, the status shows the
    /// version; directory, mounted image and settings stay.
    pub fn reset(&mut self) {
        self.under_atn = false;
        self.listening = false;
        self.talking = false;
        self.channel = 0;
        self.opening = false;
        self.name.clear();
        self.command.clear();
        self.channels = vec![Channel::Closed; 16];
        self.status = status(73, VERSION, 0, 0);
    }

    /// Power cycle: also back to the root, settings to their defaults; an
    /// image mounted with `CD` is left, a disk in the media slot stays.
    pub fn power_on(&mut self) {
        let mut fresh = Self::new(self.device);
        fresh.files = self.files.take();
        fresh.protected = self.protected;
        if self.has_media() {
            fresh.mount = self.mount.take();
        }
        *self = fresh;
    }

    fn set(&mut self, e: DosError) {
        self.set_status(e.0, e.1, 0, 0);
    }

    fn set_status(&mut self, code: u8, text: &str, track: u8, sector: u8) {
        self.status = status(code, text, track, sector);
        self.channels[15] = Channel::Closed;
    }

    fn files(&self) -> DosResult<SharedFiles> {
        self.files.clone().ok_or(NOT_READY)
    }

    // ---- bus addressing ------------------------------------------------

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

    // ---- folder access -------------------------------------------------

    pub(crate) fn read_all(fs: &mut dyn FileHost, path: &str) -> Result<Vec<u8>, FsError> {
        let h = fs.open(
            path,
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )?;
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let result = loop {
            match fs.read(h, &mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) => break Err(e),
            }
        };
        fs.close(h);
        result.map(|()| out)
    }

    fn read_head(fs: &mut dyn FileHost, path: &str, n: usize) -> Result<Vec<u8>, FsError> {
        let h = fs.open(
            path,
            OpenMode {
                read: true,
                ..OpenMode::default()
            },
        )?;
        let mut out = vec![0; n];
        let mut got = 0;
        while got < n {
            match fs.read(h, &mut out[got..]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) => {
                    fs.close(h);
                    return Err(e);
                }
            }
        }
        fs.close(h);
        out.truncate(got);
        Ok(out)
    }

    pub(crate) fn write_all(fs: &mut dyn FileHost, path: &str, data: &[u8]) -> Result<(), FsError> {
        let h = fs.open(
            path,
            OpenMode {
                write: true,
                create: true,
                truncate: true,
                ..OpenMode::default()
            },
        )?;
        let mut done = 0;
        let result = loop {
            if done == data.len() {
                break Ok(());
            }
            match fs.write(h, &data[done..]) {
                Ok(0) => break Err(FsError::Io("short write".into())),
                Ok(n) => done += n,
                Err(e) => break Err(e),
            }
        };
        fs.close(h);
        result
    }

    fn join(dir: &str, name: &str) -> String {
        if dir == "/" {
            format!("/{name}")
        } else {
            format!("{dir}/{name}")
        }
    }

    fn parent(dir: &str) -> String {
        match dir.rfind('/') {
            Some(0) | None => "/".into(),
            Some(i) => dir[..i].into(),
        }
    }

    /// The directory's entries as the C64 sees them, in host name order.
    fn scan(&self, fs: &mut dyn FileHost, dir: &str) -> DosResult<Vec<Entry>> {
        let list = fs.list_dir(dir).map_err(fs_error)?;
        let mut out = Vec::with_capacity(list.len());
        for e in list {
            let hidden = e.name.starts_with('.');
            let (stem, ext) = names::split_ext(&e.name);
            let mut entry = Entry {
                name: names::petscii_from_host(&e.name),
                dir: e.stat.is_dir,
                file_type: PRG,
                size: e.stat.size,
                x00: false,
                hidden_ext: None,
                hidden,
                mountable: false,
                host: e.name.clone(),
            };
            if !entry.dir {
                let x00 = ext
                    .and_then(names::x00_type)
                    .filter(|_| e.stat.size >= names::X00_HEADER as u64);
                let header = match x00 {
                    Some(_) => Self::read_head(fs, &Self::join(dir, &e.name), names::X00_HEADER)
                        .ok()
                        .and_then(|h| names::parse_x00(&h)),
                    None => None,
                };
                if let (Some(t), Some(name)) = (x00, header) {
                    entry.name = name;
                    entry.file_type = t;
                    entry.size -= names::X00_HEADER as u64;
                    entry.x00 = true;
                } else if let Some(t) = ext.and_then(names::type_ext).filter(|_| self.hide_ext) {
                    entry.name = names::petscii_from_host(stem);
                    entry.file_type = t;
                    entry.hidden_ext = ext.map(str::to_string);
                }
                entry.mountable = ext.is_some_and(names::is_image_ext);
            }
            out.push(entry);
        }
        // Long names: an 8.3-style alias, numbered per stem and extension.
        let mut used: Vec<(String, String, usize)> = Vec::new();
        for e in out.iter_mut().filter(|e| e.name.len() > 16) {
            let (base, ext) = names::alias_parts(&e.host);
            let n = match used.iter_mut().find(|(b, x, _)| *b == base && *x == ext) {
                Some((_, _, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    used.push((base.clone(), ext.clone(), 1));
                    1
                }
            };
            e.name = names::alias(&base, &ext, n);
        }
        Ok(out)
    }

    /// The folder directory a path part names: `//` starts at the root, `/`
    /// at the current directory; components are matched like file names,
    /// `_` (the left arrow) goes up.
    fn resolve(&self, fs: &mut dyn FileHost, path: &[u8]) -> DosResult<String> {
        let (mut dir, rest) = if let Some(r) = path.strip_prefix(b"//") {
            ("/".to_string(), r)
        } else if let Some(r) = path.strip_prefix(b"/") {
            (self.cwd.clone(), r)
        } else if path.is_empty() {
            (self.cwd.clone(), path)
        } else {
            return Err(SYNTAX);
        };
        for comp in rest.split(|&c| c == b'/').filter(|c| !c.is_empty()) {
            if comp == [0x5f] {
                dir = Self::parent(&dir);
                continue;
            }
            let entries = self.scan(fs, &dir)?;
            let found = entries
                .iter()
                .find(|e| e.dir && names::matches(comp, &e.name, self.star_1581))
                .ok_or(PATH)?;
            dir = Self::join(&dir, &found.host);
        }
        Ok(dir)
    }

    fn find_file(&self, fs: &mut dyn FileHost, dir: &str, pattern: &[u8]) -> DosResult<Entry> {
        self.scan(fs, dir)?
            .into_iter()
            .find(|e| !e.dir && names::matches(pattern, &e.name, self.star_1581))
            .ok_or(NOT_FOUND)
    }

    /// The bytes of a file in the folder (without an x00 header).
    fn file_data(fs: &mut dyn FileHost, dir: &str, e: &Entry) -> DosResult<Vec<u8>> {
        let mut data = Self::read_all(fs, &Self::join(dir, &e.host)).map_err(fs_error)?;
        if e.x00 {
            data.drain(..names::X00_HEADER.min(data.len()));
        }
        Ok(data)
    }

    /// The host file a new file `name` of `file_type` is written to in
    /// `dir`, and the bytes in front of its data (per `XE`, sd2iec README
    /// "X: Extended commands").
    fn new_file(
        &self,
        fs: &mut dyn FileHost,
        dir: &str,
        name: &[u8],
        file_type: u8,
    ) -> DosResult<(String, Vec<u8>)> {
        let host = names::host_from_petscii(name);
        let raw = file_type == PRG && names::split_ext(&host).1.is_some_and(names::is_raw_ext);
        let x00 = !raw
            && match self.ext_mode {
                1 => file_type != PRG,
                2 => true,
                _ => false,
            };
        if x00 {
            let letter = names::x00_letter(file_type);
            for n in 0..100 {
                let path = Self::join(dir, &format!("{host}.{letter}{n:02}"));
                if fs.stat(&path).is_err() {
                    return Ok((path, names::x00_header(name, 0)));
                }
            }
            return Err(EXISTS);
        }
        let with_ext = !raw && (self.ext_mode == 4 || (self.ext_mode == 3 && file_type != PRG));
        let host = if with_ext {
            format!("{host}.{}", names::ext_for(file_type))
        } else {
            host
        };
        let path = Self::join(dir, &host);
        match fs.stat(&path) {
            Ok(_) => Err(EXISTS),
            Err(_) => Ok((path, Vec::new())),
        }
    }

    // ---- the mounted image ---------------------------------------------

    fn image(&mut self) -> DosResult<&mut D64> {
        let files = self.files.clone();
        let Some(m) = &mut self.mount else {
            return Err(NOT_READY);
        };
        if m.image.is_none() {
            let bytes = match &mut m.source {
                Source::Host(path) => {
                    let files = files.ok_or(NOT_READY)?;
                    let mut fs = lock(&files);
                    Self::read_all(&mut *fs, path).map_err(fs_error)?
                }
                Source::Media(media) => {
                    let mut b = vec![0; media.backend.len() as usize];
                    media.backend.read(0, &mut b).map_err(|_| NOT_READY)?;
                    b
                }
            };
            m.image = Some(D64::parse(&bytes).map_err(|_| NOT_READY)?);
        }
        m.image.as_mut().ok_or(NOT_READY)
    }

    /// The image changed: write it back where it came from.
    fn write_back(&mut self) -> DosResult<()> {
        let files = self.files.clone();
        let Some(Mount {
            source,
            image: Some(image),
        }) = &mut self.mount
        else {
            return Ok(());
        };
        let bytes = image.to_bytes();
        match source {
            Source::Host(path) => {
                let files = files.ok_or(NOT_READY)?;
                let mut fs = lock(&files);
                Self::write_all(&mut *fs, path, &bytes).map_err(fs_error)
            }
            Source::Media(media) => media.backend.write(0, &bytes).map_err(|_| WRITE_ERROR),
        }
    }

    // ---- OPEN / CLOSE --------------------------------------------------

    fn open(&mut self, ch: u8, raw: &[u8]) {
        let raw: Vec<u8> = raw.iter().copied().filter(|&c| c != b'\r').collect();
        self.channels[usize::from(ch)] = Channel::Closed;
        let result = if ch == 0 && raw.first() == Some(&b'$') {
            self.directory(&raw[1..])
                .map(|data| Channel::Read { data, pos: 0 })
        } else if raw.first() == Some(&b'#') {
            // Direct access buffers are not emulated.
            Err(NO_CHANNEL)
        } else {
            self.open_file(ch, &raw)
        };
        match result {
            Ok(c) => {
                self.channels[usize::from(ch)] = c;
                self.set(OK);
            }
            Err(e) => self.set(e),
        }
    }

    fn open_file(&mut self, ch: u8, raw: &[u8]) -> DosResult<Channel> {
        let (replace, rest) = match raw.strip_prefix(b"@") {
            Some(r) => (true, r),
            None => (false, raw),
        };
        let (path, spec) = split_spec(rest);
        let mut parts = spec.split(|&c| c == b',');
        let name = parts.next().unwrap_or_default().to_vec();
        let mut file_type = None;
        let (mut write, mut append) = (ch == 1, false);
        for p in parts {
            match p.first() {
                Some(b'S') => file_type = Some(SEQ),
                Some(b'P') => file_type = Some(PRG),
                Some(b'U') => file_type = Some(USR),
                Some(b'L') => file_type = Some(REL),
                Some(b'W') if ch != 0 => write = true,
                Some(b'A') if ch > 1 => (write, append) = (true, true),
                Some(b'R') | Some(b'M') if ch > 1 => write = false,
                _ => {}
            }
        }
        if name.is_empty() {
            return Err(BAD_NAME);
        }
        let new_type = file_type.unwrap_or(if ch == 1 { PRG } else { SEQ });
        if write && self.protected {
            return Err(PROTECTED);
        }
        if write && !append && name.iter().any(|&c| c == b'*' || c == b'?') {
            return Err(BAD_NAME);
        }
        if self.mount.is_some() {
            if !path.is_empty() {
                return Err(PATH);
            }
            let star = self.star_1581;
            let image = self.image()?;
            let found = image
                .directory()
                .unwrap_or_default()
                .into_iter()
                .find(|e| e.file_type & 7 != 0 && names::matches(&name, &e.name, star));
            if !write {
                let e = found.ok_or(NOT_FOUND)?;
                if file_type.is_some_and(|t| t != e.file_type & 7) {
                    return Err(TYPE_MISMATCH);
                }
                let (_, data) = image.read_file(&e.name).map_err(|_| NOT_FOUND)?;
                return Ok(Channel::Read { data, pos: 0 });
            }
            if append {
                // Appending inside an image: rewrite the file at close.
                let e = found.ok_or(NOT_FOUND)?;
                let (_, data) = image.read_file(&e.name).map_err(|_| NOT_FOUND)?;
                return Ok(Channel::Write {
                    target: Target::Image {
                        name: e.name,
                        file_type: e.file_type & 7,
                        replace: true,
                    },
                    data,
                });
            }
            if found.is_some() && !replace {
                return Err(EXISTS);
            }
            return Ok(Channel::Write {
                target: Target::Image {
                    name,
                    file_type: new_type,
                    replace,
                },
                data: Vec::new(),
            });
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        if !write {
            let e = self.find_file(&mut *fs, &dir, &name)?;
            if file_type.is_some_and(|t| t != e.file_type) {
                return Err(TYPE_MISMATCH);
            }
            let data = Self::file_data(&mut *fs, &dir, &e)?;
            return Ok(Channel::Read { data, pos: 0 });
        }
        let entries = self.scan(&mut *fs, &dir)?;
        let star = self.star_1581;
        if append {
            let e = entries
                .iter()
                .find(|e| !e.dir && names::matches(&name, &e.name, star))
                .ok_or(NOT_FOUND)?;
            let path = Self::join(&dir, &e.host);
            let prefix = Self::read_all(&mut *fs, &path).map_err(fs_error)?;
            return Ok(Channel::Write {
                target: Target::Host { path, prefix },
                data: Vec::new(),
            });
        }
        if let Some(e) = entries.iter().find(|e| e.name == name) {
            // `@` replaces by deleting first (sd2iec README, "Other
            // important notes"); a directory is never replaced.
            if !replace || e.dir {
                return Err(EXISTS);
            }
            fs.delete(&Self::join(&dir, &e.host)).map_err(fs_error)?;
        }
        let (path, prefix) = self.new_file(&mut *fs, &dir, &name, new_type)?;
        Ok(Channel::Write {
            target: Target::Host { path, prefix },
            data: Vec::new(),
        })
    }

    /// CLOSE: a written file goes to the folder or into the image.
    fn close(&mut self, ch: u8) {
        let channel = std::mem::take(&mut self.channels[usize::from(ch)]);
        let Channel::Write { target, data } = channel else {
            return;
        };
        let result = match target {
            Target::Host { path, mut prefix } => self.files().and_then(|files| {
                prefix.extend_from_slice(&data);
                let mut fs = lock(&files);
                Self::write_all(&mut *fs, &path, &prefix).map_err(fs_error)
            }),
            Target::Image {
                name,
                file_type,
                replace,
            } => self.image().and_then(|image| {
                if replace {
                    let _ = image.scratch(&name);
                }
                image
                    .write_file(&name, file_type, &data)
                    .map_err(|_| DISK_FULL)
            }),
        };
        let result = result.and_then(|()| match self.mount {
            Some(_) if ch != 15 => self.write_back(),
            _ => Ok(()),
        });
        match result {
            Ok(()) => self.set(OK),
            Err(e) => self.set(e),
        }
    }

    // ---- directory -----------------------------------------------------

    /// `LOAD"$…"`: `$=P` partitions, `$[path][:pattern,…][=type]` with type
    /// P, S, U, R, D or B (directories) or H (include hidden files).
    fn directory(&mut self, spec: &[u8]) -> DosResult<Vec<u8>> {
        if spec.starts_with(b"=P") {
            return Ok(partition_listing());
        }
        let (spec, filter) = match spec.iter().rposition(|&c| c == b'=') {
            Some(i) => (&spec[..i], spec.get(i + 1).copied()),
            None => (spec, None),
        };
        if self.mount.is_some() {
            return Ok(self.image()?.listing());
        }
        let (path, patterns) = split_spec(spec);
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        let entries = self.scan(&mut *fs, &dir)?;
        let star = self.star_1581;
        let include_hidden = filter == Some(b'H');
        let shown = entries.iter().filter(|e| {
            let pattern_ok = patterns.is_empty()
                || patterns
                    .split(|&c| c == b',')
                    .any(|p| names::matches(p, &e.name, star));
            let type_ok = match filter {
                Some(b'D') | Some(b'B') => e.dir,
                Some(b'P') => !e.dir && e.file_type == PRG,
                Some(b'S') => !e.dir && e.file_type == SEQ,
                Some(b'U') => !e.dir && e.file_type == USR,
                Some(b'R') | Some(b'L') => !e.dir && e.file_type == REL,
                _ => true,
            };
            pattern_ok && type_ok && (include_hidden || !e.hidden)
        });
        let title = match dir.rsplit('/').next() {
            Some(n) if !n.is_empty() => names::petscii_from_host(n),
            _ => b"SD2IEC".to_vec(),
        };
        Ok(listing(&title, shown))
    }

    // ---- commands ------------------------------------------------------

    fn run_command(&mut self, raw: &[u8]) {
        let mut cmd: Vec<u8> = raw.to_vec();
        while cmd.last() == Some(&b'\r') {
            cmd.pop();
        }
        match self.command_result(&cmd) {
            Ok(()) => {}
            Err(e) => self.set(e),
        }
    }

    /// Runs a command; sets the status itself where it is not a plain
    /// error.
    fn command_result(&mut self, cmd: &[u8]) -> DosResult<()> {
        match cmd {
            [b'C', b'D', rest @ ..] => self.cd(rest).map(|()| self.set(OK)),
            [b'C', b'P', rest @ ..] => self.partition(parse_number(rest)),
            [b'C', 0xd0, p, ..] => self.partition(Some(u32::from(*p))),
            [b'C', rest @ ..] => self.copy(rest).map(|()| self.set(OK)),
            [b'M', b'D', rest @ ..] => self.md(rest).map(|()| self.set(OK)),
            [b'R', b'D', rest @ ..] => self.rd(rest).map(|()| self.set(OK)),
            [b'M', b'-', b'R', rest @ ..] => {
                // No drive memory: zeros, as many as asked for (sd2iec
                // answers unknown addresses with arbitrary data).
                let n = rest.get(2).copied().unwrap_or(1).max(1);
                self.channels[15] = Channel::Read {
                    data: vec![0; usize::from(n)],
                    pos: 0,
                };
                Ok(())
            }
            [b'M', b'-', b'W', lo, hi, n, data @ ..] => {
                // A 1541 takes its device number from $77 ($20 + n).
                if u16::from_le_bytes([*lo, *hi]) == 0x77 && *n >= 1 {
                    if let Some(&d) = data.first() {
                        self.change_device(d.wrapping_sub(0x20));
                    }
                }
                Ok(())
            }
            [b'M', b'-', b'E', ..] => Ok(()),
            [b'S', rest @ ..] => self.scratch(rest),
            [b'R', rest @ ..] => self.rename(rest).map(|()| self.set(OK)),
            [b'N', rest @ ..] => self.new_disk(rest).map(|()| self.set(OK)),
            [b'I', ..] | [b'V', ..] => {
                self.set(OK);
                Ok(())
            }
            [b'U', b'0', b'>', d, ..] => {
                self.change_device(*d);
                self.set(OK);
                Ok(())
            }
            [b'U', b'I', b'+' | b'-', ..] => {
                self.set(OK);
                Ok(())
            }
            [b'U', b'I'] | [b'U', b'9'] => {
                self.set_status(73, VERSION, 0, 0);
                Ok(())
            }
            [b'U', b'J', ..] | [b'U', b':', ..] => {
                self.reset();
                Ok(())
            }
            [b'U', 0xca, ..] => {
                self.power_on();
                Ok(())
            }
            [b'X', rest @ ..] => self.extended(rest),
            [b'T', b'-', b'R' | b'W', ..] => Err(SYNTAX_30),
            _ => Err(SYNTAX),
        }
    }

    fn change_device(&mut self, d: u8) {
        if (4..=30).contains(&d) {
            self.device = d;
        }
    }

    fn partition(&mut self, n: Option<u32>) -> DosResult<()> {
        match n {
            Some(0 | 1) => {
                self.set_status(2, "PARTITION SELECTED", 1, 0);
                Ok(())
            }
            _ => Err(PARTITION),
        }
    }

    /// `CD:name`, `CD/path/:name`, `CD//`, `CD:_`, `CD_`: change directory,
    /// or mount a disk image by changing into it (sd2iec README, "CD/MD/RD").
    fn cd(&mut self, arg: &[u8]) -> DosResult<()> {
        let (path, name) = split_cd(arg);
        let media = self.has_media();
        if path.is_empty() && name == [0x5f] {
            // A disk in the media slot stays until it is ejected.
            if !media && self.mount.take().is_none() {
                self.cwd = Self::parent(&self.cwd);
            }
            return Ok(());
        }
        if self.mount.is_some() {
            // Inside an image only leaving it is possible; an absolute path
            // leaves it first.
            if media || !path.starts_with(b"//") {
                return Err(PATH);
            }
            self.mount = None;
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        if name.is_empty() {
            self.cwd = dir;
            return Ok(());
        }
        if name == [0x5f] {
            self.cwd = Self::parent(&dir);
            return Ok(());
        }
        let star = self.star_1581;
        let entries = self.scan(&mut *fs, &dir)?;
        let e = entries
            .iter()
            .find(|e| (e.dir || e.mountable) && names::matches(name, &e.name, star))
            .ok_or(PATH)?;
        let target = Self::join(&dir, &e.host);
        if e.dir {
            self.cwd = target;
            return Ok(());
        }
        let bytes = Self::read_all(&mut *fs, &target).map_err(fs_error)?;
        let image = D64::parse(&bytes).map_err(|_| TYPE_MISMATCH)?;
        if image.format != Format::D64 && image.format != Format::D71 && image.format != Format::D81
        {
            return Err(TYPE_MISMATCH);
        }
        self.cwd = dir;
        self.mount = Some(Mount {
            source: Source::Host(target),
            image: Some(image),
        });
        Ok(())
    }

    /// `MD[path]:name`.
    fn md(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.mount.is_some() {
            return Err(SYNTAX);
        }
        if self.protected {
            return Err(PROTECTED);
        }
        let (path, name) = split_cd(arg);
        if name.is_empty() || name.iter().any(|&c| c == b'*' || c == b'?') {
            return Err(BAD_NAME);
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        if self.scan(&mut *fs, &dir)?.iter().any(|e| e.name == name) {
            return Err(EXISTS);
        }
        fs.mkdir(&Self::join(&dir, &names::host_from_petscii(name)))
            .map_err(fs_error)
    }

    /// `RD:name`: an empty subdirectory of the current directory.
    fn rd(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.mount.is_some() {
            return Err(SYNTAX);
        }
        if self.protected {
            return Err(PROTECTED);
        }
        let (_, name) = split_spec(arg);
        let files = self.files()?;
        let mut fs = lock(&files);
        let star = self.star_1581;
        let e = self
            .scan(&mut *fs, &self.cwd.clone())?
            .into_iter()
            .find(|e| e.dir && names::matches(name, &e.name, star))
            .ok_or(PATH)?;
        fs.delete(&Self::join(&self.cwd, &e.host)).map_err(fs_error)
    }

    /// `S[path]:pattern[,pattern…]`: directories are left alone.
    fn scratch(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.protected {
            return Err(PROTECTED);
        }
        let (path, patterns) = split_spec(arg);
        let mut count = 0usize;
        if self.mount.is_some() {
            if !path.is_empty() {
                return Err(PATH);
            }
            let image = self.image()?;
            for p in patterns.split(|&c| c == b',') {
                count += image.scratch(p).unwrap_or(0);
            }
            self.write_back()?;
        } else {
            let files = self.files()?;
            let mut fs = lock(&files);
            let dir = self.resolve(&mut *fs, path)?;
            let star = self.star_1581;
            for e in self.scan(&mut *fs, &dir)? {
                let hit = !e.dir
                    && patterns
                        .split(|&c| c == b',')
                        .any(|p| names::matches(p, &e.name, star));
                if hit && fs.delete(&Self::join(&dir, &e.host)).is_ok() {
                    count += 1;
                }
            }
        }
        self.set_status(1, " FILES SCRATCHED", count.min(99) as u8, 0);
        Ok(())
    }

    /// `R[path]:new=old`. In an x00 file only the internal name changes.
    fn rename(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.protected {
            return Err(PROTECTED);
        }
        let (path, spec) = split_spec(arg);
        let eq = spec.iter().position(|&c| c == b'=').ok_or(SYNTAX)?;
        let (new, old) = (&spec[..eq], &spec[eq + 1..]);
        if new.is_empty() || new.iter().any(|&c| c == b'*' || c == b'?') {
            return Err(BAD_NAME);
        }
        let star = self.star_1581;
        if self.mount.is_some() {
            if !path.is_empty() {
                return Err(PATH);
            }
            let image = self.image()?;
            let dir = image.directory().unwrap_or_default();
            if dir.iter().any(|e| e.file_type & 7 != 0 && e.name == new) {
                return Err(EXISTS);
            }
            let e = dir
                .into_iter()
                .find(|e| e.file_type & 7 != 0 && names::matches(old, &e.name, star))
                .ok_or(NOT_FOUND)?;
            image.rename(&e.name, new).map_err(|_| WRITE_ERROR)?;
            return self.write_back();
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        let entries = self.scan(&mut *fs, &dir)?;
        if entries.iter().any(|e| e.name == new) {
            return Err(EXISTS);
        }
        let e = entries
            .iter()
            .find(|e| names::matches(old, &e.name, star))
            .ok_or(NOT_FOUND)?;
        let from = Self::join(&dir, &e.host);
        if e.x00 {
            let mut data = Self::read_all(&mut *fs, &from).map_err(fs_error)?;
            let header = names::x00_header(new, data.get(25).copied().unwrap_or(0));
            data[..names::X00_HEADER].copy_from_slice(&header);
            return Self::write_all(&mut *fs, &from, &data).map_err(fs_error);
        }
        let mut host = names::host_from_petscii(new);
        if let Some(ext) = &e.hidden_ext {
            host = format!("{host}.{ext}");
        }
        fs.rename(&from, &Self::join(&dir, &host)).map_err(fs_error)
    }

    /// `N:name,id`: formats the mounted image, or creates an image file
    /// (`.D64`, `.D41`, `.D71`, `.D81`; `.D64` when none is given).
    fn new_disk(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.protected {
            return Err(PROTECTED);
        }
        let (path, spec) = split_spec(arg);
        let mut parts = spec.split(|&c| c == b',');
        let name = parts.next().unwrap_or_default().to_vec();
        let id = parts.next().map(|i| {
            [
                i.first().copied().unwrap_or(b' '),
                i.get(1).copied().unwrap_or(b' '),
            ]
        });
        if self.mount.is_some() {
            let image = self.image()?;
            let id = id.unwrap_or_else(|| image.id());
            *image = match image.format {
                Format::D71 => D64::blank_d71(&name, id),
                Format::D81 => D64::blank_d81(&name, id),
                _ => D64::blank(&name, id),
            };
            return self.write_back();
        }
        let id = id.ok_or(SYNTAX)?;
        let host = names::host_from_petscii(&name);
        let (stem, ext) = names::split_ext(&host);
        let (file, label, ext, defaulted) = match ext {
            Some(e) if names::is_image_ext(e) => (host.clone(), stem.to_string(), e, false),
            _ => (format!("{host}.d64"), host.clone(), "d64", true),
        };
        let label = names::petscii_from_host(&label);
        let image = match ext.to_ascii_lowercase().as_str() {
            "d71" => D64::blank_d71(&label, id),
            "d81" => D64::blank_d81(&label, id),
            _ => D64::blank(&label, id),
        };
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        let target = Self::join(&dir, &file);
        if defaulted && fs.stat(&target).is_ok() {
            return Err(EXISTS);
        }
        Self::write_all(&mut *fs, &target, &image.to_bytes()).map_err(fs_error)
    }

    /// `C[path]:target=source[,source…]`: the sources appended into a new
    /// file, in the folder or within the mounted image.
    fn copy(&mut self, arg: &[u8]) -> DosResult<()> {
        if self.protected {
            return Err(PROTECTED);
        }
        let (path, spec) = split_spec(arg);
        let eq = spec.iter().position(|&c| c == b'=').ok_or(SYNTAX)?;
        let (target, sources) = (&spec[..eq], &spec[eq + 1..]);
        if target.is_empty() || target.iter().any(|&c| c == b'*' || c == b'?') {
            return Err(BAD_NAME);
        }
        if self.mount.is_some() {
            let image = self.image()?;
            let mut data = Vec::new();
            let mut file_type = None;
            for s in sources.split(|&c| c == b',') {
                let (_, s) = split_spec(s);
                let (e, d) = image.read_file(s).map_err(|_| NOT_FOUND)?;
                file_type.get_or_insert(e.file_type & 7);
                data.extend_from_slice(&d);
            }
            if image.read_file(target).is_ok() {
                return Err(EXISTS);
            }
            image
                .write_file(target, file_type.unwrap_or(PRG), &data)
                .map_err(|_| DISK_FULL)?;
            return self.write_back();
        }
        let files = self.files()?;
        let mut fs = lock(&files);
        let dir = self.resolve(&mut *fs, path)?;
        let mut data = Vec::new();
        let mut file_type = None;
        for s in sources.split(|&c| c == b',') {
            let (spath, sname) = split_spec(s);
            let sdir = self.resolve(&mut *fs, spath)?;
            let e = self.find_file(&mut *fs, &sdir, sname)?;
            file_type.get_or_insert(e.file_type);
            data.extend(Self::file_data(&mut *fs, &sdir, &e)?);
        }
        if self.scan(&mut *fs, &dir)?.iter().any(|e| e.name == target) {
            return Err(EXISTS);
        }
        let (out, mut bytes) = self.new_file(&mut *fs, &dir, target, file_type.unwrap_or(PRG))?;
        bytes.extend_from_slice(&data);
        Self::write_all(&mut *fs, &out, &bytes).map_err(fs_error)
    }

    /// `X…`: the extended commands (sd2iec README, "X: Extended commands").
    fn extended(&mut self, arg: &[u8]) -> DosResult<()> {
        match arg {
            [] => {
                let text = format!(
                    "J-:E0{}{}:*{}",
                    self.ext_mode,
                    if self.hide_ext { '+' } else { '-' },
                    if self.star_1581 { '+' } else { '-' },
                );
                self.set_status(3, &text, self.device, 0);
            }
            [b'E', b'+'] => {
                self.hide_ext = true;
                self.set(OK);
            }
            [b'E', b'-'] => {
                self.hide_ext = false;
                self.set(OK);
            }
            [b'E', d @ b'0'..=b'4'] => {
                self.ext_mode = d - b'0';
                if self.ext_mode >= 3 {
                    self.hide_ext = true;
                }
                self.set(OK);
            }
            [b'*', b'+'] => {
                self.star_1581 = true;
                self.set(OK);
            }
            [b'*', b'-'] => {
                self.star_1581 = false;
                self.set(OK);
            }
            [b'?'] => self.set_status(3, VERSION, 0, 0),
            // The settings are not kept beyond the session (no EEPROM).
            [b'W'] => self.set(OK),
            _ => return Err(SYNTAX),
        }
        Ok(())
    }

    // ---- media slot ----------------------------------------------------

    /// A disk image inserted in the media slot is mounted.
    pub fn insert(&mut self, mut media: Media) -> Result<(), MediaError> {
        if media.kind != MediaKind::Disk {
            return Err(MediaError::Unsupported(media.kind));
        }
        let mut bytes = vec![0; media.backend.len() as usize];
        media.backend.read(0, &mut bytes)?;
        let image = D64::parse(&bytes)?;
        self.mount = Some(Mount {
            source: Source::Media(media),
            image: Some(image),
        });
        Ok(())
    }

    pub fn eject(&mut self) -> Option<Media> {
        if !self.has_media() {
            return None;
        }
        match self.mount.take()?.source {
            Source::Media(m) => Some(m),
            Source::Host(_) => None,
        }
    }

    pub fn flush(&mut self) -> Result<(), MediaError> {
        match &mut self.mount {
            Some(Mount {
                source: Source::Media(m),
                ..
            }) => m.backend.commit(),
            _ => Ok(()),
        }
    }

    pub fn dirty(&self) -> bool {
        match &self.mount {
            Some(Mount {
                source: Source::Media(m),
                ..
            }) => m.backend.dirty(),
            _ => false,
        }
    }

    /// Active while addressed or with a file open (the busy LED); error:
    /// a status of 20 or above except the 73 version message.
    pub fn drive_status(&self) -> DriveStatus {
        let open = self.channels[..15]
            .iter()
            .any(|c| !matches!(c, Channel::Closed));
        DriveStatus {
            active: self.listening || self.talking || open,
            error: self.status_code().is_some_and(|c| c >= 20 && c != 73),
            track: None,
        }
    }

    // ---- savestate -----------------------------------------------------

    /// The folder and a disk in the media slot are not state; the current
    /// directory, an image mounted from the folder (by path), the channels
    /// and the settings are.
    pub fn save(&self, w: &mut StateWriter) {
        w.u8(self.device);
        for b in [self.under_atn, self.listening, self.talking, self.opening] {
            w.bool(b);
        }
        w.u8(self.channel);
        w.bytes(&self.name);
        w.bytes(&self.command);
        w.str(&self.status);
        w.str(&self.cwd);
        match &self.mount {
            None => w.u8(0),
            Some(Mount {
                source: Source::Host(p),
                ..
            }) => {
                w.u8(1);
                w.str(p);
            }
            Some(Mount {
                source: Source::Media(_),
                ..
            }) => w.u8(2),
        }
        w.u8(self.ext_mode);
        w.bool(self.hide_ext);
        w.bool(self.star_1581);
        for ch in &self.channels {
            match ch {
                Channel::Closed => w.u8(0),
                Channel::Read { data, pos } => {
                    w.u8(1);
                    w.bytes(data);
                    w.u32(*pos as u32);
                }
                Channel::Write { target, data } => {
                    match target {
                        Target::Host { path, prefix } => {
                            w.u8(2);
                            w.str(path);
                            w.bytes(prefix);
                        }
                        Target::Image {
                            name,
                            file_type,
                            replace,
                        } => {
                            w.u8(3);
                            w.bytes(name);
                            w.u8(*file_type);
                            w.bool(*replace);
                        }
                    }
                    w.bytes(data);
                }
            }
        }
    }

    /// Counterpart of [`Sd2iecDos::save`]; the folder and a disk in the
    /// media slot stay. An image mounted from the folder is read again when
    /// next used.
    pub fn load(&mut self, c: &mut StateReader<'_>) -> Result<(), StateError> {
        let mut d = Self::new(c.u8()?);
        d.under_atn = c.bool()?;
        d.listening = c.bool()?;
        d.talking = c.bool()?;
        d.opening = c.bool()?;
        d.channel = c.u8()? & 0x0f;
        d.name = c.bytes()?.to_vec();
        d.command = c.bytes()?.to_vec();
        d.status = c.string()?;
        d.cwd = c.string()?;
        emu_core::host::check_path(&d.cwd).map_err(|_| StateError::Invalid("sd2iec directory"))?;
        match c.u8()? {
            0 | 2 => {}
            1 => {
                let p = c.string()?;
                emu_core::host::check_path(&p)
                    .map_err(|_| StateError::Invalid("sd2iec image path"))?;
                d.mount = Some(Mount {
                    source: Source::Host(p),
                    image: None,
                });
            }
            _ => return Err(StateError::Invalid("sd2iec mount")),
        }
        d.ext_mode = c.u8()?.min(4);
        d.hide_ext = c.bool()?;
        d.star_1581 = c.bool()?;
        for ch in &mut d.channels {
            *ch = match c.u8()? {
                0 => Channel::Closed,
                1 => Channel::Read {
                    data: c.bytes()?.to_vec(),
                    pos: c.u32()? as usize,
                },
                2 => {
                    let path = c.string()?;
                    emu_core::host::check_path(&path)
                        .map_err(|_| StateError::Invalid("sd2iec file path"))?;
                    Channel::Write {
                        target: Target::Host {
                            path,
                            prefix: c.bytes()?.to_vec(),
                        },
                        data: c.bytes()?.to_vec(),
                    }
                }
                3 => Channel::Write {
                    target: Target::Image {
                        name: c.bytes()?.to_vec(),
                        file_type: c.u8()? & 7,
                        replace: c.bool()?,
                    },
                    data: c.bytes()?.to_vec(),
                },
                _ => return Err(StateError::Invalid("sd2iec channel")),
            };
        }
        d.files = self.files.take();
        d.protected = self.protected;
        // A disk in the media slot is media, not state: it stays mounted.
        if self.has_media() {
            d.mount = self.mount.take();
        }
        *self = d;
        Ok(())
    }
}

impl IecDos for Sd2iecDos {
    fn set_atn(&mut self, asserted: bool) {
        self.under_atn = asserted;
    }

    fn listening(&self) -> bool {
        self.listening
    }

    fn talking(&self) -> bool {
        self.talking
    }

    fn stop_talking(&mut self) {
        self.talking = false;
    }

    fn next_byte(&mut self) -> Option<(u8, bool)> {
        let ch = usize::from(self.channel);
        match &self.channels[ch] {
            Channel::Read { data, pos } if ch != 15 || !data.is_empty() => {
                data.get(*pos).map(|&b| (b, pos + 1 == data.len()))
            }
            Channel::Read { pos, .. } if ch == 15 => {
                let text = self.status.as_bytes();
                text.get(*pos).map(|&b| (b, pos + 1 == text.len()))
            }
            _ if ch == 15 => {
                let text = self.status.as_bytes();
                text.first().map(|&b| (b, text.len() == 1))
            }
            _ => None,
        }
    }

    fn advance(&mut self) {
        let ch = usize::from(self.channel);
        if ch == 15 {
            match &mut self.channels[15] {
                Channel::Read { data, pos } if !data.is_empty() => {
                    *pos += 1;
                    if *pos >= data.len() {
                        self.channels[15] = Channel::Closed;
                    }
                }
                other => {
                    let pos = match other {
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
                }
            }
            return;
        }
        if let Channel::Read { pos, .. } = &mut self.channels[ch] {
            *pos += 1;
        }
    }

    fn received(&mut self, byte: u8, _eoi: bool) {
        if self.under_atn {
            self.command_byte(byte);
            return;
        }
        if !self.listening {
            return;
        }
        if self.opening {
            self.name.push(byte);
        } else if self.channel == 15 {
            self.command.push(byte);
        } else if let Channel::Write { data, .. } = &mut self.channels[usize::from(self.channel)] {
            data.push(byte);
        }
    }
}

/// `[partition][path]:name` → path part (partition digits dropped) and
/// name; without a colon all of it is the name.
fn split_spec(spec: &[u8]) -> (&[u8], &[u8]) {
    match spec.iter().position(|&c| c == b':') {
        Some(i) => {
            let head = &spec[..i];
            let digits = head.iter().take_while(|c| c.is_ascii_digit()).count();
            (&head[digits..], &spec[i + 1..])
        }
        None => (&[], spec),
    }
}

/// The argument of `CD`/`MD`: as [`split_spec`], but `/a/b` without a
/// colon is path `/a/` and name `b`.
fn split_cd(arg: &[u8]) -> (&[u8], &[u8]) {
    if arg.contains(&b':') {
        return split_spec(arg);
    }
    let digits = arg.iter().take_while(|c| c.is_ascii_digit()).count();
    let arg = &arg[digits..];
    match arg.iter().rposition(|&c| c == b'/') {
        Some(i) => (&arg[..=i], &arg[i + 1..]),
        None => (&[], arg),
    }
}

fn parse_number(s: &[u8]) -> Option<u32> {
    let s: Vec<u8> = s.iter().copied().filter(|&c| c != b' ').collect();
    std::str::from_utf8(&s).ok()?.parse().ok()
}

/// A listing line: link, line number, text, end.
fn line(out: &mut Vec<u8>, link_lo: u8, number: u16, text: &[u8]) {
    out.extend_from_slice(&[link_lo, 0x01]);
    out.extend_from_slice(&number.to_le_bytes());
    out.extend_from_slice(text);
    out.push(0);
}

fn header_text(title: &[u8], id: &[u8; 5]) -> Vec<u8> {
    let mut h = vec![0x12, b'"'];
    let n = title.len().min(16);
    h.extend_from_slice(&title[..n]);
    h.extend(std::iter::repeat_n(b' ', 16 - n));
    h.extend_from_slice(b"\" ");
    h.extend_from_slice(id);
    h
}

/// The folder listing as a BASIC program at `$0401`: blocks of 254 bytes,
/// the low link byte (size mod 254) + 2 so the exact size can be worked
/// out, hidden files marked with `H` (sd2iec README, "Other important
/// notes"). The folder has no free count: 65535 blocks free.
fn listing<'a>(title: &[u8], entries: impl Iterator<Item = &'a Entry>) -> Vec<u8> {
    let mut out = vec![0x01, 0x04];
    line(&mut out, 1, 0, &header_text(title, b"SD 2A"));
    for e in entries {
        let blocks = if e.dir {
            0
        } else {
            e.size.div_ceil(254).min(65535) as u16
        };
        let mut text = Vec::new();
        let pad = match blocks {
            0..=9 => 3,
            10..=99 => 2,
            100..=999 => 1,
            _ => 0,
        };
        text.extend(std::iter::repeat_n(b' ', pad));
        text.push(b'"');
        text.extend_from_slice(&e.name);
        text.push(b'"');
        text.extend(std::iter::repeat_n(b' ', 17 - e.name.len().min(16)));
        text.extend_from_slice(match (e.dir, e.file_type) {
            (true, _) => b"DIR",
            (_, SEQ) => b"SEQ",
            (_, USR) => b"USR",
            (_, REL) => b"REL",
            _ => b"PRG",
        });
        if e.hidden {
            text.extend_from_slice(b" H");
        }
        let link = if e.dir { 1 } else { (e.size % 254) as u8 + 2 };
        line(&mut out, link, blocks, &text);
    }
    line(&mut out, 1, 65535, b"BLOCKS FREE.             ");
    out.extend_from_slice(&[0, 0]);
    out
}

/// `$=P`: one FAT partition.
fn partition_listing() -> Vec<u8> {
    let mut out = vec![0x01, 0x04];
    line(&mut out, 1, 255, &header_text(b"SD2IEC", b"SD 2A"));
    line(&mut out, 1, 1, b" \"SD2IEC          \" FAT");
    line(&mut out, 1, 0, b"BLOCKS FREE.             ");
    out.extend_from_slice(&[0, 0]);
    out
}
