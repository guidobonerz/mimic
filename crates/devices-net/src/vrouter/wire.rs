//! Frame formats of the virtual router: Ethernet II, ARP (RFC 826), IPv4
//! (RFC 791), ICMP (RFC 792), UDP (RFC 768) and TCP (RFC 793) headers with
//! their checksums. Parsers take untrusted bytes and answer `None` for
//! anything malformed; builders never produce a frame above
//! [`MAX_FRAME`] bytes (they answer `None` instead).

/// Ethernet II header: destination, source, EtherType.
pub const ETH_HDR: usize = 14;
/// Largest frame without FCS: 14 bytes header + 1500 bytes payload.
pub const MAX_FRAME: usize = 1514;
/// Smallest frame without FCS; shorter ones are padded with zeros.
pub const MIN_FRAME: usize = 60;
/// Largest IPv4 packet in a frame.
pub const MTU: usize = 1500;
pub const BROADCAST_MAC: [u8; 6] = [0xff; 6];

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

pub const FIN: u8 = 0x01;
pub const SYN: u8 = 0x02;
pub const RST: u8 = 0x04;
pub const PSH: u8 = 0x08;
pub const ACK: u8 = 0x10;

pub fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o.checked_add(1)?)?]))
}

pub fn be32(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn ip4(b: &[u8], o: usize) -> Option<[u8; 4]> {
    let s = b.get(o..o.checked_add(4)?)?;
    Some([s[0], s[1], s[2], s[3]])
}

pub fn mac(b: &[u8], o: usize) -> Option<[u8; 6]> {
    let s = b.get(o..o.checked_add(6)?)?;
    Some([s[0], s[1], s[2], s[3], s[4], s[5]])
}

/// Add `data` as big-endian 16-bit words to a ones' complement sum (an odd
/// last byte counts as the high byte of a word).
pub fn sum(data: &[u8], acc: u32) -> u32 {
    let mut acc = u64::from(acc);
    let (words, rest) = data.as_chunks::<2>();
    for w in words {
        acc += u64::from(u16::from_be_bytes(*w));
    }
    if let [last] = rest {
        acc += u64::from(*last) << 8;
    }
    while acc >> 32 != 0 {
        acc = (acc & 0xffff_ffff) + (acc >> 32);
    }
    acc as u32
}

/// The Internet checksum of a finished sum.
pub fn fold(mut acc: u32) -> u16 {
    while acc >> 16 != 0 {
        acc = (acc & 0xffff) + (acc >> 16);
    }
    !(acc as u16)
}

/// The sum of the TCP/UDP pseudo header.
pub fn pseudo(src: [u8; 4], dst: [u8; 4], proto: u8, len: usize) -> u32 {
    let acc = sum(&src, 0);
    let acc = sum(&dst, acc);
    sum(&[0, proto, (len >> 8) as u8, len as u8], acc)
}

/// An Ethernet frame taken apart.
pub struct Eth<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

pub fn parse_eth(frame: &[u8]) -> Option<Eth<'_>> {
    Some(Eth {
        dst: mac(frame, 0)?,
        src: mac(frame, 6)?,
        ethertype: be16(frame, 12)?,
        payload: frame.get(ETH_HDR..)?,
    })
}

/// An ARP packet for Ethernet and IPv4.
pub struct Arp {
    /// 1 = request, 2 = reply.
    pub op: u16,
    pub sha: [u8; 6],
    pub spa: [u8; 4],
    pub tha: [u8; 6],
    pub tpa: [u8; 4],
}

pub fn parse_arp(p: &[u8]) -> Option<Arp> {
    // Hardware type 1 (Ethernet), protocol $0800, lengths 6 and 4.
    if be16(p, 0)? != 1 || be16(p, 2)? != ETHERTYPE_IPV4 || *p.get(4)? != 6 || *p.get(5)? != 4 {
        return None;
    }
    Some(Arp {
        op: be16(p, 6)?,
        sha: mac(p, 8)?,
        spa: ip4(p, 14)?,
        tha: mac(p, 18)?,
        tpa: ip4(p, 24)?,
    })
}

pub fn arp_frame(dst: [u8; 6], src: [u8; 6], a: &Arp) -> Vec<u8> {
    let mut f = eth_header(dst, src, ETHERTYPE_ARP);
    f.extend_from_slice(&[0, 1, 0x08, 0x00, 6, 4]);
    f.extend_from_slice(&a.op.to_be_bytes());
    f.extend_from_slice(&a.sha);
    f.extend_from_slice(&a.spa);
    f.extend_from_slice(&a.tha);
    f.extend_from_slice(&a.tpa);
    f
}

fn eth_header(dst: [u8; 6], src: [u8; 6], ethertype: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(MIN_FRAME);
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f
}

/// An IPv4 packet taken apart (header options skipped).
pub struct Ipv4<'a> {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub proto: u8,
    pub payload: &'a [u8],
}

/// Parse an IPv4 packet: version, header length, total length (bytes
/// behind it are padding) and header checksum are checked; fragments are
/// refused.
pub fn parse_ipv4(p: &[u8]) -> Option<Ipv4<'_>> {
    let vihl = *p.first()?;
    let ihl = usize::from(vihl & 0x0f) * 4;
    if vihl >> 4 != 4 || ihl < 20 {
        return None;
    }
    let header = p.get(..ihl)?;
    let total = usize::from(be16(p, 2)?);
    if total < ihl || fold(sum(header, 0)) != 0 {
        return None;
    }
    // More-fragments flag or a fragment offset: a fragment.
    if be16(p, 6)? & 0x3fff != 0 {
        return None;
    }
    Some(Ipv4 {
        src: ip4(p, 12)?,
        dst: ip4(p, 16)?,
        proto: *p.get(9)?,
        payload: p.get(ihl..total)?,
    })
}

/// Addresses of a frame to build.
#[derive(Clone, Copy)]
pub struct Addr {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub src: [u8; 4],
    pub dst: [u8; 4],
    /// The IPv4 identification field.
    pub ident: u16,
}

/// An Ethernet frame with an IPv4 packet (TTL 64, don't fragment).
pub fn ipv4_frame(a: &Addr, proto: u8, payload: &[u8]) -> Option<Vec<u8>> {
    let total = 20 + payload.len();
    if total > MTU {
        return None;
    }
    let mut f = eth_header(a.dst_mac, a.src_mac, ETHERTYPE_IPV4);
    let h = f.len();
    f.extend_from_slice(&[0x45, 0, (total >> 8) as u8, total as u8]);
    f.extend_from_slice(&a.ident.to_be_bytes());
    f.extend_from_slice(&[0x40, 0, 64, proto, 0, 0]);
    f.extend_from_slice(&a.src);
    f.extend_from_slice(&a.dst);
    let c = fold(sum(&f[h..], 0));
    f[h + 10..h + 12].copy_from_slice(&c.to_be_bytes());
    f.extend_from_slice(payload);
    Some(f)
}

pub fn udp_frame(a: &Addr, sport: u16, dport: u16, data: &[u8]) -> Option<Vec<u8>> {
    let len = 8 + data.len();
    if len + 20 > MTU {
        return None;
    }
    let mut u = Vec::with_capacity(len);
    u.extend_from_slice(&sport.to_be_bytes());
    u.extend_from_slice(&dport.to_be_bytes());
    u.extend_from_slice(&(len as u16).to_be_bytes());
    u.extend_from_slice(&[0, 0]);
    u.extend_from_slice(data);
    let mut c = fold(sum(&u, pseudo(a.src, a.dst, PROTO_UDP, len)));
    if c == 0 {
        // 0 means "no checksum" (RFC 768); a computed 0 is sent as $FFFF.
        c = 0xffff;
    }
    u[6..8].copy_from_slice(&c.to_be_bytes());
    ipv4_frame(a, PROTO_UDP, &u)
}

/// A UDP datagram taken apart.
pub struct Udp<'a> {
    pub sport: u16,
    pub dport: u16,
    pub data: &'a [u8],
}

pub fn parse_udp<'a>(ip: &Ipv4<'a>) -> Option<Udp<'a>> {
    let p = ip.payload;
    let len = usize::from(be16(p, 4)?);
    let dgram = p.get(..len)?;
    if len < 8 {
        return None;
    }
    if be16(p, 6)? != 0 && fold(sum(dgram, pseudo(ip.src, ip.dst, PROTO_UDP, len))) != 0 {
        return None;
    }
    Some(Udp {
        sport: be16(p, 0)?,
        dport: be16(p, 2)?,
        data: &dgram[8..],
    })
}

/// A TCP segment, parsed or to build.
#[derive(Clone, Copy, Debug)]
pub struct Tcp<'a> {
    pub sport: u16,
    pub dport: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    /// The maximum segment size option (sent with SYN only).
    pub mss: Option<u16>,
    pub data: &'a [u8],
}

impl Tcp<'_> {
    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

pub fn parse_tcp<'a>(ip: &Ipv4<'a>) -> Option<Tcp<'a>> {
    let p = ip.payload;
    let off = usize::from(*p.get(12)? >> 4) * 4;
    if off < 20 || fold(sum(p, pseudo(ip.src, ip.dst, PROTO_TCP, p.len()))) != 0 {
        return None;
    }
    let options = p.get(20..off)?;
    let mut mss = None;
    let mut i = 0;
    while let Some(&kind) = options.get(i) {
        match kind {
            0 => break,
            1 => i += 1,
            _ => {
                let Some(&len) = options.get(i + 1) else {
                    break;
                };
                let len = usize::from(len);
                if len < 2 {
                    break;
                }
                if kind == 2 && len == 4 {
                    mss = be16(options, i + 2);
                }
                i += len;
            }
        }
    }
    Some(Tcp {
        sport: be16(p, 0)?,
        dport: be16(p, 2)?,
        seq: be32(p, 4)?,
        ack: be32(p, 8)?,
        flags: *p.get(13)?,
        window: be16(p, 14)?,
        mss,
        data: p.get(off..)?,
    })
}

pub fn tcp_frame(a: &Addr, s: &Tcp<'_>) -> Option<Vec<u8>> {
    let hdr = if s.mss.is_some() { 24 } else { 20 };
    if hdr + s.data.len() + 20 > MTU {
        return None;
    }
    let mut t = Vec::with_capacity(hdr + s.data.len());
    t.extend_from_slice(&s.sport.to_be_bytes());
    t.extend_from_slice(&s.dport.to_be_bytes());
    t.extend_from_slice(&s.seq.to_be_bytes());
    t.extend_from_slice(&s.ack.to_be_bytes());
    t.push((hdr as u8 / 4) << 4);
    t.push(s.flags);
    t.extend_from_slice(&s.window.to_be_bytes());
    t.extend_from_slice(&[0, 0, 0, 0]);
    if let Some(mss) = s.mss {
        t.extend_from_slice(&[2, 4]);
        t.extend_from_slice(&mss.to_be_bytes());
    }
    t.extend_from_slice(s.data);
    let c = fold(sum(&t, pseudo(a.src, a.dst, PROTO_TCP, t.len())));
    t[16..18].copy_from_slice(&c.to_be_bytes());
    ipv4_frame(a, PROTO_TCP, &t)
}

/// An ICMP message (type, code, checksum, rest) with a fresh checksum.
pub fn icmp_message(kind: u8, code: u8, rest: &[u8]) -> Vec<u8> {
    let mut m = vec![kind, code, 0, 0];
    m.extend_from_slice(rest);
    let c = fold(sum(&m, 0));
    m[2..4].copy_from_slice(&c.to_be_bytes());
    m
}
