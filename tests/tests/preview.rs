//! Live preview end to end: viewer page, /snapshot.png and the /preview
//! WebSocket stream (key frame, deltas, the paused flag while a debugger
//! holds the machine), fed through the session's frame hook.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use emu_core::registry::MachineConfig;
use emu_debug::{DebugServer, ServerConfig, Session};
use std::sync::{Arc, Mutex};

use emu_core::AudioFormat;
use emu_preview::raw::{self, RAW_VERSION, VIDEO_HEADER_LEN};
use emu_preview::server::{FrameTap, InputSink, PreviewConfig, PreviewServer, RemoteInput};
use emu_preview::{HEADER_LEN, MAGIC};

const AUDIO_MARK: i16 = 1234;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::{Message, WebSocket};

struct Setup {
    debug: DebugServer,
    preview: PreviewServer,
    /// Viewer input received by the preview server.
    inputs: Arc<Mutex<Vec<RemoteInput>>>,
}

fn start() -> Setup {
    let machine = emu_machines::registry()
        .create("sim6502", &MachineConfig::default())
        .unwrap();
    let mut session = Session::new(machine, "sim6502");
    let tap = FrameTap::new();
    tap.publish(session.machine(), true);
    let hook = tap.clone();
    // sim6502 is silent: every frame contributes a marker sample instead.
    let format = AudioFormat {
        sample_rate: 48_000,
        channels: 1,
    };
    session.set_frame_hook(Box::new(move |m, running| {
        hook.publish(m, running);
        hook.publish_audio(&[AUDIO_MARK, -AUDIO_MARK], format);
    }));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let seen = inputs.clone();
    let sink: InputSink = Arc::new(move |ev| seen.lock().unwrap().push(ev));
    let preview = PreviewServer::start(
        PreviewConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
        },
        "sim6502",
        tap,
        Some(sink),
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
    Setup {
        debug,
        preview,
        inputs,
    }
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
    ws_open(addr, &format!("/preview?{query}"))
}

fn ws_open(addr: SocketAddr, target: &str) -> WebSocket<TcpStream> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {target} HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
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
    // Follow the stream like a viewer: key frames replace the picture,
    // deltas patch it. The bitmap window starts at cycle 4 (x = 32) of
    // line 36; $FF sets its first 8 pixels to the foreground colour.
    let px = |p: &[u8], x: usize, y: usize| p[(y * 384 + x) * 3..(y * 384 + x) * 3 + 3].to_vec();
    let shows_byte = |p: &[u8]| px(p, 32, 36) == px(p, 39, 36) && px(p, 32, 36) != px(p, 40, 36);
    let mut paused_delta = false;
    let mut done = false;
    for _ in 0..50 {
        let m = next(&mut ws);
        let (x, y, w, h) = m.rect;
        if m.key {
            picture = m.rgb.clone();
        } else {
            for row in 0..h {
                let dst = ((y + row) * 384 + x) as usize * 3;
                let src = (row * w) as usize * 3;
                picture[dst..dst + w as usize * 3]
                    .copy_from_slice(&m.rgb[src..src + w as usize * 3]);
            }
        }
        paused_delta |= m.paused && !m.key && !m.rgb.is_empty();
        if m.paused && shows_byte(&picture) {
            done = true;
            break;
        }
    }
    assert!(
        done,
        "the poked bitmap byte reaches the viewer while paused"
    );
    assert!(paused_delta, "small changes arrive as deltas");

    // Viewer input travels back over the same socket.
    ws.send(Message::text(
        r#"{"type":"key","code":"Space","down":true}"#,
    ))
    .unwrap();
    ws.send(Message::text(r#"{"type":"bogus"}"#)).unwrap();
    ws.send(Message::text(
        r#"{"type":"button","pad":0,"button":"South","down":false}"#,
    ))
    .unwrap();
    let mut got = Vec::new();
    for _ in 0..200 {
        got = s.inputs.lock().unwrap().clone();
        if got.len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        got,
        [
            RemoteInput::Key {
                code: "Space".into(),
                down: true
            },
            RemoteInput::Button {
                pad: 0,
                button: "South".into(),
                down: false
            }
        ]
    );
    s.preview.shutdown();
    s.debug.shutdown();
}

#[test]
fn raw_stream_sends_indices_changed_lines_and_audio() {
    let s = start();
    let addr = s.preview.addr();
    assert!(get(addr, "/stream").0.contains("426"));
    assert!(get(addr, "/stream?audio=2").0.contains("400"));
    let mut ws = ws_open(addr, "/stream?audio=1");

    // Follow the stream like a viewer: indices per pixel, runs patch lines.
    let (w, h) = (384usize, 272usize);
    let mut picture = vec![0u8; w * h];
    let mut key = false;
    let mut audio = false;
    let mut paused_runs = None;
    let apply = |data: &[u8], picture: &mut Vec<u8>| -> (u8, usize) {
        let u16_at = |i: usize| usize::from(u16::from_le_bytes([data[i], data[i + 1]]));
        assert_eq!(
            (data[4], data[6]),
            (RAW_VERSION, 4),
            "sim6502: 16 colours, 4 bpp"
        );
        assert_eq!((u16_at(20), u16_at(22)), (w, h));
        let mut o = VIDEO_HEADER_LEN + u16_at(24) * 3;
        let runs = u16_at(26);
        for _ in 0..runs {
            let (first, count) = (u16_at(o), u16_at(o + 2));
            o += 4;
            for y in first..first + count {
                for x in 0..w {
                    let b = data[o + x / 2];
                    picture[y * w + x] = if x % 2 == 0 { b & 15 } else { b >> 4 };
                }
                o += w / 2;
            }
        }
        assert_eq!(o, data.len(), "runs fill the message");
        (data[5], runs)
    };
    let next_raw = |ws: &mut WebSocket<TcpStream>| loop {
        if let Message::Binary(data) = ws.read().unwrap() {
            return data.to_vec();
        }
    };
    while !(key && audio) {
        let data = next_raw(&mut ws);
        if &data[..4] == raw::AUDIO_MAGIC {
            assert_eq!(
                &data[16..],
                [AUDIO_MARK.to_le_bytes(), (-AUDIO_MARK).to_le_bytes()].concat()
            );
            audio = true;
        } else {
            assert_eq!(&data[..4], raw::VIDEO_MAGIC);
            let (flags, runs) = apply(&data, &mut picture);
            if !key {
                assert_eq!(
                    flags & (raw::FLAG_KEY | raw::FLAG_PALETTE),
                    raw::FLAG_KEY | raw::FLAG_PALETTE
                );
                assert_eq!(runs, 1, "a key frame is one run over all lines");
                key = true;
            }
        }
    }

    // Same change as for /preview: one bitmap byte while paused.
    debug(
        s.debug.addr(),
        &[
            r#"{"cmd":"pause"}"#,
            r#"{"cmd":"poke","addr":8192,"bytes":[255]}"#,
            r#"{"cmd":"step_frame"}"#,
        ],
    );
    let shows_byte =
        |p: &[u8]| p[36 * w + 32] == p[36 * w + 39] && p[36 * w + 32] != p[36 * w + 40];
    for _ in 0..200 {
        let data = next_raw(&mut ws);
        if &data[..4] != raw::VIDEO_MAGIC {
            continue;
        }
        let (flags, runs) = apply(&data, &mut picture);
        if flags & raw::FLAG_PAUSED != 0 && shows_byte(&picture) {
            paused_runs = Some(runs);
            break;
        }
    }
    assert!(
        paused_runs.is_some(),
        "the poked byte reaches the viewer while paused"
    );
    s.preview.shutdown();
    s.debug.shutdown();
}
