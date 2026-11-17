//! Locomotive BASIC on the Amstrad CPC: 1.0 (464) and 1.1 (664, 6128).
//!
//! **Program.** From the byte behind the one the "start of program"
//! pointer names (`&0170`). A line is its length (two bytes, low first, 0
//! at the program's end), its number, the tokens and a 0. `&01` separates
//! statements; before ELSE and the tick it is there too and LIST hides it.
//! A variable is a token for its type (`&02` `%`, `&03` `$`, `&04` `!`,
//! `&0B`–`&0D` without a sign), two bytes the interpreter fills in, and
//! the name, the last character with bit 7 set. Numbers are tokens:
//! `&0E`–`&18` the values 0–10, `&19` a byte, `&1A` a word, `&1B` binary,
//! `&1C` hexadecimal, `&1D` the address of a line (a line number the
//! interpreter has already looked up), `&1E` a line number, `&1F` a
//! floating point number. Keywords are `&80`–`&FE`, functions `&FF` and a
//! second byte. The listing writes a line as the ROM's LIST does (`&E25D`
//! in the 6128's ROM): the text as stored, with a blank only where a
//! word, a name or a number would run into the next one. The words come
//! from the ROM's own table, 26 lists by first letter and one of symbols.
//!
//! **Where the interpreter is.** The statement loop stores the address of
//! the byte before the statement it begins (the line number's high byte,
//! a `&01`) and, for every line, the address of its number; that one is 0
//! while a command typed in runs and when the prompt appears. The stop
//! point is behind the first store. The interpreter is in the upper ROM,
//! where the disc ROM can lie as well: the stop point is an address in
//! the ROM space and hits only while BASIC is the ROM switched in.
//! Program and variables lie under the lower ROM and are read from RAM.
//! A statement behind THEN or ELSE runs without passing the loop.
//!
//! **Stack.** BASIC's own, growing upwards from a 0 byte; every entry
//! ends with its length. GOSUB (6): a kind (0, or 1 and 2 for a
//! subroutine called by an event), the address of the statement's end
//! and of its line. WHILE (7): the line, the address behind WEND, the
//! address of the condition. FOR (16 with an integer variable, 22 with a
//! real one): the address of the variable's value, limit and step, the
//! step's sign, the address of the statement's end, of its line, of
//! NEXT's line and of NEXT's end.
//!
//! **Variables.** Two areas behind the program, each entry with a link,
//! the name in upper case (the last character with bit 7 set) and a type
//! that is the value's length less one: an integer (2 bytes), a string
//! (its length and address) or a real number (5 bytes: 31 bits of
//! mantissa, low byte first, the sign in place of the leading 1, and the
//! exponent plus 128; exponent 0 is zero). A function of DEF FN is an
//! entry too, its type with bit 6 set, and is not listed. A string
//! assigned from a constant points into the program's text; the debugger
//! does not write it there. An array has its length, the
//! number of dimensions and their sizes, the last one first, before its
//! elements, of which the first index runs fastest.
//!
//! Sources: Amstrad, *CPC464 Firmware* (Soft 158) and *CPC6128 User
//! Instructions* (Soft 968), chapter 7, for the tokens and the number
//! format; the system variables (`&AE1B`, `&AE1D`, `&AE64`–`&AE6C`,
//! `&B06F` in 1.1; `&AE34`, `&AE36`, `&AE81`–`&AE89`, `&B08B` in 1.0) were
//! found in the statement loop (`&DE60`), GOSUB and RETURN (`&C78C`), FOR
//! (`&C5D4`), WHILE (`&C7E7`), the prompt (`&C058`) and the listing
//! (`&E1E3`, keyword search `&E3B8`) of the user's ROMs, read with this
//! project's Z80 disassembler, and checked against the memory of a
//! running program. No emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 10_000;
const MAX_VARS: usize = 4_000;
const MAX_FRAMES: usize = 256;

/// BASIC in the ROM space and where the Z80 sees it.
const ROM_AT: u32 = 0x4000;
const ROM_LEN: usize = 0x4000;
const ROM_BASE: usize = 0xc000;

const T_COLON: u8 = 0x01;
const T_ELSE: u8 = 0x97;
const T_TICK: u8 = 0xc0;
const T_REM: u8 = 0xc5;
const T_FN: u8 = 0xe4;
const T_FUNCTION: u8 = 0xff;

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point as the Z80 sees it.
    stop: u16,
    /// The address of the byte before the current statement.
    statement: u16,
    /// The address of the current line's number; 0 outside a program.
    line: u16,
    /// The top of BASIC's stack.
    stack: u16,
    /// The pointers to the program's end, the variables, the arrays and
    /// the free space; the one to the program's start is before them.
    areas: u16,
    /// Keywords by token, and whether they are words of letters.
    words: Vec<(u8, Vec<u8>, bool)>,
    /// The ROM's version: 0 on the 464.
    version: u8,
}

/// The decoder a CPC keeps; see the module text.
#[derive(Clone, Debug)]
pub struct Locomotive {
    /// The RAM under the ROMs.
    space: MemSpaceId,
    rom_space: MemSpaceId,
    scanned: bool,
    rom: Option<Rom>,
}

fn find(mem: &[u8], pattern: &[Option<u8>]) -> Option<usize> {
    mem.windows(pattern.len()).position(|w| {
        w.iter()
            .zip(pattern)
            .all(|(b, p)| p.is_none_or(|p| p == *b))
    })
}

fn pattern(bytes: &[i16]) -> Vec<Option<u8>> {
    bytes.iter().map(|&b| u8::try_from(b).ok()).collect()
}

fn scan(mem: &[u8]) -> Option<Rom> {
    let word = |at: usize| u16::from_le_bytes([mem[at], mem[at + 1]]);
    // LD (statement),HL : CALL poll : CALL C : CALL next token : CALL NZ
    // statement : LD A,(HL) : CP 1 : JR Z.
    let head = find(
        mem,
        &pattern(&[
            0x22, -1, -1, 0xcd, -1, -1, 0xdc, -1, -1, 0xcd, -1, -1, 0xc4, -1, -1, 0x7e, 0xfe, 0x01,
            0x28,
        ]),
    )?;
    // JR Z : LD (line),HL : INC HL : LD A,(trace) : OR A : JR Z.
    let line = find(
        mem,
        &pattern(&[0x28, -1, 0x22, -1, -1, 0x23, 0x3a, -1, -1, 0xb7, 0x28]),
    )?;
    // RETURN's search: LD HL,(stack) : DEC HL : LD A,(HL) : PUSH AF :
    // LD A,L : SUB (HL) : LD L,A.
    let stack = find(
        mem,
        &pattern(&[0x2a, -1, -1, 0x2b, 0x7e, 0xf5, 0x7d, 0x96, 0x6f]),
    )?;
    // LD HL,(program's end) and three stores to the pointers behind it.
    let areas = (0..mem.len().saturating_sub(12)).find(|&at| {
        let op = |i: usize, code: u8, n: u16| mem[at + i] == code && word(at + i + 1) == n;
        let first = word(at + 1);
        mem[at] == 0x2a && (1..4).all(|i| op(3 * i, 0x22, first.wrapping_add(2 * i as u16)))
    })?;
    // ZONE begins the table: 26 lists of words without their first
    // letter, from Z down to A, then the symbols.
    let mut at = find(mem, &pattern(&[0x4f, 0x4e, 0xc5, 0xda]))?;
    let mut words: Vec<(u8, Vec<u8>, bool)> = Vec::new();
    for list in 0..27u8 {
        while *mem.get(at)? != 0 {
            let mut text: Vec<u8> = (list < 26).then(|| b'Z' - list).into_iter().collect();
            loop {
                let b = *mem.get(at)?;
                at += 1;
                // A tab stands for blanks the tokeniser allows.
                if b & 0x7f != 0x09 {
                    text.push(b & 0x7f);
                }
                if b & 0x80 != 0 {
                    break;
                }
            }
            let token = *mem.get(at)?;
            at += 1;
            if !words.iter().any(|w| w.0 == token) {
                words.push((token, text, list < 26));
            }
        }
        at += 1;
    }
    Some(Rom {
        stop: (ROM_BASE + head + 3) as u16,
        statement: word(head + 1),
        line: word(line + 3),
        stack: word(stack + 1),
        areas: word(areas + 1),
        words,
        version: mem[2],
    })
}

/// What LIST's blank rule counts as part of a word: letters, digits and
/// the point.
fn wordy(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.'
}

fn push_char(out: &mut String, b: u8) {
    match b {
        0x20..=0x7e => out.push(char::from(b)),
        _ => out.push_str(&format!("{{${b:02x}}}")),
    }
}

fn real(b: [u8; 5]) -> f64 {
    if b[4] == 0 {
        return 0.0;
    }
    let mantissa = u32::from_le_bytes([b[0], b[1], b[2], b[3] | 0x80]);
    let v = f64::from(mantissa) / 4_294_967_296.0 * 2f64.powi(i32::from(b[4]) - 128);
    if b[3] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn real_bytes(v: f64) -> Result<[u8; 5], BasicError> {
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
    let m = (mantissa as u32).to_le_bytes();
    let sign = if v < 0.0 { 0x80 } else { 0 };
    Ok([m[0], m[1], m[2], m[3] & 0x7f | sign, (exp + 128) as u8])
}

/// A real number as the ROM writes it: nine digits, with an exponent
/// when it does not fit them.
fn real_text(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let sci = format!("{:.8e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let sign = if v < 0.0 { "-" } else { "" };
    let len = digits.len() as i32;
    if (0..9).contains(&exp) {
        let whole = (exp + 1).min(len) as usize;
        let zeros = "0".repeat((exp + 1 - len).max(0) as usize);
        let part = &digits[whole..];
        let point = if part.is_empty() { "" } else { "." };
        return format!("{sign}{}{zeros}{point}{part}", &digits[..whole]);
    }
    if exp < 0 && len - exp - 1 <= 9 {
        return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
    }
    let point = if len > 1 { "." } else { "" };
    format!(
        "{sign}{}{point}{}E{}{:02}",
        &digits[..1],
        &digits[1..],
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    len: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Str,
    Real,
}

impl Ty {
    fn of(code: u8) -> Option<Self> {
        match code {
            1 => Some(Ty::Int),
            2 => Some(Ty::Str),
            4 => Some(Ty::Real),
            _ => None,
        }
    }

    fn size(self) -> u16 {
        match self {
            Ty::Int => 2,
            Ty::Str => 3,
            Ty::Real => 5,
        }
    }

    fn basic(self) -> BasicType {
        match self {
            Ty::Int => BasicType::Int,
            Ty::Str => BasicType::Str,
            Ty::Real => BasicType::Float,
        }
    }
}

struct Var {
    /// In upper case, with `%` or `$`.
    name: String,
    ty: Ty,
    addr: u16,
    /// The value or the first element.
    data: u16,
    /// As DIM gives them; empty for a plain variable.
    dims: Vec<u32>,
    array: bool,
}

impl Var {
    fn len(&self) -> u32 {
        self.dims.iter().fold(1u32, |n, d| n.saturating_mul(*d))
    }
}

/// A line's text and where its statements begin, in the text and in the
/// line's bytes (the offset of the byte before each).
struct Listed {
    text: String,
    statements: Vec<u32>,
    marks: Vec<u16>,
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    rom: &'a Rom,
}

impl View<'_> {
    fn b(&self, addr: u16) -> u8 {
        self.host
            .peek(self.space, u32::from(addr), Size::Byte)
            .map_or(0xff, |v| v as u8)
    }

    fn w(&self, addr: u16) -> u16 {
        u16::from_le_bytes([self.b(addr), self.b(addr.wrapping_add(1))])
    }

    fn five(&self, addr: u16) -> [u8; 5] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    /// The pointers behind the program: 0 its end, 1 the variables, 2 the
    /// arrays, 3 the free space; −1 the byte before the program.
    fn area(&self, n: i16) -> u16 {
        self.w(self.rom.areas.wrapping_add_signed(2 * n))
    }

    fn lines(&self) -> Vec<Line> {
        let end = u32::from(self.area(0));
        let mut out = Vec::new();
        let mut at = u32::from(self.area(-1)) + 1;
        while at + 4 < end && out.len() < MAX_LINES {
            let addr = at as u16;
            let len = self.w(addr);
            if len < 5 || at + u32::from(len) > end {
                break;
            }
            out.push(Line {
                addr,
                number: self.w(addr.wrapping_add(2)),
                len,
            });
            at += u32::from(len);
        }
        out
    }

    fn word(&self, token: u8, function: bool) -> Option<&(u8, Vec<u8>, bool)> {
        self.rom
            .words
            .iter()
            .find(|w| w.0 == token && (w.0 < 0x80) == function)
    }

    /// LIST (`&E25D`): `spaced` is its flag that the last thing written
    /// was a word, a name without a sign or a number.
    fn listed(&self, line: &Line) -> Listed {
        let body: Vec<u8> = (4..line.len.saturating_sub(1))
            .map(|i| self.b(line.addr.wrapping_add(i)))
            .collect();
        let mut out = String::new();
        let mut statements = vec![0u32];
        let mut marks = vec![3u16];
        let mut spaced = false;
        let mut i = 0;
        let blank = |out: &mut String, spaced: bool| {
            if spaced {
                out.push(' ');
            }
        };
        while i < body.len() {
            let t = body[i];
            let mut keyword = None;
            match t {
                T_COLON => {
                    marks.push(4 + i as u16);
                    match body.get(i + 1) {
                        Some(&next @ (T_ELSE | T_TICK)) => {
                            i += 1;
                            statements.push(out.len() as u32);
                            keyword = Some(next);
                        }
                        _ => {
                            out.push(':');
                            spaced = false;
                            i += 1;
                            statements.push(out.len() as u32);
                        }
                    }
                }
                0x02..=0x0d => {
                    blank(&mut out, spaced);
                    i += 3;
                    while let Some(&c) = body.get(i) {
                        i += 1;
                        push_char(&mut out, c & 0x7f);
                        if c & 0x80 != 0 {
                            break;
                        }
                    }
                    spaced = t >= 0x0b;
                    match t {
                        0x02 => out.push('%'),
                        0x03 => out.push('$'),
                        0x04 => out.push('!'),
                        _ => {}
                    }
                }
                0x0e..=0x1f => {
                    blank(&mut out, spaced);
                    i += 1;
                    let arg = |n: usize| body.get(i + n).copied().unwrap_or(0);
                    let word = u16::from_le_bytes([arg(0), arg(1)]);
                    let (text, len) = match t {
                        0x19 => (arg(0).to_string(), 1),
                        0x1a => ((word as i16).to_string(), 2),
                        0x1b => (format!("&X{word:b}"), 2),
                        0x1c => (format!("&{word:X}"), 2),
                        0x1d => (self.w(word.wrapping_add(3)).to_string(), 2),
                        0x1e => (word.to_string(), 2),
                        0x1f => (real_text(real(std::array::from_fn(arg))), 5),
                        _ => ((t - 0x0e).to_string(), 0),
                    };
                    out.push_str(&text);
                    i += len;
                    spaced = true;
                }
                b'"' => {
                    out.push('"');
                    i += 1;
                    while let Some(&c) = body.get(i) {
                        i += 1;
                        push_char(&mut out, c);
                        if c == b'"' {
                            break;
                        }
                    }
                    spaced = false;
                }
                // An RSX: the bar, a 0, the name.
                b'|' => {
                    out.push('|');
                    i += 2;
                    if body.get(i - 1) == Some(&0) {
                        while let Some(&c) = body.get(i) {
                            i += 1;
                            push_char(&mut out, c & 0x7f);
                            if c & 0x80 != 0 {
                                break;
                            }
                        }
                    }
                    spaced = true;
                }
                0x80..=0xff => keyword = Some(t),
                _ => {
                    blank(
                        &mut out,
                        spaced && (wordy(t) || matches!(t, b'!' | b'$' | b'%')),
                    );
                    push_char(&mut out, t);
                    spaced = false;
                    i += 1;
                }
            }
            let Some(token) = keyword else {
                continue;
            };
            i += 1;
            let (token, function) = match token {
                T_FUNCTION => {
                    i += 1;
                    (body.get(i - 1).copied().unwrap_or(0), true)
                }
                _ => (token, false),
            };
            match self.word(token, function) {
                Some((_, text, letters)) => {
                    blank(&mut out, spaced && *letters);
                    text.iter().for_each(|&c| push_char(&mut out, c));
                    spaced = text.last().is_some_and(|&c| wordy(c)) && token != T_FN;
                }
                None => {
                    out.push_str(&format!("{{${token:02x}}}"));
                    spaced = false;
                }
            }
            // A comment keeps the line's rest as it is.
            if !function && matches!(token, T_REM | T_TICK) {
                body[i..].iter().for_each(|&c| push_char(&mut out, c));
                break;
            }
        }
        Listed {
            text: out,
            statements,
            marks,
        }
    }

    /// The place of the statement that the byte at `at` belongs to, in the
    /// line whose number is stored at `number_at`; `end`: `at` is the
    /// statement's end, not its beginning.
    fn pos(&self, number_at: u16, at: u16, end: bool) -> BasicPos {
        let addr = number_at.wrapping_sub(2);
        let line = Line {
            addr,
            number: self.w(number_at),
            len: self.w(addr),
        };
        let offset = at.wrapping_sub(addr);
        let before = self
            .listed(&line)
            .marks
            .iter()
            .filter(|m| **m < offset || (!end && **m == offset))
            .count();
        BasicPos {
            line: u32::from(line.number),
            statement: before.saturating_sub(1) as u16,
        }
    }

    fn state(&self) -> BasicState {
        let number_at = self.w(self.rom.line);
        let at = self.w(self.rom.statement);
        let pos = (number_at != 0).then(|| self.pos(number_at, at, false));
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(at)),
            depth: self
                .frames()
                .iter()
                .filter(|f| matches!(f, BasicFrame::Gosub { .. }))
                .count() as u32,
        }
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let vars = self.vars();
        let mut out = Vec::new();
        let mut top = self.w(self.rom.stack);
        // A subroutine called from a command typed in has no line.
        let pos = |number_at: u16, at: u16| match number_at {
            0 => BasicPos {
                line: 0,
                statement: 0,
            },
            _ => self.pos(number_at, at, true),
        };
        while out.len() < MAX_FRAMES {
            let len = u16::from(self.b(top.wrapping_sub(1)));
            let e = top.wrapping_sub(len);
            match len {
                6 => out.push(BasicFrame::Gosub {
                    from: pos(self.w(e.wrapping_add(3)), self.w(e.wrapping_add(1))),
                }),
                7 => out.push(BasicFrame::Loop {
                    word: "WHILE".into(),
                    from: pos(self.w(e), self.w(e.wrapping_add(4))),
                }),
                16 | 22 => {
                    let ty = if len == 16 { Ty::Int } else { Ty::Real };
                    let n = ty.size();
                    let data = self.w(e);
                    let var = vars.iter().find(|v| !v.array && v.data == data);
                    out.push(BasicFrame::For {
                        var: var.map_or_else(String::new, |v| v.name.clone()),
                        limit: self.value(ty, e.wrapping_add(2)),
                        step: self.value(ty, e.wrapping_add(2 + n)),
                        from: pos(
                            self.w(e.wrapping_add(5 + 2 * n)),
                            self.w(e.wrapping_add(3 + 2 * n)),
                        ),
                    });
                }
                _ => break,
            }
            top = e;
        }
        out
    }

    fn value(&self, ty: Ty, at: u16) -> BasicValue {
        match ty {
            Ty::Int => BasicValue::Int(i64::from(self.w(at) as i16)),
            Ty::Real => BasicValue::Float(real(self.five(at))),
            Ty::Str => {
                let text = self.w(at.wrapping_add(1));
                let mut s = String::new();
                for i in 0..u16::from(self.b(at)) {
                    push_char(&mut s, self.b(text.wrapping_add(i)));
                }
                BasicValue::Str(s)
            }
        }
    }

    /// The entries of both areas: link, name, type, and for an array its
    /// length, the number of dimensions and their sizes.
    fn vars(&self) -> Vec<Var> {
        let mut out = Vec::new();
        for array in [false, true] {
            let n = i16::from(array);
            let end = u32::from(self.area(2 + n));
            let mut at = u32::from(self.area(1 + n));
            while at + 4 < end && out.len() < MAX_VARS {
                let addr = at as u16;
                let mut p = addr.wrapping_add(2);
                let mut name = String::new();
                loop {
                    let c = self.b(p);
                    p = p.wrapping_add(1);
                    name.push(char::from(c & 0x7f));
                    if c & 0x80 != 0 || name.len() > 40 {
                        break;
                    }
                }
                let code = self.b(p);
                p = p.wrapping_add(1);
                // A function of DEF FN: its type with bit 6 set and the
                // address of its definition.
                if code & 0x40 != 0 && !array {
                    at = u32::from(p) + 2;
                    continue;
                }
                let Some(ty) = Ty::of(code) else {
                    break;
                };
                match ty {
                    Ty::Int => name.push('%'),
                    Ty::Str => name.push('$'),
                    Ty::Real => {}
                }
                let (data, dims, next) = if array {
                    let len = self.w(p);
                    let count = u16::from(self.b(p.wrapping_add(2)));
                    let mut dims: Vec<u32> = (0..count)
                        .map(|i| u32::from(self.w(p.wrapping_add(3 + 2 * i))))
                        .collect();
                    dims.reverse();
                    (
                        p.wrapping_add(3 + 2 * count),
                        dims,
                        u32::from(p) + 2 + u32::from(len),
                    )
                } else {
                    (p, Vec::new(), u32::from(p) + u32::from(ty.size()))
                };
                out.push(Var {
                    name,
                    ty,
                    addr,
                    data,
                    dims,
                    array,
                });
                if next <= at {
                    break;
                }
                at = next;
            }
        }
        out
    }
}

/// A name as the debugger's user writes it: without blanks, in upper
/// case, a real variable without its `!`.
fn wanted(name: &str) -> String {
    let plain: String = name
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    plain
        .strip_suffix('!')
        .map_or(plain.clone(), str::to_string)
}

impl Locomotive {
    /// `space` is the RAM as the Z80 addresses it, `rom_space` the ROMs of
    /// 16 KB one behind the other with BASIC as the second; execution
    /// breakpoints in it hit only while that ROM is switched in.
    pub fn new(space: MemSpaceId, rom_space: MemSpaceId) -> Self {
        Self {
            space,
            rom_space,
            scanned: false,
            rom: None,
        }
    }

    /// Look at the machine's ROM, once: `Some` with the decoder to keep.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.scanned {
            return None;
        }
        let mem: Vec<u8> = (0..ROM_LEN as u32)
            .map_while(|a| host.peek(self.rom_space, ROM_AT + a, Size::Byte))
            .map(|v| v as u8)
            .collect();
        Some(Self {
            scanned: true,
            rom: (mem.len() == ROM_LEN).then(|| scan(&mem)).flatten(),
            ..self.clone()
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        self.rom.as_ref().map(|rom| View {
            host,
            space: self.space,
            rom,
        })
    }
}

impl Dialect for Locomotive {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        let dialect = match &self.rom {
            Some(Rom { version: 0, .. }) => "Locomotive BASIC 1.0",
            _ => "Locomotive BASIC 1.1",
        };
        BasicInfo {
            dialect: dialect.into(),
            granularity: BasicGranularity::Statement,
            space: self.space,
            name_len: 0,
        }
    }

    fn state(&self, host: &dyn DebugTarget) -> BasicState {
        match self.view(host) {
            Some(v) => v.state(),
            None => BasicState {
                mode: BasicMode::Inactive,
                pos: None,
                text_addr: None,
                depth: 0,
            },
        }
    }

    fn program(&self, host: &dyn DebugTarget) -> Vec<BasicLine> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.lines()
            .iter()
            .map(|l| {
                let listed = v.listed(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (0..l.len).map(|i| v.b(l.addr.wrapping_add(i))).collect(),
                    text: listed.text,
                    statements: listed.statements,
                }
            })
            .collect()
    }

    fn variables(&self, host: &dyn DebugTarget) -> Vec<BasicVar> {
        let Some(v) = self.view(host) else {
            return Vec::new();
        };
        v.vars()
            .iter()
            .map(|var| BasicVar {
                name: var.name.clone(),
                ty: var.ty.basic(),
                addr: u32::from(var.addr),
                value: (!var.array).then(|| v.value(var.ty, var.data)),
                dims: var.dims.clone(),
            })
            .collect()
    }

    fn array(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        first: u32,
        count: u32,
    ) -> Result<Vec<BasicValue>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let name = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| x.array && x.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let len = var.len();
        if first > len {
            return Err(BasicError::OutOfRange);
        }
        Ok((first..len.min(first.saturating_add(count)))
            .map(|i| {
                v.value(
                    var.ty,
                    var.data.wrapping_add((u32::from(var.ty.size()) * i) as u16),
                )
            })
            .collect())
    }

    fn stack(&self, host: &dyn DebugTarget) -> Vec<BasicFrame> {
        self.view(host).map_or_else(Vec::new, |v| v.frames())
    }

    fn plan_set(
        &self,
        host: &dyn DebugTarget,
        name: &str,
        index: Option<u32>,
        value: &BasicValue,
    ) -> Result<Vec<(u32, u8)>, BasicError> {
        let v = self.view(host).ok_or(BasicError::Inactive)?;
        let name = wanted(name);
        let vars = v.vars();
        let var = vars
            .iter()
            .find(|x| x.name == name && x.array == index.is_some())
            .ok_or(BasicError::NoSuchVariable)?;
        let index = index.unwrap_or(0);
        if var.array && index >= var.len() {
            return Err(BasicError::OutOfRange);
        }
        let at = var
            .data
            .wrapping_add((u32::from(var.ty.size()) * index) as u16);
        let number = match value {
            BasicValue::Int(n) => Some(*n as f64),
            BasicValue::Float(f) => Some(*f),
            BasicValue::Str(_) => None,
        };
        let (at, bytes): (u16, Vec<u8>) = match (var.ty, number, value) {
            (Ty::Int, Some(n), _) => {
                if n.fract() != 0.0 || !(-32768.0..=32767.0).contains(&n) {
                    return Err(BasicError::OutOfRange);
                }
                (at, (n as i16).to_le_bytes().to_vec())
            }
            (Ty::Real, Some(n), _) => (at, real_bytes(n)?.to_vec()),
            (Ty::Str, None, BasicValue::Str(s)) => {
                let text = s
                    .chars()
                    .map(|c| u8::try_from(c).ok().filter(|b| (0x20..0x7f).contains(b)))
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // Only in place: the same length, and not where the
                // string is still the constant in the program's text.
                let data = v.w(at.wrapping_add(1));
                if text.len() != usize::from(v.b(at)) || (v.area(-1)..v.area(0)).contains(&data) {
                    return Err(BasicError::Unsupported);
                }
                (data, text)
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(at) + i as u32, b))
            .collect())
    }

    fn stop_points(&self, _host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        self.rom
            .iter()
            .map(|r| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.rom_space,
                    addr: ROM_AT + u32::from(r.stop) - ROM_BASE as u32,
                },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
