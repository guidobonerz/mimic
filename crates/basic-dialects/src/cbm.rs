//! Commodore BASIC (Microsoft BASIC for the 6502): PET BASIC 1.0, 2.0 and
//! 4.0, the VIC-20's and the C64's BASIC V2.
//!
//! Nothing here is tied to a ROM revision. The interpreter is found in the
//! CPU's view of `$8000`–`$FFFF` by two patterns:
//!
//! - the statement loop `JSR CHRGET : JSR GONE3 : JMP NEWSTT`
//!   (`20 lo hi 20 lo hi 4C lo hi`, the second target right behind the
//!   sequence, the third a few bytes before it; BASIC 4.0 calls a routine
//!   of its own in place of CHRGET). NEWSTT reads the text with
//!   `LDY #0 : LDA (TXTPTR),Y`; TXTPTR is CHRGET + 7, and CHRGET's
//!   address gives every other variable, since
//!   the interpreter's zero page has the same layout in all versions and
//!   only moves as a whole: TXTTAB = CHRGET − `$48`, VARTAB, ARYTAB,
//!   STREND and FRETOP 2 bytes apart behind it, CURLIN = TXTTAB + `$0E`
//!   (C64: `$2B`, `$39`, `$7A`). Where the loop is
//!   entered through `JMP ($0308)` (IGONE; VIC-20, C64) the stop point is
//!   that jump, so programs that hook the vector are followed too.
//! - the keyword table (`EN`,`D`+`$80`,`FO`,`R`+`$80` …), read from the
//!   ROM, so the listing shows the tokens the interpreter in the machine
//!   really has.
//!
//! At the stop point CURLIN holds the line (`$FFxx`: direct mode) and
//! TXTPTR points to the byte before the statement: the `:` or, at a line's
//! first statement, the line number's high byte. A program counts as
//! running while CURLIN is a line and TXTPTR is inside that line
//! (after a cold start CURLIN is not `$FFxx` yet; while INPUT reads the
//! input buffer the mode shows as direct for that moment).
//!
//! Program lines: link (2), number (2), text, 0. Variables (7 bytes from
//! VARTAB): two name bytes whose bits 7 give the type (none: float, both:
//! integer, second: string, first: function), then the 5-byte float
//! (exponent + `$80`, mantissa with the sign in bit 7 of its first byte),
//! the integer (high, low) or the string (length, pointer). Arrays (from
//! ARYTAB): name, size, number of dimensions, the sizes (high, low; last
//! dimension first), the elements with the first index running fastest.
//!
//! The GOSUB and FOR frames are on the CPU stack, ending below `$01FB`:
//! GOSUB `$8D`, CURLIN, TXTPTR (5 bytes; BASIC 2 and later leave the
//! statement loop's return address above it, seen on a C64); FOR `$81`, pointer to the
//! variable, STEP (5-byte mantissa form and a sign byte), limit (5-byte
//! float), CURLIN, TXTPTR (18 bytes). While the machine is halted inside a
//! statement, return addresses lie above them; the frames are found as the
//! first chain from the stack pointer that ends at the stack's top.
//!
//! Sources: Sheldon Leemon, "Mapping the Commodore 64" (zero page, stack
//! frames, variable formats); the PET memory maps in Raeto West,
//! "Programming the PET/CBM". No emulator source was used.

use emu_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use emu_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const SCAN_BASE: u32 = 0x8000;
const SCAN_LEN: usize = 0x8000;
const MAX_LINES: usize = 20_000;
const MAX_VARS: usize = 8_192;
const TOKEN_REM: u8 = 0x8f;
const TOKEN_FOR: u8 = 0x81;
const TOKEN_GOSUB: u8 = 0x8d;
const FRAME_GOSUB: u16 = 5;
const FRAME_FOR: u16 = 18;

/// What the patterns found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point: `JMP (IGONE)` or the `JSR CHRGET` of the loop.
    dispatch: u16,
    /// The three bytes there, to see whether the ROM is still mapped.
    signature: [u8; 3],
    chrget: u16,
    /// What `JSR GONE3` in the statement loop pushes: GOSUB leaves it on
    /// the stack above its frame (BASIC 2 and later).
    gone_return: u16,
    keywords_at: u16,
    keywords: Vec<String>,
}

impl Rom {
    fn txttab(&self) -> u16 {
        self.chrget.wrapping_sub(0x48)
    }
}

/// The decoder a Commodore machine keeps; see the module text.
#[derive(Clone, Debug)]
pub struct CbmBasic {
    space: MemSpaceId,
    rom: Option<Rom>,
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    /// Address of the terminating 0.
    end: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VarKind {
    Float,
    Int,
    Str,
    Function,
}

impl VarKind {
    fn of(b0: u8, b1: u8) -> Self {
        match (b0 & 0x80 != 0, b1 & 0x80 != 0) {
            (false, false) => VarKind::Float,
            (true, true) => VarKind::Int,
            (false, true) => VarKind::Str,
            (true, false) => VarKind::Function,
        }
    }

    fn ty(self) -> BasicType {
        match self {
            VarKind::Int => BasicType::Int,
            VarKind::Str => BasicType::Str,
            _ => BasicType::Float,
        }
    }

    fn element_bytes(self) -> u16 {
        match self {
            VarKind::Int => 2,
            VarKind::Str => 3,
            _ => 5,
        }
    }
}

struct Array {
    name: String,
    kind: VarKind,
    addr: u16,
    data: u16,
    /// As stored: last dimension first.
    dims: Vec<u32>,
}

impl Array {
    /// At most 65535 for the arrays [`View::arrays`] returns.
    fn elements(&self) -> u32 {
        self.dims.iter().fold(1u32, |n, &d| n.saturating_mul(d))
    }
}

enum RawFrame {
    Gosub {
        line: u16,
        txt: u16,
    },
    For {
        var: u16,
        step: [u8; 5],
        sign: u8,
        limit: [u8; 5],
        line: u16,
        txt: u16,
    },
}

/// PETSCII (upper case / graphics set) as `LIST` shows it; what has no
/// plain counterpart becomes `{$xx}`.
fn push_petscii(out: &mut String, b: u8) {
    match b {
        0x20..=0x5a => out.push(char::from(b)),
        0x5b => out.push('['),
        0x5c => out.push('£'),
        0x5d => out.push(']'),
        0x5e => out.push('↑'),
        0x5f => out.push('←'),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

fn petscii_from(c: char) -> Option<u8> {
    Some(match c {
        ' '..='Z' => c as u8,
        '[' => 0x5b,
        '£' => 0x5c,
        ']' => 0x5d,
        '↑' => 0x5e,
        '←' => 0x5f,
        _ => return None,
    })
}

/// A 5-byte float. `packed`: the sign is bit 7 of the first mantissa
/// byte (variables); otherwise that bit is part of the mantissa.
fn float_value(b: [u8; 5], packed: bool) -> f64 {
    if b[0] == 0 {
        return 0.0;
    }
    let mantissa = u32::from_be_bytes([b[1] | 0x80, b[2], b[3], b[4]]);
    let v = f64::from(mantissa) / 4_294_967_296.0 * 2f64.powi(i32::from(b[0]) - 128);
    if packed && b[1] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn float_bytes(v: f64) -> Result<[u8; 5], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 5]);
    }
    let mut exp = v.abs().log2().floor() as i32 + 1;
    let mut mantissa = (v.abs() / 2f64.powi(exp) * 4_294_967_296.0).round();
    // The logarithm may be off by one at powers of two; rounding may carry.
    if mantissa >= 4_294_967_296.0 {
        mantissa /= 2.0;
        exp += 1;
    } else if mantissa < 2_147_483_648.0 {
        mantissa *= 2.0;
        exp -= 1;
    }
    if exp < -127 {
        return Ok([0; 5]);
    }
    if exp > 127 {
        return Err(BasicError::OutOfRange);
    }
    let m = (mantissa as u32).to_be_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([(exp + 128) as u8, m[0] & 0x7f | sign, m[1], m[2], m[3]])
}

fn number(value: &BasicValue) -> Result<f64, BasicError> {
    match *value {
        BasicValue::Int(n) => Ok(n as f64),
        BasicValue::Float(f) => Ok(f),
        BasicValue::Str(_) => Err(BasicError::TypeMismatch),
    }
}

fn int16(value: &BasicValue) -> Result<i16, BasicError> {
    let f = number(value)?;
    if f.fract() != 0.0 || !(-32768.0..=32767.0).contains(&f) {
        return Err(BasicError::OutOfRange);
    }
    Ok(f as i16)
}

fn var_name(b0: u8, b1: u8, kind: VarKind) -> String {
    let mut name = String::new();
    for c in [b0 & 0x7f, b1 & 0x7f] {
        if c != 0 {
            push_petscii(&mut name, c);
        }
    }
    match kind {
        VarKind::Int => name.push('%'),
        VarKind::Str => name.push('$'),
        _ => {}
    }
    name
}

/// The interpreter as it is mapped now.
struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    rom: &'a Rom,
}

fn peek(host: &dyn DebugTarget, space: MemSpaceId, addr: u16) -> u8 {
    host.peek(space, u32::from(addr), Size::Byte)
        .map_or(0xff, |v| v as u8)
}

impl View<'_> {
    fn b(&self, addr: u16) -> u8 {
        peek(self.host, self.space, addr)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn bytes<const N: usize>(&self, addr: u16) -> [u8; N] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    /// A zero-page pointer by its distance from TXTTAB.
    fn pointer(&self, offset: u16) -> u16 {
        self.w(self.rom.txttab().wrapping_add(offset))
    }

    fn vartab(&self) -> u16 {
        self.pointer(2)
    }

    fn arytab(&self) -> u16 {
        self.pointer(4)
    }

    fn strend(&self) -> u16 {
        self.pointer(6)
    }

    fn fretop(&self) -> u16 {
        self.pointer(8)
    }

    fn curlin(&self) -> u16 {
        self.pointer(0x0e)
    }

    fn txtptr(&self) -> u16 {
        self.w(self.rom.chrget.wrapping_add(7))
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let mut at = self.pointer(0);
        while out.len() < MAX_LINES {
            let link = self.w(at);
            if link < 0x100 {
                break;
            }
            let text = at.wrapping_add(4);
            let mut end = text;
            while self.b(end) != 0 && end.wrapping_sub(text) < 255 {
                end = end.wrapping_add(1);
            }
            out.push(Line {
                addr: at,
                number: self.w(at.wrapping_add(2)),
                end,
            });
            if link <= at {
                break;
            }
            at = link;
        }
        out
    }

    /// A line as `LIST` shows it, where its statements begin in that text
    /// and the addresses of the colons between them.
    fn scan(&self, line: &Line) -> (String, Vec<u32>, Vec<u16>) {
        let mut text = String::new();
        let mut statements = vec![0];
        let mut colons = Vec::new();
        let (mut quote, mut rem) = (false, false);
        let mut at = line.addr.wrapping_add(4);
        while at != line.end {
            let b = self.b(at);
            match b {
                b'"' => {
                    quote = !quote;
                    text.push('"');
                }
                0xff if !quote => text.push('π'),
                0x80..=0xfe if !quote => {
                    rem |= b == TOKEN_REM;
                    match self.rom.keywords.get(usize::from(b - 0x80)) {
                        Some(k) => text.push_str(k),
                        None => push_petscii(&mut text, b),
                    }
                }
                b':' if !quote && !rem => {
                    text.push(':');
                    statements.push(text.len() as u32);
                    colons.push(at);
                }
                _ => push_petscii(&mut text, b),
            }
            at = at.wrapping_add(1);
        }
        (text, statements, colons)
    }

    /// The statement of line `number` the text pointer `txt` is in.
    /// `before`: a pointer on a colon still belongs to the statement
    /// before it (saved pointers of FOR and GOSUB).
    fn pos(&self, lines: &[Line], number: u16, txt: u16, before: bool) -> BasicPos {
        let statement = lines
            .iter()
            .find(|l| l.number == number && (l.addr..=l.end).contains(&txt))
            .map_or(0, |l| {
                let (_, _, colons) = self.scan(l);
                colons
                    .iter()
                    .filter(|&&c| if before { c < txt } else { c <= txt })
                    .count()
            });
        BasicPos {
            line: u32::from(number),
            statement: statement as u16,
        }
    }

    /// The frames on the CPU stack, innermost first.
    fn frames(&self) -> Vec<RawFrame> {
        let sp = self
            .host
            .regs()
            .get("SP")
            .or_else(|| self.host.regs().get("S"))
            .unwrap_or(0xff) as u16
            & 0xff;
        let chain = |start: u16| -> Option<Vec<RawFrame>> {
            let mut out = Vec::new();
            let mut p = start;
            loop {
                if (0x1fa..=0x200).contains(&p) {
                    return Some(out);
                }
                match self.b(p) {
                    TOKEN_GOSUB if p + FRAME_GOSUB <= 0x200 => {
                        out.push(RawFrame::Gosub {
                            line: self.w(p + 1),
                            txt: self.w(p + 3),
                        });
                        p += FRAME_GOSUB;
                        if p + 2 <= 0x200 && self.w(p) == self.rom.gone_return {
                            p += 2;
                        }
                    }
                    TOKEN_FOR if p + FRAME_FOR <= 0x200 => {
                        out.push(RawFrame::For {
                            var: self.w(p + 1),
                            step: self.bytes(p + 3),
                            sign: self.b(p + 8),
                            limit: self.bytes(p + 9),
                            line: self.w(p + 14),
                            txt: self.w(p + 16),
                        });
                        p += FRAME_FOR;
                    }
                    _ => return None,
                }
            }
        };
        let first = 0x101 + sp;
        (first..first + 64).find_map(chain).unwrap_or_default()
    }

    /// Plain variables: (entry address, name, kind).
    fn scalars(&self) -> Vec<(u16, String, VarKind)> {
        let (start, end) = (self.vartab(), self.arytab());
        let mut out = Vec::new();
        let mut at = start;
        while at < end && end - at >= 7 && out.len() < MAX_VARS {
            let (b0, b1) = (self.b(at), self.b(at + 1));
            let kind = VarKind::of(b0, b1);
            if kind != VarKind::Function {
                out.push((at, var_name(b0, b1, kind), kind));
            }
            at += 7;
        }
        out
    }

    fn arrays(&self) -> Vec<Array> {
        let (start, end) = (self.arytab(), self.strend());
        let mut out = Vec::new();
        let mut at = start;
        while at < end && end - at >= 7 && out.len() < MAX_VARS {
            let (b0, b1) = (self.b(at), self.b(at + 1));
            let kind = VarKind::of(b0, b1);
            let size = self.w(at + 2);
            let n = u16::from(self.b(at + 4));
            let header = 5 + 2 * n;
            if n == 0 || size < header || size > end - at {
                break;
            }
            let dims: Vec<u32> = (0..n)
                .map(|i| {
                    let d = at + 5 + 2 * i;
                    u32::from(u16::from_be_bytes([self.b(d), self.b(d + 1)]))
                })
                .collect();
            let array = Array {
                name: var_name(b0, b1, kind),
                kind,
                addr: at,
                data: at + header,
                dims,
            };
            // The elements have to fit the array's size.
            let fits = u64::from(array.elements()) * u64::from(kind.element_bytes())
                <= u64::from(size - header);
            if fits && kind != VarKind::Function {
                out.push(array);
            }
            at += size;
        }
        out
    }

    fn string(&self, at: u16) -> String {
        let (len, ptr) = (self.b(at), self.w(at.wrapping_add(1)));
        let mut s = String::new();
        for i in 0..u16::from(len) {
            push_petscii(&mut s, self.b(ptr.wrapping_add(i)));
        }
        s
    }

    /// The value stored at `at` (a variable's bytes behind its name, or an
    /// array element).
    fn value(&self, at: u16, kind: VarKind) -> BasicValue {
        match kind {
            VarKind::Int => {
                let bytes = [self.b(at), self.b(at.wrapping_add(1))];
                BasicValue::Int(i64::from(i16::from_be_bytes(bytes)))
            }
            VarKind::Str => BasicValue::Str(self.string(at)),
            _ => BasicValue::Float(float_value(self.bytes(at), true)),
        }
    }

    /// The bytes that store `value` at `at`.
    fn plan(
        &self,
        at: u16,
        kind: VarKind,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let (target, bytes): (u16, Vec<u8>) = match kind {
            VarKind::Int => (at, int16(value)?.to_be_bytes().to_vec()),
            VarKind::Str => {
                let BasicValue::Str(s) = value else {
                    return Err(BasicError::TypeMismatch);
                };
                let bytes = s
                    .chars()
                    .map(petscii_from)
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // Only in place: the same length, and not a literal in
                // the program text.
                let ptr = self.w(at.wrapping_add(1));
                if bytes.len() != usize::from(self.b(at)) || ptr < self.fretop() {
                    return Err(BasicError::Unsupported);
                }
                (ptr, bytes)
            }
            _ => (at, float_bytes(number(value)?)?.to_vec()),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(target.wrapping_add(i as u16)), b))
            .collect())
    }
}

impl CbmBasic {
    pub fn new(space: MemSpaceId) -> Self {
        Self { space, rom: None }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared, went away or changed.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.view(host).is_some() {
            return None;
        }
        let rom = find(host, self.space);
        (rom != self.rom).then_some(Self {
            space: self.space,
            rom,
        })
    }

    /// The interpreter, if it is mapped where it was found.
    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        let v = View {
            host,
            space: self.space,
            rom,
        };
        (v.bytes::<3>(rom.dispatch) == rom.signature
            && v.bytes::<3>(rom.keywords_at) == [b'E', b'N', b'D' | 0x80])
        .then_some(v)
    }
}

/// Search the CPU's view of `$8000`–`$FFFF` for the interpreter.
fn find(host: &dyn DebugTarget, space: MemSpaceId) -> Option<Rom> {
    let mem: Vec<u8> = (0..SCAN_LEN)
        .map(|i| peek(host, space, (SCAN_BASE as usize + i) as u16))
        .collect();
    let addr = |i: usize| (SCAN_BASE as usize + i) as u16;
    let word = |i: usize| u16::from_le_bytes([mem[i], mem[i + 1]]);
    let (loop_at, chrget) = (0x80..SCAN_LEN - 9).find_map(|i| {
        let w = &mem[i..i + 9];
        let here = addr(i);
        let back = usize::from(here.wrapping_sub(word(i + 7)));
        let fits = w[0] == 0x20
            && w[3] == 0x20
            && word(i + 4) == here.wrapping_add(9)
            && w[6] == 0x4c
            && (1..0x80).contains(&back);
        if !fits {
            return None;
        }
        // NEWSTT looks at the text: LDY #0 : LDA (TXTPTR),Y.
        let txtptr = mem[i - back..i]
            .windows(4)
            .find(|c| c[..3] == [0xa0, 0x00, 0xb1])
            .map(|c| u16::from(c[3]))?;
        Some((i, txtptr.checked_sub(7)?))
    })?;
    if chrget < 0x48 {
        return None;
    }
    let dispatch = if mem[loop_at - 3..loop_at] == [0x6c, 0x08, 0x03] {
        loop_at - 3
    } else {
        loop_at
    };
    const HEAD: &[u8] = &[
        b'E',
        b'N',
        b'D' | 0x80,
        b'F',
        b'O',
        b'R' | 0x80,
        b'N',
        b'E',
        b'X',
        b'T' | 0x80,
    ];
    let table = mem.windows(HEAD.len()).position(|w| w == HEAD)?;
    let mut keywords = Vec::new();
    let mut word_now = String::new();
    for &b in &mem[table..] {
        if b == 0 || keywords.len() == 0x7f {
            break;
        }
        push_petscii(&mut word_now, b & 0x7f);
        if b & 0x80 != 0 {
            keywords.push(std::mem::take(&mut word_now));
        }
    }
    Some(Rom {
        dispatch: addr(dispatch),
        signature: [mem[dispatch], mem[dispatch + 1], mem[dispatch + 2]],
        chrget,
        gone_return: addr(loop_at).wrapping_add(5),
        keywords_at: addr(table),
        keywords,
    })
}

impl Dialect for CbmBasic {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        let has = |word: &str| {
            self.rom
                .as_ref()
                .is_some_and(|r| r.keywords.iter().any(|k| k == word))
        };
        let dialect = if self.rom.is_none() {
            "Commodore BASIC"
        } else if has("DOPEN") {
            "Commodore BASIC 4.0"
        } else if has("GO") {
            "Commodore BASIC V2"
        } else {
            "Commodore BASIC 1.0"
        };
        BasicInfo {
            dialect: dialect.into(),
            granularity: BasicGranularity::Statement,
            space: self.space,
        }
    }

    fn state(&self, host: &dyn DebugTarget) -> BasicState {
        let Some(v) = self.view(host) else {
            return BasicState {
                mode: BasicMode::Inactive,
                pos: None,
                text_addr: None,
                depth: 0,
            };
        };
        let (curlin, txt) = (v.curlin(), v.txtptr());
        // After a cold start CURLIN is not yet $FFxx; a running program's
        // text pointer is inside the line CURLIN names.
        let lines = v.lines();
        let running = curlin >> 8 != 0xff
            && lines
                .iter()
                .any(|l| l.number == curlin && (l.addr..=l.end).contains(&txt));
        let depth = v
            .frames()
            .iter()
            .filter(|f| matches!(f, RawFrame::Gosub { .. }))
            .count();
        BasicState {
            mode: if running {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos: running.then(|| v.pos(&lines, curlin, txt, false)),
            text_addr: Some(u32::from(txt)),
            depth: depth as u32,
        }
    }

    fn program(&self, host: &dyn DebugTarget) -> Vec<BasicLine> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.lines()
            .iter()
            .map(|l| {
                let (text, statements, _) = v.scan(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (0..=l.end.wrapping_sub(l.addr))
                        .map(|i| v.b(l.addr.wrapping_add(i)))
                        .collect(),
                    text,
                    statements,
                }
            })
            .collect()
    }

    fn variables(&self, host: &dyn DebugTarget) -> Vec<BasicVar> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        let mut vars: Vec<BasicVar> = v
            .scalars()
            .into_iter()
            .map(|(at, name, kind)| BasicVar {
                name,
                ty: kind.ty(),
                addr: u32::from(at),
                value: Some(v.value(at + 2, kind)),
                dims: Vec::new(),
            })
            .collect();
        vars.extend(v.arrays().into_iter().map(|a| BasicVar {
            name: a.name,
            ty: a.kind.ty(),
            addr: u32::from(a.addr),
            value: None,
            dims: a.dims.into_iter().rev().collect(),
        }));
        vars
    }

    fn array(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        first: u32,
        count: u32,
    ) -> Result<Vec<BasicValue>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let a = v
            .arrays()
            .into_iter()
            .find(|a| a.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let total = a.elements();
        if first >= total {
            return Err(BasicError::OutOfRange);
        }
        let size = u32::from(a.kind.element_bytes());
        Ok((first..total.min(first.saturating_add(count)))
            .map(|i| v.value(a.data.wrapping_add((i * size) as u16), a.kind))
            .collect())
    }

    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        let lines = v.lines();
        v.frames()
            .into_iter()
            .map(|f| match f {
                RawFrame::Gosub { line, txt } => BasicFrame::Gosub {
                    from: v.pos(&lines, line, txt, true),
                },
                RawFrame::For {
                    var,
                    step,
                    sign,
                    limit,
                    line,
                    txt,
                } => {
                    let entry = var.wrapping_sub(2);
                    let (b0, b1) = (v.b(entry), v.b(entry.wrapping_add(1)));
                    let magnitude = float_value(step, false);
                    BasicFrame::For {
                        var: var_name(b0, b1, VarKind::of(b0, b1)),
                        limit: BasicValue::Float(float_value(limit, true)),
                        step: BasicValue::Float(if sign & 0x80 != 0 {
                            -magnitude
                        } else {
                            magnitude
                        }),
                        from: v.pos(&lines, line, txt, true),
                    }
                }
            })
            .collect()
    }

    fn plan_set(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        match index {
            None => {
                let (at, _, kind) = v
                    .scalars()
                    .into_iter()
                    .find(|(_, n, _)| n == name)
                    .ok_or(BasicError::NoSuchVariable)?;
                v.plan(at + 2, kind, value)
            }
            Some(i) => {
                let a = v
                    .arrays()
                    .into_iter()
                    .find(|a| a.name == name)
                    .ok_or(BasicError::NoSuchVariable)?;
                if i >= a.elements() {
                    return Err(BasicError::OutOfRange);
                }
                let at = a
                    .data
                    .wrapping_add((i * u32::from(a.kind.element_bytes())) as u16);
                v.plan(at, a.kind, value)
            }
        }
    }

    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        self.view(host)
            .map(|v| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(v.rom.dispatch),
                },
                granularity: BasicGranularity::Statement,
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests;
