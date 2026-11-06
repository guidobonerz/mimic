//! DNS messages for the gateway's name server (RFC 1035 4.1): a query
//! with one question is parsed (compression pointers followed with care:
//! only backwards, at most 255 bytes of name), an answer copies the
//! question and gives at most one A record.

use std::net::Ipv4Addr;

/// Record type A and class IN; "any" for both.
pub const TYPE_A: u16 = 1;
pub const TYPE_ANY: u16 = 255;
pub const CLASS_IN: u16 = 1;
pub const CLASS_ANY: u16 = 255;
/// Response codes (RFC 1035 4.1.1).
pub const NOERROR: u8 = 0;
pub const FORMERR: u8 = 1;
pub const SERVFAIL: u8 = 2;
pub const NXDOMAIN: u8 = 3;
pub const NOTIMP: u8 = 4;
/// Time to live of the answers, in seconds.
const TTL: u32 = 60;

/// A query with its one question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub id: u16,
    /// Recursion desired, copied into the answer.
    pub rd: bool,
    /// The labels, joined with dots, as received (no trailing dot).
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    /// The question in wire form, uncompressed.
    pub question: Vec<u8>,
}

impl Query {
    /// Whether the question asks for an address (A or any, class IN or
    /// any).
    pub fn wants_a(&self) -> bool {
        matches!(self.qtype, TYPE_A | TYPE_ANY) && matches!(self.qclass, CLASS_IN | CLASS_ANY)
    }
}

/// What came in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Query(Query),
    /// A query that gets an error answer without a question.
    Error {
        id: u16,
        rd: bool,
        rcode: u8,
    },
    /// Not a query (a response) or too short to answer.
    Drop,
}

/// Reads a name at `pos`: its labels and the position after it in the
/// message (after the first pointer if there is one).
fn read_name(msg: &[u8], mut pos: usize) -> Option<(Vec<Vec<u8>>, usize)> {
    let mut labels = Vec::new();
    let mut total = 0usize;
    let mut end = None;
    // A pointer must point before the place it is read from: no loops.
    let mut limit = pos;
    loop {
        let len = *msg.get(pos)?;
        match len >> 6 {
            0 => {
                if len == 0 {
                    return Some((labels, end.unwrap_or(pos + 1)));
                }
                let l = usize::from(len);
                let label = msg.get(pos + 1..pos + 1 + l)?;
                total += l + 1;
                if total > 255 {
                    return None;
                }
                labels.push(label.to_vec());
                pos += 1 + l;
            }
            3 => {
                let to = (usize::from(len & 0x3f) << 8) | usize::from(*msg.get(pos + 1)?);
                if to >= limit {
                    return None;
                }
                end.get_or_insert(pos + 2);
                limit = to;
                pos = to;
            }
            _ => return None,
        }
    }
}

/// Parses a message; never panics on any input.
pub fn parse(msg: &[u8]) -> Parsed {
    if msg.len() < 12 {
        return Parsed::Drop;
    }
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    let rd = flags & 0x0100 != 0;
    if flags & 0x8000 != 0 {
        return Parsed::Drop;
    }
    let error = |rcode| Parsed::Error { id, rd, rcode };
    if (flags >> 11) & 15 != 0 {
        return error(NOTIMP);
    }
    if u16::from_be_bytes([msg[4], msg[5]]) != 1 {
        return error(FORMERR);
    }
    let Some((labels, pos)) = read_name(msg, 12) else {
        return error(FORMERR);
    };
    let Some(tc) = msg.get(pos..pos + 4) else {
        return error(FORMERR);
    };
    let qtype = u16::from_be_bytes([tc[0], tc[1]]);
    let qclass = u16::from_be_bytes([tc[2], tc[3]]);
    let mut question = Vec::new();
    for l in &labels {
        question.push(l.len() as u8);
        question.extend_from_slice(l);
    }
    question.push(0);
    question.extend_from_slice(tc);
    let name = labels
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect::<Vec<_>>()
        .join(".");
    Parsed::Query(Query {
        id,
        rd,
        name,
        qtype,
        qclass,
        question,
    })
}

fn header(id: u16, rd: bool, aa: bool, rcode: u8, qd: u16, an: u16) -> Vec<u8> {
    // QR, opcode 0, AA, RD copied, RA (recursion available).
    let flags = 0x8000 | (u16::from(aa) << 10) | (u16::from(rd) << 8) | 0x0080 | u16::from(rcode);
    let mut m = Vec::with_capacity(64);
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&flags.to_be_bytes());
    for n in [qd, an, 0, 0] {
        m.extend_from_slice(&n.to_be_bytes());
    }
    m
}

/// An answer to `q`: the question copied, and an A record for `ip` (its
/// name a pointer to the question's).
pub fn answer(q: &Query, rcode: u8, authoritative: bool, ip: Option<Ipv4Addr>) -> Vec<u8> {
    let mut m = header(q.id, q.rd, authoritative, rcode, 1, u16::from(ip.is_some()));
    m.extend_from_slice(&q.question);
    if let Some(ip) = ip {
        m.extend_from_slice(&[0xc0, 12]);
        m.extend_from_slice(&TYPE_A.to_be_bytes());
        m.extend_from_slice(&CLASS_IN.to_be_bytes());
        m.extend_from_slice(&TTL.to_be_bytes());
        m.extend_from_slice(&4u16.to_be_bytes());
        m.extend_from_slice(&ip.octets());
    }
    m
}

/// An error answer without a question.
pub fn error(id: u16, rd: bool, rcode: u8) -> Vec<u8> {
    header(id, rd, false, rcode, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(name: &str) -> Vec<u8> {
        let mut m = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for l in name.split('.') {
            m.push(l.len() as u8);
            m.extend_from_slice(l.as_bytes());
        }
        m.extend_from_slice(&[0, 0, 1, 0, 1]);
        m
    }

    #[test]
    fn a_query_parses_and_is_answered() {
        let Parsed::Query(q) = parse(&query("Hub.logophor")) else {
            panic!()
        };
        assert_eq!(
            (q.id, q.rd, q.name.as_str()),
            (0x1234, true, "Hub.logophor")
        );
        assert!(q.wants_a());
        let a = answer(&q, NOERROR, true, Some(Ipv4Addr::new(10, 16, 0, 1)));
        assert_eq!(&a[2..4], &[0x85, 0x80]);
        assert_eq!(&a[a.len() - 4..], &[10, 16, 0, 1]);
    }

    #[test]
    fn compression_pointers_are_followed_backwards_only() {
        // "a" followed by a pointer to the name's own start: a loop.
        let mut m = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        m.extend_from_slice(&[1, b'a', 0xc0, 12, 0, 1, 0, 1]);
        assert!(matches!(parse(&m), Parsed::Error { rcode: FORMERR, .. }));
        // A forward pointer, a truncated pointer, a bad label type.
        for tail in [&[0xc0u8, 20][..], &[0xc0], &[0x80, 1]] {
            let mut m = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
            m.extend_from_slice(tail);
            assert!(matches!(parse(&m), Parsed::Error { rcode: FORMERR, .. }));
        }
    }

    #[test]
    fn junk_never_panics() {
        let mut x = 12345u32;
        for len in 0..400 {
            let m: Vec<u8> = (0..len % 80)
                .map(|_| {
                    x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (x >> 16) as u8
                })
                .collect();
            let _ = parse(&m);
            let mut q = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
            q.extend_from_slice(&m);
            let _ = parse(&q);
        }
    }
}
