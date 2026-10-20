//! Preview server: a minimal HTTP/1.1 front end on tokio with WebSocket
//! upgrade (tokio-tungstenite), loopback only.
//!
//! | Request                | Answer                                                  |
//! | ---------------------- | ------------------------------------------------------- |
//! | `GET /`                | a viewer page (live stream in the browser)              |
//! | `GET /snapshot.png`    | the latest frame as PNG                                 |
//! | `GET /preview` (WS)    | stream of [`crate::stream_message`] binary messages     |
//!
//! Parameters (query string): `filter=raw|crt` (default raw), `profile=`
//! (CRT monitor profile, default the machine's), `scale=1..4` (raw: pixel
//! factor, crt: rows per line), and for the stream `fps=1..50` (default
//! 25), `format=png|qoi` (default png), `delta=0|1` (default 1). The
//! snapshot also takes `machine=<id>` (404 if it is not this machine) and
//! reports the beam in `X-Beam-Frame/Line/Cycle` and `X-Paused` headers.
//!
//! Viewers may send input back over the stream as JSON text messages
//! (`{"type": "key", "code": "KeyA", "down": true}`, `{"type": "button",
//! "pad": 0, "button": "South", "down": true}`, `{"type": "axis", "pad": 0,
//! "axis": "LeftX", "value": -0.5}`). With an [`InputSink`] they reach the
//! frontend as [`RemoteInput`], which maps them like local host input;
//! without one they are ignored.
//!
//! Frames reach the server through a [`FrameTap`]: the emulation thread
//! publishes copies into a watch channel and never waits. Each connection
//! renders and encodes the newest frame on the blocking pool, at its own
//! rate; frames that arrive meanwhile are skipped (backpressure drops
//! frames, never the emulator's time).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use video_crt::Profile;

use emu_core::Machine;

use crate::{dirty_rect, stream_message, Filter, Format, OwnedFrame, Rect, RgbImage};

const VIEWER: &str = include_str!("viewer.html");
/// Longest accepted request head.
const MAX_HEAD: usize = 8 * 1024;
/// A delta covering more than this share of the picture is sent as a key
/// frame instead.
const KEY_THRESHOLD: u64 = 2;

/// The emulation side: publish frame copies here.
#[derive(Clone)]
pub struct FrameTap {
    tx: Arc<watch::Sender<Option<Arc<OwnedFrame>>>>,
}

impl Default for FrameTap {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTap {
    pub fn new() -> Self {
        Self {
            tx: Arc::new(watch::channel(None).0),
        }
    }

    /// Copy the machine's current frame for the preview. Never blocks.
    pub fn publish(&self, m: &dyn Machine, running: bool) {
        let frame = Arc::new(OwnedFrame::from_machine(m, running));
        self.tx.send_replace(Some(frame));
    }

    fn latest(&self) -> Option<Arc<OwnedFrame>> {
        self.tx.borrow().clone()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewConfig {
    pub bind: SocketAddr,
}

/// Input from a viewer, in host terms: W3C `KeyboardEvent.code` key names
/// and layout-neutral gamepad buttons and axes (`South`, `LeftX` …).
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteInput {
    Key {
        code: String,
        down: bool,
    },
    Button {
        pad: u8,
        button: String,
        down: bool,
    },
    /// -1.0 ..= 1.0; positive is right or down.
    Axis {
        pad: u8,
        axis: String,
        value: f32,
    },
}

/// Where viewer input goes (the frontend's host input mapping).
pub type InputSink = Arc<dyn Fn(RemoteInput) + Send + Sync>;

/// Longest accepted input message.
const MAX_INPUT_MESSAGE: usize = 1024;

fn short_name(v: &serde_json::Value, field: &str) -> Option<String> {
    let s = v[field].as_str()?;
    (!s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| s.to_string())
}

/// Parse one viewer input message; `None` if malformed.
pub fn parse_remote_input(text: &str) -> Option<RemoteInput> {
    if text.len() > MAX_INPUT_MESSAGE {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let pad = || v["pad"].as_u64().and_then(|p| u8::try_from(p).ok());
    Some(match v["type"].as_str()? {
        "key" => RemoteInput::Key {
            code: short_name(&v, "code")?,
            down: v["down"].as_bool()?,
        },
        "button" => RemoteInput::Button {
            pad: pad()?,
            button: short_name(&v, "button")?,
            down: v["down"].as_bool()?,
        },
        "axis" => RemoteInput::Axis {
            pad: pad()?,
            axis: short_name(&v, "axis")?,
            value: (v["value"].as_f64()? as f32).clamp(-1.0, 1.0),
        },
        _ => return None,
    })
}

pub struct PreviewServer {
    addr: SocketAddr,
    stop: watch::Sender<bool>,
    thread: JoinHandle<()>,
}

impl PreviewServer {
    /// Bind (loopback only) and serve frames from `tap` on a thread of its own.
    /// Bind (loopback only) and serve frames from `tap` on a thread of its
    /// own; viewer input goes to `input`, if given.
    pub fn start(
        config: PreviewConfig,
        machine: &str,
        tap: FrameTap,
        input: Option<InputSink>,
    ) -> io::Result<Self> {
        if !config.bind.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "preview server binds to loopback only",
            ));
        }
        let listener = std::net::TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let (stop, stop_rx) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let ctx = Arc::new(Context {
            machine: machine.to_string(),
            tap,
            input,
        });
        let thread = thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = TcpListener::from_std(listener) else {
                        return;
                    };
                    accept_loop(listener, ctx, stop_rx).await;
                });
            })?;
        Ok(Self { addr, stop, thread })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.thread.join();
    }
}

struct Context {
    machine: String,
    tap: FrameTap,
    input: Option<InputSink>,
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Context>, mut stop: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                tokio::spawn(connection(stream, ctx.clone()));
            }
            _ = stop.changed() => return,
        }
    }
}

/// A parsed request head.
struct Head {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    ws_key: Option<String>,
}

impl Head {
    fn param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn parse_head(text: &str) -> Option<Head> {
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    let mut upgrade = false;
    let mut ws_key = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade = value.trim().eq_ignore_ascii_case("websocket"),
            "sec-websocket-key" => ws_key = Some(value.trim().to_string()),
            _ => {}
        }
    }
    Some(Head {
        method,
        path: path.to_string(),
        query,
        ws_key: if upgrade { ws_key } else { None },
    })
}

async fn read_head(stream: &mut TcpStream) -> Option<Head> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = async {
        loop {
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                return parse_head(std::str::from_utf8(&buf[..end]).ok()?);
            }
            if buf.len() > MAX_HEAD {
                return None;
            }
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .ok()
        .flatten()
}

async fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    if stream.write_all(head.as_bytes()).await.is_ok() {
        let _ = stream.write_all(body).await;
    }
    let _ = stream.shutdown().await;
}

async fn error(stream: &mut TcpStream, status: &str, msg: &str) {
    let headers = [("Content-Type", "text/plain; charset=utf-8".to_string())];
    respond(stream, status, &headers, msg.as_bytes()).await;
}

/// Stream and snapshot settings from the query string.
struct Settings {
    filter: Filter,
    fps: u32,
    format: Format,
    delta: bool,
}

fn settings(head: &Head, machine: &str) -> Result<Settings, String> {
    let scale = match head.param("scale") {
        Some(s) => s
            .parse::<u32>()
            .ok()
            .filter(|s| (1..=4).contains(s))
            .ok_or("scale must be 1–4")?,
        None => 0,
    };
    let filter = match head.param("filter").unwrap_or("raw") {
        "raw" => Filter::Raw {
            scale: scale.max(1),
        },
        "crt" => {
            let profile = match head.param("profile") {
                Some(name) => Profile::preset(name).ok_or(format!("unknown profile '{name}'"))?,
                None => Profile::default_for(machine),
            };
            Filter::Crt {
                profile,
                scale: if scale == 0 { 2 } else { scale },
            }
        }
        other => return Err(format!("unknown filter '{other}'")),
    };
    let fps = match head.param("fps") {
        Some(s) => s
            .parse::<u32>()
            .ok()
            .filter(|f| (1..=50).contains(f))
            .ok_or("fps must be 1–50")?,
        None => 25,
    };
    let format = match head.param("format").unwrap_or("png") {
        "png" => Format::Png,
        "qoi" => Format::Qoi,
        other => return Err(format!("unknown format '{other}'")),
    };
    let delta = match head.param("delta").unwrap_or("1") {
        "1" => true,
        "0" => false,
        other => return Err(format!("delta must be 0 or 1, not '{other}'")),
    };
    Ok(Settings {
        filter,
        fps,
        format,
        delta,
    })
}

async fn connection(mut stream: TcpStream, ctx: Arc<Context>) {
    let _ = stream.set_nodelay(true);
    let Some(head) = read_head(&mut stream).await else {
        return;
    };
    if head.method != "GET" {
        return error(&mut stream, "405 Method Not Allowed", "GET only\n").await;
    }
    let settings = match settings(&head, &ctx.machine) {
        Ok(s) => s,
        Err(msg) => return error(&mut stream, "400 Bad Request", &format!("{msg}\n")).await,
    };
    match head.path.as_str() {
        "/" => {
            let headers = [("Content-Type", "text/html; charset=utf-8".to_string())];
            respond(&mut stream, "200 OK", &headers, VIEWER.as_bytes()).await;
        }
        "/snapshot.png" => {
            if head.param("machine").is_some_and(|m| m != ctx.machine) {
                return error(&mut stream, "404 Not Found", "no such machine\n").await;
            }
            let Some(frame) = ctx.tap.latest() else {
                return error(&mut stream, "503 Service Unavailable", "no frame yet\n").await;
            };
            let filter = settings.filter;
            let f = frame.clone();
            let Ok(png) =
                tokio::task::spawn_blocking(move || Format::Png.encode(&filter.render(&f))).await
            else {
                return;
            };
            let beam = frame.beam.unwrap_or_default();
            let headers = [
                ("Content-Type", "image/png".to_string()),
                ("X-Machine", ctx.machine.clone()),
                ("X-Beam-Frame", beam.frame.to_string()),
                ("X-Beam-Line", beam.line.to_string()),
                ("X-Beam-Cycle", beam.cycle.to_string()),
                ("X-Paused", u8::from(!frame.running).to_string()),
            ];
            respond(&mut stream, "200 OK", &headers, &png).await;
        }
        "/preview" => match head.ws_key.clone() {
            Some(key) => {
                let accept = derive_accept_key(key.as_bytes());
                let reply = format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                );
                if stream.write_all(reply.as_bytes()).await.is_err() {
                    return;
                }
                let ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
                stream_frames(ws, &ctx, settings).await;
            }
            None => error(&mut stream, "426 Upgrade Required", "WebSocket only\n").await,
        },
        _ => error(&mut stream, "404 Not Found", "not found\n").await,
    }
}

/// Send the newest frame whenever one arrives, at most `fps` times a second.
async fn stream_frames(mut ws: WebSocketStream<TcpStream>, ctx: &Context, s: Settings) {
    let mut rx = ctx.tap.tx.subscribe();
    let period = Duration::from_secs(1) / s.fps;
    let mut prev: Option<RgbImage> = None;
    let mut last_sent: Option<Arc<OwnedFrame>> = None;
    loop {
        let started = tokio::time::Instant::now();
        let frame = rx.borrow_and_update().clone();
        if let Some(frame) = frame {
            if !last_sent.as_ref().is_some_and(|l| Arc::ptr_eq(l, &frame)) {
                let filter = s.filter.clone();
                let (format, delta, old) = (s.format, s.delta, prev.take());
                let f = frame.clone();
                let before = last_sent.clone();
                let job = tokio::task::spawn_blocking(move || {
                    let img = filter.render(&f);
                    let full = Rect {
                        x: 0,
                        y: 0,
                        w: img.width,
                        h: img.height,
                    };
                    let msg = match old.as_ref().filter(|_| delta) {
                        None => Some(stream_message(&f, &img, full, true, format)),
                        Some(old) => match dirty_rect(old, &img) {
                            Some(r) => {
                                let big = u64::from(r.w) * u64::from(r.h) * KEY_THRESHOLD
                                    > u64::from(img.width) * u64::from(img.height);
                                Some(stream_message(
                                    &f,
                                    &img,
                                    if big { full } else { r },
                                    big,
                                    format,
                                ))
                            }
                            // Same picture: tell paused viewers about the
                            // beam, and everyone about pause/run changes.
                            None if !f.running
                                || before.is_some_and(|b| b.running != f.running) =>
                            {
                                let none = Rect { w: 0, h: 0, ..full };
                                Some(stream_message(&f, &img, none, false, format))
                            }
                            None => None,
                        },
                    };
                    (img, msg)
                });
                let Ok((img, msg)) = job.await else { return };
                prev = Some(img);
                last_sent = Some(frame);
                if let Some(msg) = msg {
                    if ws.send(Message::binary(msg)).await.is_err() {
                        return;
                    }
                }
            }
        }
        // Wait for the next frame and the rate limit; serve pings and closes.
        let pace = tokio::time::sleep_until(started + period);
        tokio::pin!(pace);
        let mut paced = false;
        let mut fresh = false;
        while !(paced && fresh) {
            tokio::select! {
                _ = &mut pace, if !paced => paced = true,
                changed = rx.changed(), if !fresh => {
                    if changed.is_err() {
                        return;
                    }
                    fresh = true;
                }
                msg = ws.next() => match msg {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(Message::Text(text))) => {
                        if let (Some(sink), Some(ev)) = (&ctx.input, parse_remote_input(&text)) {
                            sink(ev);
                        }
                    }
                    Some(Ok(_)) => {}
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_heads() {
        let h = parse_head(
            "GET /preview?fps=10&format=qoi HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc==",
        )
        .unwrap();
        assert_eq!((h.method.as_str(), h.path.as_str()), ("GET", "/preview"));
        assert_eq!(h.param("fps"), Some("10"));
        assert_eq!(h.ws_key.as_deref(), Some("abc=="));
        let h = parse_head("GET /snapshot.png HTTP/1.1\r\nSec-WebSocket-Key: abc==").unwrap();
        assert_eq!(h.ws_key, None, "a key without upgrade is no WebSocket");
        assert!(parse_head("").is_none());
    }

    #[test]
    fn viewer_input_messages() {
        assert_eq!(
            parse_remote_input(r#"{"type":"key","code":"KeyA","down":true}"#),
            Some(RemoteInput::Key {
                code: "KeyA".into(),
                down: true
            })
        );
        assert_eq!(
            parse_remote_input(r#"{"type":"axis","pad":1,"axis":"LeftY","value":3}"#),
            Some(RemoteInput::Axis {
                pad: 1,
                axis: "LeftY".into(),
                value: 1.0
            })
        );
        for bad in [
            r#"{"type":"key","code":"","down":true}"#,
            r#"{"type":"key","code":"a b","down":true}"#,
            r#"{"type":"button","pad":999,"button":"South","down":true}"#,
            r#"{"type":"jump"}"#,
            "not json",
        ] {
            assert_eq!(parse_remote_input(bad), None, "{bad}");
        }
    }

    #[test]
    fn settings_are_validated() {
        let head = |q: &str| parse_head(&format!("GET /preview?{q} HTTP/1.1")).unwrap();
        let s = settings(&head(""), "c64").unwrap();
        assert_eq!((s.fps, s.format, s.delta), (25, Format::Png, true));
        assert_eq!(s.filter, Filter::Raw { scale: 1 });
        let s = settings(&head("filter=crt&profile=1702&scale=3"), "c64").unwrap();
        assert!(matches!(s.filter, Filter::Crt { scale: 3, .. }));
        for bad in [
            "fps=0",
            "fps=51",
            "scale=9",
            "format=gif",
            "filter=blur",
            "delta=2",
            "filter=crt&profile=x",
        ] {
            assert!(settings(&head(bad), "c64").is_err(), "{bad}");
        }
    }
}
