//! Atari BASIC (the 8 KB ROM of the 400/800 cartridge and of the XL/XE
//! machines, revisions A to C).
//!
//! **Program.** From STMTAB (`$88`) on. A line is its number (low byte
//! first), its length, and its statements; a statement is the offset of
//! the next one from the line's start, the statement's token and its
//! operands. The line typed in last is stored behind the program as line
//! 32768 and run from there. Everything is tokenised when the line is
//! entered: a variable is `$80` plus its number, a number `$0E` and six
//! bytes, a string `$0F`, its length and its text, an operator or a
//! function a token from `$10` on; REM and DATA keep their text. What
//! follows THEN is a statement of its own, as on the Spectrum. The listing
//! writes a line as the ROM's LIST does (`$B58E` in revision C): a blank
//! behind every statement name, blanks around the operators that are
//! words, numbers as the floating point package writes them. Both name
//! tables are the ROM's own.
//!
//! **Where the interpreter is.** STMCUR (`$8A`) points to the line being
//! run, NXTSTD (`$A7`) holds the offset of the statement after the current
//! one, STINDEX (`$A8`) the token being looked at. The stop point is the
//! call of the statement's routine in the statement loop, behind the store
//! of both. The editor sets the stack pointer to `$FF` and so does every
//! entry into the loop, so the first address on the 6502's stack is one
//! in the loop while a program runs: the return address of the
//! statement's call, between two statements the one of the loop's test
//! for BREAK or of its step to the next line, or the place where an
//! interrupt met the loop. In the editor another one is there. That and a
//! line number below 32768 tell a running program.
//!
//! **Stack.** The run time stack lies between RUNSTK (`$8E`) and its top
//! (`$90`). GOSUB pushes four bytes: 0, the line number and the offset of
//! its statement's token in the line. FOR pushes the limit and the step,
//! six bytes each, and then the same four bytes with the variable's token
//! in the first.
//!
//! **Variables.** The names are in a table from VNTP (`$82`), the last
//! character of each with bit 7 set, `$` and `(` being part of the name.
//! The values are in a table from VVTP (`$86`), eight bytes each: the kind
//! (`$00` a number, `$40` an array, `$80` a string, bit 0 set once it is
//! dimensioned), the variable's number, and six bytes: the number; or the
//! array's offset from STARP (`$8C`) and its two sizes; or the string's
//! offset, its length and its dimensioned length. A number is six bytes:
//! the sign and an exponent of 100 with 64 added, then ten decimal digits,
//! the point behind the first two. All numbers are such; a string can be
//! given any length up to the dimensioned one.
//!
//! Sources: Bill Wilkinson, Kathleen O'Brien and Paul Laughton, *The Atari
//! BASIC Source Book* (COMPUTE! Books, 1983) for the names of the zero
//! page pointers, the token format and the tables; Ian Chadwick, *Mapping
//! the Atari* (COMPUTE! Books, 1985) for the zero page; *De Re Atari*
//! (Atari, 1981), chapter 10, for the variable tables. The statement loop
//! (`$A961`), FOR (`$B67D`), GOSUB (`$B6D2`), RETURN (`$BDA8`), the stack
//! routines (`$B83E`, `$B871`), the editor's entry (`$A060`) and LIST's
//! line printer (`$B58E`) were read from the user's ROM (revision C) with
//! this project's disassembler. No emulator source was used.

use emu_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use emu_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 10_000;
const MAX_FRAMES: usize = 256;

const VNTP: u16 = 0x82;
const VNTD: u16 = 0x84;
const VVTP: u16 = 0x86;
const STMTAB: u16 = 0x88;
const STMCUR: u16 = 0x8a;
const STARP: u16 = 0x8c;
const RUNSTK: u16 = 0x8e;
const STACK_TOP: u16 = 0x90;
const NXTSTD: u16 = 0xa7;
const STINDEX: u16 = 0xa8;
/// The first return address on the 6502's stack.
const CPU_STACK: u16 = 0x01fe;

/// Where the cartridge and the built-in ROM lie.
const ROM_START: u16 = 0xa000;
const ROM_LEN: usize = 0x2000;

/// The statement loop with its step to the next line, from its first
/// instruction (`$A95E`) to its last (`$A99B`), as distances from the
/// stop point (`$A977`).
const LOOP_BEFORE: u16 = 25;
const LOOP_AFTER: u16 = 36;

const T_REM: u8 = 0x00;
const T_DATA: u8 = 0x01;
/// LET without the word.
const T_IMPLIED_LET: u8 = 0x36;
/// A line with a syntax error, kept as text.
const T_ERROR: u8 = 0x37;
const T_NUMBER: u8 = 0x0e;
const T_STRING: u8 = 0x0f;
const T_EOL: u8 = 0x16;
/// The first function; operators before it that are words get blanks.
const T_FUNCTIONS: u8 = 0x3d;

/// LDA (STMCUR),Y : STA NXTSTD : TYA : INY : LDA (STMCUR),Y : INY :
/// STY STINDEX : JSR.
const LOOP: [u8; 12] = [
    0xb1, 0x8a, 0x85, 0xa7, 0x98, 0xc8, 0xb1, 0x8a, 0xc8, 0x84, 0xa8, 0x20,
];

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The statement loop's call of the statement.
    stop: u16,
    /// The statements' names by token.
    statements: Vec<Vec<u8>>,
    /// The operators' and functions' names from token `$10`.
    operators: Vec<Vec<u8>>,
}

/// The decoder an Atari keeps; see the module text.
#[derive(Clone, Debug)]
pub struct AtariBasic {
    space: MemSpaceId,
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

/// Names of a table: `skip` bytes, then characters up to one with bit 7.
fn names(mem: &[u8], mut at: usize, skip: usize, count: usize) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while out.len() < count {
        at += skip;
        let mut word = Vec::new();
        loop {
            let Some(&b) = mem.get(at) else {
                return out;
            };
            if b == 0 && word.is_empty() {
                return out;
            }
            at += 1;
            word.push(b & 0x7f);
            if b & 0x80 != 0 || word.len() > 16 {
                break;
            }
        }
        out.push(word);
    }
    out
}

fn scan(mem: &[u8]) -> Option<Rom> {
    let table = |at: usize| {
        let addr = usize::from(u16::from_be_bytes([mem[at], mem[at + 2]]));
        addr.checked_sub(usize::from(ROM_START))
    };
    // JSR break test : BEQ : LDY NXTSTD : CPY LLNGTH : BCS, then the loop.
    let mut head = pattern(&[0x20, -1, -1, 0xf0, -1, 0xa4, 0xa7, 0xc4, 0x9f, 0xb0, -1]);
    head.extend(LOOP.map(Some));
    let stop = find(mem, &head)? + head.len() - 1;
    // LIST: STA $AF : LDX #2 : LDA #>table : LDY #<table : JSR.
    let statements = find(
        mem,
        &pattern(&[0x85, 0xaf, 0xa2, 0x02, 0xa9, -1, 0xa0, -1, 0x20]),
    )
    .and_then(|at| table(at + 5))?;
    // SEC : SBC #$10 : STA $AF : LDX #0 : LDA #>table : LDY #<table.
    let operators = find(
        mem,
        &pattern(&[0x38, 0xe9, 0x10, 0x85, 0xaf, 0xa2, 0x00, 0xa9, -1, 0xa0, -1]),
    )
    .and_then(|at| table(at + 8))?;
    Some(Rom {
        stop: ROM_START + stop as u16,
        statements: names(mem, statements, 2, 0x38),
        operators: names(mem, operators, 0, 0x70),
    })
}

/// ATASCII to Unicode; `None` for the graphics and control characters.
fn atascii_char(b: u8) -> Option<char> {
    match b {
        0x20..=0x5f | 0x61..=0x7a | 0x7c => Some(char::from(b)),
        _ => None,
    }
}

fn atascii_code(c: char) -> Option<u8> {
    u8::try_from(c).ok().filter(|b| atascii_char(*b).is_some())
}

fn push_char(out: &mut String, b: u8) {
    match atascii_char(b) {
        Some(c) => out.push(c),
        None => out.push_str(&format!("{{${b:02x}}}")),
    }
}

/// The ten digits of a number and its exponent of 100.
fn digits(b: [u8; 6]) -> (String, i32) {
    let d: String = b[1..]
        .iter()
        .map(|x| format!("{:x}{:x}", x >> 4, x & 15))
        .collect();
    (d, i32::from(b[0] & 0x7f) - 64)
}

fn number(b: [u8; 6]) -> f64 {
    let (d, e) = digits(b);
    if !d.bytes().all(|c| c.is_ascii_digit()) {
        return 0.0;
    }
    let v: f64 = format!("{d}e{}", 2 * e - 8).parse().unwrap_or(0.0);
    if b[0] & 0x80 != 0 {
        -v
    } else {
        v
    }
}

fn number_bytes(v: f64) -> Result<[u8; 6], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 6]);
    }
    // Ten digits when the first pair has two, nine when it has one.
    let split = |text: String| -> (String, i32) {
        let (m, e) = text.split_once('e').unwrap_or((&text, "0"));
        (m.replace('.', ""), e.parse().unwrap_or(0))
    };
    let (mut d, mut e) = split(format!("{:.9e}", v.abs()));
    if e.rem_euclid(2) == 0 {
        (d, e) = split(format!("{:.8e}", v.abs()));
        if e.rem_euclid(2) == 0 {
            d.insert(0, '0');
        } else {
            // Rounded up into the next power of ten.
            d.push('0');
        }
    }
    let e100 = e.div_euclid(2);
    if e100 < -64 {
        return Ok([0; 6]);
    }
    if e100 > 48 {
        return Err(BasicError::OutOfRange);
    }
    let d = d.as_bytes();
    let mut out = [0u8; 6];
    out[0] = (e100 + 64) as u8 | if v < 0.0 { 0x80 } else { 0 };
    for (i, o) in out[1..].iter_mut().enumerate() {
        *o = (d[2 * i] - b'0') << 4 | (d[2 * i + 1] - b'0');
    }
    Ok(out)
}

/// A number as the floating point package's FASC writes it: plain from
/// 0.01 to below 1E10, else with an exponent of two digits (compared with
/// what LIST puts on the screen).
fn fasc(b: [u8; 6]) -> String {
    let (d, e) = digits(b);
    if d.bytes().all(|c| c == b'0') {
        return "0".into();
    }
    let sign = if b[0] & 0x80 != 0 { "-" } else { "" };
    if (-1..=4).contains(&e) {
        let (whole, part) = d.split_at(2 * (e + 1) as usize);
        let whole = whole.trim_start_matches('0');
        let part = part.trim_end_matches('0');
        let whole = if whole.is_empty() { "0" } else { whole };
        return if part.is_empty() {
            format!("{sign}{whole}")
        } else {
            format!("{sign}{whole}.{part}")
        };
    }
    // The point goes behind the first digit; the zeros at the end go, but
    // not those of the first pair: 1E+10, but 1.0E+11.
    let (first, rest) = d.split_at(2);
    let rest = rest.trim_end_matches('0');
    let (first, e10) = match first.strip_prefix('0') {
        Some(digit) => (digit, 2 * e),
        None => (first, 2 * e + 1),
    };
    let mantissa = format!("{}.{}{rest}", &first[..1], &first[1..]);
    format!(
        "{sign}{}E{}{:02}",
        mantissa.trim_end_matches('.'),
        if e10 < 0 { '-' } else { '+' },
        e10.abs()
    )
}

#[derive(Clone, Copy)]
struct Line {
    addr: u16,
    number: u16,
    len: u8,
}

/// A statement's place in its line: the offset of its first byte (the
/// next one's offset) and the next one's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stmt {
    start: u8,
    next: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Num,
    Str,
    Array,
}

struct Var {
    /// Without the array's parenthesis.
    name: String,
    kind: Kind,
    /// The entry in the value table.
    addr: u16,
    dimensioned: bool,
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

    fn six(&self, addr: u16) -> [u8; 6] {
        std::array::from_fn(|i| self.b(addr.wrapping_add(i as u16)))
    }

    fn line_at(&self, addr: u16) -> Line {
        Line {
            addr,
            number: self.w(addr),
            len: self.b(addr.wrapping_add(2)),
        }
    }

    fn lines(&self) -> Vec<Line> {
        let end = u32::from(self.w(STARP));
        let mut out = Vec::new();
        let mut at = u32::from(self.w(STMTAB));
        while at + 4 <= end && out.len() < MAX_LINES {
            let line = self.line_at(at as u16);
            if line.number >= 0x8000 || line.len < 5 {
                break;
            }
            out.push(line);
            at += u32::from(line.len);
        }
        out
    }

    fn stmts(&self, line: &Line) -> Vec<Stmt> {
        let mut out = Vec::new();
        let mut start = 3u8;
        while start < line.len {
            let next = self.b(line.addr.wrapping_add(u16::from(start)));
            if next < start.saturating_add(2) || next > line.len {
                break;
            }
            out.push(Stmt { start, next });
            start = next;
        }
        out
    }

    /// The names of the variables as stored, `$` and `(` included.
    fn names(&self) -> Vec<Vec<u8>> {
        let end = self.w(VNTD);
        let mut out = Vec::new();
        let mut at = self.w(VNTP);
        let mut word = Vec::new();
        while at < end && out.len() < 128 {
            let b = self.b(at);
            at = at.wrapping_add(1);
            if b == 0 && word.is_empty() {
                break;
            }
            word.push(b & 0x7f);
            if b & 0x80 != 0 {
                out.push(std::mem::take(&mut word));
            }
        }
        out
    }

    /// A line's text as LIST writes it and where its statements begin.
    fn text(&self, line: &Line, names: &[Vec<u8>]) -> (String, Vec<u32>) {
        let at = |i: u8| self.b(line.addr.wrapping_add(u16::from(i)));
        let word = |out: &mut String, w: &[u8]| w.iter().for_each(|&c| push_char(out, c));
        let mut out = String::new();
        let mut statements = Vec::new();
        for s in self.stmts(line) {
            statements.push(out.len() as u32);
            let token = at(s.start + 1);
            if token != T_IMPLIED_LET {
                match self.rom.statements.get(usize::from(token)) {
                    Some(name) => word(&mut out, name),
                    None => out.push_str(&format!("{{${token:02x}}}")),
                }
                out.push(' ');
            }
            let mut i = s.start + 2;
            if matches!(token, T_REM | T_DATA | T_ERROR) {
                while i < s.next && at(i) != 0x9b {
                    push_char(&mut out, at(i));
                    i += 1;
                }
                continue;
            }
            while i < s.next {
                let t = at(i);
                i += 1;
                match t {
                    0x80..=0xff => match names.get(usize::from(t & 0x7f)) {
                        Some(name) => {
                            word(&mut out, name);
                            // The parenthesis is in the name.
                            if name.last() == Some(&b'(') {
                                i += 1;
                            }
                        }
                        None => out.push_str(&format!("{{${t:02x}}}")),
                    },
                    T_NUMBER => {
                        out.push_str(&fasc(std::array::from_fn(|n| at(i + n as u8))));
                        i += 6;
                    }
                    T_STRING => {
                        let len = at(i);
                        out.push('"');
                        for n in 0..len {
                            push_char(&mut out, at(i + 1 + n));
                        }
                        out.push('"');
                        i = i.saturating_add(len).saturating_add(1);
                    }
                    T_EOL => break,
                    0x10..=0x7f => match self.rom.operators.get(usize::from(t - 0x10)) {
                        Some(name) => {
                            let spaced = t < T_FUNCTIONS
                                && name.first().is_some_and(u8::is_ascii_alphabetic);
                            if spaced {
                                out.push(' ');
                            }
                            // The array's parentheses have empty names.
                            for &c in name.iter().filter(|c| **c != 0) {
                                push_char(&mut out, c);
                            }
                            if spaced {
                                out.push(' ');
                            }
                        }
                        None => out.push_str(&format!("{{${t:02x}}}")),
                    },
                    _ => out.push_str(&format!("{{${t:02x}}}")),
                }
            }
        }
        if statements.is_empty() {
            statements.push(0);
        }
        (out, statements)
    }

    /// The place of the statement at offset `start` of line `number`.
    fn pos(&self, lines: &[Line], number: u16, start: u8) -> BasicPos {
        let statement = lines
            .iter()
            .find(|l| l.number == number)
            .and_then(|l| self.stmts(l).iter().position(|s| s.start == start))
            .unwrap_or(0);
        BasicPos {
            line: u32::from(number),
            statement: statement as u16,
        }
    }

    fn running(&self) -> bool {
        // A return address into the loop, or an interrupt's out of it.
        let stop = self.rom.stop;
        (stop - LOOP_BEFORE..=stop + LOOP_AFTER).contains(&self.w(CPU_STACK))
    }

    fn state(&self) -> BasicState {
        let cur = self.w(STMCUR);
        let line = self.line_at(cur);
        let pos = (self.running() && line.number < 0x8000).then(|| {
            // The statement whose successor NXTSTD names; none while a
            // jump is under way (then the line's first).
            let next = self.b(NXTSTD);
            let statement = self.stmts(&line).iter().position(|s| s.next == next);
            BasicPos {
                line: u32::from(line.number),
                statement: statement.unwrap_or(0) as u16,
            }
        });
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos,
            text_addr: Some(u32::from(cur) + u32::from(self.b(STINDEX))),
            depth: self
                .frames()
                .iter()
                .filter(|f| matches!(f, BasicFrame::Gosub { .. }))
                .count() as u32,
        }
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let base = u32::from(self.w(RUNSTK));
        let mut top = u32::from(self.w(STACK_TOP));
        let lines = self.lines();
        let names = self.names();
        let mut out = Vec::new();
        while top >= base + 4 && out.len() < MAX_FRAMES {
            let head = (top - 4) as u16;
            let kind = self.b(head);
            let from = self.pos(
                &lines,
                self.w(head.wrapping_add(1)),
                self.b(head.wrapping_add(3)).wrapping_sub(1),
            );
            if kind == 0 {
                out.push(BasicFrame::Gosub { from });
                top -= 4;
                continue;
            }
            if top < base + 16 {
                break;
            }
            top -= 16;
            let name = names.get(usize::from(kind & 0x7f));
            out.push(BasicFrame::For {
                var: name.map_or_else(String::new, |n| n.iter().map(|&c| char::from(c)).collect()),
                limit: BasicValue::Float(number(self.six(top as u16))),
                step: BasicValue::Float(number(self.six(top as u16 + 6))),
                from,
            });
        }
        out
    }

    fn vars(&self) -> Vec<Var> {
        let table = self.w(VVTP);
        self.names()
            .iter()
            .enumerate()
            .map(|(i, raw)| {
                let addr = table.wrapping_add(8 * i as u16);
                let kind = self.b(addr);
                let name: String = raw
                    .iter()
                    .filter(|c| **c != b'(')
                    .map(|&c| char::from(c))
                    .collect();
                Var {
                    name,
                    kind: match kind & 0xc0 {
                        0x80 => Kind::Str,
                        0x40 => Kind::Array,
                        _ => Kind::Num,
                    },
                    addr,
                    dimensioned: kind & 1 != 0,
                }
            })
            .collect()
    }

    /// Where a string's or an array's data is.
    fn data(&self, var: &Var) -> u16 {
        self.w(STARP).wrapping_add(self.w(var.addr.wrapping_add(2)))
    }

    /// An array's sizes. With one dimension the second is stored as 1,
    /// as for `DIM A(n,0)`; both are reported with one.
    fn dims(&self, var: &Var) -> Vec<u32> {
        if !var.dimensioned {
            return vec![0];
        }
        let first = u32::from(self.w(var.addr.wrapping_add(4)));
        match u32::from(self.w(var.addr.wrapping_add(6))) {
            0 | 1 => vec![first],
            second => vec![first, second],
        }
    }

    fn string(&self, var: &Var) -> String {
        let mut s = String::new();
        if var.dimensioned {
            let data = self.data(var);
            for i in 0..self.w(var.addr.wrapping_add(4)) {
                push_char(&mut s, self.b(data.wrapping_add(i)));
            }
        }
        s
    }
}

/// A name as the debugger's user writes it: without blanks, in upper case.
fn wanted(name: &str) -> String {
    name.chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

impl AtariBasic {
    /// `space` is the CPU's view.
    pub fn new(space: MemSpaceId) -> Self {
        Self { space, rom: None }
    }

    /// Look at the machine again: `Some` with the decoder to keep from now
    /// on if the interpreter appeared or went away.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.rom.as_ref().is_some_and(|r| self.mapped(host, r)) {
            return None;
        }
        let mem: Vec<u8> = (0..ROM_LEN as u32)
            .map(|a| host.peek(self.space, u32::from(ROM_START) + a, Size::Byte))
            .map(|v| v.map_or(0xff, |v| v as u8))
            .collect();
        let rom = scan(&mem);
        (rom != self.rom).then(|| Self {
            rom,
            ..self.clone()
        })
    }

    /// The ROM is where it was found (the XL can switch it off).
    fn mapped(&self, host: &dyn DebugTarget, rom: &Rom) -> bool {
        let first = u32::from(rom.stop) + 1 - LOOP.len() as u32;
        LOOP.iter().enumerate().all(|(i, &b)| {
            host.peek(self.space, first + i as u32, Size::Byte) == Some(u32::from(b))
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        self.mapped(host, rom).then_some(View {
            host,
            space: self.space,
            rom,
        })
    }
}

impl Dialect for AtariBasic {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "Atari BASIC".into(),
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
        let names = v.names();
        v.lines()
            .iter()
            .map(|l| {
                let (text, statements) = v.text(l, &names);
                BasicLine {
                    number: u32::from(l.number),
                    addr: u32::from(l.addr),
                    raw: (0..u16::from(l.len))
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
        v.vars()
            .iter()
            .map(|var| BasicVar {
                name: var.name.clone(),
                ty: match var.kind {
                    Kind::Str => BasicType::Str,
                    _ => BasicType::Float,
                },
                addr: u32::from(var.addr),
                value: match var.kind {
                    Kind::Num => Some(BasicValue::Float(number(v.six(var.addr.wrapping_add(2))))),
                    Kind::Str => Some(BasicValue::Str(v.string(var))),
                    Kind::Array => None,
                },
                dims: match var.kind {
                    Kind::Array => v.dims(var),
                    _ => Vec::new(),
                },
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
            .find(|x| x.kind == Kind::Array && x.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let len = v.dims(var).iter().product::<u32>();
        if first > len {
            return Err(BasicError::OutOfRange);
        }
        let data = v.data(var);
        Ok((first..len.min(first.saturating_add(count)))
            .map(|i| BasicValue::Float(number(v.six(data.wrapping_add((6 * i) as u16)))))
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
            .find(|x| x.name == name && (x.kind == Kind::Array) == index.is_some())
            .ok_or(BasicError::NoSuchVariable)?;
        let (at, bytes): (u16, Vec<u8>) = match (var.kind, value) {
            (Kind::Num | Kind::Array, BasicValue::Int(_) | BasicValue::Float(_)) => {
                let n = match value {
                    BasicValue::Int(n) => *n as f64,
                    BasicValue::Float(f) => *f,
                    BasicValue::Str(_) => unreachable!(),
                };
                let at = match index {
                    None => var.addr.wrapping_add(2),
                    Some(i) if i < v.dims(var).iter().product::<u32>() => {
                        v.data(var).wrapping_add((6 * i) as u16)
                    }
                    Some(_) => return Err(BasicError::OutOfRange),
                };
                (at, number_bytes(n)?.to_vec())
            }
            (Kind::Str, BasicValue::Str(s)) => {
                let text = s
                    .chars()
                    .map(atascii_code)
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // In place, up to the dimensioned length.
                let room = usize::from(v.w(var.addr.wrapping_add(6)));
                if !var.dimensioned || text.len() > room {
                    return Err(BasicError::Unsupported);
                }
                let len = (text.len() as u16).to_le_bytes();
                let mut writes: Vec<(u32, u8)> = text
                    .into_iter()
                    .enumerate()
                    .map(|(i, b)| (u32::from(v.data(var)) + i as u32, b))
                    .collect();
                writes.push((u32::from(var.addr) + 4, len[0]));
                writes.push((u32::from(var.addr) + 5, len[1]));
                return Ok(writes);
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (u32::from(at) + i as u32, b))
            .collect())
    }

    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        self.view(host)
            .map(|v| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: u32::from(v.rom.stop),
                },
                granularity: BasicGranularity::Statement,
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests;
