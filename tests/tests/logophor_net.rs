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
