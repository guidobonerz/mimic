//! BBC BASIC V for the ARM: the BASIC module of RISC OS on the Archimedes
//! (read from version 1.05 in RISC OS 3.11).
//!
//! **Program.** As on the 6502: a line is `&0D`, its number (high byte
//! first), its length and its text; `&0D &FF` ends the program. PAGE and
//! TOP are words of the workspace the register R8 points to (ARGP,
//! `&8700`): PAGE at R8 − `&94`, TOP − `&90`, the variables' end − `&8C`,
//! LOMEM − `&88`, HIMEM − `&84`. Keywords are tokens from `&7F`; `&C6`,
//! `&C7` and `&C8` begin a token of two bytes (functions, commands,
//! statements), `&8D` a line number in three bytes. The listing is LIST's
//! with `LISTO 0`: the text as stored, the tokens written out with the
//! ROM's own table (word, token, a byte of flags that tells the two-byte
//! tokens and their prefix), nothing expanded inside quotes.
//!
//! **Where the interpreter is.** The text pointer is the register R12
//! while statements run (R11 inside expressions). Every statement begins
//! with `LDRB R10,[R12],#1 : LDR R4,[PC,R10,LSL #2] : ADD PC,PC,R4`, a
//! jump through a table by the statement's first byte; the second
//! instruction is the stop point, where R10 is that byte and R12 points
//! behind it. Blanks, colons and a line's end pass the same jump (the
//! table sends them back): at the stop point with one of these in R10 the
//! interpreter is between two statements, which is reported as running
//! without a place, so the debugger does not count it. The line is the
//! one the pointer lies in.
//!
//! **Statements.** A colon begins one; so do THEN, ELSE and REPEAT unless
//! a line number, a colon or the line's end follows. REM, DATA and a `*`
//! command take the rest of the line.
//!
//! **Stack.** One stack (R13, growing down from HIMEM) for everything; a
//! frame begins with a word that names it: `&F8` GOSUB (then the text
//! pointer behind the statement's delimiter), `&FD` REPEAT (the pointer
//! behind the word), `&CE` WHILE (the loop's body, the condition), `&ED`
//! a FOR with an integer variable (the variable's address, the text
//! pointer behind the FOR statement's delimiter, step, limit), `&E3` a FOR
//! with a real variable (the same with step and limit as a mantissa word
//! and a word of sign and exponent each), `&F2` PROC and `&A4` FN (the
//! caller's two text pointers, then the variables saved for parameters
//! and LOCAL). Values of an expression under way and saved variables lie
//! between the frames without a mark, so the frames are found by their
//! shape: the word and, where the frame has them, pointers into the
//! program and to a variable. A call is reported as a GOSUB frame and
//! counts for the depth.
//!
//! **Variables.** `@%` and `A%` to `Z%` are words from R8 − `&100` (listed
//! while they are not 0). All others hang in chains by their first
//! character, the heads from R8 − `&200`, four bytes for each character
//! from `@`: the next entry's address, the rest of the name with its `%`,
//! `$` or `(`, a zero byte and the value: an integer (four bytes, in the
//! next aligned word), a real (unaligned: four bytes of mantissa, low first, the sign in its top
//! bit, and the exponent with 128 added), a string (address and length).
//! An array's value is the address of its block, in the next aligned
//! word: the sizes as words, a word 0, the number of elements, the elements with the last index
//! running fastest.
//!
//! Sources: the *BBC BASIC Reference Manual* (Acorn, 1992) for the
//! language and the token values. The statement loop (`&38B204C`) with
//! its table, the line's end (`&38B1E44`), GOSUB/RETURN (`&38B7888`,
//! `&38B8430`), FOR (`&38B77C0`), REPEAT/UNTIL (`&38B8384`, `&38B88AC`),
//! ENDWHILE (`&38B7724`), the stack unwinder (`&38B845C`), PROC/FN and
//! ENDPROC (`&38B6C6C`, `&38B76EC`, `&38B6FD8`) and the keyword table
//! (`&38BC690`) were read from the user's RISC OS 3.11 ROM with this
//! project's ARM disassembler; the workspace, the variables and the
//! frames from the memory of a running program (`archimedes` test). No
//! emulator source was used.

use mimic_core::basic::{
    BasicError, BasicFrame, BasicGranularity, BasicInfo, BasicLine, BasicMode, BasicPos,
    BasicState, BasicStopPoint, BasicType, BasicValue, BasicVar,
};
use mimic_core::{BreakpointKind, DebugTarget, MemSpaceId, Size};

use crate::Dialect;

const MAX_LINES: usize = 20_000;
const MAX_VARS: usize = 4_000;
const MAX_FRAMES: usize = 256;
/// Words of the stack looked at for frames.
const MAX_STACK: u32 = 65_536;

const ROM_START: u32 = 0x0380_0000;
const ROM_LEN: u32 = 0x0020_0000;

/// Offsets below ARGP (R8).
const PAGE: u32 = 0x94;
const TOP: u32 = 0x90;
const VARS_END: u32 = 0x8c;
const LOMEM: u32 = 0x88;
const HIMEM: u32 = 0x84;
const RESIDENT: u32 = 0x100;
const HEADS: u32 = 0x200;

/// LDRB R10,[R12],#1 : LDR R4,[PC,R10,LSL #2] : ADD PC,PC,R4.
const LOOP: [u32; 3] = [0xe4dc_a001, 0xe79f_410a, 0xe08f_f004];

const T_LINE: u8 = 0x8d;
const T_ELSE: u8 = 0x8b;
const T_THEN: u8 = 0x8c;
const T_ELSE2: u8 = 0xcc;
const T_DATA: u8 = 0xdc;
const T_REM: u8 = 0xf4;
const T_REPEAT: u8 = 0xf5;
const F_GOSUB: u32 = 0xf8;
const F_REPEAT: u32 = 0xfd;
const F_WHILE: u32 = 0xce;
const F_FOR_INT: u32 = 0xed;
const F_FOR_REAL: u32 = 0xe3;
const F_PROC: u32 = 0xf2;
const F_FN: u32 = 0xa4;

/// What was found in the ROM.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rom {
    /// The stop point: the table jump's load.
    stop: u32,
    /// Words of the one-byte tokens, by token.
    keywords: Vec<Vec<u8>>,
    /// Words of the two-byte tokens: prefix, second byte, word.
    long: Vec<Long>,
}

/// The decoder an Archimedes keeps; see the module text.
#[derive(Clone, Debug)]
pub struct BbcBasicV {
    space: MemSpaceId,
    rom: Option<Rom>,
    /// The ROM was looked at (it does not change).
    scanned: bool,
}

/// A token of two bytes: prefix, second byte, word.
type Long = (u8, u8, Vec<u8>);

/// The keyword table from `AND`: word, token, flags.
fn keywords(mem: &[u8]) -> Option<(Vec<Vec<u8>>, Vec<Long>)> {
    let mut at = mem.windows(4).position(|w| w == b"AND\x80")?;
    let mut short = vec![Vec::new(); 256];
    let mut long = Vec::new();
    loop {
        let len = mem[at..]
            .iter()
            .take(12)
            .position(|b| !matches!(b, b'A'..=b'Z' | b'$' | b'('))?;
        let (token, flags) = (*mem.get(at + len)?, *mem.get(at + len + 1)?);
        if len == 0 || token < 0x7f {
            break;
        }
        let word = mem[at..at + len].to_vec();
        if flags & 0x08 != 0 {
            // A token of two bytes: a statement, a function or a command.
            let prefix = match flags {
                f if f & 0x40 != 0 => 0xc8,
                f if f & 0x04 != 0 => 0xc6,
                _ => 0xc7,
            };
            long.push((prefix, token, word));
        } else if short[usize::from(token)].is_empty() {
            // The statement forms of PAGE, PTR, TIME, LOMEM and HIMEM
            // have the token + &40.
            if flags & 0x40 != 0 {
                short[usize::from(token) + 0x40] = word.clone();
            }
            short[usize::from(token)] = word;
        }
        at += len + 2;
    }
    Some((short, long))
}

fn bbc_char(b: u8) -> Option<char> {
    match b {
        0x60 => Some('£'),
        0x20..=0x7e => Some(char::from(b)),
        _ => None,
    }
}

fn bbc_code(c: char) -> Option<u8> {
    match c {
        '£' => Some(0x60),
        '`' => None,
        _ => u8::try_from(c).ok().filter(|b| (0x20..=0x7e).contains(b)),
    }
}

fn push_char(out: &mut String, b: u8) {
    match bbc_char(b) {
        Some(c) => out.push(c),
        None => out.push_str(&format!("{{${b:02x}}}")),
    }
}

/// A real from its mantissa (the top bit set), its sign and its exponent
/// with 128 added.
fn real(mantissa: u32, negative: bool, exponent: u8) -> f64 {
    if exponent == 0 && mantissa & 0x7fff_ffff == 0 {
        return 0.0;
    }
    let v =
        f64::from(mantissa | 0x8000_0000) / 4_294_967_296.0 * 2f64.powi(i32::from(exponent) - 128);
    if negative {
        -v
    } else {
        v
    }
}

/// A stored real: the mantissa's four bytes, low first, the sign in place
/// of its top bit; the exponent.
fn real_bytes(v: f64) -> Result<[u8; 5], BasicError> {
    if !v.is_finite() {
        return Err(BasicError::OutOfRange);
    }
    if v == 0.0 {
        return Ok([0; 5]);
    }
    let mut e = v.abs().log2().floor() as i32 + 1;
    let mut m = (v.abs() / 2f64.powi(e) * 4_294_967_296.0).round();
    if m >= 4_294_967_296.0 {
        m /= 2.0;
        e += 1;
    }
    if m < 2_147_483_648.0 {
        m *= 2.0;
        e -= 1;
    }
    if e > 127 {
        return Err(BasicError::OutOfRange);
    }
    if e < -127 {
        return Ok([0; 5]);
    }
    let sign = if v < 0.0 { 0x8000_0000 } else { 0 };
    let m = (m as u32 & 0x7fff_ffff | sign).to_le_bytes();
    Ok([m[0], m[1], m[2], m[3], (e + 128) as u8])
}

#[derive(Clone, Copy, Debug)]
struct Line {
    addr: u32,
    number: u16,
    len: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Real,
    Str,
}

impl Kind {
    fn size(self) -> u32 {
        match self {
            Kind::Int => 4,
            Kind::Real | Kind::Str => 5,
        }
    }

    fn ty(self) -> BasicType {
        match self {
            Kind::Int => BasicType::Int,
            Kind::Real => BasicType::Float,
            Kind::Str => BasicType::Str,
        }
    }
}

struct Var {
    name: String,
    kind: Kind,
    array: bool,
    entry: u32,
    /// The value; for an array the word that holds its block's address.
    value: u32,
}

struct Text {
    text: String,
    offsets: Vec<u32>,
    starts: Vec<u32>,
}

struct View<'a> {
    host: &'a dyn DebugTarget,
    space: MemSpaceId,
    rom: &'a Rom,
    /// ARGP.
    r8: u32,
}

impl View<'_> {
    fn b(&self, addr: u32) -> u8 {
        self.host
            .peek(self.space, addr, Size::Byte)
            .map_or(0xff, |v| v as u8)
    }

    fn w(&self, addr: u32) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|i| self.b(addr.wrapping_add(i as u32))))
    }

    fn reg(&self, name: &str) -> u32 {
        self.host.regs().get(name).unwrap_or(0)
    }

    fn sys(&self, offset: u32) -> u32 {
        self.w(self.r8.wrapping_sub(offset))
    }

    /// The workspace is BASIC's: its pointers lie in order and the
    /// program begins with a line.
    fn active(&self) -> bool {
        let (page, top) = (self.sys(PAGE), self.sys(TOP));
        let (lomem, end, himem) = (self.sys(LOMEM), self.sys(VARS_END), self.sys(HIMEM));
        self.r8 >= 0x8200
            && self.r8 < 0x0100_0000
            && page >= 0x8000
            && page < top
            && top <= lomem
            && lomem <= end
            && end <= himem
            && himem < 0x0200_0000
            && self.b(page) == 0x0d
    }

    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        let (mut at, top) = (self.sys(PAGE), self.sys(TOP));
        while at + 4 < top && out.len() < MAX_LINES {
            let high = self.b(at + 1);
            let len = self.b(at + 3);
            if self.b(at) != 0x0d || high >= 0x80 || len < 4 {
                break;
            }
            out.push(Line {
                addr: at,
                number: u16::from(high) << 8 | u16::from(self.b(at + 2)),
                len,
            });
            at += u32::from(len);
        }
        out
    }

    fn text(&self, line: &Line) -> Text {
        let end = line.addr + u32::from(line.len);
        let mut t = Text {
            text: String::new(),
            offsets: vec![0],
            starts: vec![line.addr + 4],
        };
        let mut at = line.addr + 4;
        let mut quoted = false;
        let mut rest = false;
        let mut first = true;
        while at < end {
            let b = self.b(at);
            at += 1;
            let begins = first && b != b' ';
            if b != b' ' {
                first = false;
            }
            if rest || quoted {
                quoted &= b != b'"';
                push_char(&mut t.text, b);
                continue;
            }
            let begin = |t: &mut Text, at: u32| {
                t.offsets.push(t.text.len() as u32);
                t.starts.push(at);
            };
            match b {
                b'"' => {
                    quoted = true;
                    t.text.push('"');
                }
                b':' => {
                    t.text.push(':');
                    begin(&mut t, at);
                    first = true;
                }
                b'*' if begins => {
                    rest = true;
                    t.text.push('*');
                }
                T_LINE => {
                    let (a, b, c) = (self.b(at), self.b(at + 1), self.b(at + 2));
                    at += 3;
                    let low = (a << 2 & 0xc0) ^ b;
                    let high = (a << 4 & 0xc0) ^ c;
                    t.text
                        .push_str(&(u16::from(high) << 8 | u16::from(low)).to_string());
                }
                0xc6..=0xc8 => {
                    let second = self.b(at);
                    at += 1;
                    let word = self.rom.long.iter().find(|l| l.0 == b && l.1 == second);
                    match word {
                        Some((_, _, word)) => {
                            word.iter().for_each(|&c| t.text.push(char::from(c)));
                        }
                        None => t.text.push_str(&format!("{{${b:02x}}}{{${second:02x}}}")),
                    }
                }
                0x7f..=0xff => {
                    match self.rom.keywords.get(usize::from(b)) {
                        Some(word) if !word.is_empty() => {
                            word.iter().for_each(|&c| t.text.push(char::from(c)));
                        }
                        _ => t.text.push_str(&format!("{{${b:02x}}}")),
                    }
                    match b {
                        T_REM | T_DATA => rest = true,
                        T_REPEAT | T_THEN | T_ELSE | T_ELSE2 => {
                            // A statement follows, unless it is a line
                            // number, a colon (which begins one itself)
                            // or the line's end.
                            let mut next = at;
                            while next < end && self.b(next) == b' ' {
                                next += 1;
                            }
                            if next < end && !matches!(self.b(next), T_LINE | b':') {
                                begin(&mut t, at);
                                first = true;
                            }
                        }
                        _ => {}
                    }
                }
                _ => push_char(&mut t.text, b),
            }
        }
        t
    }

    /// The line and statement the text at `p` belongs to: a line owns the
    /// bytes behind its `&0D` up to the next line's, a statement its
    /// delimiter.
    fn place(&self, lines: &[Line], p: u32) -> Option<BasicPos> {
        let line = lines
            .iter()
            .find(|l| p > l.addr && p <= l.addr + u32::from(l.len))?;
        let starts = self.text(line).starts;
        let statement = starts.iter().filter(|s| **s <= p).count().max(1) - 1;
        Some(BasicPos {
            line: u32::from(line.number),
            statement: statement as u16,
        })
    }

    fn in_program(&self, p: u32) -> bool {
        p > self.sys(PAGE) && p <= self.sys(TOP)
    }

    /// A variable's value lies there: a resident integer, or between
    /// LOMEM and the variables' end.
    fn is_variable(&self, p: u32) -> bool {
        let resident = self.r8 - RESIDENT;
        (resident..resident + 4 * 27).contains(&p)
            || (self.sys(LOMEM)..self.sys(VARS_END)).contains(&p)
    }

    /// The frames on the stack with their addresses, innermost first.
    fn raw_frames(&self) -> Vec<(u32, u32)> {
        let himem = self.sys(HIMEM);
        let mut at = self.reg("R13").wrapping_add(3) & !3;
        let mut out = Vec::new();
        if at >= himem || himem - at > 4 * MAX_STACK {
            return out;
        }
        while at + 8 <= himem && out.len() < MAX_FRAMES {
            let mark = self.w(at);
            let size = match mark {
                F_GOSUB | F_REPEAT if self.in_program(self.w(at + 4)) => 8,
                F_WHILE if self.in_program(self.w(at + 4)) && self.in_program(self.w(at + 8)) => 12,
                F_FOR_INT | F_FOR_REAL
                    if self.is_variable(self.w(at + 4)) && self.in_program(self.w(at + 8)) =>
                {
                    if mark == F_FOR_INT {
                        20
                    } else {
                        28
                    }
                }
                // The caller's two text pointers: in the program, or in
                // the buffer of a typed command for the outermost call.
                F_PROC | F_FN if self.in_program(self.w(at + 8)) || out.is_empty() => 12,
                _ => 0,
            };
            if size == 0 {
                at += 4;
            } else {
                out.push((mark, at));
                at += size;
            }
        }
        out
    }

    fn pos_or_none(&self, lines: &[Line], p: u32) -> BasicPos {
        self.place(lines, p).unwrap_or(BasicPos {
            line: 0,
            statement: 0,
        })
    }

    fn frames(&self) -> Vec<BasicFrame> {
        let lines = self.lines();
        let vars = self.vars();
        let name = |addr: u32| {
            let resident = self.r8 - RESIDENT;
            if (resident..resident + 4 * 27).contains(&addr) {
                return format!("{}%", char::from(b'@' + ((addr - resident) / 4) as u8));
            }
            vars.iter()
                .find(|v| !v.array && v.value == addr)
                .map_or_else(|| format!("&{addr:X}"), |v| v.name.clone())
        };
        self.raw_frames()
            .into_iter()
            .map(|(mark, at)| match mark {
                F_REPEAT => {
                    // The pointer is behind the word and the blanks and
                    // colons that follow it: back to the word.
                    let behind = self.w(at + 4) - 1;
                    let word = lines
                        .iter()
                        .find(|l| behind > l.addr && behind <= l.addr + u32::from(l.len))
                        .and_then(|l| (l.addr + 4..=behind).rev().find(|a| self.b(*a) == T_REPEAT));
                    BasicFrame::Loop {
                        word: "REPEAT".into(),
                        from: self.pos_or_none(&lines, word.unwrap_or(behind)),
                    }
                }
                F_WHILE => BasicFrame::Loop {
                    word: "WHILE".into(),
                    from: self.pos_or_none(&lines, self.w(at + 8) - 1),
                },
                F_FOR_INT => BasicFrame::For {
                    var: name(self.w(at + 4)),
                    limit: BasicValue::Int(i64::from(self.w(at + 16) as i32)),
                    step: BasicValue::Int(i64::from(self.w(at + 12) as i32)),
                    from: self.pos_or_none(&lines, self.w(at + 8) - 1),
                },
                F_FOR_REAL => {
                    let value = |at: u32| {
                        let sign = self.w(at + 4);
                        BasicValue::Float(real(self.w(at), sign >> 31 != 0, sign as u8))
                    };
                    BasicFrame::For {
                        var: name(self.w(at + 4)),
                        limit: value(at + 20),
                        step: value(at + 12),
                        from: self.pos_or_none(&lines, self.w(at + 8) - 1),
                    }
                }
                // GOSUB: behind the statement's delimiter; a call: the
                // caller's statement pointer.
                F_GOSUB => BasicFrame::Gosub {
                    from: self.pos_or_none(&lines, self.w(at + 4) - 1),
                },
                _ => BasicFrame::Gosub {
                    from: self.pos_or_none(&lines, self.w(at + 8) - 1),
                },
            })
            .collect()
    }

    fn state(&self) -> BasicState {
        let lines = self.lines();
        let r12 = self.reg("R12");
        let p = r12.wrapping_sub(1);
        let pos = self.place(&lines, p);
        // At the stop point with a blank, a colon or a line's end: between
        // two statements.
        let at_stop = self.reg("PC") & 0x03ff_fffc == self.rom.stop;
        let between = at_stop && matches!(self.reg("R10") as u8, b' ' | b':' | 0x0d);
        BasicState {
            mode: if pos.is_some() {
                BasicMode::Running
            } else {
                BasicMode::Direct
            },
            pos: pos.filter(|_| !between),
            text_addr: Some(p),
            depth: self
                .raw_frames()
                .iter()
                .filter(|f| matches!(f.0, F_GOSUB | F_PROC | F_FN))
                .count() as u32,
        }
    }

    fn vars(&self) -> Vec<Var> {
        let mut out = Vec::new();
        for c in b'@'..=b'Z' {
            let at = self.r8 - RESIDENT + 4 * u32::from(c - b'@');
            if self.w(at) != 0 {
                out.push(Var {
                    name: format!("{}%", char::from(c)),
                    kind: Kind::Int,
                    array: false,
                    entry: at,
                    value: at,
                });
            }
        }
        let (low, high) = (self.sys(LOMEM), self.sys(VARS_END));
        for c in b'A'..=b'z' {
            let mut entry = self.w(self.r8 - HEADS + 4 * u32::from(c - b'@'));
            while (low..high).contains(&entry) && out.len() < MAX_VARS {
                let mut name = vec![c];
                let mut at = entry + 4;
                while self.b(at) != 0 && name.len() < 64 {
                    name.push(self.b(at));
                    at += 1;
                }
                let array = name.last() == Some(&b'(');
                if array {
                    name.pop();
                }
                let kind = match name.last() {
                    Some(b'%') => Kind::Int,
                    Some(b'$') => Kind::Str,
                    _ => Kind::Real,
                };
                let mut text = String::new();
                name.iter().for_each(|&b| push_char(&mut text, b));
                // Words are aligned (an integer, an array's address),
                // the five bytes of a real or a string are not.
                let value = if array || kind == Kind::Int {
                    (at + 4) & !3
                } else {
                    at + 1
                };
                out.push(Var {
                    name: text,
                    kind,
                    array,
                    entry,
                    value,
                });
                entry = self.w(entry);
            }
        }
        out
    }

    /// An array's sizes and the address of its first element.
    fn array(&self, var: &Var) -> (Vec<u32>, u32) {
        let mut at = self.w(var.value);
        let mut dims = Vec::new();
        if at < 0x8000 {
            return (dims, 0);
        }
        while self.w(at) != 0 && dims.len() < 16 {
            dims.push(self.w(at));
            at += 4;
        }
        (dims, at + 8)
    }

    fn value(&self, kind: Kind, at: u32) -> BasicValue {
        match kind {
            Kind::Int => BasicValue::Int(i64::from(self.w(at) as i32)),
            Kind::Real => {
                let m = self.w(at);
                BasicValue::Float(real(m, m >> 31 != 0, self.b(at + 4)))
            }
            Kind::Str => {
                let text = self.w(at);
                let mut s = String::new();
                for i in 0..u32::from(self.b(at + 4)) {
                    push_char(&mut s, self.b(text + i));
                }
                BasicValue::Str(s)
            }
        }
    }
}

/// A name as the debugger's user writes it: without blanks. BBC BASIC
/// tells upper and lower case apart.
fn wanted(name: &str) -> String {
    name.chars().filter(|c| *c != ' ').collect()
}

impl BbcBasicV {
    /// `space` is the CPU's view.
    pub fn new(space: MemSpaceId) -> Self {
        Self {
            space,
            rom: None,
            scanned: false,
        }
    }

    /// Look at the ROM once: `Some` with the decoder to keep.
    pub fn rescan(&self, host: &dyn DebugTarget) -> Option<Self> {
        if self.scanned {
            return None;
        }
        let word = |a: u32| host.peek(self.space, a, Size::Long).unwrap_or(0);
        let stop = (ROM_START..ROM_START + ROM_LEN - 12)
            .step_by(4)
            .find(|&a| word(a) == LOOP[0] && word(a + 4) == LOOP[1] && word(a + 8) == LOOP[2])
            .map(|a| a + 4);
        let rom = stop.and_then(|stop| {
            // The table lies in the module, behind the interpreter.
            let mem: Vec<u8> = (stop..(stop + 0x1_0000).min(ROM_START + ROM_LEN))
                .map(|a| host.peek(self.space, a, Size::Byte).unwrap_or(0) as u8)
                .collect();
            let (keywords, long) = keywords(&mem)?;
            Some(Rom {
                stop,
                keywords,
                long,
            })
        });
        Some(Self {
            space: self.space,
            rom,
            scanned: true,
        })
    }

    fn view<'a>(&'a self, host: &'a dyn DebugTarget) -> Option<View<'a>> {
        let rom = self.rom.as_ref()?;
        let v = View {
            host,
            space: self.space,
            rom,
            r8: host.regs().get("R8").unwrap_or(0),
        };
        v.active().then_some(v)
    }
}

impl Dialect for BbcBasicV {
    fn space(&self) -> MemSpaceId {
        self.space
    }

    fn info(&self) -> BasicInfo {
        BasicInfo {
            dialect: "BBC BASIC V".into(),
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
                let t = v.text(l);
                BasicLine {
                    number: u32::from(l.number),
                    addr: l.addr,
                    raw: (0..u32::from(l.len)).map(|i| v.b(l.addr + i)).collect(),
                    text: t.text,
                    statements: t.offsets,
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
                ty: var.kind.ty(),
                addr: var.entry,
                value: (!var.array).then(|| v.value(var.kind, var.value)),
                dims: if var.array {
                    v.array(var).0
                } else {
                    Vec::new()
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
            .find(|x| x.array && x.name == name)
            .ok_or(BasicError::NoSuchVariable)?;
        let (dims, data) = v.array(var);
        let len = if dims.is_empty() {
            0
        } else {
            dims.iter().product::<u32>()
        };
        if first > len {
            return Err(BasicError::OutOfRange);
        }
        Ok((first..len.min(first.saturating_add(count)))
            .map(|i| v.value(var.kind, data + i * var.kind.size()))
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
        let mut vars = v.vars();
        // A resident integer that is 0 is not listed, but it is there.
        if let [c @ b'@'..=b'Z', b'%'] = name.as_bytes() {
            let at = v.r8 - RESIDENT + 4 * u32::from(c - b'@');
            vars.push(Var {
                name: name.clone(),
                kind: Kind::Int,
                array: false,
                entry: at,
                value: at,
            });
        }
        let var = vars
            .iter()
            .find(|x| x.name == name && x.array == index.is_some())
            .ok_or(BasicError::NoSuchVariable)?;
        let at = match index {
            None => var.value,
            Some(i) => {
                let (dims, data) = v.array(var);
                if dims.is_empty() || i >= dims.iter().product::<u32>() {
                    return Err(BasicError::OutOfRange);
                }
                data + i * var.kind.size()
            }
        };
        let number = match value {
            BasicValue::Int(n) => Some(*n as f64),
            BasicValue::Float(f) => Some(*f),
            BasicValue::Str(_) => None,
        };
        let (at, bytes): (u32, Vec<u8>) = match (var.kind, value, number) {
            (Kind::Int, _, Some(n)) => {
                if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {
                    return Err(BasicError::OutOfRange);
                }
                (at, (n as i32).to_le_bytes().to_vec())
            }
            (Kind::Real, _, Some(n)) => (at, real_bytes(n)?.to_vec()),
            (Kind::Str, BasicValue::Str(s), _) => {
                let text = s
                    .chars()
                    .map(bbc_code)
                    .collect::<Option<Vec<u8>>>()
                    .ok_or(BasicError::OutOfRange)?;
                // In place: the same length.
                if text.len() != usize::from(v.b(at + 4)) {
                    return Err(BasicError::Unsupported);
                }
                (v.w(at), text)
            }
            _ => return Err(BasicError::TypeMismatch),
        };
        Ok(bytes
            .into_iter()
            .enumerate()
            .map(|(i, b)| (at + i as u32, b))
            .collect())
    }

    fn stop_points(&self, host: &dyn DebugTarget) -> Vec<BasicStopPoint> {
        let _ = host;
        self.rom
            .iter()
            .map(|rom| BasicStopPoint {
                kind: BreakpointKind::Exec {
                    space: self.space,
                    addr: rom.stop,
                },
                granularity: BasicGranularity::Statement,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
