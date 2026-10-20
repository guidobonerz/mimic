//! Live preview end to end: viewer page, /snapshot.png and the /preview
//! WebSocket stream (key frame, deltas, the paused flag while a debugger
//! holds the machine), fed through the session's frame hook.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use emu_core::registry::MachineConfig;
use emu_debug::{DebugServer, ServerConfig, Session};
use emu_preview::server::{FrameTap, PreviewConfig, PreviewServer};
use emu_preview::{HEADER_LEN, MAGIC};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::{Message, WebSocket};

struct Setup {
    debug: DebugServer,
    preview: PreviewServer,
}

fn start() -> Setup {
    let machine = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    let mut session = Session::new(machine, "sim6502");
    let tap = FrameTap::new();
    tap.publish(session.machine(), true);
    let hook = tap.clone();
    session.set_frame_hook(Box::new(move |m, running| hook.publish(m, running)));
    let preview = PreviewServer::start(
        PreviewConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
        },
        "sim6502",
        tap,
    )
    .unwrap();
    let debug = DebugServer::start(
        session,
        ServerConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            vice: None,
            run: true,
            realtime: true,
        },
    )
    .unwrap();
    Setup { debug, preview }
}

/// Raw HTTP GET: status line, headers (lower-case names), body.
fn get(addr: SocketAddr, path: &str) -> (String, Vec<(String, String)>, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
    let mut data = Vec::new();
    s.read_to_end(&mut data).unwrap();
    let end = data.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(data[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    (status, headers, data[end + 4..].to_vec())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    &headers.iter().find(|(k, _)| k == name).unwrap().1
}

fn ws_connect(addr: SocketAddr, query: &str) -> WebSocket<TcpStream> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET /preview?{query} HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.starts_with("HTTP/1.1 101"), "{line}");
    let mut accept = None;
    loop {
        let mut l = String::new();
        reader.read_line(&mut l).unwrap();
        if l == "\r\n" {
            break;
        }
        if let Some(v) = l.strip_prefix("Sec-WebSocket-Accept: ") {
            accept = Some(v.trim().to_string());
        }
    }
    // The example key and accept value from RFC 6455, section 1.3.
    assert_eq!(accept.as_deref(), Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    WebSocket::from_raw_socket(s, Role::Client, None)
}

/// A decoded stream message: header fields and the RGB image (if any).
struct Msg {
    key: bool,
    paused: bool,
    full: (u32, u32),
    rect: (u32, u32, u32, u32),
    rgb: Vec<u8>,
}

fn next(ws: &mut WebSocket<TcpStream>) -> Msg {
    loop {
        let Message::Binary(data) = ws.read().unwrap() else {
            continue;
        };
        assert_eq!(&data[..4], MAGIC);
        let u16_at = |i: usize| u32::from(u16::from_le_bytes([data[i], data[i + 1]]));
        assert_eq!(data[6], 1, "qoi");
        let rgb = if data.len() > HEADER_LEN {
            qoi::decode_to_vec(&data[HEADER_LEN..]).unwrap().1
        } else {
            Vec::new()
        };
        return Msg {
            key: data[5] == 0,
            paused: data[7] & 1 != 0,
            full: (u16_at(20), u16_at(22)),
            rect: (u16_at(24), u16_at(26), u16_at(28), u16_at(30)),
            rgb,
        };
    }
}

/// Native protocol call (one line in, response line out).
fn debug(addr: SocketAddr, cmds: &[&str]) {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    for (i, cmd) in cmds.iter().enumerate() {
        let req = cmd.replacen('{', &format!("{{\"id\":{i},"), 1);
        writeln!(w, "{req}").unwrap();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.contains("\"id\"") {
                assert!(line.contains("\"ok\":true"), "{cmd}: {line}");
                break;
            }
        }
    }
}

#[test]
fn snapshot_viewer_and_errors() {
    let s = start();
    let addr = s.preview.addr();
    let (status, headers, body) = get(addr, "/snapshot.png?machine=sim6502");
    assert!(status.contains("200"), "{status}");
    assert_eq!(header(&headers, "content-type"), "image/png");
    assert_eq!(&body[..8], b"\x89PNG\r\n\x1a\n");
    let decoder = png::Decoder::new(body.as_slice());
    let info = decoder.read_info().unwrap();
    assert_eq!((info.info().width, info.info().height), (384, 272));

    let (status, headers, body) = get(addr, "/snapshot.png?filter=crt&profile=1702&scale=2");
    assert!(status.contains("200"), "{status}");
    assert_eq!(header(&headers, "x-machine"), "sim6502");
    assert!(body.len() > 8);

    let (status, headers, body) = get(addr, "/");
    assert!(status.contains("200"));
    assert!(header(&headers, "content-type").starts_with("text/html"));
    assert!(String::from_utf8(body).unwrap().contains("/preview"));

    assert!(get(addr, "/snapshot.png?machine=c64").0.contains("404"));
    assert!(get(addr, "/nothing").0.contains("404"));
    assert!(get(addr, "/snapshot.png?scale=9").0.contains("400"));
    assert!(
        get(addr, "/preview").0.contains("426"),
        "plain GET on the stream"
    );
    s.preview.shutdown();
    s.debug.shutdown();
}

#[test]
fn stream_sends_key_frame_then_deltas_and_the_paused_state() {
    let s = start();
    let mut ws = ws_connect(s.preview.addr(), "format=qoi&fps=50");
    let first = next(&mut ws);
    assert!(first.key);
    assert_eq!(first.full, (384, 272));
    assert_eq!(first.rgb.len(), 384 * 272 * 3);
    let mut picture = first.rgb;

    // Stop the machine, put a byte into the bitmap, draw one frame.
    debug(
        s.debug.addr(),
        &[
            r#"{"cmd":"pause"}"#,
            r#"{"cmd":"poke","addr":8192,"bytes":[255]}"#,
            r#"{"cmd":"step_frame"}"#,
        ],
    );
    let mut paused_seen = false;
    let mut changed = false;
    for _ in 0..20 {
        let m = next(&mut ws);
        if !m.paused {
            continue;
        }
        paused_seen = true;
        let (x, y, w, h) = m.rect;
        if m.rgb.is_empty() {
            continue;
        }
        assert!(!m.key, "a small change is a delta");
        for row in 0..h {
            let dst = ((y + row) * 384 + x) as usize * 3;
            let src = (row * w) as usize * 3;
            picture[dst..dst + w as usize * 3].copy_from_slice(&m.rgb[src..src + w as usize * 3]);
        }
        changed = true;
        break;
    }
    assert!(paused_seen, "paused flag reaches the stream");
    assert!(changed, "the poked bitmap byte arrives as a delta");
    // The bitmap window starts at cycle 4 (x = 32) of line 36; $FF sets the
    // first 8 pixels to the foreground colour, unlike the pixel after them.
    let px = |x: usize, y: usize| &picture[(y * 384 + x) * 3..(y * 384 + x) * 3 + 3];
    assert_eq!(px(32, 36), px(39, 36));
    assert_ne!(px(32, 36), px(40, 36));
    s.preview.shutdown();
    s.debug.shutdown();
}
