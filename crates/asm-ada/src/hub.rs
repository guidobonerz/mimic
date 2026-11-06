//! `ada-asm hub`: the virtual network the Logophor 16s meet in (SPEC.md
//! 10.1). A UDP relay on the host: every machine started with
//! `--option net=HOST:PORT` sends it its frames and a hello every 2
//! seconds; the hub sends each frame on to every other machine it has
//! heard from in the last 30 seconds, like an Ethernet hub.
//!
//! Datagrams: `LGN1`, then 0 and a frame with its FCS, or 1 (hello).

use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: u16 = 6516;
const MAGIC: &[u8; 4] = b"LGN1";
const FORGET: Duration = Duration::from_secs(30);

/// Relay on `socket` until `stop` is set; `log` gets a line per new or
/// forgotten machine.
pub fn run(
    socket: &UdpSocket,
    stop: &AtomicBool,
    log: &mut dyn FnMut(String),
) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut peers: BTreeMap<SocketAddr, Instant> = BTreeMap::new();
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            // Windows reports a peer that went away on the next receive.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => continue,
            Err(e) => return Err(e),
        };
        let now = Instant::now();
        peers.retain(|a, seen| {
            let keep = now.duration_since(*seen) < FORGET;
            if !keep {
                log(format!("{a} left"));
            }
            keep
        });
        let d = &buf[..n];
        if n < 5 || &d[..4] != MAGIC {
            continue;
        }
        if peers.insert(from, now).is_none() {
            log(format!("{from} joined ({} on the net)", peers.len()));
        }
        if d[4] == 0 {
            for to in peers.keys().filter(|a| **a != from) {
                let _ = socket.send_to(d, to);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
