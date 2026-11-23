//! A scripted [`NetBackend`] for tests: no real network, fixed answers.
//!
//! - `connect` to host `"echo"` (any port): every byte sent comes back.
//! - `connect` to host `"greeter"`: sends `HELLO\r\n`, then closes.
//! - any host ending in `.invalid`: the name does not resolve.
//! - other hosts: the connection is refused.
//! - Connections stand after [`FakeNet::connect_polls`] calls of `state`.
//! - `http_get`/`http_post`/`http_request`: answers from
//!   [`FakeNet::pages`] with the field `Content-Type` from
//!   [`FakeNet::types`] (404 otherwise; a request with a body answers with
//!   it and its content type); every request is kept in
//!   [`FakeNet::requests`]; answers come after
//!   [`FakeNet::http_polls`] calls of `poll_http`; `https:` is
//!   unsupported.
//! - `listen` works on any port; [`FakeNet::incoming`] connections (echo
//!   sockets) are handed out by `accept`.
//! - The clock stands at [`FakeNet::time`].
//! - Scripted peers ([`FakeNet::script_host`]): `connect` to such a name
//!   stands; what the device sends is kept ([`FakeNet::sent`], datagrams of
//!   a UDP socket in [`FakeNet::sent_datagrams`]), the test feeds what
//!   arrives ([`FakeNet::push_rx`], one datagram per call on a UDP socket)
//!   and closes from the far side ([`FakeNet::peer_close`]);
//!   [`FakeNet::set_quota`] limits how many bytes `send` takes. Accepted
//!   connections ([`FakeNet::accepted`]) can be driven the same way.
//!   Every `connect` is logged in [`FakeNet::connects`];
//!   [`FakeNet::refuse_listen`] makes `listen` fail.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mimic_core::host::{
    ConnState, HostTime, HttpRequest, HttpResponse, HttpTicket, NetBackend, NetError, NetHandle,
    NetInfo, Recv, SocketKind,
};

#[derive(Clone, Debug)]
struct Socket {
    polls_left: u32,
    rx: VecDeque<u8>,
    echo: bool,
    /// Closes once `rx` is empty.
    closing: bool,
    failed: Option<NetError>,
    /// Keep what is sent (scripted peers, accepted connections).
    capture: bool,
    sent: Vec<u8>,
    /// How many bytes `send` still takes; `None`: all.
    quota: Option<usize>,
    /// A scripted UDP socket: datagrams keep their boundaries.
    udp: bool,
    rx_datagrams: VecDeque<Vec<u8>>,
    sent_datagrams: Vec<Vec<u8>>,
}

impl Socket {
    fn new(polls_left: u32) -> Self {
        Self {
            polls_left,
            rx: VecDeque::new(),
            echo: false,
            closing: false,
            failed: None,
            capture: false,
            sent: Vec::new(),
            quota: None,
            udp: false,
            rx_datagrams: VecDeque::new(),
            sent_datagrams: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FakeNet {
    /// `false`: every call fails with [`NetError::NoNetwork`].
    pub online: bool,
    pub connect_polls: u32,
    pub http_polls: u32,
    /// URL → (status, body).
    pub pages: BTreeMap<String, (u16, Vec<u8>)>,
    /// URL → content type of its page.
    pub types: BTreeMap<String, String>,
    /// The requests made, in order.
    pub requests: Vec<HttpRequest>,
    pub incoming: u32,
    pub time: HostTime,
    /// Every `connect`: handle, kind, host name, port.
    pub connects: Vec<(NetHandle, SocketKind, String, u16)>,
    /// The connections handed out by `accept`.
    pub accepted: Vec<NetHandle>,
    /// `true`: `listen` fails.
    pub refuse_listen: bool,
    scripted: BTreeSet<String>,
    sockets: BTreeMap<NetHandle, Socket>,
    listeners: BTreeMap<NetHandle, u16>,
    http: BTreeMap<HttpTicket, (u32, Result<HttpResponse, NetError>)>,
    next: u32,
}

impl Default for FakeNet {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeNet {
    pub fn new() -> Self {
        Self {
            online: true,
            connect_polls: 2,
            http_polls: 3,
            pages: BTreeMap::new(),
            types: BTreeMap::new(),
            requests: Vec::new(),
            incoming: 0,
            // 2026-10-25 13:14:15 UTC, local time one hour ahead.
            time: HostTime {
                unix_secs: 1_792_934_055,
                utc_offset_secs: 3600,
            },
            connects: Vec::new(),
            accepted: Vec::new(),
            refuse_listen: false,
            scripted: BTreeSet::new(),
            sockets: BTreeMap::new(),
            listeners: BTreeMap::new(),
            http: BTreeMap::new(),
            next: 1,
        }
    }

    /// Serve `body` with status 200 at `url`.
    pub fn page(mut self, url: &str, body: &[u8]) -> Self {
        self.pages.insert(url.into(), (200, body.to_vec()));
        self
    }

    /// Make `host` a scripted peer (see the module documentation).
    pub fn script_host(&mut self, host: &str) {
        self.scripted.insert(host.into());
    }

    /// What was sent on a scripted or accepted stream socket so far.
    pub fn sent(&self, h: NetHandle) -> &[u8] {
        self.sockets.get(&h).map_or(&[], |s| s.sent.as_slice())
    }

    /// The datagrams sent on a scripted UDP socket so far.
    pub fn sent_datagrams(&self, h: NetHandle) -> &[Vec<u8>] {
        self.sockets
            .get(&h)
            .map_or(&[], |s| s.sent_datagrams.as_slice())
    }

    /// Let `data` arrive on socket `h` (one datagram on a scripted UDP
    /// socket).
    pub fn push_rx(&mut self, h: NetHandle, data: &[u8]) {
        if let Some(s) = self.sockets.get_mut(&h) {
            if s.udp {
                s.rx_datagrams.push_back(data.to_vec());
            } else {
                s.rx.extend(data);
            }
        }
    }

    /// Bytes that arrived on `h` and were not read yet.
    pub fn unread(&self, h: NetHandle) -> usize {
        self.sockets.get(&h).map_or(0, |s| s.rx.len())
    }

    /// The far side closes `h` (what arrived before can still be read).
    pub fn peer_close(&mut self, h: NetHandle) {
        if let Some(s) = self.sockets.get_mut(&h) {
            s.closing = true;
        }
    }

    /// Limit how many bytes `send` on `h` takes from now on (`None`: all).
    pub fn set_quota(&mut self, h: NetHandle, quota: Option<usize>) {
        if let Some(s) = self.sockets.get_mut(&h) {
            s.quota = quota;
        }
    }

    /// Whether socket `h` exists (was not closed by the device).
    pub fn is_open(&self, h: NetHandle) -> bool {
        self.sockets.contains_key(&h)
    }

    /// Whether a listener stands on `port`.
    pub fn listening(&self, port: u16) -> bool {
        self.listeners.values().any(|p| *p == port)
    }

    fn handle(&mut self) -> u32 {
        let h = self.next;
        self.next += 1;
        h
    }

    fn online(&self) -> Result<(), NetError> {
        if self.online {
            Ok(())
        } else {
            Err(NetError::NoNetwork)
        }
    }

    /// Serve `body` as `application/json` at `url`.
    pub fn json_page(mut self, url: &str, body: &str) -> Self {
        self.types.insert(url.into(), "application/json".into());
        self.page(url, body.as_bytes())
    }

    fn request(&mut self, req: HttpRequest) -> Result<HttpTicket, NetError> {
        self.online()?;
        if !req.url.starts_with("http://") {
            return Err(NetError::Unsupported(format!("scheme of {}", req.url)));
        }
        let typed = |ct: Option<&str>| {
            ct.map(|t| vec![("Content-Type".to_string(), t.to_string())])
                .unwrap_or_default()
        };
        let res = match self.pages.get(&req.url) {
            Some((status, body)) => HttpResponse {
                status: *status,
                headers: typed(self.types.get(&req.url).map(String::as_str)),
                body: body.clone(),
            },
            None if !req.body.is_empty() || req.method == "POST" => HttpResponse {
                status: 200,
                headers: typed(req.header("content-type")),
                body: req.body.clone(),
            },
            None => HttpResponse {
                status: 404,
                body: b"not found".to_vec(),
                ..Default::default()
            },
        };
        self.requests.push(req);
        let t = self.handle();
        self.http.insert(t, (self.http_polls, Ok(res)));
        Ok(t)
    }
}

impl NetBackend for FakeNet {
    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError> {
        self.online()?;
        let mut s = Socket::new(self.connect_polls);
        match host {
            h if self.scripted.contains(h) => {
                s.capture = true;
                s.udp = kind == SocketKind::Udp;
            }
            "echo" => s.echo = true,
            "greeter" => {
                s.rx.extend(b"HELLO\r\n");
                s.closing = true;
            }
            h if h.ends_with(".invalid") => s.failed = Some(NetError::Resolve(h.into())),
            h => s.failed = Some(NetError::Connect(format!("{h}: refused"))),
        }
        let h = self.handle();
        self.sockets.insert(h, s);
        self.connects.push((h, kind, host.into(), port));
        Ok(h)
    }

    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        self.online()?;
        if self.refuse_listen {
            return Err(NetError::Connect(format!("port {port} in use")));
        }
        let h = self.handle();
        self.listeners.insert(h, port);
        Ok(h)
    }

    fn accept(&mut self, l: NetHandle) -> Result<Option<NetHandle>, NetError> {
        if !self.listeners.contains_key(&l) {
            return Err(NetError::BadHandle);
        }
        if self.incoming == 0 {
            return Ok(None);
        }
        self.incoming -= 1;
        let h = self.handle();
        let mut s = Socket::new(0);
        s.echo = true;
        s.capture = true;
        self.sockets.insert(h, s);
        self.accepted.push(h);
        Ok(Some(h))
    }

    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError> {
        let s = self.sockets.get_mut(&h).ok_or(NetError::BadHandle)?;
        if s.polls_left > 0 {
            s.polls_left -= 1;
            return Ok(ConnState::Connecting);
        }
        Ok(match (&s.failed, s.closing && s.rx.is_empty()) {
            (Some(e), _) => ConnState::Failed(e.clone()),
            (None, true) => ConnState::Closed,
            (None, false) => ConnState::Connected,
        })
    }

    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError> {
        let s = self.sockets.get_mut(&h).ok_or(NetError::BadHandle)?;
        if s.polls_left > 0 {
            return Ok(0);
        }
        if s.failed.is_some() {
            return Err(NetError::Io("not connected".into()));
        }
        let n = match &mut s.quota {
            Some(q) => {
                let n = data.len().min(*q);
                *q -= n;
                n
            }
            None => data.len(),
        };
        let data = &data[..n];
        if s.udp {
            s.sent_datagrams.push(data.to_vec());
            return Ok(n);
        }
        if s.capture {
            s.sent.extend_from_slice(data);
        }
        if s.echo {
            s.rx.extend(data);
        }
        Ok(n)
    }

    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        let s = self.sockets.get_mut(&h).ok_or(NetError::BadHandle)?;
        if s.failed.is_some() {
            return Err(NetError::Io("not connected".into()));
        }
        if s.udp {
            return Ok(match s.rx_datagrams.pop_front() {
                Some(d) => {
                    let n = buf.len().min(d.len());
                    buf[..n].copy_from_slice(&d[..n]);
                    Recv::Data(n)
                }
                None if s.closing => Recv::Closed,
                None => Recv::Data(0),
            });
        }
        if s.rx.is_empty() {
            return Ok(if s.closing {
                Recv::Closed
            } else {
                Recv::Data(0)
            });
        }
        let n = buf.len().min(s.rx.len());
        for b in buf.iter_mut().take(n) {
            *b = s.rx.pop_front().unwrap_or(0);
        }
        Ok(Recv::Data(n))
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
        let Some((left, _)) = self.http.get_mut(&t) else {
            return Some(Err(NetError::BadHandle));
        };
        if *left > 0 {
            *left -= 1;
            return None;
        }
        self.http.remove(&t).map(|(_, r)| r)
    }

    fn info(&mut self) -> Result<NetInfo, NetError> {
        self.online()?;
        Ok(NetInfo::default())
    }

    fn time(&mut self) -> HostTime {
        self.time
    }
}
