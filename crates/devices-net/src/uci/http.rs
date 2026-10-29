//! HTTP target (UCI target 6, firmware 3.15 on; source: Ultimate
//! documentation, "HTTP Target",
//! https://1541u-documentation.readthedocs.io/en/latest/uci/http_target.html).
//!
//! A request is a header handle (verb, URL, header fields) and an optional
//! body handle (binary, JSON object, JSON array or URL-encoded); an
//! exchange sends them through [`emu_core::host::NetBackend::http_request`]
//! and either hands back new handles for the response's header and body
//! (`$31`, JSON bodies parsed so they can be queried) or the raw body with
//! the response header on the status channel (`$32`). 16 header and 16
//! body slots; they survive a C64 reset (only the cartridge's power-off
//! frees them; [`super::Uci::reset`] keeps them).
//!
//! Status texts are the documentation's three-digit codes ("000 OK",
//! "400 BAD COMMAND", "507 NO HEADER SLOT", …); after an exchange the
//! status is the server's code and reason phrase (RFC 9110 15).
//!
//! Unverified (the documentation leaves them open, chosen plausibly):
//! handles count from 0; a URL without scheme gets `http://`; array
//! indices in paths count from 0; BODY_MOVE's path starts at the root;
//! the keys inside BODY_ADD's object values carry a length byte like the
//! top-level keys; a key at array level is ignored (the value is
//! appended); JSON `null` reads back as type `$00` without data, a
//! fractional number as a string, an integer outside 32 bits as a string;
//! strings and entry counts over 255 are cut; nested values in a
//! URL-encoded body are sent as JSON text; a body is parsed as JSON when
//! its content type names JSON and it parses (else binary); an invalid
//! handle answers "400 BAD REQUEST" for every command.

use super::*;

const SLOTS: usize = 16;
const IDENT: &[u8] = b"ULTIMATE HTTP TARGET V1.0";
/// Status channel limit of the raw exchange's response header.
const HEAD_MAX: usize = 256;
const VERBS: [&str; 9] = [
    "GET", "PUT", "POST", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];
/// "No body" in the exchange commands.
const NO_BODY: u8 = 0xff;

/// Body formats of BODY_CREATE.
pub mod format {
    pub const BINARY: u8 = 1;
    pub const OBJECT: u8 = 2;
    pub const ARRAY: u8 = 3;
    pub const URL_ENCODED: u8 = 4;
}

/// Value types of BODY_ADD and BODY_QUERY.
pub mod kind {
    pub const NULL: u8 = 0;
    pub const INT: u8 = 1;
    pub const BOOL: u8 = 2;
    pub const STRING: u8 = 3;
    pub const OBJECT: u8 = 4;
    pub const ARRAY: u8 = 5;
}

/// A JSON value; objects keep their keys in insertion order so requests
/// and savestates are deterministic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    /// A number with fraction or exponent, kept as its text.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Header {
    /// 1–9 for a request ([`VERBS`]); 0 for a response's header.
    verb: u8,
    /// The request's URL with scheme.
    url: String,
    /// A response's status code.
    status: u16,
    fields: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Body {
    format: u8,
    data: Vec<u8>,
    json: Json,
    pointer: Vec<Step>,
}

/// The target's handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpState {
    headers: Vec<Option<Header>>,
    bodies: Vec<Option<Body>>,
}

impl Default for HttpState {
    fn default() -> Self {
        Self {
            headers: vec![None; SLOTS],
            bodies: vec![None; SLOTS],
        }
    }
}

/// RFC 9110 reason phrases (section 15) for the status channel.
fn reason(code: u16) -> &'static str {
    match code {
        100 => "CONTINUE",
        200 => "OK",
        201 => "CREATED",
        202 => "ACCEPTED",
        204 => "NO CONTENT",
        206 => "PARTIAL CONTENT",
        301 => "MOVED PERMANENTLY",
        302 => "FOUND",
        303 => "SEE OTHER",
        304 => "NOT MODIFIED",
        307 => "TEMPORARY REDIRECT",
        308 => "PERMANENT REDIRECT",
        400 => "BAD REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT FOUND",
        405 => "METHOD NOT ALLOWED",
        408 => "REQUEST TIMEOUT",
        409 => "CONFLICT",
        410 => "GONE",
        413 => "CONTENT TOO LARGE",
        415 => "UNSUPPORTED MEDIA TYPE",
        422 => "UNPROCESSABLE CONTENT",
        429 => "TOO MANY REQUESTS",
        500 => "INTERNAL SERVER ERROR",
        501 => "NOT IMPLEMENTED",
        502 => "BAD GATEWAY",
        503 => "SERVICE UNAVAILABLE",
        504 => "GATEWAY TIMEOUT",
        _ => "",
    }
}

fn status_text(code: u16) -> String {
    format!("{code:03} {}", reason(code)).trim_end().to_string()
}

// --- JSON text ------------------------------------------------------------

impl Json {
    /// Compact JSON text (RFC 8259).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => out.push_str(&i.to_string()),
            Json::Num(n) => out.push_str(n),
            Json::Str(s) => write_str(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }

    /// Parse JSON text; `None` if it is not valid JSON (nesting is limited
    /// to 64 levels).
    pub fn parse(text: &str) -> Option<Json> {
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value(0)?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|b| b" \t\r\n".contains(b)) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.i..].starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: u8) -> Option<Json> {
        if depth > 64 {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            b'n' => self.eat(b"null").then_some(Json::Null),
            b't' => self.eat(b"true").then_some(Json::Bool(true)),
            b'f' => self.eat(b"false").then_some(Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b"]") {
                    return Some(Json::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b"]") {
                        return Some(Json::Arr(a));
                    }
                    if !self.eat(b",") {
                        return None;
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut o = Vec::new();
                self.ws();
                if self.eat(b"}") {
                    return Some(Json::Obj(o));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(b":") {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    set_key(&mut o, k, v);
                    self.ws();
                    if self.eat(b"}") {
                        return Some(Json::Obj(o));
                    }
                    if !self.eat(b",") {
                        return None;
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|b| b"+-.eE0123456789".contains(b))
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).ok()?;
                match t.parse::<i64>() {
                    Ok(v) => Some(Json::Int(v)),
                    Err(_) => t.parse::<f64>().ok().map(|_| Json::Num(t.to_string())),
                }
            }
            _ => None,
        }
    }

    fn string(&mut self) -> Option<String> {
        if !self.eat(b"\"") {
            return None;
        }
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.s.get(self.i).is_some_and(|&b| b != b'"' && b != b'\\') {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).ok()?);
            match *self.s.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1;
                    let c = *self.s.get(self.i)?;
                    self.i += 1;
                    out.push(match c {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let mut cp = u32::from_str_radix(hex, 16).ok()?;
                            // A surrogate pair.
                            if (0xd800..0xdc00).contains(&cp) && self.eat(b"\\u") {
                                let lo =
                                    std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                                self.i += 4;
                                let lo = u32::from_str_radix(lo, 16).ok()?;
                                cp = 0x10000 + ((cp - 0xd800) << 10) + (lo.wrapping_sub(0xdc00));
                            }
                            char::from_u32(cp).unwrap_or('\u{fffd}')
                        }
                        _ => return None,
                    });
                }
            }
        }
    }
}

fn set_key(o: &mut Vec<(String, Json)>, k: String, v: Json) {
    match o.iter_mut().find(|(key, _)| *key == k) {
        Some(slot) => slot.1 = v,
        None => o.push((k, v)),
    }
}

// --- the C64's encoding ---------------------------------------------------

/// Bytes from the C64 as text: one char per byte (Latin-1).
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Text to the C64: one byte per char (Latin-1; others become `?`).
fn to_bytes(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

fn short_str(s: &str, out: &mut Vec<u8>) {
    let b = to_bytes(s);
    let n = b.len().min(255);
    out.push(n as u8);
    out.extend_from_slice(&b[..n]);
}

/// BODY_QUERY's encoding of a value.
fn encode(v: &Json, out: &mut Vec<u8>) {
    match v {
        Json::Null => out.push(kind::NULL),
        Json::Int(i) if i32::try_from(*i).is_ok() => {
            out.push(kind::INT);
            out.extend((*i as i32).to_le_bytes());
        }
        Json::Int(i) => {
            out.push(kind::STRING);
            short_str(&i.to_string(), out);
        }
        Json::Num(n) => {
            out.push(kind::STRING);
            short_str(n, out);
        }
        Json::Bool(b) => out.extend([kind::BOOL, u8::from(*b)]),
        Json::Str(s) => {
            out.push(kind::STRING);
            short_str(s, out);
        }
        Json::Obj(o) => {
            out.push(kind::OBJECT);
            out.push(o.len().min(255) as u8);
            for (k, v) in o.iter().take(255) {
                short_str(k, out);
                encode(v, out);
            }
        }
        Json::Arr(a) => {
            out.push(kind::ARRAY);
            out.push(a.len().min(255) as u8);
            for v in a.iter().take(255) {
                encode(v, out);
            }
        }
    }
}

/// Reads BODY_ADD's encoding.
struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn done(&self) -> bool {
        self.i >= self.b.len()
    }

    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let v = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(v)
    }

    fn short_str(&mut self) -> Option<String> {
        let n = usize::from(self.u8()?);
        self.take(n).map(latin1)
    }

    fn value(&mut self, depth: u8) -> Option<Json> {
        if depth > 32 {
            return None;
        }
        Some(match self.u8()? {
            kind::INT => {
                let b = self.take(4)?;
                Json::Int(i64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            }
            kind::BOOL => Json::Bool(self.u8()? != 0),
            kind::STRING => Json::Str(self.short_str()?),
            kind::OBJECT => {
                let n = self.u8()?;
                let mut o = Vec::new();
                for _ in 0..n {
                    let k = self.short_str()?;
                    let v = self.value(depth + 1)?;
                    set_key(&mut o, k, v);
                }
                Json::Obj(o)
            }
            kind::ARRAY => {
                let n = self.u8()?;
                let mut a = Vec::new();
                for _ in 0..n {
                    a.push(self.value(depth + 1)?);
                }
                Json::Arr(a)
            }
            _ => return None,
        })
    }
}

/// `user/cars%2`, `%1/name` → steps.
fn parse_path(text: &str) -> Option<Vec<Step>> {
    let mut steps = Vec::new();
    for seg in text.split('/').filter(|s| !s.is_empty()) {
        let mut parts = seg.split('%');
        let key = parts.next()?;
        if !key.is_empty() {
            steps.push(Step::Key(key.to_string()));
        }
        for idx in parts {
            steps.push(Step::Index(idx.parse().ok()?));
        }
    }
    Some(steps)
}

fn node<'a>(root: &'a Json, path: &[Step]) -> Option<&'a Json> {
    path.iter().try_fold(root, |v, step| match (v, step) {
        (Json::Obj(o), Step::Key(k)) => o.iter().find(|(key, _)| key == k).map(|(_, v)| v),
        (Json::Arr(a), Step::Index(i)) => a.get(*i),
        _ => None,
    })
}

fn node_mut<'a>(root: &'a mut Json, path: &[Step]) -> Option<&'a mut Json> {
    path.iter().try_fold(root, |v, step| match (v, step) {
        (Json::Obj(o), Step::Key(k)) => o.iter_mut().find(|(key, _)| key == k).map(|(_, v)| v),
        (Json::Arr(a), Step::Index(i)) => a.get_mut(*i),
        _ => None,
    })
}

/// Percent-encoding of `application/x-www-form-urlencoded` (WHATWG URL,
/// "application/x-www-form-urlencoded serializing").
fn form_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(b))
            }
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
}

impl Body {
    fn new(format: u8) -> Self {
        Self {
            format,
            data: Vec::new(),
            json: if format == format::ARRAY {
                Json::Arr(Vec::new())
            } else {
                Json::Obj(Vec::new())
            },
            pointer: Vec::new(),
        }
    }

    fn content_type(&self) -> &'static str {
        match self.format {
            format::BINARY => "application/octet-stream",
            format::URL_ENCODED => "application/x-www-form-urlencoded",
            _ => "application/json",
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match self.format {
            format::BINARY => self.data.clone(),
            format::URL_ENCODED => {
                let mut out = String::new();
                if let Json::Obj(o) = &self.json {
                    for (i, (k, v)) in o.iter().enumerate() {
                        if i > 0 {
                            out.push('&');
                        }
                        form_encode(k, &mut out);
                        out.push('=');
                        let text = match v {
                            Json::Str(s) | Json::Num(s) => s.clone(),
                            Json::Null => String::new(),
                            other => other.to_text(),
                        };
                        form_encode(&text, &mut out);
                    }
                }
                out.into_bytes()
            }
            _ => self.json.to_text().into_bytes(),
        }
    }

    /// Add `v` under `key` at the pointer (appended in an array).
    fn add(&mut self, key: String, v: Json) -> bool {
        match node_mut(&mut self.json, &self.pointer) {
            Some(Json::Obj(o)) => {
                set_key(o, key, v);
                true
            }
            Some(Json::Arr(a)) => {
                a.push(v);
                true
            }
            _ => false,
        }
    }

    /// Add a container and move the pointer into it.
    fn add_container(&mut self, key: String, v: Json) -> bool {
        let step = match node(&self.json, &self.pointer) {
            Some(Json::Obj(_)) => Step::Key(key.clone()),
            Some(Json::Arr(a)) => Step::Index(a.len()),
            _ => return false,
        };
        self.add(key, v) && {
            self.pointer.push(step);
            true
        }
    }
}

impl HttpState {
    fn header(&mut self, h: u8) -> Option<&mut Header> {
        self.headers.get_mut(usize::from(h))?.as_mut()
    }

    fn body(&mut self, h: u8) -> Option<&mut Body> {
        self.bodies.get_mut(usize::from(h))?.as_mut()
    }

    fn new_header(&mut self, hd: Header) -> Option<u8> {
        let i = self.headers.iter().position(Option::is_none)?;
        self.headers[i] = Some(hd);
        Some(i as u8)
    }

    fn new_body(&mut self, b: Body) -> Option<u8> {
        let i = self.bodies.iter().position(Option::is_none)?;
        self.bodies[i] = Some(b);
        Some(i as u8)
    }

    pub fn save(&self, w: &mut StateWriter) {
        for h in &self.headers {
            w.bool(h.is_some());
            if let Some(h) = h {
                w.u8(h.verb);
                w.str(&h.url);
                w.u16(h.status);
                w.u32(h.fields.len() as u32);
                for (k, v) in &h.fields {
                    w.str(k);
                    w.str(v);
                }
            }
        }
        for b in &self.bodies {
            w.bool(b.is_some());
            if let Some(b) = b {
                w.u8(b.format);
                w.bytes(&b.data);
                w.str(&b.json.to_text());
                w.u32(b.pointer.len() as u32);
                for s in &b.pointer {
                    match s {
                        Step::Key(k) => {
                            w.u8(0);
                            w.str(k);
                        }
                        Step::Index(i) => {
                            w.u8(1);
                            w.u32(*i as u32);
                        }
                    }
                }
            }
        }
    }

    pub fn load(r: &mut StateReader<'_>) -> Result<Self, StateError> {
        let bad = StateError::Invalid("UCI HTTP state");
        let mut s = Self::default();
        for slot in &mut s.headers {
            if r.bool()? {
                let verb = r.u8()?;
                let url = r.string()?;
                let status = r.u16()?;
                let n = r.u32()? as usize;
                if verb > 9 || n > r.remaining() {
                    return Err(bad);
                }
                let mut fields = Vec::with_capacity(n);
                for _ in 0..n {
                    fields.push((r.string()?, r.string()?));
                }
                *slot = Some(Header {
                    verb,
                    url,
                    status,
                    fields,
                });
            }
        }
        for slot in &mut s.bodies {
            if r.bool()? {
                let format = r.u8()?;
                let data = r.bytes()?.to_vec();
                let json = Json::parse(&r.string()?).ok_or(bad.clone())?;
                let n = r.u32()? as usize;
                if !(1..=4).contains(&format) || n > r.remaining() {
                    return Err(bad);
                }
                let mut pointer = Vec::with_capacity(n);
                for _ in 0..n {
                    pointer.push(match r.u8()? {
                        0 => Step::Key(r.string()?),
                        1 => Step::Index(r.u32()? as usize),
                        _ => return Err(bad),
                    });
                }
                *slot = Some(Body {
                    format,
                    data,
                    json,
                    pointer,
                });
            }
        }
        Ok(s)
    }
}

impl Uci {
    pub(super) fn http_command(&mut self, cmd: &[u8]) {
        let Some((&op, p)) = cmd.split_first() else {
            self.set_status("400 BAD COMMAND");
            return self.finish(false);
        };
        // Most commands name a handle first.
        let handle = p.first().copied();
        let rest = p.get(1..).unwrap_or(&[]);
        match op {
            0x01 => {
                self.set_resp(IDENT.to_vec());
                self.set_status("000 OK");
            }
            0x10 => {
                self.http = HttpState::default();
                self.set_status("000 OK");
            }
            0x11 => match (handle, rest) {
                (Some(verb @ 1..=9), url) if !url.is_empty() => {
                    let url = latin1(url);
                    let url = if url.contains("://") {
                        url
                    } else {
                        format!("http://{url}")
                    };
                    let hd = Header {
                        verb,
                        url,
                        ..Default::default()
                    };
                    match self.http.new_header(hd) {
                        Some(h) => {
                            self.set_resp(vec![h]);
                            self.set_status("000 OK");
                        }
                        None => self.set_status("507 NO HEADER SLOT"),
                    }
                }
                _ => self.set_status("400 BAD COMMAND"),
            },
            0x12 => {
                if let Some(slot) = handle.and_then(|h| self.http.headers.get_mut(usize::from(h))) {
                    *slot = None;
                }
                self.set_status("000 OK");
            }
            0x13 => {
                let line = latin1(rest);
                let status = match (
                    handle.and_then(|h| self.http.header(h)),
                    line.split_once(": "),
                ) {
                    (None, _) => "400 BAD REQUEST",
                    (Some(_), None) => "500 BAD FORMAT",
                    (Some(hd), Some((k, v))) if !k.trim().is_empty() => {
                        let (k, v) = (k.trim().to_string(), v.trim().to_string());
                        match hd
                            .fields
                            .iter_mut()
                            .find(|(n, _)| n.eq_ignore_ascii_case(&k))
                        {
                            Some(f) => f.1 = v,
                            None => hd.fields.push((k, v)),
                        }
                        "000 OK"
                    }
                    _ => "500 BAD FORMAT",
                };
                self.set_status(status);
            }
            0x14 => {
                let key = latin1(rest);
                match handle.and_then(|h| self.http.header(h)) {
                    None => self.set_status("400 BAD REQUEST"),
                    Some(hd) => {
                        match hd.fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
                            Some((_, v)) => {
                                let v = to_bytes(v);
                                self.set_status("000 OK");
                                return self.respond(v);
                            }
                            None => self.set_status("404 KEY NOT PRESENT"),
                        }
                    }
                }
            }
            0x15 => {
                let index = usize::from(rest.first().copied().unwrap_or(0));
                match handle.and_then(|h| self.http.header(h)).cloned() {
                    None => self.set_status("400 BAD REQUEST"),
                    Some(hd) if index == 0 => {
                        self.set_status("000 OK");
                        return self.respond(to_bytes(&header_text(&hd)));
                    }
                    Some(hd) => match hd.fields.get(index - 1) {
                        Some((k, v)) => {
                            self.set_status("000 OK");
                            return self.respond(to_bytes(&format!("{k}: {v}")));
                        }
                        None => self.set_status("400 INDEX OUT OF BOUNDS"),
                    },
                }
            }
            0x21 => match handle {
                Some(f @ 1..=4) if rest.is_empty() => match self.http.new_body(Body::new(f)) {
                    Some(h) => {
                        self.set_resp(vec![h]);
                        self.set_status("000 OK");
                    }
                    None => self.set_status("507 NO DATA SLOT"),
                },
                _ => self.set_status("400 BAD COMMAND"),
            },
            0x22 => {
                if let Some(slot) = handle.and_then(|h| self.http.bodies.get_mut(usize::from(h))) {
                    *slot = None;
                }
                self.set_status("000 OK");
            }
            0x2e => match handle.and_then(|h| self.http.body(h)) {
                Some(b) => {
                    *b = Body::new(b.format);
                    self.set_status("000 OK");
                }
                None => self.set_status("400 BAD REQUEST"),
            },
            0x23..=0x27 | 0x2d => {
                let status = match handle.and_then(|h| self.http.body(h)) {
                    None => "400 BAD REQUEST",
                    Some(b) if b.format == format::BINARY => "400 BAD REQUEST",
                    Some(b) => {
                        if add_items(b, op, rest).is_some() {
                            "000 OK"
                        } else {
                            "400 BAD COMMAND"
                        }
                    }
                };
                self.set_status(status);
            }
            0x28 => match handle.and_then(|h| self.http.body(h)) {
                Some(b) => {
                    b.pointer.pop();
                    self.set_status("000 OK");
                }
                None => self.set_status("400 BAD REQUEST"),
            },
            0x29..=0x2b => {
                let path = latin1(rest);
                let Some(b) = handle.and_then(|h| self.http.body(h)) else {
                    self.set_status("400 BAD REQUEST");
                    return self.finish(false);
                };
                if b.format == format::BINARY {
                    self.set_status("400 BAD REQUEST");
                    return self.finish(false);
                }
                let Some(steps) = parse_path(&path) else {
                    self.set_status("400 BAD FORMAT: BAD PATH");
                    return self.finish(false);
                };
                match op {
                    0x2a => match node(&b.json, &steps) {
                        Some(v) => {
                            let mut out = Vec::new();
                            encode(v, &mut out);
                            self.set_status("000 OK");
                            return self.respond(out);
                        }
                        None => self.set_status("400 BAD FORMAT: PATH NOT FOUND"),
                    },
                    0x2b => match node(&b.json, &steps) {
                        Some(Json::Obj(_) | Json::Arr(_)) => {
                            b.pointer = steps;
                            self.set_status("000 OK");
                        }
                        Some(_) => self.set_status("400 BAD FORMAT: NOT AN OBJECT OR ARRAY"),
                        None => self.set_status("400 BAD FORMAT: PATH NOT FOUND"),
                    },
                    _ => {
                        let Some((last, parent)) = steps.split_last() else {
                            self.set_status("400 BAD FORMAT: EMPTY PATH");
                            return self.finish(false);
                        };
                        let removed = match (node_mut(&mut b.json, parent), last) {
                            (Some(Json::Obj(o)), Step::Key(k)) => {
                                let before = o.len();
                                o.retain(|(key, _)| key != k);
                                o.len() < before
                            }
                            (Some(Json::Arr(a)), Step::Index(i)) if *i < a.len() => {
                                a.remove(*i);
                                true
                            }
                            _ => false,
                        };
                        if removed {
                            // The pointer may have pointed into what went.
                            if node(&b.json, &b.pointer).is_none() {
                                b.pointer.clear();
                            }
                            self.set_status("000 OK");
                        } else {
                            self.set_status("400 BAD FORMAT: PATH NOT FOUND");
                        }
                    }
                }
            }
            0x2c => match handle.and_then(|h| self.http.body(h)) {
                Some(b) if b.format == format::BINARY => {
                    b.data.extend_from_slice(rest);
                    self.set_status("000 OK");
                }
                _ => self.set_status("400 BAD REQUEST"),
            },
            0x31 | 0x32 => {
                let (Some(h), Some(&b)) = (handle, rest.first()) else {
                    self.set_status("400 BAD COMMAND");
                    return self.finish(false);
                };
                return self.exchange(h, b, op == 0x32);
            }
            _ => self.set_status("400 BAD COMMAND"),
        }
        self.finish(false)
    }

    /// DO_EXCHANGE: send the request; the answer is polled.
    fn exchange(&mut self, h: u8, b: u8, raw: bool) {
        let Some(hd) = self.http.header(h).cloned().filter(|hd| hd.verb != 0) else {
            self.set_status("400 BAD REQUEST");
            return self.finish(false);
        };
        let body = if b == NO_BODY {
            None
        } else {
            match self.http.body(b) {
                Some(body) => Some(body.clone()),
                None => {
                    self.set_status("400 BAD REQUEST");
                    return self.finish(false);
                }
            }
        };
        let mut req = HttpRequest {
            method: VERBS[usize::from(hd.verb - 1)].into(),
            url: hd.url.clone(),
            headers: hd.fields.clone(),
            body: Vec::new(),
        };
        if let Some(body) = &body {
            if req.header("content-type").is_none() {
                req.headers
                    .push(("Content-Type".into(), body.content_type().into()));
            }
            req.body = body.bytes();
        }
        let r = match &self.host.net {
            Some(n) => lock(n).http_request(&req),
            None => Err(NetError::NoNetwork),
        };
        match r {
            Ok(ticket) => {
                self.pending = Pending::Http {
                    ticket,
                    header: h,
                    body: b,
                    raw,
                };
                self.state = State::Busy;
                self.timer = POLL;
            }
            Err(_) => {
                self.set_status("503 SERVICE UNAVAILABLE");
                self.finish(false)
            }
        }
    }

    pub(super) fn poll_exchange(&mut self, ticket: HttpTicket, header: u8, body: u8, raw: bool) {
        let r = match &self.host.net {
            Some(n) => lock(n).poll_http(ticket),
            None => Some(Err(NetError::NoNetwork)),
        };
        let res = match r {
            None => {
                self.pending = Pending::Http {
                    ticket,
                    header,
                    body,
                    raw,
                };
                self.state = State::Busy;
                self.timer = POLL;
                return;
            }
            Some(Err(_)) => {
                self.pending = Pending::None;
                self.set_status("503 SERVICE UNAVAILABLE");
                return self.finish(false);
            }
            Some(Ok(res)) => res,
        };
        self.pending = Pending::None;
        let hd = Header {
            verb: 0,
            url: String::new(),
            status: res.status,
            fields: res.headers,
        };
        if raw {
            let mut head = to_bytes(&header_text(&hd));
            head.truncate(HEAD_MAX);
            self.stat = head;
            self.stat_pos = 0;
            return self.respond(res.body);
        }
        let json_type = hd.fields.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("json")
        });
        let parsed = json_type
            .then(|| std::str::from_utf8(&res.body).ok().and_then(Json::parse))
            .flatten();
        let mut new_body = match parsed {
            Some(j @ Json::Obj(_)) => Body {
                json: j,
                ..Body::new(format::OBJECT)
            },
            Some(j @ Json::Arr(_)) => Body {
                json: j,
                ..Body::new(format::ARRAY)
            },
            _ => Body::new(format::BINARY),
        };
        if new_body.format == format::BINARY {
            new_body.data = res.body;
        }
        let code = hd.status;
        let Some(h) = self.http.new_header(hd) else {
            self.set_status("507 NO HEADER SLOT");
            return self.finish(false);
        };
        let Some(b) = self.http.new_body(new_body) else {
            self.http.headers[usize::from(h)] = None;
            self.set_status("507 NO DATA SLOT");
            return self.finish(false);
        };
        self.set_resp(vec![h, b]);
        self.set_status(&status_text(code));
        self.finish(false)
    }
}

/// HEADER_LIST 0: the request line (or status line) and the fields.
fn header_text(hd: &Header) -> String {
    let mut out = if hd.verb == 0 {
        format!("HTTP/1.1 {} {}\r\n", hd.status, reason(hd.status))
    } else {
        let rest = hd.url.split_once("://").map_or(hd.url.as_str(), |(_, r)| r);
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        let (host, path) = rest.split_at(split);
        let path = if path.is_empty() { "/" } else { path };
        format!(
            "{} {path} HTTP/1.1\r\nHost: {host}\r\n",
            VERBS[usize::from(hd.verb - 1)]
        )
    };
    for (k, v) in &hd.fields {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out
}

/// BODY_ADD and the single-value helpers.
fn add_items(b: &mut Body, op: u8, p: &[u8]) -> Option<()> {
    let mut r = Reader { b: p, i: 0 };
    let at_array = matches!(node(&b.json, &b.pointer), Some(Json::Arr(_)));
    match op {
        0x2d => {
            if r.done() {
                return None;
            }
            while !r.done() {
                let key = if at_array {
                    String::new()
                } else {
                    r.short_str()?
                };
                let v = r.value(0)?;
                b.add(key, v).then_some(())?;
            }
            Some(())
        }
        _ => {
            let key = r.short_str()?;
            match op {
                0x23 => {
                    let raw = r.b.get(r.i..).filter(|v| (1..=4).contains(&v.len()))?;
                    // Signed, LSB first: sign-extend from the top byte.
                    let mut v = i64::from(raw[raw.len() - 1] as i8);
                    for &byte in raw.iter().rev().skip(1) {
                        v = (v << 8) | i64::from(byte);
                    }
                    b.add(key, Json::Int(v)).then_some(())
                }
                0x24 => {
                    let v = r.u8()?;
                    r.done().then_some(())?;
                    b.add(key, Json::Bool(v != 0)).then_some(())
                }
                0x25 => {
                    let v = r.short_str()?;
                    r.done().then_some(())?;
                    b.add(key, Json::Str(v)).then_some(())
                }
                0x26 => {
                    r.done().then_some(())?;
                    b.add_container(key, Json::Obj(Vec::new())).then_some(())
                }
                _ => {
                    r.done().then_some(())?;
                    b.add_container(key, Json::Arr(Vec::new())).then_some(())
                }
            }
        }
    }
}
