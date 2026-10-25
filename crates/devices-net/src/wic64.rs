//! WiC64: a WLAN adapter (ESP32) on the C64's user port, firmware 2.x.
//!
//! Wiring and handshake (sources: WiC64 hardware notes, Forum64 thread
//! "WiC64 Hardware Infos", https://www.forum64.de/index.php?thread/119265-wic64-hardware-infos/;
//! pagetable.com, "UP9600"/user port articles, https://www.pagetable.com/?p=1656;
//! C64 Programmer's Reference Guide, user port pinout):
//!
//! - PB0–PB7 (CIA2 port B) carry the data in both directions.
//! - PA2 (CIA2 port A bit 2) is the direction, set by the C64: high = the
//!   C64 sends, low = the WiC64 sends.
//! - PC2 (pulses low after every access to CIA2 port B): the C64 has
//!   written a byte (sending) or read one (receiving).
//! - FLAG2 (CIA2 /FLAG, ICR bit 4): the WiC64 pulses it low when it has
//!   taken a byte (C64 sending) or put the next one on PB (C64 receiving).
//!
//! The ESP answers within microseconds; here every handshake takes
//! [`DELAY`] cycles (unverified, chosen plausibly).
//!
//! Protocols and commands (source: WiC64 library documentation,
//! https://github.com/WiC64-Team/wic64-library, README "Protocols",
//! "Commands", and the command constants of its `wic64.h`):
//!
//! - Legacy: `"W"`, total length (LSB, MSB, including the 4 header
//!   bytes), command, payload. Response: length (MSB first), payload; no
//!   status.
//! - Standard (protocol 2): `"R"`, command, payload length (LSB, MSB),
//!   payload. Response: status, length (LSB, MSB), payload.
//! - Extended: `"E"`, command, 32-bit payload length (LSB first), payload.
//!   Response: status, 32-bit length, payload.
//!
//! Status codes: 0 success, 1 internal error, 2 client error, 3 connection
//! error (no WLAN), 4 network error, 5 server error.
//!
//! Commands: version string `$00` and numbers `$26`; HTTP GET `$01`,
//! GET with encoded data `$0F` (`<$` + 16-bit length + bytes → hex), POST
//! URL `$28` and POST data `$2B`; TCP open `$21` ("host:port"), available
//! `$30`, read `$22`, write `$23`, close `$2E`; IP `$06`, MAC `$14`, SSID
//! `$10`, RSSI `$11`, scan `$0C`, connect `$02`/`$0D`, is connected `$2C`,
//! is configured `$2F`; server get `$12`/set `$08`; local time `$15`,
//! time zone set `$16`/get `$17`; status message `$2A`; transfer and
//! remote time-outs `$2D`/`$32`; is hardware `$31` (answers "WiC64 is
//! emulated"), reboot `$29`, firmware update `$27` (refused); test
//! commands force time-out `$FC`, force error `$FD`, echo `$FE`. URLs
//! expand `%mac` (MAC without colons) and a leading `!` or `%ser` (the
//! server). Without network (`net=off`, the web frontend) the remote
//! commands answer with a connection error ("WiFi not connected").
//!
//! Unverified (not documented, chosen plausibly): [`DELAY`]; the legacy
//! protocol's error answer `!0`; the version reported (2.1.0); the MAC,
//! SSID, RSSI and scan answers (fixed values from the host's
//! [`emu_core::host::NetInfo`]); the time-zone table (`TIMEZONES`); POST
//! sends the data as the request body (`application/octet-stream`), not
//! as a form field; a response over 64 KB in the legacy or standard
//! protocol is a client error; the response of a request that is still
//! sending when the C64 starts a new one is dropped. Nothing here comes
//! from the WiC64 firmware's source (GPL; golden rule 7).
//!
//! Savestates hold the whole device; the host's TCP connection and an
//! HTTP request in flight are host handles: after loading, a live host
//! answers them with errors, a replay from its journal.

use emu_core::host::{
    lock, ConnState, HostServices, HttpResponse, HttpTicket, NetError, NetHandle, Recv, SharedNet,
    SocketKind,
};
use emu_core::input::InputEvent;
use emu_core::port::{user_port, PortDevice, PortKind, PortLines};
use emu_core::{Cycle, StateError, StateReader, StateWriter};

/// Cycles the ESP takes for a handshake (unverified).
pub const DELAY: u64 = 20;
/// FLAG2 pulse length in cycles.
const FLAG_LEN: u64 = 2;
/// Cycles between polls of the host while a request runs.
const POLL: u64 = 1000;
/// Cycles per second of the time-outs (the C64's φ2, about 1 MHz).
const SECOND: u64 = 1_000_000;
/// Largest request kept (longer ones are answered with a client error).
const MAX_REQUEST: usize = 1 << 20;
const DEFAULT_SERVER: &str = "http://x.wic64.net/prg/";
const VERSION: [u8; 4] = [2, 1, 0, 0];

/// Time-zone offsets in seconds for `$16` (unverified: the firmware has
/// 32 entries; these are the common GMT offsets in order).
const TIMEZONES: [i32; 32] = [
    0, 0, 3600, 7200, 10800, 12600, 14400, 16200, 18000, 19800, 20700, 21600, 25200, 28800, 32400,
    34200, 36000, 39600, 43200, 46800, -3600, -7200, -10800, -12600, -14400, -18000, -21600,
    -25200, -28800, -32400, -36000, -39600,
];

pub mod status {
    pub const SUCCESS: u8 = 0;
    pub const INTERNAL_ERROR: u8 = 1;
    pub const CLIENT_ERROR: u8 = 2;
    pub const CONNECTION_ERROR: u8 = 3;
    pub const NETWORK_ERROR: u8 = 4;
    pub const SERVER_ERROR: u8 = 5;
}

/// Command ids (WiC64 library, `wic64.h`).
pub mod cmd {
    pub const GET_VERSION_STRING: u8 = 0x00;
    pub const HTTP_GET: u8 = 0x01;
    pub const CONNECT_WITH_SSID_STRING: u8 = 0x02;
    pub const GET_IP: u8 = 0x06;
    pub const LEGACY_VERSION: u8 = 0x07;
    pub const SET_SERVER: u8 = 0x08;
    pub const SCAN_WIFI_NETWORKS: u8 = 0x0c;
    pub const CONNECT_WITH_SSID_INDEX: u8 = 0x0d;
    pub const HTTP_GET_ENCODED: u8 = 0x0f;
    pub const GET_SSID: u8 = 0x10;
    pub const GET_RSSI: u8 = 0x11;
    pub const GET_SERVER: u8 = 0x12;
    pub const GET_MAC: u8 = 0x14;
    pub const GET_LOCAL_TIME: u8 = 0x15;
    pub const SET_TIMEZONE: u8 = 0x16;
    pub const GET_TIMEZONE: u8 = 0x17;
    pub const TCP_OPEN: u8 = 0x21;
    pub const TCP_READ: u8 = 0x22;
    pub const TCP_WRITE: u8 = 0x23;
    pub const GET_VERSION_NUMBERS: u8 = 0x26;
    pub const UPDATE_FIRMWARE: u8 = 0x27;
    pub const HTTP_POST_URL: u8 = 0x28;
    pub const REBOOT: u8 = 0x29;
    pub const GET_STATUS_MESSAGE: u8 = 0x2a;
    pub const HTTP_POST_DATA: u8 = 0x2b;
    pub const IS_CONNECTED: u8 = 0x2c;
    pub const SET_TRANSFER_TIMEOUT: u8 = 0x2d;
    pub const TCP_CLOSE: u8 = 0x2e;
    pub const IS_CONFIGURED: u8 = 0x2f;
    pub const TCP_AVAILABLE: u8 = 0x30;
    pub const IS_HARDWARE: u8 = 0x31;
    pub const SET_REMOTE_TIMEOUT: u8 = 0x32;
    pub const FORCE_TIMEOUT: u8 = 0xfc;
    pub const FORCE_ERROR: u8 = 0xfd;
    pub const ECHO: u8 = 0xfe;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Legacy,
    Standard,
    Extended,
}

impl Protocol {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Option<Self> {
        [Self::Legacy, Self::Standard, Self::Extended]
            .get(usize::from(v))
            .copied()
    }
}

/// Work running in the background of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Busy {
    None,
    Http {
        ticket: HttpTicket,
        deadline: u64,
    },
    TcpOpen {
        handle: NetHandle,
        deadline: u64,
    },
    TcpWrite {
        data: Vec<u8>,
        sent: usize,
        deadline: u64,
    },
    Delay {
        until: u64,
    },
}

/// The WiC64 at pin level. Network from [`PortDevice::attach_host`].
pub struct Wic64 {
    /// PA2 high: the C64 sends.
    c64_sends: bool,
    /// The byte the WiC64 drives onto PB0–PB7.
    drive: Option<u8>,
    /// FLAG2 is low while `now` is below this.
    flag_until: u64,
    /// A FLAG2 pulse due at this time (byte taken).
    flag_at: Option<u64>,
    /// A byte is on PB, waiting for the C64's read.
    offered: bool,
    /// Earliest time for the next byte.
    next_at: u64,
    rx: Vec<u8>,
    /// Bytes of the current request (may exceed what `rx` keeps).
    rx_count: usize,
    tx: Vec<u8>,
    tx_pos: usize,
    proto: Protocol,
    command: u8,
    busy: Busy,
    poll_at: u64,
    server: String,
    post_url: String,
    timezone: Option<u8>,
    remote_timeout: u8,
    message: String,
    tcp: Option<NetHandle>,
    tcp_buf: Vec<u8>,
    tcp_closed: bool,
    /// The last time the machine called `write_lines`.
    now: u64,
    net: Option<SharedNet>,
}

impl Default for Wic64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Wic64 {
    pub fn new() -> Self {
        Self {
            c64_sends: true,
            drive: None,
            flag_until: 0,
            flag_at: None,
            offered: false,
            next_at: 0,
            rx: Vec::new(),
            rx_count: 0,
            tx: Vec::new(),
            tx_pos: 0,
            proto: Protocol::Standard,
            command: 0,
            busy: Busy::None,
            poll_at: 0,
            server: DEFAULT_SERVER.into(),
            post_url: String::new(),
            timezone: None,
            remote_timeout: 5,
            message: "Success".into(),
            tcp: None,
            tcp_buf: Vec::new(),
            tcp_closed: false,
            now: 0,
            net: None,
        }
    }

    /// Bytes of the request so far: complete when this is reached.
    fn request_len(&self) -> Option<usize> {
        let r = &self.rx;
        match r.first()? {
            b'W' if r.len() >= 3 => Some(usize::from(u16::from_le_bytes([r[1], r[2]])).max(4)),
            b'R' if r.len() >= 4 => Some(4 + usize::from(u16::from_le_bytes([r[2], r[3]]))),
            b'E' if r.len() >= 6 => Some(6 + u32::from_le_bytes([r[2], r[3], r[4], r[5]]) as usize),
            _ => None,
        }
    }

    /// A byte from the C64.
    fn receive(&mut self, b: u8, now: u64) {
        if self.rx_count == 0 && !matches!(b, b'W' | b'R' | b'E') {
            return; // not a request header: ignored
        }
        if self.rx.len() < MAX_REQUEST {
            self.rx.push(b);
        }
        self.rx_count += 1;
        if self.request_len().is_some_and(|n| self.rx_count >= n) {
            self.process(now);
            self.rx.clear();
            self.rx_count = 0;
        }
    }

    fn header_len(&self) -> usize {
        match self.proto {
            Protocol::Extended => 6,
            _ => 4,
        }
    }

    fn respond(&mut self, status: u8, payload: &[u8], message: &str) {
        self.message = message.into();
        let mut out = Vec::with_capacity(payload.len() + 5);
        match self.proto {
            Protocol::Legacy => {
                let p: &[u8] = if status == status::SUCCESS {
                    payload
                } else {
                    b"!0"
                };
                let p = &p[..p.len().min(0xffff)];
                out.extend((p.len() as u16).to_be_bytes());
                out.extend(p);
            }
            Protocol::Standard => {
                if payload.len() > 0xffff {
                    return self.respond(
                        status::CLIENT_ERROR,
                        &[],
                        "Response too large, use the extended protocol",
                    );
                }
                out.push(status);
                out.extend((payload.len() as u16).to_le_bytes());
                out.extend(payload);
            }
            Protocol::Extended => {
                out.push(status);
                out.extend((payload.len() as u32).to_le_bytes());
                out.extend(payload);
            }
        }
        self.tx = out;
        self.tx_pos = 0;
        self.busy = Busy::None;
    }

    fn ok(&mut self, payload: &[u8]) {
        self.respond(status::SUCCESS, payload, "Success");
    }

    fn fail(&mut self, status: u8, message: &str) {
        self.respond(status, &[], message);
    }

    fn net_fail(&mut self, e: &NetError) {
        match e {
            NetError::NoNetwork => self.fail(status::CONNECTION_ERROR, "WiFi not connected"),
            other => self.fail(status::NETWORK_ERROR, &other.to_string()),
        }
    }

    fn online(&self) -> Result<emu_core::host::NetInfo, NetError> {
        match &self.net {
            Some(n) => lock(n).info(),
            None => Err(NetError::NoNetwork),
        }
    }

    fn deadline(&self, now: u64) -> u64 {
        now + u64::from(self.remote_timeout.max(1)) * SECOND
    }

    fn mac_hex(&self, colons: bool) -> String {
        let mac = self.online().map(|i| i.mac).unwrap_or([0; 6]);
        let parts: Vec<String> = mac.iter().map(|b| format!("{b:02X}")).collect();
        parts.join(if colons { ":" } else { "" })
    }

    /// Expand `%mac`, `%ser` and a leading `!` in a URL.
    fn expand_url(&self, url: &[u8]) -> String {
        let s: String = url
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        let s = match s.strip_prefix('!') {
            Some(rest) => format!("{}{rest}", self.server),
            None => s,
        };
        s.replace("%ser", &self.server)
            .replace("%mac", &self.mac_hex(false))
    }

    /// `<$` + 16-bit length + bytes → the bytes as upper-case hex.
    fn encode_url(url: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(url.len() * 2);
        let mut i = 0;
        while i < url.len() {
            if url[i] == b'<' && url.get(i + 1) == Some(&b'$') && i + 3 < url.len() {
                let n = usize::from(u16::from_le_bytes([url[i + 2], url[i + 3]]));
                let data = &url[i + 4..(i + 4 + n).min(url.len())];
                for b in data {
                    out.extend(format!("{b:02X}").bytes());
                }
                i += 4 + n;
            } else {
                out.push(url[i]);
                i += 1;
            }
        }
        out
    }

    fn start_http(&mut self, r: Result<HttpTicket, NetError>, now: u64) {
        match r {
            Ok(ticket) => {
                self.busy = Busy::Http {
                    ticket,
                    deadline: self.deadline(now),
                };
                self.poll_at = now;
            }
            Err(e) => self.net_fail(&e),
        }
    }

    /// A complete request.
    fn process(&mut self, now: u64) {
        let rx = std::mem::take(&mut self.rx);
        self.proto = match rx[0] {
            b'W' => Protocol::Legacy,
            b'R' => Protocol::Standard,
            _ => Protocol::Extended,
        };
        let header = self.header_len();
        let command = if self.proto == Protocol::Legacy {
            rx.get(3).copied().unwrap_or(0)
        } else {
            rx[1]
        };
        self.command = command;
        self.tx.clear();
        self.tx_pos = 0;
        if self.rx_count > MAX_REQUEST {
            return self.fail(status::CLIENT_ERROR, "Request too large");
        }
        let payload = rx.get(header..).unwrap_or(&[]);
        let extended_ok = matches!(
            command,
            cmd::HTTP_GET | cmd::HTTP_GET_ENCODED | cmd::HTTP_POST_URL | cmd::HTTP_POST_DATA
        );
        if self.proto == Protocol::Extended && !extended_ok {
            return self.fail(status::CLIENT_ERROR, "Extended protocol not supported");
        }
        let net = self.net.clone();
        match command {
            cmd::GET_VERSION_STRING | cmd::LEGACY_VERSION => {
                self.ok(format!("{}.{}.{}", VERSION[0], VERSION[1], VERSION[2]).as_bytes())
            }
            cmd::GET_VERSION_NUMBERS => self.ok(&VERSION),
            cmd::HTTP_GET | cmd::HTTP_GET_ENCODED => {
                let raw = if command == cmd::HTTP_GET_ENCODED {
                    Self::encode_url(payload)
                } else {
                    payload.to_vec()
                };
                let url = self.expand_url(&raw);
                if url.is_empty() {
                    return self.fail(status::CLIENT_ERROR, "No URL specified");
                }
                if url.len() > 2000 {
                    return self.fail(status::CLIENT_ERROR, "URL too long");
                }
                let r = match &net {
                    Some(n) => self.online().and_then(|_| lock(n).http_get(&url)),
                    None => Err(NetError::NoNetwork),
                };
                self.start_http(r, now);
            }
            cmd::HTTP_POST_URL => {
                let url = self.expand_url(payload);
                if url.is_empty() {
                    return self.fail(status::CLIENT_ERROR, "No URL specified");
                }
                self.post_url = url;
                self.ok(&[]);
            }
            cmd::HTTP_POST_DATA => {
                if self.post_url.is_empty() {
                    return self.fail(status::CLIENT_ERROR, "No URL set");
                }
                let url = self.post_url.clone();
                let r = match &net {
                    Some(n) => self
                        .online()
                        .and_then(|_| lock(n).http_post(&url, "application/octet-stream", payload)),
                    None => Err(NetError::NoNetwork),
                };
                self.start_http(r, now);
            }
            cmd::TCP_OPEN => {
                let target: String = payload
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| char::from(b))
                    .collect();
                let Some((host, port)) = target
                    .rsplit_once(':')
                    .and_then(|(h, p)| Some((h.to_string(), p.trim().parse::<u16>().ok()?)))
                else {
                    return self.fail(status::CLIENT_ERROR, "No URL specified");
                };
                self.close_tcp();
                let r = match &net {
                    Some(n) => self
                        .online()
                        .and_then(|_| lock(n).connect(SocketKind::Tcp, &host, port)),
                    None => Err(NetError::NoNetwork),
                };
                match r {
                    Ok(handle) => {
                        self.busy = Busy::TcpOpen {
                            handle,
                            deadline: self.deadline(now),
                        };
                        self.poll_at = now;
                    }
                    Err(e) => self.net_fail(&e),
                }
            }
            cmd::TCP_AVAILABLE | cmd::TCP_READ => {
                if self.tcp.is_none() {
                    return self.fail(status::NETWORK_ERROR, "No TCP connection");
                }
                self.fill_tcp_buf();
                let limit = if self.proto == Protocol::Extended {
                    usize::MAX
                } else {
                    0xffff
                };
                if command == cmd::TCP_AVAILABLE {
                    let n = self.tcp_buf.len().min(0xffff) as u16;
                    self.ok(&n.to_le_bytes())
                } else if self.tcp_buf.is_empty() && self.tcp_closed {
                    self.close_tcp();
                    self.fail(status::NETWORK_ERROR, "Connection closed")
                } else {
                    let n = self.tcp_buf.len().min(limit);
                    let data: Vec<u8> = self.tcp_buf.drain(..n).collect();
                    self.ok(&data)
                }
            }
            cmd::TCP_WRITE => {
                if self.tcp.is_none() {
                    return self.fail(status::NETWORK_ERROR, "No TCP connection");
                }
                self.busy = Busy::TcpWrite {
                    data: payload.to_vec(),
                    sent: 0,
                    deadline: self.deadline(now),
                };
                self.poll_at = now;
            }
            cmd::TCP_CLOSE => {
                self.close_tcp();
                self.ok(&[]);
            }
            cmd::GET_IP => {
                let ip = self.online().map(|i| i.ip).unwrap_or([0; 4]);
                let s = format!("{}.{}.{}.{}\0", ip[0], ip[1], ip[2], ip[3]);
                self.ok(s.as_bytes())
            }
            cmd::GET_MAC => {
                let s = format!("{}\0", self.mac_hex(true));
                self.ok(s.as_bytes())
            }
            cmd::GET_SSID => match self.online() {
                Ok(i) => self.ok(format!("{}\0", i.ssid).as_bytes()),
                Err(_) => self.ok(b"\0"),
            },
            cmd::GET_RSSI => match self.online() {
                Ok(i) => self.ok(format!("{:+}dBm\0", i.rssi).as_bytes()),
                Err(_) => self.ok(b"\0"),
            },
            cmd::SCAN_WIFI_NETWORKS => match self.online() {
                Ok(i) => {
                    let mut out = Vec::new();
                    let sep = if self.proto == Protocol::Legacy { 1 } else { 0 };
                    for field in ["0".to_string(), i.ssid, i.rssi.to_string()] {
                        out.extend(field.bytes());
                        out.push(sep);
                    }
                    if self.proto != Protocol::Legacy {
                        out.push(0xff);
                    }
                    self.ok(&out)
                }
                Err(_) => self.fail(status::NETWORK_ERROR, "No networks found"),
            },
            cmd::CONNECT_WITH_SSID_STRING | cmd::CONNECT_WITH_SSID_INDEX | cmd::IS_CONNECTED => {
                match self.online() {
                    Ok(_) => self.ok(&[]),
                    Err(_) => self.fail(status::CONNECTION_ERROR, "WiFi not connected"),
                }
            }
            cmd::IS_CONFIGURED => self.ok(&[8]),
            cmd::GET_SERVER => {
                let s = format!("{}\0", self.server);
                self.ok(s.as_bytes())
            }
            cmd::SET_SERVER => {
                let s: String = payload
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| char::from(b))
                    .collect();
                self.server = if s.is_empty() {
                    DEFAULT_SERVER.into()
                } else {
                    s
                };
                self.ok(&[])
            }
            cmd::GET_LOCAL_TIME => match (self.online(), &net) {
                (Ok(_), Some(n)) => {
                    let mut t = lock(n).time();
                    if let Some(tz) = self.timezone {
                        t.utc_offset_secs = TIMEZONES[usize::from(tz)];
                    }
                    let (y, mo, d, h, mi, s, _) = t.local();
                    let text = format!("{h:02}:{mi:02}:{s:02} {d:02}-{mo:02}-{y:04}\0");
                    self.ok(text.as_bytes())
                }
                _ => self.fail(status::INTERNAL_ERROR, "Could not get local time"),
            },
            cmd::SET_TIMEZONE => {
                let digit = |b: u8| if b >= b'0' { b - b'0' } else { b };
                let (Some(&lo), Some(&hi)) = (payload.first(), payload.get(1)) else {
                    return self.fail(status::CLIENT_ERROR, "No timezone specified");
                };
                let idx = digit(lo) + 10 * digit(hi);
                if usize::from(idx) >= TIMEZONES.len() {
                    return self.fail(status::CLIENT_ERROR, "Invalid timezone");
                }
                self.timezone = Some(idx);
                self.ok(&[])
            }
            cmd::GET_TIMEZONE => {
                let offset = match self.timezone {
                    Some(tz) => TIMEZONES[usize::from(tz)],
                    None => net.as_ref().map_or(0, |n| lock(n).time().utc_offset_secs),
                };
                self.ok(format!("{offset}\0").as_bytes())
            }
            cmd::GET_STATUS_MESSAGE => {
                let mut m = self.message.clone();
                if payload.first().copied().unwrap_or(0) != 0 {
                    m = m.to_uppercase();
                }
                let mut bytes: Vec<u8> = m.bytes().take(39).collect();
                bytes.push(0);
                // Reporting does not change the message.
                let keep = self.message.clone();
                self.ok(&bytes);
                self.message = keep;
            }
            cmd::SET_TRANSFER_TIMEOUT | cmd::SET_REMOTE_TIMEOUT => match payload.first() {
                Some(&s) if s >= 1 => {
                    self.ok(&[]);
                    if command == cmd::SET_REMOTE_TIMEOUT {
                        self.remote_timeout = s;
                        return;
                    }
                }
                _ => self.fail(status::CLIENT_ERROR, "Invalid timeout"),
            },
            cmd::IS_HARDWARE => self.fail(status::INTERNAL_ERROR, "WiC64 is emulated"),
            cmd::FORCE_TIMEOUT => {
                let secs = payload.first().copied().unwrap_or(1).max(1);
                self.busy = Busy::Delay {
                    until: now + u64::from(secs) * SECOND,
                };
                self.poll_at = now;
            }
            cmd::FORCE_ERROR => self.fail(status::INTERNAL_ERROR, "Test error"),
            cmd::ECHO => {
                let data = payload.to_vec();
                self.ok(&data)
            }
            cmd::UPDATE_FIRMWARE => {
                self.fail(status::INTERNAL_ERROR, "Firmware update not supported")
            }
            cmd::REBOOT => {
                self.reboot();
                // One handshake when the "reboot" is done.
                self.flag_at = Some(now + 10 * DELAY);
            }
            _ => self.fail(status::CLIENT_ERROR, "Unknown command"),
        }
        // The remote time-out holds for one request only.
        if !matches!(self.busy, Busy::None) {
            return;
        }
        self.remote_timeout = 5;
    }

    fn reboot(&mut self) {
        self.close_tcp();
        self.cancel_busy();
        let (net, now, c64_sends) = (self.net.take(), self.now, self.c64_sends);
        *self = Self::new();
        self.net = net;
        self.now = now;
        self.c64_sends = c64_sends;
    }

    fn fill_tcp_buf(&mut self) {
        let (Some(h), Some(n)) = (self.tcp, &self.net) else {
            return;
        };
        let mut buf = vec![0; 4096];
        let mut net = lock(n);
        while self.tcp_buf.len() < 0x10000 {
            match net.recv(h, &mut buf) {
                Ok(Recv::Data(0)) => break,
                Ok(Recv::Data(k)) => self.tcp_buf.extend(&buf[..k]),
                Ok(Recv::Closed) | Err(_) => {
                    self.tcp_closed = true;
                    break;
                }
            }
        }
    }

    fn close_tcp(&mut self) {
        if let (Some(h), Some(n)) = (self.tcp.take(), &self.net) {
            lock(n).close(h);
        }
        self.tcp_buf.clear();
        self.tcp_closed = false;
    }

    fn cancel_busy(&mut self) {
        if let (Busy::TcpOpen { handle, .. }, Some(n)) = (&self.busy, &self.net) {
            lock(n).close(*handle);
        }
        self.busy = Busy::None;
    }

    /// Background work: poll the host for the running request.
    fn poll(&mut self, now: u64) {
        let Some(net) = self.net.clone() else {
            if !matches!(self.busy, Busy::None | Busy::Delay { .. }) {
                self.net_fail(&NetError::NoNetwork);
            }
            return;
        };
        match self.busy.clone() {
            Busy::None => {}
            Busy::Delay { until } => {
                if now >= until {
                    self.ok(&[]);
                }
            }
            Busy::Http { ticket, deadline } => {
                let r = lock(&net).poll_http(ticket);
                match r {
                    None if now >= deadline => self.fail(status::NETWORK_ERROR, "Timeout"),
                    None => {}
                    Some(Ok(HttpResponse { status: s, body })) if (200..300).contains(&s) => {
                        self.ok(&body)
                    }
                    Some(Ok(HttpResponse { status: s, .. })) => {
                        self.fail(status::SERVER_ERROR, &format!("HTTP error {s}"))
                    }
                    Some(Err(e)) => self.net_fail(&e),
                }
            }
            Busy::TcpOpen { handle, deadline } => {
                let r = lock(&net).state(handle);
                match r {
                    Ok(ConnState::Connecting) if now < deadline => {}
                    Ok(ConnState::Connected | ConnState::Closed) => {
                        self.tcp = Some(handle);
                        self.tcp_buf.clear();
                        self.tcp_closed = false;
                        self.busy = Busy::None;
                        self.ok(&[]);
                    }
                    Ok(ConnState::Connecting) => {
                        lock(&net).close(handle);
                        self.busy = Busy::None;
                        self.fail(status::NETWORK_ERROR, "Timeout")
                    }
                    Ok(ConnState::Failed(e)) | Err(e) => {
                        lock(&net).close(handle);
                        self.busy = Busy::None;
                        self.fail(status::NETWORK_ERROR, &e.to_string())
                    }
                }
            }
            Busy::TcpWrite {
                data,
                mut sent,
                deadline,
            } => {
                let Some(h) = self.tcp else {
                    return self.fail(status::NETWORK_ERROR, "No TCP connection");
                };
                let r = lock(&net).send(h, &data[sent..]);
                match r {
                    Ok(n) => {
                        sent += n;
                        if sent >= data.len() {
                            self.ok(&[]);
                        } else if now >= deadline {
                            self.fail(status::NETWORK_ERROR, "Timeout");
                        } else {
                            self.busy = Busy::TcpWrite {
                                data,
                                sent,
                                deadline,
                            };
                        }
                    }
                    Err(e) => self.fail(status::NETWORK_ERROR, &e.to_string()),
                }
            }
        }
        if matches!(self.busy, Busy::None) {
            self.remote_timeout = 5;
        }
    }

    fn save_busy(&self, w: &mut StateWriter) {
        match &self.busy {
            Busy::None => w.u8(0),
            Busy::Http { ticket, deadline } => {
                w.u8(1);
                w.u32(*ticket);
                w.u64(*deadline);
            }
            Busy::TcpOpen { handle, deadline } => {
                w.u8(2);
                w.u32(*handle);
                w.u64(*deadline);
            }
            Busy::TcpWrite {
                data,
                sent,
                deadline,
            } => {
                w.u8(3);
                w.bytes(data);
                w.u64(*sent as u64);
                w.u64(*deadline);
            }
            Busy::Delay { until } => {
                w.u8(4);
                w.u64(*until);
            }
        }
    }

    fn load_busy(c: &mut StateReader<'_>) -> Result<Busy, StateError> {
        Ok(match c.u8()? {
            0 => Busy::None,
            1 => Busy::Http {
                ticket: c.u32()?,
                deadline: c.u64()?,
            },
            2 => Busy::TcpOpen {
                handle: c.u32()?,
                deadline: c.u64()?,
            },
            3 => {
                let data = c.bytes()?.to_vec();
                let sent = c.u64()? as usize;
                if sent > data.len() {
                    return Err(StateError::Invalid("WiC64 write progress"));
                }
                Busy::TcpWrite {
                    data,
                    sent,
                    deadline: c.u64()?,
                }
            }
            4 => Busy::Delay { until: c.u64()? },
            _ => return Err(StateError::Invalid("WiC64 request state")),
        })
    }
}

impl PortDevice for Wic64 {
    fn kind(&self) -> PortKind {
        PortKind::CbmUserPort
    }

    fn read_lines(&self, now: Cycle) -> PortLines {
        let mut low = self.drive.map_or(0, user_port::pb_lines);
        if now.0 < self.flag_until {
            low |= user_port::FLAG2;
        }
        PortLines {
            pulled_low: low,
            analog: [u16::MAX; 2],
        }
    }

    /// Called by the machine every cycle with the C64's lines.
    fn write_lines(&mut self, lines: PortLines, now: Cycle) {
        let now = now.0;
        self.now = now;
        let pa2_high = lines.pulled_low & user_port::PA2 == 0;
        let pc2 = lines.pulled_low & user_port::PC2 != 0;
        if pa2_high != self.c64_sends {
            self.c64_sends = pa2_high;
            self.drive = None;
            self.offered = false;
            self.next_at = now + DELAY;
            if pa2_high {
                // A new request starts: whatever was left is dropped.
                self.rx.clear();
                self.rx_count = 0;
                self.tx.clear();
                self.tx_pos = 0;
                self.cancel_busy();
            }
        }
        if self.c64_sends {
            if pc2 {
                self.receive(user_port::pb(lines.pulled_low), now);
                self.flag_at = Some(now + DELAY);
            }
        } else {
            if pc2 && self.offered {
                self.offered = false;
                self.tx_pos += 1;
                self.next_at = now + DELAY;
            }
            if !self.offered && now >= self.next_at && self.tx_pos < self.tx.len() {
                self.drive = Some(self.tx[self.tx_pos]);
                self.offered = true;
                self.flag_until = now + FLAG_LEN;
            }
        }
        if self.flag_at.is_some_and(|t| now >= t) {
            self.flag_at = None;
            self.flag_until = now + FLAG_LEN;
        }
        if !matches!(self.busy, Busy::None) && now >= self.poll_at {
            self.poll_at = now + POLL;
            self.poll(now);
        }
    }

    fn input(&mut self, _ev: &InputEvent) {}

    fn attach_host(&mut self, host: &HostServices) {
        self.net = host.net.clone();
    }

    fn save_state(&self) -> Vec<u8> {
        let mut w = StateWriter::new();
        w.chunk(*b"WIC6", 1, |w| {
            w.bool(self.c64_sends);
            w.bool(self.drive.is_some());
            w.u8(self.drive.unwrap_or(0));
            w.u64(self.flag_until);
            w.bool(self.flag_at.is_some());
            w.u64(self.flag_at.unwrap_or(0));
            w.bool(self.offered);
            w.u64(self.next_at);
            w.bytes(&self.rx);
            w.u64(self.rx_count as u64);
            w.bytes(&self.tx);
            w.u64(self.tx_pos as u64);
            w.u8(self.proto.to_u8());
            w.u8(self.command);
            self.save_busy(w);
            w.u64(self.poll_at);
            w.str(&self.server);
            w.str(&self.post_url);
            w.bool(self.timezone.is_some());
            w.u8(self.timezone.unwrap_or(0));
            w.u8(self.remote_timeout);
            w.str(&self.message);
            w.bool(self.tcp.is_some());
            w.u32(self.tcp.unwrap_or(0));
            w.bytes(&self.tcp_buf);
            w.bool(self.tcp_closed);
            w.u64(self.now);
        });
        w.into_bytes()
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), StateError> {
        let (_, mut c) = StateReader::new(s).chunk(*b"WIC6", 1)?;
        let mut n = Wic64::new();
        n.c64_sends = c.bool()?;
        let has = c.bool()?;
        let b = c.u8()?;
        n.drive = has.then_some(b);
        n.flag_until = c.u64()?;
        let has = c.bool()?;
        let t = c.u64()?;
        n.flag_at = has.then_some(t);
        n.offered = c.bool()?;
        n.next_at = c.u64()?;
        n.rx = c.bytes()?.to_vec();
        n.rx_count = c.u64()? as usize;
        n.tx = c.bytes()?.to_vec();
        n.tx_pos = c.u64()? as usize;
        if n.rx.len() > MAX_REQUEST || n.rx_count < n.rx.len() || n.tx_pos > n.tx.len() {
            return Err(StateError::Invalid("WiC64 buffers"));
        }
        n.proto = Protocol::from_u8(c.u8()?).ok_or(StateError::Invalid("WiC64 protocol"))?;
        n.command = c.u8()?;
        n.busy = Self::load_busy(&mut c)?;
        n.poll_at = c.u64()?;
        n.server = c.string()?;
        n.post_url = c.string()?;
        let has = c.bool()?;
        let tz = c.u8()?;
        if usize::from(tz) >= TIMEZONES.len() {
            return Err(StateError::Invalid("WiC64 time zone"));
        }
        n.timezone = has.then_some(tz);
        n.remote_timeout = c.u8()?;
        n.message = c.string()?;
        let has = c.bool()?;
        let h = c.u32()?;
        n.tcp = has.then_some(h);
        n.tcp_buf = c.bytes()?.to_vec();
        n.tcp_closed = c.bool()?;
        n.now = c.u64()?;
        // Host handles open before the load are not referenced any more.
        self.close_tcp();
        self.cancel_busy();
        n.net = self.net.take();
        *self = n;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
