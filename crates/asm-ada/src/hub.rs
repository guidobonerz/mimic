//! `ada-asm hub`: the virtual network the Logophor 16s meet in (SPEC.md
//! 10.1 and 11.3). A UDP relay on the host: every machine started with
//! `--option net=HOST:PORT` sends it its frames and a hello every 2
//! seconds; the hub sends each frame on to every other machine it has
//! heard from in the last 30 seconds, like an Ethernet hub.
//!
//! The hub is also the net's gateway ([`gateway::Gateway`], 10.16.0.1):
//! it sees every frame, answers ARP, ping, DHCP and DNS and carries UDP
//! and TCP to the host's network ([`outside::HostOutside`]); its frames
//! go to every machine. `ada-asm hub --no-gateway` is the plain relay.
//!
//! Datagrams: `LGN1`, then 0 and a frame with its FCS, or 1 (hello).

pub mod dhcp;
pub mod dns;
pub mod gateway;
pub mod outside;
pub mod packet;
pub mod tcp;

#[cfg(test)]
mod gateway_tests;

use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use gateway::Gateway;
pub use outside::{HostOutside, Offline, Outside};
pub use packet::fcs;

pub const DEFAULT_PORT: u16 = 6516;
const MAGIC: &[u8; 4] = b"LGN1";
const FORGET: Duration = Duration::from_secs(30);

/// A frame (without FCS) as a tunnel datagram: `LGN1`, 0, frame, FCS.
pub fn datagram(frame: &[u8]) -> Vec<u8> {
    let mut d = Vec::with_capacity(frame.len() + 9);
    d.extend_from_slice(MAGIC);
    d.push(0);
    d.extend_from_slice(frame);
    d.extend_from_slice(&fcs(frame));
    d
}

/// The hub with its gateway on `socket` until `stop` is set; `log` gets
/// a line per new or forgotten machine and the gateway's news.
pub fn run(
    socket: &UdpSocket,
    stop: &AtomicBool,
    log: &mut dyn FnMut(String),
) -> std::io::Result<()> {
    run_with(socket, stop, true, log)
}

/// The hub, with the gateway (`gateway`) or as the plain relay.
pub fn run_with(
    socket: &UdpSocket,
    stop: &AtomicBool,
    gateway: bool,
    log: &mut dyn FnMut(String),
) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_millis(10)))?;
    let mut peers: BTreeMap<SocketAddr, Instant> = BTreeMap::new();
    let mut gw = gateway.then(|| (Gateway::new(), HostOutside::new()));
    let start = Instant::now();
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        let mut answers = Vec::new();
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let now = Instant::now();
                peers.retain(|a, seen| {
                    let keep = now.duration_since(*seen) < FORGET;
                    if !keep {
                        log(format!("{a} left"));
                    }
                    keep
                });
                let d = &buf[..n];
                if n >= 5 && &d[..4] == MAGIC {
                    if peers.insert(from, now).is_none() {
                        log(format!("{from} joined ({} on the net)", peers.len()));
                    }
                    if d[4] == 0 {
                        for to in peers.keys().filter(|a| **a != from) {
                            let _ = socket.send_to(d, to);
                        }
                        // The gateway takes frames with a good FCS only.
                        if let Some((g, out)) = &mut gw {
                            if n >= 5 + 14 + 4 {
                                let (frame, f) = d[5..].split_at(n - 9);
                                if fcs(frame) == f {
                                    let ms = start.elapsed().as_millis() as u64;
                                    answers = g.frame(frame, ms, out);
                                }
                            }
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            // Windows reports a peer that went away on the next receive.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e),
        }
        if let Some((g, out)) = &mut gw {
            answers.extend(g.poll(start.elapsed().as_millis() as u64, out));
            for line in g.take_log() {
                log(line);
            }
            for f in answers {
                let d = datagram(&f);
                for to in peers.keys() {
                    let _ = socket.send_to(&d, to);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod relay_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn frames_reach_every_other_machine() {
        let hub = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = hub.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let t = std::thread::spawn(move || run(&hub, &s, &mut |_| {}).unwrap());
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        for x in [&a, &b, &c] {
            x.connect(addr).unwrap();
            x.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            x.send(b"LGN1\x01").unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        a.send(b"LGN1\x00frame").unwrap();
        let mut buf = [0u8; 64];
        for x in [&b, &c] {
            let n = x.recv(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"LGN1\x00frame");
        }
        a.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        assert!(a.recv(&mut buf).is_err(), "not back to the sender");
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }
}
