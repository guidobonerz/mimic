//! A virtual router: user-mode NAT between an emulated machine's Ethernet
//! interface and the host's network as [`mimic_core::host::NetBackend`]
//! offers it (sockets by host name and port; every call returns at once).
//!
//! The machine hands over whole Ethernet II frames without FCS
//! ([`Router::guest_frame`]) and fetches the frames for the guest
//! ([`Router::take_frame`]); [`Router::poll`] lets time pass. All time is
//! emulated microseconds from the caller; there is no wall clock and no
//! randomness (initial sequence numbers, ports and addresses come from
//! counters in the state), so the same inputs give the same frames
//! (golden rule 3). The host's answers are journalled by the frontend
//! like for every other device.
//!
//! # The network the guest sees
//!
//! With the default [`RouterConfig`]: net 10.0.2.0/24, the router
//! (gateway) at 10.0.2.2, its name server at 10.0.2.3, the guest at
//! 10.0.2.15. Router and name server share the locally administered MAC
//! [`GATEWAY_MAC`]. Nothing else lives on the virtual LAN.
//!
//! - **ARP** (RFC 826): requests for the gateway, the name server and for
//!   every address outside the subnet are answered with the router's MAC.
//!   Requests for the guest's own address (address probes) and for other
//!   addresses of the subnet get no answer. The guest's MAC and address
//!   are learned from ARP, DHCP and every IPv4 packet with a source
//!   address in the subnet (a guest with a static address works).
//! - **DHCP** (RFC 2131, options of RFC 2132): DISCOVER → OFFER, REQUEST →
//!   ACK (NAK for any address but the configured one), INFORM → ACK;
//!   subnet mask, broadcast address, router, name server, lease of one day
//!   (T1/T2 at a half and seven eighths). The one address is never given
//!   to anyone else, so leases need no bookkeeping.
//! - **ICMP** (RFC 792): echo requests to the gateway and the name server
//!   are answered. Pings to anything else are dropped; the router sends no
//!   ICMP errors.
//! - **DNS** (RFC 1035) on the name server address, UDP port 53: the
//!   backend connects by name and has no resolver, so every A query is
//!   answered at once with a synthetic address from 198.18.1.1 up (the
//!   benchmark range 198.18.0.0/15 of RFC 2544, which no real host uses):
//!   in query order, the same address for the same name; the table of
//!   [`DNS_MAX`] names lives in the state and forgets the name asked
//!   longest ago. A connection to such an address is opened by its name.
//!   A name that is a dotted quad is answered with that address. Other
//!   query types (AAAA …) get an empty answer; whether a name exists shows
//!   only when a connection to it fails.
//! - **Destinations**: the gateway address stands for the PC itself (the
//!   backend connects to `127.0.0.1`); an address from the DNS table is
//!   connected by name; any other address outside the subnet as a
//!   dotted-quad string (hosts in the user's LAN, literal addresses).
//!   Addresses of the subnet, broadcast, multicast, 0.x and 127.x are
//!   dropped; TCP to the name server or to a synthetic address without a
//!   name is refused with RST.
//! - **UDP** (RFC 768): one connected host socket per flow (guest port,
//!   destination, destination port); what comes back is delivered from
//!   that destination. A flow without traffic for [`UDP_IDLE_US`] is
//!   closed; of more than [`UDP_MAX`] flows the quietest goes. Datagrams
//!   above 1472 bytes from the host are dropped (no fragmentation).
//!   Broadcast and multicast from the guest are dropped (so the
//!   Ultimate's UDP identify on port 64 is not reachable from outside).
//! - **TCP from the guest**: ended locally by the router's own endpoint
//!   ([`tcp`]) and relayed to a host socket with back-pressure both ways.
//!   The SYN-ACK goes out when the host socket stands; a failed or
//!   refused connection (also: no network attached) is a RST.
//! - **TCP to the guest** ([`Forward`]): the router listens on the host
//!   port and connects each accepted connection to the guest's port from
//!   the gateway address and a port from 49152 up. While the guest's
//!   address is unknown, accepted connections are closed at once. A
//!   listener that cannot be opened is tried again every
//!   [`LISTEN_RETRY_US`].
//! - IPv4 fragments from the guest are dropped, as is everything that is
//!   not ARP or IPv4 (IPv6, VLAN tags) and every frame with a wrong
//!   length or checksum. Frames to the guest never exceed 1514 bytes.
//!
//! # Savestates
//!
//! The whole router is saved: learned addresses, the DNS table, UDP
//! flows, every TCP connection with its buffers, the frames not fetched
//! yet. Host sockets cannot survive a load: at the first `poll` after
//! [`Router::load`] every TCP connection that had a host socket is reset
//! towards the guest (RST), UDP flows get a new socket silently and the
//! forwards are listened on again. Sockets open before the load are
//! closed.
//!
//! Sources: RFC 791, 792, 793, 768, 826, 1035, 1122, 2131, 2132, 2544
//! (the address range); nothing was taken from another implementation.

mod tcp;
#[cfg(test)]
mod tests;
mod wire;

use std::collections::{BTreeMap, VecDeque};
use std::sync::MutexGuard;

use mimic_core::host::{
    lock, ConnState, HostServices, NetBackend, NetError, NetHandle, Recv, SharedNet, SocketKind,
};
use mimic_core::{StateError, StateReader, StateWriter};

use tcp::{Conn, Key, Step};
use wire::{Addr, BROADCAST_MAC, MAX_FRAME, MIN_FRAME};

/// The router's (and its name server's) MAC address: locally administered.
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0x6d, 0x69, 0x6d, 0x72, 0x01];
/// Names in the DNS table.
pub const DNS_MAX: usize = 256;
/// TCP connections at a time; further ones are refused.
pub const TCP_MAX: usize = 64;
/// UDP flows at a time.
pub const UDP_MAX: usize = 64;
/// A UDP flow without traffic for this long is closed (emulated time).
pub const UDP_IDLE_US: u64 = 60_000_000;
/// Pause before a failed `listen` of a forward is tried again.
pub const LISTEN_RETRY_US: u64 = 5_000_000;

/// Datagrams of a flow waiting for the host socket to stand.
const UDP_PENDING_MAX: usize = 16;
/// Datagrams read from one flow per poll.
const UDP_RECV_PER_POLL: usize = 8;
/// Largest UDP payload in one frame: 1500 - 20 - 8.
const UDP_MAX_DATA: usize = 1472;
/// Frames waiting for the guest; beyond this new ones are dropped.
const OUT_MAX: usize = 512;
/// From this many waiting frames on TCP holds back data.
const OUT_SOFT: usize = 64;
/// Connections taken from one listener per poll.
const ACCEPTS_PER_POLL: usize = 4;
/// Longest host name (RFC 1035, 2.3.4).
const NAME_MAX: usize = 253;
/// Synthetic addresses: 198.18.x.y with x and y from 1 to 254.
const POOL: u32 = 254 * 254;
const LEASE_SECS: u32 = 86_400;
const DHCP_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

/// A TCP port of the host that leads to a port of the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forward {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterConfig {
    /// The guest's MAC; learned from its first frame otherwise.
    pub guest_mac_hint: Option<[u8; 6]>,
    /// The subnet (/24).
    pub net: [u8; 4],
    /// The router: answers ARP and ICMP echo; as a destination the PC
    /// itself.
    pub gateway: [u8; 4],
    /// The router's name server.
    pub dns: [u8; 4],
    /// The address DHCP hands out.
    pub guest: [u8; 4],
    pub forwards: Vec<Forward>,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            guest_mac_hint: None,
            net: [10, 0, 2, 0],
            gateway: [10, 0, 2, 2],
            dns: [10, 0, 2, 3],
            guest: [10, 0, 2, 15],
            forwards: Vec::new(),
        }
    }
}

/// The host's network, locked for one call of the router; without one
/// every call fails.
pub(crate) struct Net<'a>(Option<MutexGuard<'a, dyn NetBackend + 'static>>);

impl<'a> Net<'a> {
    fn new(shared: &'a Option<SharedNet>) -> Self {
        Net(shared.as_ref().map(|n| lock(n)))
    }

    fn is_some(&self) -> bool {
        self.0.is_some()
    }

    fn connect(&mut self, kind: SocketKind, host: &str, port: u16) -> Result<NetHandle, NetError> {
        match &mut self.0 {
            Some(n) => n.connect(kind, host, port),
            None => Err(NetError::NoNetwork),
        }
    }

    fn listen(&mut self, port: u16) -> Result<NetHandle, NetError> {
        match &mut self.0 {
            Some(n) => n.listen(port),
            None => Err(NetError::NoNetwork),
        }
    }

    fn accept(&mut self, l: NetHandle) -> Result<Option<NetHandle>, NetError> {
        match &mut self.0 {
            Some(n) => n.accept(l),
            None => Err(NetError::NoNetwork),
        }
    }

    fn state(&mut self, h: NetHandle) -> Result<ConnState, NetError> {
        match &mut self.0 {
            Some(n) => n.state(h),
            None => Err(NetError::NoNetwork),
        }
    }

    fn send(&mut self, h: NetHandle, data: &[u8]) -> Result<usize, NetError> {
        match &mut self.0 {
            Some(n) => n.send(h, data),
            None => Err(NetError::NoNetwork),
        }
    }

    fn recv(&mut self, h: NetHandle, buf: &mut [u8]) -> Result<Recv, NetError> {
        match &mut self.0 {
            Some(n) => n.recv(h, buf),
            None => Err(NetError::NoNetwork),
        }
    }

    fn close(&mut self, h: NetHandle) {
        if let Some(n) = &mut self.0 {
            n.close(h);
        }
    }
}

/// The wire to the guest: its addresses and the frames waiting for it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Link {
    out: VecDeque<Vec<u8>>,
    /// The next IPv4 identification.
    ip_id: u16,
    guest_mac: Option<[u8; 6]>,
    guest_ip: Option<[u8; 4]>,
}

impl Link {
    fn push(&mut self, mut frame: Vec<u8>) {
        if frame.len() > MAX_FRAME || self.out.len() >= OUT_MAX {
            return;
        }
        if frame.len() < MIN_FRAME {
            frame.resize(MIN_FRAME, 0);
        }
        self.out.push_back(frame);
    }

    fn ident(&mut self) -> u16 {
        let id = self.ip_id;
        self.ip_id = id.wrapping_add(1);
        id
    }

    /// Addresses of a packet from `src` to the guest; `None` while the
    /// guest is unknown.
    fn guest_addr(&mut self, src: [u8; 4]) -> Option<Addr> {
        Some(Addr {
            dst_mac: self.guest_mac?,
            src_mac: GATEWAY_MAC,
            src,
            dst: self.guest_ip?,
            ident: self.ident(),
        })
    }

    fn tcp(&mut self, src: [u8; 4], seg: &wire::Tcp<'_>) {
        if let Some(f) = self.guest_addr(src).and_then(|a| wire::tcp_frame(&a, seg)) {
            self.push(f);
        }
    }

    fn udp(&mut self, src: [u8; 4], sport: u16, dport: u16, data: &[u8]) {
        if let Some(f) = self
            .guest_addr(src)
            .and_then(|a| wire::udp_frame(&a, sport, dport, data))
        {
            self.push(f);
        }
    }

    /// Too many frames wait: TCP holds its data back.
    fn congested(&self) -> bool {
        self.out.len() >= OUT_SOFT
    }
}

/// A UDP flow of the guest.
#[derive(Clone, Debug)]
struct Flow {
    /// `None` after a load: connected again at the next poll.
    handle: Option<NetHandle>,
    /// The name the socket is connected to.
    name: String,
    last_us: u64,
    /// Datagrams the host socket has not taken yet (still connecting).
    pending: VecDeque<Vec<u8>>,
}

/// Where a destination address of the guest leads.
enum Target {
    /// Connect to this host name.
    Name(String),
    /// Nothing listens there: RST for TCP.
    Refuse,
    Drop,
}

pub struct Router {
    config: RouterConfig,
    host: HostServices,
    link: Link,
    now: u64,
    /// Name and synthetic address, the one asked longest ago first.
    dns: Vec<(String, [u8; 4])>,
    dns_next: u32,
    udp: BTreeMap<Key, Flow>,
    tcp: BTreeMap<Key, Conn>,
    iss_next: u32,
    port_next: u16,
    /// One per forward of the configuration.
    listeners: Vec<Option<NetHandle>>,
    listen_retry_at: u64,
}

fn dotted(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

fn parse_dotted(name: &str) -> Option<[u8; 4]> {
    let mut ip = [0u8; 4];
    let mut parts = name.split('.');
    for b in &mut ip {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        *b = p.parse().ok()?;
    }
    parts.next().is_none().then_some(ip)
}

impl Router {
    pub fn new(config: RouterConfig) -> Self {
        let link = Link {
            guest_mac: config.guest_mac_hint,
            ..Link::default()
        };
        let listeners = vec![None; config.forwards.len()];
        Self {
            config,
            host: HostServices::default(),
            link,
            now: 0,
            dns: Vec::new(),
            dns_next: 0,
            udp: BTreeMap::new(),
            tcp: BTreeMap::new(),
            iss_next: 0x0001_0000,
            port_next: 0,
            listeners,
            listen_retry_at: 0,
        }
    }

    /// Lend the router the host's network. Without one (`host.net` is
    /// `None`) the link stays up and the router itself answers (ARP,
    /// DHCP, DNS, ping), but every connection to the outside is refused.
    /// Sockets of a network attached before are closed; connections over
    /// them are reset at the next poll.
    pub fn attach_host(&mut self, host: HostServices) {
        self.drop_host_handles();
        self.host = host;
    }

    /// Close every host socket and listener; the state keeps going as
    /// after a load.
    fn drop_host_handles(&mut self) {
        let shared = self.host.net.clone();
        let mut net = Net::new(&shared);
        for l in &mut self.listeners {
            if let Some(h) = l.take() {
                net.close(h);
            }
        }
        for f in self.udp.values_mut() {
            if let Some(h) = f.handle.take() {
                net.close(h);
            }
        }
        for c in self.tcp.values_mut() {
            c.orphan(&mut net);
        }
        self.listen_retry_at = 0;
    }

    pub fn has_frame(&self) -> bool {
        !self.link.out.is_empty()
    }

    /// The next frame for the guest.
    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        self.link.out.pop_front()
    }

    /// The forwards of the configuration and whether each one's listener
    /// stands.
    pub fn forwards_status(&self) -> Vec<(Forward, bool)> {
        self.config
            .forwards
            .iter()
            .zip(&self.listeners)
            .map(|(f, l)| (*f, l.is_some()))
            .collect()
    }

    // --- addresses -------------------------------------------------------

    fn in_subnet(&self, ip: [u8; 4]) -> bool {
        ip[..3] == self.config.net[..3]
    }

    /// An address the guest may use as its own.
    fn is_guest_addr(&self, ip: [u8; 4]) -> bool {
        self.in_subnet(ip)
            && ip != self.config.gateway
            && ip != self.config.dns
            && ip[3] != 0
            && ip[3] != 255
    }

    fn target(&self, ip: [u8; 4]) -> Target {
        if ip == self.config.gateway {
            return Target::Name("127.0.0.1".into());
        }
        if ip == self.config.dns {
            return Target::Refuse;
        }
        if self.in_subnet(ip) || ip[0] == 0 || ip[0] == 127 || ip[0] >= 224 {
            return Target::Drop;
        }
        if ip[0] == 198 && ip[1] & 0xfe == 18 {
            return match self.dns.iter().find(|(_, a)| *a == ip) {
                Some((name, _)) => Target::Name(name.clone()),
                None => Target::Refuse,
            };
        }
        Target::Name(dotted(ip))
    }

    /// The synthetic address of `name` (lower case, no trailing dot).
    fn dns_address(&mut self, name: &str) -> [u8; 4] {
        if let Some(ip) = parse_dotted(name) {
            return ip;
        }
        if let Some(i) = self.dns.iter().position(|(n, _)| n == name) {
            let entry = self.dns.remove(i);
            let ip = entry.1;
            self.dns.push(entry);
            return ip;
        }
        if self.dns.len() >= DNS_MAX {
            self.dns.remove(0);
        }
        let ip = loop {
            let i = self.dns_next % POOL;
            self.dns_next = (i + 1) % POOL;
            let ip = [198, 18, 1 + (i / 254) as u8, 1 + (i % 254) as u8];
            if !self.dns.iter().any(|(_, a)| *a == ip) {
                break ip;
            }
        };
        self.dns.push((name.into(), ip));
        ip
    }

    // --- frames from the guest ---------------------------------------------

    /// A frame from the guest. `now_us`: emulated microseconds (monotonic).
    pub fn guest_frame(&mut self, frame: &[u8], now_us: u64) {
        self.now = self.now.max(now_us);
        if frame.len() > MAX_FRAME {
            return;
        }
        let Some(eth) = wire::parse_eth(frame) else {
            return;
        };
        // From a unicast address that is not the router's, to the router
        // or to everyone.
        if eth.src[0] & 1 != 0 || eth.src == GATEWAY_MAC {
            return;
        }
        if eth.dst != GATEWAY_MAC && eth.dst != BROADCAST_MAC {
            return;
        }
        match eth.ethertype {
            wire::ETHERTYPE_ARP => self.arp_in(&eth),
            wire::ETHERTYPE_IPV4 => self.ip_in(&eth),
            _ => {}
        }
    }

    fn arp_in(&mut self, eth: &wire::Eth<'_>) {
        let Some(arp) = wire::parse_arp(eth.payload) else {
            return;
        };
        if arp.sha != eth.src {
            return;
        }
        self.link.guest_mac = Some(arp.sha);
        if self.is_guest_addr(arp.spa) {
            self.link.guest_ip = Some(arp.spa);
        }
        if arp.op != 1 || arp.tpa == arp.spa {
            return;
        }
        let ours = arp.tpa == self.config.gateway || arp.tpa == self.config.dns;
        if !ours && self.in_subnet(arp.tpa) {
            return;
        }
        let reply = wire::Arp {
            op: 2,
            sha: GATEWAY_MAC,
            spa: arp.tpa,
            tha: arp.sha,
            tpa: arp.spa,
        };
        self.link
            .push(wire::arp_frame(arp.sha, GATEWAY_MAC, &reply));
    }

    fn ip_in(&mut self, eth: &wire::Eth<'_>) {
        let Some(ip) = wire::parse_ipv4(eth.payload) else {
            return;
        };
        if ip.proto == wire::PROTO_UDP {
            let Some(udp) = wire::parse_udp(&ip) else {
                return;
            };
            // DHCP comes before the guest has an address.
            if udp.dport == 67 {
                if udp.sport == 68 {
                    self.dhcp_in(eth.src, &udp);
                }
                return;
            }
            if self.learn_guest(eth, &ip) {
                self.udp_in(&ip, &udp);
            }
            return;
        }
        if !self.learn_guest(eth, &ip) {
            return;
        }
        match ip.proto {
            wire::PROTO_ICMP => self.icmp_in(&ip),
            wire::PROTO_TCP => {
                if let Some(seg) = wire::parse_tcp(&ip) {
                    self.tcp_in(&ip, &seg);
                }
            }
            _ => {}
        }
    }

    /// A unicast packet from an address the guest may have: learn it.
    fn learn_guest(&mut self, eth: &wire::Eth<'_>, ip: &wire::Ipv4<'_>) -> bool {
        if eth.dst != GATEWAY_MAC || !self.is_guest_addr(ip.src) {
            return false;
        }
        self.link.guest_mac = Some(eth.src);
        self.link.guest_ip = Some(ip.src);
        true
    }

    fn icmp_in(&mut self, ip: &wire::Ipv4<'_>) {
        if ip.dst != self.config.gateway && ip.dst != self.config.dns {
            return;
        }
        let m = ip.payload;
        // Echo request: type 8, code 0; the rest comes back as type 0.
        if m.len() < 8 || m[0] != 8 || m[1] != 0 || wire::fold(wire::sum(m, 0)) != 0 {
            return;
        }
        let reply = wire::icmp_message(0, 0, &m[4..]);
        if let Some(f) = self
            .link
            .guest_addr(ip.dst)
            .and_then(|a| wire::ipv4_frame(&a, wire::PROTO_ICMP, &reply))
        {
            self.link.push(f);
        }
    }

    // --- DHCP ----------------------------------------------------------------

    /// A BOOTP request (RFC 2131 figure 1): op at 0, htype 1, hlen 2, xid
    /// 4–7, flags 10–11 (bit 15: answer by broadcast), ciaddr 12–15,
    /// yiaddr 16–19, siaddr 20–23, giaddr 24–27, chaddr 28–43, the magic
    /// cookie at 236, options from 240.
    fn dhcp_in(&mut self, src_mac: [u8; 6], udp: &wire::Udp<'_>) {
        let p = udp.data;
        if p.len() < 240 || p[0] != 1 || p[1] != 1 || p[2] != 6 || p[236..240] != DHCP_COOKIE {
            return;
        }
        let ciaddr = [p[12], p[13], p[14], p[15]];
        let broadcast = p[10] & 0x80 != 0;
        let mut kind = None;
        let mut requested = None;
        let mut server = None;
        let mut i = 240;
        while let Some(&code) = p.get(i) {
            match code {
                0 => i += 1,
                255 => break,
                _ => {
                    let Some(&len) = p.get(i + 1) else {
                        break;
                    };
                    let Some(v) = p.get(i + 2..i + 2 + usize::from(len)) else {
                        break;
                    };
                    match (code, v) {
                        (53, [k]) => kind = Some(*k),
                        (50, [a, b, c, d]) => requested = Some([*a, *b, *c, *d]),
                        (54, [a, b, c, d]) => server = Some([*a, *b, *c, *d]),
                        _ => {}
                    }
                    i += 2 + usize::from(len);
                }
            }
        }
        // A request that names another server is not for this one.
        if server.is_some_and(|s| s != self.config.gateway) {
            return;
        }
        const OFFER: u8 = 2;
        const ACK: u8 = 5;
        const NAK: u8 = 6;
        let guest = self.config.guest;
        let (reply, yiaddr, inform) = match kind {
            Some(1) => (OFFER, guest, false),
            Some(3) => {
                let want = requested.unwrap_or(if ciaddr == [0; 4] { guest } else { ciaddr });
                if want == guest {
                    (ACK, guest, false)
                } else {
                    (NAK, [0; 4], false)
                }
            }
            Some(8) => (ACK, [0; 4], true),
            _ => return,
        };
        self.link.guest_mac = Some(src_mac);
        if reply == ACK {
            if !inform {
                self.link.guest_ip = Some(guest);
            } else if self.is_guest_addr(ciaddr) {
                self.link.guest_ip = Some(ciaddr);
            }
        }

        let gw = self.config.gateway;
        let mut m = vec![0u8; 240];
        m[0] = 2;
        m[1] = 1;
        m[2] = 6;
        m[4..8].copy_from_slice(&p[4..8]);
        m[10..12].copy_from_slice(&p[10..12]);
        if reply == ACK {
            m[12..16].copy_from_slice(&ciaddr);
        }
        m[16..20].copy_from_slice(&yiaddr);
        m[20..24].copy_from_slice(&gw);
        m[24..28].copy_from_slice(&p[24..28]);
        m[28..44].copy_from_slice(&p[28..44]);
        m[236..240].copy_from_slice(&DHCP_COOKIE);
        m.extend_from_slice(&[53, 1, reply, 54, 4]);
        m.extend_from_slice(&gw);
        if reply != NAK {
            if !inform {
                for (code, secs) in [
                    (51, LEASE_SECS),
                    (58, LEASE_SECS / 2),
                    (59, LEASE_SECS / 8 * 7),
                ] {
                    m.extend_from_slice(&[code, 4]);
                    m.extend_from_slice(&secs.to_be_bytes());
                }
            }
            let net = self.config.net;
            m.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
            m.extend_from_slice(&[28, 4, net[0], net[1], net[2], 255]);
            m.extend_from_slice(&[3, 4]);
            m.extend_from_slice(&gw);
            m.extend_from_slice(&[6, 4]);
            m.extend_from_slice(&self.config.dns);
        }
        m.push(255);
        // BOOTP's minimum size.
        if m.len() < 300 {
            m.resize(300, 0);
        }
        // RFC 2131, 4.1: to ciaddr if the client has an address, by
        // broadcast if it asks for that (and every NAK), else to the new
        // address at the client's hardware address.
        let (dst_mac, dst) = if reply == NAK {
            (BROADCAST_MAC, [255; 4])
        } else if ciaddr != [0; 4] {
            (src_mac, ciaddr)
        } else if broadcast {
            (BROADCAST_MAC, [255; 4])
        } else {
            (src_mac, yiaddr)
        };
        let a = Addr {
            dst_mac,
            src_mac: GATEWAY_MAC,
            src: gw,
            dst,
            ident: self.link.ident(),
        };
        if let Some(f) = wire::udp_frame(&a, 67, 68, &m) {
            self.link.push(f);
        }
    }

    // --- DNS -----------------------------------------------------------------

    /// A DNS query (RFC 1035, 4.1): header of six 16-bit fields (id,
    /// flags, QDCOUNT, ANCOUNT, NSCOUNT, ARCOUNT), then the question:
    /// name as length-prefixed labels, QTYPE, QCLASS.
    fn dns_in(&mut self, udp: &wire::Udp<'_>) {
        let q = udp.data;
        let (Some(flags), Some(qdcount)) = (wire::be16(q, 2), wire::be16(q, 4)) else {
            return;
        };
        if q.len() < 12 || flags & 0x8000 != 0 {
            return;
        }
        const FORMERR: u16 = 1;
        const NOTIMP: u16 = 4;
        let mut rcode = 0;
        let mut question: &[u8] = &[];
        let mut answer = None;
        if flags & 0x7800 != 0 {
            rcode = NOTIMP;
        } else if qdcount == 0 {
            rcode = FORMERR;
        } else {
            match Self::dns_question(q) {
                Some((name, end, qtype, qclass)) => {
                    question = &q[12..end];
                    // An A record in class IN; the root has none.
                    if qtype == 1 && qclass == 1 && !name.is_empty() {
                        answer = Some(self.dns_address(&name));
                    }
                }
                None => rcode = FORMERR,
            }
        }
        let mut m = Vec::with_capacity(12 + question.len() + 16);
        m.extend_from_slice(&q[0..2]);
        // Response, the query's opcode and RD, recursion available.
        let out_flags = 0x8080 | (flags & 0x7900) | rcode;
        m.extend_from_slice(&out_flags.to_be_bytes());
        m.extend_from_slice(&[0, u8::from(!question.is_empty())]);
        m.extend_from_slice(&[0, u8::from(answer.is_some())]);
        m.extend_from_slice(&[0, 0, 0, 0]);
        m.extend_from_slice(question);
        if let Some(ip) = answer {
            // Name: pointer to offset 12; type A, class IN, TTL 60 s,
            // four bytes of address.
            m.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
            m.extend_from_slice(&ip);
        }
        let dns = self.config.dns;
        self.link.udp(dns, 53, udp.sport, &m);
    }

    /// The first question: name in lower case, the offset behind the
    /// question, QTYPE and QCLASS.
    fn dns_question(q: &[u8]) -> Option<(String, usize, u16, u16)> {
        let mut name = String::new();
        let mut pos = 12;
        loop {
            let len = usize::from(*q.get(pos)?);
            pos += 1;
            if len == 0 {
                break;
            }
            // No compression pointers in a question's name.
            if len > 63 {
                return None;
            }
            let label = q.get(pos..pos + len)?;
            pos += len;
            if !name.is_empty() {
                name.push('.');
            }
            for &b in label {
                if !b.is_ascii_graphic() || b == b'.' {
                    return None;
                }
                name.push(char::from(b.to_ascii_lowercase()));
            }
            if name.len() > NAME_MAX {
                return None;
            }
        }
        Some((name, pos + 4, wire::be16(q, pos)?, wire::be16(q, pos + 2)?))
    }

    // --- UDP -----------------------------------------------------------------

    fn udp_in(&mut self, ip: &wire::Ipv4<'_>, udp: &wire::Udp<'_>) {
        if ip.dst == self.config.dns {
            if udp.dport == 53 {
                self.dns_in(udp);
            }
            return;
        }
        let Target::Name(name) = self.target(ip.dst) else {
            return;
        };
        let now = self.now;
        let key: Key = (udp.sport, ip.dst, udp.dport);
        let shared = self.host.net.clone();
        let mut net = Net::new(&shared);
        if !self.udp.contains_key(&key) {
            if self.udp.len() >= UDP_MAX {
                let quietest = self.udp.iter().min_by_key(|(k, f)| (f.last_us, **k));
                if let Some(k) = quietest.map(|(k, _)| *k) {
                    if let Some(h) = self.udp.remove(&k).and_then(|f| f.handle) {
                        net.close(h);
                    }
                }
            }
            let Ok(h) = net.connect(SocketKind::Udp, &name, udp.dport) else {
                return;
            };
            self.udp.insert(
                key,
                Flow {
                    handle: Some(h),
                    name,
                    last_us: now,
                    pending: VecDeque::new(),
                },
            );
        }
        let Some(flow) = self.udp.get_mut(&key) else {
            return;
        };
        flow.last_us = now;
        if flow.pending.len() < UDP_PENDING_MAX {
            flow.pending.push_back(udp.data.to_vec());
        }
        if !Self::flush_flow(flow, &mut net) {
            if let Some(h) = self.udp.remove(&key).and_then(|f| f.handle) {
                net.close(h);
            }
        }
    }

    /// Hand the waiting datagrams to the host socket; `false`: it failed.
    fn flush_flow(flow: &mut Flow, net: &mut Net<'_>) -> bool {
        let Some(h) = flow.handle else {
            return true;
        };
        while let Some(d) = flow.pending.front() {
            match net.send(h, d) {
                // Not taken: still connecting.
                Ok(0) if !d.is_empty() => {
                    return !matches!(net.state(h), Ok(ConnState::Failed(_)) | Err(_));
                }
                Ok(_) => {
                    flow.pending.pop_front();
                }
                Err(_) => return false,
            }
        }
        true
    }

    fn poll_udp(&mut self, net: &mut Net<'_>, now: u64) {
        let keys: Vec<Key> = self.udp.keys().copied().collect();
        let mut buf = [0u8; 2048];
        for key in keys {
            let Some(flow) = self.udp.get_mut(&key) else {
                continue;
            };
            let mut alive = now.saturating_sub(flow.last_us) < UDP_IDLE_US;
            if alive && flow.handle.is_none() {
                flow.handle = net.connect(SocketKind::Udp, &flow.name, key.2).ok();
                alive = flow.handle.is_some();
            }
            alive = alive && Self::flush_flow(flow, net);
            if let (true, Some(h)) = (alive, flow.handle) {
                for _ in 0..UDP_RECV_PER_POLL {
                    match net.recv(h, &mut buf) {
                        Ok(Recv::Data(0)) => break,
                        Ok(Recv::Data(n)) => {
                            flow.last_us = now;
                            if n <= UDP_MAX_DATA {
                                self.link.udp(key.1, key.2, key.0, &buf[..n]);
                            }
                        }
                        Ok(Recv::Closed) | Err(_) => {
                            alive = false;
                            break;
                        }
                    }
                }
            }
            if !alive {
                if let Some(h) = self.udp.remove(&key).and_then(|f| f.handle) {
                    net.close(h);
                }
            }
        }
    }

    // --- TCP -----------------------------------------------------------------

    fn next_iss(&mut self) -> u32 {
        let iss = self.iss_next;
        self.iss_next = iss.wrapping_add(64_021);
        iss
    }

    /// Room for one more connection, if need be by forgetting the
    /// finished one that lingers the shortest.
    fn tcp_room(&mut self) -> bool {
        if self.tcp.len() < TCP_MAX {
            return true;
        }
        let oldest = self
            .tcp
            .iter()
            .filter(|(_, c)| c.is_lingering())
            .min_by_key(|(k, c)| (c.deadline(), **k))
            .map(|(k, _)| *k);
        match oldest {
            Some(k) => {
                self.tcp.remove(&k);
                true
            }
            None => false,
        }
    }

    /// Answer a segment nobody wants with a RST (RFC 793, "Reset
    /// Generation").
    fn tcp_refuse(&mut self, key: Key, seg: &wire::Tcp<'_>) {
        let (seq, ack, flags) = if seg.has(wire::ACK) {
            (seg.ack, 0, wire::RST)
        } else {
            let len = seg.data.len() as u32
                + u32::from(seg.has(wire::SYN))
                + u32::from(seg.has(wire::FIN));
            (0, seg.seq.wrapping_add(len), wire::RST | wire::ACK)
        };
        self.link.tcp(
            key.1,
            &wire::Tcp {
                sport: key.2,
                dport: key.0,
                seq,
                ack,
                flags,
                window: 0,
                mss: None,
                data: &[],
            },
        );
    }

    fn tcp_in(&mut self, ip: &wire::Ipv4<'_>, seg: &wire::Tcp<'_>) {
        let now = self.now;
        let key: Key = (seg.sport, ip.dst, seg.dport);
        let shared = self.host.net.clone();
        let mut net = Net::new(&shared);
        let opening = seg.flags & (wire::SYN | wire::ACK | wire::RST) == wire::SYN;
        if let Some(c) = self.tcp.get_mut(&key) {
            // A new SYN replaces a finished connection of the same ports.
            if !(opening && c.is_lingering()) {
                if c.on_segment(&mut self.link, key, seg, now) == Step::Remove {
                    c.close_host(&mut net);
                    self.tcp.remove(&key);
                } else {
                    c.output(&mut self.link, key, now, false);
                }
                return;
            }
            self.tcp.remove(&key);
        }
        if seg.has(wire::RST) {
            return;
        }
        if !opening {
            self.tcp_refuse(key, seg);
            return;
        }
        let name = match self.target(ip.dst) {
            Target::Name(n) => n,
            Target::Refuse => {
                self.tcp_refuse(key, seg);
                return;
            }
            Target::Drop => return,
        };
        if !self.tcp_room() {
            self.tcp_refuse(key, seg);
            return;
        }
        match net.connect(SocketKind::Tcp, &name, seg.dport) {
            Ok(h) => {
                let iss = self.next_iss();
                self.tcp.insert(key, Conn::outbound(seg, h, iss, now));
            }
            Err(_) => self.tcp_refuse(key, seg),
        }
    }

    /// A connection accepted on a forward: connect it to the guest.
    fn tcp_accepted(&mut self, h: NetHandle, guest_port: u16, net: &mut Net<'_>, now: u64) {
        let known = self.link.guest_mac.is_some() && self.link.guest_ip.is_some();
        if !known || !self.tcp_room() {
            net.close(h);
            return;
        }
        let gateway = self.config.gateway;
        for _ in 0..0x4000 {
            let port = 0xc000 + (self.port_next & 0x3fff);
            self.port_next = self.port_next.wrapping_add(1);
            let key: Key = (guest_port, gateway, port);
            if !self.tcp.contains_key(&key) {
                let iss = self.next_iss();
                let c = Conn::inbound(h, iss, &mut self.link, key, now);
                self.tcp.insert(key, c);
                return;
            }
        }
        net.close(h);
    }

    fn poll_listeners(&mut self, net: &mut Net<'_>, now: u64) {
        if !net.is_some() {
            return;
        }
        if now >= self.listen_retry_at {
            let mut failed = false;
            for (f, l) in self.config.forwards.iter().zip(&mut self.listeners) {
                if l.is_none() {
                    *l = net.listen(f.host_port).ok();
                    failed |= l.is_none();
                }
            }
            if failed {
                self.listen_retry_at = now.saturating_add(LISTEN_RETRY_US);
            }
        }
        for i in 0..self.listeners.len() {
            let (Some(l), Some(f)) = (self.listeners[i], self.config.forwards.get(i)) else {
                continue;
            };
            let guest_port = f.guest_port;
            for _ in 0..ACCEPTS_PER_POLL {
                match net.accept(l) {
                    Ok(Some(h)) => self.tcp_accepted(h, guest_port, net, now),
                    Ok(None) => break,
                    Err(_) => {
                        net.close(l);
                        self.listeners[i] = None;
                        self.listen_retry_at = now.saturating_add(LISTEN_RETRY_US);
                        break;
                    }
                }
            }
        }
    }

    fn poll_tcp(&mut self, net: &mut Net<'_>, now: u64) {
        let link = &mut self.link;
        self.tcp.retain(|key, c| {
            if c.poll(link, *key, net, now) == Step::Remove {
                c.close_host(net);
                false
            } else {
                true
            }
        });
    }

    /// Let time pass: poll sockets, run timers, queue frames for the
    /// guest. Call regularly (e.g. every emulated millisecond).
    pub fn poll(&mut self, now_us: u64) {
        self.now = self.now.max(now_us);
        let now = self.now;
        let shared = self.host.net.clone();
        let mut net = Net::new(&shared);
        self.poll_listeners(&mut net, now);
        self.poll_udp(&mut net, now);
        self.poll_tcp(&mut net, now);
    }

    // --- savestate -------------------------------------------------------------

    pub fn save(&self, w: &mut StateWriter) {
        w.chunk(*b"VRT0", 1, |w| {
            w.u64(self.now);
            w.bool(self.link.guest_mac.is_some());
            w.raw(&self.link.guest_mac.unwrap_or_default());
            w.bool(self.link.guest_ip.is_some());
            w.raw(&self.link.guest_ip.unwrap_or_default());
            w.u16(self.link.ip_id);
            w.u32(self.link.out.len() as u32);
            for f in &self.link.out {
                w.bytes(f);
            }
            w.u32(self.dns_next);
            w.u32(self.dns.len() as u32);
            for (name, ip) in &self.dns {
                w.str(name);
                w.raw(ip);
            }
            w.u32(self.iss_next);
            w.u16(self.port_next);
            w.u32(self.udp.len() as u32);
            for (key, f) in &self.udp {
                save_key(w, key);
                w.str(&f.name);
                w.u64(f.last_us);
                w.u32(f.pending.len() as u32);
                for d in &f.pending {
                    w.bytes(d);
                }
            }
            w.u32(self.tcp.len() as u32);
            for (key, c) in &self.tcp {
                save_key(w, key);
                c.save(w);
            }
        });
    }

    /// Atomic: on an error nothing changes. Host sockets open before the
    /// load are closed; see the module documentation for what happens to
    /// the loaded connections.
    pub fn load(&mut self, r: &mut StateReader<'_>) -> Result<(), StateError> {
        let (_, mut c) = r.chunk(*b"VRT0", 1)?;
        let bad = StateError::Invalid("router state");
        let now = c.u64()?;
        let mut link = Link::default();
        let has_mac = c.bool()?;
        let mut mac = [0u8; 6];
        c.raw_into(&mut mac)?;
        link.guest_mac = has_mac.then_some(mac);
        let has_ip = c.bool()?;
        let mut ip = [0u8; 4];
        c.raw_into(&mut ip)?;
        link.guest_ip = has_ip.then_some(ip);
        link.ip_id = c.u16()?;
        let frames = c.u32()? as usize;
        if frames > OUT_MAX {
            return Err(bad);
        }
        for _ in 0..frames {
            let f = c.bytes()?;
            if f.len() < MIN_FRAME || f.len() > MAX_FRAME {
                return Err(bad);
            }
            link.out.push_back(f.to_vec());
        }
        let dns_next = c.u32()? % POOL;
        let names = c.u32()? as usize;
        if names > DNS_MAX {
            return Err(bad);
        }
        let mut dns = Vec::with_capacity(names);
        for _ in 0..names {
            let name = c.string()?;
            let mut ip = [0u8; 4];
            c.raw_into(&mut ip)?;
            if name.is_empty() || name.len() > NAME_MAX {
                return Err(bad);
            }
            dns.push((name, ip));
        }
        let iss_next = c.u32()?;
        let port_next = c.u16()?;
        let flows = c.u32()? as usize;
        if flows > UDP_MAX {
            return Err(bad);
        }
        let mut udp = BTreeMap::new();
        for _ in 0..flows {
            let key = load_key(&mut c)?;
            let name = c.string()?;
            let last_us = c.u64()?;
            let count = c.u32()? as usize;
            if name.is_empty() || name.len() > NAME_MAX || count > UDP_PENDING_MAX {
                return Err(bad);
            }
            let mut pending = VecDeque::with_capacity(count);
            for _ in 0..count {
                let d = c.bytes()?;
                if d.len() > UDP_MAX_DATA {
                    return Err(bad);
                }
                pending.push_back(d.to_vec());
            }
            udp.insert(
                key,
                Flow {
                    handle: None,
                    name,
                    last_us,
                    pending,
                },
            );
        }
        let conns = c.u32()? as usize;
        if conns > TCP_MAX {
            return Err(bad);
        }
        let mut tcp = BTreeMap::new();
        for _ in 0..conns {
            let key = load_key(&mut c)?;
            tcp.insert(key, Conn::load(&mut c)?);
        }

        self.drop_host_handles();
        self.now = now;
        self.link = link;
        self.dns = dns;
        self.dns_next = dns_next;
        self.iss_next = iss_next;
        self.port_next = port_next;
        self.udp = udp;
        self.tcp = tcp;
        Ok(())
    }
}

fn save_key(w: &mut StateWriter, key: &Key) {
    w.u16(key.0);
    w.raw(&key.1);
    w.u16(key.2);
}

fn load_key(r: &mut StateReader<'_>) -> Result<Key, StateError> {
    let port = r.u16()?;
    let mut ip = [0u8; 4];
    r.raw_into(&mut ip)?;
    Ok((port, ip, r.u16()?))
}
