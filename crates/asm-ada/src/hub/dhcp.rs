//! DHCP messages for the gateway's server (RFC 2131 section 2 and 4.3,
//! options from RFC 2132): the fields of a client's message the server
//! needs, and the replies OFFER, ACK and NAK.

use std::net::Ipv4Addr;

use super::packet::Mac;

/// Server and client ports (RFC 2131 4.1).
pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;
/// Message types (option 53, RFC 2132 9.6).
pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const DECLINE: u8 = 4;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;
pub const RELEASE: u8 = 7;
pub const INFORM: u8 = 8;
const COOKIE: [u8; 4] = [99, 130, 83, 99];

/// A client's message (BOOTREQUEST).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub xid: u32,
    pub flags: u16,
    pub ciaddr: Ipv4Addr,
    pub chaddr: Mac,
    pub msg_type: u8,
    /// Option 50.
    pub requested: Option<Ipv4Addr>,
    /// Option 54.
    pub server_id: Option<Ipv4Addr>,
    /// Option 12.
    pub host_name: Option<String>,
}

fn ip4(d: &[u8]) -> Option<Ipv4Addr> {
    let b: [u8; 4] = d.try_into().ok()?;
    Some(Ipv4Addr::from(b))
}

/// Parses a BOOTREQUEST for Ethernet with the DHCP cookie and a message
/// type; `None` for anything else.
pub fn parse(m: &[u8]) -> Option<Request> {
    if m.len() < 240 || m[0] != 1 || m[1] != 1 || m[2] != 6 || m[236..240] != COOKIE {
        return None;
    }
    let mut r = Request {
        xid: u32::from_be_bytes(m[4..8].try_into().ok()?),
        flags: u16::from_be_bytes([m[10], m[11]]),
        ciaddr: ip4(&m[12..16])?,
        chaddr: m[28..34].try_into().ok()?,
        msg_type: 0,
        requested: None,
        server_id: None,
        host_name: None,
    };
    let mut o = &m[240..];
    while let Some(&code) = o.first() {
        match code {
            0 => o = &o[1..],
            255 => break,
            _ => {
                let len = usize::from(*o.get(1)?);
                let v = o.get(2..2 + len)?;
                match code {
                    53 if len == 1 => r.msg_type = v[0],
                    50 => r.requested = ip4(v),
                    54 => r.server_id = ip4(v),
                    12 if len > 0 => r.host_name = Some(String::from_utf8_lossy(v).into_owned()),
                    _ => {}
                }
                o = &o[2 + len..];
            }
        }
    }
    (r.msg_type != 0).then_some(r)
}

/// What a reply carries besides the request's fields.
pub struct Reply<'a> {
    pub msg_type: u8,
    pub yiaddr: Ipv4Addr,
    pub server: Ipv4Addr,
    /// Mask, router/DNS server, domain, lease seconds; `None` for a NAK.
    pub config: Option<(Ipv4Addr, Ipv4Addr, &'a str, Option<u32>)>,
}

/// A BOOTREPLY to `req`, padded to the classic 300 bytes.
pub fn reply(req: &Request, r: &Reply<'_>) -> Vec<u8> {
    let mut m = vec![0u8; 240];
    m[0] = 2;
    m[1] = 1;
    m[2] = 6;
    m[4..8].copy_from_slice(&req.xid.to_be_bytes());
    m[10..12].copy_from_slice(&req.flags.to_be_bytes());
    if r.msg_type != NAK {
        m[12..16].copy_from_slice(&req.ciaddr.octets());
    }
    m[16..20].copy_from_slice(&r.yiaddr.octets());
    m[28..34].copy_from_slice(&req.chaddr);
    m[236..240].copy_from_slice(&COOKIE);
    m.extend_from_slice(&[53, 1, r.msg_type]);
    m.extend_from_slice(&[54, 4]);
    m.extend_from_slice(&r.server.octets());
    if let Some((mask, router, domain, lease)) = r.config {
        m.extend_from_slice(&[1, 4]);
        m.extend_from_slice(&mask.octets());
        m.extend_from_slice(&[3, 4]);
        m.extend_from_slice(&router.octets());
        m.extend_from_slice(&[6, 4]);
        m.extend_from_slice(&router.octets());
        m.extend_from_slice(&[15, domain.len() as u8]);
        m.extend_from_slice(domain.as_bytes());
        if let Some(l) = lease {
            m.extend_from_slice(&[51, 4]);
            m.extend_from_slice(&l.to_be_bytes());
        }
    }
    m.push(255);
    if m.len() < 300 {
        m.resize(300, 0);
    }
    m
}

/// Reads the options of a reply (for tests and logs): code → value.
pub fn options(m: &[u8]) -> std::collections::BTreeMap<u8, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut o = m.get(240..).unwrap_or(&[]);
    while let Some(&code) = o.first() {
        match code {
            0 => o = &o[1..],
            255 => break,
            _ => {
                let Some(&len) = o.get(1) else { break };
                let Some(v) = o.get(2..2 + usize::from(len)) else {
                    break;
                };
                out.insert(code, v.to_vec());
                o = &o[2 + usize::from(len)..];
            }
        }
    }
    out
}
