//! A scripted [`NetBackend`] for tests: no real network, fixed answers.
//!
//! - `connect` to host `"echo"` (any port): every byte sent comes back.
//! - `connect` to host `"greeter"`: sends `HELLO\r\n`, then closes.
//! - any host ending in `.invalid`: the name does not resolve.
//! - other hosts: the connection is refused.
//! - Connections stand after [`FakeNet::connect_polls`] calls of `state`.
//! - `http_get`/`http_post`: answers from [`FakeNet::pages`] (404
//!   otherwise; a POST answers with its body), after
//!   [`FakeNet::http_polls`] calls of `poll_http`; `https:` is
//!   unsupported.
//! - `listen` works on any port; [`FakeNet::incoming`] connections (echo
//!   sockets) are handed out by `accept`.
//! - The clock stands at [`FakeNet::time`].

use std::collections::{BTreeMap, VecDeque};

use emu_core::host::{
    ConnState, HostTime, HttpResponse, HttpTicket, NetBackend, NetError, NetHandle, NetInfo, Recv,
    SocketKind,
};

#[derive(Clone, Debug)]
struct Socket {
    polls_left: u32,
    rx: VecDeque<u8>,
    echo: bool,
    /// Closes once `rx` is empty.
    closing: bool,
    failed: Option<NetError>,
}

#[derive(Clone, Debug)]
pub struct FakeNet {
    /// `false`: every call fails with [`NetError::NoNetwork`].
    pub online: bool,
    pub connect_polls: u32,
    pub http_polls: u32,
    /// URL → (status, body).
    pub pages: BTreeMap<String, (u16, Vec<u8>)>,
    pub incoming: u32,
    pub time: HostTime,
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
            incoming: 0,
            // 2026-10-25 13:14:15 UTC, local time one hour ahead.
            time: HostTime {
                unix_secs: 1_792_934_055,
                utc_offset_secs: 3600,
            },
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

    fn request(&mut self, url: &str, post: Option<&[u8]>) -> Result<HttpTicket, NetError> {
        self.online()?;
        if !url.starts_with("http://") {
            return Err(NetError::Unsupported(format!("scheme of {url}")));
        }
        let res = match (self.pages.get(url), post) {
            (Some((status, body)), _) => HttpResponse {
                status: *status,
                body: body.clone(),
            },
            (None, Some(body)) => HttpResponse {
                status: 200,
                body: body.to_vec(),
            },
            (None, None) => HttpResponse {
                status: 404,
                body: b"not found".to_vec(),
            },
        };
        let t = self.handle();
        self.http.insert(t, (self.http_polls, Ok(res)));
        Ok(t)
    }
}

impl NetBackend for FakeNet {
    fn connect(&mut self, _: SocketKind, host: &str, _port: u16) -> Result<NetHandle, NetError> {
        self.online()?;
        let mut s = Socket {
            polls_left: self.connect_polls,
            rx: VecDeque::new(),
            echo: false,
            closing: false,
            failed: None,
        };
        match host {
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
        Ok(h)
    }

    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        self.online()?;
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
        self.sockets.insert(
            h,
            Socket {
                polls_left: 0,
                rx: VecDeque::new(),
                echo: true,
                closing: false,
                failed: None,
            },
        );
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
        if s.echo {
            s.rx.extend(data);
        }
        Ok(data.len())
    }

    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        let s = self.sockets.get_mut(&h).ok_or(NetError::BadHandle)?;
        if s.failed.is_some() {
            return Err(NetError::Io("not connected".into()));
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
        self.request(url, None)
    }

    fn http_post(&mut self, url: &str, _: &str, body: &[u8]) -> Result<HttpTicket, NetError> {
        self.request(url, Some(body))
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
