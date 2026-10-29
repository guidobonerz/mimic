//! The host's network for emulated devices ([`emu_core::host::NetBackend`]):
//! the Ultimate command interface's sockets and the WiC64's HTTP and TCP.
//!
//! Everything the emulation thread calls returns at once: name lookups,
//! connects and HTTP requests run on short-lived worker threads, sockets
//! are non-blocking. HTTP is a small HTTP/1.1 client over TCP (any verb,
//! the caller's header fields, `Connection: close`, chunked or sized
//! bodies, up to five redirects; message syntax per RFC 9112, redirect
//! rules per RFC 9110 15.4); `https:` is reported as unsupported (no TLS
//! crate in the workspace). TCP listeners bind to the loopback interface only, so a
//! guest program cannot open a port to the network by itself.
//!
//! The clock is the host's UTC time; the local offset comes from the
//! environment variable `EMU_UTC_OFFSET` (`+02:00`, `-5`, or seconds),
//! else 0 (the standard library knows no time zones).
//!
//! Not for `wasm32`: browsers have no raw sockets (the web frontend offers
//! no network).

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use emu_core::host::{
    lock, ConnState, HostTime, HttpRequest, HttpResponse, HttpTicket, NetBackend, NetError,
    NetHandle, NetInfo, Recv, SocketKind,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest HTTP response kept.
const MAX_BODY: usize = 64 << 20;
/// Unpolled HTTP results kept at most.
const MAX_REQUESTS: usize = 64;
const USER_AGENT: &str = "WiC64/2.1.0 (mimic emulator)";

enum Conn {
    Tcp(TcpStream),
    Udp(UdpSocket),
}

type Slot<T> = Arc<Mutex<Option<Result<T, NetError>>>>;

enum Sock {
    Pending(Slot<Conn>),
    Open { conn: Conn, eof: bool },
    Failed(NetError),
}

/// Real sockets through `std::net`. Constructed disabled (`--net off`),
/// every network call fails with [`NetError::NoNetwork`]; the clock still
/// works.
pub struct StdNet {
    enabled: bool,
    sockets: BTreeMap<NetHandle, Sock>,
    listeners: BTreeMap<NetHandle, TcpListener>,
    http: BTreeMap<HttpTicket, Slot<HttpResponse>>,
    next: u32,
    info: NetInfo,
    utc_offset: i32,
}

impl StdNet {
    pub fn new(enabled: bool) -> Self {
        let mut info = NetInfo::default();
        if enabled {
            if let Some(ip) = local_ipv4() {
                info.ip = ip;
                info.gateway = [ip[0], ip[1], ip[2], 1];
            }
        }
        Self {
            enabled,
            sockets: BTreeMap::new(),
            listeners: BTreeMap::new(),
            http: BTreeMap::new(),
            next: 1,
            info,
            utc_offset: std::env::var("EMU_UTC_OFFSET")
                .ok()
                .and_then(|s| parse_offset(&s))
                .unwrap_or(0),
        }
    }

    fn handle(&mut self) -> u32 {
        let h = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        h
    }

    fn enabled(&self) -> Result<(), NetError> {
        if self.enabled {
            Ok(())
        } else {
            Err(NetError::NoNetwork)
        }
    }

    fn settle(&mut self, h: NetHandle) -> Result<&mut Sock, NetError> {
        let s = self.sockets.get_mut(&h).ok_or(NetError::BadHandle)?;
        if let Sock::Pending(slot) = s {
            let done = lock(slot).take();
            match done {
                Some(Ok(conn)) => *s = Sock::Open { conn, eof: false },
                Some(Err(e)) => *s = Sock::Failed(e),
                None => {}
            }
        }
        Ok(s)
    }

    fn request(&mut self, req: HttpRequest) -> Result<HttpTicket, NetError> {
        self.enabled()?;
        // Fail early on what the worker would refuse anyway.
        parse_url(&req.url)?;
        if req.method.is_empty() || !req.method.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(NetError::Unsupported(format!(
                "HTTP method '{}'",
                req.method
            )));
        }
        let slot: Slot<HttpResponse> = Arc::new(Mutex::new(None));
        let worker = slot.clone();
        std::thread::Builder::new()
            .name("mimic-http".into())
            .spawn(move || {
                let r = http(&req, 0);
                *lock(&worker) = Some(r);
            })
            .map_err(|e| NetError::Io(e.to_string()))?;
        while self.http.len() >= MAX_REQUESTS {
            self.http.pop_first();
        }
        let t = self.handle();
        self.http.insert(t, slot);
        Ok(t)
    }
}

impl NetBackend for StdNet {
    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError> {
        self.enabled()?;
        let slot: Slot<Conn> = Arc::new(Mutex::new(None));
        let worker = slot.clone();
        let host = host.to_string();
        std::thread::Builder::new()
            .name("mimic-connect".into())
            .spawn(move || {
                let r = open(kind, &host, port);
                *lock(&worker) = Some(r);
            })
            .map_err(|e| NetError::Io(e.to_string()))?;
        let h = self.handle();
        self.sockets.insert(h, Sock::Pending(slot));
        Ok(h)
    }

    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        self.enabled()?;
        let l = TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| NetError::Connect(format!("port {port}: {e}")))?;
        l.set_nonblocking(true)
            .map_err(|e| NetError::Io(e.to_string()))?;
        let h = self.handle();
        self.listeners.insert(h, l);
        Ok(h)
    }

    fn accept(&mut self, listener: NetHandle) -> Result<Option<NetHandle>, NetError> {
        let l = self.listeners.get(&listener).ok_or(NetError::BadHandle)?;
        match l.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(true)
                    .map_err(|e| NetError::Io(e.to_string()))?;
                let _ = stream.set_nodelay(true);
                let h = self.handle();
                self.sockets.insert(
                    h,
                    Sock::Open {
                        conn: Conn::Tcp(stream),
                        eof: false,
                    },
                );
                Ok(Some(h))
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(NetError::Io(e.to_string())),
        }
    }

    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError> {
        Ok(match self.settle(h)? {
            Sock::Pending(_) => ConnState::Connecting,
            Sock::Open { eof: true, .. } => ConnState::Closed,
            Sock::Open { .. } => ConnState::Connected,
            Sock::Failed(e) => ConnState::Failed(e.clone()),
        })
    }

    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError> {
        let r = match self.settle(h)? {
            Sock::Pending(_) => return Ok(0),
            Sock::Failed(e) => return Err(e.clone()),
            Sock::Open {
                conn: Conn::Tcp(s), ..
            } => s.write(data),
            Sock::Open {
                conn: Conn::Udp(s), ..
            } => s.send(data),
        };
        match r {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(NetError::Io(e.to_string())),
        }
    }

    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        let (r, tcp) = match self.settle(h)? {
            Sock::Pending(_) => return Ok(Recv::Data(0)),
            Sock::Failed(e) => return Err(e.clone()),
            Sock::Open { eof: true, .. } => return Ok(Recv::Closed),
            Sock::Open {
                conn: Conn::Tcp(s), ..
            } => (s.read(buf), true),
            Sock::Open {
                conn: Conn::Udp(s), ..
            } => (s.recv(buf), false),
        };
        match r {
            Ok(0) if tcp && !buf.is_empty() => {
                if let Some(Sock::Open { eof, .. }) = self.sockets.get_mut(&h) {
                    *eof = true;
                }
                Ok(Recv::Closed)
            }
            Ok(n) => Ok(Recv::Data(n)),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                Ok(Recv::Data(0))
            }
            // A UDP peer that is not listening reports itself on Windows.
            Err(e) if !tcp && e.kind() == ErrorKind::ConnectionReset => Ok(Recv::Data(0)),
            Err(e) => Err(NetError::Io(e.to_string())),
        }
    }

    fn close(&mut self, h: NetHandle) {
        self.sockets.remove(&h);
        self.listeners.remove(&h);
    }

    fn http_get(&mut self, url: &str) -> Result<HttpTicket, NetError> {
        self.request(HttpRequest {
            method: "GET".into(),
            url: url.into(),
            ..Default::default()
        })
    }

    fn http_post(&mut self, url: &str, ct: &str, body: &[u8]) -> Result<HttpTicket, NetError> {
        let ct = if ct.is_empty() {
            "application/octet-stream"
        } else {
            ct
        };
        self.request(HttpRequest {
            method: "POST".into(),
            url: url.into(),
            headers: vec![("Content-Type".into(), ct.into())],
            body: body.to_vec(),
        })
    }

    fn http_request(&mut self, req: &HttpRequest) -> Result<HttpTicket, NetError> {
        self.request(req.clone())
    }

    fn poll_http(&mut self, t: HttpTicket) -> Option<Result<HttpResponse, NetError>> {
        let Some(slot) = self.http.get(&t) else {
            return Some(Err(NetError::BadHandle));
        };
        let done = lock(slot).take();
        if done.is_some() {
            self.http.remove(&t);
        }
        done
    }

    fn info(&mut self) -> Result<NetInfo, NetError> {
        self.enabled()?;
        Ok(self.info.clone())
    }

    fn time(&mut self) -> HostTime {
        let unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        HostTime {
            unix_secs,
            utc_offset_secs: self.utc_offset,
        }
    }
}

/// `+02:00`, `-5`, `3600` → seconds.
fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Some((h, m)) = s.split_once(':') {
        let sign = if h.starts_with('-') { -1 } else { 1 };
        let h: i32 = h.trim_start_matches(['+', '-']).parse().ok()?;
        let m: i32 = m.parse().ok()?;
        return Some(sign * (h * 3600 + m * 60));
    }
    let v: i32 = s.parse().ok()?;
    Some(if v.abs() <= 14 { v * 3600 } else { v })
}

/// The address the host would use for outgoing traffic (no packet is
/// sent: connecting a UDP socket only picks the route).
fn local_ipv4() -> Option<[u8; 4]> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_unspecified() => Some(a.ip().octets()),
        _ => None,
    }
}

fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, NetError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| NetError::Resolve(host.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(NetError::Resolve(host.to_string()));
    }
    Ok(addrs)
}

/// On a worker thread: resolve and connect.
fn open(kind: SocketKind, host: &str, port: u16) -> Result<Conn, NetError> {
    let addrs = resolve(host, port)?;
    let mut last = NetError::Connect(format!("{host}:{port}"));
    for addr in addrs {
        match kind {
            SocketKind::Tcp => match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(s) => {
                    s.set_nonblocking(true)
                        .map_err(|e| NetError::Io(e.to_string()))?;
                    let _ = s.set_nodelay(true);
                    return Ok(Conn::Tcp(s));
                }
                Err(e) => last = NetError::Connect(format!("{host}:{port}: {e}")),
            },
            SocketKind::Udp => {
                let local = if addr.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                let r = UdpSocket::bind(local).and_then(|s| {
                    s.connect(addr)?;
                    s.set_nonblocking(true)?;
                    Ok(s)
                });
                match r {
                    Ok(s) => return Ok(Conn::Udp(s)),
                    Err(e) => last = NetError::Connect(format!("{host}:{port}: {e}")),
                }
            }
        }
    }
    Err(last)
}

/// `http://host[:port]/path` → (host, port, host header, path).
fn parse_url(url: &str) -> Result<(String, u16, String, String), NetError> {
    let lower = url.to_ascii_lowercase();
    let rest = if lower.starts_with("http://") {
        &url[7..]
    } else if lower.starts_with("https://") {
        return Err(NetError::Unsupported("https (no TLS in this build)".into()));
    } else {
        return Err(NetError::Unsupported(format!("URL scheme of '{url}'")));
    };
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(split);
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (h, p) = v6
                .split_once(']')
                .ok_or(NetError::Resolve(authority.into()))?;
            (
                h.to_string(),
                p.strip_prefix(':').and_then(|p| p.parse().ok()),
            )
        }
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()),
            None => (authority.to_string(), None),
        },
    };
    if host.is_empty() {
        return Err(NetError::Resolve(url.into()));
    }
    Ok((host, port.unwrap_or(80), authority.to_string(), path))
}

/// Header fields the client frames itself; the caller's are dropped.
const FRAMING: [&str; 4] = ["host", "connection", "content-length", "transfer-encoding"];

/// The request head: the caller's fields replace the defaults of the same
/// name; a body always gets its length.
fn request_head(req: &HttpRequest, authority: &str, path: &str) -> String {
    let mut head = format!("{} {path} HTTP/1.1\r\nHost: {authority}\r\n", req.method);
    for (k, v) in [("User-Agent", USER_AGENT), ("Accept", "*/*")] {
        if req.header(k).is_none() {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    let clean = |s: &str| s.replace(['\r', '\n'], "");
    for (k, v) in &req.headers {
        if !FRAMING.iter().any(|f| k.eq_ignore_ascii_case(f)) && !k.trim().is_empty() {
            head.push_str(&format!("{}: {}\r\n", clean(k).trim(), clean(v).trim()));
        }
    }
    head.push_str("Connection: close\r\n");
    if !req.body.is_empty() || matches!(req.method.as_str(), "POST" | "PUT" | "PATCH") {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    head
}

/// On a worker thread: one HTTP/1.1 request, following redirects.
fn http(req: &HttpRequest, depth: u8) -> Result<HttpResponse, NetError> {
    let (host, port, authority, path) = parse_url(&req.url)?;
    let addrs = resolve(&host, port)?;
    let mut stream = addrs
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
        .ok_or_else(|| NetError::Connect(format!("{host}:{port}")))?;
    let io = |e: std::io::Error| NetError::Io(e.to_string());
    stream.set_read_timeout(Some(HTTP_TIMEOUT)).map_err(io)?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT)).map_err(io)?;
    let mut message = request_head(req, &authority, &path).into_bytes();
    message.extend_from_slice(&req.body);
    stream.write_all(&message).map_err(io)?;
    let mut raw = Vec::new();
    stream
        .take(MAX_BODY as u64 + 65536)
        .read_to_end(&mut raw)
        .map_err(io)?;
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| NetError::Io("malformed HTTP response".into()))?;
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| NetError::Io("malformed HTTP status line".into()))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let field = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    if matches!(status, 301 | 302 | 303 | 307 | 308) && depth < 5 {
        if let Some(loc) = field("location") {
            let url = if loc.starts_with('/') {
                format!("http://{authority}{loc}")
            } else {
                loc.to_string()
            };
            // 303, and 301/302 after a POST, continue as GET without body.
            let as_get = status == 303 || (matches!(status, 301 | 302) && req.method == "POST");
            let next = if as_get {
                HttpRequest {
                    method: if req.method == "HEAD" { "HEAD" } else { "GET" }.into(),
                    url,
                    headers: req
                        .headers
                        .iter()
                        .filter(|(k, _)| !k.eq_ignore_ascii_case("content-type"))
                        .cloned()
                        .collect(),
                    body: Vec::new(),
                }
            } else {
                HttpRequest { url, ..req.clone() }
            };
            return http(&next, depth + 1);
        }
    }
    let rest = &raw[head_end + 4..];
    let body = if req.method == "HEAD" || matches!(status, 100..=199 | 204 | 304) {
        Vec::new()
    } else if field("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked"))
    {
        dechunk(rest)?
    } else if let Some(n) = field("content-length").and_then(|n| n.parse().ok()) {
        rest[..rest.len().min(n)].to_vec()
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse {
        status,
        body: body[..body.len().min(MAX_BODY)].to_vec(),
        headers,
    })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, NetError> {
    let bad = || NetError::Io("malformed chunked body".into());
    let mut out = Vec::new();
    loop {
        let eol = data.windows(2).position(|w| w == b"\r\n").ok_or_else(bad)?;
        let size_text = String::from_utf8_lossy(&data[..eol]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| bad())?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        let chunk = data.get(..size).ok_or_else(bad)?;
        out.extend_from_slice(chunk);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_http(net: &mut StdNet, t: HttpTicket) -> Result<HttpResponse, NetError> {
        for _ in 0..500 {
            if let Some(r) = net.poll_http(t) {
                return r;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("HTTP request did not finish");
    }

    /// A one-shot HTTP server on the loopback interface.
    fn serve(response: &'static [u8]) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(response);
                // Close without unread data (Windows answers that with RST).
                let _ = s.shutdown(std::net::Shutdown::Write);
                let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
            }
        });
        port
    }

    #[test]
    fn urls_and_offsets() {
        assert_eq!(
            parse_url("http://example.org:8080/a?b=1").unwrap(),
            (
                "example.org".into(),
                8080,
                "example.org:8080".into(),
                "/a?b=1".into()
            )
        );
        assert_eq!(parse_url("HTTP://x.y").unwrap().3, "/");
        assert_eq!(parse_url("http://x.y?q").unwrap().3, "/?q");
        assert!(matches!(
            parse_url("https://x.y/"),
            Err(NetError::Unsupported(_))
        ));
        assert!(matches!(
            parse_url("ftp://x.y/"),
            Err(NetError::Unsupported(_))
        ));
        assert_eq!(parse_offset("+02:00"), Some(7200));
        assert_eq!(parse_offset("-5"), Some(-18000));
        assert_eq!(parse_offset("3600"), Some(3600));
        assert_eq!(
            dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n").unwrap(),
            b"abcde"
        );
    }

    #[test]
    fn disabled_network_refuses_everything_but_the_clock() {
        let mut net = StdNet::new(false);
        assert_eq!(net.http_get("http://127.0.0.1/"), Err(NetError::NoNetwork));
        assert_eq!(
            net.connect(SocketKind::Tcp, "127.0.0.1", 1),
            Err(NetError::NoNetwork)
        );
        assert_eq!(net.info(), Err(NetError::NoNetwork));
        assert!(net.time().unix_secs > 1_700_000_000);
    }

    #[test]
    fn http_get_from_a_local_server() {
        let port =
            serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n");
        let mut net = StdNet::new(true);
        let t = net.http_get(&format!("http://127.0.0.1:{port}/x")).unwrap();
        let r = wait_http(&mut net, t).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
        assert_eq!(net.poll_http(t), Some(Err(NetError::BadHandle)));
    }

    #[test]
    fn requests_carry_the_callers_verb_and_fields() {
        let req = HttpRequest {
            method: "PUT".into(),
            url: "http://h/".into(),
            headers: vec![
                ("Accept".into(), "application/json".into()),
                ("X-Key".into(), "a\r\nb".into()),
                ("Content-Length".into(), "999".into()),
            ],
            body: b"{}".to_vec(),
        };
        let head = request_head(&req, "h", "/x");
        assert!(head.starts_with("PUT /x HTTP/1.1\r\nHost: h\r\n"), "{head}");
        assert!(head.contains("Accept: application/json\r\n"));
        assert!(!head.contains("*/*"));
        assert!(head.contains("X-Key: ab\r\n"));
        assert!(head.contains("Content-Length: 2\r\n") && !head.contains("999"));
        let port = serve(b"HTTP/1.1 201 Created\r\nX-Id: 7\r\nContent-Length: 2\r\n\r\nokjunk");
        let mut net = StdNet::new(true);
        let t = net
            .http_request(&HttpRequest {
                url: format!("http://127.0.0.1:{port}/x"),
                ..req
            })
            .unwrap();
        let r = wait_http(&mut net, t).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (201, &b"ok"[..]));
        assert_eq!(r.headers[0], ("X-Id".to_string(), "7".to_string()));
    }

    #[test]
    fn tcp_through_a_local_listener() {
        let mut net = StdNet::new(true);
        let l = net.listen(0);
        // Port 0 binds some free port; find it through a second listener.
        drop(l);
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = server.accept().unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).unwrap();
            s.write_all(&buf).unwrap();
        });
        let h = net.connect(SocketKind::Tcp, "127.0.0.1", port).unwrap();
        let mut state = ConnState::Connecting;
        for _ in 0..500 {
            state = net.state(h).unwrap();
            if state != ConnState::Connecting {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(state, ConnState::Connected);
        assert_eq!(net.send(h, b"ping").unwrap(), 4);
        let mut got: Vec<u8> = Vec::new();
        let mut buf = [0u8; 16];
        for _ in 0..500 {
            match net.recv(h, &mut buf).unwrap() {
                Recv::Data(n) => got.extend(&buf[..n]),
                Recv::Closed => break,
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, b"ping");
        net.close(h);
        assert_eq!(net.state(h), Err(NetError::BadHandle));
    }
}
