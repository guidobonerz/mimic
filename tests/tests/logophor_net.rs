//! Phase 31, step 10: two Logophor 16s chat through `ada-asm hub` over
//! real UDP sockets on this host (the native network backend of
//! `media-backends`, as the frontends use it).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use emu_core::host::HostServices;
use emu_core::input::{InputEvent, InputTarget, MachineKey, TimedInput};
use emu_core::media::{Media, MediaBackend, MediaError, MediaKind, SlotId};
use emu_core::registry::MachineConfig;
use emu_core::{DebugHooks, Machine, MemSpaceId, Size, StopReason};
use machine_logophor16::Logophor;

struct Mem(Vec<u8>);

impl MediaBackend for Mem {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&mut self, o: u64, b: &mut [u8]) -> Result<(), MediaError> {
        b.copy_from_slice(&self.0[o as usize..o as usize + b.len()]);
        Ok(())
    }
    fn write(&mut self, o: u64, d: &[u8]) -> Result<(), MediaError> {
        self.0[o as usize..o as usize + d.len()].copy_from_slice(d);
        Ok(())
    }
    fn commit(&mut self) -> Result<(), MediaError> {
        Ok(())
    }
}

fn frames(m: &mut Logophor, n: usize) {
    let mut dbg = DebugHooks::new();
    for _ in 0..n {
        assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
    }
}

fn screen(m: &mut Logophor) -> String {
    let mut s = String::new();
    for a in 0..3700u32 {
        let t = m.debug_target(machine_logophor16::CPU).unwrap();
        let c = t.peek(MemSpaceId(0), 0x1_1000 + 2 * a, Size::Byte).unwrap() as u8;
        s.push(c as char);
        if a % 100 == 99 {
            s.push('\n');
        }
    }
    s
}

fn key(m: &mut Logophor, usage: u16, down: bool) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Keyboard,
        event: InputEvent::Key {
            machine_key: MachineKey(usage),
            down,
        },
    });
}

/// Type lower-case letters, spaces and Enter.
fn type_text(m: &mut Logophor, text: &str) {
    for ch in text.chars() {
        let u = match ch {
            'a'..='z' => 4 + u16::from(ch as u8 - b'a'),
            ' ' => 0x2c,
            '\n' => 0x28,
            _ => panic!("{ch:?}"),
        };
        key(m, u, true);
        key(m, u, false);
        frames(m, 1);
    }
}

#[test]
fn two_logophors_chat_through_the_hub_over_udp() {
    let hub = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = hub.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let t = std::thread::spawn(move || asm_ada::hub::run(&hub, &s, &mut |_| {}).unwrap());

    let src = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/machine-logophor16/asm/examples/chat.s"),
    )
    .unwrap();
    let obj = asm_ada::assemble("chat.s", &src, &asm_ada::HostFiles).unwrap();
    let img = asm_ada::link(&[obj], asm_ada::Options::default()).unwrap();
    let (_, bin) = img.contiguous(0xff, 1 << 20).unwrap();
    let card = asm_ada::sdimage::fat32(64, &[("BOOT/BOILER.BIN".into(), bin)]).unwrap();
    let config = MachineConfig {
        options: [("net".into(), format!("127.0.0.1:{port}"))].into(),
        ..MachineConfig::default()
    };
    let mut ms: Vec<Logophor> = (0..2)
        .map(|_| {
            let mut m = Logophor::new(&config).unwrap();
            m.attach_host(HostServices::new(
                Some(Box::new(media_backends::net::StdNet::new(true))),
                None,
            ));
            let media = Media {
                name: "chat.img".into(),
                kind: MediaKind::Disk,
                backend: Box::new(Mem(card.clone())),
            };
            m.media_slot(SlotId(1)).unwrap().insert(media).unwrap();
            frames(&mut m, 200);
            m
        })
        .collect();
    // The sockets connect on worker threads: wait for both links.
    for _ in 0..200 {
        if ms.iter().all(|m| m.board.net.link()) {
            break;
        }
        for m in &mut ms {
            frames(m, 1);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ms.iter().all(|m| m.board.net.link()), "both links up");
    type_text(&mut ms[0], "ada\n");
    type_text(&mut ms[1], "bab\n");
    // Hellos every 2 s: by now the hub knows both.
    for _ in 0..150 {
        for m in &mut ms {
            frames(m, 1);
        }
    }
    type_text(&mut ms[0], "hello over udp\n");
    let mut seen = false;
    for _ in 0..300 {
        frames(&mut ms[1], 1);
        if screen(&mut ms[1]).contains("ada: hello over udp") {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
    assert!(seen, "{}", screen(&mut ms[1]));
}

fn mouse(m: &mut Logophor, dx: i16, dy: i16, buttons: u8) {
    let at = m.now();
    m.input(TimedInput {
        at,
        target: InputTarget::Port(emu_core::port::PortId(0)),
        event: InputEvent::MouseRel { dx, dy, buttons },
    });
    frames(m, 3);
}

fn point(m: &mut Logophor, from: (i16, i16), to: (i16, i16)) {
    let (mut x, mut y) = from;
    while (x, y) != to {
        let (dx, dy) = ((to.0 - x).clamp(-100, 100), (to.1 - y).clamp(-100, 100));
        mouse(m, dx, dy, 0);
        x += dx;
        y += dy;
    }
}

/// `net=host`: the system card's radio plays its first built-in station
/// (Deutschlandfunk) through the gateway in the machine and the host's
/// real network, without a hub (needs the Internet).
#[test]
#[ignore]
fn net_host_plays_a_real_station() {
    let config = MachineConfig {
        options: [
            ("card".into(), "system".into()),
            ("net".into(), "host".into()),
        ]
        .into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    m.attach_host(HostServices::new(
        Some(Box::new(media_backends::net::StdNet::new(true))),
        None,
    ));
    frames(&mut m, 600);
    point(&mut m, (512, 384), (994, 236));
    mouse(&mut m, 0, 0, 1);
    mouse(&mut m, 0, 0, 0);
    frames(&mut m, 60);
    point(&mut m, (994, 236), (230, 40 + 22 + 28 + 10));
    mouse(&mut m, 0, 0, 1);
    mouse(&mut m, 0, 0, 0);
    let reg = |m: &mut Logophor, off: u32| {
        let t = m.debug_target(machine_logophor16::CPU).unwrap();
        t.peek(MemSpaceId(0), 0xffff_0000 + off, Size::Word)
            .unwrap()
    };
    // up to 30 s of the machine's time for DNS, the redirect and the stream
    let mut decoded = 0;
    for _ in 0..30 {
        frames(&mut m, 60);
        decoded = reg(&mut m, 0x321c) << 16 | reg(&mut m, 0x321e);
        eprintln!(
            "PHSTAT {:04x} rate {} frames {decoded}",
            reg(&mut m, 0x320e),
            reg(&mut m, 0x3210)
        );
        if decoded > 200 {
            break;
        }
    }
    assert!(decoded > 200, "decoded {decoded} frames");
    assert_eq!(reg(&mut m, 0x3210), 4800, "48 kHz (the ARD streams)");
}

/// Development aid: the radio over `net=host` for EMU_SECONDS (default 60)
/// of machine time: per second the decoded frames, the ring's fill, the
/// starved flag and how long the host took (needs the Internet).
#[test]
#[ignore]
fn net_host_radio_watch() {
    let secs: usize = std::env::var("EMU_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let station: usize = std::env::var("EMU_STATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let config = MachineConfig {
        options: [
            ("card".into(), "system".into()),
            ("net".into(), "host".into()),
        ]
        .into(),
        ..MachineConfig::default()
    };
    let mut m = Logophor::new(&config).unwrap();
    m.attach_host(HostServices::new(
        Some(Box::new(media_backends::net::StdNet::new(true))),
        None,
    ));
    frames(&mut m, 600);
    point(&mut m, (512, 384), (994, 236));
    mouse(&mut m, 0, 0, 1);
    mouse(&mut m, 0, 0, 0);
    frames(&mut m, 60);
    let y = 40 + 22 + 28 + 10 + 20 * station as i16;
    point(&mut m, (994, 236), (230, y));
    mouse(&mut m, 0, 0, 1);
    mouse(&mut m, 0, 0, 0);
    let reg = |m: &mut Logophor, off: u32| {
        let t = m.debug_target(machine_logophor16::CPU).unwrap();
        t.peek(MemSpaceId(0), 0xffff_0000 + off, Size::Word)
            .unwrap()
    };
    let mut last = 0;
    let mut audio = Vec::new();
    // EMU_WAV=file.wav: the whole output as 48 kHz stereo 16-bit WAV
    let mut all: Vec<i16> = Vec::new();
    for s in 0..secs {
        let t0 = std::time::Instant::now();
        // a frame at a time, taking the audio as a frontend does (Calliope
        // keeps at most a second of it)
        let mut dbg = DebugHooks::new();
        for _ in 0..60 {
            assert_eq!(m.run_frame(&mut dbg), StopReason::FrameDone);
            m.audio(&mut audio);
            all.extend_from_slice(&audio);
            audio.clear();
        }
        let host_ms = t0.elapsed().as_millis();
        // in real time, as a frontend runs it (60 fields: 998.5 ms)
        if let Some(rest) = std::time::Duration::from_micros(998_500).checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
        let second = &all[all.len().saturating_sub(96_000)..];
        let quiet = second
            .chunks(96)
            .filter(|c| c.iter().all(|v| v.unsigned_abs() < 8))
            .count();
        let decoded = reg(&mut m, 0x321c) << 16 | reg(&mut m, 0x321e);
        let fill = (reg(&mut m, 0x320a) + 16384 - reg(&mut m, 0x320c)) % 16384;
        eprintln!(
            "{s:3}: frames +{:3} ring {fill:5} PHSTAT {:04x} rate {} kbps {} host {host_ms} ms, silent ms {quiet}",
            decoded - last,
            reg(&mut m, 0x320e),
            reg(&mut m, 0x3210),
            reg(&mut m, 0x3214),
        );
        last = decoded;
    }
    if let Ok(path) = std::env::var("EMU_WAV") {
        let data_len = (all.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + all.len() * 2);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&2u16.to_le_bytes()); // stereo
        w.extend_from_slice(&48_000u32.to_le_bytes());
        w.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in &all {
            w.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&path, w).expect("EMU_WAV");
        eprintln!("wrote {path}");
    }
}
