//! Host services for emulated devices that reach outside the machine.
//!
//! Some peripherals talk to the world: the Ultimate 64's command interface
//! opens files in a folder and network sockets, the WiC64 fetches web
//! pages. Machines never see host devices (golden rule 10), so those
//! devices get the host only through the traits here, the way media bytes
//! come through [`crate::media::MediaBackend`]:
//!
//! - [`NetBackend`]: TCP/UDP sockets, a TCP listener, asynchronous HTTP
//!   requests (GET/POST, or any verb with headers through
//!   [`NetBackend::http_request`]), network information and the host's clock. Every call
//!   returns at once; nothing ever blocks the emulation thread.
//! - [`FileHost`]: one sandboxed folder. Paths are canonical, `/`-separated
//!   and relative to the folder's root (see [`join_path`]); no path can
//!   leave the root. [`MountFileHost`] hangs further folders into it as
//!   directories of the root (the Ultimate's `/SD`, `/Usb0` …).
//!
//! The frontend implements them (native: `media-backends`; the web
//! frontend offers a [`MemFileHost`] and no network) and hands them to the
//! machine with [`crate::Machine::attach_host`], wrapped in a
//! [`Journal`]: results from the host are not deterministic (golden rule
//! 3), so the journal records each call's result in call order while a
//! recording runs and answers from the record on replay.

mod journal;
mod memfs;
mod mount;

pub use journal::{Journal, JournalMode, ReplayStatus};
pub use memfs::MemFileHost;
pub use mount::MountFileHost;

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

/// A socket or listener of a [`NetBackend`].
pub type NetHandle = u32;
/// An HTTP request in flight.
pub type HttpTicket = u32;
/// An open file of a [`FileHost`].
pub type FileHandle = u32;

/// Why a network call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// The network is switched off, or the frontend has none (web).
    NoNetwork,
    /// Not supported by this host (e.g. `https:` without TLS).
    Unsupported(String),
    /// The host name did not resolve.
    Resolve(String),
    /// Connecting, binding or the request failed.
    Connect(String),
    /// Unknown or closed handle.
    BadHandle,
    /// Any other host error.
    Io(String),
    /// A replayed journal did not match the calls (see [`Journal`]).
    Replay(String),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetError::NoNetwork => f.write_str("no network"),
            NetError::Unsupported(m) => write!(f, "unsupported: {m}"),
            NetError::Resolve(m) => write!(f, "host not found: {m}"),
            NetError::Connect(m) => write!(f, "connection failed: {m}"),
            NetError::BadHandle => f.write_str("bad handle"),
            NetError::Io(m) => write!(f, "network error: {m}"),
            NetError::Replay(m) => write!(f, "replay: {m}"),
        }
    }
}

impl std::error::Error for NetError {}

/// Socket type for [`NetBackend::connect`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketKind {
    Tcp,
    /// A connected UDP socket: datagrams to and from one peer.
    Udp,
}

/// State of a socket opened with [`NetBackend::connect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnState {
    /// Resolving or connecting in the background.
    Connecting,
    Connected,
    /// The peer closed the connection (data received before may still be
    /// read).
    Closed,
    /// Resolving or connecting failed.
    Failed(NetError),
}

/// Result of [`NetBackend::recv`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recv {
    /// `n` bytes were read; 0 = nothing there yet.
    Data(usize),
    /// The peer closed the connection and everything was read.
    Closed,
}

/// A finished HTTP request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code (200 …).
    pub status: u16,
    /// The response's header fields in the order received (names as sent).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// An HTTP request with any verb and header fields
/// ([`NetBackend::http_request`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequest {
    /// "GET", "PUT", "POST", "PATCH", "DELETE", "HEAD", "OPTIONS" …
    pub method: String,
    pub url: String,
    /// Header fields to send. `Host`, `Connection` and `Content-Length` are
    /// the backend's (it frames the message); a field given here replaces
    /// the backend's default of the same name (`User-Agent`, `Accept`).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// The value of header field `name` (case-insensitive), if given.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// The network as the emulated device reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetInfo {
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub mac: [u8; 6],
    /// Name of the (pretend) WLAN.
    pub ssid: String,
    /// Signal strength in dBm.
    pub rssi: i8,
}

impl Default for NetInfo {
    /// Plausible fixed values: a private address, a locally administered MAC.
    fn default() -> Self {
        Self {
            ip: [192, 168, 1, 64],
            netmask: [255, 255, 255, 0],
            gateway: [192, 168, 1, 1],
            mac: [0x02, 0x6d, 0x69, 0x6d, 0x69, 0x63],
            ssid: "mimic".into(),
            rssi: -50,
        }
    }
}

/// The host's wall clock: comes from outside, so it is journalled too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostTime {
    /// Seconds since 1970-01-01 00:00 UTC.
    pub unix_secs: i64,
    /// Local time minus UTC, in seconds.
    pub utc_offset_secs: i32,
}

impl HostTime {
    /// Local calendar time: (year, month 1–12, day 1–31, hour, minute,
    /// second, weekday 0 = Sunday).
    pub fn local(&self) -> (i64, u8, u8, u8, u8, u8, u8) {
        let t = self.unix_secs + i64::from(self.utc_offset_secs);
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        // Civil-from-days (Howard Hinnant's public-domain algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let y = yoe + era * 400 + i64::from(m <= 2);
        let weekday = (days + 4).rem_euclid(7) as u8;
        (
            y,
            m,
            d,
            (secs / 3600) as u8,
            (secs / 60 % 60) as u8,
            (secs % 60) as u8,
            weekday,
        )
    }
}

/// Network access for emulated devices. Every call returns immediately.
pub trait NetBackend: Send {
    /// Start connecting to `host:port`. The handle is valid at once; its
    /// [`NetBackend::state`] tells when the connection stands.
    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError>;

    /// Listen for TCP connections on `port`.
    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError>;

    /// A connection waiting on `listener`, if any (a connected socket).
    fn accept(&mut self, listener: NetHandle) -> Result<Option<NetHandle>, NetError>;

    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError>;

    /// Queue `data`; returns how much was taken (0 while still connecting).
    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError>;

    /// Read what has arrived into `buf`.
    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError>;

    /// Close a socket or listener; unknown handles are ignored.
    fn close(&mut self, h: NetHandle);

    /// Start an HTTP GET of `url`.
    fn http_get(&mut self, url: &str) -> Result<HttpTicket, NetError>;

    /// Start an HTTP POST of `body` to `url`.
    fn http_post(
        &mut self,
        url: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<HttpTicket, NetError>;

    /// Start a request with any verb and header fields. The default
    /// covers what [`NetBackend::http_get`] and [`NetBackend::http_post`]
    /// can express (other verbs and extra fields: unsupported); backends
    /// that speak HTTP themselves implement it fully.
    fn http_request(&mut self, req: &HttpRequest) -> Result<HttpTicket, NetError> {
        let extra = req.headers.iter().any(|(k, _)| {
            !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("content-length")
        });
        match req.method.as_str() {
            _ if extra => Err(NetError::Unsupported("HTTP header fields".into())),
            "GET" if req.body.is_empty() => self.http_get(&req.url),
            "POST" => self.http_post(
                &req.url,
                req.header("content-type").unwrap_or(""),
                &req.body,
            ),
            m => Err(NetError::Unsupported(format!("HTTP {m}"))),
        }
    }

    /// `None` while the request runs; then its result, once (the ticket is
    /// gone afterwards). An unknown ticket: `Some(Err(BadHandle))`.
    fn poll_http(&mut self, t: HttpTicket) -> Option<Result<HttpResponse, NetError>>;

    /// Addresses and WLAN data the device reports.
    fn info(&mut self) -> Result<NetInfo, NetError>;

    /// The host's clock.
    fn time(&mut self) -> HostTime;
}

/// Why a file call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    Exists,
    /// Read-only, or not opened for this access.
    Denied,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    /// The path is malformed or would leave the root.
    InvalidPath,
    BadHandle,
    /// No folder attached.
    Unavailable,
    Io(String),
    /// A replayed journal did not match the calls (see [`Journal`]).
    Replay(String),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::NotFound => f.write_str("not found"),
            FsError::Exists => f.write_str("exists"),
            FsError::Denied => f.write_str("access denied"),
            FsError::NotADirectory => f.write_str("not a directory"),
            FsError::IsADirectory => f.write_str("is a directory"),
            FsError::NotEmpty => f.write_str("directory not empty"),
            FsError::InvalidPath => f.write_str("invalid path"),
            FsError::BadHandle => f.write_str("bad handle"),
            FsError::Unavailable => f.write_str("no folder attached"),
            FsError::Io(m) => write!(f, "{m}"),
            FsError::Replay(m) => write!(f, "replay: {m}"),
        }
    }
}

impl std::error::Error for FsError {}

/// How [`FileHost::open`] opens a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenMode {
    pub read: bool,
    pub write: bool,
    /// Create the file if it is missing.
    pub create: bool,
    /// Fail if the file exists.
    pub create_new: bool,
    /// Empty the file.
    pub truncate: bool,
}

/// What [`FileHost::stat`] reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    pub is_dir: bool,
    pub read_only: bool,
    /// Last modification, seconds since 1970 (UTC), if known.
    pub modified: Option<i64>,
}

/// One entry of [`FileHost::list_dir`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub stat: FileStat,
}

/// A sandboxed folder. Paths are canonical (see [`join_path`]): they start
/// with `/` (the folder's root) and contain no `.`, `..` or empty parts.
/// Implementations reject anything else with [`FsError::InvalidPath`] and
/// never follow a path outside the root.
pub trait FileHost: Send {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError>;
    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError>;
    /// Move to byte `pos` from the start.
    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError>;
    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError>;
    /// Close a file; unknown handles are ignored.
    fn close(&mut self, h: FileHandle);
    fn stat(&mut self, path: &str) -> Result<FileStat, FsError>;
    /// The directory's entries, sorted by name.
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError>;
    fn mkdir(&mut self, path: &str) -> Result<(), FsError>;
    /// Delete a file or an empty directory.
    fn delete(&mut self, path: &str) -> Result<(), FsError>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
}

/// A [`NetBackend`] shared by the devices of a machine.
pub type SharedNet = Arc<Mutex<dyn NetBackend>>;
/// A [`FileHost`] shared by the devices of a machine.
pub type SharedFiles = Arc<Mutex<dyn FileHost>>;

/// The host services a frontend lends a machine
/// ([`crate::Machine::attach_host`]). Cloning shares the services.
#[derive(Clone, Default)]
pub struct HostServices {
    pub net: Option<SharedNet>,
    pub files: Option<SharedFiles>,
}

impl fmt::Debug for HostServices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostServices")
            .field("net", &self.net.is_some())
            .field("files", &self.files.is_some())
            .finish()
    }
}

impl HostServices {
    pub fn new(net: Option<Box<dyn NetBackend>>, files: Option<Box<dyn FileHost>>) -> Self {
        Self {
            net: net.map(|n| Arc::new(Mutex::new(BoxedNet(n))) as SharedNet),
            files: files.map(|f| Arc::new(Mutex::new(BoxedFiles(f))) as SharedFiles),
        }
    }
}

/// Lock a shared service; a poisoned lock (a panicking thread) is still
/// usable, the services keep no invariants across calls.
pub fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct BoxedNet(Box<dyn NetBackend>);

impl NetBackend for BoxedNet {
    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError> {
        self.0.connect(kind, host, port)
    }
    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        self.0.listen(port)
    }
    fn accept(&mut self, l: NetHandle) -> Result<Option<NetHandle>, NetError> {
        self.0.accept(l)
    }
    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError> {
        self.0.state(h)
    }
    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError> {
        self.0.send(h, data)
    }
    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        self.0.recv(h, buf)
    }
    fn close(&mut self, h: NetHandle) {
        self.0.close(h)
    }
    fn http_get(&mut self, url: &str) -> Result<HttpTicket, NetError> {
        self.0.http_get(url)
    }
    fn http_post(&mut self, url: &str, ct: &str, body: &[u8]) -> Result<HttpTicket, NetError> {
        self.0.http_post(url, ct, body)
    }
    fn http_request(&mut self, req: &HttpRequest) -> Result<HttpTicket, NetError> {
        self.0.http_request(req)
    }
    fn poll_http(&mut self, t: HttpTicket) -> Option<Result<HttpResponse, NetError>> {
        self.0.poll_http(t)
    }
    fn info(&mut self) -> Result<NetInfo, NetError> {
        self.0.info()
    }
    fn time(&mut self) -> HostTime {
        self.0.time()
    }
}

struct BoxedFiles(Box<dyn FileHost>);

impl FileHost for BoxedFiles {
    fn open(&mut self, path: &str, mode: OpenMode) -> Result<FileHandle, FsError> {
        self.0.open(path, mode)
    }
    fn read(&mut self, h: FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        self.0.read(h, buf)
    }
    fn write(&mut self, h: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        self.0.write(h, data)
    }
    fn seek(&mut self, h: FileHandle, pos: u64) -> Result<(), FsError> {
        self.0.seek(h, pos)
    }
    fn fstat(&mut self, h: FileHandle) -> Result<FileStat, FsError> {
        self.0.fstat(h)
    }
    fn close(&mut self, h: FileHandle) {
        self.0.close(h)
    }
    fn stat(&mut self, path: &str) -> Result<FileStat, FsError> {
        self.0.stat(path)
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        self.0.list_dir(path)
    }
    fn mkdir(&mut self, path: &str) -> Result<(), FsError> {
        self.0.mkdir(path)
    }
    fn delete(&mut self, path: &str) -> Result<(), FsError> {
        self.0.delete(path)
    }
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        self.0.rename(from, to)
    }
}

/// Resolve `input` against the canonical directory `cwd` into a canonical
/// path: `/` starts at the root, `.` and `..` are resolved, `..` at the
/// root is an error (the sandbox cannot be left). Backslashes, colons
/// (drive letters, alternate streams) and NUL are rejected.
pub fn join_path(cwd: &str, input: &str) -> Result<String, FsError> {
    if input.contains(['\\', ':', '\0']) {
        return Err(FsError::InvalidPath);
    }
    let mut parts: Vec<&str> = if input.starts_with('/') {
        Vec::new()
    } else {
        check_path(cwd)?;
        cwd.split('/').filter(|p| !p.is_empty()).collect()
    };
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or(FsError::InvalidPath)?;
            }
            p => parts.push(p),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Check that `path` is canonical (see [`FileHost`]); returns its parts.
pub fn check_path(path: &str) -> Result<Vec<&str>, FsError> {
    let rest = path.strip_prefix('/').ok_or(FsError::InvalidPath)?;
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = rest.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(['\\', ':', '\0']))
    {
        return Err(FsError::InvalidPath);
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_root() {
        assert_eq!(join_path("/", "a/b").unwrap(), "/a/b");
        assert_eq!(join_path("/a/b", "../c").unwrap(), "/a/c");
        assert_eq!(join_path("/a", "/x/./y/").unwrap(), "/x/y");
        assert_eq!(join_path("/a", "..").unwrap(), "/");
        assert_eq!(join_path("/", ".."), Err(FsError::InvalidPath));
        assert_eq!(join_path("/a", "../../etc"), Err(FsError::InvalidPath));
        assert_eq!(join_path("/", "c:/windows"), Err(FsError::InvalidPath));
        assert_eq!(join_path("/", "a\\..\\..\\b"), Err(FsError::InvalidPath));
        assert!(check_path("/a/b").is_ok());
        for bad in ["a", "/a/../b", "/a//b", "/./a", "/a/"] {
            assert_eq!(check_path(bad), Err(FsError::InvalidPath), "{bad}");
        }
    }

    #[test]
    fn local_time_from_unix_seconds() {
        // 2026-10-25 13:14:15 UTC, a Sunday; one hour ahead locally.
        let t = HostTime {
            unix_secs: 1_792_934_055,
            utc_offset_secs: 3600,
        };
        assert_eq!(t.local(), (2026, 10, 25, 14, 14, 15, 0));
        assert_eq!(HostTime::default().local(), (1970, 1, 1, 0, 0, 0, 4));
    }
}
