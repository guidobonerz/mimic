//! What the gateway reaches outside the Logophor 16 net: the host's name
//! resolver, UDP and TCP sockets. Everything host-side goes through the
//! [`Outside`] trait so the gateway stays deterministic and tests can use
//! a fake; [`HostOutside`] is the real one with `std::net` non-blocking
//! sockets, [`Offline`] a host without a network.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// How long a TCP connect may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Lookups running at once at most.
const MAX_LOOKUPS: usize = 16;

/// A TCP connection's progress outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connect {
    Pending,
    Open,
    Failed,
}

/// The result of reading a TCP connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpRead {
    /// That many bytes were read into the buffer.
    Data(usize),
    /// Nothing there yet.
    Nothing,
    /// The other side closed its direction (end of file).
    Closed,
    /// The connection broke (reset, error).
    Failed,
}

/// The host side of the gateway. Connections are named by ids the
/// gateway chooses; UDP sockets by the machine's address and port.
pub trait Outside {
    /// Starts looking up an IPv4 address for `name`; the answer comes from
    /// [`Outside::resolved`]. False when no more lookups can be taken now.
    fn resolve(&mut self, id: u32, name: &str) -> bool;
    /// A finished lookup: its id and the address, `None` if the name is
    /// unknown.
    fn resolved(&mut self) -> Option<(u32, Option<Ipv4Addr>)>;

    /// Sends a datagram from the host socket that stands for `machine`
    /// (made on first use).
    fn udp_send(&mut self, machine: SocketAddrV4, to: SocketAddrV4, data: &[u8]);
    /// A datagram that came in: the machine socket it is for, the sender
    /// and the data.
    fn udp_recv(&mut self) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)>;
    /// The machine's socket is no longer needed.
    fn udp_close(&mut self, machine: SocketAddrV4);

    /// Starts connecting connection `id` to `to` (at most
    /// [`CONNECT_TIMEOUT`]).
    fn tcp_connect(&mut self, id: u32, to: SocketAddrV4);
    fn tcp_connect_state(&mut self, id: u32) -> Connect;
    /// Sends what the connection takes now: `Some(bytes taken)` (maybe 0),
    /// `None` when it broke.
    fn tcp_send(&mut self, id: u32, data: &[u8]) -> Option<usize>;
    fn tcp_recv(&mut self, id: u32, buf: &mut [u8]) -> TcpRead;
    /// No more data will be sent (a FIN outside).
    fn tcp_shutdown(&mut self, id: u32);
    /// Forgets the connection and closes it.
    fn tcp_close(&mut self, id: u32);
}

/// A host without a network: names are unknown, connections fail and
/// datagrams vanish.
#[derive(Default)]
pub struct Offline {
    lookups: Vec<u32>,
}

impl Outside for Offline {
    fn resolve(&mut self, id: u32, _name: &str) -> bool {
        self.lookups.push(id);
        true
    }
    fn resolved(&mut self) -> Option<(u32, Option<Ipv4Addr>)> {
        if self.lookups.is_empty() {
            None
        } else {
            Some((self.lookups.remove(0), None))
        }
    }
    fn udp_send(&mut self, _: SocketAddrV4, _: SocketAddrV4, _: &[u8]) {}
    fn udp_recv(&mut self) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)> {
        None
    }
    fn udp_close(&mut self, _: SocketAddrV4) {}
    fn tcp_connect(&mut self, _: u32, _: SocketAddrV4) {}
    fn tcp_connect_state(&mut self, _: u32) -> Connect {
        Connect::Failed
    }
    fn tcp_send(&mut self, _: u32, _: &[u8]) -> Option<usize> {
        None
    }
    fn tcp_recv(&mut self, _: u32, _: &mut [u8]) -> TcpRead {
        TcpRead::Failed
    }
    fn tcp_shutdown(&mut self, _: u32) {}
    fn tcp_close(&mut self, _: u32) {}
}

enum Conn {
    Connecting,
    Open(TcpStream),
    Failed,
}

/// The host's real network: non-blocking `std::net` sockets; lookups
/// (`ToSocketAddrs`) and connects run on short-lived helper threads so
/// the hub never waits for them.
pub struct HostOutside {
    udp: BTreeMap<SocketAddrV4, UdpSocket>,
    tcp: BTreeMap<u32, Conn>,
    lookups: usize,
    lookup_tx: Sender<(u32, Option<Ipv4Addr>)>,
    lookup_rx: Receiver<(u32, Option<Ipv4Addr>)>,
    connect_tx: Sender<(u32, std::io::Result<TcpStream>)>,
    connect_rx: Receiver<(u32, std::io::Result<TcpStream>)>,
    buf: Vec<u8>,
}

impl Default for HostOutside {
    fn default() -> Self {
        Self::new()
    }
}

impl HostOutside {
    pub fn new() -> Self {
        let (lookup_tx, lookup_rx) = channel();
        let (connect_tx, connect_rx) = channel();
        HostOutside {
            udp: BTreeMap::new(),
            tcp: BTreeMap::new(),
            lookups: 0,
            lookup_tx,
            lookup_rx,
            connect_tx,
            connect_rx,
            buf: vec![0; 65536],
        }
    }

    fn finish_connects(&mut self) {
        while let Ok((id, r)) = self.connect_rx.try_recv() {
            // A connection given up meanwhile is dropped (closed) here.
            if let Some(c) = self.tcp.get_mut(&id) {
                *c = match r.and_then(|s| {
                    s.set_nonblocking(true)?;
                    let _ = s.set_nodelay(true);
                    Ok(s)
                }) {
                    Ok(s) => Conn::Open(s),
                    Err(_) => Conn::Failed,
                };
            }
        }
    }
}

impl Outside for HostOutside {
    fn resolve(&mut self, id: u32, name: &str) -> bool {
        if self.lookups >= MAX_LOOKUPS {
            return false;
        }
        self.lookups += 1;
        let tx = self.lookup_tx.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            let ip = (name.as_str(), 0)
                .to_socket_addrs()
                .ok()
                .and_then(|mut it| {
                    it.find_map(|a| match a.ip() {
                        IpAddr::V4(v4) => Some(v4),
                        IpAddr::V6(_) => None,
                    })
                });
            let _ = tx.send((id, ip));
        });
        true
    }

    fn resolved(&mut self) -> Option<(u32, Option<Ipv4Addr>)> {
        let r = self.lookup_rx.try_recv().ok()?;
        self.lookups = self.lookups.saturating_sub(1);
        Some(r)
    }

    fn udp_send(&mut self, machine: SocketAddrV4, to: SocketAddrV4, data: &[u8]) {
        if let std::collections::btree_map::Entry::Vacant(e) = self.udp.entry(machine) {
            let Ok(s) = UdpSocket::bind("0.0.0.0:0") else {
                return;
            };
            if s.set_nonblocking(true).is_err() {
                return;
            }
            e.insert(s);
        }
        if let Some(s) = self.udp.get(&machine) {
            let _ = s.send_to(data, to);
        }
    }

    fn udp_recv(&mut self) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)> {
        for (machine, s) in &self.udp {
            loop {
                match s.recv_from(&mut self.buf) {
                    Ok((n, SocketAddr::V4(from))) => {
                        return Some((*machine, from, self.buf[..n].to_vec()));
                    }
                    Ok(_) => continue,
                    // Windows reports an ICMP "port unreachable" this way.
                    Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                    Err(_) => break,
                }
            }
        }
        None
    }

    fn udp_close(&mut self, machine: SocketAddrV4) {
        self.udp.remove(&machine);
    }

    fn tcp_connect(&mut self, id: u32, to: SocketAddrV4) {
        self.tcp.insert(id, Conn::Connecting);
        let tx = self.connect_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((
                id,
                TcpStream::connect_timeout(&SocketAddr::V4(to), CONNECT_TIMEOUT),
            ));
        });
    }

    fn tcp_connect_state(&mut self, id: u32) -> Connect {
        self.finish_connects();
        match self.tcp.get(&id) {
            Some(Conn::Connecting) => Connect::Pending,
            Some(Conn::Open(_)) => Connect::Open,
            Some(Conn::Failed) | None => Connect::Failed,
        }
    }

    fn tcp_send(&mut self, id: u32, data: &[u8]) -> Option<usize> {
        let Some(Conn::Open(s)) = self.tcp.get_mut(&id) else {
            return None;
        };
        match s.write(data) {
            Ok(n) => Some(n),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Some(0),
            Err(_) => None,
        }
    }

    fn tcp_recv(&mut self, id: u32, buf: &mut [u8]) -> TcpRead {
        let Some(Conn::Open(s)) = self.tcp.get_mut(&id) else {
            return TcpRead::Failed;
        };
        match s.read(buf) {
            Ok(0) => TcpRead::Closed,
            Ok(n) => TcpRead::Data(n),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                TcpRead::Nothing
            }
            Err(_) => TcpRead::Failed,
        }
    }

    fn tcp_shutdown(&mut self, id: u32) {
        if let Some(Conn::Open(s)) = self.tcp.get(&id) {
            let _ = s.shutdown(Shutdown::Write);
        }
    }

    fn tcp_close(&mut self, id: u32) {
        self.tcp.remove(&id);
    }
}
