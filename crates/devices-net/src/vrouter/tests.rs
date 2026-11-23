//! Tests of the virtual router with a small test-side guest: it builds
//! and parses frames and keeps just enough TCP state to talk.

use std::sync::{Arc, Mutex, MutexGuard};

use mimic_core::host::{lock, HostServices, NetHandle, SharedNet, SocketKind};
use mimic_core::{StateError, StateReader, StateWriter};

use super::tcp::TX_CAP;
use super::wire::{self, Addr, ACK, FIN, PSH, RST, SYN};
use super::*;
use crate::testing::FakeNet;

const GMAC: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const GIP: [u8; 4] = [10, 0, 2, 15];
const GW: [u8; 4] = [10, 0, 2, 2];
const DNS: [u8; 4] = [10, 0, 2, 3];

/// A TCP segment from the router.
struct Seg {
    src: [u8; 4],
    sport: u16,
    dport: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: Option<u16>,
    data: Vec<u8>,
}

fn tcp_of(frame: &[u8]) -> Option<Seg> {
    let eth = wire::parse_eth(frame)?;
    if eth.ethertype != wire::ETHERTYPE_IPV4 {
        return None;
    }
    let ip = wire::parse_ipv4(eth.payload)?;
    if ip.proto != wire::PROTO_TCP {
        return None;
    }
    let t = wire::parse_tcp(&ip)?;
    Some(Seg {
        src: ip.src,
        sport: t.sport,
        dport: t.dport,
        seq: t.seq,
        ack: t.ack,
        flags: t.flags,
        window: t.window,
        mss: t.mss,
        data: t.data.to_vec(),
    })
}

/// Source address, source port, destination port and data of a UDP frame.
fn udp_of(frame: &[u8]) -> Option<([u8; 4], u16, u16, Vec<u8>)> {
    let eth = wire::parse_eth(frame)?;
    if eth.ethertype != wire::ETHERTYPE_IPV4 {
        return None;
    }
    let ip = wire::parse_ipv4(eth.payload)?;
    if ip.proto != wire::PROTO_UDP {
        return None;
    }
    let u = wire::parse_udp(&ip)?;
    Some((ip.src, u.sport, u.dport, u.data.to_vec()))
}

/// The guest's end of a TCP connection.
struct GTcp {
    sport: u16,
    peer: [u8; 4],
    pport: u16,
    seq: u32,
    ack: u32,
    wnd: u16,
    got: Vec<u8>,
    fin: bool,
    rst: bool,
    peer_ack: u32,
    peer_wnd: u16,
    max_seg: usize,
    /// Data segments refused (wrong number or beyond the window).
    refused: usize,
}

struct Guest {
    r: Router,
    net: Arc<Mutex<FakeNet>>,
    now: u64,
    ip: [u8; 4],
    /// Every frame from the router.
    log: Vec<Vec<u8>>,
    /// Every frame to the router.
    sent: Vec<Vec<u8>>,
}

fn forward_config() -> RouterConfig {
    RouterConfig {
        forwards: vec![Forward {
            host_port: 6464,
            guest_port: 64,
        }],
        ..RouterConfig::default()
    }
}

impl Guest {
    fn new(config: RouterConfig) -> Self {
        let mut fake = FakeNet::new();
        for h in ["peer.example", "udp.example", "127.0.0.1", "192.168.1.50"] {
            fake.script_host(h);
        }
        let net = Arc::new(Mutex::new(fake));
        let shared: SharedNet = net.clone();
        let mut r = Router::new(config);
        r.attach_host(HostServices {
            net: Some(shared),
            files: None,
        });
        Self {
            r,
            net,
            now: 0,
            ip: GIP,
            log: Vec::new(),
            sent: Vec::new(),
        }
    }

    /// A guest the router knows already (static address, one ARP).
    fn known() -> Self {
        let mut g = Self::new(RouterConfig::default());
        assert!(g.arp(GIP, GW).is_some());
        g
    }

    fn fake(&self) -> MutexGuard<'_, FakeNet> {
        lock(&self.net)
    }

    fn last_handle(&self) -> NetHandle {
        self.fake().connects.last().expect("a connect").0
    }

    fn send(&mut self, frame: Vec<u8>) {
        self.r.guest_frame(&frame, self.now);
        self.sent.push(frame);
    }

    /// The frames waiting; each one is checked for size, address and
    /// checksums.
    fn frames(&mut self) -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        while self.r.has_frame() {
            let f = self.r.take_frame().expect("a frame");
            assert!((60..=1514).contains(&f.len()), "frame of {} bytes", f.len());
            let eth = wire::parse_eth(&f).expect("ethernet");
            assert!(eth.dst == GMAC || eth.dst == wire::BROADCAST_MAC);
            assert_eq!(eth.src, GATEWAY_MAC);
            if eth.ethertype == wire::ETHERTYPE_IPV4 {
                let ip = wire::parse_ipv4(eth.payload).expect("valid IPv4");
                match ip.proto {
                    wire::PROTO_TCP => assert!(wire::parse_tcp(&ip).is_some()),
                    wire::PROTO_UDP => assert!(wire::parse_udp(&ip).is_some()),
                    wire::PROTO_ICMP => assert_eq!(wire::fold(wire::sum(ip.payload, 0)), 0),
                    p => panic!("protocol {p}"),
                }
            }
            self.log.push(f.clone());
            v.push(f);
        }
        v
    }

    /// Let `ms` milliseconds pass, polling every millisecond.
    fn run(&mut self, ms: u64) -> Vec<Vec<u8>> {
        let mut v = self.frames();
        for _ in 0..ms {
            self.now += 1000;
            self.r.poll(self.now);
            v.extend(self.frames());
        }
        v
    }

    fn addr(&self, dst: [u8; 4]) -> Addr {
        Addr {
            dst_mac: GATEWAY_MAC,
            src_mac: GMAC,
            src: self.ip,
            dst,
            ident: 7,
        }
    }

    /// ARP request from `spa` for `tpa`; the reply, if any.
    fn arp(&mut self, spa: [u8; 4], tpa: [u8; 4]) -> Option<wire::Arp> {
        let req = wire::Arp {
            op: 1,
            sha: GMAC,
            spa,
            tha: [0; 6],
            tpa,
        };
        self.send(wire::arp_frame(wire::BROADCAST_MAC, GMAC, &req));
        let frames = self.frames();
        let f = frames.first()?;
        let eth = wire::parse_eth(f)?;
        assert_eq!(eth.ethertype, wire::ETHERTYPE_ARP);
        wire::parse_arp(eth.payload)
    }

    fn udp(&mut self, sport: u16, dst: [u8; 4], dport: u16, data: &[u8]) {
        let f = wire::udp_frame(&self.addr(dst), sport, dport, data).expect("fits");
        self.send(f);
    }

    /// Ask the router's name server; the whole answer.
    fn dns(&mut self, name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.extend_from_slice(&[0, (qtype >> 8) as u8, qtype as u8, 0, 1]);
        self.udp(5353, DNS, 53, &q);
        let frames = self.frames();
        let (src, sport, dport, data) = frames.iter().find_map(|f| udp_of(f)).expect("answer");
        assert_eq!((src, sport, dport), (DNS, 53, 5353));
        assert_eq!(data[..2], [0x12, 0x34]);
        // The question comes back.
        assert_eq!(data[12..q.len()], q[12..]);
        data
    }

    /// The address the name server gives `name`.
    fn resolve(&mut self, name: &str) -> [u8; 4] {
        let a = self.dns(name, 1);
        assert_eq!(a[2..4], [0x81, 0x80], "flags");
        assert_eq!(a[6..8], [0, 1], "one answer");
        let n = a.len();
        assert_eq!(a[n - 16..n - 4], [0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        [a[n - 4], a[n - 3], a[n - 2], a[n - 1]]
    }

    fn seg(&mut self, t: &GTcp, flags: u8, data: &[u8]) {
        let seg = wire::Tcp {
            sport: t.sport,
            dport: t.pport,
            seq: t.seq,
            ack: t.ack,
            flags,
            window: t.wnd,
            mss: (flags & SYN != 0).then_some(1460),
            data,
        };
        let f = wire::tcp_frame(&self.addr(t.peer), &seg).expect("fits");
        self.send(f);
    }

    /// Send a SYN.
    fn open(&mut self, peer: [u8; 4], pport: u16, sport: u16, wnd: u16) -> GTcp {
        let mut t = GTcp {
            sport,
            peer,
            pport,
            seq: 1000,
            ack: 0,
            wnd,
            got: Vec::new(),
            fin: false,
            rst: false,
            peer_ack: 0,
            peer_wnd: 0,
            max_seg: 0,
            refused: 0,
        };
        self.seg(&t, SYN, &[]);
        t.seq += 1;
        t
    }

    /// Open a connection and wait for the answer (SYN-ACK or RST).
    fn connect(&mut self, peer: [u8; 4], pport: u16, sport: u16, wnd: u16) -> GTcp {
        let mut t = self.open(peer, pport, sport, wnd);
        for _ in 0..50 {
            for f in self.run(1) {
                let Some(s) = tcp_of(&f) else { continue };
                assert_eq!((s.src, s.sport, s.dport), (peer, pport, sport));
                if s.flags & RST != 0 {
                    assert_eq!(s.ack, t.seq);
                    t.rst = true;
                    return t;
                }
                assert_eq!(s.flags, SYN | ACK);
                assert_eq!(s.ack, t.seq);
                assert_eq!(s.mss, Some(1460));
                t.ack = s.seq.wrapping_add(1);
                t.peer_ack = s.ack;
                t.peer_wnd = s.window;
                self.seg(&t, ACK, &[]);
                return t;
            }
        }
        panic!("no answer to the SYN");
    }

    /// The guest's TCP: take what continues the stream and fits the
    /// window, acknowledge everything.
    fn handle(&mut self, t: &mut GTcp, frames: &[Vec<u8>]) {
        for f in frames {
            let Some(s) = tcp_of(f) else { continue };
            if (s.src, s.sport, s.dport) != (t.peer, t.pport, t.sport) {
                continue;
            }
            if s.flags & RST != 0 {
                t.rst = true;
                continue;
            }
            if s.flags & ACK != 0 {
                t.peer_ack = s.ack;
                t.peer_wnd = s.window;
            }
            if s.data.is_empty() && s.flags & FIN == 0 {
                continue;
            }
            if s.seq == t.ack && s.data.len() <= usize::from(t.wnd) {
                t.max_seg = t.max_seg.max(s.data.len());
                t.got.extend_from_slice(&s.data);
                t.ack = t.ack.wrapping_add(s.data.len() as u32);
                if s.flags & FIN != 0 {
                    t.ack = t.ack.wrapping_add(1);
                    t.fin = true;
                }
            } else {
                t.refused += 1;
            }
            self.seg(t, ACK, &[]);
        }
    }

    fn pump(&mut self, t: &mut GTcp, ms: u64) {
        for _ in 0..ms {
            let f = self.run(1);
            self.handle(t, &f);
        }
    }

    fn write(&mut self, t: &mut GTcp, data: &[u8]) {
        self.seg(t, ACK | PSH, data);
        t.seq = t.seq.wrapping_add(data.len() as u32);
    }

    fn close(&mut self, t: &mut GTcp) {
        self.seg(t, FIN | ACK, &[]);
        t.seq = t.seq.wrapping_add(1);
    }

    /// A DHCP message from the guest (it has no address yet).
    fn dhcp(&mut self, kind: u8, ciaddr: [u8; 4], broadcast: bool, options: &[u8]) -> Vec<u8> {
        let mut m = vec![0u8; 240];
        m[0] = 1;
        m[1] = 1;
        m[2] = 6;
        m[4..8].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        m[10] = if broadcast { 0x80 } else { 0 };
        m[12..16].copy_from_slice(&ciaddr);
        m[28..34].copy_from_slice(&GMAC);
        m[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        m.extend_from_slice(&[53, 1, kind]);
        m.extend_from_slice(options);
        m.push(255);
        let a = Addr {
            dst_mac: wire::BROADCAST_MAC,
            src_mac: GMAC,
            src: ciaddr,
            dst: [255; 4],
            ident: 1,
        };
        self.send(wire::udp_frame(&a, 68, 67, &m).expect("fits"));
        let frames = self.frames();
        assert_eq!(frames.len(), 1);
        let (src, sport, dport, data) = udp_of(&frames[0]).expect("DHCP answer");
        assert_eq!((src, sport, dport), (GW, 67, 68));
        assert!(data.len() >= 300);
        assert_eq!(data[0], 2);
        assert_eq!(data[4..8], [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(data[28..34], GMAC);
        data
    }
}

fn dhcp_option(m: &[u8], want: u8) -> Option<Vec<u8>> {
    let mut i = 240;
    while i + 1 < m.len() {
        match m[i] {
            0 => i += 1,
            255 => break,
            code => {
                let len = usize::from(m[i + 1]);
                if code == want {
                    return Some(m[i + 2..i + 2 + len].to_vec());
                }
                i += 2 + len;
            }
        }
    }
    None
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 + i / 251 + 3) as u8).collect()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[test]
fn internet_checksum() {
    // The header example found in many texts: checksum $B861.
    let mut h = [
        0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61, 0xc0, 0xa8, 0x00,
        0x01, 0xc0, 0xa8, 0x00, 0xc7,
    ];
    assert_eq!(wire::fold(wire::sum(&h, 0)), 0);
    h[10] = 0;
    h[11] = 0;
    assert_eq!(wire::fold(wire::sum(&h, 0)), 0xb861);
    // An odd byte counts as the high byte of a word.
    assert_eq!(wire::sum(&[0x12, 0x34, 0x56], 0), 0x1234 + 0x5600);
}

#[test]
fn arp_answers_for_router_and_outside() {
    let mut g = Guest::new(RouterConfig::default());
    for tpa in [GW, DNS, [8, 8, 8, 8], [192, 168, 1, 50]] {
        let a = g.arp(GIP, tpa).expect("answer");
        assert_eq!(a.op, 2);
        assert_eq!(a.sha, GATEWAY_MAC);
        assert_eq!(a.spa, tpa);
        assert_eq!((a.tha, a.tpa), (GMAC, GIP));
    }
    // Other addresses of the subnet, and probes for the guest's own.
    assert!(g.arp(GIP, [10, 0, 2, 99]).is_none());
    assert!(g.arp([0; 4], GIP).is_none());
    assert!(g.arp(GIP, GIP).is_none());
    // A probe from 0.0.0.0 for the gateway is answered too.
    assert!(g.arp([0; 4], GW).is_some());
}

#[test]
fn dhcp_exchange() {
    let mut g = Guest::new(RouterConfig::default());
    let offer = g.dhcp(1, [0; 4], false, &[]);
    assert_eq!(dhcp_option(&offer, 53), Some(vec![2]));
    assert_eq!(offer[16..20], GIP);
    assert_eq!(dhcp_option(&offer, 54), Some(GW.to_vec()));
    // Unicast to the new address at the client's MAC.
    let eth = wire::parse_eth(g.log.last().unwrap()).unwrap();
    assert_eq!(eth.dst, GMAC);
    assert_eq!(wire::parse_ipv4(eth.payload).unwrap().dst, GIP);

    let mut req = vec![50, 4];
    req.extend_from_slice(&GIP);
    req.extend_from_slice(&[54, 4]);
    req.extend_from_slice(&GW);
    let ack = g.dhcp(3, [0; 4], true, &req);
    assert_eq!(dhcp_option(&ack, 53), Some(vec![5]));
    assert_eq!(ack[16..20], GIP);
    assert_eq!(dhcp_option(&ack, 1), Some(vec![255, 255, 255, 0]));
    assert_eq!(dhcp_option(&ack, 3), Some(GW.to_vec()));
    assert_eq!(dhcp_option(&ack, 6), Some(DNS.to_vec()));
    assert_eq!(
        dhcp_option(&ack, 51),
        Some(86_400u32.to_be_bytes().to_vec())
    );
    // The broadcast flag was set.
    let eth = wire::parse_eth(g.log.last().unwrap()).unwrap();
    assert_eq!(eth.dst, wire::BROADCAST_MAC);
    assert_eq!(wire::parse_ipv4(eth.payload).unwrap().dst, [255; 4]);

    // The router knows the guest now: a ping gets through.
    let echo = wire::icmp_message(8, 0, &[0, 1, 0, 2, b'h', b'i']);
    let f = wire::ipv4_frame(&g.addr(GW), wire::PROTO_ICMP, &echo).unwrap();
    g.send(f);
    assert_eq!(g.frames().len(), 1);

    // Renewal (ciaddr set, no options), INFORM, a foreign address, a
    // request to another server.
    let renew = g.dhcp(3, GIP, false, &[]);
    assert_eq!(dhcp_option(&renew, 53), Some(vec![5]));
    assert_eq!(renew[12..16], GIP);
    let inform = g.dhcp(8, GIP, false, &[]);
    assert_eq!(dhcp_option(&inform, 53), Some(vec![5]));
    assert_eq!(inform[16..20], [0; 4]);
    assert_eq!(dhcp_option(&inform, 51), None);
    assert_eq!(dhcp_option(&inform, 6), Some(DNS.to_vec()));
    let nak = g.dhcp(3, [0; 4], false, &[50, 4, 10, 0, 2, 77]);
    assert_eq!(dhcp_option(&nak, 53), Some(vec![6]));
    let mut m = vec![0u8; 240];
    m[..3].copy_from_slice(&[1, 1, 6]);
    m[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    m.extend_from_slice(&[53, 1, 3, 54, 4, 192, 168, 1, 1, 255]);
    let a = Addr {
        dst_mac: wire::BROADCAST_MAC,
        src_mac: GMAC,
        src: [0; 4],
        dst: [255; 4],
        ident: 1,
    };
    g.send(wire::udp_frame(&a, 68, 67, &m).unwrap());
    assert!(g.frames().is_empty());
}

#[test]
fn ping_the_gateway() {
    let mut g = Guest::new(RouterConfig::default());
    // A guest with a static address in the subnet, never seen before.
    g.ip = [10, 0, 2, 40];
    let payload: Vec<u8> = (0..56).collect();
    let mut rest = vec![0xab, 0xcd, 0x00, 0x05];
    rest.extend_from_slice(&payload);
    let echo = wire::icmp_message(8, 0, &rest);
    for dst in [GW, DNS] {
        let f = wire::ipv4_frame(&g.addr(dst), wire::PROTO_ICMP, &echo).unwrap();
        g.send(f);
        let frames = g.frames();
        assert_eq!(frames.len(), 1);
        let eth = wire::parse_eth(&frames[0]).unwrap();
        let ip = wire::parse_ipv4(eth.payload).unwrap();
        assert_eq!((ip.src, ip.dst, ip.proto), (dst, g.ip, wire::PROTO_ICMP));
        assert_eq!(ip.payload[..2], [0, 0]);
        assert_eq!(ip.payload[4..], rest[..]);
    }
    // Outside hosts do not answer pings; a broken checksum is dropped.
    let f = wire::ipv4_frame(&g.addr([8, 8, 8, 8]), wire::PROTO_ICMP, &echo).unwrap();
    g.send(f);
    let mut bad = echo.clone();
    bad[10] ^= 1;
    let f = wire::ipv4_frame(&g.addr(GW), wire::PROTO_ICMP, &bad).unwrap();
    g.send(f);
    assert!(g.run(5).is_empty());
}

#[test]
fn dns_gives_synthetic_addresses() {
    let mut g = Guest::known();
    assert_eq!(g.resolve("echo"), [198, 18, 1, 1]);
    assert_eq!(g.resolve("Peer.Example"), [198, 18, 1, 2]);
    assert_eq!(g.resolve("echo"), [198, 18, 1, 1]);
    assert_eq!(g.resolve("peer.example"), [198, 18, 1, 2]);
    // A literal address is itself.
    assert_eq!(g.resolve("192.168.1.50"), [192, 168, 1, 50]);
    // AAAA: no error, no records.
    let a = g.dns("echo", 28);
    assert_eq!(a[2..4], [0x81, 0x80]);
    assert_eq!(a[6..8], [0, 0]);
    // A response, or a query that is cut short, gets no answer.
    g.udp(
        5353,
        DNS,
        53,
        &[0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0],
    );
    g.udp(5353, DNS, 53, &[0x12, 0x34, 0x01]);
    assert!(g.frames().is_empty());
    // A question that ends early: format error.
    g.udp(
        5353,
        DNS,
        53,
        &[0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 5, b'a'],
    );
    let (_, _, _, data) = udp_of(&g.frames()[0]).unwrap();
    assert_eq!(data[2..4], [0x81, 0x81]);
}

#[test]
fn dns_table_recycles_the_oldest_name() {
    let mut g = Guest::known();
    let first = g.resolve("host0.example");
    for i in 1..DNS_MAX {
        g.resolve(&format!("host{i}.example"));
    }
    // Asking again keeps a name young.
    assert_eq!(g.resolve("host0.example"), first);
    g.resolve("one.more.example");
    assert_eq!(g.r.dns.len(), DNS_MAX);
    assert_eq!(g.resolve("host0.example"), first);
    // host1 was forgotten and gets a new address.
    assert_ne!(g.resolve("host1.example"), [198, 18, 1, 2]);
    // A connection to a forgotten address is refused.
    let t = g.connect([198, 18, 1, 2], 80, 40000, 4096);
    assert!(t.rst);
}

#[test]
fn tcp_by_name_reaches_the_host() {
    let mut g = Guest::known();
    let ip = g.resolve("echo");
    let mut t = g.connect(ip, 6400, 40000, 4096);
    assert!(!t.rst);
    assert_eq!(
        g.fake().connects.last().map(|c| (c.1, c.2.clone(), c.3)),
        Some((SocketKind::Tcp, "echo".to_string(), 6400))
    );
    g.write(&mut t, b"hello");
    g.pump(&mut t, 3);
    assert_eq!(t.got, b"hello");
    assert_eq!(t.peer_ack, t.seq);

    // The guest closes first: the host socket goes, the FIN comes back.
    let h = g.last_handle();
    g.close(&mut t);
    g.pump(&mut t, 3);
    assert!(!g.fake().is_open(h));
    assert!(t.fin && !t.rst);
    assert_eq!(t.peer_ack, t.seq);
    // After the linger time the connection is forgotten: RST.
    g.pump(&mut t, 2100);
    g.seg(&t, ACK, &[]);
    g.pump(&mut t, 1);
    assert!(t.rst);
    assert!(g.r.tcp.is_empty());
}

#[test]
fn tcp_literal_and_gateway_addresses() {
    let mut g = Guest::known();
    let t = g.connect([192, 168, 1, 50], 8080, 40000, 4096);
    assert!(!t.rst);
    assert_eq!(g.fake().connects.last().unwrap().2, "192.168.1.50");
    // The gateway is the PC itself.
    let t = g.connect(GW, 8080, 40001, 4096);
    assert!(!t.rst);
    assert_eq!(g.fake().connects.last().unwrap().2, "127.0.0.1");
    // Nothing answers TCP on the name server; the subnet is empty.
    let t = g.connect(DNS, 53, 40002, 4096);
    assert!(t.rst);
    g.open([10, 0, 2, 99], 80, 40003, 4096);
    g.open([224, 0, 0, 1], 80, 40004, 4096);
    assert!(g.run(20).is_empty());
    assert_eq!(g.fake().connects.len(), 2);
}

#[test]
fn tcp_refused_is_a_reset() {
    let mut g = Guest::known();
    // FakeNet refuses unknown hosts and does not resolve `.invalid`.
    let t = g.connect([203, 0, 113, 9], 80, 40000, 4096);
    assert!(t.rst);
    assert_eq!(g.fake().connects.last().unwrap().2, "203.0.113.9");
    let ip = g.resolve("nowhere.invalid");
    let t = g.connect(ip, 80, 40001, 4096);
    assert!(t.rst);
    let h = g.last_handle();
    assert!(!g.fake().is_open(h));
    assert!(g.r.tcp.is_empty());

    // Without a network the router itself still answers.
    let mut g = Guest::new(RouterConfig::default());
    g.r.attach_host(HostServices::default());
    assert!(g.arp(GIP, GW).is_some());
    assert_eq!(g.resolve("echo"), [198, 18, 1, 1]);
    let t = g.connect([198, 18, 1, 1], 80, 40000, 4096);
    assert!(t.rst);
}

#[test]
fn tcp_host_closes_first() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let mut t = g.connect(ip, 21, 40000, 4096);
    let h = g.last_handle();
    g.fake().push_rx(h, b"bye");
    g.fake().peer_close(h);
    g.pump(&mut t, 3);
    assert_eq!(t.got, b"bye");
    assert!(t.fin);
    // Half closed: the guest can still send.
    assert!(g.fake().is_open(h));
    g.write(&mut t, b"late");
    g.pump(&mut t, 2);
    assert_eq!(g.fake().sent(h), b"late");
    assert_eq!(t.peer_ack, t.seq);
    g.close(&mut t);
    g.pump(&mut t, 2);
    assert!(!g.fake().is_open(h));
    assert_eq!(t.peer_ack, t.seq);
    assert!(!t.rst);
    g.pump(&mut t, 2100);
    assert!(g.r.tcp.is_empty());
}

#[test]
fn tcp_reset_from_the_guest_closes_the_host_socket() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let t = g.connect(ip, 23, 40000, 4096);
    let h = g.last_handle();
    assert!(g.fake().is_open(h));
    g.seg(&t, RST, &[]);
    assert!(!g.fake().is_open(h));
    assert!(g.r.tcp.is_empty());
    assert!(g.run(5).is_empty());
}

#[test]
fn tcp_large_transfer_to_a_small_window() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let mut t = g.connect(ip, 80, 40000, 300);
    let h = g.last_handle();
    let data = pattern(100_000);
    g.fake().push_rx(h, &data);
    g.pump(&mut t, 1);
    // The router takes only what its buffer holds.
    assert!(g.fake().unread(h) >= data.len() - TX_CAP - 300);
    for _ in 0..2000 {
        g.pump(&mut t, 1);
        if t.got.len() == data.len() {
            break;
        }
    }
    assert_eq!(t.got, data);
    assert_eq!(t.max_seg, 300);
    assert_eq!(t.refused, 0);
    assert_eq!(g.fake().unread(h), 0);
}

#[test]
fn tcp_large_transfer_to_a_slow_host() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let mut t = g.connect(ip, 80, 40000, 4096);
    let h = g.last_handle();
    let data = pattern(100_000);
    g.fake().set_quota(h, Some(0));
    let mut sent = 0;
    let mut min_wnd = u16::MAX;
    for _ in 0..1000 {
        // The guest sends what the window offered allows.
        loop {
            let in_flight = t.seq.wrapping_sub(t.peer_ack);
            let room = u32::from(t.peer_wnd).saturating_sub(in_flight) as usize;
            let n = room.min(1460).min(data.len() - sent);
            if n == 0 {
                break;
            }
            g.write(&mut t, &data[sent..sent + n]);
            sent += n;
        }
        // The host takes 700 bytes per millisecond.
        g.fake().set_quota(h, Some(700));
        g.pump(&mut t, 1);
        min_wnd = min_wnd.min(t.peer_wnd);
        if g.fake().sent(h).len() == data.len() {
            break;
        }
    }
    assert_eq!(g.fake().sent(h), data);
    assert_eq!(min_wnd, 0, "the window closes while the host is slow");
    assert_eq!(t.peer_ack, t.seq);
    assert!(!t.rst);
}

#[test]
fn tcp_out_of_order_and_repeated_segments() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let mut t = g.connect(ip, 80, 40000, 4096);
    let h = g.last_handle();
    // A segment after a hole is dropped and the old number acknowledged.
    let first = t.seq;
    t.seq = first.wrapping_add(3);
    g.seg(&t, ACK | PSH, b"def");
    g.pump(&mut t, 1);
    assert_eq!(t.peer_ack, first);
    t.seq = first;
    g.write(&mut t, b"abc");
    g.write(&mut t, b"def");
    // A repeat that overlaps: only its new part counts.
    t.seq = first.wrapping_add(4);
    g.write(&mut t, b"efgh");
    g.pump(&mut t, 2);
    assert_eq!(g.fake().sent(h), b"abcdefgh");
    assert_eq!(t.peer_ack, t.seq);
    // A keep-alive (one byte below the next number) is acknowledged.
    let before = g.log.len();
    t.seq = t.seq.wrapping_sub(1);
    g.seg(&t, ACK, &[]);
    t.seq = t.seq.wrapping_add(1);
    g.pump(&mut t, 1);
    assert_eq!(g.log.len(), before + 1);
    assert_eq!(g.fake().sent(h), b"abcdefgh");
}

#[test]
fn tcp_retransmits_and_probes_a_closed_window() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    let mut t = g.connect(ip, 80, 40000, 4096);
    let h = g.last_handle();
    // The guest loses a segment: it comes again after the time-out.
    g.fake().push_rx(h, b"again and again");
    let lost = g.run(5);
    assert_eq!(lost.len(), 1);
    assert_eq!(tcp_of(&lost[0]).unwrap().data, b"again and again");
    g.pump(&mut t, 900);
    assert!(t.got.is_empty());
    g.pump(&mut t, 200);
    assert_eq!(t.got, b"again and again");

    // The guest closes its window; data waits and one byte probes.
    t.wnd = 0;
    g.seg(&t, ACK, &[]);
    g.fake().push_rx(h, b"behind the window");
    g.pump(&mut t, 900);
    assert_eq!(t.refused, 0);
    g.pump(&mut t, 200);
    assert_eq!(t.refused, 1);
    g.pump(&mut t, 12_000);
    assert!(t.refused >= 3, "probes go on");
    assert!(!t.rst);
    assert_eq!(t.got, b"again and again");
    // The window opens: everything arrives at once.
    t.wnd = 4096;
    g.seg(&t, ACK, &[]);
    g.pump(&mut t, 2);
    assert_eq!(t.got, b"again and againbehind the window");

    // A guest that stops answering is given up on: RST, host socket
    // closed.
    g.fake().push_rx(h, b"anyone?");
    let mut reset = false;
    for _ in 0..70_000 {
        reset |= g
            .run(1)
            .iter()
            .any(|f| tcp_of(f).is_some_and(|s| s.flags & RST != 0));
    }
    assert!(reset);
    assert!(!g.fake().is_open(h));
    assert!(g.r.tcp.is_empty());
}

#[test]
fn tcp_connection_table_is_bounded() {
    let mut g = Guest::known();
    let ip = g.resolve("peer.example");
    for i in 0..TCP_MAX as u16 {
        let t = g.connect(ip, 80, 40000 + i, 4096);
        assert!(!t.rst);
    }
    let t = g.connect(ip, 80, 50000, 4096);
    assert!(t.rst);
    assert_eq!(g.r.tcp.len(), TCP_MAX);
}

#[test]
fn udp_flow_both_ways_and_timeout() {
    let mut g = Guest::known();
    let ip = g.resolve("udp.example");
    g.udp(7000, ip, 123, b"ping");
    g.udp(7000, ip, 123, b"ping2");
    assert!(g.run(3).is_empty());
    let h = g.last_handle();
    assert_eq!(
        g.fake().connects.last().map(|c| (c.1, c.2.clone(), c.3)),
        Some((SocketKind::Udp, "udp.example".to_string(), 123))
    );
    assert_eq!(g.fake().connects.len(), 1);
    assert_eq!(
        g.fake().sent_datagrams(h),
        [b"ping".to_vec(), b"ping2".to_vec()]
    );
    g.fake().push_rx(h, b"pong");
    g.fake().push_rx(h, &[0x55; 1472]);
    // Too big for one frame: dropped.
    g.fake().push_rx(h, &[0x66; 1473]);
    let frames = g.run(1);
    assert_eq!(frames.len(), 2);
    assert_eq!(udp_of(&frames[0]), Some((ip, 123, 7000, b"pong".to_vec())));
    assert_eq!(udp_of(&frames[1]).unwrap().3, vec![0x55; 1472]);
    assert_eq!(frames[1].len(), 1514);

    // Broadcasts stay inside.
    g.udp(64, [255; 4], 64, b"identify");
    g.udp(64, [10, 0, 2, 255], 64, b"identify");
    g.run(2);
    assert_eq!(g.fake().connects.len(), 1);

    // Quiet for a minute: the flow is closed.
    g.now += UDP_IDLE_US - 10_000;
    g.run(1);
    assert!(g.fake().is_open(h));
    g.run(10);
    assert!(!g.fake().is_open(h));
    assert!(g.r.udp.is_empty());

    // More flows than the table holds: the quietest goes.
    for i in 0..=UDP_MAX as u16 {
        g.udp(8000 + i, ip, 123, b"x");
        g.run(1);
    }
    assert_eq!(g.r.udp.len(), UDP_MAX);
    assert!(!g.r.udp.contains_key(&(8000, ip, 123)));
}

#[test]
fn forwarded_connection_to_the_guest() {
    let mut g = Guest::new(forward_config());
    let fwd = g.r.config.forwards[0];
    assert_eq!(g.r.forwards_status(), [(fwd, false)]);

    // The guest is unknown: connections are closed at once.
    g.fake().incoming = 1;
    assert!(g.run(2).is_empty());
    assert_eq!(g.r.forwards_status(), [(fwd, true)]);
    assert!(g.fake().listening(6464));
    let h = g.fake().accepted[0];
    assert!(!g.fake().is_open(h));

    assert!(g.arp(GIP, GW).is_some());
    g.fake().incoming = 1;
    let frames = g.run(1);
    assert_eq!(frames.len(), 1);
    let syn = tcp_of(&frames[0]).unwrap();
    assert_eq!((syn.src, syn.dport, syn.flags), (GW, 64, SYN));
    assert_eq!(syn.mss, Some(1460));
    assert!(syn.sport >= 49152);
    let h = g.fake().accepted[1];

    // The guest accepts.
    let mut t = GTcp {
        sport: 64,
        peer: GW,
        pport: syn.sport,
        seq: 5000,
        ack: syn.seq.wrapping_add(1),
        wnd: 8192,
        got: Vec::new(),
        fin: false,
        rst: false,
        peer_ack: 0,
        peer_wnd: 0,
        max_seg: 0,
        refused: 0,
    };
    g.seg(&t, SYN | ACK, &[]);
    t.seq += 1;
    let frames = g.frames();
    assert_eq!(frames.len(), 1);
    let ack = tcp_of(&frames[0]).unwrap();
    assert_eq!((ack.flags, ack.ack, ack.seq), (ACK, t.seq, t.ack));

    // Data both ways (accepted FakeNet sockets echo).
    g.write(&mut t, b"abc");
    g.pump(&mut t, 2);
    assert_eq!(g.fake().sent(h), b"abc");
    assert_eq!(t.got, b"abc");
    g.fake().push_rx(h, b"xyz");
    g.pump(&mut t, 2);
    assert_eq!(t.got, b"abcxyz");

    // The host closes, then the guest.
    g.fake().peer_close(h);
    g.pump(&mut t, 2);
    assert!(t.fin);
    g.close(&mut t);
    g.pump(&mut t, 2);
    assert!(!g.fake().is_open(h));
    assert_eq!(t.peer_ack, t.seq);
    assert!(!t.rst);

    // A closed port on the guest: its RST closes the host connection.
    g.fake().incoming = 1;
    let frames = g.run(1);
    let syn = tcp_of(&frames[0]).unwrap();
    assert_ne!(syn.sport, t.pport);
    let h = g.fake().accepted[2];
    let refuse = GTcp {
        pport: syn.sport,
        seq: 0,
        ack: syn.seq.wrapping_add(1),
        ..t
    };
    g.seg(&refuse, RST | ACK, &[]);
    assert!(!g.fake().is_open(h));

    // A guest that never answers: the SYN is repeated, then given up.
    g.fake().incoming = 1;
    let frames = g.run(70_000);
    let flags: Vec<u8> = frames.iter().map(|f| tcp_of(f).unwrap().flags).collect();
    assert_eq!(flags, [SYN, SYN, SYN, SYN, SYN, SYN, RST]);
    let h = g.fake().accepted[3];
    assert!(!g.fake().is_open(h));
}

#[test]
fn forward_listener_is_retried() {
    let mut g = Guest::new(forward_config());
    g.fake().refuse_listen = true;
    g.run(1);
    assert!(!g.r.forwards_status()[0].1);
    g.fake().refuse_listen = false;
    g.run(4900);
    assert!(!g.r.forwards_status()[0].1);
    g.run(200);
    assert!(g.r.forwards_status()[0].1);
}

/// A session with a bit of everything; returns the guest.
fn session() -> Guest {
    let mut g = Guest::new(forward_config());
    g.dhcp(1, [0; 4], false, &[]);
    let mut req = vec![50, 4];
    req.extend_from_slice(&GIP);
    g.dhcp(3, [0; 4], false, &req);
    g.arp(GIP, GW);
    let echo = wire::icmp_message(8, 0, &[0, 1, 0, 2, 1, 2, 3, 4]);
    let f = wire::ipv4_frame(&g.addr(GW), wire::PROTO_ICMP, &echo).unwrap();
    g.send(f);
    let ip = g.resolve("echo");
    let mut t = g.connect(ip, 6400, 40000, 2000);
    let data = pattern(5000);
    for chunk in data.chunks(1000) {
        g.write(&mut t, chunk);
        g.pump(&mut t, 2);
    }
    g.pump(&mut t, 20);
    assert_eq!(t.got, data);
    let udp = g.resolve("udp.example");
    g.udp(7000, udp, 123, b"ping");
    g.run(3);
    let h = g.last_handle();
    g.fake().push_rx(h, b"pong");
    g.run(2);
    g.close(&mut t);
    g.pump(&mut t, 5);
    let peer = g.resolve("peer.example");
    let mut t = g.connect(peer, 80, 40001, 500);
    g.write(&mut t, b"GET / HTTP/1.0\r\n\r\n");
    let h = g.last_handle();
    g.fake().push_rx(h, &pattern(3000));
    g.pump(&mut t, 3);
    g
}

#[test]
fn same_inputs_same_frames() {
    let a = session();
    let b = session();
    assert!(a.log.len() > 20, "{} frames", a.log.len());
    assert_eq!(a.log, b.log);
    let (mut wa, mut wb) = (StateWriter::new(), StateWriter::new());
    a.r.save(&mut wa);
    b.r.save(&mut wb);
    assert_eq!(wa.into_bytes(), wb.into_bytes());
}

fn saved(r: &Router) -> Vec<u8> {
    let mut w = StateWriter::new();
    r.save(&mut w);
    w.into_bytes()
}

#[test]
fn savestate_round_trip_mid_connection() {
    // A connection with data waiting in both buffers, a UDP flow, a
    // forwarded connection being set up.
    let mut g = session();
    g.fake().incoming = 1;
    g.udp(7001, [192, 168, 1, 50], 9, b"queued");
    g.run(1);
    let state = saved(&g.r);
    let open: Vec<NetHandle> = {
        let fake = g.fake();
        let accepted = fake.accepted.clone();
        fake.connects
            .iter()
            .map(|c| c.0)
            .chain(accepted)
            .filter(|h| fake.is_open(*h))
            .collect()
    };
    assert!(open.len() >= 3);

    // Into a fresh router on another host.
    let mut g2 = Guest::new(forward_config());
    g2.r.load(&mut StateReader::new(&state)).unwrap();
    assert_eq!(saved(&g2.r), state);
    assert_eq!(g2.r.link.guest_ip, Some(GIP));
    // Frames not fetched yet come first; then every connection that had
    // a host socket is reset, the listener stands again.
    g2.now = g.now;
    let frames = g2.run(1);
    let resets: Vec<Seg> = frames
        .iter()
        .filter_map(|f| tcp_of(f))
        .filter(|s| s.flags & RST != 0)
        .collect();
    assert_eq!(resets.len(), 2);
    assert!(resets.iter().any(|s| s.sport == 80 && s.dport == 40001));
    assert!(resets.iter().any(|s| s.src == GW && s.dport == 64));
    assert!(g2.r.tcp.values().all(|c| c.is_lingering()));
    assert!(g2.fake().listening(6464));
    // The names keep their addresses; the UDP flow has a new socket and
    // still knows where it leads.
    assert_eq!(g2.resolve("peer.example"), [198, 18, 1, 3]);
    assert_eq!(g2.resolve("new.example"), [198, 18, 1, 4]);
    let udp: Vec<_> = g2
        .fake()
        .connects
        .iter()
        .map(|c| (c.1, c.2.clone(), c.3))
        .collect();
    assert_eq!(
        udp,
        [
            (SocketKind::Udp, "udp.example".to_string(), 123),
            (SocketKind::Udp, "192.168.1.50".to_string(), 9)
        ]
    );

    // Loading into the running router closes its sockets.
    g.r.load(&mut StateReader::new(&state)).unwrap();
    for h in open {
        assert!(!g.fake().is_open(h));
    }
    assert!(!g.fake().listening(6464));
    g.run(1);
    assert!(g.fake().listening(6464));
}

#[test]
fn truncated_and_corrupt_states() {
    let mut g = session();
    let state = saved(&g.r);
    for len in 0..state.len() {
        let r = g.r.load(&mut StateReader::new(&state[..len]));
        assert!(
            matches!(r, Err(StateError::Truncated | StateError::Missing(_))),
            "length {len}: {r:?}"
        );
    }
    // A failed load changes nothing.
    assert_eq!(saved(&g.r), state);
    assert!(g.fake().listening(6464));

    // Corrupt bytes: an error or a state the router can go on with.
    let corpus = g.sent.clone();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..400 {
        let mut s = state.clone();
        for _ in 0..1 + rng.below(4) {
            let i = 10 + rng.below(s.len() - 10);
            s[i] = rng.next() as u8;
        }
        let mut g2 = Guest::new(forward_config());
        if g2.r.load(&mut StateReader::new(&s)).is_ok() {
            g2.now = g2.r.now;
            for _ in 0..40 {
                let f = corpus[rng.below(corpus.len())].clone();
                g2.r.guest_frame(&f, g2.now);
                g2.now += 250_000;
                g2.r.poll(g2.now);
                while let Some(f) = g2.r.take_frame() {
                    assert!((60..=1514).contains(&f.len()));
                }
            }
            saved(&g2.r);
        }
    }
}

/// Make the checksums of a (mutated) frame right again, as far as its
/// lengths allow.
fn fix_checksums(f: &mut [u8]) {
    if f.len() < 34 || f[12..14] != [0x08, 0x00] {
        return;
    }
    let ihl = usize::from(f[14] & 0x0f) * 4;
    let total = usize::from(u16::from_be_bytes([f[16], f[17]]));
    if ihl < 20 || 14 + ihl > f.len() || total < ihl || 14 + total > f.len() {
        return;
    }
    f[24] = 0;
    f[25] = 0;
    let c = wire::fold(wire::sum(&f[14..14 + ihl], 0));
    f[24..26].copy_from_slice(&c.to_be_bytes());
    let src = [f[26], f[27], f[28], f[29]];
    let dst = [f[30], f[31], f[32], f[33]];
    let proto = f[23];
    let p = &mut f[14 + ihl..14 + total];
    let at = match proto {
        wire::PROTO_TCP => 16,
        wire::PROTO_UDP => 6,
        wire::PROTO_ICMP => 2,
        _ => return,
    };
    if p.len() < at + 2 {
        return;
    }
    p[at] = 0;
    p[at + 1] = 0;
    let start = if proto == wire::PROTO_ICMP {
        0
    } else {
        wire::pseudo(src, dst, proto, p.len())
    };
    let c = wire::fold(wire::sum(p, start));
    p[at..at + 2].copy_from_slice(&c.to_be_bytes());
}

#[test]
fn garbage_frames_never_panic() {
    let corpus = session().sent;
    assert!(corpus.len() > 20);
    let mut g = Guest::new(forward_config());
    g.fake().script_host("echo2");
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let mut frames_out = 0;
    for round in 0..40_000 {
        let mut f = corpus[rng.below(corpus.len())].clone();
        match rng.below(8) {
            // As it is: the session moves on.
            0 => {}
            1 => f.truncate(rng.below(f.len() + 1)),
            2 => {
                let n = rng.below(1600);
                f.extend((0..n).map(|_| rng.next() as u8));
            }
            3 => f = (0..rng.below(200)).map(|_| rng.next() as u8).collect(),
            _ => {
                for _ in 0..1 + rng.below(4) {
                    let i = rng.below(f.len());
                    f[i] = rng.next() as u8;
                }
            }
        }
        // Mostly with right checksums, so the mutation gets past them.
        if rng.below(4) != 0 {
            fix_checksums(&mut f);
        }
        g.r.guest_frame(&f, g.now);
        if round % 8 == 0 {
            g.now += 1000 * rng.below(3000) as u64;
            g.r.poll(g.now);
        }
        if round % 1000 == 0 {
            g.fake().incoming = 1;
        }
        while let Some(f) = g.r.take_frame() {
            assert!((60..=1514).contains(&f.len()));
            frames_out += 1;
        }
        assert!(g.r.tcp.len() <= TCP_MAX);
        assert!(g.r.udp.len() <= UDP_MAX);
        assert!(g.r.dns.len() <= DNS_MAX);
    }
    assert!(frames_out > 1000);
    // The state of whatever came of it loads again.
    let state = saved(&g.r);
    let mut g2 = Guest::new(forward_config());
    g2.r.load(&mut StateReader::new(&state)).unwrap();
    assert_eq!(saved(&g2.r), state);
}
