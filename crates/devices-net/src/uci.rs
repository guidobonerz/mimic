//! Ultimate Command Interface (UCI) of the Ultimate 64 / 1541 Ultimate II+.
//!
//! Four registers in I/O2 (source: Ultimate documentation, "Core UCI
//! Architecture", https://1541u-documentation.readthedocs.io/en/latest/uci/core_uci_architecture.html):
//!
//! | Address | Write                      | Read                          |
//! | ------- | -------------------------- | ----------------------------- |
//! | `$DF1C` | control                    | status                        |
//! | `$DF1D` | command data (FIFO)        | identification, `$C9`         |
//! | `$DF1E` | –                          | response data (queue)         |
//! | `$DF1F` | –                          | status data (queue)           |
//!
//! Control (write): bit 3 CLR_ERR, bit 2 ABORT, bit 1 DATA_ACC, bit 0
//! PUSH_CMD (bits 7–5, DMA/TRIGGER/IRQ, are not modelled). Status (read):
//! bit 7 DATA_AV, bit 6 STAT_AV, bits 5–4 state (00 idle, 01 busy, 10 data
//! last, 11 data more), bit 3 ERROR (command pushed while not idle), bit 2
//! ABORT_P, bit 1 DATA_ACC, bit 0 CMD_BUSY. Queues: command 896 bytes,
//! response 896, status 256.
//!
//! Protocol: write the command bytes to `$DF1D` (first byte = target),
//! set PUSH_CMD; the interface goes busy, then to "data last" or "data
//! more" with the response and status text ("00,OK") in their queues; the
//! C64 reads them and sets DATA_ACC: from "data last" back to idle, from
//! "data more" busy again for the next packet.
//!
//! Targets:
//! - 1 and 2: Ultimate DOS, each with its own directory and open file, on
//!   the frontend's [`FileHost`] folder (source: "Ultimate DOS Target",
//!   https://1541u-documentation.readthedocs.io/en/latest/uci/ultimate_dos_target.html).
//! - 3: network, TCP/UDP sockets on the [`NetBackend`] (source: "Network
//!   Target", https://1541u-documentation.readthedocs.io/en/latest/uci/network_target.html).
//! - 4: control, IDENTIFY and GET_HWINFO only (source: "Control Target",
//!   https://1541u-documentation.readthedocs.io/en/latest/uci/control_target.html);
//!   the machine-control commands (freeze, reboot, drives, REU, palette)
//!   and the DOS target's mount/REU commands are not offered.
//!
//! Unverified (not in the documentation, chosen plausibly): the busy time
//! of a command (64 cycles) and the poll interval while connecting (1000
//! cycles); the status texts for file-system errors ("87,…") and failed
//! connections ("85,CONNECTION FAILED"), unknown commands/targets
//! ("21,UNKNOWN COMMAND"), bad socket handles ("82,…"); socket handles
//! counting from 1; READ_SOCKET with nothing there (a zero count with
//! "00,OK"); GET_PATH's trailing `/`; GET_TIME's weekday prefix; the DOS
//! identification string; the TCP listener commands `$12`–`$15` and their
//! state values (from the Ultimate's network target as remembered, not in
//! the current documentation). Nothing here comes from the Ultimate
//! firmware's source (GPL; golden rule 7).
//!
//! Savestates hold everything but the host's side: open files, sockets
//! and the listener are host handles. After loading a state, the handles
//! the state names are used as they are: a live host normally no longer
//! knows them and the guest sees errors; a replay answers them from its
//! journal. Handles open before the load are closed.

use std::collections::BTreeMap;

use emu_core::host::{
    join_path, lock, ConnState, DirEntry, FileHandle, FileHost, FileStat, FsError, HostServices,
    NetError, NetHandle, OpenMode, Recv, SocketKind,
};
use emu_core::{StateError, StateReader, StateWriter};

/// The UCI's registers: `$DF1C–$DF1F`.
pub fn is_uci(addr: u16) -> bool {
    (0xdf1c..=0xdf1f).contains(&addr)
}

const CMD_MAX: usize = 896;
const RESP_MAX: usize = 896;
const STATUS_MAX: usize = 256;
/// `$DF1D` read: the identification byte.
pub const IDENT: u8 = 0xc9;
/// Cycles a command or packet keeps the interface busy (unverified).
const TURNAROUND: u32 = 64;
/// Cycles between polls of a socket being connected (unverified).
const POLL: u32 = 1000;
/// Chunk size of READ_DATA.
const CHUNK: usize = 512;

/// Control register bits.
pub mod ctrl {
    pub const PUSH_CMD: u8 = 1 << 0;
    pub const DATA_ACC: u8 = 1 << 1;
    pub const ABORT: u8 = 1 << 2;
    pub const CLR_ERR: u8 = 1 << 3;
}

/// Status register bits.
pub mod status {
    pub const CMD_BUSY: u8 = 1 << 0;
    pub const DATA_ACC: u8 = 1 << 1;
    pub const ABORT_P: u8 = 1 << 2;
    pub const ERROR: u8 = 1 << 3;
    pub const STATE_SHIFT: u8 = 4;
    pub const STAT_AV: u8 = 1 << 6;
    pub const DATA_AV: u8 = 1 << 7;
}

/// Targets (first command byte).
pub mod target {
    pub const DOS1: u8 = 1;
    pub const DOS2: u8 = 2;
    pub const NETWORK: u8 = 3;
    pub const CONTROL: u8 = 4;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle = 0,
    Busy = 1,
    DataLast = 2,
    DataMore = 3,
}

/// Work left while busy.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    None,
    /// A pushed command runs when the busy time is over.
    Command,
    /// More READ_DATA chunks of DOS target `t`.
    ReadFile {
        t: u8,
        left: u32,
    },
    /// The next READ_DIR entry of DOS target `t`.
    ReadDir {
        t: u8,
    },
    /// OPEN_TCP/OPEN_UDP waiting for the connection.
    Connect {
        handle: NetHandle,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct OpenFile {
    handle: FileHandle,
    path: String,
    write: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Dos {
    cwd: String,
    file: Option<OpenFile>,
    dir: Option<(Vec<DirEntry>, usize)>,
}

impl Default for Dos {
    fn default() -> Self {
        Self {
            cwd: "/".into(),
            file: None,
            dir: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Listener {
    handle: NetHandle,
    /// 0 not listening, 1 listening, 2 connected, 3 error (unverified).
    state: u8,
    socket: Option<u8>,
}

/// The command interface. Host services come from [`Uci::attach_host`];
/// without them files and network report errors.
#[derive(Clone)]
pub struct Uci {
    cmd: Vec<u8>,
    resp: Vec<u8>,
    resp_pos: usize,
    stat: Vec<u8>,
    stat_pos: usize,
    state: State,
    error: bool,
    abort: bool,
    data_acc: bool,
    pending: Pending,
    timer: u32,
    dos: [Dos; 2],
    /// UCI socket number → host handle.
    sockets: BTreeMap<u8, NetHandle>,
    listener: Option<Listener>,
    host: HostServices,
}

impl std::fmt::Debug for Uci {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Uci")
            .field("state", &self.state)
            .field("cmd", &self.cmd.len())
            .field("resp", &self.resp.len())
            .finish()
    }
}

/// Machine state only; the host services are not compared.
impl PartialEq for Uci {
    fn eq(&self, o: &Self) -> bool {
        (
            &self.cmd,
            &self.resp,
            self.resp_pos,
            &self.stat,
            self.stat_pos,
            self.state,
            self.error,
            self.abort,
            self.data_acc,
            &self.pending,
            self.timer,
        ) == (
            &o.cmd, &o.resp, o.resp_pos, &o.stat, o.stat_pos, o.state, o.error, o.abort,
            o.data_acc, &o.pending, o.timer,
        ) && self.dos == o.dos
            && self.sockets == o.sockets
            && self.listener == o.listener
    }
}

impl Default for Uci {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes from the C64 as a name: one char per byte (Latin-1).
fn name(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| char::from(b)).collect()
}

/// Two NUL-separated names.
fn two_names(bytes: &[u8]) -> Option<(String, String)> {
    let z = bytes.iter().position(|&b| b == 0)?;
    Some((name(&bytes[..z]), name(&bytes[z + 1..])))
}

fn fs_status(e: &FsError) -> String {
    match e {
        FsError::NotFound => "88,FILE NOT FOUND".into(),
        FsError::Exists => "87,FILE EXISTS".into(),
        FsError::Denied => "87,ACCESS DENIED".into(),
        FsError::NotADirectory => "83,NO SUCH DIRECTORY".into(),
        FsError::InvalidPath => "87,INVALID PATH".into(),
        FsError::Unavailable => "87,DISK ERR: NO FOLDER".into(),
        other => format!("87,DISK ERR: {}", other.to_string().to_uppercase()),
    }
}

fn net_status(e: &NetError) -> String {
    match e {
        NetError::Resolve(_) => "84,UNRESOLVED HOST".into(),
        NetError::NoNetwork => "85,CONNECTION FAILED: NO NETWORK".into(),
        _ => "85,CONNECTION FAILED".into(),
    }
}

/// FAT date and time words from seconds since 1970 (UTC).
fn fat_date_time(t: Option<i64>) -> (u16, u16) {
    let Some(t) = t else { return (0x21, 0) }; // 1980-01-01
    let (y, mo, d, h, mi, s, _) = emu_core::host::HostTime {
        unix_secs: t,
        utc_offset_secs: 0,
    }
    .local();
    let y = (y - 1980).clamp(0, 127) as u16;
    (
        (y << 9) | (u16::from(mo) << 5) | u16::from(d),
        (u16::from(h) << 11) | (u16::from(mi) << 5) | u16::from(s / 2),
    )
}

/// FAT attribute byte of a directory entry.
fn fat_attrib(s: &FileStat) -> u8 {
    let mut a = if s.is_dir { 0x10 } else { 0x20 };
    if s.read_only {
        a |= 0x01;
    }
    a
}

/// FILE_INFO / FILE_STAT packet: size, date, time, extension, attribute,
/// name.
fn info_packet(stat: &FileStat, path: &str) -> Vec<u8> {
    let file = path.rsplit('/').next().unwrap_or("");
    let ext: Vec<u8> = match file.rsplit_once('.') {
        Some((_, e)) if !stat.is_dir => e.bytes().map(|b| b.to_ascii_uppercase()).collect(),
        _ => Vec::new(),
    };
    let (date, time) = fat_date_time(stat.modified);
    let mut p = Vec::new();
    p.extend((stat.size.min(u64::from(u32::MAX)) as u32).to_le_bytes());
    p.extend(date.to_le_bytes());
    p.extend(time.to_le_bytes());
    for i in 0..3 {
        p.push(*ext.get(i).unwrap_or(&b' '));
    }
    p.push(fat_attrib(stat));
    p.extend(file.chars().map(|c| c as u32 as u8));
    p
}

impl Uci {
    pub fn new() -> Self {
        Self {
            cmd: Vec::new(),
            resp: Vec::new(),
            resp_pos: 0,
            stat: Vec::new(),
            stat_pos: 0,
            state: State::Idle,
            error: false,
            abort: false,
            data_acc: false,
            pending: Pending::None,
            timer: 0,
            dos: Default::default(),
            sockets: BTreeMap::new(),
            listener: None,
            host: HostServices::default(),
        }
    }

    pub fn attach_host(&mut self, host: HostServices) {
        self.host = host;
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The status register.
    pub fn status(&self) -> u8 {
        let mut v = (self.state as u8) << status::STATE_SHIFT;
        if self.resp_pos < self.resp.len() {
            v |= status::DATA_AV;
        }
        if self.stat_pos < self.stat.len() {
            v |= status::STAT_AV;
        }
        if self.error {
            v |= status::ERROR;
        }
        if self.abort {
            v |= status::ABORT_P;
        }
        if self.data_acc {
            v |= status::DATA_ACC;
        }
        if self.pending == Pending::Command {
            v |= status::CMD_BUSY;
        }
        v
    }

    /// Side-effect-free read of `$DF1C–$DF1F`.
    pub fn peek(&self, addr: u16) -> u8 {
        match addr & 3 {
            0 => self.status(),
            1 => IDENT,
            2 => self.resp.get(self.resp_pos).copied().unwrap_or(0),
            _ => self.stat.get(self.stat_pos).copied().unwrap_or(0),
        }
    }

    /// CPU read: the data and status registers take the byte from their
    /// queue.
    pub fn read(&mut self, addr: u16) -> u8 {
        let v = self.peek(addr);
        match addr & 3 {
            2 if self.resp_pos < self.resp.len() => self.resp_pos += 1,
            3 if self.stat_pos < self.stat.len() => self.stat_pos += 1,
            _ => {}
        }
        v
    }

    pub fn write(&mut self, addr: u16, v: u8) {
        match addr & 3 {
            0 => self.control(v),
            // Command bytes are taken while idle only (unverified).
            1 if self.state == State::Idle && self.cmd.len() < CMD_MAX => self.cmd.push(v),
            _ => {}
        }
    }

    fn control(&mut self, v: u8) {
        if v & ctrl::CLR_ERR != 0 {
            self.error = false;
        }
        if v & ctrl::ABORT != 0 {
            self.abort = true;
            self.timer = TURNAROUND;
        }
        if v & ctrl::DATA_ACC != 0 {
            self.clear_queues();
            match self.state {
                State::DataLast => {
                    self.state = State::Idle;
                    self.pending = Pending::None;
                }
                State::DataMore => {
                    self.state = State::Busy;
                    self.data_acc = true;
                    self.timer = TURNAROUND;
                }
                _ => {}
            }
        }
        if v & ctrl::PUSH_CMD != 0 {
            if self.state == State::Idle {
                self.state = State::Busy;
                self.pending = Pending::Command;
                self.timer = TURNAROUND;
            } else {
                self.error = true;
            }
        }
    }

    fn clear_queues(&mut self) {
        self.resp.clear();
        self.resp_pos = 0;
        self.stat.clear();
        self.stat_pos = 0;
    }

    /// One cycle. Costs a compare while nothing is pending.
    #[inline]
    pub fn tick(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
            if self.timer == 0 {
                self.run();
            }
        }
    }

    /// RESET: the interface restarts with the C64; host files and sockets
    /// are closed.
    pub fn reset(&mut self) {
        self.release_host_handles();
        let host = std::mem::take(&mut self.host);
        *self = Self::new();
        self.host = host;
    }

    fn release_host_handles(&mut self) {
        let files: Vec<FileHandle> = self
            .dos
            .iter()
            .filter_map(|d| d.file.as_ref().map(|f| f.handle))
            .collect();
        if let Some(fs) = &self.host.files {
            let mut fs = lock(fs);
            for h in files {
                fs.close(h);
            }
        }
        let mut socks: Vec<NetHandle> = self.sockets.values().copied().collect();
        socks.extend(self.listener.map(|l| l.handle));
        if let Pending::Connect { handle } = self.pending {
            socks.push(handle);
        }
        if let Some(net) = &self.host.net {
            let mut net = lock(net);
            for h in socks {
                net.close(h);
            }
        }
    }

    fn set_status(&mut self, s: &str) {
        self.stat = s.bytes().take(STATUS_MAX).collect();
        self.stat_pos = 0;
    }

    fn set_resp(&mut self, data: Vec<u8>) {
        self.resp = data;
        self.resp.truncate(RESP_MAX);
        self.resp_pos = 0;
    }

    fn finish(&mut self, more: bool) {
        self.state = if more {
            State::DataMore
        } else {
            State::DataLast
        };
        self.data_acc = false;
        if !more {
            self.pending = Pending::None;
        }
    }

    /// The busy time is over: do what is pending.
    fn run(&mut self) {
        if self.abort {
            self.abort = false;
            if let Pending::Connect { handle } = self.pending {
                if let Some(net) = &self.host.net {
                    lock(net).close(handle);
                }
            }
            self.pending = Pending::None;
            self.cmd.clear();
            self.clear_queues();
            self.state = State::Idle;
            self.data_acc = false;
            return;
        }
        if self.state != State::Busy {
            return;
        }
        match self.pending.clone() {
            Pending::None => self.finish(false),
            Pending::Command => {
                let cmd = std::mem::take(&mut self.cmd);
                self.clear_queues();
                self.pending = Pending::None;
                self.execute(&cmd);
            }
            Pending::ReadFile { t, left } => {
                self.clear_queues();
                self.read_chunk(t, left);
            }
            Pending::ReadDir { t } => {
                self.clear_queues();
                self.read_dir_entry(t);
            }
            Pending::Connect { handle } => self.poll_connect(handle),
        }
    }

    fn execute(&mut self, cmd: &[u8]) {
        let Some((&t, rest)) = cmd.split_first() else {
            self.set_status("81,INVALID PARAMS");
            return self.finish(false);
        };
        match t {
            target::DOS1 | target::DOS2 => self.dos_command(t - 1, cmd, rest),
            target::NETWORK => self.net_command(rest),
            target::CONTROL => self.control_command(rest),
            _ => {
                self.set_status("21,UNKNOWN COMMAND");
                self.finish(false)
            }
        }
    }

    // --- DOS target -----------------------------------------------------

    fn fs<T>(&self, f: impl FnOnce(&mut dyn FileHost) -> Result<T, FsError>) -> Result<T, FsError> {
        match &self.host.files {
            Some(fs) => f(&mut *lock(fs)),
            None => Err(FsError::Unavailable),
        }
    }

    fn done(&mut self, r: Result<(), FsError>) {
        match r {
            Ok(()) => self.set_status("00,OK"),
            Err(e) => self.set_status(&fs_status(&e)),
        }
        self.finish(false);
    }

    fn dos_command(&mut self, t: u8, whole: &[u8], cmd: &[u8]) {
        let i = usize::from(t);
        let Some((&op, p)) = cmd.split_first() else {
            self.set_status("81,INVALID PARAMS");
            return self.finish(false);
        };
        let path = |s: &Self, bytes: &[u8]| join_path(&s.dos[i].cwd, &name(bytes));
        match op {
            0x01 => {
                self.set_resp(b"ULTIMATE-II DOS V1.2".to_vec());
                self.set_status("00,OK");
                self.finish(false)
            }
            0x02 => {
                let Some((&attrib, file)) = p.split_first() else {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                };
                self.close_file(i);
                let mode = OpenMode {
                    read: attrib & 0x01 != 0,
                    write: attrib & 0x0e != 0,
                    create: attrib & 0x08 != 0,
                    create_new: attrib & 0x0c == 0x04,
                    truncate: attrib & 0x08 != 0,
                };
                let r = path(self, file).and_then(|pa| {
                    self.fs(|f| f.open(&pa, mode)).map(|h| OpenFile {
                        handle: h,
                        path: pa,
                        write: mode.write,
                    })
                });
                let r = r.map(|of| self.dos[i].file = Some(of));
                self.done(r)
            }
            0x03 => {
                if self.dos[i].file.is_some() {
                    self.close_file(i);
                    self.set_status("00,OK");
                } else {
                    self.set_status("84,NO FILE TO CLOSE");
                }
                self.finish(false)
            }
            0x04 => {
                if self.dos[i].file.is_none() {
                    self.set_status("85,NO FILE OPEN");
                    return self.finish(false);
                }
                let len = u32::from(u16::from_le_bytes([
                    p.first().copied().unwrap_or(0),
                    p.get(1).copied().unwrap_or(0),
                ]));
                self.read_chunk(t, len)
            }
            0x05 => {
                let Some(of) = self.dos[i].file.clone() else {
                    self.set_status("85,NO FILE OPEN");
                    return self.finish(false);
                };
                if !of.write {
                    self.set_status("87,ACCESS DENIED");
                    return self.finish(false);
                }
                let data = p.get(2..).unwrap_or(&[]);
                let r = self.fs(|f| f.write(of.handle, data)).map(|_| ());
                self.done(r)
            }
            0x06 => {
                let Some(of) = self.dos[i].file.clone() else {
                    self.set_status("85,NO FILE OPEN");
                    return self.finish(false);
                };
                let mut b = [0u8; 4];
                for (d, s) in b.iter_mut().zip(p) {
                    *d = *s;
                }
                let r = self.fs(|f| f.seek(of.handle, u64::from(u32::from_le_bytes(b))));
                self.done(r)
            }
            0x07 => {
                let Some(of) = self.dos[i].file.clone() else {
                    self.set_status("85,NO FILE OPEN");
                    return self.finish(false);
                };
                match self.fs(|f| f.fstat(of.handle)) {
                    Ok(s) => {
                        self.set_resp(info_packet(&s, &of.path));
                        self.set_status("00,OK");
                    }
                    Err(_) => self.set_status("88,NO INFORMATION AVAILABLE"),
                }
                self.finish(false)
            }
            0x08 => {
                let r = path(self, p).and_then(|pa| self.fs(|f| f.stat(&pa)).map(|s| (s, pa)));
                match r {
                    Ok((s, pa)) => {
                        self.set_resp(info_packet(&s, &pa));
                        self.set_status("00,OK");
                    }
                    Err(e) => self.set_status(&fs_status(&e)),
                }
                self.finish(false)
            }
            0x09 => {
                let r = path(self, p).and_then(|pa| self.fs(|f| f.delete(&pa)));
                self.done(r)
            }
            0x0a | 0x0b => {
                let Some((a, b)) = two_names(p) else {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                };
                let cwd = self.dos[i].cwd.clone();
                let r = join_path(&cwd, &a).and_then(|from| {
                    let to = join_path(&cwd, &b)?;
                    if op == 0x0a {
                        self.fs(|f| f.rename(&from, &to))
                    } else {
                        self.fs(|f| copy_file(f, &from, &to))
                    }
                });
                self.done(r)
            }
            0x11 => {
                let r = path(self, p).and_then(|pa| {
                    let s = self.fs(|f| f.stat(&pa))?;
                    s.is_dir.then_some(pa).ok_or(FsError::NotADirectory)
                });
                match r {
                    Ok(pa) => {
                        self.dos[i].cwd = pa;
                        self.set_status("00,OK");
                    }
                    Err(FsError::NotFound | FsError::NotADirectory | FsError::InvalidPath) => {
                        self.set_status("83,NO SUCH DIRECTORY")
                    }
                    Err(e) => self.set_status(&fs_status(&e)),
                }
                self.finish(false)
            }
            0x12 | 0x17 => {
                if op == 0x17 {
                    self.dos[i].cwd = "/".into();
                }
                let cwd = &self.dos[i].cwd;
                let mut s = cwd.clone();
                if !s.ends_with('/') {
                    s.push('/');
                }
                self.set_resp(s.chars().map(|c| c as u32 as u8).collect());
                self.set_status("00,OK");
                self.finish(false)
            }
            0x13 => {
                let cwd = self.dos[i].cwd.clone();
                match self.fs(|f| f.list_dir(&cwd)) {
                    Ok(list) => {
                        self.set_status(if list.is_empty() {
                            "01,DIRECTORY EMPTY"
                        } else {
                            "00,OK"
                        });
                        self.dos[i].dir = Some((list, 0));
                    }
                    Err(_) => {
                        self.dos[i].dir = None;
                        self.set_status("86,CAN'T READ DIRECTORY");
                    }
                }
                self.finish(false)
            }
            0x14 => self.read_dir_entry(t),
            0x15 => {
                self.set_status("99,FUNCTION NOT IMPLEMENTED");
                self.finish(false)
            }
            0x16 => {
                let r = path(self, p).and_then(|pa| self.fs(|f| f.mkdir(&pa)));
                self.done(r)
            }
            0x26 => {
                let (y, mo, d, h, mi, s, wd) = self.host_time().local();
                let mut text = format!("{y:04}/{mo:02}/{d:02} {h:02}:{mi:02}:{s:02}");
                if p.first() == Some(&1) {
                    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                    text = format!("{} {text}", DAYS[usize::from(wd) % 7]);
                }
                self.set_resp(text.into_bytes());
                self.set_status("00,OK");
                self.finish(false)
            }
            0x27 => {
                self.set_status("98,FUNCTION PROHIBITED");
                self.finish(false)
            }
            0xf0 => {
                self.set_resp(whole.to_vec());
                self.set_status("00,OK");
                self.finish(false)
            }
            // LOAD_REU, SAVE_REU, MOUNT_DISK, UMOUNT_DISK, SWAP_DISK: the
            // Ultimate's machine control, not offered here.
            0x21..=0x25 => {
                self.set_status("99,FUNCTION NOT IMPLEMENTED");
                self.finish(false)
            }
            _ => {
                self.set_status("21,UNKNOWN COMMAND");
                self.finish(false)
            }
        }
    }

    fn host_time(&self) -> emu_core::host::HostTime {
        self.host
            .net
            .as_ref()
            .map(|n| lock(n).time())
            .unwrap_or_default()
    }

    fn close_file(&mut self, i: usize) {
        if let Some(of) = self.dos[i].file.take() {
            let _ = self.fs(|f| {
                f.close(of.handle);
                Ok(())
            });
        }
    }

    /// One READ_DATA chunk of up to 512 bytes; more follow while `left`.
    fn read_chunk(&mut self, t: u8, left: u32) {
        let i = usize::from(t);
        let Some(of) = self.dos[i].file.clone() else {
            self.set_status("85,NO FILE OPEN");
            return self.finish(false);
        };
        let want = (left as usize).min(CHUNK);
        let mut buf = vec![0; want];
        match self.fs(|f| f.read(of.handle, &mut buf)) {
            Ok(n) => {
                buf.truncate(n);
                self.set_resp(buf);
                let left = left - n as u32;
                if left > 0 && n == want && n > 0 {
                    self.pending = Pending::ReadFile { t, left };
                    self.finish(true)
                } else {
                    self.finish(false)
                }
            }
            Err(e) => {
                self.set_status(&fs_status(&e));
                self.finish(false)
            }
        }
    }

    /// One READ_DIR packet: attribute byte and name.
    fn read_dir_entry(&mut self, t: u8) {
        let i = usize::from(t);
        let Some((list, pos)) = &mut self.dos[i].dir else {
            return self.finish(false);
        };
        let Some(e) = list.get(*pos) else {
            return self.finish(false);
        };
        let mut p = vec![fat_attrib(&e.stat)];
        p.extend(e.name.chars().map(|c| c as u32 as u8));
        *pos += 1;
        let more = *pos < list.len();
        self.set_resp(p);
        if more {
            self.pending = Pending::ReadDir { t };
        }
        self.finish(more)
    }

    // --- network target -------------------------------------------------

    fn net_command(&mut self, cmd: &[u8]) {
        let Some((&op, p)) = cmd.split_first() else {
            self.set_status("81,INVALID PARAMS");
            return self.finish(false);
        };
        let net = self.host.net.clone();
        let info = || match &net {
            Some(n) => lock(n).info(),
            None => Err(NetError::NoNetwork),
        };
        match op {
            0x01 => {
                self.set_resp(b"ULTIMATE-II NETWORK INTERFACE V1.0".to_vec());
                self.set_status("00,OK");
            }
            0x02 => {
                self.set_resp(vec![1]);
                self.set_status("00,OK");
            }
            0x04 | 0x05 => {
                if p.first().copied().unwrap_or(0) != 0 {
                    self.set_status("82,PARAMETER(S) OUT OF RANGE");
                } else {
                    let i = info().unwrap_or_else(|_| emu_core::host::NetInfo {
                        ip: [0; 4],
                        netmask: [0; 4],
                        gateway: [0; 4],
                        ..Default::default()
                    });
                    self.set_resp(if op == 0x04 {
                        i.mac.to_vec()
                    } else {
                        [i.ip, i.netmask, i.gateway].concat()
                    });
                    self.set_status("00,OK");
                }
            }
            0x06 => self.set_status("00,OK"),
            0x07 | 0x08 => {
                if p.len() < 3 {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                }
                let port = u16::from_le_bytes([p[0], p[1]]);
                let host = name(&p[2..]);
                let kind = if op == 0x07 {
                    SocketKind::Tcp
                } else {
                    SocketKind::Udp
                };
                let r = match &net {
                    Some(n) => lock(n).connect(kind, &host, port),
                    None => Err(NetError::NoNetwork),
                };
                match r {
                    Ok(handle) => return self.poll_connect(handle),
                    Err(e) => self.set_status(&net_status(&e)),
                }
            }
            0x09 => match p.first().and_then(|id| self.sockets.remove(id)) {
                Some(h) => {
                    if let Some(n) = &net {
                        lock(n).close(h);
                    }
                    if let Some(l) = &mut self.listener {
                        if l.socket == p.first().copied() {
                            l.socket = None;
                        }
                    }
                    self.set_status("00,OK");
                }
                None => self.set_status("82,PARAMETER(S) OUT OF RANGE"),
            },
            0x10 => {
                let (Some(&id), Some(&lo), Some(&hi)) = (p.first(), p.get(1), p.get(2)) else {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                };
                let Some(&h) = self.sockets.get(&id) else {
                    self.set_status("82,PARAMETER(S) OUT OF RANGE");
                    return self.finish(false);
                };
                let want = usize::from(u16::from_le_bytes([lo, hi])).min(RESP_MAX - 2);
                let mut buf = vec![0; want];
                let r = match &net {
                    Some(n) => lock(n).recv(h, &mut buf),
                    None => Err(NetError::NoNetwork),
                };
                let n = match r {
                    Ok(Recv::Data(n)) => {
                        self.set_status("00,OK");
                        n
                    }
                    Ok(Recv::Closed) => {
                        self.set_status("01,CONNECTION CLOSED BY HOST");
                        0
                    }
                    Err(e) => {
                        self.set_status(&format!("85,{}", e.to_string().to_uppercase()));
                        0
                    }
                };
                let mut out = (n as u16).to_le_bytes().to_vec();
                out.extend(&buf[..n]);
                self.set_resp(out);
            }
            0x11 => {
                let Some((&id, data)) = p.split_first() else {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                };
                let Some(&h) = self.sockets.get(&id) else {
                    self.set_status("82,PARAMETER(S) OUT OF RANGE");
                    return self.finish(false);
                };
                let r = match &net {
                    Some(n) => lock(n).send(h, data),
                    None => Err(NetError::NoNetwork),
                };
                match r {
                    Ok(n) => {
                        self.set_resp((n as u16).to_le_bytes().to_vec());
                        self.set_status("00,OK");
                    }
                    Err(e) => {
                        self.set_resp(vec![0, 0]);
                        self.set_status(&format!("85,{}", e.to_string().to_uppercase()));
                    }
                }
            }
            // TCP listener (unverified, see the module documentation).
            0x12 => {
                if p.len() < 2 {
                    self.set_status("81,INVALID PARAMS");
                    return self.finish(false);
                }
                self.stop_listener();
                let port = u16::from_le_bytes([p[0], p[1]]);
                let r = match &net {
                    Some(n) => lock(n).listen(port),
                    None => Err(NetError::NoNetwork),
                };
                match r {
                    Ok(handle) => {
                        self.listener = Some(Listener {
                            handle,
                            state: 1,
                            socket: None,
                        });
                        self.set_status("00,OK");
                    }
                    Err(e) => self.set_status(&net_status(&e)),
                }
            }
            0x13 => {
                self.stop_listener();
                self.set_status("00,OK");
            }
            0x14 => {
                self.poll_listener();
                let state = self.listener.map_or(0, |l| l.state);
                self.set_resp(vec![state]);
                self.set_status("00,OK");
            }
            0x15 => {
                self.poll_listener();
                match self.listener.and_then(|l| l.socket) {
                    Some(id) => {
                        self.set_resp(vec![id]);
                        self.set_status("00,OK");
                    }
                    None => self.set_status("02,NOT CONNECTED"),
                }
            }
            _ => self.set_status("21,UNKNOWN COMMAND"),
        }
        self.finish(false)
    }

    fn free_socket_id(&self) -> Option<u8> {
        (1..=u8::MAX).find(|id| !self.sockets.contains_key(id))
    }

    fn poll_connect(&mut self, handle: NetHandle) {
        let r = match &self.host.net {
            Some(n) => lock(n).state(handle),
            None => Err(NetError::NoNetwork),
        };
        match r {
            Ok(ConnState::Connecting) => {
                self.pending = Pending::Connect { handle };
                self.state = State::Busy;
                self.timer = POLL;
                return;
            }
            Ok(ConnState::Connected | ConnState::Closed) => match self.free_socket_id() {
                Some(id) => {
                    self.sockets.insert(id, handle);
                    self.set_resp(vec![id]);
                    self.set_status("00,OK");
                }
                None => {
                    if let Some(n) = &self.host.net {
                        lock(n).close(handle);
                    }
                    self.set_status("85,CONNECTION FAILED: NO FREE SOCKET");
                }
            },
            Ok(ConnState::Failed(e)) | Err(e) => {
                if let Some(n) = &self.host.net {
                    lock(n).close(handle);
                }
                self.set_status(&net_status(&e));
            }
        }
        self.pending = Pending::None;
        self.finish(false)
    }

    fn stop_listener(&mut self) {
        if let Some(l) = self.listener.take() {
            if let Some(n) = &self.host.net {
                lock(n).close(l.handle);
            }
        }
    }

    fn poll_listener(&mut self) {
        let Some(mut l) = self.listener else { return };
        if l.state == 1 {
            let r = match &self.host.net {
                Some(n) => lock(n).accept(l.handle),
                None => Err(NetError::NoNetwork),
            };
            match r {
                Ok(Some(h)) => match self.free_socket_id() {
                    Some(id) => {
                        self.sockets.insert(id, h);
                        l.socket = Some(id);
                        l.state = 2;
                    }
                    None => {
                        if let Some(n) = &self.host.net {
                            lock(n).close(h);
                        }
                    }
                },
                Ok(None) => {}
                Err(_) => l.state = 3,
            }
        }
        self.listener = Some(l);
    }

    // --- control target -------------------------------------------------

    fn control_command(&mut self, cmd: &[u8]) {
        match cmd {
            [0x01, ..] => {
                self.set_resp(b"CONTROL TARGET V1.1".to_vec());
                self.set_status("00,OK");
            }
            [0x28] | [0x28, 0x00, ..] => {
                self.set_resp(b"ULTIMATE 64".to_vec());
                self.set_status("00,OK");
            }
            [] => self.set_status("81,INVALID PARAMS"),
            _ => self.set_status("21,UNKNOWN COMMAND"),
        }
        self.finish(false)
    }

    // --- savestate ------------------------------------------------------

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"UCI0", 1, |w| {
            w.bytes(&self.cmd);
            w.bytes(&self.resp);
            w.u32(self.resp_pos as u32);
            w.bytes(&self.stat);
            w.u32(self.stat_pos as u32);
            w.u8(self.state as u8);
            w.bool(self.error);
            w.bool(self.abort);
            w.bool(self.data_acc);
            w.u32(self.timer);
            match &self.pending {
                Pending::None => w.u8(0),
                Pending::Command => w.u8(1),
                Pending::ReadFile { t, left } => {
                    w.u8(2);
                    w.u8(*t);
                    w.u32(*left);
                }
                Pending::ReadDir { t } => {
                    w.u8(3);
                    w.u8(*t);
                }
                Pending::Connect { handle } => {
                    w.u8(4);
                    w.u32(*handle);
                }
            }
            for d in &self.dos {
                w.str(&d.cwd);
                w.bool(d.file.is_some());
                if let Some(f) = &d.file {
                    w.u32(f.handle);
                    w.str(&f.path);
                    w.bool(f.write);
                }
                w.bool(d.dir.is_some());
                if let Some((list, pos)) = &d.dir {
                    w.u32(list.len() as u32);
                    for e in list {
                        w.str(&e.name);
                        w.u64(e.stat.size);
                        w.bool(e.stat.is_dir);
                        w.bool(e.stat.read_only);
                    }
                    w.u32(*pos as u32);
                }
            }
            w.u32(self.sockets.len() as u32);
            for (id, h) in &self.sockets {
                w.u8(*id);
                w.u32(*h);
            }
            w.bool(self.listener.is_some());
            if let Some(l) = &self.listener {
                w.u32(l.handle);
                w.u8(l.state);
                w.u8(l.socket.unwrap_or(0));
            }
        });
    }

    /// Atomic: on error nothing changes. Host handles open before the load
    /// are closed (see the module documentation).
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"UCI0", 1)?;
        let mut n = Self::new();
        let bad = StateError::Invalid("UCI state");
        n.cmd = c.bytes()?.to_vec();
        n.resp = c.bytes()?.to_vec();
        n.resp_pos = c.u32()? as usize;
        n.stat = c.bytes()?.to_vec();
        n.stat_pos = c.u32()? as usize;
        if n.cmd.len() > CMD_MAX
            || n.resp.len() > RESP_MAX
            || n.stat.len() > STATUS_MAX
            || n.resp_pos > n.resp.len()
            || n.stat_pos > n.stat.len()
        {
            return Err(bad);
        }
        n.state = match c.u8()? {
            0 => State::Idle,
            1 => State::Busy,
            2 => State::DataLast,
            3 => State::DataMore,
            _ => return Err(bad),
        };
        n.error = c.bool()?;
        n.abort = c.bool()?;
        n.data_acc = c.bool()?;
        n.timer = c.u32()?;
        n.pending = match c.u8()? {
            0 => Pending::None,
            1 => Pending::Command,
            2 => Pending::ReadFile {
                t: c.u8()? & 1,
                left: c.u32()?,
            },
            3 => Pending::ReadDir { t: c.u8()? & 1 },
            4 => Pending::Connect { handle: c.u32()? },
            _ => return Err(bad),
        };
        for d in &mut n.dos {
            d.cwd = c.string()?;
            join_path("/", &d.cwd).map_err(|_| StateError::Invalid("UCI directory"))?;
            if c.bool()? {
                d.file = Some(OpenFile {
                    handle: c.u32()?,
                    path: c.string()?,
                    write: c.bool()?,
                });
            }
            if c.bool()? {
                let len = c.u32()? as usize;
                if len > c.remaining() {
                    return Err(StateError::Truncated);
                }
                let mut list = Vec::with_capacity(len);
                for _ in 0..len {
                    list.push(DirEntry {
                        name: c.string()?,
                        stat: FileStat {
                            size: c.u64()?,
                            is_dir: c.bool()?,
                            read_only: c.bool()?,
                            modified: None,
                        },
                    });
                }
                let pos = c.u32()? as usize;
                d.dir = Some((list, pos));
            }
        }
        let count = c.u32()?;
        if count > 255 {
            return Err(bad);
        }
        for _ in 0..count {
            let id = c.u8()?;
            n.sockets.insert(id, c.u32()?);
        }
        if c.bool()? {
            let handle = c.u32()?;
            let state = c.u8()?;
            let socket = c.u8()?;
            n.listener = Some(Listener {
                handle,
                state,
                socket: (socket != 0).then_some(socket),
            });
        }
        self.release_host_handles();
        n.host = std::mem::take(&mut self.host);
        *self = n;
        Ok(())
    }
}

/// COPY_FILE: through the host's read and write.
fn copy_file(f: &mut dyn FileHost, from: &str, to: &str) -> Result<(), FsError> {
    let src = f.open(
        from,
        OpenMode {
            read: true,
            ..Default::default()
        },
    )?;
    let dst = match f.open(
        to,
        OpenMode {
            write: true,
            create_new: true,
            ..Default::default()
        },
    ) {
        Ok(h) => h,
        Err(e) => {
            f.close(src);
            return Err(e);
        }
    };
    let mut buf = vec![0; 4096];
    let r = loop {
        match f.read(src, &mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                if let Err(e) = f.write(dst, &buf[..n]) {
                    break Err(e);
                }
            }
            Err(e) => break Err(e),
        }
    };
    f.close(src);
    f.close(dst);
    r
}

#[cfg(test)]
mod tests;
