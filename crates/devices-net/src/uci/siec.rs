//! Software IEC target (UCI target 5; source: Ultimate documentation,
//! "Software IEC Target",
//! https://1541u-documentation.readthedocs.io/en/latest/uci/software_iec_target.html).
//!
//! Direct access to the files a KERNAL replacement would reach through the
//! Ultimate's Software IEC drive, without the serial protocol: LOAD and
//! SAVE move the data by DMA, OPEN/CHKIN/CHKOUT/CLOSE mirror the KERNAL
//! calls. Partitions map numbers to folders inside the frontend's
//! [`FileHost`] folder (partition 0 is its root until replaced).
//!
//! Names follow CBM DOS: an optional `@` (replace), an optional
//! `<partition>[/path/]:` prefix, the name with `*` and `?` wildcards
//! when reading, then `,P`/`,S`/`,U`/`,R` and `,R`/`,W`/`,A`. Host files
//! carry the type as extension (`.prg`, `.seq`, `.usr`, `.rel`); a file
//! with another extension is a PRG under its full name. `$` loads a
//! directory listing as a BASIC program (`$=P`: the partitions).
//!
//! Status: a single status byte on the status channel (the documentation:
//! "the status returned by a command starts with a single status byte"),
//! [`code`]; LOAD_EX also puts it first in the data, followed by the end
//! address.
//!
//! Unverified (the documentation leaves them open, chosen plausibly):
//! partition 0 at the folder's root; LOAD_SU answers the file's own start
//! address and reads the whole file then; LOAD_EX's end address is
//! exclusive (as the KERNAL's) and the data stops at `$FFFF`; SAVE's end
//! address is exclusive; OPEN's status byte; CHKIN's packets grow from 32
//! to 256 bytes by doubling, the last packet ends the stream ("data
//! last"); a command pushed while a CHKIN stream is open ends it and the
//! channel keeps what the C64 has not read; LOAD without LOAD_SU and
//! CHKOUT to a channel not open for writing answer `$01`/`$02`; the
//! directory listing's layout and its blocks-free count (65535); GET_FATNAME's
//! answers for read channels name the existing file when one matches.

use super::*;

/// Status bytes.
pub mod code {
    pub const OK: u8 = 0x00;
    pub const FILE_NOT_FOUND: u8 = 0x01;
    pub const SAVE_ERROR: u8 = 0x02;
    pub const NO_INPUT_CHANNEL: u8 = 0x03;
    pub const UNKNOWN_COMMAND: u8 = 0x04;
    pub const NOT_LOADED: u8 = 0x05;
    pub const INVALID_PARAMETERS: u8 = 0x06;
    pub const INVALID_NAME: u8 = 0x07;
    pub const INVALID_PARTITION: u8 = 0x08;
    pub const INVALID_DIRECTORY: u8 = 0x09;
    pub const VERIFY_ERROR: u8 = 0x80;
}

/// File types of GET_IECNAME.
pub mod ftype {
    pub const ANY: u8 = 0;
    pub const PRG: u8 = 1;
    pub const SEQ: u8 = 2;
    pub const USR: u8 = 3;
    pub const REL: u8 = 4;
    pub const DIR: u8 = 5;
}

const IDENT: &[u8] = b"SOFTWARE IEC TARGET V1.0";
const FIRST_PACKET: u16 = 32;
const LAST_PACKET: u16 = 256;

/// A channel opened with OPEN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub handle: FileHandle,
    write: bool,
    path: String,
    /// The file position (the host has no "tell").
    pos: u64,
}

/// A load prepared by LOAD_SU.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Load {
    addr: u16,
    data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiecState {
    /// Partition number → (name, folder path).
    pub(super) partitions: BTreeMap<u8, (String, String)>,
    /// Secondary address → channel.
    pub(super) channels: BTreeMap<u8, Channel>,
    load: Option<Load>,
}

impl Default for SiecState {
    fn default() -> Self {
        Self {
            partitions: BTreeMap::from([(0, ("SD".into(), "/".into()))]),
            channels: BTreeMap::new(),
            load: None,
        }
    }
}

impl SiecState {
    pub fn save(&self, w: &mut StateWriter) {
        w.u32(self.partitions.len() as u32);
        for (i, (n, p)) in &self.partitions {
            w.u8(*i);
            w.str(n);
            w.str(p);
        }
        w.u32(self.channels.len() as u32);
        for (sa, c) in &self.channels {
            w.u8(*sa);
            w.u32(c.handle);
            w.bool(c.write);
            w.str(&c.path);
            w.u64(c.pos);
        }
        w.bool(self.load.is_some());
        if let Some(l) = &self.load {
            w.u16(l.addr);
            w.bytes(&l.data);
        }
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let bad = StateError::Invalid("UCI Software IEC state");
        let mut s = Self {
            partitions: BTreeMap::new(),
            channels: BTreeMap::new(),
            load: None,
        };
        let n = r.u32()?;
        if n > 256 {
            return Err(bad);
        }
        for _ in 0..n {
            let i = r.u8()?;
            let name = r.string()?;
            let path = r.string()?;
            check_path(&path).map_err(|_| bad.clone())?;
            s.partitions.insert(i, (name, path));
        }
        let n = r.u32()?;
        if n > 16 {
            return Err(bad);
        }
        for _ in 0..n {
            let sa = r.u8()? & 0x0f;
            s.channels.insert(
                sa,
                Channel {
                    handle: r.u32()?,
                    write: r.bool()?,
                    path: r.string()?,
                    pos: r.u64()?,
                },
            );
        }
        if r.bool()? {
            s.load = Some(Load {
                addr: r.u16()?,
                data: r.bytes()?.to_vec(),
            });
        }
        Ok(s)
    }
}

use emu_core::host::check_path;

/// A CBM DOS file name taken apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct IecName {
    partition: u8,
    /// Folder path inside the partition (`/`-separated, may be empty).
    dir: String,
    name: String,
    ftype: Option<u8>,
    /// `R`, `W`, `A` or `M`.
    mode: Option<char>,
    replace: bool,
}

fn parse_name(raw: &str) -> Option<IecName> {
    let mut n = IecName::default();
    let mut rest = raw;
    if let Some(r) = rest.strip_prefix('@') {
        n.replace = true;
        rest = r;
    }
    if let Some((prefix, r)) = rest.split_once(':') {
        let digits = prefix
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(prefix.len());
        if digits > 0 {
            n.partition = prefix[..digits].parse().ok()?;
        }
        let path = &prefix[digits..];
        if !path.is_empty() && !path.starts_with('/') {
            return None;
        }
        n.dir = path.trim_matches('/').to_string();
        rest = r;
    }
    let mut parts = rest.split(',');
    n.name = parts.next()?.to_string();
    for p in parts {
        let c = p.trim().chars().next()?.to_ascii_uppercase();
        match c {
            'P' if n.ftype.is_none() => n.ftype = Some(ftype::PRG),
            'S' if n.ftype.is_none() => n.ftype = Some(ftype::SEQ),
            'U' if n.ftype.is_none() => n.ftype = Some(ftype::USR),
            'L' if n.ftype.is_none() => n.ftype = Some(ftype::REL),
            'R' | 'W' | 'A' | 'M' if n.mode.is_none() => n.mode = Some(c),
            // `,R` right after the name is REL only with a record length.
            _ => return None,
        }
    }
    Some(n)
}

/// A host file name's type and IEC name.
fn iec_of(host: &str, is_dir: bool) -> (u8, String) {
    if is_dir {
        return (ftype::DIR, host.to_string());
    }
    match host.rsplit_once('.') {
        Some((stem, ext)) => match ext.to_ascii_lowercase().as_str() {
            "prg" => (ftype::PRG, stem.into()),
            "seq" => (ftype::SEQ, stem.into()),
            "usr" => (ftype::USR, stem.into()),
            "rel" => (ftype::REL, stem.into()),
            _ => (ftype::PRG, host.into()),
        },
        None => (ftype::PRG, host.into()),
    }
}

fn extension(t: u8) -> &'static str {
    match t {
        ftype::SEQ => ".seq",
        ftype::USR => ".usr",
        ftype::REL => ".rel",
        _ => ".prg",
    }
}

/// CBM DOS pattern match, case-insensitive: `*` ends the pattern, `?`
/// matches one character.
fn matches(pattern: &str, name: &str) -> bool {
    let mut n = name.chars();
    for p in pattern.chars() {
        match p {
            '*' => return true,
            '?' => {
                if n.next().is_none() {
                    return false;
                }
            }
            c => match n.next() {
                Some(x) if x.eq_ignore_ascii_case(&c) => {}
                _ => return false,
            },
        }
    }
    n.next().is_none()
}

impl Uci {
    fn siec_status(&mut self, b: u8) {
        self.stat = vec![b];
        self.stat_pos = 0;
    }

    /// The folder of a name's partition and path.
    fn siec_dir(&self, n: &IecName) -> Result<String, u8> {
        let (_, root) = self
            .siec
            .partitions
            .get(&n.partition)
            .ok_or(code::INVALID_PARTITION)?;
        join_path(root, &n.dir).map_err(|_| code::INVALID_DIRECTORY)
    }

    /// The existing file a read of `n` opens.
    fn siec_find(&self, n: &IecName) -> Result<String, u8> {
        let dir = self.siec_dir(n)?;
        let list = self
            .fs(|f| f.list_dir(&dir))
            .map_err(|_| code::INVALID_DIRECTORY)?;
        list.iter()
            .find(|e| {
                let (t, name) = iec_of(&e.name, e.stat.is_dir);
                t != ftype::DIR && n.ftype.is_none_or(|w| w == t) && matches(&n.name, &name)
            })
            .and_then(|e| join_path(&dir, &e.name).ok())
            .ok_or(code::FILE_NOT_FOUND)
    }

    /// The file a write of `n` creates.
    fn siec_target(&self, n: &IecName, default: u8) -> Result<String, u8> {
        if n.name.is_empty() || n.name.contains(['*', '?', '/']) {
            return Err(code::INVALID_NAME);
        }
        let dir = self.siec_dir(n)?;
        let t = n.ftype.unwrap_or(default);
        join_path(&dir, &format!("{}{}", n.name, extension(t))).map_err(|_| code::INVALID_NAME)
    }

    pub(super) fn siec_command(&mut self, cmd: &[u8], m: &mut dyn UciMachine) {
        let Some((&op, p)) = cmd.split_first() else {
            self.siec_status(code::INVALID_PARAMETERS);
            return self.finish(false);
        };
        match op {
            0x01 => {
                self.set_resp(IDENT.to_vec());
                self.siec_status(code::OK);
            }
            0x10 => {
                let status = match p {
                    [sa, _verify, lo, hi, name @ ..] => {
                        self.load_setup(*sa, u16::from_le_bytes([*lo, *hi]), name)
                    }
                    _ => Err(code::INVALID_PARAMETERS),
                };
                match status {
                    Ok(start) => {
                        self.set_resp(start.to_le_bytes().to_vec());
                        self.siec_status(code::OK);
                    }
                    Err(c) => self.siec_status(c),
                }
            }
            0x11 => {
                let verify = p.get(1).is_some_and(|&v| v != 0);
                let (status, end) = match self.siec.load.take() {
                    None => (code::FILE_NOT_FOUND, 0),
                    Some(l) => {
                        let mut addr = l.addr;
                        let mut status = code::OK;
                        for &b in l.data.iter().skip(2) {
                            if verify {
                                if m.dma_read(addr) != b {
                                    status = code::VERIFY_ERROR;
                                    break;
                                }
                            } else {
                                m.dma_write(addr, b);
                            }
                            if addr == 0xffff {
                                break;
                            }
                            addr += 1;
                        }
                        (status, addr)
                    }
                };
                let [lo, hi] = end.to_le_bytes();
                self.set_resp(vec![status, lo, hi]);
                self.siec_status(status);
            }
            0x12 => {
                let status = match p {
                    [_verify, _sa, s0, s1, e0, e1, name @ ..] => {
                        let (start, end) = (
                            u16::from_le_bytes([*s0, *s1]),
                            u16::from_le_bytes([*e0, *e1]),
                        );
                        self.save_file(start, end, name, m)
                    }
                    _ => code::INVALID_PARAMETERS,
                };
                self.siec_status(status);
            }
            0x13 => {
                let status = match p {
                    [sa, _, name @ ..] => self.open_channel(*sa & 0x0f, name),
                    _ => code::INVALID_PARAMETERS,
                };
                self.siec_status(status);
            }
            0x14 => {
                if let Some(&sa) = p.first() {
                    self.close_channel(sa & 0x0f);
                }
                self.siec_status(code::OK);
            }
            0x15 => match p.first().map(|sa| sa & 0x0f) {
                Some(sa) if self.siec.channels.get(&sa).is_some_and(|c| !c.write) => {
                    return self.chkin_packet(sa, FIRST_PACKET);
                }
                _ => self.siec_status(code::NO_INPUT_CHANNEL),
            },
            0x16 => {
                let status = match p {
                    [sa @ 0xf0..=0xff, _, name @ ..] => self.open_channel(sa & 0x0f, name),
                    [sa @ 0xe0..=0xef, ..] => {
                        self.close_channel(sa & 0x0f);
                        code::OK
                    }
                    [sa, _, data @ ..] => self.write_channel(sa & 0x0f, data),
                    _ => code::INVALID_PARAMETERS,
                };
                self.siec_status(status);
            }
            0x20 => {
                let colon = p.iter().position(|&b| b == b':');
                let status = match (p.split_first(), colon) {
                    (Some((&index, rest)), Some(c)) if cmd.len() >= 5 && c > 0 => {
                        let ident = name(&rest[..c - 1]);
                        match join_path("/", &name(&rest[c..])) {
                            Ok(path) => {
                                self.siec.partitions.insert(index, (ident, path));
                                code::OK
                            }
                            Err(_) => code::INVALID_DIRECTORY,
                        }
                    }
                    _ => code::INVALID_PARAMETERS,
                };
                self.siec_status(status);
            }
            0x21 => {
                if let [index] = p {
                    self.siec.partitions.remove(index);
                    self.siec_status(code::OK);
                } else {
                    self.siec_status(code::INVALID_PARAMETERS);
                }
            }
            0x22 => match p.split_first() {
                Some((&channel, raw)) if !raw.is_empty() => match self.fat_name(channel, raw) {
                    Ok(path) => {
                        self.set_resp(path.chars().map(|c| c as u32 as u8).collect());
                        self.siec_status(code::OK);
                    }
                    Err(c) => self.siec_status(c),
                },
                _ => self.siec_status(code::INVALID_PARAMETERS),
            },
            0x23 => {
                let host = name(p);
                if host.is_empty() {
                    self.siec_status(code::INVALID_PARAMETERS);
                } else {
                    let file = host.rsplit('/').next().unwrap_or(&host);
                    let (t, iec) = iec_of(file, host.ends_with('/'));
                    let mut out = vec![t];
                    out.extend(iec.chars().map(|c| c as u32 as u8));
                    self.set_resp(out);
                    self.siec_status(code::OK);
                }
            }
            _ => self.siec_status(code::UNKNOWN_COMMAND),
        }
        self.finish(false)
    }

    /// LOAD_SU: read the file (or the directory) and keep it for LOAD_EX.
    fn load_setup(&mut self, sa: u8, addr: u16, raw: &[u8]) -> Result<u16, u8> {
        let text = name(raw);
        let data = if let Some(pattern) = text.strip_prefix('$') {
            self.listing(pattern)?
        } else {
            let n = parse_name(&text).ok_or(code::INVALID_NAME)?;
            let path = self.siec_find(&n)?;
            self.fs(|f| read_file(f, &path))
                .map_err(|_| code::FILE_NOT_FOUND)?
        };
        if data.len() < 2 {
            return Err(code::FILE_NOT_FOUND);
        }
        let start = u16::from_le_bytes([data[0], data[1]]);
        self.siec.load = Some(Load {
            addr: if sa == 0 { addr } else { start },
            data,
        });
        Ok(start)
    }

    fn save_file(&mut self, start: u16, end: u16, raw: &[u8], m: &mut dyn UciMachine) -> u8 {
        let Some(n) = parse_name(&name(raw)) else {
            return code::INVALID_NAME;
        };
        let path = match self.siec_target(&n, ftype::PRG) {
            Ok(p) => p,
            Err(c) => return c,
        };
        let mut data = start.to_le_bytes().to_vec();
        let mut a = start;
        while a < end {
            data.push(m.dma_read(a));
            a += 1;
        }
        let r = self.fs(|f| {
            if !n.replace && f.stat(&path).is_ok() {
                return Err(FsError::Exists);
            }
            write_file(f, &path, &data)
        });
        if r.is_ok() {
            code::OK
        } else {
            code::SAVE_ERROR
        }
    }

    fn open_channel(&mut self, sa: u8, raw: &[u8]) -> u8 {
        self.close_channel(sa);
        let Some(n) = parse_name(&name(raw)) else {
            return code::INVALID_NAME;
        };
        let write = match n.mode {
            Some('W' | 'A') => true,
            Some(_) => false,
            None => sa == 1,
        };
        let default = if sa <= 1 { ftype::PRG } else { ftype::SEQ };
        let path = if write {
            match n.mode {
                Some('A') => self.siec_find(&n),
                _ => self.siec_target(&n, default),
            }
        } else {
            self.siec_find(&n)
        };
        let path = match path {
            Ok(p) => p,
            Err(c) => return c,
        };
        let mode = OpenMode {
            read: !write,
            write,
            create: write,
            create_new: write && !n.replace && n.mode != Some('A'),
            truncate: write && n.mode != Some('A'),
        };
        let r = self.fs(|f| {
            let h = f.open(&path, mode)?;
            let pos = if n.mode == Some('A') {
                let size = f.fstat(h)?.size;
                f.seek(h, size)?;
                size
            } else {
                0
            };
            Ok((h, pos))
        });
        match r {
            Ok((handle, pos)) => {
                self.siec.channels.insert(
                    sa,
                    Channel {
                        handle,
                        write,
                        path,
                        pos,
                    },
                );
                code::OK
            }
            Err(_) if write => code::SAVE_ERROR,
            Err(_) => code::FILE_NOT_FOUND,
        }
    }

    fn close_channel(&mut self, sa: u8) {
        if let Some(c) = self.siec.channels.remove(&sa) {
            let _ = self.fs(|f| {
                f.close(c.handle);
                Ok(())
            });
        }
    }

    fn write_channel(&mut self, sa: u8, data: &[u8]) -> u8 {
        let Some(c) = self.siec.channels.get(&sa).filter(|c| c.write).cloned() else {
            return code::SAVE_ERROR;
        };
        match self.fs(|f| f.write(c.handle, data)) {
            Ok(n) => {
                if let Some(ch) = self.siec.channels.get_mut(&sa) {
                    ch.pos += n as u64;
                }
                code::OK
            }
            Err(_) => code::SAVE_ERROR,
        }
    }

    /// One CHKIN packet of up to `size` bytes; more follow while the file
    /// lasts.
    pub(super) fn chkin_packet(&mut self, sa: u8, size: u16) {
        let Some(c) = self.siec.channels.get(&sa).cloned() else {
            self.siec_status(code::NO_INPUT_CHANNEL);
            return self.finish(false);
        };
        let mut buf = vec![0; usize::from(size)];
        let n = match self.fs(|f| f.read(c.handle, &mut buf)) {
            Ok(n) => n,
            Err(_) => {
                self.siec_status(code::FILE_NOT_FOUND);
                return self.finish(false);
            }
        };
        buf.truncate(n);
        if let Some(ch) = self.siec.channels.get_mut(&sa) {
            ch.pos += n as u64;
        }
        self.set_resp(buf);
        self.siec_status(code::OK);
        if n < usize::from(size) {
            return self.finish(false);
        }
        self.pending = Pending::Chkin {
            sa,
            start: c.pos,
            size: (size * 2).min(LAST_PACKET),
        };
        self.finish(true)
    }

    /// A command while a CHKIN stream is open: the channel goes back to
    /// the first byte the C64 has not read.
    pub(super) fn end_chkin(&mut self) {
        if let Pending::Chkin { sa, start, .. } = self.pending {
            let pos = start + self.resp_pos as u64;
            if let Some(c) = self.siec.channels.get(&sa).cloned() {
                if self.fs(|f| f.seek(c.handle, pos)).is_ok() {
                    if let Some(ch) = self.siec.channels.get_mut(&sa) {
                        ch.pos = pos;
                    }
                }
            }
        }
        self.pending = Pending::None;
        self.clear_queues();
        self.state = State::Idle;
        self.data_acc = false;
    }

    fn fat_name(&self, channel: u8, raw: &[u8]) -> Result<String, u8> {
        let text = name(raw);
        if text.starts_with('#') {
            return Ok("/buffer".into());
        }
        if let Some(rest) = text.strip_prefix('$') {
            if rest.starts_with("=P") {
                return Ok("/partitions".into());
            }
            let n = parse_name(rest.trim_start_matches(':')).unwrap_or_default();
            return self.siec_dir(&n);
        }
        let n = parse_name(&text).ok_or(code::INVALID_NAME)?;
        let ch = channel & 0x0f;
        let write = match n.mode {
            Some('W' | 'A') => true,
            Some(_) => false,
            None => ch == 1,
        };
        let default = if ch <= 1 { ftype::PRG } else { ftype::SEQ };
        if write {
            self.siec_target(&n, default)
        } else {
            self.siec_find(&n)
                .or_else(|_| self.siec_target(&n, default))
        }
    }

    /// LOAD"$": the folder (or `$=P`: the partitions) as a BASIC listing
    /// at `$0401`, like a 1541's.
    fn listing(&self, pattern: &str) -> Result<Vec<u8>, u8> {
        let mut lines: Vec<(u16, Vec<u8>)> = Vec::new();
        let quoted = |s: &str| {
            let mut v = vec![b'"'];
            v.extend(s.chars().take(16).map(|c| c as u32 as u8));
            v.push(b'"');
            v
        };
        let pad = |v: &mut Vec<u8>, n: usize| {
            while v.len() < n {
                v.push(b' ');
            }
        };
        if pattern.starts_with("=P") {
            let mut head = vec![0x12];
            head.extend(quoted("PARTITIONS"));
            lines.push((0, head));
            for (i, (name, _)) in &self.siec.partitions {
                let mut v = Vec::new();
                pad(&mut v, 4 - i.to_string().len().min(3));
                v.extend(quoted(name));
                pad(&mut v, 23);
                v.extend(b"DIR");
                lines.push((u16::from(*i), v));
            }
        } else {
            let n = parse_name(pattern.trim_start_matches(':')).unwrap_or_default();
            let dir = self.siec_dir(&n)?;
            let filter = if n.name.is_empty() {
                "*"
            } else {
                n.name.as_str()
            };
            let list = self
                .fs(|f| f.list_dir(&dir))
                .map_err(|_| code::INVALID_DIRECTORY)?;
            let (_, title) = &self.siec.partitions[&n.partition];
            let mut head = vec![0x12];
            let mut t = quoted(title);
            t.truncate(1);
            t.extend(title.chars().take(16).map(|c| c as u32 as u8));
            pad(&mut t, 17);
            t.push(b'"');
            head.extend(t);
            head.extend(b" 00 2A");
            lines.push((u16::from(n.partition), head));
            for e in &list {
                let (t, iec) = iec_of(&e.name, e.stat.is_dir);
                if !matches(filter, &iec) || n.ftype.is_some_and(|w| w != t) {
                    continue;
                }
                let blocks = e.stat.size.div_ceil(254).min(65535) as u16;
                let mut v = Vec::new();
                pad(&mut v, 4 - blocks.to_string().len().min(3));
                v.extend(quoted(&iec));
                pad(&mut v, 23);
                v.extend(match t {
                    ftype::SEQ => b"SEQ",
                    ftype::USR => b"USR",
                    ftype::REL => b"REL",
                    ftype::DIR => b"DIR",
                    _ => b"PRG",
                });
                lines.push((blocks, v));
            }
            lines.push((65535, b"BLOCKS FREE.".to_vec()));
        }
        let mut out = vec![0x01, 0x04];
        let mut addr: u16 = 0x0401;
        for (num, text) in lines {
            let next = addr + 4 + text.len() as u16 + 1;
            out.extend(next.to_le_bytes());
            out.extend(num.to_le_bytes());
            out.extend(text);
            out.push(0);
            addr = next;
        }
        out.extend([0, 0]);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_patterns() {
        let n = parse_name("@3/games/:ZORK*,S,W").unwrap();
        assert_eq!(
            n,
            IecName {
                partition: 3,
                dir: "games".into(),
                name: "ZORK*".into(),
                ftype: Some(ftype::SEQ),
                mode: Some('W'),
                replace: true,
            }
        );
        assert_eq!(parse_name("0:HELLO").unwrap().name, "HELLO");
        assert!(parse_name("HELLO,X").is_none());
        assert!(matches("ZO?K*", "zork i"));
        assert!(matches("hello", "HELLO"));
        assert!(!matches("hell", "hello"));
        assert_eq!(iec_of("game.PRG", false), (ftype::PRG, "game".into()));
        assert_eq!(iec_of("notes.seq", false), (ftype::SEQ, "notes".into()));
        assert_eq!(
            iec_of("readme.txt", false),
            (ftype::PRG, "readme.txt".into())
        );
    }
}
