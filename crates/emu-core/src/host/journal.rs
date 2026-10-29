//! Journal of host-service results for exact replays (golden rule 3).
//!
//! A live network or folder answers differently from run to run. The
//! [`Journal`] sits between the machine and the frontend's services:
//!
//! - [`JournalMode::Live`]: calls pass through, nothing is kept.
//! - [`JournalMode::Record`]: calls pass through and each call's result
//!   (including received bytes, the clock, HTTP bodies) is appended in call
//!   order. Repeats of the same answer (a socket with nothing to read, an
//!   HTTP request still running) are stored once with a count.
//! - [`JournalMode::Replay`]: calls are answered from a recorded journal in
//!   order, without touching the host. Given the same savestate and input,
//!   the machine makes the same calls in the same order, so the run is
//!   reproduced exactly. A call that does not match the next record puts
//!   the journal into a diverged state: that and every later call fails
//!   with [`NetError::Replay`] / [`FsError::Replay`] (never a panic) and
//!   [`Journal::replay_status`] tells why. When the record is used up, the
//!   journal goes back to live.
//!
//! The debug server records the journal along with the input of a
//! recording (`record_start`/`record_stop`) and installs it for
//! `replay_input`.

use std::sync::{Arc, Mutex};

use super::{
    lock, ConnState, DirEntry, FileHandle, FileHost, FileStat, FsError, HostServices, HostTime,
    HttpRequest, HttpResponse, HttpTicket, NetBackend, NetError, NetHandle, NetInfo, OpenMode,
    Recv, SharedFiles, SharedNet, SocketKind,
};
use crate::savestate::{StateError, StateReader, StateWriter};

/// What the journal does with calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalMode {
    Live,
    Record,
    Replay,
}

/// Progress of a replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayStatus {
    /// Recorded calls not yet replayed.
    pub remaining: u64,
    /// Why the replay stopped matching, if it did.
    pub diverged: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    call: u8,
    data: Vec<u8>,
    repeat: u32,
}

struct State {
    mode: JournalMode,
    net: Option<Box<dyn NetBackend>>,
    files: Option<Box<dyn FileHost>>,
    entries: Vec<Entry>,
    cursor: usize,
    used: u32,
    diverged: Option<String>,
}

/// The journal around a frontend's services; clones share it.
#[derive(Clone)]
pub struct Journal(Arc<Mutex<State>>);

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = lock(&self.0);
        f.debug_struct("Journal")
            .field("mode", &s.mode)
            .field("entries", &s.entries.len())
            .finish()
    }
}

const TAG: [u8; 4] = *b"HJNL";
const VERSION: u16 = 1;

// Call identifiers in the record.
mod call {
    pub const CONNECT: u8 = 1;
    pub const LISTEN: u8 = 2;
    pub const ACCEPT: u8 = 3;
    pub const STATE: u8 = 4;
    pub const SEND: u8 = 5;
    pub const RECV: u8 = 6;
    pub const CLOSE: u8 = 7;
    pub const HTTP_GET: u8 = 8;
    pub const HTTP_POST: u8 = 9;
    pub const POLL_HTTP: u8 = 10;
    pub const INFO: u8 = 11;
    pub const TIME: u8 = 12;
    pub const HTTP_REQUEST: u8 = 13;
    pub const OPEN: u8 = 32;
    pub const READ: u8 = 33;
    pub const WRITE: u8 = 34;
    pub const SEEK: u8 = 35;
    pub const FSTAT: u8 = 36;
    pub const FCLOSE: u8 = 37;
    pub const STAT: u8 = 38;
    pub const LIST: u8 = 39;
    pub const MKDIR: u8 = 40;
    pub const DELETE: u8 = 41;
    pub const RENAME: u8 = 42;
}

impl Journal {
    /// A live journal around the frontend's services (either may be
    /// missing: no network, no folder).
    pub fn new(net: Option<Box<dyn NetBackend>>, files: Option<Box<dyn FileHost>>) -> Self {
        Self(Arc::new(Mutex::new(State {
            mode: JournalMode::Live,
            net,
            files,
            entries: Vec::new(),
            cursor: 0,
            used: 0,
            diverged: None,
        })))
    }

    /// The services to attach to a machine; they go through this journal.
    pub fn services(&self) -> HostServices {
        HostServices {
            net: Some(Arc::new(Mutex::new(JournalNet(self.0.clone()))) as SharedNet),
            files: Some(Arc::new(Mutex::new(JournalFiles(self.0.clone()))) as SharedFiles),
        }
    }

    pub fn mode(&self) -> JournalMode {
        lock(&self.0).mode
    }

    /// Start recording (a replay in progress ends).
    pub fn start_recording(&self) {
        let mut s = lock(&self.0);
        s.mode = JournalMode::Record;
        s.entries.clear();
        s.cursor = 0;
        s.used = 0;
        s.diverged = None;
    }

    /// Stop recording, back to live; returns the record ([`Journal::to_bytes`]
    /// form). Not recording: an empty record.
    pub fn stop_recording(&self) -> Vec<u8> {
        let mut s = lock(&self.0);
        let bytes = if s.mode == JournalMode::Record {
            encode(&s.entries)
        } else {
            encode(&[])
        };
        s.mode = JournalMode::Live;
        s.entries.clear();
        bytes
    }

    /// The record so far, while recording.
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(&lock(&self.0).entries)
    }

    /// Answer the next calls from `record` (from [`Journal::stop_recording`]).
    /// Malformed data is rejected and changes nothing.
    pub fn start_replay(&self, record: &[u8]) -> Result<(), StateError> {
        let entries = decode(record)?;
        let mut s = lock(&self.0);
        s.mode = if entries.is_empty() {
            JournalMode::Live
        } else {
            JournalMode::Replay
        };
        s.entries = entries;
        s.cursor = 0;
        s.used = 0;
        s.diverged = None;
        Ok(())
    }

    pub fn replay_status(&self) -> ReplayStatus {
        let s = lock(&self.0);
        let remaining = if s.mode == JournalMode::Replay {
            s.entries[s.cursor.min(s.entries.len())..]
                .iter()
                .map(|e| u64::from(e.repeat))
                .sum::<u64>()
                .saturating_sub(u64::from(s.used))
        } else {
            0
        };
        ReplayStatus {
            remaining,
            diverged: s.diverged.clone(),
        }
    }
}

fn encode(entries: &[Entry]) -> Vec<u8> {
    let mut w = StateWriter::new();
    w.chunk(TAG, VERSION, |w| {
        w.u32(entries.len() as u32);
        for e in entries {
            w.u8(e.call);
            w.u32(e.repeat);
            w.bytes(&e.data);
        }
    });
    w.into_bytes()
}

fn decode(bytes: &[u8]) -> Result<Vec<Entry>, StateError> {
    let (_, mut c) = StateReader::new(bytes).chunk(TAG, VERSION)?;
    let n = c.u32()? as usize;
    // Each entry takes at least 9 bytes: bound the allocation by the data.
    if n > c.remaining() / 9 {
        return Err(StateError::Truncated);
    }
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let call = c.u8()?;
        let repeat = c.u32()?;
        if repeat == 0 {
            return Err(StateError::Invalid("journal repeat count"));
        }
        entries.push(Entry {
            call,
            repeat,
            data: c.bytes()?.to_vec(),
        });
    }
    Ok(entries)
}

/// A result type that can be journalled.
trait Codec: Sized {
    fn enc(&self, w: &mut StateWriter);
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError>;
    /// The answer when a replay no longer matches.
    fn replay_error(msg: String) -> Self;
}

/// Run `live` (or answer from the record) for call `id`.
fn journalled<T: Codec>(m: &Mutex<State>, id: u8, live: impl FnOnce(&mut State) -> T) -> T {
    let mut s = lock(m);
    match s.mode {
        JournalMode::Live => live(&mut s),
        JournalMode::Record => {
            let result = live(&mut s);
            let mut w = StateWriter::new();
            result.enc(&mut w);
            let data = w.into_bytes();
            match s.entries.last_mut() {
                Some(last) if last.call == id && last.data == data && last.repeat < u32::MAX => {
                    last.repeat += 1
                }
                _ => s.entries.push(Entry {
                    call: id,
                    data,
                    repeat: 1,
                }),
            }
            result
        }
        JournalMode::Replay => {
            if let Some(why) = &s.diverged {
                return T::replay_error(why.clone());
            }
            let Some(entry) = s.entries.get(s.cursor) else {
                // Record used up: the run continues live.
                s.mode = JournalMode::Live;
                s.entries.clear();
                return live(&mut s);
            };
            if entry.call != id {
                let why = format!(
                    "call {id} where the journal has call {} (entry {})",
                    entry.call, s.cursor
                );
                s.diverged = Some(why.clone());
                return T::replay_error(why);
            }
            let decoded = T::dec(&mut StateReader::new(&entry.data));
            let repeat = entry.repeat;
            match decoded {
                Ok(v) => {
                    s.used += 1;
                    if s.used >= repeat {
                        s.cursor += 1;
                        s.used = 0;
                    }
                    v
                }
                Err(e) => {
                    let why = format!("journal entry {}: {e}", s.cursor);
                    s.diverged = Some(why.clone());
                    T::replay_error(why)
                }
            }
        }
    }
}

// --- codecs -------------------------------------------------------------

fn enc_str(w: &mut StateWriter, s: &str) {
    w.str(s);
}

impl Codec for NetError {
    fn enc(&self, w: &mut StateWriter) {
        let (tag, msg) = match self {
            NetError::NoNetwork => (0, ""),
            NetError::Unsupported(m) => (1, m.as_str()),
            NetError::Resolve(m) => (2, m.as_str()),
            NetError::Connect(m) => (3, m.as_str()),
            NetError::BadHandle => (4, ""),
            NetError::Io(m) => (5, m.as_str()),
            NetError::Replay(m) => (6, m.as_str()),
        };
        w.u8(tag);
        enc_str(w, msg);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let tag = r.u8()?;
        let m = r.string()?;
        Ok(match tag {
            0 => NetError::NoNetwork,
            1 => NetError::Unsupported(m),
            2 => NetError::Resolve(m),
            3 => NetError::Connect(m),
            4 => NetError::BadHandle,
            5 => NetError::Io(m),
            6 => NetError::Replay(m),
            _ => return Err(StateError::Invalid("journal network error")),
        })
    }
    fn replay_error(msg: String) -> Self {
        NetError::Replay(msg)
    }
}

impl Codec for FsError {
    fn enc(&self, w: &mut StateWriter) {
        let (tag, msg) = match self {
            FsError::NotFound => (0, ""),
            FsError::Exists => (1, ""),
            FsError::Denied => (2, ""),
            FsError::NotADirectory => (3, ""),
            FsError::IsADirectory => (4, ""),
            FsError::NotEmpty => (5, ""),
            FsError::InvalidPath => (6, ""),
            FsError::BadHandle => (7, ""),
            FsError::Unavailable => (8, ""),
            FsError::Io(m) => (9, m.as_str()),
            FsError::Replay(m) => (10, m.as_str()),
        };
        w.u8(tag);
        enc_str(w, msg);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let tag = r.u8()?;
        let m = r.string()?;
        Ok(match tag {
            0 => FsError::NotFound,
            1 => FsError::Exists,
            2 => FsError::Denied,
            3 => FsError::NotADirectory,
            4 => FsError::IsADirectory,
            5 => FsError::NotEmpty,
            6 => FsError::InvalidPath,
            7 => FsError::BadHandle,
            8 => FsError::Unavailable,
            9 => FsError::Io(m),
            10 => FsError::Replay(m),
            _ => return Err(StateError::Invalid("journal file error")),
        })
    }
    fn replay_error(msg: String) -> Self {
        FsError::Replay(msg)
    }
}

/// Values inside `Ok`.
trait Value: Sized {
    fn enc(&self, w: &mut StateWriter);
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError>;
}

/// Errors inside `Err`.
trait ErrValue: Codec {}
impl ErrValue for NetError {}
impl ErrValue for FsError {}

impl<T: Value, E: ErrValue> Codec for Result<T, E> {
    fn enc(&self, w: &mut StateWriter) {
        match self {
            Ok(v) => {
                w.u8(0);
                v.enc(w);
            }
            Err(e) => {
                w.u8(1);
                e.enc(w);
            }
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        match r.u8()? {
            0 => Ok(Ok(T::dec(r)?)),
            1 => Ok(Err(E::dec(r)?)),
            _ => Err(StateError::Invalid("journal result")),
        }
    }
    fn replay_error(msg: String) -> Self {
        Err(E::replay_error(msg))
    }
}

impl Value for () {
    fn enc(&self, _: &mut StateWriter) {}
    fn dec(_: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(())
    }
}

impl Value for u32 {
    fn enc(&self, w: &mut StateWriter) {
        w.u32(*self);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        r.u32()
    }
}

impl Value for usize {
    fn enc(&self, w: &mut StateWriter) {
        w.u64(*self as u64);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        usize::try_from(r.u64()?).map_err(|_| StateError::Invalid("journal length"))
    }
}

impl Value for Option<u32> {
    fn enc(&self, w: &mut StateWriter) {
        w.bool(self.is_some());
        w.u32(self.unwrap_or(0));
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let some = r.bool()?;
        let v = r.u32()?;
        Ok(some.then_some(v))
    }
}

impl Value for ConnState {
    fn enc(&self, w: &mut StateWriter) {
        match self {
            ConnState::Connecting => w.u8(0),
            ConnState::Connected => w.u8(1),
            ConnState::Closed => w.u8(2),
            ConnState::Failed(e) => {
                w.u8(3);
                Codec::enc(e, w);
            }
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(match r.u8()? {
            0 => ConnState::Connecting,
            1 => ConnState::Connected,
            2 => ConnState::Closed,
            3 => ConnState::Failed(<NetError as Codec>::dec(r)?),
            _ => return Err(StateError::Invalid("journal connection state")),
        })
    }
}

/// A read: the result and the bytes it delivered.
struct Received<R>(R, Vec<u8>);

impl<T: Value, E: ErrValue> Codec for Received<Result<T, E>> {
    fn enc(&self, w: &mut StateWriter) {
        self.0.enc(w);
        w.bytes(&self.1);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let res = <Result<T, E> as Codec>::dec(r)?;
        Ok(Received(res, r.bytes()?.to_vec()))
    }
    fn replay_error(msg: String) -> Self {
        Received(Err(E::replay_error(msg)), Vec::new())
    }
}

impl Value for Recv {
    fn enc(&self, w: &mut StateWriter) {
        match self {
            Recv::Data(n) => {
                w.u8(0);
                w.u64(*n as u64);
            }
            Recv::Closed => w.u8(1),
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(match r.u8()? {
            0 => Recv::Data(usize::try_from(r.u64()?).map_err(|_| StateError::Invalid("n"))?),
            1 => Recv::Closed,
            _ => return Err(StateError::Invalid("journal receive result")),
        })
    }
}

/// The header fields follow the body; records made before they were kept
/// end after the body (no fields).
impl Value for HttpResponse {
    fn enc(&self, w: &mut StateWriter) {
        w.u16(self.status);
        w.bytes(&self.body);
        w.u32(self.headers.len() as u32);
        for (k, v) in &self.headers {
            w.str(k);
            w.str(v);
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let status = r.u16()?;
        let body = r.bytes()?.to_vec();
        let mut headers = Vec::new();
        if r.remaining() > 0 {
            let n = r.u32()? as usize;
            if n > r.remaining() {
                return Err(StateError::Truncated);
            }
            for _ in 0..n {
                headers.push((r.string()?, r.string()?));
            }
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

impl Codec for Option<Result<HttpResponse, NetError>> {
    fn enc(&self, w: &mut StateWriter) {
        w.bool(self.is_some());
        if let Some(res) = self {
            res.enc(w);
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(if r.bool()? {
            Some(<Result<HttpResponse, NetError> as Codec>::dec(r)?)
        } else {
            None
        })
    }
    fn replay_error(msg: String) -> Self {
        Some(Err(NetError::Replay(msg)))
    }
}

impl Value for NetInfo {
    fn enc(&self, w: &mut StateWriter) {
        w.raw(&self.ip);
        w.raw(&self.netmask);
        w.raw(&self.gateway);
        w.raw(&self.mac);
        w.str(&self.ssid);
        w.u8(self.rssi as u8);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let mut i = NetInfo::default();
        r.raw_into(&mut i.ip)?;
        r.raw_into(&mut i.netmask)?;
        r.raw_into(&mut i.gateway)?;
        r.raw_into(&mut i.mac)?;
        i.ssid = r.string()?;
        i.rssi = r.u8()? as i8;
        Ok(i)
    }
}

impl Codec for HostTime {
    fn enc(&self, w: &mut StateWriter) {
        w.u64(self.unix_secs as u64);
        w.u32(self.utc_offset_secs as u32);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(HostTime {
            unix_secs: r.u64()? as i64,
            utc_offset_secs: r.u32()? as i32,
        })
    }
    fn replay_error(_: String) -> Self {
        HostTime::default()
    }
}

/// `close` has no result; journalled for the call order.
struct Done;

impl Codec for Done {
    fn enc(&self, _: &mut StateWriter) {}
    fn dec(_: &mut StateReader<'_>) -> Result<Self, StateError> {
        Ok(Done)
    }
    fn replay_error(_: String) -> Self {
        Done
    }
}

impl Value for FileStat {
    fn enc(&self, w: &mut StateWriter) {
        w.u64(self.size);
        w.bool(self.is_dir);
        w.bool(self.read_only);
        w.bool(self.modified.is_some());
        w.u64(self.modified.unwrap_or(0) as u64);
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let size = r.u64()?;
        let is_dir = r.bool()?;
        let read_only = r.bool()?;
        let has = r.bool()?;
        let t = r.u64()? as i64;
        Ok(FileStat {
            size,
            is_dir,
            read_only,
            modified: has.then_some(t),
        })
    }
}

impl Value for Vec<DirEntry> {
    fn enc(&self, w: &mut StateWriter) {
        w.u32(self.len() as u32);
        for e in self {
            w.str(&e.name);
            e.stat.enc(w);
        }
    }
    fn dec(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let n = r.u32()? as usize;
        if n > r.remaining() {
            return Err(StateError::Truncated);
        }
        (0..n)
            .map(|_| {
                Ok(DirEntry {
                    name: r.string()?,
                    stat: FileStat::dec(r)?,
                })
            })
            .collect()
    }
}

// --- proxies ------------------------------------------------------------

struct JournalNet(Arc<Mutex<State>>);

fn with_net<T>(
    s: &mut State,
    f: impl FnOnce(&mut dyn NetBackend) -> Result<T, NetError>,
) -> Result<T, NetError> {
    match s.net.as_deref_mut() {
        Some(n) => f(n),
        None => Err(NetError::NoNetwork),
    }
}

impl NetBackend for JournalNet {
    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError> {
        journalled(&self.0, call::CONNECT, |s| {
            with_net(s, |n| n.connect(kind, host, port))
        })
    }

    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        journalled(&self.0, call::LISTEN, |s| with_net(s, |n| n.listen(port)))
    }

    fn accept(&mut self, l: NetHandle) -> Result<Option<NetHandle>, NetError> {
        journalled(&self.0, call::ACCEPT, |s| with_net(s, |n| n.accept(l)))
    }

    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError> {
        journalled(&self.0, call::STATE, |s| with_net(s, |n| n.state(h)))
    }

    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError> {
        journalled(&self.0, call::SEND, |s| with_net(s, |n| n.send(h, data)))
    }

    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        let len = buf.len();
        let Received(res, data) = journalled(&self.0, call::RECV, |s| {
            let mut tmp = vec![0; len];
            let res = with_net(s, |n| n.recv(h, &mut tmp));
            let got = match res {
                Ok(Recv::Data(k)) => k.min(len),
                _ => 0,
            };
            tmp.truncate(got);
            Received(res, tmp)
        });
        let k = data.len().min(len);
        buf[..k].copy_from_slice(&data[..k]);
        match res {
            Ok(Recv::Data(_)) => Ok(Recv::Data(k)),
            other => other,
        }
    }

    fn close(&mut self, h: NetHandle) {
        journalled(&self.0, call::CLOSE, |s| {
            if let Some(n) = s.net.as_deref_mut() {
                n.close(h);
            }
            Done
        });
    }

    fn http_get(&mut self, url: &str) -> Result<HttpTicket, NetError> {
        journalled(&self.0, call::HTTP_GET, |s| {
            with_net(s, |n| n.http_get(url))
        })
    }

    fn http_post(&mut self, url: &str, ct: &str, body: &[u8]) -> Result<HttpTicket, NetError> {
        journalled(&self.0, call::HTTP_POST, |s| {
            with_net(s, |n| n.http_post(url, ct, body))
        })
    }

    fn http_request(&mut self, req: &HttpRequest) -> Result<HttpTicket, NetError> {
        journalled(&self.0, call::HTTP_REQUEST, |s| {
            with_net(s, |n| n.http_request(req))
        })
    }

    fn poll_http(&mut self, t: HttpTicket) -> Option<Result<HttpResponse, NetError>> {
        journalled(&self.0, call::POLL_HTTP, |s| match s.net.as_deref_mut() {
            Some(n) => n.poll_http(t),
            None => Some(Err(NetError::NoNetwork)),
        })
    }

    fn info(&mut self) -> Result<NetInfo, NetError> {
        journalled(&self.0, call::INFO, |s| with_net(s, |n| n.info()))
    }

    fn time(&mut self) -> HostTime {
        journalled(&self.0, call::TIME, |s| {
            s.net.as_deref_mut().map(|n| n.time()).unwrap_or_default()
        })
    }
}

struct JournalFiles(Arc<Mutex<State>>);

fn with_files<T>(
    s: &mut State,
    f: impl FnOnce(&mut dyn FileHost) -> Result<T, FsError>,
) -> Result<T, FsError> {
    match s.files.as_deref_mut() {
        Some(h) => f(h),
        None => Err(FsError::Unavailable),
    }
}

impl FileHost for JournalFiles {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        journalled(&self.0, call::OPEN, |s| {
            with_files(s, |f| f.open(path, mode))
        })
    }

    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        let len = buf.len();
        let Received(res, data) = journalled(&self.0, call::READ, |s| {
            let mut tmp = vec![0; len];
            let res = with_files(s, |f| f.read(h, &mut tmp));
            tmp.truncate(*res.as_ref().unwrap_or(&0).min(&len));
            Received(res, tmp)
        });
        let k = data.len().min(len);
        buf[..k].copy_from_slice(&data[..k]);
        res.map(|_| k)
    }

    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        journalled(&self.0, call::WRITE, |s| {
            with_files(s, |f| f.write(h, data))
        })
    }

    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        journalled(&self.0, call::SEEK, |s| with_files(s, |f| f.seek(h, pos)))
    }

    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        journalled(&self.0, call::FSTAT, |s| with_files(s, |f| f.fstat(h)))
    }

    fn close(&mut self, h: FileHandle) {
        journalled(&self.0, call::FCLOSE, |s| {
            if let Some(f) = s.files.as_deref_mut() {
                f.close(h);
            }
            Done
        });
    }

    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        journalled(&self.0, call::STAT, |s| with_files(s, |f| f.stat(path)))
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        journalled(&self.0, call::LIST, |s| with_files(s, |f| f.list_dir(path)))
    }

    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        journalled(&self.0, call::MKDIR, |s| with_files(s, |f| f.mkdir(path)))
    }

    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        journalled(&self.0, call::DELETE, |s| with_files(s, |f| f.delete(path)))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        journalled(&self.0, call::RENAME, |s| {
            with_files(s, |f| f.rename(from, to))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::MemFileHost;

    /// A network whose answers change with every call, like a real one.
    struct Counter(u32);

    impl NetBackend for Counter {
        fn connect(&mut self, _: SocketKind, _: &str, _: u16) -> Result<NetHandle, NetError> {
            self.0 += 1;
            Ok(self.0)
        }
        fn listen(&mut self, _: u16) -> Result<NetHandle, NetError> {
            Err(NetError::Connect("in use".into()))
        }
        fn accept(&mut self, _: NetHandle) -> Result<Option<NetHandle>, NetError> {
            Ok(None)
        }
        fn state(&mut self, _: NetHandle) -> Result<ConnState, NetError> {
            Ok(ConnState::Connected)
        }
        fn send(&mut self, _: NetHandle, d: &[u8]) -> Result<usize, NetError> {
            Ok(d.len())
        }
        fn recv(&mut self, _: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
            self.0 += 1;
            if self.0.is_multiple_of(3) {
                return Ok(Recv::Data(0));
            }
            buf[0] = self.0 as u8;
            Ok(Recv::Data(1))
        }
        fn close(&mut self, _: NetHandle) {}
        fn http_get(&mut self, _: &str) -> Result<HttpTicket, NetError> {
            Ok(7)
        }
        fn http_post(&mut self, _: &str, _: &str, _: &[u8]) -> Result<HttpTicket, NetError> {
            Err(NetError::Unsupported("post".into()))
        }
        fn poll_http(&mut self, _: HttpTicket) -> Option<Result<HttpResponse, NetError>> {
            self.0 += 1;
            (self.0 > 20).then(|| {
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![("Content-Type".into(), "text/plain".into())],
                    body: b"hello".to_vec(),
                })
            })
        }
        fn info(&mut self) -> Result<NetInfo, NetError> {
            Ok(NetInfo::default())
        }
        fn time(&mut self) -> HostTime {
            self.0 += 1;
            HostTime {
                unix_secs: i64::from(self.0) * 1000,
                utc_offset_secs: 3600,
            }
        }
    }

    fn session(net: &mut dyn NetBackend, files: &mut dyn FileHost) -> Vec<String> {
        let mut log = Vec::new();
        let h = net.connect(SocketKind::Tcp, "example.org", 80).unwrap();
        log.push(format!("{h} {:?}", net.state(h)));
        for _ in 0..5 {
            let mut buf = [0u8; 4];
            let r = net.recv(h, &mut buf);
            log.push(format!("{r:?} {buf:?}"));
        }
        let t = net.http_get("http://x/").unwrap();
        loop {
            if let Some(r) = net.poll_http(t) {
                log.push(format!("{r:?}"));
                break;
            }
        }
        let put = HttpRequest {
            method: "PUT".into(),
            url: "http://x/".into(),
            ..Default::default()
        };
        log.push(format!("{:?} {:?}", net.http_request(&put), net.time()));
        log.push(format!("{:?} {:?}", net.time(), net.listen(1)));
        net.close(h);
        let f = files
            .open(
                "/a.txt",
                OpenMode {
                    write: true,
                    read: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        files.write(f, b"data").unwrap();
        files.seek(f, 1).unwrap();
        let mut buf = [0u8; 8];
        let n = files.read(f, &mut buf).unwrap();
        log.push(format!("{n} {buf:?} {:?}", files.list_dir("/")));
        files.close(f);
        log
    }

    #[test]
    fn replay_reproduces_a_recorded_session_without_the_host() {
        let journal = Journal::new(
            Some(Box::new(Counter(0))),
            Some(Box::new(MemFileHost::new())),
        );
        let services = journal.services();
        let (net, files) = (services.net.unwrap(), services.files.unwrap());
        journal.start_recording();
        let first = session(&mut *lock(&net), &mut *lock(&files));
        let record = journal.stop_recording();
        // The polls that returned "still running" are stored once.
        assert!(record.len() < 600, "{}", record.len());

        // Live again: the counter has moved on, the answers differ.
        let live = session(&mut *lock(&net), &mut *lock(&files));
        assert_ne!(live, first);

        journal.start_replay(&record).unwrap();
        assert_eq!(journal.mode(), JournalMode::Replay);
        let again = session(&mut *lock(&net), &mut *lock(&files));
        assert_eq!(again, first);
        assert_eq!(journal.replay_status(), ReplayStatus::default());
        // Used up: live again.
        lock(&net).info().unwrap();
        assert_eq!(journal.mode(), JournalMode::Live);
    }

    #[test]
    fn a_mismatching_call_diverges_without_panicking() {
        let journal = Journal::new(Some(Box::new(Counter(0))), None);
        let net = journal.services().net.unwrap();
        journal.start_recording();
        lock(&net).time();
        let record = journal.stop_recording();
        journal.start_replay(&record).unwrap();
        assert!(matches!(lock(&net).info(), Err(NetError::Replay(_))));
        assert!(matches!(
            lock(&net).connect(SocketKind::Tcp, "a", 1),
            Err(NetError::Replay(_))
        ));
        assert!(journal.replay_status().diverged.is_some());
        // Files without a folder.
        let files = journal.services().files.unwrap();
        assert!(matches!(lock(&files).stat("/"), Err(FsError::Replay(_))));
    }

    #[test]
    fn corrupt_records_are_rejected() {
        let journal = Journal::new(None, None);
        assert!(journal.start_replay(&[1, 2, 3]).is_err());
        let mut w = StateWriter::new();
        w.chunk(TAG, VERSION, |w| w.u32(u32::MAX));
        assert!(journal.start_replay(&w.into_bytes()).is_err());
        assert_eq!(journal.mode(), JournalMode::Live);
        // No services: the devices see "no network" / "no folder".
        let s = journal.services();
        assert_eq!(
            lock(s.net.as_ref().unwrap()).info(),
            Err(NetError::NoNetwork)
        );
        assert_eq!(
            lock(s.files.as_ref().unwrap()).stat("/"),
            Err(FsError::Unavailable)
        );
    }
}
