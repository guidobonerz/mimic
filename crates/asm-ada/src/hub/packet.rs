//! Wire formats on the Logophor 16 net, as the gateway reads and writes
//! them: Ethernet II frames, ARP (RFC 826), IPv4 (RFC 791), ICMP
//! (RFC 792), UDP (RFC 768) and TCP (RFC 793) headers, the Internet
//! checksum (RFC 791/768/793) and the frame check sequence (IEEE 802.3).
//!
//! Parsers return `None` for anything malformed: frames come from guest
//! programs and must never make the hub panic.

use std::net::{Ipv4Addr, SocketAddrV4};

/// A MAC address.
pub type Mac = [u8; 6];
/// The broadcast MAC.
pub const BROADCAST: Mac = [0xff; 6];
/// EtherType IPv4.
pub const ETH_IPV4: u16 = 0x0800;
/// EtherType ARP.
pub const ETH_ARP: u16 = 0x0806;
/// IP protocol numbers.
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

/// TCP header flags (RFC 793 3.1).
pub const FIN: u8 = 0x01;
pub const SYN: u8 = 0x02;
pub const RST: u8 = 0x04;
pub const PSH: u8 = 0x08;
pub const ACK: u8 = 0x10;

/// The frame check sequence of a frame (IEEE 802.3 CRC-32: reflected
/// polynomial $EDB88320, initial and final value inverted), in the order
/// it goes on the wire (low byte first).
pub fn fcs(frame: &[u8]) -> [u8; 4] {
    let mut c = !0u32;
    for &b in frame {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    (!c).to_le_bytes()
}

fn be16(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([d[at], d[at + 1]])
}

fn be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn ip_at(d: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(d[at], d[at + 1], d[at + 2], d[at + 3])
}

/// An Ethernet II frame (without FCS).
#[derive(Clone, Copy, Debug)]
pub struct Ether<'a> {
    pub dst: Mac,
    pub src: Mac,
    pub ty: u16,
    pub payload: &'a [u8],
}

pub fn parse_ethernet(f: &[u8]) -> Option<Ether<'_>> {
    if f.len() < 14 {
        return None;
    }
    Some(Ether {
        dst: f[0..6].try_into().ok()?,
        src: f[6..12].try_into().ok()?,
        ty: be16(f, 12),
        payload: &f[14..],
    })
}

pub fn ethernet(dst: Mac, src: Mac, ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(14 + payload.len());
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ty.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

/// An ARP packet for IPv4 over Ethernet (RFC 826).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arp {
    /// 1 request, 2 reply.
    pub oper: u16,
    pub sha: Mac,
    pub spa: Ipv4Addr,
    pub tha: Mac,
    pub tpa: Ipv4Addr,
}

pub fn parse_arp(p: &[u8]) -> Option<Arp> {
    // Hardware type 1 (Ethernet), protocol $0800, lengths 6 and 4.
    if p.len() < 28 || be16(p, 0) != 1 || be16(p, 2) != ETH_IPV4 || p[4] != 6 || p[5] != 4 {
        return None;
    }
    Some(Arp {
        oper: be16(p, 6),
        sha: p[8..14].try_into().ok()?,
        spa: ip_at(p, 14),
        tha: p[18..24].try_into().ok()?,
        tpa: ip_at(p, 24),
    })
}

pub fn arp(a: &Arp) -> Vec<u8> {
    let mut p = Vec::with_capacity(28);
    p.extend_from_slice(&[0, 1, 0x08, 0x00, 6, 4]);
    p.extend_from_slice(&a.oper.to_be_bytes());
    p.extend_from_slice(&a.sha);
    p.extend_from_slice(&a.spa.octets());
    p.extend_from_slice(&a.tha);
    p.extend_from_slice(&a.tpa.octets());
    p
}

/// Adds 16-bit big-endian words of `data` to a ones' complement sum.
pub fn sum16(mut sum: u32, data: &[u8]) -> u32 {
    for w in data.chunks(2) {
        let hi = u32::from(w[0]) << 8;
        sum += hi | w.get(1).map_or(0, |&lo| u32::from(lo));
    }
    sum
}

/// Folds a sum and complements it: the Internet checksum.
pub fn fold(mut sum: u32) -> u16 {
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// The Internet checksum of `data` (RFC 791 3.1).
pub fn checksum(data: &[u8]) -> u16 {
    fold(sum16(0, data))
}

/// An IPv4 datagram (no options kept, no fragments).
#[derive(Clone, Copy, Debug)]
pub struct Ipv4<'a> {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub proto: u8,
    pub payload: &'a [u8],
}

/// Parses an IPv4 header (RFC 791 3.1): version 4, a valid header
/// checksum, total length within the frame; fragments are refused.
pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4<'_>> {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(p[0] & 15) * 4;
    let total = usize::from(be16(p, 2));
    if ihl < 20 || total < ihl || total > p.len() || checksum(&p[..ihl]) != 0 {
        return None;
    }
    // MF set or a fragment offset: the net has no fragments.
    if be16(p, 6) & 0x3fff != 0 {
        return None;
    }
    Some(Ipv4 {
        src: ip_at(p, 12),
        dst: ip_at(p, 16),
        proto: p[9],
        payload: &p[ihl..total],
    })
}

/// An IPv4 datagram: TTL 64, don't fragment.
pub fn ipv4(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let total = (20 + payload.len()) as u16;
    let mut p = Vec::with_capacity(20 + payload.len());
    p.extend_from_slice(&[0x45, 0]);
    p.extend_from_slice(&total.to_be_bytes());
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&[0x40, 0, 64, proto, 0, 0]);
    p.extend_from_slice(&src.octets());
    p.extend_from_slice(&dst.octets());
    let c = checksum(&p);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

/// The pseudo header's sum for UDP and TCP checksums (RFC 768, 793).
fn pseudo(src: Ipv4Addr, dst: Ipv4Addr, proto: u8, len: usize) -> u32 {
    let mut s = sum16(0, &src.octets());
    s = sum16(s, &dst.octets());
    s + u32::from(proto) + len as u32
}

/// A UDP datagram.
#[derive(Clone, Copy, Debug)]
pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub data: &'a [u8],
}

/// Parses UDP (RFC 768); a checksum of 0 means none.
pub fn parse_udp<'a>(ip: &Ipv4<'a>) -> Option<Udp<'a>> {
    let p = ip.payload;
    if p.len() < 8 {
        return None;
    }
    let len = usize::from(be16(p, 4));
    if len < 8 || len > p.len() {
        return None;
    }
    let p = &p[..len];
    if be16(p, 6) != 0 && fold(sum16(pseudo(ip.src, ip.dst, PROTO_UDP, len), p)) != 0 {
        return None;
    }
    Some(Udp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        data: &p[8..],
    })
}

/// A UDP header and data (the IP payload).
pub fn udp(src: SocketAddrV4, dst: SocketAddrV4, data: &[u8]) -> Vec<u8> {
    let len = 8 + data.len();
    let mut p = Vec::with_capacity(len);
    p.extend_from_slice(&src.port().to_be_bytes());
    p.extend_from_slice(&dst.port().to_be_bytes());
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&[0, 0]);
    p.extend_from_slice(data);
    let mut c = fold(sum16(pseudo(*src.ip(), *dst.ip(), PROTO_UDP, len), &p));
    if c == 0 {
        c = 0xffff;
    }
    p[6..8].copy_from_slice(&c.to_be_bytes());
    p
}

/// A TCP segment (RFC 793 3.1).
#[derive(Clone, Copy, Debug)]
pub struct Tcp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// The maximum segment size option (kind 2), if present.
    pub mss: Option<u16>,
    pub data: &'a [u8],
}

impl Tcp<'_> {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    /// The sequence space the segment takes: data plus SYN and FIN.
    pub fn seq_len(&self) -> u32 {
        self.data.len() as u32 + u32::from(self.has(SYN)) + u32::from(self.has(FIN))
    }
}

/// Parses a TCP segment and checks its checksum.
pub fn parse_tcp<'a>(ip: &Ipv4<'a>) -> Option<Tcp<'a>> {
    let p = ip.payload;
    if p.len() < 20 {
        return None;
    }
    let off = usize::from(p[12] >> 4) * 4;
    if off < 20 || off > p.len() {
        return None;
    }
    if fold(sum16(pseudo(ip.src, ip.dst, PROTO_TCP, p.len()), p)) != 0 {
        return None;
    }
    let mut mss = None;
    let mut o = &p[20..off];
    while let Some(&kind) = o.first() {
        match kind {
            0 => break,
            1 => o = &o[1..],
            _ => {
                let len = usize::from(*o.get(1)?);
                if len < 2 || len > o.len() {
                    return None;
                }
                if kind == 2 && len == 4 {
                    mss = Some(be16(o, 2));
                }
                o = &o[len..];
            }
        }
    }
    Some(Tcp {
        src_port: be16(p, 0),
        dst_port: be16(p, 2),
        seq: be32(p, 4),
        ack: be32(p, 8),
        flags: p[13] & 0x3f,
        window: be16(p, 14),
        mss,
        data: &p[off..],
    })
}

/// A TCP header (with an MSS option when given) and data: the IP payload.
#[allow(clippy::too_many_arguments)]
pub fn tcp(
    src: SocketAddrV4,
    dst: SocketAddrV4,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: Option<u16>,
    data: &[u8],
) -> Vec<u8> {
    let off = if mss.is_some() { 24 } else { 20 };
    let mut p = Vec::with_capacity(off + data.len());
    p.extend_from_slice(&src.port().to_be_bytes());
    p.extend_from_slice(&dst.port().to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&ack.to_be_bytes());
    p.push(((off / 4) as u8) << 4);
    p.push(flags);
    p.extend_from_slice(&window.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    if let Some(m) = mss {
        p.extend_from_slice(&[2, 4]);
        p.extend_from_slice(&m.to_be_bytes());
    }
    p.extend_from_slice(data);
    let c = fold(sum16(pseudo(*src.ip(), *dst.ip(), PROTO_TCP, p.len()), &p));
    p[16..18].copy_from_slice(&c.to_be_bytes());
    p
}

/// Sequence number comparison modulo 2^32 (RFC 793 3.3): a < b.
pub fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// a <= b modulo 2^32.
pub fn seq_le(a: u32, b: u32) -> bool {
    a == b || seq_lt(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fcs_is_ieee_802_3() {
        assert_eq!(u32::from_le_bytes(fcs(b"123456789")), 0xcbf4_3926);
    }

    #[test]
    fn the_ip_header_checksum_matches_a_known_header() {
        // A widely published example header; its checksum is $B861.
        let h = ipv4(
            Ipv4Addr::new(192, 168, 0, 1),
            Ipv4Addr::new(192, 168, 0, 199),
            PROTO_UDP,
            0,
            &[0; 0x73 - 20],
        );
        assert_eq!(&h[10..12], &[0xb8, 0x61]);
        assert!(parse_ipv4(&h).is_some());
    }

    #[test]
    fn udp_and_tcp_round_trip_with_checksums() {
        let a = SocketAddrV4::new(Ipv4Addr::new(10, 16, 0, 10), 1234);
        let b = SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 80);
        let d = ipv4(*a.ip(), *b.ip(), PROTO_UDP, 7, &udp(a, b, b"odd"));
        let ip = parse_ipv4(&d).unwrap();
        assert_eq!(parse_udp(&ip).unwrap().data, b"odd");
        let d = ipv4(
            *a.ip(),
            *b.ip(),
            PROTO_TCP,
            8,
            &tcp(a, b, 5, 6, SYN, 100, Some(1460), b"x"),
        );
        let ip = parse_ipv4(&d).unwrap();
        let t = parse_tcp(&ip).unwrap();
        assert_eq!((t.seq, t.ack, t.flags, t.window), (5, 6, SYN, 100));
        assert_eq!((t.mss, t.data), (Some(1460), &b"x"[..]));
        let mut bad = d.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(parse_tcp(&parse_ipv4(&bad).unwrap()).is_none());
    }

    #[test]
    fn sequence_numbers_wrap() {
        assert!(seq_lt(0xffff_fff0, 5));
        assert!(!seq_lt(5, 0xffff_fff0));
        assert!(seq_le(7, 7));
    }
}
