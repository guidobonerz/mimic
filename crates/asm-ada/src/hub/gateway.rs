//! The gateway (SPEC.md 11.3): the hub as a host on the Logophor 16 net,
//! 10.16.0.1 with MAC 02:4C:31:36:FF:01. It sees every frame on the net
//! and answers those for its MAC and broadcasts:
//!
//! - ARP (RFC 826) for 10.16.0.1 and for every address outside
//!   10.16.0.0/24 (all reached through the gateway);
//! - ICMP echo (RFC 792) to 10.16.0.1;
//! - DHCP (RFC 2131, options RFC 2132): addresses from 10.16.0.10 up in
//!   the order MACs first ask, kept for the hub's run; mask, router, DNS
//!   server, domain `logophor`, lease 1 hour; the host name (option 12)
//!   is remembered for DNS; replies are broadcast;
//! - DNS (RFC 1035, UDP 53, A records): the machines' host names (`name`
//!   or `name.logophor`, any case), `hub` and `gateway` for 10.16.0.1,
//!   other names through the host's resolver, unknown ones NXDOMAIN;
//! - UDP to outside addresses (RFC 768) through a host socket per machine
//!   address and port (NAT), answers back from the outside address;
//! - TCP to outside addresses terminated in the hub ([`super::tcp`]).
//!
//! The gateway is deterministic: it keeps its tables in `BTreeMap`s, gets
//! the time as an argument and reaches the host only through
//! [`Outside`]. Frames in and out are without FCS ([`fcs`] adds one).
//! Pings beyond 10.16.0.1 get no answer (the host gives no raw sockets);
//! IP fragments are dropped.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};

use super::outside::Outside;
use super::packet::{
    self, Arp, Mac, BROADCAST, ETH_ARP, ETH_IPV4, PROTO_ICMP, PROTO_TCP, PROTO_UDP,
};
use super::{dhcp, dns, tcp};

pub use super::packet::fcs;

/// The gateway's address and MAC.
pub const GATEWAY_IP: Ipv4Addr = Ipv4Addr::new(10, 16, 0, 1);
pub const GATEWAY_MAC: Mac = [0x02, 0x4c, 0x31, 0x36, 0xff, 0x01];
/// The net and its mask.
pub const NET: Ipv4Addr = Ipv4Addr::new(10, 16, 0, 0);
pub const NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
/// The first address DHCP gives.
pub const FIRST_LEASE: Ipv4Addr = Ipv4Addr::new(10, 16, 0, 10);
/// The DHCP domain.
pub const DOMAIN: &str = "logophor";
/// The DHCP lease time in seconds.
pub const LEASE_SECS: u32 = 3600;
/// A UDP binding without traffic for this long is closed.
const UDP_IDLE_MS: u64 = 120_000;
/// A name lookup that takes longer is answered SERVFAIL.
const LOOKUP_MS: u64 = 10_000;
const MAX_LOOKUPS: usize = 64;
/// The largest UDP data that fits a 1500-byte IP datagram.
const UDP_MAX: usize = 1472;

/// Whether `ip` is on the Logophor net.
pub fn in_net(ip: Ipv4Addr) -> bool {
    u32::from(ip) & u32::from(NETMASK) == u32::from(NET)
}

/// Whether an address outside the net can be reached through the
/// gateway (not "this host", multicast, broadcast or reserved).
fn routable(ip: Ipv4Addr) -> bool {
    !in_net(ip)
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && ip.octets()[0] < 240
}

/// A DHCP lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub ip: Ipv4Addr,
    /// The host name the machine gave (option 12).
    pub name: Option<String>,
}

/// A DNS query waiting for the host's resolver.
struct Lookup {
    query: dns::Query,
    mac: Mac,
    machine: SocketAddrV4,
    since: u64,
}

/// The gateway; see the module docs.
#[derive(Default)]
pub struct Gateway {
    leases: BTreeMap<Mac, Lease>,
    /// Addresses on the net and the MACs seen sending from them.
    macs: BTreeMap<Ipv4Addr, Mac>,
    ip_id: u16,
    /// UDP bindings: machine address and port → last use.
    udp: BTreeMap<SocketAddrV4, u64>,
    lookups: BTreeMap<u32, Lookup>,
    next_lookup: u32,
    tcp: tcp::Tcp,
    log: Vec<String>,
}

impl Gateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// The leases given so far, by MAC.
    pub fn leases(&self) -> &BTreeMap<Mac, Lease> {
        &self.leases
    }

    /// The state of a TCP connection the gateway terminates.
    pub fn tcp_state(&self, machine: SocketAddrV4, remote: SocketAddrV4) -> Option<tcp::State> {
        self.tcp.state(machine, remote)
    }

    /// The number of TCP connections the gateway keeps.
    pub fn tcp_connections(&self) -> usize {
        self.tcp.len()
    }

    /// Log lines since the last call (leases, connections).
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// A frame (without FCS) seen on the net at time `now_ms`; returns the
    /// frames (without FCS) the gateway puts on the net in answer.
    pub fn frame(&mut self, frame: &[u8], now_ms: u64, out: &mut dyn Outside) -> Vec<Vec<u8>> {
        let mut res = Vec::new();
        let Some(e) = packet::parse_ethernet(frame) else {
            return res;
        };
        if e.src == GATEWAY_MAC || e.src[0] & 1 != 0 {
            return res;
        }
        let for_us = e.dst == GATEWAY_MAC || e.dst == BROADCAST;
        match e.ty {
            ETH_ARP => {
                if let Some(a) = packet::parse_arp(e.payload) {
                    self.learn(a.spa, a.sha);
                    if for_us {
                        self.arp(&a, &mut res);
                    }
                }
            }
            ETH_IPV4 => {
                if let Some(ip) = packet::parse_ipv4(e.payload) {
                    self.learn(ip.src, e.src);
                    if for_us {
                        self.ip(e.src, &ip, now_ms, out, &mut res);
                    }
                }
            }
            _ => {}
        }
        res
    }

    /// Work without a frame: answers from outside (names, datagrams,
    /// connections), retransmissions and timeouts. Call it often (every
    /// few milliseconds); returns frames for the net.
    pub fn poll(&mut self, now_ms: u64, out: &mut dyn Outside) -> Vec<Vec<u8>> {
        let mut res = Vec::new();
        // Names.
        while let Some((id, ip)) = out.resolved() {
            if let Some(l) = self.lookups.remove(&id) {
                let msg = match ip {
                    Some(ip) => dns::answer(&l.query, dns::NOERROR, false, Some(ip)),
                    None => dns::answer(&l.query, dns::NXDOMAIN, false, None),
                };
                self.send_udp(l.mac, dns_addr(), l.machine, &msg, &mut res);
            }
        }
        let late: Vec<u32> = self
            .lookups
            .iter()
            .filter(|(_, l)| now_ms.saturating_sub(l.since) >= LOOKUP_MS)
            .map(|(id, _)| *id)
            .collect();
        for id in late {
            if let Some(l) = self.lookups.remove(&id) {
                let msg = dns::answer(&l.query, dns::SERVFAIL, false, None);
                self.send_udp(l.mac, dns_addr(), l.machine, &msg, &mut res);
            }
        }
        // Datagrams from outside.
        for _ in 0..256 {
            let Some((machine, from, data)) = out.udp_recv() else {
                break;
            };
            if data.len() > UDP_MAX {
                continue;
            }
            let Some(&mac) = self.macs.get(machine.ip()) else {
                continue;
            };
            if let Some(t) = self.udp.get_mut(&machine) {
                *t = now_ms;
                self.send_udp(mac, from, machine, &data, &mut res);
            }
        }
        let idle: Vec<SocketAddrV4> = self
            .udp
            .iter()
            .filter(|(_, t)| now_ms.saturating_sub(**t) >= UDP_IDLE_MS)
            .map(|(a, _)| *a)
            .collect();
        for a in idle {
            self.udp.remove(&a);
            out.udp_close(a);
        }
        // Connections.
        let mut segs = Vec::new();
        self.tcp.poll(now_ms, out, &mut self.log, &mut segs);
        for s in segs {
            self.send_ip(s.mac, *s.from.ip(), *s.to.ip(), PROTO_TCP, &s.tcp, &mut res);
        }
        res
    }

    fn learn(&mut self, ip: Ipv4Addr, mac: Mac) {
        if in_net(ip)
            && ip != GATEWAY_IP
            && mac[0] & 1 == 0
            && ip.octets()[3] != 0
            && ip.octets()[3] != 255
        {
            self.macs.insert(ip, mac);
        }
    }

    fn arp(&mut self, a: &Arp, res: &mut Vec<Vec<u8>>) {
        // Requests for us and for everything beyond the net; a machine's
        // own announcement (sender = target) is not answered.
        if a.oper != 1 || a.spa == a.tpa || !(a.tpa == GATEWAY_IP || routable(a.tpa)) {
            return;
        }
        let reply = Arp {
            oper: 2,
            sha: GATEWAY_MAC,
            spa: a.tpa,
            tha: a.sha,
            tpa: a.spa,
        };
        res.push(packet::ethernet(
            a.sha,
            GATEWAY_MAC,
            ETH_ARP,
            &packet::arp(&reply),
        ));
    }

    fn send_ip(
        &mut self,
        mac: Mac,
        src: Ipv4Addr,
        dst: Ipv4Addr,
        proto: u8,
        payload: &[u8],
        res: &mut Vec<Vec<u8>>,
    ) {
        self.ip_id = self.ip_id.wrapping_add(1);
        let ip = packet::ipv4(src, dst, proto, self.ip_id, payload);
        res.push(packet::ethernet(mac, GATEWAY_MAC, ETH_IPV4, &ip));
    }

    fn send_udp(
        &mut self,
        mac: Mac,
        from: SocketAddrV4,
        to: SocketAddrV4,
        data: &[u8],
        res: &mut Vec<Vec<u8>>,
    ) {
        let u = packet::udp(from, to, data);
        self.send_ip(mac, *from.ip(), *to.ip(), PROTO_UDP, &u, res);
    }

    fn ip(
        &mut self,
        mac: Mac,
        ip: &packet::Ipv4<'_>,
        now: u64,
        out: &mut dyn Outside,
        res: &mut Vec<Vec<u8>>,
    ) {
        match ip.proto {
            PROTO_ICMP if ip.dst == GATEWAY_IP => self.icmp(mac, ip, res),
            PROTO_UDP => {
                let Some(u) = packet::parse_udp(ip) else {
                    return;
                };
                let from = SocketAddrV4::new(ip.src, u.src_port);
                let to = SocketAddrV4::new(ip.dst, u.dst_port);
                if u.dst_port == dhcp::SERVER_PORT
                    && (ip.dst == GATEWAY_IP || ip.dst.is_broadcast())
                {
                    self.dhcp(u.data, res);
                } else if ip.dst == GATEWAY_IP && u.dst_port == 53 {
                    self.dns(mac, from, u.data, now, out, res);
                } else if routable(ip.dst) && in_net(ip.src) {
                    self.udp.insert(from, now);
                    out.udp_send(from, to, u.data);
                }
            }
            PROTO_TCP => {
                let Some(t) = packet::parse_tcp(ip) else {
                    return;
                };
                let machine = SocketAddrV4::new(ip.src, t.src_port);
                let remote = SocketAddrV4::new(ip.dst, t.dst_port);
                if !in_net(ip.src) {
                    return;
                }
                let mut segs = Vec::new();
                if routable(ip.dst) {
                    self.tcp
                        .segment(mac, machine, remote, &t, now, out, &mut self.log, &mut segs);
                } else if ip.dst == GATEWAY_IP && !t.has(packet::RST) {
                    // The hub listens on no port of its own.
                    let (seq, ack, flags) = if t.has(packet::ACK) {
                        (t.ack, 0, packet::RST)
                    } else {
                        (
                            0,
                            t.seq.wrapping_add(t.seq_len()),
                            packet::RST | packet::ACK,
                        )
                    };
                    segs.push(tcp::Seg {
                        mac,
                        from: remote,
                        to: machine,
                        tcp: packet::tcp(remote, machine, seq, ack, flags, 0, None, &[]),
                    });
                }
                for s in segs {
                    self.send_ip(s.mac, *s.from.ip(), *s.to.ip(), PROTO_TCP, &s.tcp, res);
                }
            }
            _ => {}
        }
    }

    /// Echo request (type 8) → echo reply (type 0) with the same
    /// identifier, sequence number and data (RFC 792).
    fn icmp(&mut self, mac: Mac, ip: &packet::Ipv4<'_>, res: &mut Vec<Vec<u8>>) {
        let p = ip.payload;
        if p.len() < 8 || p[0] != 8 || p[1] != 0 || packet::checksum(p) != 0 {
            return;
        }
        let mut r = p.to_vec();
        r[0] = 0;
        r[2] = 0;
        r[3] = 0;
        let c = packet::checksum(&r);
        r[2..4].copy_from_slice(&c.to_be_bytes());
        self.send_ip(mac, GATEWAY_IP, ip.src, PROTO_ICMP, &r, res);
    }

    /// The lease of `mac`, given now if it has none (`None`: the net is
    /// full).
    fn lease(&mut self, mac: Mac) -> Option<Ipv4Addr> {
        if let Some(l) = self.leases.get(&mac) {
            return Some(l.ip);
        }
        let first = u32::from(FIRST_LEASE);
        let ip = Ipv4Addr::from(first + self.leases.len() as u32);
        if !in_net(ip) || ip.octets()[3] == 255 {
            return None;
        }
        self.leases.insert(mac, Lease { ip, name: None });
        Some(ip)
    }

    fn dhcp(&mut self, msg: &[u8], res: &mut Vec<Vec<u8>>) {
        let Some(req) = dhcp::parse(msg) else {
            return;
        };
        let mac = req.chaddr;
        if mac[0] & 1 != 0 || mac == GATEWAY_MAC {
            return;
        }
        let reply_type = match req.msg_type {
            dhcp::DISCOVER => dhcp::OFFER,
            dhcp::REQUEST => {
                if req.server_id.is_some_and(|s| s != GATEWAY_IP) {
                    // The client chose another server.
                    return;
                }
                dhcp::ACK
            }
            dhcp::INFORM => dhcp::ACK,
            _ => return,
        };
        let Some(ip) = self.lease(mac) else {
            return;
        };
        if let (Some(name), Some(l)) = (&req.host_name, self.leases.get_mut(&mac)) {
            l.name = Some(name.clone());
        }
        let mut r = dhcp::Reply {
            msg_type: reply_type,
            yiaddr: ip,
            server: GATEWAY_IP,
            config: Some((NETMASK, GATEWAY_IP, DOMAIN, Some(LEASE_SECS))),
        };
        if req.msg_type == dhcp::INFORM {
            r.yiaddr = Ipv4Addr::UNSPECIFIED;
            r.config = Some((NETMASK, GATEWAY_IP, DOMAIN, None));
        } else if req.msg_type == dhcp::REQUEST {
            let wanted = req
                .requested
                .or((!req.ciaddr.is_unspecified()).then_some(req.ciaddr));
            if wanted != Some(ip) {
                r.msg_type = dhcp::NAK;
                r.yiaddr = Ipv4Addr::UNSPECIFIED;
                r.config = None;
            } else {
                let name = self.leases[&mac].name.clone().unwrap_or_default();
                self.log
                    .push(format!("dhcp: {} is {ip} {name}", mac_text(mac)));
            }
        }
        let m = dhcp::reply(&req, &r);
        self.send_udp(
            BROADCAST,
            SocketAddrV4::new(GATEWAY_IP, dhcp::SERVER_PORT),
            SocketAddrV4::new(Ipv4Addr::BROADCAST, dhcp::CLIENT_PORT),
            &m,
            res,
        );
    }

    /// A name on the net: `hub`, `gateway` or a machine's host name, with
    /// or without `.logophor`. `Err(())` for a `.logophor` name nobody
    /// has, `Ok(None)` for a name to ask the host about.
    fn local_name(&self, name: &str) -> Result<Option<Ipv4Addr>, ()> {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        let (base, local) = match name.strip_suffix(".logophor") {
            Some(b) => (b.to_string(), true),
            None => (name.clone(), name == DOMAIN),
        };
        if base == "hub" || base == "gateway" {
            return Ok(Some(GATEWAY_IP));
        }
        let found = self
            .leases
            .values()
            .find(|l| {
                l.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&base))
            })
            .map(|l| l.ip);
        match found {
            Some(ip) => Ok(Some(ip)),
            None if local => Err(()),
            None => Ok(None),
        }
    }

    fn dns(
        &mut self,
        mac: Mac,
        machine: SocketAddrV4,
        msg: &[u8],
        now: u64,
        out: &mut dyn Outside,
        res: &mut Vec<Vec<u8>>,
    ) {
        let q = match dns::parse(msg) {
            dns::Parsed::Query(q) => q,
            dns::Parsed::Error { id, rd, rcode } => {
                let m = dns::error(id, rd, rcode);
                self.send_udp(mac, dns_addr(), machine, &m, res);
                return;
            }
            dns::Parsed::Drop => return,
        };
        let answer = match self.local_name(&q.name) {
            Ok(Some(ip)) => Some(dns::answer(
                &q,
                dns::NOERROR,
                true,
                q.wants_a().then_some(ip),
            )),
            Err(()) => Some(dns::answer(&q, dns::NXDOMAIN, true, None)),
            Ok(None) if !q.wants_a() => Some(dns::answer(&q, dns::NOERROR, false, None)),
            Ok(None) if !host_name_ok(&q.name) => Some(dns::answer(&q, dns::NXDOMAIN, false, None)),
            Ok(None) => {
                let id = self.next_lookup;
                if self.lookups.len() < MAX_LOOKUPS && out.resolve(id, &q.name) {
                    self.next_lookup = self.next_lookup.wrapping_add(1);
                    self.lookups.insert(
                        id,
                        Lookup {
                            query: q.clone(),
                            mac,
                            machine,
                            since: now,
                        },
                    );
                    None
                } else {
                    Some(dns::answer(&q, dns::SERVFAIL, false, None))
                }
            }
        };
        if let Some(m) = answer {
            self.send_udp(mac, dns_addr(), machine, &m, res);
        }
    }
}

fn dns_addr() -> SocketAddrV4 {
    SocketAddrV4::new(GATEWAY_IP, 53)
}

/// Letters, digits, hyphens and underscores in dot-separated labels: what
/// may go to the host's resolver.
fn host_name_ok(name: &str) -> bool {
    let name = name.trim_end_matches('.');
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// A MAC as text, colon-separated.
pub fn mac_text(m: Mac) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
