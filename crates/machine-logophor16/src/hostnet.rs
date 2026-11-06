//! `net=host` (SPEC.md 10.1, decision 83): the gateway of `ada-asm hub`
//! (10.16.0.1: ARP, ping, DHCP, DNS, UDP and TCP to the outside) built
//! into the machine, so a single Logophor 16 reaches the Internet without
//! a hub program. The network unit's frames go to the gateway, its
//! answers come back as received frames; the outside is the host's
//! network services (`emu_core::host::NetBackend`, journalled like every
//! host access), never a host socket of the machine's own.
//!
//! The host services take names, not addresses, and have no name lookup
//! of their own, so DNS answers name outside addresses with stand-ins
//! **[S]**: each new name gets the next address of 198.18.0.0/15 (RFC
//! 2544's benchmarking net, never routed), and a connection to a
//! stand-in goes out by its name; addresses typed as numbers go out as
//! they are. Every name gets an address: one that does not exist fails
//! at the connection, not at the lookup. The host services have no half
//! close either: a machine's FIN closes its direction only in the gateway
//! (the host connection stays open until either side closes it).
//!
//! Like the tunnel this is not machine state: a savestate load keeps the
//! gateway and its host connections as they are.

use std::collections::{BTreeMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};

use asm_ada::hub::outside::{Connect, Outside, TcpRead};
use asm_ada::hub::{fcs, Gateway};
use emu_core::host::{ConnState, NetBackend, NetHandle, Recv, SocketKind};

/// The stand-in addresses: 198.18.0.0/15.
const STAND_IN_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);
const STAND_INS: u32 = 1 << 17;

/// The largest datagram taken from a host UDP socket.
const UDP_BUF: usize = 2048;

/// The gateway in the machine and its connections to the host.
#[derive(Default)]
pub(crate) struct HostNet {
    gateway: Gateway,
    tables: Tables,
}

/// What the gateway's outside keeps between calls.
#[derive(Default)]
struct Tables {
    /// Names and their stand-in addresses, both ways.
    names: BTreeMap<String, Ipv4Addr>,
    addrs: BTreeMap<Ipv4Addr, String>,
    next: u32,
    /// Lookups answered, waiting for the gateway to take them.
    resolved: VecDeque<(u32, Option<Ipv4Addr>)>,
    /// A connected host UDP socket per machine address and outside peer.
    udp: BTreeMap<(SocketAddrV4, SocketAddrV4), NetHandle>,
    /// Host TCP connections by the gateway's ids (`None`: refused at once).
    tcp: BTreeMap<u32, Option<NetHandle>>,
}

impl Tables {
    /// The name or number a connection to `ip` goes out by.
    fn host_name(&self, ip: Ipv4Addr) -> String {
        self.addrs
            .get(&ip)
            .cloned()
            .unwrap_or_else(|| ip.to_string())
    }

    fn stand_in(&mut self, name: &str) -> Ipv4Addr {
        if let Some(ip) = self.names.get(name) {
            return *ip;
        }
        // 198.18.0.1 up, round again after 131 071 names
        self.next = self.next % (STAND_INS - 1) + 1;
        let ip = Ipv4Addr::from(STAND_IN_BASE + self.next);
        if let Some(old) = self.addrs.insert(ip, name.to_string()) {
            self.names.remove(&old);
        }
        self.names.insert(name.to_string(), ip);
        ip
    }
}

impl HostNet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Closes every host socket (before the host services change).
    pub(crate) fn close_all(&mut self, backend: &mut dyn NetBackend) {
        for h in std::mem::take(&mut self.tables.udp).into_values() {
            backend.close(h);
        }
        for h in std::mem::take(&mut self.tables.tcp).into_values().flatten() {
            backend.close(h);
        }
    }

    /// The network unit's frames (with FCS) in, at `now_ms` of the
    /// machine's time; returns the frames (with FCS) it receives.
    pub(crate) fn service(
        &mut self,
        backend: &mut dyn NetBackend,
        outbox: Vec<Vec<u8>>,
        now_ms: u64,
    ) -> Vec<Vec<u8>> {
        let mut out = Out {
            b: backend,
            t: &mut self.tables,
        };
        let mut answers = Vec::new();
        for f in outbox {
            if f.len() < 4 {
                continue;
            }
            let (body, sum) = f.split_at(f.len() - 4);
            if fcs(body) == sum {
                answers.extend(self.gateway.frame(body, now_ms, &mut out));
            }
        }
        answers.extend(self.gateway.poll(now_ms, &mut out));
        for a in &mut answers {
            let sum = fcs(a);
            a.extend_from_slice(&sum);
        }
        answers
    }
}

/// The gateway's outside for one call: the host services and the tables.
struct Out<'a> {
    b: &'a mut dyn NetBackend,
    t: &'a mut Tables,
}

impl Outside for Out<'_> {
    fn resolve(&mut self, id: u32, name: &str) -> bool {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        let ip = match name.parse::<Ipv4Addr>() {
            Ok(ip) => ip,
            Err(_) => self.t.stand_in(&name),
        };
        self.t.resolved.push_back((id, Some(ip)));
        true
    }

    fn resolved(&mut self) -> Option<(u32, Option<Ipv4Addr>)> {
        self.t.resolved.pop_front()
    }

    fn udp_send(&mut self, machine: SocketAddrV4, to: SocketAddrV4, data: &[u8]) {
        let h = match self.t.udp.get(&(machine, to)) {
            Some(h) => *h,
            None => {
                let host = self.t.host_name(*to.ip());
                let Ok(h) = self.b.connect(SocketKind::Udp, &host, to.port()) else {
                    return;
                };
                self.t.udp.insert((machine, to), h);
                h
            }
        };
        // a datagram sent while the socket still connects is lost, as
        // datagrams may be
        let _ = self.b.send(h, data);
    }

    fn udp_recv(&mut self) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)> {
        let mut buf = [0u8; UDP_BUF];
        for (&(machine, from), &h) in &self.t.udp {
            if let Ok(Recv::Data(n)) = self.b.recv(h, &mut buf) {
                if n > 0 {
                    return Some((machine, from, buf[..n].to_vec()));
                }
            }
        }
        None
    }

    fn udp_close(&mut self, machine: SocketAddrV4) {
        let gone: Vec<_> = self
            .t
            .udp
            .keys()
            .filter(|(m, _)| *m == machine)
            .copied()
            .collect();
        for k in gone {
            if let Some(h) = self.t.udp.remove(&k) {
                self.b.close(h);
            }
        }
    }

    fn tcp_connect(&mut self, id: u32, to: SocketAddrV4) {
        let host = self.t.host_name(*to.ip());
        let h = self.b.connect(SocketKind::Tcp, &host, to.port()).ok();
        self.t.tcp.insert(id, h);
    }

    fn tcp_connect_state(&mut self, id: u32) -> Connect {
        let Some(Some(h)) = self.t.tcp.get(&id) else {
            return Connect::Failed;
        };
        match self.b.state(*h) {
            Ok(ConnState::Connecting) => Connect::Pending,
            // closed at once: what came is read before the end
            Ok(ConnState::Connected | ConnState::Closed) => Connect::Open,
            Ok(ConnState::Failed(_)) | Err(_) => Connect::Failed,
        }
    }

    fn tcp_send(&mut self, id: u32, data: &[u8]) -> Option<usize> {
        let h = (*self.t.tcp.get(&id)?)?;
        self.b.send(h, data).ok()
    }

    fn tcp_recv(&mut self, id: u32, buf: &mut [u8]) -> TcpRead {
        let Some(Some(h)) = self.t.tcp.get(&id) else {
            return TcpRead::Failed;
        };
        match self.b.recv(*h, buf) {
            Ok(Recv::Data(0)) => TcpRead::Nothing,
            Ok(Recv::Data(n)) => TcpRead::Data(n),
            Ok(Recv::Closed) => TcpRead::Closed,
            Err(_) => TcpRead::Failed,
        }
    }

    fn tcp_shutdown(&mut self, _id: u32) {
        // no half close in the host services (module docs)
    }

    fn tcp_close(&mut self, id: u32) {
        if let Some(Some(h)) = self.t.tcp.remove(&id) {
            self.b.close(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_get_stand_ins_and_numbers_stay() {
        let mut t = Tables::default();
        let a = t.stand_in("radio.example");
        let b = t.stand_in("bbs.example");
        assert_eq!(a, Ipv4Addr::new(198, 18, 0, 1));
        assert_eq!(b, Ipv4Addr::new(198, 18, 0, 2));
        assert_eq!(t.stand_in("radio.example"), a, "the same name again");
        assert_eq!(t.host_name(a), "radio.example");
        assert_eq!(
            t.host_name(Ipv4Addr::new(93, 184, 216, 34)),
            "93.184.216.34"
        );
        // the range turns round and forgets the oldest name
        t.next = STAND_INS - 1;
        let c = t.stand_in("c.example");
        assert_eq!(c, a);
        assert_eq!(t.host_name(a), "c.example");
        assert!(!t.names.contains_key("radio.example"));
    }
}
