//! Tests of the gateway with hand-built frames and a fake host.

use std::collections::{BTreeMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};

use super::gateway::{Gateway, GATEWAY_IP, GATEWAY_MAC};
use super::outside::{Connect, HostOutside, Outside, TcpRead};
use super::packet::{
    self, Arp, Mac, ACK, BROADCAST, ETH_ARP, ETH_IPV4, FIN, PROTO_ICMP, PROTO_TCP, PROTO_UDP, PSH,
    RST, SYN,
};
use super::tcp::State;
use super::{dhcp, dns};

const MAC1: Mac = [0x02, 0x4c, 0x31, 0x36, 0x00, 0x01];
const MAC2: Mac = [0x02, 0x4c, 0x31, 0x36, 0x00, 0x02];
const IP1: Ipv4Addr = Ipv4Addr::new(10, 16, 0, 10);

struct FakeConn {
    to: SocketAddrV4,
    state: Connect,
    sent: Vec<u8>,
    incoming: VecDeque<u8>,
    eof: bool,
    shut: bool,
}

/// A host in a box: names from a table, datagrams and connections in
/// queues the test fills and reads.
#[derive(Default)]
struct Fake {
    names: BTreeMap<String, Ipv4Addr>,
    resolved: VecDeque<(u32, Option<Ipv4Addr>)>,
    udp_out: Vec<(SocketAddrV4, SocketAddrV4, Vec<u8>)>,
    udp_in: VecDeque<(SocketAddrV4, SocketAddrV4, Vec<u8>)>,
    udp_closed: Vec<SocketAddrV4>,
    conns: BTreeMap<u32, FakeConn>,
    closed: Vec<u32>,
}

impl Outside for Fake {
    fn resolve(&mut self, id: u32, name: &str) -> bool {
        let ip = self.names.get(&name.to_ascii_lowercase()).copied();
        self.resolved.push_back((id, ip));
        true
    }
    fn resolved(&mut self) -> Option<(u32, Option<Ipv4Addr>)> {
        self.resolved.pop_front()
    }
    fn udp_send(&mut self, machine: SocketAddrV4, to: SocketAddrV4, data: &[u8]) {
        self.udp_out.push((machine, to, data.to_vec()));
    }
    fn udp_recv(&mut self) -> Option<(SocketAddrV4, SocketAddrV4, Vec<u8>)> {
        self.udp_in.pop_front()
    }
    fn udp_close(&mut self, machine: SocketAddrV4) {
        self.udp_closed.push(machine);
    }
    fn tcp_connect(&mut self, id: u32, to: SocketAddrV4) {
        self.conns.insert(
            id,
            FakeConn {
                to,
                state: Connect::Pending,
                sent: Vec::new(),
                incoming: VecDeque::new(),
                eof: false,
                shut: false,
            },
        );
    }
    fn tcp_connect_state(&mut self, id: u32) -> Connect {
        self.conns.get(&id).map_or(Connect::Failed, |c| c.state)
    }
    fn tcp_send(&mut self, id: u32, data: &[u8]) -> Option<usize> {
        let c = self.conns.get_mut(&id)?;
        c.sent.extend_from_slice(data);
        Some(data.len())
    }
    fn tcp_recv(&mut self, id: u32, buf: &mut [u8]) -> TcpRead {
        let Some(c) = self.conns.get_mut(&id) else {
            return TcpRead::Failed;
        };
        if c.incoming.is_empty() {
            return if c.eof {
                TcpRead::Closed
            } else {
                TcpRead::Nothing
            };
        }
        let n = buf.len().min(c.incoming.len());
        for (b, x) in buf.iter_mut().zip(c.incoming.drain(..n)) {
            *b = x;
        }
        TcpRead::Data(n)
    }
    fn tcp_shutdown(&mut self, id: u32) {
        if let Some(c) = self.conns.get_mut(&id) {
            c.shut = true;
        }
    }
    fn tcp_close(&mut self, id: u32) {
        self.conns.remove(&id);
        self.closed.push(id);
    }
}

fn sa(ip: Ipv4Addr, port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(ip, port)
}

fn ip_frame(
    src_mac: Mac,
    dst_mac: Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    proto: u8,
    p: &[u8],
) -> Vec<u8> {
    packet::ethernet(
        dst_mac,
        src_mac,
        ETH_IPV4,
        &packet::ipv4(src, dst, proto, 1, p),
    )
}

fn udp_frame(
    src_mac: Mac,
    dst_mac: Mac,
    from: SocketAddrV4,
    to: SocketAddrV4,
    data: &[u8],
) -> Vec<u8> {
    ip_frame(
        src_mac,
        dst_mac,
        *from.ip(),
        *to.ip(),
        PROTO_UDP,
        &packet::udp(from, to, data),
    )
}

/// A frame the gateway sent: Ethernet destination, IP source and
/// destination, protocol and payload.
struct Out {
    dst_mac: Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    proto: u8,
    payload: Vec<u8>,
}

fn out_ip(f: &[u8]) -> Out {
    let e = packet::parse_ethernet(f).unwrap();
    assert_eq!(e.src, GATEWAY_MAC);
    assert_eq!(e.ty, ETH_IPV4);
    let ip = packet::parse_ipv4(e.payload).expect("a good IP header");
    Out {
        dst_mac: e.dst,
        src: ip.src,
        dst: ip.dst,
        proto: ip.proto,
        payload: ip.payload.to_vec(),
    }
}

/// A UDP answer: from, to, data.
fn out_udp(f: &[u8]) -> (Mac, SocketAddrV4, SocketAddrV4, Vec<u8>) {
    let e = packet::parse_ethernet(f).unwrap();
    let ip = packet::parse_ipv4(e.payload).unwrap();
    let u = packet::parse_udp(&ip).expect("a good UDP checksum");
    (
        e.dst,
        sa(ip.src, u.src_port),
        sa(ip.dst, u.dst_port),
        u.data.to_vec(),
    )
}

// ---------------------------------------------------------------- ARP, ping

fn arp_request(tpa: Ipv4Addr) -> Vec<u8> {
    let a = Arp {
        oper: 1,
        sha: MAC1,
        spa: IP1,
        tha: [0; 6],
        tpa,
    };
    packet::ethernet(BROADCAST, MAC1, ETH_ARP, &packet::arp(&a))
}

#[test]
fn arp_is_answered_for_the_gateway_and_beyond_the_net() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    for tpa in [GATEWAY_IP, Ipv4Addr::new(8, 8, 8, 8)] {
        let r = g.frame(&arp_request(tpa), 0, &mut o);
        assert_eq!(r.len(), 1);
        let e = packet::parse_ethernet(&r[0]).unwrap();
        assert_eq!((e.dst, e.src, e.ty), (MAC1, GATEWAY_MAC, ETH_ARP));
        let a = packet::parse_arp(e.payload).unwrap();
        assert_eq!(
            a,
            Arp {
                oper: 2,
                sha: GATEWAY_MAC,
                spa: tpa,
                tha: MAC1,
                tpa: IP1
            }
        );
    }
    // Another machine on the net answers for itself.
    assert!(g
        .frame(&arp_request(Ipv4Addr::new(10, 16, 0, 20)), 0, &mut o)
        .is_empty());
}

fn echo(dst: Ipv4Addr) -> Vec<u8> {
    let mut p = vec![8, 0, 0, 0, 0x12, 0x34, 0, 7];
    p.extend_from_slice(b"brass pipes");
    let c = packet::checksum(&p);
    p[2..4].copy_from_slice(&c.to_be_bytes());
    ip_frame(MAC1, GATEWAY_MAC, IP1, dst, PROTO_ICMP, &p)
}

#[test]
fn the_gateway_answers_pings() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let r = g.frame(&echo(GATEWAY_IP), 0, &mut o);
    assert_eq!(r.len(), 1);
    let x = out_ip(&r[0]);
    assert_eq!(
        (x.dst_mac, x.src, x.dst, x.proto),
        (MAC1, GATEWAY_IP, IP1, PROTO_ICMP)
    );
    assert_eq!(x.payload[0], 0, "echo reply");
    assert_eq!(&x.payload[4..8], &[0x12, 0x34, 0, 7]);
    assert_eq!(&x.payload[8..], b"brass pipes");
    assert_eq!(packet::checksum(&x.payload), 0);
    // No raw sockets: pings beyond the gateway stay unanswered.
    assert!(g
        .frame(&echo(Ipv4Addr::new(1, 1, 1, 1)), 0, &mut o)
        .is_empty());
}

// ---------------------------------------------------------------- DHCP

fn dhcp_msg(mac: Mac, ty: u8, opts: &[(u8, &[u8])]) -> Vec<u8> {
    let mut m = vec![0u8; 240];
    m[0] = 1;
    m[1] = 1;
    m[2] = 6;
    m[4..8].copy_from_slice(&0xdead_beefu32.to_be_bytes());
    m[28..34].copy_from_slice(&mac);
    m[236..240].copy_from_slice(&[99, 130, 83, 99]);
    m.extend_from_slice(&[53, 1, ty]);
    for (code, v) in opts {
        m.push(*code);
        m.push(v.len() as u8);
        m.extend_from_slice(v);
    }
    m.push(255);
    m
}

fn dhcp_frame(mac: Mac, msg: &[u8]) -> Vec<u8> {
    udp_frame(
        mac,
        BROADCAST,
        sa(Ipv4Addr::UNSPECIFIED, 68),
        sa(Ipv4Addr::BROADCAST, 67),
        msg,
    )
}

/// Sends a DHCP message; the reply's type, address and options.
fn dhcp_ask(
    g: &mut Gateway,
    o: &mut Fake,
    mac: Mac,
    ty: u8,
    opts: &[(u8, &[u8])],
) -> Option<(u8, Ipv4Addr, BTreeMap<u8, Vec<u8>>)> {
    let r = g.frame(&dhcp_frame(mac, &dhcp_msg(mac, ty, opts)), 0, o);
    let f = r.first()?;
    let (dst_mac, from, to, data) = out_udp(f);
    assert_eq!(dst_mac, BROADCAST);
    assert_eq!(from, sa(GATEWAY_IP, 67));
    assert_eq!(to, sa(Ipv4Addr::BROADCAST, 68));
    assert_eq!(data[0], 2, "BOOTREPLY");
    assert_eq!(&data[4..8], &0xdead_beefu32.to_be_bytes());
    assert_eq!(&data[28..34], &mac);
    let opts = dhcp::options(&data);
    let yi = Ipv4Addr::new(data[16], data[17], data[18], data[19]);
    Some((opts[&53][0], yi, opts))
}

/// DISCOVER and REQUEST; the address given.
fn lease(g: &mut Gateway, o: &mut Fake, mac: Mac, name: &str) -> Ipv4Addr {
    let (t, ip, _) = dhcp_ask(g, o, mac, dhcp::DISCOVER, &[(12, name.as_bytes())]).unwrap();
    assert_eq!(t, dhcp::OFFER);
    let (t, ip2, _) = dhcp_ask(
        g,
        o,
        mac,
        dhcp::REQUEST,
        &[
            (50, &ip.octets()),
            (54, &GATEWAY_IP.octets()),
            (12, name.as_bytes()),
        ],
    )
    .unwrap();
    assert_eq!((t, ip2), (dhcp::ACK, ip));
    ip
}

#[test]
fn dhcp_gives_addresses_in_order_and_keeps_them() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let (t, ip, opts) = dhcp_ask(&mut g, &mut o, MAC1, dhcp::DISCOVER, &[]).unwrap();
    assert_eq!((t, ip), (dhcp::OFFER, IP1));
    assert_eq!(opts[&1], [255, 255, 255, 0]);
    assert_eq!(opts[&3], GATEWAY_IP.octets());
    assert_eq!(opts[&6], GATEWAY_IP.octets());
    assert_eq!(opts[&15], b"logophor");
    assert_eq!(opts[&51], 3600u32.to_be_bytes());
    assert_eq!(opts[&54], GATEWAY_IP.octets());
    assert_eq!(lease(&mut g, &mut o, MAC1, "alpha"), IP1);
    assert_eq!(
        lease(&mut g, &mut o, MAC2, "beta"),
        Ipv4Addr::new(10, 16, 0, 11)
    );
    assert_eq!(lease(&mut g, &mut o, MAC1, "alpha"), IP1, "the same again");
    // A request for someone else's address: NAK.
    let (t, ip, opts) = dhcp_ask(
        &mut g,
        &mut o,
        MAC1,
        dhcp::REQUEST,
        &[(50, &[10, 16, 0, 11])],
    )
    .unwrap();
    assert_eq!((t, ip), (dhcp::NAK, Ipv4Addr::UNSPECIFIED));
    assert!(!opts.contains_key(&51));
    // A request that chose another server: no answer.
    assert!(dhcp_ask(
        &mut g,
        &mut o,
        MAC1,
        dhcp::REQUEST,
        &[(50, &IP1.octets()), (54, &[10, 16, 0, 2])],
    )
    .is_none());
    assert_eq!(g.leases()[&MAC2].name.as_deref(), Some("beta"));
    assert!(g.take_log().iter().any(|l| l.contains("10.16.0.11 beta")));
}

// ---------------------------------------------------------------- DNS

fn dns_query(id: u16, name: &str) -> Vec<u8> {
    let mut m = id.to_be_bytes().to_vec();
    m.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for l in name.split('.') {
        m.push(l.len() as u8);
        m.extend_from_slice(l.as_bytes());
    }
    m.extend_from_slice(&[0, 0, 1, 0, 1]);
    m
}

fn dns_frame(msg: &[u8]) -> Vec<u8> {
    udp_frame(MAC1, GATEWAY_MAC, sa(IP1, 5353), sa(GATEWAY_IP, 53), msg)
}

/// Rcode, the answer's address (if any) and the id of a DNS answer.
fn dns_reply(f: &[u8]) -> (u8, Option<Ipv4Addr>, u16) {
    let (mac, from, to, m) = out_udp(f);
    assert_eq!((mac, from, to), (MAC1, sa(GATEWAY_IP, 53), sa(IP1, 5353)));
    assert_eq!(m[2] & 0x80, 0x80, "a response");
    assert_eq!(m[3] & 0x80, 0x80, "recursion available");
    let an = u16::from_be_bytes([m[6], m[7]]);
    let n = m.len();
    let ip = (an == 1).then(|| Ipv4Addr::new(m[n - 4], m[n - 3], m[n - 2], m[n - 1]));
    (m[3] & 15, ip, u16::from_be_bytes([m[0], m[1]]))
}

#[test]
fn dns_knows_machines_the_hub_and_asks_the_host_for_the_rest() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    o.names
        .insert("example.com".into(), Ipv4Addr::new(93, 184, 216, 34));
    lease(&mut g, &mut o, MAC1, "alpha");
    for (name, ip) in [
        ("alpha", IP1),
        ("ALPHA.Logophor", IP1),
        ("hub", GATEWAY_IP),
        ("gateway.logophor", GATEWAY_IP),
    ] {
        let r = g.frame(&dns_frame(&dns_query(7, name)), 0, &mut o);
        assert_eq!(r.len(), 1, "{name}");
        assert_eq!(dns_reply(&r[0]), (dns::NOERROR, Some(ip), 7), "{name}");
        // The question is copied.
        let (_, _, _, m) = out_udp(&r[0]);
        let q = dns_query(7, name);
        assert_eq!(&m[12..q.len()], &q[12..]);
    }
    // A .logophor name nobody has.
    let r = g.frame(&dns_frame(&dns_query(8, "beta.logophor")), 0, &mut o);
    assert_eq!(dns_reply(&r[0]), (dns::NXDOMAIN, None, 8));
    // Outside names go to the host's resolver and come back in poll.
    assert!(g
        .frame(&dns_frame(&dns_query(9, "example.com")), 0, &mut o)
        .is_empty());
    assert!(g
        .frame(&dns_frame(&dns_query(10, "nothing.invalid")), 0, &mut o)
        .is_empty());
    let r = g.poll(5, &mut o);
    assert_eq!(r.len(), 2);
    assert_eq!(
        dns_reply(&r[0]),
        (dns::NOERROR, Some(Ipv4Addr::new(93, 184, 216, 34)), 9)
    );
    assert_eq!(dns_reply(&r[1]), (dns::NXDOMAIN, None, 10));
}

#[test]
fn malformed_dns_queries_get_formerr_or_nothing() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    // A name pointing to itself.
    let mut m = vec![0, 42, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    m.extend_from_slice(&[1, b'a', 0xc0, 12, 0, 1, 0, 1]);
    let r = g.frame(&dns_frame(&m), 0, &mut o);
    assert_eq!(dns_reply(&r[0]), (dns::FORMERR, None, 42));
    // Too short for a header; a response.
    assert!(g.frame(&dns_frame(&[1, 2, 3, 4, 5]), 0, &mut o).is_empty());
    let mut resp = dns_query(1, "hub");
    resp[2] |= 0x80;
    assert!(g.frame(&dns_frame(&resp), 0, &mut o).is_empty());
    // Junk never panics.
    let mut x = 99u32;
    for len in 0..300 {
        let mut m = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        m.extend((0..len % 60).map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        }));
        let _ = g.frame(&dns_frame(&m), 0, &mut o);
    }
    let _ = g.poll(1, &mut o);
}

// ---------------------------------------------------------------- UDP NAT

#[test]
fn udp_goes_out_and_answers_come_back() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let me = sa(IP1, 4000);
    let there = sa(Ipv4Addr::new(1, 2, 3, 4), 7);
    assert!(g
        .frame(&udp_frame(MAC1, GATEWAY_MAC, me, there, b"ping"), 0, &mut o)
        .is_empty());
    assert_eq!(o.udp_out, vec![(me, there, b"ping".to_vec())]);
    o.udp_in.push_back((me, there, b"pong".to_vec()));
    let r = g.poll(10, &mut o);
    assert_eq!(r.len(), 1);
    assert_eq!(out_udp(&r[0]), (MAC1, there, me, b"pong".to_vec()));
    // Idle bindings are closed.
    assert!(g.poll(200_000, &mut o).is_empty());
    assert_eq!(o.udp_closed, vec![me]);
}

// ---------------------------------------------------------------- TCP

const ME: SocketAddrV4 = SocketAddrV4::new(IP1, 1024);
const THERE: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 80);
const MSEQ: u32 = 1000;

#[allow(clippy::too_many_arguments)]
fn seg(seq: u32, ack: u32, flags: u8, window: u16, data: &[u8]) -> Vec<u8> {
    let mss = (flags & SYN != 0).then_some(1460);
    let t = packet::tcp(ME, THERE, seq, ack, flags, window, mss, data);
    ip_frame(MAC1, GATEWAY_MAC, *ME.ip(), *THERE.ip(), PROTO_TCP, &t)
}

#[derive(Debug, PartialEq, Eq)]
struct Got {
    seq: u32,
    ack: u32,
    flags: u8,
    data: Vec<u8>,
}

fn got(f: &[u8]) -> Got {
    got_from(f, THERE)
}

/// A segment the gateway sent from `there` to the machine.
fn got_from(f: &[u8], there: SocketAddrV4) -> Got {
    let x = out_ip(f);
    assert_eq!(
        (x.dst_mac, x.src, x.dst, x.proto),
        (MAC1, *there.ip(), IP1, PROTO_TCP)
    );
    let ip = packet::Ipv4 {
        src: x.src,
        dst: x.dst,
        proto: x.proto,
        payload: &x.payload,
    };
    let t = packet::parse_tcp(&ip).expect("a good TCP checksum");
    assert_eq!((t.src_port, t.dst_port), (there.port(), ME.port()));
    Got {
        seq: t.seq,
        ack: t.ack,
        flags: t.flags,
        data: t.data.to_vec(),
    }
}

/// Opens a connection with the machine's window `window`; returns the
/// hub's first data sequence number.
fn open(g: &mut Gateway, o: &mut Fake, window: u16) -> u32 {
    assert!(g.frame(&seg(MSEQ, 0, SYN, window, &[]), 0, o).is_empty());
    assert_eq!(o.conns[&0].to, THERE);
    assert!(g.poll(1, o).is_empty(), "still connecting");
    o.conns.get_mut(&0).unwrap().state = Connect::Open;
    let r = g.poll(2, o);
    assert_eq!(r.len(), 1);
    let sa = got(&r[0]);
    assert_eq!((sa.ack, sa.flags), (MSEQ + 1, SYN | ACK));
    // The MSS option is there: a 24-byte header.
    assert_eq!(out_ip(&r[0]).payload[12] >> 4, 6);
    assert_eq!(g.tcp_state(ME, THERE), Some(State::SynReceived));
    assert!(g
        .frame(&seg(MSEQ + 1, sa.seq + 1, ACK, window, &[]), 3, o)
        .is_empty());
    assert_eq!(g.tcp_state(ME, THERE), Some(State::Established));
    sa.seq + 1
}

#[test]
fn tcp_carries_data_both_ways() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let h = open(&mut g, &mut o, 8192);
    let r = g.frame(&seg(MSEQ + 1, h, ACK | PSH, 8192, b"hello"), 10, &mut o);
    assert_eq!(o.conns[&0].sent, b"hello");
    assert_eq!(r.len(), 1);
    assert_eq!(
        got(&r[0]),
        Got {
            seq: h,
            ack: MSEQ + 6,
            flags: ACK,
            data: vec![]
        }
    );
    o.conns.get_mut(&0).unwrap().incoming.extend(b"world");
    let r = g.poll(20, &mut o);
    assert_eq!(
        got(&r[0]),
        Got {
            seq: h,
            ack: MSEQ + 6,
            flags: ACK | PSH,
            data: b"world".to_vec()
        }
    );
    // A repeated segment is not passed on twice.
    let r = g.frame(&seg(MSEQ + 1, h + 5, ACK | PSH, 8192, b"hello"), 30, &mut o);
    assert_eq!(o.conns[&0].sent, b"hello");
    assert_eq!(got(&r[0]).ack, MSEQ + 6);
    // Acknowledged: nothing comes again.
    assert!(g.poll(5000, &mut o).is_empty());
}

#[test]
fn tcp_sends_within_the_machines_window_and_mss() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let h = open(&mut g, &mut o, 1000);
    o.conns
        .get_mut(&0)
        .unwrap()
        .incoming
        .extend(vec![7u8; 3000]);
    let r = g.poll(10, &mut o);
    let lens: Vec<usize> = r.iter().map(|f| got(f).data.len()).collect();
    assert_eq!(lens, [1000]);
    // The machine takes it and opens its window wide: MSS-sized segments.
    let r = g.frame(&seg(MSEQ + 1, h + 1000, ACK, 8000, &[]), 20, &mut o);
    let segs: Vec<Got> = r.iter().map(|f| got(f)).collect();
    assert_eq!(
        segs.iter().map(|s| s.data.len()).collect::<Vec<_>>(),
        [1460, 540]
    );
    assert_eq!((segs[0].seq, segs[1].seq), (h + 1000, h + 2460));
}

#[test]
fn tcp_retransmits_after_a_second() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let h = open(&mut g, &mut o, 8192);
    o.conns.get_mut(&0).unwrap().incoming.extend(b"abc");
    let first = got(&g.poll(100, &mut o)[0]);
    assert!(g.poll(1099, &mut o).is_empty());
    let r = g.poll(1100, &mut o);
    assert_eq!(got(&r[0]), first);
    assert_eq!(first.seq, h);
    // Acknowledged now: quiet.
    g.frame(&seg(MSEQ + 1, h + 3, ACK, 8192, &[]), 1200, &mut o);
    assert!(g.poll(3000, &mut o).is_empty());
}

#[test]
fn tcp_closes_when_the_machine_closes_first() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let h = open(&mut g, &mut o, 8192);
    let r = g.frame(&seg(MSEQ + 1, h, FIN | ACK, 8192, &[]), 10, &mut o);
    assert_eq!(got(&r[0]).ack, MSEQ + 2);
    assert_eq!(g.tcp_state(ME, THERE), Some(State::CloseWait));
    assert!(o.conns[&0].shut, "shut down outside");
    o.conns.get_mut(&0).unwrap().incoming.extend(b"bye");
    o.conns.get_mut(&0).unwrap().eof = true;
    let r = g.poll(20, &mut o);
    let s: Vec<Got> = r.iter().map(|f| got(f)).collect();
    assert_eq!(s[0].data, b"bye");
    assert_eq!((s[1].seq, s[1].flags), (h + 3, FIN | ACK));
    assert_eq!(g.tcp_state(ME, THERE), Some(State::LastAck));
    g.frame(&seg(MSEQ + 2, h + 4, ACK, 8192, &[]), 30, &mut o);
    assert_eq!(g.tcp_state(ME, THERE), None);
    assert_eq!(o.closed, [0]);
}

#[test]
fn tcp_closes_when_the_outside_closes_first() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    let h = open(&mut g, &mut o, 8192);
    o.conns.get_mut(&0).unwrap().eof = true;
    let r = g.poll(10, &mut o);
    assert_eq!(
        got(&r[0]),
        Got {
            seq: h,
            ack: MSEQ + 1,
            flags: FIN | ACK,
            data: vec![]
        }
    );
    assert_eq!(g.tcp_state(ME, THERE), Some(State::FinWait1));
    g.frame(&seg(MSEQ + 1, h + 1, ACK, 8192, &[]), 20, &mut o);
    assert_eq!(g.tcp_state(ME, THERE), Some(State::FinWait2));
    // The machine still sends, then closes.
    let r = g.frame(&seg(MSEQ + 1, h + 1, ACK | FIN, 8192, b"x"), 30, &mut o);
    assert_eq!(got(&r[0]).ack, MSEQ + 3);
    assert_eq!(o.conns[&0].sent, b"x");
    assert_eq!(g.tcp_state(ME, THERE), Some(State::TimeWait));
    // A repeated FIN is acknowledged again.
    let r = g.frame(&seg(MSEQ + 2, h + 1, ACK | FIN, 8192, &[]), 40, &mut o);
    assert_eq!(got(&r[0]).ack, MSEQ + 3);
    g.poll(3000, &mut o);
    assert_eq!(g.tcp_state(ME, THERE), None);
    assert_eq!(g.tcp_connections(), 0);
}

#[test]
fn tcp_resets_when_the_outside_refuses() {
    let mut g = Gateway::new();
    let mut o = Fake::default();
    g.frame(&seg(MSEQ, 0, SYN, 8192, &[]), 0, &mut o);
    o.conns.get_mut(&0).unwrap().state = Connect::Failed;
    let r = g.poll(5, &mut o);
    let s = got(&r[0]);
    assert_eq!((s.seq, s.ack, s.flags), (0, MSEQ + 1, RST | ACK));
    assert_eq!(g.tcp_connections(), 0);
    // A connect that takes too long ends the same way.
    g.frame(&seg(MSEQ, 0, SYN, 8192, &[]), 100, &mut o);
    assert!(g.poll(5099, &mut o).is_empty());
    let r = g.poll(5100, &mut o);
    assert_eq!(got(&r[0]).flags, RST | ACK);
    assert_eq!(o.closed, [0, 1]);
    // A segment for no connection.
    let r = g.frame(&seg(MSEQ, 777, ACK, 8192, b"?"), 6000, &mut o);
    let s = got(&r[0]);
    assert_eq!((s.seq, s.flags), (777, RST));
}

#[test]
fn tcp_through_real_sockets_reaches_a_local_echo_server() {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0u8; 256];
        loop {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            s.write_all(&buf[..n]).unwrap();
        }
    });
    let there = sa(Ipv4Addr::LOCALHOST, port);
    let frame = |seq: u32, ack: u32, flags: u8, data: &[u8]| {
        let mss = (flags & SYN != 0).then_some(1460);
        let t = packet::tcp(ME, there, seq, ack, flags, 8192, mss, data);
        ip_frame(MAC1, GATEWAY_MAC, *ME.ip(), *there.ip(), PROTO_TCP, &t)
    };
    let mut g = Gateway::new();
    let mut o = HostOutside::new();
    let t0 = Instant::now();
    let wait = |g: &mut Gateway, o: &mut HostOutside| -> Got {
        loop {
            assert!(t0.elapsed() < Duration::from_secs(5), "no answer in time");
            let r = g.poll(t0.elapsed().as_millis() as u64, o);
            if let Some(f) = r.first() {
                return got_from(f, there);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    g.frame(&frame(MSEQ, 0, SYN, &[]), 0, &mut o);
    let sa_ = wait(&mut g, &mut o);
    assert_eq!(sa_.flags, SYN | ACK);
    let h = sa_.seq + 1;
    let now = t0.elapsed().as_millis() as u64;
    g.frame(&frame(MSEQ + 1, h, ACK, &[]), now, &mut o);
    let now = t0.elapsed().as_millis() as u64;
    let r = g.frame(&frame(MSEQ + 1, h, ACK | PSH, b"steam"), now, &mut o);
    assert_eq!(r.len(), 1, "the data is acknowledged");
    let mut echoed = Vec::new();
    while echoed.len() < 5 {
        let d = wait(&mut g, &mut o);
        echoed.extend_from_slice(&d.data);
        let now = t0.elapsed().as_millis() as u64;
        g.frame(
            &frame(MSEQ + 6, d.seq + d.data.len() as u32, ACK, &[]),
            now,
            &mut o,
        );
    }
    assert_eq!(echoed, b"steam");
    // The machine closes; the server sees the end and closes too.
    let now = t0.elapsed().as_millis() as u64;
    g.frame(&frame(MSEQ + 6, h + 5, FIN | ACK, &[]), now, &mut o);
    let fin = wait(&mut g, &mut o);
    assert_ne!(fin.flags & FIN, 0);
    let now = t0.elapsed().as_millis() as u64;
    g.frame(&frame(MSEQ + 7, h + 6, ACK, &[]), now, &mut o);
    assert_eq!(g.tcp_connections(), 0);
    server.join().unwrap();
}
